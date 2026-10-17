use std::collections::HashSet;

use dotty_core::ast::{
    ApplyKind, AstArena, DefDef, Modifiers, NumberKind, Tree, TreeKind, UntypedNode,
};
use dotty_core::{
    Constant, Definitions, MethodKind, Name, NameId, Namespace, SemanticStore, SourceDefinition,
    SourceId, SourceSemanticIndex, SourceSpan, Symbol, SymbolFlags, SymbolId, SymbolInfo,
    SymbolKind, SymbolOrigin, TermRefTarget, TreeId, Type, TypeId, Typed, Untyped, Visibility,
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
/// Validation emits no bytes. Consumers may explicitly build an incomplete
/// declaration skeleton from this checked input.
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
    /// Check the frozen class/default-constructor/multiple-Int-literal method slice.
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
        self.name_text(name.text())
    }

    fn name_text(&self, name: NameId) -> Result<&'a str, PickleError> {
        let text = self
            .plan
            .store
            .names
            .try_resolve(name)
            .ok_or(PickleError::InvalidName { name })?;
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

    fn parameter_metadata(
        &mut self,
        metadata: &Modifiers,
        symbol: SymbolId,
    ) -> Result<(), PickleError> {
        self.charge(metadata.modifiers.len())?;
        self.charge(metadata.annotations.len())?;
        let mut saw_param = false;
        for modifier in &metadata.modifiers {
            if *modifier != dotty_core::ast::Modifier::Param || saw_param {
                return Err(self.unsupported(UnsupportedFeature::SourceMetadata, Some(symbol)));
            }
            saw_param = true;
        }
        if metadata.visibility.is_some() || !metadata.annotations.is_empty() {
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
        if clauses.iter().any(|clause| !clause.is_empty()) {
            self.scope(method)?;
        }
        let int = self.builtin(Builtin::Int, self.plan.definitions.int, "Int")?;
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
                self.parameter_metadata(&parameter.metadata, symbol)?;
                if parameter.rhs.is_some() {
                    return Err(
                        self.unsupported(UnsupportedFeature::ParameterClauses, Some(symbol))
                    );
                }
                let TreeKind::Ident(parameter_type) = &self.source_tree(parameter.tpt)?.kind else {
                    return Err(
                        self.unsupported(UnsupportedFeature::ParameterClauses, Some(symbol))
                    );
                };
                if parameter_type.name != self.symbol(int)?.name {
                    return Err(
                        self.unsupported(UnsupportedFeature::ParameterClauses, Some(symbol))
                    );
                }
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
                if self.scope(method)?.lookup_all(parameter.name.as_name()) != [symbol] {
                    return Err(PickleError::ScopeMembers { owner: method });
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
        self.metadata(&class.metadata, p.class)?;
        self.source_symbol(
            p.class,
            p.class_tree,
            SymbolKind::Class,
            *class.name.as_name(),
        )?;
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
        if self.scope(package)?.lookup_all(&class_name) != [p.class] {
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
        self.charge(template.body.len())?;
        for &tree in &template.body {
            if !matches!(self.source_tree(tree)?.kind, TreeKind::DefDef(_)) {
                return Err(self.unsupported(UnsupportedFeature::ClassMembers, Some(p.class)));
            }
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
        let mut invalid_scope = false;
        for symbol in class_scope.entered_symbols() {
            self.charge(1)?;
            if !expected.contains(&symbol) || !entered.insert(symbol) {
                // Finish the bounded scan before reporting membership: the
                // winning error must not depend on HashMap iteration order.
                invalid_scope = true;
            }
        }
        if invalid_scope || entered != expected {
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
        if p.methods.is_empty() {
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
        self.charge(definition.value_param_clauses.len())?;
        if definition.value_param_clauses.len() > 1
            || definition
                .value_param_clauses
                .first()
                .is_some_and(|clause| !clause.is_empty())
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
        if definition.source_param_clause_order.is_some() {
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

    fn same_class_method_target(
        &mut self,
        target: SymbolId,
        source_name: dotty_core::Name,
        prefix: TypeId,
        source_tree: TreeId<Untyped>,
    ) -> Result<(), PickleError> {
        let p = self.plan;
        let target_method = p
            .methods
            .iter()
            .find(|candidate| candidate.symbol == target)
            .ok_or(PickleError::Reference {
                symbol: target,
                reason: crate::ReferenceError::NotRegistered,
            })?;
        self.kind(target, SymbolKind::Method)?;
        self.owner(target, Some(p.class))?;
        let target_tree = self.source_tree(target_method.source_tree)?;
        let TreeKind::DefDef(target_definition) = &target_tree.kind else {
            return Err(PickleError::SourceShape {
                tree: target_method.source_tree,
                expected: SourceRole::Method,
            });
        };
        if target_definition.name.as_name() != &source_name
            || !target_method.parameter_clauses.is_empty()
        {
            return Err(PickleError::NameMismatch {
                symbol: target,
                tree: source_tree,
            });
        }
        self.source_symbol(
            target,
            target_method.source_tree,
            SymbolKind::Method,
            *target_definition.name.as_name(),
        )?;
        self.metadata(&target_definition.metadata, target)?;
        let target_type = self.complete(target)?;
        if target_type != p.definitions.int {
            return Err(PickleError::MalformedType {
                ty: target_type,
                role: TypeRole::MethodResult,
            });
        }
        if !matches!(self.ty(prefix)?, Type::ThisType { class } if *class == p.class) {
            return Err(PickleError::Reference {
                symbol: target,
                reason: crate::ReferenceError::Owner,
            });
        }
        Ok(())
    }

    fn same_class_method_application_target(
        &mut self,
        target: SymbolId,
        source_name: dotty_core::Name,
        prefix: TypeId,
        source_tree: TreeId<Untyped>,
    ) -> Result<(), PickleError> {
        let p = self.plan;
        let target_method = p
            .methods
            .iter()
            .find(|candidate| candidate.symbol == target)
            .ok_or(PickleError::Reference {
                symbol: target,
                reason: crate::ReferenceError::NotRegistered,
            })?;
        self.kind(target, SymbolKind::Method)?;
        self.owner(target, Some(p.class))?;
        let target_tree = self.source_tree(target_method.source_tree)?;
        let TreeKind::DefDef(definition) = &target_tree.kind else {
            return Err(PickleError::SourceShape {
                tree: target_method.source_tree,
                expected: SourceRole::Method,
            });
        };
        if definition.name.as_name() != &source_name
            || target_method.parameter_clauses.len() != 1
            || target_method.parameter_clauses[0].len() != 1
            || definition.value_param_clauses.len() != 1
        {
            return Err(PickleError::NameMismatch {
                symbol: target,
                tree: source_tree,
            });
        }
        self.source_symbol(
            target,
            target_method.source_tree,
            SymbolKind::Method,
            *definition.name.as_name(),
        )?;
        self.metadata(&definition.metadata, target)?;
        let ty = self.complete(target)?;
        let Type::Method(signature) = self.ty(ty)? else {
            return Err(PickleError::MalformedType {
                ty,
                role: TypeRole::Parameter,
            });
        };
        let parameter = target_method.parameter_clauses[0][0];
        if signature.kind != MethodKind::Plain
            || signature.params.len() != 1
            || signature.result != p.definitions.int
            || signature.params[0].name.as_name() != &self.symbol(parameter)?.name
            || signature.params[0].ty != p.definitions.int
            || signature.params[0].erased
            || signature.params[0].varargs
            || self.complete(parameter)? != p.definitions.int
        {
            return Err(PickleError::MalformedType {
                ty,
                role: TypeRole::Parameter,
            });
        }
        if !matches!(self.ty(prefix)?, Type::ThisType { class } if *class == p.class) {
            return Err(PickleError::Reference {
                symbol: target,
                reason: crate::ReferenceError::Owner,
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
        if definition.value_param_clauses.len() > 1
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
        match definition.value_param_clauses.as_slice() {
            [] if ty == p.definitions.int => {}
            [] => {
                return Err(PickleError::MalformedType {
                    ty,
                    role: TypeRole::MethodResult,
                });
            }
            [clause] => {
                let Type::Method(signature) = self.ty(ty)? else {
                    return Err(PickleError::MalformedType {
                        ty,
                        role: if clause.is_empty() {
                            TypeRole::MethodResult
                        } else {
                            TypeRole::Parameter
                        },
                    });
                };
                if signature.kind != MethodKind::Plain
                    || signature.params.len() != method.parameter_clauses[0].len()
                {
                    return Err(PickleError::MalformedType {
                        ty,
                        role: TypeRole::Parameter,
                    });
                }
                if signature.result != p.definitions.int {
                    return Err(PickleError::MalformedType {
                        ty,
                        role: TypeRole::MethodResult,
                    });
                }
                for (formal, &parameter) in
                    signature.params.iter().zip(&method.parameter_clauses[0])
                {
                    if formal.name.as_name() != &self.symbol(parameter)?.name
                        || formal.ty != p.definitions.int
                        || formal.erased
                        || formal.varargs
                    {
                        return Err(PickleError::MalformedType {
                            ty,
                            role: TypeRole::Parameter,
                        });
                    }
                }
            }
            _ => {
                return Err(
                    self.unsupported(UnsupportedFeature::ParameterClauses, Some(method.symbol))
                );
            }
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
        let expected = match (&source_rhs.kind, &node.kind) {
            (TreeKind::Ident(source_ident), TreeKind::Ident(typed_ident)) => {
                if source_ident.name != typed_ident.name
                    || source_ident.backquoted != typed_ident.backquoted
                {
                    return Err(PickleError::SourceShape {
                        tree: rhs,
                        expected: SourceRole::Body,
                    });
                }
                let Type::TermRef {
                    prefix,
                    target: TermRefTarget::Symbol(target),
                } = body_type
                else {
                    return Err(PickleError::BodyType {
                        tree: body,
                        ty: node.ty,
                    });
                };
                if p.methods
                    .iter()
                    .any(|candidate| candidate.symbol == *target)
                {
                    self.same_class_method_target(*target, source_ident.name, *prefix, rhs)?;
                    None
                } else {
                    let parameter = target;
                    self.kind(*parameter, SymbolKind::Parameter)?;
                    self.owner(*parameter, Some(method.symbol))?;
                    if !method
                        .parameter_clauses
                        .iter()
                        .flatten()
                        .any(|candidate| candidate == parameter)
                    {
                        return Err(PickleError::Reference {
                            symbol: *parameter,
                            reason: crate::ReferenceError::NotRegistered,
                        });
                    }
                    if self.symbol(*parameter)?.name != source_ident.name {
                        return Err(PickleError::NameMismatch {
                            symbol: *parameter,
                            tree: rhs,
                        });
                    }
                    if *prefix != p.definitions.no_prefix
                        || !matches!(self.ty(*prefix)?, Type::NoPrefix)
                    {
                        return Err(PickleError::Reference {
                            symbol: *parameter,
                            reason: crate::ReferenceError::Owner,
                        });
                    }
                    if self.complete(*parameter)? != p.definitions.int {
                        return Err(PickleError::UnsupportedTypeReference { ty: node.ty });
                    }
                    None
                }
            }
            (TreeKind::Select(source_select), TreeKind::Select(typed_select)) => {
                if source_select.name != typed_select.name
                    || source_select.backquoted != typed_select.backquoted
                {
                    return Err(PickleError::SourceShape {
                        tree: rhs,
                        expected: SourceRole::Body,
                    });
                }
                let source_qualifier = self.source_tree(source_select.qualifier)?;
                let typed_qualifier = p.typed_ast.try_get(typed_select.qualifier).ok_or(
                    PickleError::InvalidTypedTree {
                        tree: typed_select.qualifier,
                    },
                )?;
                if !matches!(source_qualifier.kind, TreeKind::This(this) if this.qual.is_none())
                    || !matches!(typed_qualifier.kind, TreeKind::This(this) if this.qual.is_none())
                {
                    return Err(PickleError::Unsupported {
                        feature: UnsupportedFeature::BodyExpression,
                        symbol: Some(method.symbol),
                    });
                }
                let actual_qualifier = p
                    .typed_bindings
                    .typed_at(p.source.id, source_select.qualifier);
                if actual_qualifier != Some(typed_select.qualifier) {
                    return Err(PickleError::BodyBinding {
                        source: p.source.id,
                        tree: source_select.qualifier,
                        expected: typed_select.qualifier,
                        actual: actual_qualifier,
                    });
                }
                self.span(source_qualifier.position)?;
                self.span(typed_qualifier.position)?;
                if let (Some(source), Some(typed)) =
                    (source_qualifier.position, typed_qualifier.position)
                    && source != typed
                {
                    return Err(PickleError::BodySpan {
                        tree: typed_select.qualifier,
                    });
                }
                let Type::TermRef {
                    prefix,
                    target: TermRefTarget::Symbol(target),
                } = body_type
                else {
                    return Err(PickleError::BodyType {
                        tree: body,
                        ty: node.ty,
                    });
                };
                if !p
                    .methods
                    .iter()
                    .any(|candidate| candidate.symbol == *target)
                {
                    return Err(PickleError::Reference {
                        symbol: *target,
                        reason: crate::ReferenceError::NotRegistered,
                    });
                }
                self.same_class_method_target(*target, source_select.name, *prefix, rhs)?;
                if !matches!(self.ty(typed_qualifier.ty)?, Type::ThisType { class } if *class == p.class)
                {
                    return Err(PickleError::Reference {
                        symbol: *target,
                        reason: crate::ReferenceError::Owner,
                    });
                }
                None
            }
            (TreeKind::Literal(source_literal), TreeKind::Literal(literal)) => {
                if !matches!((&literal.value, body_type), (Constant::Int(value), Type::Constant(Constant::Int(ty_value))) if value == ty_value)
                {
                    return Err(PickleError::BodyType {
                        tree: body,
                        ty: node.ty,
                    });
                }
                match source_literal.value {
                    Constant::Int(value) => Some(value),
                    _ => None,
                }
            }
            (TreeKind::Apply(source_apply), TreeKind::Apply(typed_apply)) => {
                if source_apply.kind != ApplyKind::Regular
                    || typed_apply.kind != ApplyKind::Regular
                    || source_apply.args.len() != 1
                    || typed_apply.args.len() != 1
                    || source_apply.function == source_apply.args[0]
                    || typed_apply.function == typed_apply.args[0]
                {
                    return Err(
                        self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                    );
                }
                let source_function = self.source_tree(source_apply.function)?;
                let typed_function = p.typed_ast.try_get(typed_apply.function).ok_or(
                    PickleError::InvalidTypedTree {
                        tree: typed_apply.function,
                    },
                )?;
                let (name, explicit_this) = match (&source_function.kind, &typed_function.kind) {
                    (TreeKind::Ident(source), TreeKind::Ident(typed))
                        if source.name == typed.name =>
                    {
                        (source.name, false)
                    }
                    (TreeKind::Select(source), TreeKind::Select(typed))
                        if source.name == typed.name =>
                    {
                        let qualifier = self.source_tree(source.qualifier)?;
                        let typed_qualifier = p.typed_ast.try_get(typed.qualifier).ok_or(
                            PickleError::InvalidTypedTree {
                                tree: typed.qualifier,
                            },
                        )?;
                        if !matches!(qualifier.kind, TreeKind::This(this) if this.qual.is_none())
                            || !matches!(typed_qualifier.kind, TreeKind::This(this) if this.qual.is_none())
                        {
                            return Err(self.unsupported(
                                UnsupportedFeature::BodyExpression,
                                Some(method.symbol),
                            ));
                        }
                        if p.typed_bindings.typed_at(p.source.id, source.qualifier)
                            != Some(typed.qualifier)
                        {
                            return Err(PickleError::BodyBinding {
                                source: p.source.id,
                                tree: source.qualifier,
                                expected: typed.qualifier,
                                actual: p.typed_bindings.typed_at(p.source.id, source.qualifier),
                            });
                        }
                        self.span(qualifier.position)?;
                        self.span(typed_qualifier.position)?;
                        if let (Some(source_span), Some(typed_span)) =
                            (qualifier.position, typed_qualifier.position)
                            && source_span != typed_span
                        {
                            return Err(PickleError::BodySpan {
                                tree: typed.qualifier,
                            });
                        }
                        if !matches!(self.ty(typed_qualifier.ty)?, Type::ThisType { class } if *class == p.class)
                        {
                            return Err(PickleError::Reference {
                                symbol: method.symbol,
                                reason: crate::ReferenceError::Owner,
                            });
                        }
                        (source.name, true)
                    }
                    _ => {
                        return Err(PickleError::SourceShape {
                            tree: source_apply.function,
                            expected: SourceRole::Body,
                        });
                    }
                };
                if p.typed_bindings
                    .typed_at(p.source.id, source_apply.function)
                    != Some(typed_apply.function)
                {
                    return Err(PickleError::BodyBinding {
                        source: p.source.id,
                        tree: source_apply.function,
                        expected: typed_apply.function,
                        actual: p
                            .typed_bindings
                            .typed_at(p.source.id, source_apply.function),
                    });
                }
                let Type::TermRef {
                    prefix,
                    target: TermRefTarget::Symbol(target),
                } = self.ty(typed_function.ty)?
                else {
                    return Err(PickleError::BodyType {
                        tree: typed_apply.function,
                        ty: typed_function.ty,
                    });
                };
                if !p
                    .methods
                    .iter()
                    .any(|candidate| candidate.symbol == *target)
                {
                    return Err(PickleError::Reference {
                        symbol: *target,
                        reason: crate::ReferenceError::NotRegistered,
                    });
                }
                self.same_class_method_application_target(
                    *target,
                    name,
                    *prefix,
                    source_apply.function,
                )?;
                if explicit_this
                    && !matches!(self.ty(typed_function.ty)?, Type::TermRef { prefix, .. } if matches!(self.ty(*prefix)?, Type::ThisType { class } if *class == p.class))
                {
                    return Err(PickleError::Reference {
                        symbol: *target,
                        reason: crate::ReferenceError::Owner,
                    });
                }
                let source_arg = source_apply.args[0];
                let typed_arg = typed_apply.args[0];
                if p.typed_bindings.typed_at(p.source.id, source_arg) != Some(typed_arg) {
                    return Err(PickleError::BodyBinding {
                        source: p.source.id,
                        tree: source_arg,
                        expected: typed_arg,
                        actual: p.typed_bindings.typed_at(p.source.id, source_arg),
                    });
                }
                let source_argument = self.source_tree(source_arg)?;
                let typed_argument = p
                    .typed_ast
                    .try_get(typed_arg)
                    .ok_or(PickleError::InvalidTypedTree { tree: typed_arg })?;
                self.span(source_argument.position)?;
                self.span(typed_argument.position)?;
                if let (Some(source), Some(typed)) =
                    (source_argument.position, typed_argument.position)
                    && source != typed
                {
                    return Err(PickleError::BodySpan { tree: typed_arg });
                }
                match (&source_argument.kind, &typed_argument.kind) {
                    (TreeKind::Ident(source), TreeKind::Ident(typed))
                        if source.name == typed.name && source.backquoted == typed.backquoted =>
                    {
                        let Type::TermRef {
                            target: TermRefTarget::Symbol(symbol),
                            ..
                        } = self.ty(typed_argument.ty)?
                        else {
                            return Err(PickleError::BodyType {
                                tree: typed_arg,
                                ty: typed_argument.ty,
                            });
                        };
                        if !method
                            .parameter_clauses
                            .iter()
                            .flatten()
                            .any(|parameter| parameter == symbol)
                        {
                            return Err(PickleError::Reference {
                                symbol: *symbol,
                                reason: crate::ReferenceError::NotRegistered,
                            });
                        }
                        if self.symbol(*symbol)?.name != source.name {
                            return Err(PickleError::NameMismatch {
                                symbol: *symbol,
                                tree: source_arg,
                            });
                        }
                    }
                    (TreeKind::Literal(source), TreeKind::Literal(typed))
                        if source.value == typed.value
                            && matches!(typed.value, Constant::Int(_)) => {}
                    (
                        TreeKind::PhaseSpecific(UntypedNode::Number(number)),
                        TreeKind::Literal(typed),
                    ) => {
                        let digits = self.name_text(number.text)?.replace('_', "");
                        let value = match number.kind {
                            NumberKind::Whole(10) => digits.parse::<i32>().ok(),
                            NumberKind::Whole(radix @ (2 | 16)) => {
                                let digits = if radix == 16 {
                                    digits
                                        .strip_prefix("0x")
                                        .or_else(|| digits.strip_prefix("0X"))
                                } else {
                                    digits
                                        .strip_prefix("0b")
                                        .or_else(|| digits.strip_prefix("0B"))
                                };
                                digits
                                    .and_then(|digits| u32::from_str_radix(digits, radix).ok())
                                    .map(|bits| bits as i32)
                            }
                            _ => None,
                        };
                        if value
                            != match typed.value {
                                Constant::Int(value) => Some(value),
                                _ => None,
                            }
                        {
                            return Err(PickleError::BodyType {
                                tree: typed_arg,
                                ty: typed_argument.ty,
                            });
                        }
                    }
                    _ => {
                        return Err(PickleError::SourceShape {
                            tree: source_arg,
                            expected: SourceRole::Body,
                        });
                    }
                }
                let argument_type = self.ty(typed_argument.ty)?;
                let valid_argument_type = match argument_type {
                    Type::Constant(Constant::Int(_)) => true,
                    Type::TermRef {
                        prefix,
                        target: TermRefTarget::Symbol(symbol),
                    } => {
                        if !matches!(self.ty(*prefix)?, Type::NoPrefix) {
                            false
                        } else {
                            self.kind(*symbol, SymbolKind::Parameter)?;
                            self.owner(*symbol, Some(method.symbol))?;
                            self.complete(*symbol)? == p.definitions.int
                        }
                    }
                    _ => typed_argument.ty == p.definitions.int,
                };
                if !valid_argument_type {
                    return Err(PickleError::BodyType {
                        tree: typed_arg,
                        ty: typed_argument.ty,
                    });
                }
                if node.ty != p.definitions.int
                    && !matches!(self.ty(node.ty)?, Type::Constant(Constant::Int(_)))
                {
                    return Err(PickleError::BodyType {
                        tree: body,
                        ty: node.ty,
                    });
                }
                None
            }
            (TreeKind::PhaseSpecific(UntypedNode::Number(number)), TreeKind::Literal(literal)) => {
                if !matches!((&literal.value, body_type), (Constant::Int(value), Type::Constant(Constant::Int(ty_value))) if value == ty_value)
                {
                    return Err(PickleError::BodyType {
                        tree: body,
                        ty: node.ty,
                    });
                }
                // Expression parsing retains unsuffixed integers as raw numbers.
                // Bound the interned spelling before normalizing or decoding it.
                let digits = self.name_text(number.text)?.replace('_', "");
                let value = match number.kind {
                    NumberKind::Whole(10) => digits.parse::<i32>().ok(),
                    NumberKind::Whole(radix @ (2 | 16)) => {
                        let digits = if radix == 16 {
                            digits
                                .strip_prefix("0x")
                                .or_else(|| digits.strip_prefix("0X"))
                        } else {
                            digits
                                .strip_prefix("0b")
                                .or_else(|| digits.strip_prefix("0B"))
                        };
                        // Nondecimal Int literals retain their signed 32-bit pattern,
                        // matching the source typer (e.g. 0xffffffff is -1).
                        digits
                            .and_then(|digits| u32::from_str_radix(digits, radix).ok())
                            .map(|bits| bits as i32)
                    }
                    NumberKind::Whole(_) => None,
                    NumberKind::Decimal | NumberKind::Floating => {
                        return Err(self
                            .unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
                    }
                };
                Some(value.ok_or(PickleError::SourceShape {
                    tree: rhs,
                    expected: SourceRole::Body,
                })?)
            }
            (TreeKind::Ident(_), _) => {
                return Err(PickleError::SourceShape {
                    tree: rhs,
                    expected: SourceRole::Body,
                });
            }
            (_, _) => {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                );
            }
        };
        if let Some(expected) = expected {
            let TreeKind::Literal(literal) = &node.kind else {
                return Err(PickleError::SourceShape {
                    tree: rhs,
                    expected: SourceRole::Body,
                });
            };
            if let Constant::Int(actual) = &literal.value
                && expected != *actual
            {
                return Err(PickleError::BodySourceValue {
                    source_tree: rhs,
                    typed_tree: body,
                    expected,
                    actual: *actual,
                });
            }
        }
        Ok(())
    }
}
