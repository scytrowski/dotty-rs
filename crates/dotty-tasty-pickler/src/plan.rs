use std::collections::{HashMap, HashSet};

mod class_state;
mod contextual_closure;
mod contextual_default;
mod contextual_poly;
mod expression_validation;
mod source_binding;

pub(crate) use contextual_default::ContextualDefault;

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
    /// Identity of this source, type, or annotation in its owning session table.
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
    /// Returns the exact typed root recorded for this source tree in the same canonical frontend
    /// session, or None when no binding exists. Do not synthesize bindings from names or arena
    /// allocation order.
    fn typed_at(&self, source: SourceId, tree: TreeId<Untyped>) -> Option<TreeId<Typed>>;

    /// The frontend's receipt for a generated method or parameter declaration.
    fn synthetic_definition_symbol(&self, _tree: TreeId<Typed>) -> Option<SymbolId> {
        None
    }

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

    /// Ordered source/typed arguments and their proven origins for one
    /// Formal + NestedNew constructor pair. Producers must derive the receipt
    /// from the common ordered cross-class proof in this same session.
    fn ordered_constructor_call_argument_pair(
        &self,
        _source: SourceId,
        _method: SymbolId,
    ) -> Option<OrderedConstructorCallArgumentPairBinding> {
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

    /// Canonical source/typed/symbol/case-scope tuples for pattern Bind roots.
    /// The complete class-state producer consumes these separately from ordinary
    /// ValDef locals; the older literal PicklePlan path rejects case bindings.
    /// Every supplied binding
    /// must occur in this method's validated cases, in the same session arenas.
    fn pattern_value_bindings(
        &self,
        _source: SourceId,
        _method: SymbolId,
    ) -> Vec<LocalValueBinding> {
        Vec::new()
    }

    /// Authenticated direct-New initializer for a local admitted by a
    /// FormalLocalPair or LocalValPair constructor receipt. This is separate
    /// from the legacy PlannedCallBinding path used by helper-local values.
    fn local_counter_initializer(
        &self,
        _source: SourceId,
        _method: SymbolId,
    ) -> Option<LocalCounterInitializerBinding> {
        None
    }

    /// Authenticated direct-New initializers for the bounded immutable-local
    /// constructor body. The default preserves existing single-local users.
    fn local_counter_initializers(
        &self,
        source: SourceId,
        method: SymbolId,
    ) -> Vec<LocalCounterInitializerBinding> {
        self.local_counter_initializer(source, method)
            .into_iter()
            .collect()
    }

    /// Authenticated immutable local reference alias for the one supported
    /// three-local Counter block. The alias remains a declaration of its own;
    /// its initializer target is the original first local.
    fn local_reference_alias(
        &self,
        _source: SourceId,
        _method: SymbolId,
    ) -> Option<LocalReferenceAliasBinding> {
        None
    }

    /// Borrowed receipt for the mutable Counter declaration fragment only.
    /// It does not authorize serialization of the surrounding method body.
    fn mutable_counter_local_initializer(
        &self,
        _source: SourceId,
        _method: SymbolId,
    ) -> Option<MutableCounterLocalInitializerBinding> {
        None
    }

    /// Borrowed source/typed receipt for the single original Counter
    /// reassignment. It authorizes only that assignment in the incomplete
    /// mutable Counter fragment, not general reference mutation.
    fn mutable_counter_reference_assignment(
        &self,
        _source: SourceId,
        _method: SymbolId,
    ) -> Option<MutableCounterReferenceAssignmentBinding> {
        None
    }

    /// Borrowed identity receipt for an immutable local selected between two
    /// existing references by one Boolean IF. This proof carries no producer
    /// authorization; implementations must preserve the same source session.
    fn conditional_local_reference_selection(
        &self,
        _source: SourceId,
        _method: SymbolId,
    ) -> Option<ConditionalLocalReferenceSelectionBinding> {
        None
    }

    /// Borrowed identity receipt for the original Unit-valued conditional
    /// whose THEN and ELSE arms assign one initialized mutable Counter local.
    /// It does not authorize arbitrary reference assignments or nested flow.
    fn conditional_mutable_reference_assignment(
        &self,
        _source: SourceId,
        _method: SymbolId,
    ) -> Option<ConditionalMutableReferenceAssignmentBinding> {
        None
    }

    /// Exact ordered direct-formal/local pair receipt from the source proof.
    fn formal_local_constructor_call_pair(
        &self,
        _source: SourceId,
        _method: SymbolId,
    ) -> Option<FormalLocalConstructorCallPairBinding> {
        None
    }

    /// Exact source/typed identity for the single authenticated local-receiver
    /// Unit statement admitted between the two local declarations and final
    /// constructor result. This receipt is derived from the same frontend
    /// session as `typed_at`; it does not authorize arbitrary statement calls.
    fn local_receiver_unit_call(
        &self,
        _source: SourceId,
        _method: SymbolId,
    ) -> Option<LocalReceiverUnitCallBinding> {
        None
    }

    /// Exact source-ordered identities for the two authenticated local-receiver
    /// Unit statements admitted by the bounded PairFactory body shape.
    fn local_receiver_unit_call_pair(
        &self,
        _source: SourceId,
        _method: SymbolId,
    ) -> Option<[LocalReceiverUnitCallBinding; 2]> {
        None
    }
}

/// Source and typed tree identities retained by the cross-class constructor
/// proof for one `new Inner(formal)` used as another constructor's argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NestedConstructorCallBinding {
    /// Exact source application root in the canonical session arena.
    pub source_apply: TreeId<Untyped>,
    /// Exact typed application root in the canonical session arena.
    pub typed_apply: TreeId<Typed>,
    /// Exact source member selection root in the canonical session arena.
    pub source_select: TreeId<Untyped>,
    /// Exact source construction root in the canonical session arena.
    pub source_new: TreeId<Untyped>,
    /// Exact source argument root in the canonical session arena.
    pub source_argument: TreeId<Untyped>,
    /// Exact typed member selection root in the canonical session arena.
    pub typed_select: TreeId<Typed>,
    /// Exact typed construction root in the canonical session arena.
    pub typed_new: TreeId<Typed>,
    /// Exact typed argument root in the canonical session arena.
    pub typed_argument: TreeId<Typed>,
    /// Class declaration identity being inspected in the supplied session.
    pub class: SymbolId,
    /// Constructor declaration identity being inspected in the supplied session.
    pub constructor: SymbolId,
    /// Formal parameter identity; this is not inferred from its name.
    pub formal: SymbolId,
    /// Semantic type of the canonical formal parameter.
    pub formal_type: TypeId,
    /// Semantic result type in the canonical type arena.
    pub result_type: TypeId,
}

/// Ordered pair of singleton constructor receipts and the exact caller-method
/// formals referenced by each original leaf. Keeping leaf identities outside
/// `NestedConstructorCallBinding` preserves its existing singleton contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NestedConstructorCallPairBinding {
    /// Nested constructor calls in source argument order.
    pub calls: [NestedConstructorCallBinding; 2],
    /// Formal identities in constructor argument order.
    pub argument_formals: [SymbolId; 2],
}

/// Ordered proof roots for exactly two direct caller-formal constructor
/// arguments. The producer derives these from the immutable ordered
/// cross-class receipt; the pickler rechecks them against its original trees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirectFormalConstructorCallPairBinding {
    /// Exact source method parameter declarations roots in original source order; retain their
    /// paired bindings.
    pub source_method_formals: [TreeId<Untyped>; 2],
    /// Original method formals in source declaration order.
    pub method_formals: [SymbolId; 2],
    /// Exact source arguments roots in original source order; retain their paired bindings.
    pub source_arguments: [TreeId<Untyped>; 2],
    /// Exact typed arguments roots in original source order; retain their paired bindings.
    pub typed_arguments: [TreeId<Typed>; 2],
    /// Formal identities in constructor argument order.
    pub argument_formals: [SymbolId; 2],
}

