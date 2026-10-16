use std::collections::HashSet;

use dotty_core::ast::{AstArena, DefDef, Modifiers, Tree, TreeKind};
use dotty_core::{
    Constant, Definitions, MethodKind, Name, Namespace, SemanticStore, SourceDefinition, SourceId,
    SourceSemanticIndex, SourceSpan, Symbol, SymbolFlags, SymbolId, SymbolInfo, SymbolKind,
    SymbolOrigin, TreeId, Type, TypeId, Typed, Untyped, Visibility,
};

use crate::{Builtin, LimitKind, PickleError, SourceRole, TypeRole, UnsupportedFeature};

/// Owned source provenance supplied by the integration layer. Core stores only
/// SourceId, not the filename or source-text length; those remain producer facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceMetadata {
    pub id: SourceId,
    /// Normalized source-relative path using `/`, without `.`/`..` segments.
    pub name: String,
}

/// Read-only port over the actual frontend source-to-typed index.
///
/// Implementations must delegate to the index from the supplied compilation
/// session and remain consistent during validation. This is producer evidence,
/// not a way to authenticate arbitrary arena-relative numeric IDs.
pub trait SourceTypedBindings {
    fn typed_at(&self, source: SourceId, tree: TreeId<Untyped>) -> Option<TreeId<Typed>>;
}

impl<F> SourceTypedBindings for F
where
    F: Fn(SourceId, TreeId<Untyped>) -> Option<TreeId<Typed>>,
{
    fn typed_at(&self, source: SourceId, tree: TreeId<Untyped>) -> Option<TreeId<Typed>> {
        self(source, tree)
    }
}

/// Exact source declaration for the existing canonical default constructor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefaultConstructorPlan {
    pub symbol: SymbolId,
    pub source_tree: TreeId<Untyped>,
    /// Explicit source clauses, not the normalized semantic empty clause.
    pub parameter_clauses: Vec<Vec<SymbolId>>,
}

/// A canonical method and its real typed RHS, in source declaration order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MethodPicklePlan {
    pub symbol: SymbolId,
    pub source_tree: TreeId<Untyped>,
    /// Preserve clause boundaries: `[]` and `[[]]` are distinct.
    pub parameter_clauses: Vec<Vec<SymbolId>>,
    /// None means a declaration; a concrete source RHS requires its exact root.
    pub body: Option<TreeId<Typed>>,
}

/// Borrowed, unvalidated input. No semantic records or typed trees are cloned.
/// The explicit source root prevents silently omitting extra top-level forms.
pub struct PicklePlan<'a> {
    pub store: &'a SemanticStore,
    pub definitions: &'a Definitions,
    pub typed_ast: &'a AstArena<Typed>,
    pub source_ast: &'a AstArena<Untyped>,
    pub source_index: &'a SourceSemanticIndex,
    pub typed_bindings: &'a dyn SourceTypedBindings,
    pub source: SourceMetadata,
    pub source_root: TreeId<Untyped>,
    pub class: SymbolId,
    pub class_tree: TreeId<Untyped>,
    pub constructor: DefaultConstructorPlan,
    pub methods: &'a [MethodPicklePlan],
}

/// Budgets checked before traversing caller-controlled collections or chains.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValidationLimits {
    pub max_owner_depth: usize,
    pub max_items: usize,
    pub max_name_bytes: usize,
}

impl Default for ValidationLimits {
    fn default() -> Self {
        Self {
            max_owner_depth: 256,
            max_items: 1_000_000,
            max_name_bytes: 4096,
        }
    }
}

/// Immutable checked view tied to the plan and its borrowed compilation session.
/// This type proves the minimal input contract only; no bytes are emitted yet.
pub struct ValidatedPicklePlan<'p, 'a> {
    plan: &'p PicklePlan<'a>,
    package: SymbolId,
}

