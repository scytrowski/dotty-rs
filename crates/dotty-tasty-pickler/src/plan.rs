use std::collections::{HashMap, HashSet};

mod expression_validation;
mod source_binding;

use dotty_core::ast::{
    ApplyKind, AstArena, DefDef, Modifiers, NumberKind, Tree, TreeKind, UntypedNode,
};
use dotty_core::{
    Constant, Definitions, MethodKind, Name, NameId, Namespace, SemanticStore, SourceDefinition,
    SourceId, SourceSemanticIndex, SourceSpan, Symbol, SymbolFlags, SymbolId, SymbolInfo,
    SymbolKind, SymbolOrigin, TermRefTarget, TreeId, Type, TypeId, Typed, Untyped, Visibility,
};

use crate::{
    Builtin, LimitKind, PickleError, SourceRole, TypeRole, UnsupportedFeature,
    bounded_expression_depth,
};

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

/// Exact source/typed/symbol/scope evidence for one method-local immutable value.
///
/// The source integration obtains this tuple from its `SourceTyper` session;
/// the pickler never reconstructs local identity from names or allocation order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalValueBinding {
    pub source_tree: TreeId<Untyped>,
    pub symbol: SymbolId,
    pub typed_declaration: TreeId<Typed>,
    pub scope: dotty_core::ScopeId,
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
    /// Canonical bindings for supported local `val` declarations across methods.
    pub local_values: &'a [LocalValueBinding],
}

/// Budgets checked before traversing caller-controlled collections or chains.
/// Expression recursion is capped at 256 for stack safety.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValidationLimits {
    pub max_owner_depth: usize,
    pub max_expression_depth: usize,
    pub max_items: usize,
    pub max_name_bytes: usize,
}

impl Default for ValidationLimits {
    fn default() -> Self {
        Self {
            max_owner_depth: 256,
            max_expression_depth: 256,
            max_items: 1_000_000,
            max_name_bytes: 4096,
        }
    }
}

impl ValidationLimits {
    fn expression_depth_limit(self) -> usize {
        bounded_expression_depth(self.max_expression_depth)
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
            local_bindings: HashMap::new(),
            registered_local_symbols: HashSet::new(),
            seen_local_bindings: HashSet::new(),
            seen_local_symbols: HashSet::new(),
            visible_locals: HashSet::new(),
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
    local_bindings: HashMap<TreeId<Untyped>, LocalValueBinding>,
    registered_local_symbols: HashSet<SymbolId>,
    seen_local_bindings: HashSet<TreeId<Untyped>>,
    seen_local_symbols: HashSet<SymbolId>,
    visible_locals: HashSet<SymbolId>,
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
        let boolean = self.builtin(Builtin::Boolean, self.plan.definitions.boolean, "Boolean")?;
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
                let declared_type = match parameter_type.name {
                    name if name == self.symbol(int)?.name => self.plan.definitions.int,
                    name if name == self.symbol(boolean)?.name => self.plan.definitions.boolean,
                    _ => {
                        return Err(
                            self.unsupported(UnsupportedFeature::ParameterClauses, Some(symbol))
                        );
                    }
                };
                self.source_symbol(
                    symbol,
                    tree,
                    SymbolKind::Parameter,
                    *parameter.name.as_name(),
                )?;
                self.owner(symbol, Some(method))?;
                let ty = self.complete(symbol)?;
                if ty != declared_type {
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
        self.charge(p.local_values.len())?;
        let mut local_symbols = HashSet::new();
        for binding in p.local_values {
            if self
                .local_bindings
                .insert(binding.source_tree, *binding)
                .is_some()
                || !local_symbols.insert(binding.symbol)
            {
                return Err(PickleError::DuplicateDeclaration {
                    symbol: binding.symbol,
                });
            }
            self.registered_local_symbols.insert(binding.symbol);
        }
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
        self.builtin(Builtin::Boolean, p.definitions.boolean, "Boolean")?;
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
        if self.seen_local_bindings.len() != p.local_values.len() {
            let binding = p
                .local_values
                .iter()
                .find(|binding| !self.seen_local_bindings.contains(&binding.source_tree))
                .expect("binding count mismatch has an unseen binding");
            return Err(PickleError::SourceDefinition {
                symbol: binding.symbol,
            });
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

    fn method(&mut self, method: &MethodPicklePlan) -> Result<(), PickleError> {
        self.visible_locals.clear();
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
        let boolean = self.ty(p.definitions.boolean)?.reference_symbol().ok_or(
            PickleError::MalformedType {
                ty: p.definitions.boolean,
                role: TypeRole::Builtin(Builtin::Boolean),
            },
        )?;
        let unit =
            self.ty(p.definitions.unit)?
                .reference_symbol()
                .ok_or(PickleError::MalformedType {
                    ty: p.definitions.unit,
                    role: TypeRole::Builtin(Builtin::Unit),
                })?;
        self.name(result.name)?;
        match definition.value_param_clauses.as_slice() {
            [] if ty == p.definitions.int
                || ty == p.definitions.boolean
                || ty == p.definitions.unit => {}
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
                if signature.result != p.definitions.int
                    && signature.result != p.definitions.boolean
                    && signature.result != p.definitions.unit
                {
                    return Err(PickleError::MalformedType {
                        ty,
                        role: TypeRole::MethodResult,
                    });
                }
                for (formal, &parameter) in
                    signature.params.iter().zip(&method.parameter_clauses[0])
                {
                    if formal.name.as_name() != &self.symbol(parameter)?.name
                        || (formal.ty != p.definitions.int && formal.ty != p.definitions.boolean)
                        || self.complete(parameter)? != formal.ty
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
        let result_type = match self.ty(ty)? {
            Type::Method(signature) => signature.result,
            _ => ty,
        };
        let expected_result_name = if result_type == p.definitions.int {
            self.symbol(int)?.name
        } else if result_type == p.definitions.boolean {
            self.symbol(boolean)?.name
        } else if result_type == p.definitions.unit {
            self.symbol(unit)?.name
        } else {
            return Err(PickleError::MalformedType {
                ty: result_type,
                role: TypeRole::MethodResult,
            });
        };
        if result.name != expected_result_name {
            return Err(PickleError::MalformedType {
                ty: result_type,
                role: TypeRole::MethodResult,
            });
        }
        self.validate_method_expression(definition.rhs, method.body, method, result_type)
    }
}