/// Origin of one argument in the bounded mixed constructor pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderedConstructorCallArgumentOrigin {
    /// An original method formal passed directly to the outer constructor.
    Formal {
        /// Formal parameter identity; this is not inferred from its name.
        formal: SymbolId,
        /// Semantic type of the canonical formal parameter.
        formal_type: TypeId,
        /// Semantic result type in the canonical type arena.
        result_type: TypeId,
    },
    /// An original one-level `new Counter(seed)` expression.
    NestedNew {
        /// Validated identities of the nested constructor call.
        call: NestedConstructorCallBinding,
        /// Original formal supplying this constructor or method argument.
        argument_formal: SymbolId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Exact source and typed argument roots with their canonical formal or immutable-local origin.
pub struct FormalLocalConstructorCallArgumentBinding {
    /// Exact source argument root in the canonical session arena.
    pub source_argument: TreeId<Untyped>,
    /// Exact typed argument root in the canonical session arena.
    pub typed_argument: TreeId<Typed>,
    /// Provenance of this declaration, artifact, or argument binding.
    pub origin: FormalLocalConstructorCallArgumentOrigin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Canonical source binding supplying one argument in the supported formal/local constructor pair.
pub enum FormalLocalConstructorCallArgumentOrigin {
    /// Original method formal passed directly to the constructor.
    Formal {
        /// Formal parameter identity; this is not inferred from its name.
        formal: SymbolId,
        /// Semantic type of the canonical formal parameter.
        formal_type: TypeId,
        /// Semantic result type in the canonical type arena.
        result_type: TypeId,
    },
    /// Original immutable local value passed to the constructor.
    LocalValue {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Semantic result type in the canonical type arena.
        result_type: TypeId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Original method formals and two argument bindings in constructor argument order.
/// All identities must come from the same canonical frontend session.
pub struct FormalLocalConstructorCallPairBinding {
    /// Exact source method parameter declarations roots in original source order; retain their
    /// paired bindings.
    pub source_method_formals: [TreeId<Untyped>; 2],
    /// Original method formals in source declaration order.
    pub method_formals: [SymbolId; 2],
    /// Argument values or validated bindings in original argument order.
    pub arguments: [FormalLocalConstructorCallArgumentBinding; 2],
}

/// Canonical call roots retained by the source proof for
/// `first.set(secondSeed): Unit` in the bounded two-local Pair factory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalReceiverUnitCallBinding {
    /// Exact source application root in the canonical session arena.
    pub source_apply: TreeId<Untyped>,
    /// Exact typed application root in the canonical session arena.
    pub typed_apply: TreeId<Typed>,
    /// Exact source member selection root in the canonical session arena.
    pub source_select: TreeId<Untyped>,
    /// Exact typed member selection root in the canonical session arena.
    pub typed_select: TreeId<Typed>,
    /// Exact source receiver root in the canonical session arena.
    pub source_receiver: TreeId<Untyped>,
    /// Exact typed receiver root in the canonical session arena.
    pub typed_receiver: TreeId<Typed>,
    /// Exact source argument root in the canonical session arena.
    pub source_argument: TreeId<Untyped>,
    /// Exact typed argument root in the canonical session arena.
    pub typed_argument: TreeId<Typed>,
    /// Canonical symbol used as the call receiver.
    pub receiver: SymbolId,
    /// Lookup scope in which the original receiver binding was entered.
    pub receiver_scope: dotty_core::ScopeId,
    /// Method declaration identity being inspected in the supplied session.
    pub method: SymbolId,
    /// Original formal supplying this constructor or method argument.
    pub argument_formal: SymbolId,
}

/// Original source/typed identities for the direct `new Counter(seed)` local
/// initializer authorized by the cross-class immutable-local proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalCounterInitializerBinding {
    /// Exact source block root in the canonical session arena.
    pub source_block: TreeId<Untyped>,
    /// Exact typed block root in the canonical session arena.
    pub typed_block: TreeId<Typed>,
    /// Exact source declaration root in the canonical session arena.
    pub source_declaration: TreeId<Untyped>,
    /// Exact typed declaration root in the canonical session arena.
    pub typed_declaration: TreeId<Typed>,
    /// Exact source initializer root in the canonical session arena.
    pub source_initializer: TreeId<Untyped>,
    /// Exact typed initializer root in the canonical session arena.
    pub typed_initializer: TreeId<Typed>,
    /// Exact source type syntax root in the canonical session arena.
    pub source_type_tree: TreeId<Untyped>,
    /// Exact typed type syntax root in the canonical session arena.
    pub typed_type_tree: TreeId<Typed>,
    /// Local binding identity in the canonical session symbol table.
    pub local: SymbolId,
    /// Lookup-scope identity in the canonical session scope arena.
    pub scope: dotty_core::ScopeId,
    /// Type of the local binding in the canonical type arena.
    pub local_type: TypeId,
    /// Original formal supplying this constructor or method argument.
    pub argument_formal: SymbolId,
    /// Validated identities of the nested constructor call.
    pub call: NestedConstructorCallBinding,
}

/// Original source/typed roots proving one immutable local aliases an earlier
/// local of the same reference type in the same method scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalReferenceAliasBinding {
    /// Exact source block root in the canonical session arena.
    pub source_block: TreeId<Untyped>,
    /// Exact typed block root in the canonical session arena.
    pub typed_block: TreeId<Typed>,
    /// Exact source declaration root in the canonical session arena.
    pub source_declaration: TreeId<Untyped>,
    /// Exact typed declaration root in the canonical session arena.
    pub typed_declaration: TreeId<Typed>,
    /// Exact source initializer root in the canonical session arena.
    pub source_initializer: TreeId<Untyped>,
    /// Exact typed initializer root in the canonical session arena.
    pub typed_initializer: TreeId<Typed>,
    /// Exact source type syntax root in the canonical session arena.
    pub source_type_tree: TreeId<Untyped>,
    /// Exact typed type syntax root in the canonical session arena.
    pub typed_type_tree: TreeId<Typed>,
    /// Exact source call target root in the canonical session arena.
    pub source_target: TreeId<Untyped>,
    /// Exact typed call target root in the canonical session arena.
    pub typed_target: TreeId<Typed>,
    /// Local binding identity in the canonical session symbol table.
    pub local: SymbolId,
    /// Binding or control-flow destination selected by this node.
    pub target: SymbolId,
    /// Lookup-scope identity in the canonical session scope arena.
    pub scope: dotty_core::ScopeId,
    /// Type of the local binding in the canonical type arena.
    pub local_type: TypeId,
}

/// Exact declaration and original initializer roots from the #2299 proof.
/// This is evidence for a separately incomplete declaration fragment only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MutableCounterLocalInitializerBinding {
    /// Identity of the source buffer in the current compilation session.
    pub source_id: SourceId,
    /// Semantic owner of the declaration; ownership is distinct from lookup scope.
    pub owner: SymbolId,
    /// Method declaration identity being inspected in the supplied session.
    pub method: SymbolId,
    /// Lookup-scope identity in the canonical session scope arena.
    pub scope: dotty_core::ScopeId,
    /// Exact source block root in the canonical session arena.
    pub source_block: TreeId<Untyped>,
    /// Exact typed block root in the canonical session arena.
    pub typed_block: TreeId<Typed>,
    /// The two original local declarations in their real source order.
    pub prefix_source_declarations: [TreeId<Untyped>; 2],
    /// Exact typed prefix declarations in original statement order.
    pub prefix_typed_declarations: [TreeId<Typed>; 2],
    /// Local identities in the original block statement order.
    pub prefix_locals: [SymbolId; 2],
    /// Exact source declaration root in the canonical session arena.
    pub source_declaration: TreeId<Untyped>,
    /// Exact typed declaration root in the canonical session arena.
    pub typed_declaration: TreeId<Typed>,
    /// Exact source initializer root in the canonical session arena.
    pub source_initializer: TreeId<Untyped>,
    /// Exact typed initializer root in the canonical session arena.
    pub typed_initializer: TreeId<Typed>,
    /// Local binding identity in the canonical session symbol table.
    pub local: SymbolId,
    /// Binding or control-flow destination selected by this node.
    pub target: SymbolId,
    /// Type of the local binding in the canonical type arena.
    pub local_type: TypeId,
}

