Warning: truncated output (original token count: 15515)
Total output lines: 1535

use std::collections::{HashMap, HashSet};

mod class_state;
mod expression_validation;
mod source_binding;

pub use class_state::{
    ClassFieldPicklePlan, ClassMemberPicklePlan, ClassStatePicklePlan,
    DistinctReferenceMethodParameter, DistinctReferenceMethodSignature,
    PrimaryConstructorInputPlan, ValidatedClassStatePlan, ValidatedSameBatchReferenceField,
};

use dotty_core::ast::{
    ApplyKind, AstArena, DefDef, Modifiers, NumberKind, Tree, TreeKind, UntypedNode,
};
use dotty_core::{
    Constant, Definitions, MethodKind, Name, NameId, Namespace, SemanticStore, SourceDefinition,
    SourceId, SourceSemanticIndex, SourceSpan, Symbol, SymbolFlags, SymbolId, SymbolInfo,
    SymbolKind, SymbolOrigin, TermRefTarget, TreeId, Type, TypeId, TypeRefTarget, Typed, Untyped,
    Visibility,
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

    /// Canonical classes present in the complete source/JVM/TASTy batch, when
    /// the producer has such a batch plan. The pickler validates each listed
    /// class against the same source index and semantic store before using it.
    fn planned_class_symbols(&self) -> &[SymbolId] {
        &[]
    }

    /// Canonical methods included in the complete source/JVM/TASTy batch.
    /// Cross-class body references must resolve to one of these exact symbols.
    fn planned_method_symbols(&self) -> &[SymbolId] {
        &[]
    }

    /// Exact source/typed identities for the single nested constructor
    /// argument authorized by the source frontend's cross-class proof.
    /// Implementations must derive this from the same compilation session.
    fn nested_constructor_call(
        &self,
        _source: SourceId,
        _method: SymbolId,
    ) -> Option<NestedConstructorCallBinding> {
        None
    }

    /// Exact ordered source/typed identities for two sibling nested
    /// constructor arguments authorized by one source frontend proof.
    fn nested_constructor_call_pair(
        &self,
        _source: SourceId,
        _method: SymbolId,
    ) -> Option<NestedConstructorCallPairBinding> {
        None
    }

    /// Exact original source/typed argument leaves and method-formal roots
    /// authorized by the cross-class proof for a direct-formal constructor pair.
    fn direct_formal_constructor_call_pair(
        &self,
        _source: SourceId,
        _method: SymbolId,
    ) -> Option<DirectFormalConstructorCallPairBinding> {
        None
    }

    /// Exact source/typed/symbol evidence for one same-instance planned method
    /// call used as a constructor argument. Producers must derive this from
    /// the same source/typed proof session as `typed_at`.
    fn planned_call(&self, _source: SourceId, _method: SymbolId) -> Option<PlannedCallBinding> {
        None
    }

    /// Exact source/typed/symbol/scope proofs for method-local value declarations.
    /// Producers that support local values must return the canonical frontend
    /// bindings; the pickler never reconstructs them from names.
    fn local_value_bindings(&self, _source: SourceId, _method: SymbolId) -> Vec<LocalValueBinding> {
        Vec::new()
    }
}

/// Source and typed tree identities retained by the cross-class constructor
/// proof for one `new Inner(formal)` used as another constructor's argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NestedConstructorCallBinding {
    pub source_apply: TreeId<Untyped>,
    pub typed_apply: TreeId<Typed>,
    pub source_select: TreeId<Untyped>,
    pub source_new: TreeId<Untyped>,
    pub source_argument: TreeId<Untyped>,
    pub typed_select: TreeId<Typed>,
    pub typed_new: TreeId<Typed>,
    pub typed_argument: TreeId<Typed>,
    pub class: SymbolId,
    pub constructor: SymbolId,
    pub formal: SymbolId,
    pub formal_type: TypeId,
    pub result_type: TypeId,
}