impl<'p, 'a> ValidatedPicklePlan<'p, 'a> {
    pub fn plan(&self) -> &'p PicklePlan<'a> {
        self.plan
    }
    /// The canonical named package reached through the root class owner chain.
    pub fn package(&self) -> SymbolId {
        self.package
    }
}

impl<'a> PicklePlan<'a> {
    /// Check the frozen class/default-constructor/parameterless Int-literal slice.
    /// Neither success nor failure changes stores, arenas, indices, or files.
    pub fn validate(&self) -> Result<ValidatedPicklePlan<'_, 'a>, PickleError> {
        self.validate_with_limits(ValidationLimits::default())
    }

    pub fn validate_with_limits(
        &self,
        limits: ValidationLimits,
    ) -> Result<ValidatedPicklePlan<'_, 'a>, PickleError> {
        let mut validator = Validator {
            plan: self,
            limits,
            remaining: limits.max_items,
        };
        let package = validator.validate()?;
        Ok(ValidatedPicklePlan {
            plan: self,
            package,
        })
    }
}

struct Validator<'p, 'a> {
    plan: &'p PicklePlan<'a>,
    limits: ValidationLimits,
    remaining: usize,
}

impl<'a> Validator<'_, 'a> {
    fn charge(&mut self, count: usize) -> Result<(), PickleError> {
        self.remaining = self
            .remaining
            .checked_sub(count)
            .ok_or(PickleError::LimitExceeded {
                kind: LimitKind::Items,
                limit: self.limits.max_items,
            })?;
        Ok(())
    }

    fn unsupported(&self, feature: UnsupportedFeature, symbol: Option<SymbolId>) -> PickleError {
        PickleError::Unsupported { feature, symbol }
    }

    fn name(&self, name: Name) -> Result<&'a str, PickleError> {
        let text = self
            .plan
            .store
            .names
            .try_resolve(name.text())
            .ok_or(PickleError::InvalidName { name: name.text() })?;
        if text.len() > self.limits.max_name_bytes {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::NameBytes,
                limit: self.limits.max_name_bytes,
            });
        }
        Ok(text)
    }

    fn span(&self, position: Option<SourceSpan>) -> Result<(), PickleError> {
        if let Some(position) = position
            && position.source() != self.plan.source.id
        {
            return Err(PickleError::SourceSpan {
                expected: self.plan.source.id,
                actual: position.source(),
            });
        }
        Ok(()) // Span/TextRange constructors already guarantee local range invariants.
    }

    fn symbol(&self, symbol: SymbolId) -> Result<&'a Symbol, PickleError> {
        if !self.plan.store.symbols.contains(symbol) {
            return Err(PickleError::InvalidSymbol { symbol });
        }
        let found = self.plan.store.symbols.get(symbol);
        self.name(found.name)?;
        Ok(found)
    }

    fn ty(&self, ty: TypeId) -> Result<&'a Type, PickleError> {
        let Some(found) = self.plan.store.types.try_get(ty) else {
            return Err(if self.plan.store.types.contains(ty) {
                PickleError::IncompleteType { ty }
            } else {
                PickleError::InvalidType { ty }
            });
        };
        if matches!(found, Type::NoType | Type::Error(_)) {
            return Err(PickleError::IncompleteType { ty });
        }
        Ok(found)
    }

    fn source_tree(&self, tree: TreeId<Untyped>) -> Result<&'a Tree<Untyped>, PickleError> {
        let node = self
            .plan
            .source_ast
            .try_get(tree)
            .ok_or(PickleError::InvalidSourceTree { tree })?;
        self.span(node.position)?;
        Ok(node)
    }

    fn complete(&self, symbol: SymbolId) -> Result<TypeId, PickleError> {
        let info = self.symbol(symbol)?.info;
        let SymbolInfo::Complete(ty) = info else {
            return Err(PickleError::IncompleteSymbol { symbol, info });
        };
        self.ty(ty)?;
        Ok(ty)
    }

    fn kind(&self, symbol: SymbolId, expected: SymbolKind) -> Result<(), PickleError> {
        let actual = self.symbol(symbol)?.kind;
        if actual != expected {
            return Err(PickleError::Kind {
                symbol,
                expected,
                actual,
            });
        }
        Ok(())
    }

    fn owner(&self, symbol: SymbolId, expected: Option<SymbolId>) -> Result<(), PickleError> {
        let actual = self.symbol(symbol)?.owner;
        if let Some(owner) = actual {
            self.symbol(owner)?;
        }
        if actual != expected {
            return Err(PickleError::Owner {
                symbol,
                expected,
                actual,
            });
        }
        Ok(())
    }

    fn source_symbol(
        &self,
        symbol: SymbolId,
        tree: TreeId<Untyped>,
        kind: SymbolKind,
        name: Name,
    ) -> Result<(), PickleError> {
        self.kind(symbol, kind)?;
        let actual = self.plan.source_index.symbol_at(self.plan.source.id, tree);
        if actual != Some(symbol) {
            return Err(PickleError::SourceBinding {
                source: self.plan.source.id,
                tree,
                expected: symbol,
                actual,
            });
        }
        if self.plan.source_index.definition_of(symbol)
            != Some(SourceDefinition::Canonical {
                source: self.plan.source.id,
                tree,
            })
        {
            return Err(PickleError::SourceDefinition { symbol });
        }
        let record = self.symbol(symbol)?;
        if record.origin != SymbolOrigin::Source(self.plan.source.id) {
            return Err(PickleError::SourceOrigin {
                symbol,
                expected: self.plan.source.id,
                actual: record.origin,
            });
        }
        self.name(name)?;
        if record.name != name {
            return Err(PickleError::NameMismatch { symbol, tree });
        }
        self.span(record.position)?;
        if record.visibility != Visibility::Public
            || record.flags != SymbolFlags::EMPTY
            || !record.annotations.is_empty()
        {
            return Err(self.unsupported(UnsupportedFeature::SymbolMetadata, Some(symbol)));
        }
        Ok(())
    }

    fn metadata(&self, metadata: &Modifiers, symbol: SymbolId) -> Result<(), PickleError> {
        if metadata.visibility.is_some()
            || !metadata.modifiers.is_empty()
            || !metadata.annotations.is_empty()
        {
            return Err(self.unsupported(UnsupportedFeature::SourceMetadata, Some(symbol)));
        }
        Ok(())
    }

    fn builtin(&self, builtin: Builtin, ty: TypeId, text: &str) -> Result<SymbolId, PickleError> {
        let Type::TypeRef {
            prefix,
            target: dotty_core::TypeRefTarget::Symbol(symbol),
        } = self.ty(ty)?
        else {
            return Err(PickleError::MalformedType {
                ty,
                role: TypeRole::Builtin(builtin),
            });
        };
        self.ty(*prefix)?;
        let record = self.symbol(*symbol)?;
        if *prefix != self.plan.definitions.no_prefix
            || record.origin != SymbolOrigin::Builtin
            || record.owner.is_some()
            || record.kind != SymbolKind::Class
            || !record.name.is_type()
            || record.visibility != Visibility::Public
            || self.name(record.name)? != text
        {
            return Err(PickleError::BuiltinIdentity { builtin, ty });
        }
        Ok(*symbol)
    }

    fn scope(&self, owner: SymbolId) -> Result<&'a dotty_core::Scope, PickleError> {
        let scope = self
            .plan
            .source_index
            .scope_of(owner)
            .ok_or(PickleError::ScopeMembers { owner })?;
        if !self.plan.store.scopes.contains(scope) {
            return Err(PickleError::InvalidScope { scope });
        }
        let found = self.plan.store.scopes.get(scope);
        if found.owner != Some(owner) {
            return Err(PickleError::ScopeMembers { owner });
        }
        Ok(found)
    }

    fn package(&mut self, package: SymbolId) -> Result<(), PickleError> {
        let mut current = Some(package);
        let mut seen = HashSet::new();
        let mut chain = Vec::new();
        while let Some(symbol) = current {
            if chain.len() >= self.limits.max_owner_depth {
                return Err(PickleError::LimitExceeded {
                    kind: LimitKind::OwnerDepth,
                    limit: self.limits.max_owner_depth,
                });
            }
            self.charge(1)?;
            if !seen.insert(symbol) {
                return Err(PickleError::OwnerCycle { symbol });
            }
            self.kind(symbol, SymbolKind::Package)?;
            let record = self.symbol(symbol)?;
            if record.name.namespace() != Namespace::Term {
                return Err(PickleError::NameMismatch {
                    symbol,
                    tree: self.plan.source_root,
                });
            }
            current = record.owner;
            chain.push(symbol);
        }
        let [leaf, root] = chain.as_slice() else {
            return Err(self.unsupported(UnsupportedFeature::PackageBoundary, Some(package)));
        };
        if !self.name(self.symbol(*root)?.name)?.is_empty()
            || self.name(self.symbol(*leaf)?.name)?.is_empty()
        {
            return Err(self.unsupported(UnsupportedFeature::PackageBoundary, Some(package)));
        }
        Ok(())
    }

    fn parameters(
        &mut self,
        method: SymbolId,
        definition: &DefDef<Untyped>,
        clauses: &[Vec<SymbolId>],
    ) -> Result<(), PickleError> {
        self.charge(clauses.len())?;
        if clauses.len() != definition.value_param_clauses.len() {
            return Err(PickleError::ParameterOrder { method, clause: 0 });
        }
        let mut seen = HashSet::new();
        for (clause, (symbols, trees)) in clauses
            .iter()
            .zip(&definition.value_param_clauses)
            .enumerate()
        {
            self.charge(symbols.len())?;
            if symbols.len() != trees.len() {
                return Err(PickleError::ParameterOrder { method, clause });
            }
            for (&symbol, &tree) in symbols.iter().zip(trees) {
                if !seen.insert(symbol) {
                    return Err(PickleError::DuplicateDeclaration { symbol });
                }
                let TreeKind::ValDef(parameter) = &self.source_tree(tree)?.kind else {
                    return Err(PickleError::SourceShape {
                        tree,
                        expected: SourceRole::Parameter,
                    });
                };
                self.source_symbol(
                    symbol,
                    tree,
                    SymbolKind::Parameter,
                    *parameter.name.as_name(),
                )?;
                self.owner(symbol, Some(method))?;
                let ty = self.complete(symbol)?;
                if ty != self.plan.definitions.int {
                    return Err(PickleError::UnsupportedTypeReference { ty });
                }
            }
        }
        Ok(())
    }

    fn validate(&mut self) -> Result<SymbolId, PickleError> {
        let p = self.plan;
        self.charge(1)?;
        if p.source.name.len() > self.limits.max_name_bytes {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::NameBytes,
                limit: self.limits.max_name_bytes,
            });
        }
        if p.source.name.is_empty()
            || p.source.name.contains(['\\', ':', '\0'])
            || p.source
                .name
                .split('/')
                .any(|part| matches!(part, "" | "." | ".."))
        {
            return Err(PickleError::InvalidSourceName);
        }
        if !matches!(self.ty(p.definitions.no_prefix)?, Type::NoPrefix) {
            return Err(PickleError::MalformedType {
                ty: p.definitions.no_prefix,
                role: TypeRole::NoPrefix,
            });
        }
        self.builtin(Builtin::Int, p.definitions.int, "Int")?;
        self.builtin(Builtin::Unit, p.definitions.unit, "Unit")?;
        if self.builtin(Builtin::Object, p.definitions.object_type, "Object")?
            != p.definitions.object_class
        {
            return Err(PickleError::BuiltinIdentity {
                builtin: Builtin::Object,
                ty: p.definitions.object_type,
            });
        }
        let TreeKind::PackageDef(package_tree) = &self.source_tree(p.source_root)?.kind else {
            return Err(PickleError::SourceShape {
                tree: p.source_root,
                expected: SourceRole::Package,
            });
        };
        self.charge(package_tree.stats.len())?;
        if package_tree.stats.as_slice() != [p.class_tree] {
            return Err(self.unsupported(UnsupportedFeature::PackageBoundary, Some(p.class)));
        }
        let package = p
            .source_index
            .symbol_at(p.source.id, p.source_root)
            .ok_or(PickleError::SourceDefinition { symbol: p.class })?;
        self.package(package)?;
        self.owner(p.class, Some(package))?;
        let TreeKind::Ident(package_name) = &self.source_tree(package_tree.name)?.kind else {
            return Err(self.unsupported(UnsupportedFeature::PackageBoundary, Some(package)));
        };
        if package_name.name != self.symbol(package)?.name {
            return Err(PickleError::NameMismatch {
                symbol: package,
                tree: package_tree.name,
            });
        }
        let class_name = self.symbol(p.class)?.name;
        if !class_name.is_type() {
            return Err(PickleError::NameMismatch {
                symbol: p.class,
                tree: p.class_tree,
            });
        }
        for symbol in [package, p.class] {
            let name = self.name(self.symbol(symbol)?.name)?;
            if name.is_empty() || name.contains(['/', '\\', '.', '\0']) {
                return Err(self.unsupported(UnsupportedFeature::Name, Some(symbol)));
            }
        }
        let TreeKind::TypeDef(class) = &self.source_tree(p.class_tree)?.kind else {
            return Err(PickleError::SourceShape {
                tree: p.class_tree,
                expected: SourceRole::Class,
            });
        };
        self.source_symbol(
            p.class,
            p.class_tree,
            SymbolKind::Class,
            *class.name.as_name(),
        )?;
        self.metadata(&class.metadata, p.class)?;
        if class.variance.is_some() {
            return Err(self.unsupported(UnsupportedFeature::TypeParameters, Some(p.class)));
        }
        let info_id = self.complete(p.class)?;
        let Type::ClassInfo(info) = self.ty(info_id)? else {
            return Err(PickleError::MalformedType {
                ty: info_id,
                role: TypeRole::ClassInfo,
            });
        };
        if info.class != p.class
            || info.prefix != p.definitions.no_prefix
            || p.source_index.scope_of(p.class) != Some(info.declarations)
        {
            return Err(PickleError::MalformedType {
                ty: info_id,
                role: TypeRole::ClassInfo,
            });
        }
        self.ty(info.prefix)?;
        self.charge(info.parents.len())?;
        for &parent in &info.parents {
            self.ty(parent)?;
        }
        if info.parents.as_slice() != [p.definitions.object_type] {
            return Err(self.unsupported(UnsupportedFeature::Parents, Some(p.class)));
        }
        if info.self_type.is_some() {
            return Err(self.unsupported(UnsupportedFeature::SelfType, Some(p.class)));
        }
        let class_scope = self.scope(p.class)?;
        if self
            .scope(package)?
            .lookup_all(&class_name)
            .iter()
            .filter(|&&s| s == p.class)
            .count()
            != 1
        {
            return Err(PickleError::ScopeMembers { owner: package });
        }
        let TreeKind::Template(template) = &self.source_tree(class.rhs)?.kind else {
            return Err(PickleError::SourceShape {
                tree: class.rhs,
                expected: SourceRole::Template,
            });
        };
        if !template.parents.is_empty() {
            return Err(self.unsupported(UnsupportedFeature::Parents, Some(p.class)));
        }
        if template.self_val.is_some() {
            return Err(self.unsupported(UnsupportedFeature::SelfType, Some(p.class)));
        }
        if !template.metadata.derives.is_empty() || !template.metadata.uses.is_empty() {
            return Err(self.unsupported(UnsupportedFeature::TemplateMetadata, Some(p.class)));
        }
        self.charge(p.methods.len())?;
        let mut expected = HashSet::new();
        expected.insert(p.constructor.symbol);
        for method in p.methods {
            self.symbol(method.symbol)?;
            if !expected.insert(method.symbol) {
                return Err(PickleError::DuplicateDeclaration {
                    symbol: method.symbol,
                });
            }
        }
        self.charge(template.body.len())?;
        if template.body.len() != p.methods.len()
            || !template
                .body
                .iter()
                .zip(p.methods)
                .all(|(&tree, method)| tree == method.source_tree)
        {
            return Err(PickleError::SourceOrder { class: p.class });
        }
        let mut entered = HashSet::new();
        for symbol in class_scope.entered_symbols() {
            self.charge(1)?;
            if !expected.contains(&symbol) || !entered.insert(symbol) {
                return Err(PickleError::ScopeMembers { owner: p.class });
            }
        }
        if entered != expected {
            return Err(PickleError::ScopeMembers { owner: p.class });
        }
        for symbol in std::iter::once(p.constructor.symbol)
            .chain(p.methods.iter().map(|method| method.symbol))
        {
            let name = self.symbol(symbol)?.name;
            if !class_scope.lookup_all(&name).contains(&symbol) {
                return Err(PickleError::ScopeMembers { owner: p.class });
            }
        }
        self.constructor(template.constructor)?;
        for method in p.methods {
            self.method(method)?;
        }
        if p.methods.len() != 1 {
            return Err(self.unsupported(UnsupportedFeature::MethodCount, Some(p.class)));
        }
        Ok(package)
    }

    fn constructor(&mut self, source_tree: TreeId<Untyped>) -> Result<(), PickleError> {
        let p = self.plan;
        let constructor = &p.constructor;
        if source_tree != constructor.source_tree {
            return Err(PickleError::SourceOrder { class: p.class });
        }
        let TreeKind::DefDef(definition) = &self.source_tree(source_tree)?.kind else {
            return Err(PickleError::SourceShape {
                tree: source_tree,
                expected: SourceRole::Constructor,
            });
        };
        self.source_symbol(
            constructor.symbol,
            source_tree,
            SymbolKind::Constructor,
            *definition.name.as_name(),
        )?;
        self.owner(constructor.symbol, Some(p.class))?;
        if self.name(*definition.name.as_name())? != "<init>"
            || !definition.name.as_name().is_term()
        {
            return Err(PickleError::NameMismatch {
                symbol: constructor.symbol,
                tree: source_tree,
            });
        }
        self.metadata(&definition.metadata, constructor.symbol)?;
        if !definition.type_params.is_empty() {
            return Err(
                self.unsupported(UnsupportedFeature::TypeParameters, Some(constructor.symbol))
            );
        }
        if definition
            .value_param_clauses
            .iter()
            .any(|clause| !clause.is_empty())
        {
            return Err(self.unsupported(
                UnsupportedFeature::ParameterClauses,
                Some(constructor.symbol),
            ));
        }
        self.parameters(
            constructor.symbol,
            definition,
            &constructor.parameter_clauses,
        )?;
        if definition.value_param_clauses.iter().any(|c| !c.is_empty())
            || definition.value_param_clauses.len() > 1
            || definition.source_param_clause_order.is_some()
        {
            return Err(self.unsupported(
                UnsupportedFeature::ParameterClauses,
                Some(constructor.symbol),
            ));
        }
        if definition.rhs.is_some() {
            return Err(self.unsupported(
                UnsupportedFeature::ConstructorBody,
                Some(constructor.symbol),
            ));
        }
        if !matches!(
            self.source_tree(definition.tpt)?.kind,
            TreeKind::TypeTree(_)
        ) {
            return Err(PickleError::SourceShape {
                tree: definition.tpt,
                expected: SourceRole::TypeSyntax,
            });
        }
        let ty = self.complete(constructor.symbol)?;
        let Type::Method(signature) = self.ty(ty)? else {
            return Err(PickleError::MalformedType {
                ty,
                role: TypeRole::ConstructorSignature,
            });
        };
        if !signature.params.is_empty() || signature.kind != MethodKind::Plain {
            return Err(PickleError::MalformedType {
                ty,
                role: TypeRole::ConstructorSignature,
            });
        }
        let result = signature.result;
        if !matches!(self.ty(result)?, Type::TypeRef { prefix, target: dotty_core::TypeRefTarget::Symbol(symbol) }
            if *prefix == p.definitions.no_prefix && *symbol == p.class)
        {
            return Err(PickleError::MalformedType {
                ty: result,
                role: TypeRole::ConstructorResult,
            });
        }
        Ok(())
    }

    fn method(&mut self, method: &MethodPicklePlan) -> Result<(), PickleError> {
        self.charge(1)?;
        let p = self.plan;
        let TreeKind::DefDef(definition) = &self.source_tree(method.source_tree)?.kind else {
            return Err(PickleError::SourceShape {
                tree: method.source_tree,
                expected: SourceRole::Method,
            });
        };
        self.source_symbol(
            method.symbol,
            method.source_tree,
            SymbolKind::Method,
            *definition.name.as_name(),
        )?;
        self.owner(method.symbol, Some(p.class))?;
        if !definition.name.as_name().is_term() || self.name(*definition.name.as_name())?.is_empty()
        {
            return Err(PickleError::NameMismatch {
                symbol: method.symbol,
                tree: method.source_tree,
            });
        }
        self.metadata(&definition.metadata, method.symbol)?;
        if !definition.type_params.is_empty() {
            return Err(self.unsupported(UnsupportedFeature::TypeParameters, Some(method.symbol)));
        }
        self.parameters(method.symbol, definition, &method.parameter_clauses)?;
        if !definition.value_param_clauses.is_empty()
            || definition.source_param_clause_order.is_some()
        {
            return Err(self.unsupported(UnsupportedFeature::ParameterClauses, Some(method.symbol)));
        }
        let ty = self.complete(method.symbol)?;
        let TreeKind::Ident(result) = &self.source_tree(definition.tpt)?.kind else {
            return Err(self.unsupported(UnsupportedFeature::ResultTypeSyntax, Some(method.symbol)));
        };
        let int =
            self.ty(p.definitions.int)?
                .reference_symbol()
                .ok_or(PickleError::MalformedType {
                    ty: p.definitions.int,
                    role: TypeRole::Builtin(Builtin::Int),
                })?;
        self.name(result.name)?;
        if result.name != self.symbol(int)?.name {
            return Err(PickleError::UnsupportedTypeReference { ty });
        }
        if ty != p.definitions.int {
            return Err(PickleError::MalformedType {
                ty,
                role: TypeRole::MethodResult,
            });
        }
        let Some(rhs) = definition.rhs else {
            return Err(self.unsupported(UnsupportedFeature::AbstractMethod, Some(method.symbol)));
        };
        let source_rhs = self.source_tree(rhs)?;
        let body = method.body.ok_or(PickleError::MissingBody {
            method: method.symbol,
        })?;
        let node = p
            .typed_ast
            .try_get(body)
            .ok_or(PickleError::InvalidTypedTree { tree: body })?;
        let actual = p.typed_bindings.typed_at(p.source.id, rhs);
        if actual != Some(body) {
            return Err(PickleError::BodyBinding {
                source: p.source.id,
                tree: rhs,
                expected: body,
                actual,
            });
        }
        self.span(node.position)?;
        if let (Some(source), Some(typed)) = (source_rhs.position, node.position)
            && source != typed
        {
            return Err(PickleError::BodySpan { tree: body });
        }
        let body_type = self.ty(node.ty)?;
        let TreeKind::Literal(literal) = &node.kind else {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        };
        if !matches!((&literal.value, body_type), (Constant::Int(value), Type::Constant(Constant::Int(ty_value))) if value == ty_value)
        {
            return Err(PickleError::BodyType {
                tree: body,
                ty: node.ty,
            });
        }
        Ok(())
    }
}