/// Exact original source/typed identities for one selected-local Counter
/// assignment borrowed from the #2299 reassignment proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MutableCounterReferenceAssignmentBinding {
    /// Identity of the source buffer in the current compilation session.
    pub source_id: SourceId,
    /// Semantic owner of the declaration; ownership is distinct from lookup scope.
    pub owner: SymbolId,
    /// Method declaration identity being inspected in the supplied session.
    pub method: SymbolId,
    /// Lookup-scope identity in the canonical session scope arena.
    pub scope: dotty_core::ScopeId,
    /// Exact source block root in the canonical session arena.
    pub source_block: TreeId<Untyped>,
    /// Exact typed block root in the canonical session arena.
    pub typed_block: TreeId<Typed>,
    /// Exact source assignment root in the canonical session arena.
    pub source_assignment: TreeId<Untyped>,
    /// Exact typed assignment root in the canonical session arena.
    pub typed_assignment: TreeId<Typed>,
    /// Exact source assignment target root in the canonical session arena.
    pub source_lhs: TreeId<Untyped>,
    /// Exact typed assignment target root in the canonical session arena.
    pub typed_lhs: TreeId<Typed>,
    /// Exact source right-hand side root in the canonical session arena.
    pub source_rhs: TreeId<Untyped>,
    /// Exact typed right-hand side root in the canonical session arena.
    pub typed_rhs: TreeId<Typed>,
    /// Local binding identity in the canonical session symbol table.
    pub local: SymbolId,
    /// Local binding referenced by the assignment value.
    pub rhs_local: SymbolId,
    /// Second local binding in the ordered construction proof.
    pub second_local: SymbolId,
    /// Type of the local binding in the canonical type arena.
    pub local_type: TypeId,
}

#[derive(Clone, Copy)]
struct FragmentPredecessorInitializer {
    source_declaration: TreeId<Untyped>,
    typed_declaration: TreeId<Typed>,
    local: SymbolId,
    proof: MutableCounterLocalInitializerBinding,
    source_initializer: TreeId<Untyped>,
    typed_initializer: TreeId<Typed>,
}

/// Exact source/typed identities for one immutable local reference selection.
/// The binding is provenance only and never authorizes serialization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConditionalLocalReferenceSelectionBinding {
    /// Identity of the source buffer in the current compilation session.
    pub source_id: SourceId,
    /// Semantic owner of the declaration; ownership is distinct from lookup scope.
    pub owner: SymbolId,
    /// Method declaration identity being inspected in the supplied session.
    pub method: SymbolId,
    /// Lookup-scope identity in the canonical session scope arena.
    pub scope: dotty_core::ScopeId,
    /// Exact source block root in the canonical session arena.
    pub source_block: TreeId<Untyped>,
    /// Exact typed block root in the canonical session arena.
    pub typed_block: TreeId<Typed>,
    /// Exact source declaration root in the canonical session arena.
    pub source_declaration: TreeId<Untyped>,
    /// Exact typed declaration root in the canonical session arena.
    pub typed_declaration: TreeId<Typed>,
    /// Exact source conditional root in the canonical session arena.
    pub source_if: TreeId<Untyped>,
    /// Exact typed conditional root in the canonical session arena.
    pub typed_if: TreeId<Typed>,
    /// Exact source condition root in the canonical session arena.
    pub source_condition: TreeId<Untyped>,
    /// Exact typed condition root in the canonical session arena.
    pub typed_condition: TreeId<Typed>,
    /// Exact source branches roots in original source order; retain their paired bindings.
    pub source_arms: [TreeId<Untyped>; 2],
    /// Exact typed branches roots in original source order; retain their paired bindings.
    pub typed_arms: [TreeId<Typed>; 2],
    /// Original Boolean formal used as the branch condition.
    pub condition_formal: SymbolId,
    /// Target declaration identities in source branch order.
    pub targets: [SymbolId; 2],
    /// Declaration selected by the recorded binding.
    pub selected: SymbolId,
    /// Type of the local binding in the canonical type arena.
    pub local_type: TypeId,
}

/// Exact source and typed identities for the two ordered assignments of one
/// mutable local inside the original Unit-valued conditional. This records
/// provenance only and does not authorize serialization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConditionalMutableReferenceAssignmentBinding {
    /// Identity of the source buffer in the current compilation session.
    pub source_id: SourceId,
    /// Semantic owner of the declaration; ownership is distinct from lookup scope.
    pub owner: SymbolId,
    /// Method declaration identity being inspected in the supplied session.
    pub method: SymbolId,
    /// Lookup-scope identity in the canonical session scope arena.
    pub scope: dotty_core::ScopeId,
    /// Exact source block root in the canonical session arena.
    pub source_block: TreeId<Untyped>,
    /// Exact typed block root in the canonical session arena.
    pub typed_block: TreeId<Typed>,
    /// Exact source conditional root in the canonical session arena.
    pub source_if: TreeId<Untyped>,
    /// Exact typed conditional root in the canonical session arena.
    pub typed_if: TreeId<Typed>,
    /// Exact source condition root in the canonical session arena.
    pub source_condition: TreeId<Untyped>,
    /// Exact typed condition root in the canonical session arena.
    pub typed_condition: TreeId<Typed>,
    /// THEN assignment followed by ELSE assignment.
    pub source_assignments: [TreeId<Untyped>; 2],
    /// Exact typed assignments roots in original source order; retain their paired bindings.
    pub typed_assignments: [TreeId<Typed>; 2],
    /// Exact source assignment targets roots in original source order; retain their paired
    /// bindings.
    pub source_lhss: [TreeId<Untyped>; 2],
    /// Exact typed assignment targets roots in original source order; retain their paired
    /// bindings.
    pub typed_lhss: [TreeId<Typed>; 2],
    /// Exact source right-hand sides roots in original source order; retain their paired bindings.
    pub source_rhss: [TreeId<Untyped>; 2],
    /// Exact typed right-hand sides roots in original source order; retain their paired bindings.
    pub typed_rhss: [TreeId<Typed>; 2],
    /// Original Boolean formal used as the branch condition.
    pub condition_formal: SymbolId,
    /// Local binding identity in the canonical session symbol table.
    pub local: SymbolId,
    /// THEN RHS followed by ELSE RHS, retaining semantic identity order.
    pub rhs_locals: [SymbolId; 2],
    /// Type of the local binding in the canonical type arena.
    pub local_type: TypeId,
}