/// Ordered pair of singleton constructor receipts and the exact caller-method
/// formals referenced by each original leaf. Keeping leaf identities outside
/// `NestedConstructorCallBinding` preserves its existing singleton contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NestedConstructorCallPairBinding {
    pub calls: [NestedConstructorCallBinding; 2],
    pub argument_formals: [SymbolId; 2],
}

/// Ordered proof roots for exactly two direct caller-formal constructor
/// arguments. The producer derives these from the immutable ordered
/// cross-class receipt; the pickler rechecks them against its original trees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirectFormalConstructorCallPairBinding {
    pub source_method_formals: [TreeId<Untyped>; 2],
    pub method_formals: [SymbolId; 2],
    pub source_arguments: [TreeId<Untyped>; 2],
    pub typed_arguments: [TreeId<Typed>; 2],
    pub argument_formals: [SymbolId; 2],
}

/// Exact provenance retained by the source frontend's opaque planned-call
/// receipt. Every tree is an original source-session node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlannedCallBinding {
    pub source_apply: TreeId<Untyped>,
    pub typed_apply: TreeId<Typed>,
    pub source_function: TreeId<Untyped>,
    pub typed_function: TreeId<Typed>,
    pub source_argument: TreeId<Untyped>,
    pub typed_argument: TreeId<Typed>,
    pub source_receiver: Option<TreeId<Untyped>>,
    pub typed_receiver: Option<TreeId<Typed>>,
    pub helper_source_tree: TreeId<Untyped>,
    pub helper_source_rhs: TreeId<Untyped>,
    pub helper_typed_rhs: TreeId<Typed>,
    pub owner: SymbolId,
    pub method: SymbolId,
    pub caller_formal: SymbolId,
    pub helper_formal: SymbolId,
    pub formal_type: TypeId,
    pub result_type: TypeId,
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

/// Exact source/typed/symbol/scope evidence for one method-local Int `val` or `var`.
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
    /// Canonical bindings for supported local Int `val` and `var` declarations across methods.
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
            class_members: None,
            reference_fields: &[],
            planned_class_symbols: &[],
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

/// Validate one ordinary method from a class-state plan using the same paired
/// source/typed checks, parameter identities, and local-binding rules as the
/// established method serializer. Class-state validation calls this only after
/// validating its own field and template contract.
pub(crate) fn validate_class_state_method(
    class: &crate::ClassStatePicklePlan<'_>,
    methods: &[MethodPicklePlan],
    local_values: &[LocalValueBinding],
    reference_fields: &[SymbolId],
) -> Result<(), PickleError> {
    let constructor = DefaultConstructorPlan {
        symbol: class.constructor.symbol,
        source_tree: class.constructor.source_tree,
        parameter_clauses: class.constructor.parameter_clauses.clone(),
    };
    let plan = PicklePlan {
        store: class.store,
        definitions: class.definitions,
        typed_ast: class.typed_ast,
        source_ast: class.source_ast,
        source_index: class.source_index,
        typed_bindings: class.typed_bindings,
        source: class.source.clone(),
        // The method validator does not inspect the compilation-unit root.
        source_root: class.class_tree,
        class: class.class,
        class_tree: class.class_tree,
        constructor,
        methods,
        local_values,
    };
    let mut validator = Validator {
        plan: &plan,
        limits: ValidationLimits::default(),
        remaining: ValidationLimits::default().max_items,
        class_members: Some(class.members),
        reference_fields,
        planned_class_symbols: class.typed_bindings.planned_class_symbols(),
        local_bindings: HashMap::new(),
        registered_local_symbols: HashSet::new(),
        seen_local_bindings: HashSet::new(),
        seen_local_symbols: HashSet::new(),
        visible_locals: HashSet::new(),
    };
    for binding in local_values {
        if validator
            .local_bindings
            .insert(binding.source_tree, *binding)
            .is_some()
            || !validator.registered_local_symbols.insert(binding.symbol)
        {
            return Err(PickleError::DuplicateDeclaration {
                symbol: binding.symbol,
            });
        }
    }
    for method in methods {
        validator.method(method)?;
    }
    if validator.seen_local_bindings.len() != local_values.len() {
        let binding = local_values
            .iter()
            .find(|binding| !validator.seen_local_bindings.contains(&binding.source_tree))
            .expect("binding count mismatch has an unseen binding");
        return Err(PickleError::SourceDefinition {
            symbol: binding.symbol,
        });
    }
    Ok(())
}