/// Exact source and typed roots plus the source-proven origin for one outer
/// constructor argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OrderedConstructorCallArgumentBinding {
    /// Exact source argument root in the canonical session arena.
    pub source_argument: TreeId<Untyped>,
    /// Exact typed argument root in the canonical session arena.
    pub typed_argument: TreeId<Typed>,
    /// Provenance of this declaration, artifact, or argument binding.
    pub origin: OrderedConstructorCallArgumentOrigin,
}

/// Original method parameter roots and the two per-position source-proven
/// origins for one mixed constructor application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OrderedConstructorCallArgumentPairBinding {
    /// Exact source method parameter declarations roots in original source order; retain their
    /// paired bindings.
    pub source_method_formals: [TreeId<Untyped>; 2],
    /// Original method formals in source declaration order.
    pub method_formals: [SymbolId; 2],
    /// Argument values or validated bindings in original argument order.
    pub arguments: [OrderedConstructorCallArgumentBinding; 2],
}

/// Exact provenance retained by the source frontend's opaque planned-call
/// receipt. Every tree is an original source-session node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlannedCallBinding {
    /// Exact source application root in the canonical session arena.
    pub source_apply: TreeId<Untyped>,
    /// Exact typed application root in the canonical session arena.
    pub typed_apply: TreeId<Typed>,
    /// Exact source function root in the canonical session arena.
    pub source_function: TreeId<Untyped>,
    /// Exact typed function root in the canonical session arena.
    pub typed_function: TreeId<Typed>,
    /// Exact source argument root in the canonical session arena.
    pub source_argument: TreeId<Untyped>,
    /// Exact typed argument root in the canonical session arena.
    pub typed_argument: TreeId<Typed>,
    /// Exact source receiver root in the canonical session arena.
    pub source_receiver: Option<TreeId<Untyped>>,
    /// Exact typed receiver root in the canonical session arena.
    pub typed_receiver: Option<TreeId<Typed>>,
    /// Exact source tree root in the canonical session arena.
    pub helper_source_tree: TreeId<Untyped>,
    /// Exact source right-hand side root in the canonical session arena.
    pub helper_source_rhs: TreeId<Untyped>,
    /// Exact typed right-hand side root in the canonical session arena.
    pub helper_typed_rhs: TreeId<Typed>,
    /// Semantic owner of the declaration; ownership is distinct from lookup scope.
    pub owner: SymbolId,
    /// Method declaration identity being inspected in the supplied session.
    pub method: SymbolId,
    /// Original method parameter supplying the argument.
    pub caller_formal: SymbolId,
    /// Original helper method parameter supplying the result.
    pub helper_formal: SymbolId,
    /// Semantic type of the canonical formal parameter.
    pub formal_type: TypeId,
    /// Semantic result type in the canonical type arena.
    pub result_type: TypeId,
}

impl SourceTypedBindings for dotty_core::SourceTypedIndex {
    fn typed_at(&self, source: SourceId, tree: TreeId<Untyped>) -> Option<TreeId<Typed>> {
        self.get(source, tree)
    }
    fn synthetic_definition_symbol(&self, tree: TreeId<Typed>) -> Option<SymbolId> {
        dotty_core::SourceTypedIndex::synthetic_definition_symbol(self, tree)
    }
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
    /// Declaration identity involved in the operation, relative to the caller’s symbol table.
    pub symbol: SymbolId,
    /// Exact source tree root in the canonical session arena.
    pub source_tree: TreeId<Untyped>,
    /// Explicit source clauses, not the normalized semantic empty clause.
    pub parameter_clauses: Vec<Vec<SymbolId>>,
}

/// A canonical method and its real typed RHS, in source declaration order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MethodPicklePlan {
    /// Declaration identity involved in the operation, relative to the caller’s symbol table.
    pub symbol: SymbolId,
    /// Exact source tree root in the canonical session arena.
    pub source_tree: TreeId<Untyped>,
    /// Preserve clause boundaries: `[]` and `[[]]` are distinct.
    pub parameter_clauses: Vec<Vec<SymbolId>>,
    /// None means a declaration; a concrete source RHS requires its exact root.
    pub body: Option<TreeId<Typed>>,
}

/// Exact source/typed/symbol/scope evidence for one supported method-local value.
///
/// The source integration obtains this tuple from its `SourceTyper` session;
/// the pickler never reconstructs local identity from names or allocation order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalValueBinding {
    /// Exact source tree root in the canonical session arena.
    pub source_tree: TreeId<Untyped>,
    /// Declaration identity involved in the operation, relative to the caller’s symbol table.
    pub symbol: SymbolId,
    /// Exact typed declaration root in the canonical session arena.
    pub typed_declaration: TreeId<Typed>,
    /// Lookup-scope identity in the canonical session scope arena.
    pub scope: dotty_core::ScopeId,
}

/// Borrowed, unvalidated input. No semantic records or typed trees are cloned.
/// The explicit source root prevents silently omitting extra top-level forms.
pub struct PicklePlan<'a> {
    /// Borrowed canonical semantic store; all plan identities must belong to it.
    pub store: &'a SemanticStore,
    /// Builtin identities bootstrapped in the same semantic store.
    pub definitions: &'a Definitions,
    /// Typed arena paired with the original source arena.
    pub typed_ast: &'a AstArena<Typed>,
    /// Original untyped arena for the exact source compilation session.
    pub source_ast: &'a AstArena<Untyped>,
    /// Canonical naming index for these source trees and symbols.
    pub source_index: &'a SourceSemanticIndex,
    /// Source-to-typed binding evidence from the same frontend session.
    pub typed_bindings: &'a dyn SourceTypedBindings,
    /// Source buffer identity and normalized relative path supplied by the producer.
    pub source: SourceMetadata,
    /// Exact source root root in the canonical session arena.
    pub source_root: TreeId<Untyped>,
    /// Class declaration identity being inspected in the supplied session.
    pub class: SymbolId,
    /// Original source class declaration root in the session arena.
    pub class_tree: TreeId<Untyped>,
    /// Canonical no-argument constructor identity and its original source tree.
    pub constructor: DefaultConstructorPlan,
    /// Declared methods in source or class-file order.
    pub methods: &'a [MethodPicklePlan],
    /// Canonical bindings for supported method-local value declarations across methods.
    pub local_values: &'a [LocalValueBinding],
}

/// Budgets checked before traversing caller-controlled collections or chains.
/// Expression recursion is capped at 256 for stack safety.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValidationLimits {
    /// Maximum owner-chain depth accepted by validation.
    pub max_owner_depth: usize,
    /// Maximum recursive expression depth accepted by validation.
    pub max_expression_depth: usize,
    /// Maximum number of items processed by the bounded operation.
    pub max_items: usize,
    /// Maximum byte length accepted for a name.
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
    pub(crate) context_closures: HashMap<SymbolId, dotty_core::CheckedUnaryContextClosure>,
    pub(crate) context_defaults: HashMap<SymbolId, ContextualDefault>,
}

/// Validated input for the explicitly incomplete #2300 declaration fragment.
/// This type cannot be converted to `PickledTasty` or used for complete-body
/// serialization.
pub struct ValidatedMutableCounterDeclarationFragment<'p, 'a> {
    pub(crate) plan: ValidatedClassStatePlan<'p, 'a>,
    pub(crate) binding: MutableCounterLocalInitializerBinding,
}

/// Validated input for the explicitly incomplete #2303 assignment fragment.
/// This is limited to the original mutable Counter local and its one borrowed
/// assignment; it cannot be converted to `PickledTasty`.
pub struct ValidatedMutableCounterReferenceAssignmentFragment<'p, 'a> {
    pub(crate) plan: ValidatedClassStatePlan<'p, 'a>,
    pub(crate) declaration: MutableCounterLocalInitializerBinding,
    pub(crate) assignment: MutableCounterReferenceAssignmentBinding,
}

impl<'p, 'a> ValidatedPicklePlan<'p, 'a> {
    /// Borrows the original plan retained by this validation receipt.
    pub fn plan(&self) -> &'p PicklePlan<'a> {
        self.plan
    }
    /// The canonical named package reached through the root class owner chain.
    pub fn package(&self) -> SymbolId {
        self.package
    }
}

impl<'a> PicklePlan<'a> {
    /// Check the canonical class, default constructor, method signatures and
    /// bounded primitive bodies, including supported local Int returns.
    /// Stateful classes and case bindings use the separate class-state plan.
    /// Neither success nor failure changes stores, arenas, indices, or files.
    pub fn validate(&self) -> Result<ValidatedPicklePlan<'_, 'a>, PickleError> {
        self.validate_with_limits(ValidationLimits::default())
    }

    /// Validates canonical identities, source-to-typed bindings, supported shapes, and explicit
    /// work limits without mutating stores or arenas. Returns a borrowing receipt on success or a
    /// typed PickleError on rejection.
    pub fn validate_with_limits(
        &self,
        limits: ValidationLimits,
    ) -> Result<ValidatedPicklePlan<'_, 'a>, PickleError> {
        self.validate_mode(limits)
    }

    fn validate_mode(
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
            mutable_counter_assignment: None,
            conditional_mutable_assignment: None,
            local_bindings: HashMap::new(),
            pattern_bindings: HashMap::new(),
            registered_local_symbols: HashSet::new(),
            seen_local_bindings: HashSet::new(),
            seen_local_symbols: HashSet::new(),
            visible_locals: HashSet::new(),
            context_closures: HashMap::new(),
            context_defaults: HashMap::new(),
        };
        let package = validator.validate(None)?;
        Ok(ValidatedPicklePlan {
            plan: self,
            package,
            context_closures: validator.context_closures,
            context_defaults: validator.context_defaults,
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
    signature_only: bool,
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
        mutable_counter_assignment: methods.first().and_then(|method| {
            class
                .typed_bindings
                .mutable_counter_reference_assignment(class.source.id, method.symbol)
        }),
        conditional_mutable_assignment: methods.first().and_then(|method| {
            class
                .typed_bindings
                .conditional_mutable_reference_assignment(class.source.id, method.symbol)
        }),
        local_bindings: HashMap::new(),
        pattern_bindings: HashMap::new(),
        registered_local_symbols: HashSet::new(),
        seen_local_bindings: HashSet::new(),
        seen_local_symbols: HashSet::new(),
        visible_locals: HashSet::new(),
        context_closures: HashMap::new(),
        context_defaults: HashMap::new(),
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
        let declaration = class
            .typed_bindings
            .mutable_counter_local_initializer(class.source.id, method.symbol);
        let assignment = class
            .typed_bindings
            .mutable_counter_reference_assignment(class.source.id, method.symbol);
        if declaration.is_some() != assignment.is_some() {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::BodyExpression,
                symbol: Some(method.symbol),
            });
        }
        let patterns = class
            .typed_bindings
            .pattern_value_bindings(class.source.id, method.symbol);
        validator.charge(patterns.len())?;
        let mut sources = HashSet::new();
        let mut symbols = HashSet::new();
        for binding in &patterns {
            if !sources.insert(binding.source_tree)
                || !symbols.insert(binding.symbol)
                || validator
                    .pattern_bindings
                    .insert(binding.source_tree, *binding)
                    .is_some()
            {
                return Err(PickleError::DuplicateDeclaration {
                    symbol: binding.symbol,
                });
            }
        }
        validator.method(method, signature_only)?;
        if !signature_only {
            for binding in patterns {
                if !validator.seen_local_symbols.contains(&binding.symbol) {
                    return Err(PickleError::SourceDefinition {
                        symbol: binding.symbol,
                    });
                }
            }
        }
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

pub(crate) fn validate_class_state_mutable_declaration_fragment(
    class: &crate::ClassStatePicklePlan<'_>,
    method_symbol: SymbolId,
    binding: MutableCounterLocalInitializerBinding,
    reference_fields: &[SymbolId],
) -> Result<(), PickleError> {
    let (method, local_values) =
        crate::class_state_output::method_input_for_fragment(class, method_symbol)?;
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
        source_root: class.class_tree,
        class: class.class,
        class_tree: class.class_tree,
        constructor,
        methods: std::slice::from_ref(&method),
        local_values: &local_values,
    };
    let mut validator = Validator {
        plan: &plan,
        limits: ValidationLimits::default(),
        remaining: ValidationLimits::default().max_items,
        class_members: Some(class.members),
        reference_fields,
        planned_class_symbols: class.typed_bindings.planned_class_symbols(),
        mutable_counter_assignment: None,
        conditional_mutable_assignment: None,
        local_bindings: HashMap::new(),
        pattern_bindings: HashMap::new(),
        registered_local_symbols: HashSet::new(),
        seen_local_bindings: HashSet::new(),
        seen_local_symbols: HashSet::new(),
        visible_locals: HashSet::new(),
        context_closures: HashMap::new(),
        context_defaults: HashMap::new(),
    };
    for local in &local_values {
        if validator
            .local_bindings
            .insert(local.source_tree, *local)
            .is_some()
            || !validator.registered_local_symbols.insert(local.symbol)
        {
            return Err(PickleError::DuplicateDeclaration {
                symbol: local.symbol,
            });
        }
    }
    validator.method(&method, true)?;
    validator.validate_mutable_counter_declaration_fragment(binding)?;
    Ok(())
}

pub(crate) fn validate_class_state_mutable_reference_assignment_fragment(
    class: &crate::ClassStatePicklePlan<'_>,
    method_symbol: SymbolId,
    declaration: MutableCounterLocalInitializerBinding,
    assignment: MutableCounterReferenceAssignmentBinding,
    reference_fields: &[SymbolId],
) -> Result<(), PickleError> {
    let (method, local_values) =
        crate::class_state_output::method_input_for_fragment(class, method_symbol)?;
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
        source_root: class.class_tree,
        class: class.class,
        class_tree: class.class_tree,
        constructor,
        methods: std::slice::from_ref(&method),
        local_values: &local_values,
    };
    let mut validator = Validator {
        plan: &plan,
        limits: ValidationLimits::default(),
        remaining: ValidationLimits::default().max_items,
        class_members: Some(class.members),
        reference_fields,
        planned_class_symbols: class.typed_bindings.planned_class_symbols(),
        mutable_counter_assignment: Some(assignment),
        conditional_mutable_assignment: None,
        local_bindings: HashMap::new(),
        pattern_bindings: HashMap::new(),
        registered_local_symbols: HashSet::new(),
        seen_local_bindings: HashSet::new(),
        seen_local_symbols: HashSet::new(),
        visible_locals: HashSet::new(),
        context_closures: HashMap::new(),
        context_defaults: HashMap::new(),
    };
    for local in &local_values {
        if validator
            .local_bindings
            .insert(local.source_tree, *local)
            .is_some()
            || !validator.registered_local_symbols.insert(local.symbol)
        {
            return Err(PickleError::DuplicateDeclaration {
                symbol: local.symbol,
            });
        }
    }
    validator.method(&method, true)?;
    validator.validate_mutable_counter_declaration_fragment(declaration)?;
    validator.validate_mutable_counter_reference_assignment_fragment(declaration, assignment)
}