struct Validator<'p, 'a> {
    plan: &'p PicklePlan<'a>,
    limits: ValidationLimits,
    remaining: usize,
    class_members: Option<&'a [ClassMemberPicklePlan]>,
    reference_fields: &'a [SymbolId],
    planned_class_symbols: &'a [SymbolId],
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

    fn is_planned_sibling_class_type(&self, ty: TypeId, syntax_name: Option<Name>) -> bool {
        let Some(Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(class),
        }) = self.plan.store.types.try_get(ty)
        else {
            return false;
        };
        if *class == self.plan.class || !self.planned_class_symbols.contains(class) {
            return false;
        }
        if !self.plan.store.symbols.contains(*class)
            || !self.plan.store.symbols.contains(self.plan.class)
        {
            return false;
        }
        let record = self.plan.store.symbols.get(*class);
        let Some(package) = self.plan.store.symbols.get(self.plan.class).owner else {
            return false;
        };
        record.kind == SymbolKind::Class
            && record.origin == SymbolOrigin::Source(self.plan.source.id)
            && record.owner == Some(package)
            && record.visibility == Visibility::Public
            && record.flags.is_empty()
            && record.annotations.is_empty()
            && syntax_name.is_none_or(|name| record.name == name)
            && matches!(
                self.plan.store.types.try_get(*prefix),
                Some(Type::TypeRef {
                    prefix: package_prefix,
                    target: TypeRefTarget::Symbol(target_package),
                }) if *package_prefix == self.plan.definitions.no_prefix
                    && *target_package == package
            )
    }

    fn same_planned_sibling_class_type(&self, left: TypeId, right: TypeId) -> bool {
        matches!(
            (self.plan.store.types.try_get(left), self.plan.store.types.try_get(right)),
            (
                Some(Type::TypeRef {
                    target: TypeRefTarget::Symbol(left_class),
                    ..
                }),
                Some(Type::TypeRef {
                    target: TypeRefTarget::Symbol(right_class),
                    ..
                })
            ) if left_class == right_class
                && self.is_planned_sibling_class_type(left, None)
                && self.is_planned_sibling_class_type(right, None)
        )
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

    fn package(&mut self, package: SymbolI…3515 tokens truncated…lt<bool, PickleError> {
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
        if definition.value_param_clauses.len() > 1 {
            return Err(self.unsupported(
                UnsupportedFeature::ParameterClauses,
                Some(constructor.symbol),
            ));
        }
        if definition
            .source_param_clause_order
            .as_ref()
            .is_some_and(|order| {
                order.as_slice() != [dotty_core::ast::DefParamClauseOrder::ValueParams(0)]
                    || definition.value_param_clauses.len() != 1
            })
        {
            return Err(self.unsupported(
                UnsupportedFeature::ParameterClauses,
                Some(constructor.symbol),
            ));
        }
        let has_two_reference_parameters =
            self.constructor_parameters(definition, &constructor.parameter_clauses)?;
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
        if signature.kind != MethodKind::Plain
            || signature.params.len() != constructor.parameter_clauses.iter().flatten().count()
        {
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
        Ok(has_two_reference_parameters)
    }

    fn constructor_parameters(
        &mut self,
        definition: &DefDef<Untyped>,
        clauses: &[Vec<SymbolId>],
    ) -> Result<bool, PickleError> {
        let constructor = self.plan.constructor.symbol;
        let class = self.plan.class;
        self.charge(clauses.len())?;
        if clauses.len() != definition.value_param_clauses.len() || clauses.len() > 1 {
            return Err(self.unsupported(UnsupportedFeature::ParameterClauses, Some(constructor)));
        }
        let signature_id = self.complete(constructor)?;
        let Type::Method(signature) = self.ty(signature_id)? else {
            return Err(PickleError::MalformedType {
                ty: signature_id,
                role: TypeRole::ConstructorSignature,
            });
        };
        let int = self.builtin(Builtin::Int, self.plan.definitions.int, "Int")?;
        let int_name = self.symbol(int)?.name;
        let mut seen_constructor = HashSet::new();
        let mut seen_template = HashSet::new();
        let mut reference_class = None;
        let mut reference_parameters = 0usize;
        let actual_parameters = definition.value_param_clauses.iter().flatten().count();
        if actual_parameters != clauses.iter().flatten().count()
            || signature.params.len() != actual_parameters
        {
            return Err(PickleError::ParameterOrder {
                method: constructor,
                clause: 0,
            });
        }
        if actual_parameters != 0 {
            self.scope(constructor)?;
        }
        let mut signature_index = 0;
        for (clause_index, (source_clause, symbols)) in definition
            .value_param_clauses
            .iter()
            .zip(clauses)
            .enumerate()
        {
            self.charge(source_clause.len())?;
            if source_clause.len() != symbols.len() {
                return Err(PickleError::ParameterOrder {
                    method: constructor,
                    clause: clause_index,
                });
            }
            for (&tree, &parameter_symbol) in source_clause.iter().zip(symbols) {
                if !seen_constructor.insert(parameter_symbol) {
                    return Err(PickleError::DuplicateDeclaration {
                        symbol: parameter_symbol,
                    });
                }
                let TreeKind::ValDef(parameter) = &self.source_tree(tree)?.kind else {
                    return Err(PickleError::SourceShape {
                        tree,
                        expected: SourceRole::Parameter,
                    });
                };
                let source = self.plan.source.id;
                if self
                    .plan
                    .source_index
                    .derived_symbol_at(constructor, source, tree)
                    != Some(parameter_symbol)
                {
                    return Err(PickleError::SourceBinding {
                        source,
                        tree,
                        expected: parameter_symbol,
                        actual: self
                            .plan
                            .source_index
                            .derived_symbol_at(constructor, source, tree),
                    });
                }
                let template_symbol = self.plan.source_index.symbol_at(source, tree).ok_or(
                    PickleError::SourceDefinition {
                        symbol: parameter_symbol,
                    },
                )?;
                if !seen_template.insert(template_symbol) {
                    return Err(PickleError::DuplicateDeclaration {
                        symbol: template_symbol,
                    });
                }
                let template_record = self.symbol(template_symbol)?;
                let parameter_record = self.symbol(parameter_symbol)?;
                // These paired declarations bypass source_symbol, so validate
                // provenance for both independently owned parameter identities.
                self.span(template_record.position)?;
                self.span(parameter_record.position)?;
                // Parser-internal accessors are expected for ordinary class
                // parameters; PrivateLocal distinguishes a plain constructor
                // input from source `val`/`var` accessor parameters. Reject
                // those forms before examining their distinct field identity.
                if parameter.metadata.modifiers.as_slice()
                    != [
                        dotty_core::ast::Modifier::ParamAccessor,
                        dotty_core::ast::Modifier::PrivateLocal,
                    ]
                    || parameter.metadata.visibility.is_some()
                    || !parameter.metadata.annotations.is_empty()
                    || parameter.rhs.is_some()
                {
                    return Err(self.unsupported(
                        UnsupportedFeature::PrimaryConstructorInput,
                        Some(parameter_symbol),
                    ));
                }
                if template_symbol == parameter_symbol
                    || template_record.kind != SymbolKind::Parameter
                    || template_record.owner != Some(class)
                    || template_record.origin != SymbolOrigin::Source(source)
                    || template_record.visibility != Visibility::Private
                    || !template_record.flags.is_empty()
                    || !template_record.annotations.is_empty()
                    || parameter_record.kind != SymbolKind::Parameter
                    || parameter_record.owner != Some(constructor)
                    || parameter_record.origin != SymbolOrigin::Source(source)
                    || parameter_record.visibility != Visibility::Public
                    || !parameter_record.flags.is_empty()
                    || !parameter_record.annotations.is_empty()
                    || template_record.name != *parameter.name.as_name()
                    || parameter_record.name != *parameter.name.as_name()
                    || self.plan.source_index.definition_of(template_symbol)
                        != Some(SourceDefinition::Canonical { source, tree })
                    || self.plan.source_index.definition_of(parameter_symbol)
                        != Some(SourceDefinition::Derived { source, tree })
                {
                    return Err(PickleError::SourceDefinition {
                        symbol: parameter_symbol,
                    });
                }
                let TreeKind::Ident(parameter_type) = &self.source_tree(parameter.tpt)?.kind else {
                    return Err(self.unsupported(
                        UnsupportedFeature::PrimaryConstructorInput,
                        Some(parameter_symbol),
                    ));
                };
                let semantic_type = self.complete(parameter_symbol)?;
                let template_type = match template_record.info {
                    SymbolInfo::Complete(ty) => Some(ty),
                    SymbolInfo::Missing | SymbolInfo::Deferred(_) => None,
                    info => {
                        return Err(PickleError::IncompleteSymbol {
                            symbol: template_symbol,
                            info,
                        });
                    }
                };
                let signature_parameter = signature.params[signature_index];
                let is_int = semantic_type == self.plan.definitions.int;
                let _is_reference = if is_int {
                    false
                } else {
                    let Type::TypeRef {
                        prefix,
                        target: TypeRefTarget::Symbol(target_class),
                    } = self.ty(semantic_type)?
                    else {
                        return Err(PickleError::UnsupportedTypeReference { ty: semantic_type });
                    };
                    let target_class = *target_class;
                    let target_record = self.symbol(target_class)?;
                    let target_package = self.symbol(class)?.owner;
                    let package_prefix_matches = if let Some(package) = target_package {
                        matches!(self.ty(*prefix)?, Type::TypeRef {
                            prefix,
                            target: TypeRefTarget::Symbol(actual_package),
                        } if *prefix == self.plan.definitions.no_prefix && *actual_package == package)
                    } else {
                        false
                    };
                    let Some(SourceDefinition::Canonical { source, tree }) =
                        self.plan.source_index.definition_of(target_class)
                    else {
                        return Err(PickleError::SourceDefinition {
                            symbol: target_class,
                        });
                    };
                    let TreeKind::TypeDef(target_definition) = &self.source_tree(tree)?.kind else {
                        return Err(PickleError::SourceShape {
                            tree,
                            expected: SourceRole::Class,
                        });
                    };
                    if source != self.plan.source.id
                        || self.plan.source_index.symbol_at(source, tree) != Some(target_class)
                        || target_record.kind != SymbolKind::Class
                        || target_record.origin != SymbolOrigin::Source(source)
                        || target_record.owner != target_package
                        || target_record.visibility != Visibility::Public
                        || !target_record.flags.is_empty()
                        || !target_record.annotations.is_empty()
                        || target_record.name != parameter_type.name
                        || target_definition.name.as_name() != &parameter_type.name
                        || !package_prefix_matches
                        || !self
                            .plan
                            .typed_bindings
                            .planned_class_symbols()
                            .contains(&target_class)
                        || reference_class.is_some_and(|prior| prior != target_class)
                    {
                        return Err(PickleError::UnsupportedTypeReference { ty: semantic_type });
                    }
                    reference_class = Some(target_class);
                    reference_parameters += 1;
                    if !matches!(
                        self.ty(signature_parameter.ty)?,
                        Type::TypeRef { prefix: signature_prefix, target: TypeRefTarget::Symbol(signature_class) }
                            if signature_prefix == prefix && *signature_class == target_class
                    ) || template_type.is_some_and(|ty| {
                        !matches!(self.ty(ty), Ok(Type::TypeRef { prefix: template_prefix, target: TypeRefTarget::Symbol(template_class) })
                            if template_prefix == prefix && *template_class == target_class)
                    }) {
                        return Err(PickleError::UnsupportedTypeReference {
                            ty: signature_parameter.ty,
                        });
                    }
                    true
                };
                if (is_int
                    && (parameter_type.name != int_name
                        || !matches!(self.ty(signature_parameter.ty)?, Type::TypeRef { prefix, target: TypeRefTarget::Symbol(symbol) }
                            if *prefix == self.plan.definitions.no_prefix && *symbol == int)
                        || template_type.is_some_and(|ty| ty != self.plan.definitions.int)))
                    || signature_parameter.name.as_name() != parameter.name.as_name()
                    || signature_parameter.erased
                    || signature_parameter.varargs
                    || (is_int && semantic_type != self.plan.definitions.int)
                    || self
                        .scope(constructor)?
                        .lookup_all(parameter.name.as_name())
                        != [parameter_symbol]
                {
                    return Err(PickleError::Unsupported {
                        feature: UnsupportedFeature::PrimaryConstructorInput,
                        symbol: Some(parameter_symbol),
                    });
                }
                signature_index += 1;
            }
        }
        if reference_parameters != 0
            && !((reference_parameters == 1 && actual_parameters == 1)
                || (reference_parameters == 2 && actual_parameters == 2))
        {
            return Err(self.unsupported(
                UnsupportedFeature::PrimaryConstructorInput,
                Some(constructor),
            ));
        }
        Ok(reference_parameters == 2)
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
        let supports_result = |validator: &Self, result_ty| {
            result_ty == p.definitions.int
                || result_ty == p.definitions.boolean
                || result_ty == p.definitions.unit
                || validator.is_planned_sibling_class_type(result_ty, Some(result.name))
        };
        match definition.value_param_clauses.as_slice() {
            [] if supports_result(self, ty) => {}
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
                let planned_class_parameters = signature
                    .params
                    .iter()
                    .filter(|formal| self.is_planned_sibling_class_type(formal.ty, None))
                    .count();
                if planned_class_parameters != 0
                    && !((planned_class_parameters == 1 && signature.params.len() == 1)
                        || (planned_class_parameters == 2
                            && signature.params.len() == 2
                            && self.same_planned_sibling_class_type(
                                signature.params[0].ty,
                                signature.params[1].ty,
                            )
                            && self.is_planned_sibling_class_type(signature.result, None)
                            && !self.same_planned_sibling_class_type(
                                signature.params[0].ty,
                                signature.result,
                            )))
                {
                    return Err(PickleError::MalformedType {
                        ty,
                        role: TypeRole::Parameter,
                    });
                }
                if !supports_result(self, signature.result) {
                    return Err(PickleError::MalformedType {
                        ty,
                        role: TypeRole::MethodResult,
                    });
                }
                for (formal, &parameter) in
                    signature.params.iter().zip(&method.parameter_clauses[0])
                {
                    if formal.name.as_name() != &self.symbol(parameter)?.name
                        || (formal.ty != p.definitions.int
                            && formal.ty != p.definitions.boolean
                            && !self.is_planned_sibling_class_type(formal.ty, None))
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
        } else if self.is_planned_sibling_class_type(result_type, Some(result.name)) {
            result.name
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