struct Validator<'p, 'a> {
    plan: &'p PicklePlan<'a>,
    limits: ValidationLimits,
    remaining: usize,
    class_members: Option<&'a [ClassMemberPicklePlan]>,
    reference_fields: &'a [SymbolId],
    planned_class_symbols: &'a [SymbolId],
    mutable_counter_assignment: Option<MutableCounterReferenceAssignmentBinding>,
    conditional_mutable_assignment: Option<ConditionalMutableReferenceAssignmentBinding>,
    local_bindings: HashMap<TreeId<Untyped>, LocalValueBinding>,
    pattern_bindings: HashMap<TreeId<Untyped>, LocalValueBinding>,
    registered_local_symbols: HashSet<SymbolId>,
    seen_local_bindings: HashSet<TreeId<Untyped>>,
    seen_local_symbols: HashSet<SymbolId>,
    visible_locals: HashSet<SymbolId>,
    context_closures: HashMap<SymbolId, dotty_core::CheckedUnaryContextClosure>,
    context_defaults: HashMap<SymbolId, ContextualDefault>,
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
            || !(record.flags.is_empty()
                || (kind == SymbolKind::Parameter
                    && matches!(record.flags, SymbolFlags::GIVEN | SymbolFlags::IMPLICIT)))
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
        let mut contextual = SymbolFlags::EMPTY;
        for modifier in &metadata.modifiers {
            match modifier {
                dotty_core::ast::Modifier::Param if !saw_param => saw_param = true,
                dotty_core::ast::Modifier::Given if contextual.is_empty() => {
                    contextual = SymbolFlags::GIVEN
                }
                dotty_core::ast::Modifier::Implicit if contextual.is_empty() => {
                    contextual = SymbolFlags::IMPLICIT
                }
                _ => return Err(self.unsupported(UnsupportedFeature::SourceMetadata, Some(symbol))),
            }
        }
        if self.symbol(symbol)?.flags != contextual {
            return Err(self.unsupported(UnsupportedFeature::SymbolMetadata, Some(symbol)));
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
                if parameter.rhs.is_some()
                    && !self
                        .context_defaults
                        .get(&method)
                        .is_some_and(|d| d.defaulted == symbol)
                {
                    return Err(
                        self.unsupported(UnsupportedFeature::ParameterClauses, Some(symbol))
                    );
                }
                let TreeKind::Ident(parameter_type) = &self.source_tree(parameter.tpt)?.kind else {
                    return Err(
                        self.unsupported(UnsupportedFeature::ParameterClauses, Some(symbol))
                    );
                };
                let declared_type = if parameter_type.name == self.symbol(int)?.name {
                    Some(self.plan.definitions.int)
                } else if parameter_type.name == self.symbol(boolean)?.name {
                    Some(self.plan.definitions.boolean)
                } else {
                    None
                };
                self.source_symbol(
                    symbol,
                    tree,
                    SymbolKind::Parameter,
                    *parameter.name.as_name(),
                )?;
                self.owner(symbol, Some(method))?;
                let ty = self.complete(symbol)?;
                if !declared_type.is_some_and(|declared| declared == ty)
                    && !self.is_planned_sibling_class_type(ty, Some(parameter_type.name))
                {
                    return Err(PickleError::UnsupportedTypeReference { ty });
                }
                if self.scope(method)?.lookup_all(parameter.name.as_name()) != [symbol] {
                    return Err(PickleError::ScopeMembers { owner: method });
                }
            }
        }
        Ok(())
    }

    fn validate(
        &mut self,
        fragment: Option<MutableCounterLocalInitializerBinding>,
    ) -> Result<SymbolId, PickleError> {
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
        // Check constructor inputs before class-scope exact-membership checks:
        // an accessor-bearing `val`/`var` parameter adds a legitimate field to
        // that scope, but this serializer slice must report it as an
        // unsupported constructor form rather than an opaque scope mismatch.
        let has_two_reference_parameters = self.constructor(template.constructor)?;
        for method in p.methods {
            if let Some(default) = self.contextual_default(method)? {
                if !expected.insert(default.getter) {
                    return Err(PickleError::DuplicateDeclaration {
                        symbol: default.getter,
                    });
                }
                self.context_defaults.insert(method.symbol, default);
            }
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
        if has_two_reference_parameters && !p.methods.is_empty() {
            return Err(self.unsupported(
                UnsupportedFeature::PrimaryConstructorInput,
                Some(p.constructor.symbol),
            ));
        }
        if let Some(binding) = fragment
            && !p
                .methods
                .iter()
                .any(|method| binding.method == method.symbol)
        {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(binding.method)));
        }
        for method in p.methods {
            self.method(method, fragment.is_some())?;
        }
        if let Some(binding) = fragment {
            self.validate_mutable_counter_declaration_fragment(binding)?;
        } else if self.seen_local_bindings.len() != p.local_values.len() {
            let binding = p
                .local_values
                .iter()
                .find(|binding| !self.seen_local_bindings.contains(&binding.source_tree))
                .expect("binding count mismatch has an unseen binding");
            return Err(PickleError::SourceDefinition {
                symbol: binding.symbol,
            });
        }
        if p.methods.is_empty() && !has_two_reference_parameters {
            return Err(self.unsupported(UnsupportedFeature::MethodCount, Some(p.class)));
        }
        Ok(package)
    }

    fn validate_mutable_counter_declaration_fragment(
        &mut self,
        binding: MutableCounterLocalInitializerBinding,
    ) -> Result<(), PickleError> {
        let p = self.plan;
        let method = p
            .methods
            .iter()
            .find(|method| method.symbol == binding.method)
            .ok_or(PickleError::SourceDefinition {
                symbol: binding.method,
            })?;
        let block_source = self.source_tree(binding.source_block)?;
        let block_typed =
            p.typed_ast
                .try_get(binding.typed_block)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: binding.typed_block,
                })?;
        let (TreeKind::Block(source_block), TreeKind::Block(typed_block)) =
            (&block_source.kind, &block_typed.kind)
        else {
            return Err(PickleError::SourceShape {
                tree: binding.source_block,
                expected: SourceRole::Body,
            });
        };
        self.source_typed_binding(binding.source_block, binding.typed_block)?;
        self.paired_source_typed_span(
            block_source.position,
            block_typed.position,
            binding.typed_block,
        )?;
        let source_method = self.source_tree(method.source_tree)?;
        let TreeKind::DefDef(definition) = &source_method.kind else {
            return Err(PickleError::SourceShape {
                tree: method.source_tree,
                expected: SourceRole::Method,
            });
        };
        if binding.source_id != p.source.id
            || binding.owner != p.class
            || binding.method != method.symbol
            || definition.rhs != Some(binding.source_block)
            || method.body != Some(binding.typed_block)
            || source_block.stats.len() != 5
            || typed_block.stats.len() != source_block.stats.len()
            || source_block.stats[..2] != binding.prefix_source_declarations
            || typed_block.stats[..2] != binding.prefix_typed_declarations
            || source_block.stats[2] != binding.source_declaration
            || typed_block.stats[2] != binding.typed_declaration
        {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(binding.method)));
        }
        if binding.prefix_locals[0] == binding.prefix_locals[1]
            || !binding.prefix_locals.contains(&binding.target)
        {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(binding.method)));
        }
        let initializers = p
            .typed_bindings
            .local_counter_initializers(p.source.id, binding.method);
        if initializers.len() != binding.prefix_locals.len() {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(binding.method)));
        }
        let mut visited_initializers = HashSet::new();
        for index in 0..2 {
            self.validate_fragment_local(
                binding.prefix_source_declarations[index],
                binding.prefix_typed_declarations[index],
                binding.prefix_locals[index],
                binding,
                false,
                &mut visited_initializers,
            )?;
        }
        self.validate_fragment_local(
            binding.source_declaration,
            binding.typed_declaration,
            binding.local,
            binding,
            true,
            &mut visited_initializers,
        )?;
        Ok(())
    }

    fn validate_fragment_local(
        &mut self,
        source_id: TreeId<Untyped>,
        typed_id: TreeId<Typed>,
        symbol_id: SymbolId,
        proof: MutableCounterLocalInitializerBinding,
        selected: bool,
        visited_initializers: &mut HashSet<TreeId<Typed>>,
    ) -> Result<(), PickleError> {
        let p = self.plan;
        let local = p
            .local_values
            .iter()
            .find(|local| local.symbol == symbol_id)
            .copied()
            .ok_or(PickleError::SourceDefinition { symbol: symbol_id })?;
        if local.source_tree != source_id || local.typed_declaration != typed_id {
            return Err(PickleError::BodyBinding {
                source: p.source.id,
                tree: source_id,
                expected: local.typed_declaration,
                actual: p.typed_bindings.typed_at(p.source.id, source_id),
            });
        }
        let source = self.source_tree(source_id)?;
        let typed = p
            .typed_ast
            .try_get(typed_id)
            .ok_or(PickleError::InvalidTypedTree { tree: typed_id })?;
        let (TreeKind::ValDef(source_val), TreeKind::ValDef(typed_val)) =
            (&source.kind, &typed.kind)
        else {
            return Err(PickleError::SourceShape {
                tree: source_id,
                expected: SourceRole::Body,
            });
        };
        self.source_typed_binding(source_id, typed_id)?;
        self.paired_source_typed_span(source.position, typed.position, typed_id)?;
        let symbol = self.symbol(symbol_id)?;
        let expected_modifier = if selected {
            &[dotty_core::ast::Modifier::Var][..]
        } else {
            &[][..]
        };
        let expected_flags = if selected {
            SymbolFlags::MUTABLE
        } else {
            SymbolFlags::EMPTY
        };
        let ty = self.complete(symbol_id)?;
        if source_val.metadata.modifiers.as_slice() != expected_modifier
            || source_val.metadata.visibility.is_some()
            || !source_val.metadata.annotations.is_empty()
            || symbol.flags != expected_flags
            || symbol.owner != Some(proof.method)
            || symbol.origin != SymbolOrigin::Source(p.source.id)
            || symbol.kind != SymbolKind::Local
            || !self.same_reference_type(ty, proof.local_type)?
            || typed.ty != ty
            || symbol.visibility != Visibility::Public
            || !symbol.annotations.is_empty()
            || source_val.name != typed_val.name
            || *source_val.name.as_name() != symbol.name
        {
            return Err(self.unsupported(UnsupportedFeature::SymbolMetadata, Some(symbol_id)));
        }
        if !p.store.scopes.contains(local.scope)
            || local.scope != proof.scope
            || p.store.scopes.get(local.scope).owner != Some(proof.method)
            || p.store.scopes.get(local.scope).lookup_all(&symbol.name) != [symbol_id]
        {
            return Err(PickleError::ScopeMembers {
                owner: proof.method,
            });
        }
        let source_tpt = self.source_tree(source_val.tpt)?;
        let typed_tpt =
            p.typed_ast
                .try_get(typed_val.tpt)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: typed_val.tpt,
                })?;
        let Type::TypeRef {
            target: TypeRefTarget::Symbol(class),
            ..
        } = self.ty(ty)?
        else {
            return Err(PickleError::UnsupportedTypeReference { ty });
        };
        if !self.is_planned_sibling_class_type(ty, None)
            || !matches!(&source_tpt.kind, TreeKind::Ident(name) if name.name == self.symbol(*class)?.name)
            || !matches!(typed_tpt.kind, TreeKind::TypeTree(_))
            || typed_tpt.ty != ty
        {
            return Err(PickleError::UnsupportedTypeReference { ty });
        }
        self.source_typed_binding(source_val.tpt, typed_val.tpt)?;
        self.paired_source_typed_span(source_tpt.position, typed_tpt.position, typed_val.tpt)?;
        let source_rhs = source_val.rhs.ok_or(PickleError::MissingBody {
            method: proof.method,
        })?;
        let typed_rhs = typed_val.rhs.ok_or(PickleError::MissingBody {
            method: proof.method,
        })?;
        if (source_id == proof.source_declaration && source_rhs != proof.source_initializer)
            || (typed_id == proof.typed_declaration && typed_rhs != proof.typed_initializer)
            || p.typed_bindings.typed_at(p.source.id, source_rhs) != Some(typed_rhs)
        {
            return Err(PickleError::BodyBinding {
                source: p.source.id,
                tree: source_rhs,
                expected: typed_rhs,
                actual: p.typed_bindings.typed_at(p.source.id, source_rhs),
            });
        }
        if selected {
            if !visited_initializers.insert(typed_rhs) {
                return Err(PickleError::RepeatedExpression { tree: typed_rhs });
            }
            let typed_initializer = p
                .typed_ast
                .try_get(typed_rhs)
                .ok_or(PickleError::InvalidTypedTree { tree: typed_rhs })?;
            let source_initializer = self.source_tree(source_rhs)?;
            if source_initializer.position.is_none() || typed_initializer.position.is_none() {
                return Err(PickleError::BodySpan { tree: typed_rhs });
            }
            self.paired_source_typed_span(
                source_initializer.position,
                typed_initializer.position,
                typed_rhs,
            )?;
            let (TreeKind::Ident(source_ident), TreeKind::Ident(typed_ident)) =
                (&source_initializer.kind, &typed_initializer.kind)
            else {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(proof.method))
                );
            };
            let Type::TermRef {
                prefix,
                target: TermRefTarget::Symbol(target),
            } = self.ty(typed_initializer.ty)?
            else {
                return Err(PickleError::BodyType {
                    tree: typed_rhs,
                    ty: typed_initializer.ty,
                });
            };
            if *target != proof.target
                || *prefix != p.definitions.no_prefix
                || !proof.prefix_locals.contains(target)
                || !self.visible_locals.contains(target)
                || source_ident.name != self.symbol(*target)?.name
                || typed_ident.name != self.symbol(*target)?.name
            {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(proof.method))
                );
            }
        } else {
            self.validate_fragment_predecessor_initializer(
                FragmentPredecessorInitializer {
                    source_declaration: source_id,
                    typed_declaration: typed_id,
                    local: symbol_id,
                    proof,
                    source_initializer: source_rhs,
                    typed_initializer: typed_rhs,
                },
                visited_initializers,
            )?;
        }
        if !self.seen_local_bindings.insert(source_id) || !self.seen_local_symbols.insert(symbol_id)
        {
            return Err(PickleError::DuplicateDeclaration { symbol: symbol_id });
        }
        self.visible_locals.insert(symbol_id);
        Ok(())
    }

    fn validate_fragment_predecessor_initializer(
        &mut self,
        fragment: FragmentPredecessorInitializer,
        visited: &mut HashSet<TreeId<Typed>>,
    ) -> Result<(), PickleError> {
        let FragmentPredecessorInitializer {
            source_declaration,
            typed_declaration,
            local,
            proof,
            source_initializer,
            typed_initializer,
        } = fragment;
        let p = self.plan;
        let declarations = p
            .typed_bindings
            .local_counter_initializers(p.source.id, proof.method);
        let mut matching = declarations
            .iter()
            .filter(|initializer| initializer.local == local);
        let initializer = matching.next().copied().ok_or_else(|| {
            self.unsupported(UnsupportedFeature::BodyExpression, Some(proof.method))
        })?;
        if matching.next().is_some()
            || initializer.source_block != proof.source_block
            || initializer.typed_block != proof.typed_block
            || initializer.source_declaration != source_declaration
            || initializer.typed_declaration != typed_declaration
            || initializer.source_initializer != source_initializer
            || initializer.typed_initializer != typed_initializer
            || initializer.local != local
            || initializer.scope != proof.scope
            || !self.same_reference_type(initializer.local_type, proof.local_type)?
            || initializer.call.source_apply != source_initializer
            || initializer.call.typed_apply != typed_initializer
        {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(proof.method)));
        }
        let source_declaration_node = self.source_tree(source_declaration)?;
        let typed_declaration_node =
            p.typed_ast
                .try_get(typed_declaration)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: typed_declaration,
                })?;
        let (TreeKind::ValDef(source_val), TreeKind::ValDef(typed_val)) =
            (&source_declaration_node.kind, &typed_declaration_node.kind)
        else {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(proof.method)));
        };
        if initializer.source_type_tree != source_val.tpt
            || initializer.typed_type_tree != typed_val.tpt
        {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(proof.method)));
        }
        let source = self.source_tree(source_initializer)?;
        let typed =
            p.typed_ast
                .try_get(typed_initializer)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: typed_initializer,
                })?;
        self.source_typed_binding(source_initializer, typed_initializer)?;
        self.paired_source_typed_span(source.position, typed.position, typed_initializer)?;
        let method = p
            .methods
            .iter()
            .find(|method| method.symbol == proof.method)
            .ok_or(PickleError::SourceDefinition {
                symbol: proof.method,
            })?;
        if !visited.insert(typed_initializer) {
            return Err(PickleError::RepeatedExpression {
                tree: typed_initializer,
            });
        }
        self.validate_nested_constructor_argument(
            initializer.call,
            initializer.argument_formal,
            proof.local_type,
            method,
            1,
            visited,
        )?;
        let (TreeKind::New(source_new), TreeKind::New(typed_new)) = (
            &self.source_tree(initializer.call.source_new)?.kind,
            &p.typed_ast
                .try_get(initializer.call.typed_new)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: initializer.call.typed_new,
                })?
                .kind,
        ) else {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(proof.method)));
        };
        for (source_id, typed_id) in [
            (source_initializer, typed_initializer),
            (
                initializer.call.source_select,
                initializer.call.typed_select,
            ),
            (initializer.call.source_new, initializer.call.typed_new),
            (source_new.tpt, typed_new.tpt),
            (
                initializer.call.source_argument,
                initializer.call.typed_argument,
            ),
        ] {
            let source = self.source_tree(source_id)?;
            let typed = p
                .typed_ast
                .try_get(typed_id)
                .ok_or(PickleError::InvalidTypedTree { tree: typed_id })?;
            if source.position.is_none() || typed.position.is_none() {
                return Err(PickleError::BodySpan { tree: typed_id });
            }
            self.source_typed_binding(source_id, typed_id)?;
            self.paired_source_typed_span(source.position, typed.position, typed_id)?;
        }
        Ok(())
    }

    fn constructor(&mut self, source_tree: TreeId<Untyped>) -> Result<bool, PickleError> {
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

    fn method(
        &mut self,
        method: &MethodPicklePlan,
        signature_only: bool,
    ) -> Result<(), PickleError> {
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
            return self.polymorphic_evidence_method(definition, method, signature_only);
        }
        self.parameters(method.symbol, definition, &method.parameter_clauses)?;
        let mutable_rebinding_method = self
            .plan
            .typed_bindings
            .mutable_counter_reference_assignment(p.source.id, method.symbol)
            .is_some_and(|assignment| {
                assignment.method == method.symbol
                    && self
                        .plan
                        .typed_bindings
                        .mutable_counter_local_initializer(p.source.id, method.symbol)
                        .is_some_and(|declaration| {
                            declaration.method == method.symbol
                                && declaration.source_block == assignment.source_block
                                && declaration.typed_block == assignment.typed_block
                        })
            });
        let curried = method.parameter_clauses.len() > 1;
        if (curried
            && !method.parameter_clauses.iter().flatten().any(|&parameter| {
                matches!(
                    p.store.symbols.get(parameter).flags,
                    SymbolFlags::GIVEN | SymbolFlags::IMPLICIT
                )
            }))
            || (definition.source_param_clause_order.is_some() && !mutable_rebinding_method)
        {
            return Err(self.unsupported(UnsupportedFeature::ParameterClauses, Some(method.symbol)));
        }
        let ty = self.complete(method.symbol)?;
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
        let mut result_type = ty;
        for parameters in &method.parameter_clauses {
            let Type::Method(signature) = self.ty(result_type)? else {
                return Err(PickleError::MalformedType {
                    ty,
                    role: if parameters.is_empty() {
                        TypeRole::MethodResult
                    } else {
                        TypeRole::Parameter
                    },
                });
            };
            if (parameters.is_empty() && signature.kind != MethodKind::Plain)
                || signature.params.len() != parameters.len()
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
                && (curried
                    || !((planned_class_parameters == 1 && signature.params.len() == 1)
                        || (planned_class_parameters == 1
                            && signature.params.len() == 2
                            && self.is_planned_sibling_class_type(signature.params[0].ty, None)
                            && signature.params[1].ty == p.definitions.int
                            && self.is_planned_sibling_class_type(signature.result, None)
                            && !self.same_planned_sibling_class_type(
                                signature.params[0].ty,
                                signature.result,
                            ))
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
                            ))))
            {
                return Err(PickleError::MalformedType {
                    ty,
                    role: TypeRole::Parameter,
                });
            }
            for (formal, &parameter) in signature.params.iter().zip(parameters) {
                let expected_kind = match self.symbol(parameter)?.flags {
                    SymbolFlags::GIVEN => MethodKind::Contextual,
                    SymbolFlags::IMPLICIT => MethodKind::Implicit,
                    _ => MethodKind::Plain,
                };
                if signature.kind != expected_kind
                    || formal.name.as_name() != &self.symbol(parameter)?.name
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
            result_type = signature.result;
        }
        if matches!(
            self.source_tree(definition.tpt)?.kind,
            TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(_))
        ) && self.class_members.is_none()
        {
            return self.context_result_closure(definition, method, result_type);
        }
        let TreeKind::Ident(result) = &self.source_tree(definition.tpt)?.kind else {
            return Err(self.unsupported(UnsupportedFeature::ResultTypeSyntax, Some(method.symbol)));
        };
        self.name(result.name)?;
        let supports_result = |validator: &Self, result_ty| {
            result_ty == p.definitions.int
                || result_ty == p.definitions.boolean
                || result_ty == p.definitions.unit
                || validator.is_planned_sibling_class_type(result_ty, Some(result.name))
        };
        if !supports_result(self, result_type) {
            return Err(PickleError::MalformedType {
                ty: result_type,
                role: TypeRole::MethodResult,
            });
        }
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
        if signature_only {
            if definition.rhs.is_none() || method.body.is_none() {
                return Err(PickleError::MissingBody {
                    method: method.symbol,
                });
            }
            Ok(())
        } else {
            self.validate_method_expression(definition.rhs, method.body, method, result_type)
        }
    }
}
