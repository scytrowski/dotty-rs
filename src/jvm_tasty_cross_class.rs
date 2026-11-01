//! Read-only source/typed proofs for bounded same-batch cross-class calls used
//! by the JVM/TASTy producer sequence.
//!
//! This validates source and typed provenance only. It does not lower or
//! serialize a cross-class TASTy signature or method body.

use std::fmt;

use dotty_core::ast::{ApplyKind, AstArena, TreeKind, Typed, TypedAst, Untyped};
use dotty_core::{
    Definitions, SemanticStore, SourceDefinition, SourceId, SourceSemanticIndex, SourceTypedIndex,
    SymbolFlags, SymbolId, SymbolInfo, SymbolKind, SymbolOrigin, TermRefTarget, TreeId, Type,
    TypeId, TypeRefTarget,
};
use dotty_jvm::{JvmClassPlan, JvmCompilationUnit};
use dotty_typer::{SourceTyper, TypedLocalBinding};

/// A typed failure while validating canonical constructor-call proofs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrossClassConstructorCallProofError {
    /// The source does not have the required complete class/method shape.
    SourceShape(&'static str),
    /// A source declaration is not bound to the canonical identity in this session.
    SourceIdentity(&'static str),
    /// A symbol has the wrong owner, visibility, flags, or semantic type.
    SemanticIdentity(&'static str),
    /// The exact source-to-typed mapping or canonical typed tree is invalid.
    TypedBinding(&'static str),
    /// The complete JVM unit does not contain the source-proven constructor/method.
    JvmPlan(&'static str),
}

impl fmt::Display for CrossClassConstructorCallProofError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cross-class constructor-call proof failed: {self:?}")
    }
}

impl std::error::Error for CrossClassConstructorCallProofError {}

/// Borrowed inputs for a structural source/typed constructor-call proof scan.
pub struct CrossClassConstructorCallProofInput<'session> {
    /// Source identity used by the parser and namer.
    pub source_id: SourceId,
    /// Exact package root containing the complete source batch.
    pub source_root: TreeId<Untyped>,
    /// Canonical semantic symbols and types for this session.
    pub store: &'session SemanticStore,
    /// Canonical primitive definitions for this session.
    pub definitions: &'session Definitions,
    /// Source trees from this session.
    pub source_ast: &'session AstArena<Untyped>,
    /// Typed trees from this session.
    pub typed_ast: &'session TypedAst,
    /// Canonical source declaration and scope mappings from this session.
    pub source_index: &'session SourceSemanticIndex,
    /// Exact source-to-typed tree mappings from this session.
    pub source_typed_index: &'session SourceTypedIndex,
    /// Complete JVM plans from the same semantic and typed session.
    pub jvm_unit: &'session JvmCompilationUnit<'session>,
}

/// Immutable proof for one source method that passes its exact formal to the
/// selected primary constructor of another planned class and returns that
/// constructed class. The receipt borrows the original one-session stores via
/// its lifetime and preserves source and typed roots for downstream TASTy
/// signature/body validation.
pub struct ValidatedCrossClassConstructorCallProof<'session> {
    _store: &'session SemanticStore,
    _source_ast: &'session AstArena<Untyped>,
    _typed_ast: &'session TypedAst,
    _source_index: &'session SourceSemanticIndex,
    _source_typed_index: &'session SourceTypedIndex,
    _jvm_unit: &'session JvmCompilationUnit<'session>,
    source_id: SourceId,
    caller_class: SymbolId,
    method: SymbolId,
    method_formal: SymbolId,
    formal_class: Option<SymbolId>,
    target_class: SymbolId,
    constructor: SymbolId,
    source_rhs: TreeId<Untyped>,
    source_select: TreeId<Untyped>,
    source_new: TreeId<Untyped>,
    source_argument: TreeId<Untyped>,
    typed_rhs: TreeId<Typed>,
    typed_select: TreeId<Typed>,
    typed_new: TreeId<Typed>,
    typed_argument: TreeId<Typed>,
    argument_proof: ConstructorArgumentProof,
    formal_type: TypeId,
    constructor_formal_type: TypeId,
    result_type: TypeId,
    method_descriptor: String,
    constructor_descriptor: String,
}

/// Supported shape of a validated cross-class constructor argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstructorArgumentProofKind {
    /// The outer constructor receives the calling method's exact formal.
    Formal,
    /// The outer constructor receives one nested `new Target(formal)` call.
    NestedNew,
    /// The outer constructor receives two ordered sibling `new Target(formal)` calls.
    NestedNewPair,
    /// The outer constructor receives two ordered original method formal references.
    DirectFormalPair,
    /// The outer constructor receives a same-instance planned method result.
    PlannedCall,
    /// The outer constructor receives one initialized immutable local val.
    LocalVal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FormalConstructorArgumentProof {
    source_argument: TreeId<Untyped>,
    typed_argument: TreeId<Typed>,
    formal: SymbolId,
    formal_type: TypeId,
    result_type: TypeId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConstructorArgumentProofShape {
    Formal(FormalConstructorArgumentProof),
    NestedNew(NestedConstructorArgumentProof),
    OrderedPair(OrderedConstructorArgumentPairProof),
    PlannedCall(PlannedCallProof),
    LocalVal(LocalValConstructorArgumentProof),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OrderedConstructorArgumentPairProof {
    source_method_formals: [TreeId<Untyped>; 2],
    method_formals: [SymbolId; 2],
    method_formal_types: [TypeId; 2],
    source_constructor_formals: [TreeId<Untyped>; 2],
    constructor_formals: [SymbolId; 2],
    constructor_formal_types: [TypeId; 2],
    arguments: [OrderedConstructorArgumentProof; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OrderedConstructorArgumentProof {
    source_argument: TreeId<Untyped>,
    typed_argument: TreeId<Typed>,
    origin: OrderedConstructorArgumentOrigin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OrderedConstructorArgumentOrigin {
    Formal(FormalConstructorArgumentProof),
    NestedNew(NestedConstructorArgumentProof),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LocalValConstructorArgumentProof {
    source_block: TreeId<Untyped>,
    source_declaration: TreeId<Untyped>,
    source_initializer: TreeId<Untyped>,
    source_use: TreeId<Untyped>,
    typed_block: TreeId<Typed>,
    typed_declaration: TreeId<Typed>,
    typed_initializer: TreeId<Typed>,
    typed_use: TreeId<Typed>,
    symbol: SymbolId,
    scope: dotty_core::ScopeId,
    local_type: TypeId,
    result_type: TypeId,
    initializer_call: PlannedCallProof,
}

/// Opaque receipt for one validated constructor-argument expression shape.
/// Its private sum type prevents callers from manufacturing partially proved
/// variants while preserving a compact, explicit shape contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConstructorArgumentProof {
    shape: ConstructorArgumentProofShape,
}

impl ConstructorArgumentProof {
    pub const fn kind(self) -> ConstructorArgumentProofKind {
        match self.shape {
            ConstructorArgumentProofShape::Formal(_) => ConstructorArgumentProofKind::Formal,
            ConstructorArgumentProofShape::NestedNew(_) => ConstructorArgumentProofKind::NestedNew,
            ConstructorArgumentProofShape::OrderedPair(proof) => {
                if matches!(
                    proof.arguments[0].origin,
                    OrderedConstructorArgumentOrigin::NestedNew(_)
                ) && matches!(
                    proof.arguments[1].origin,
                    OrderedConstructorArgumentOrigin::NestedNew(_)
                ) {
                    ConstructorArgumentProofKind::NestedNewPair
                } else {
                    ConstructorArgumentProofKind::DirectFormalPair
                }
            }
            ConstructorArgumentProofShape::PlannedCall(_) => {
                ConstructorArgumentProofKind::PlannedCall
            }
            ConstructorArgumentProofShape::LocalVal(_) => ConstructorArgumentProofKind::LocalVal,
        }
    }

    pub const fn planned_call(self) -> Option<PlannedCallProof> {
        match self.shape {
            ConstructorArgumentProofShape::PlannedCall(proof) => Some(proof),
            _ => None,
        }
    }

    pub const fn local_val(self) -> Option<LocalValConstructorArgumentProofView> {
        match self.shape {
            ConstructorArgumentProofShape::LocalVal(proof) => {
                Some(LocalValConstructorArgumentProofView { proof })
            }
            _ => None,
        }
    }

    pub const fn formal(self) -> Option<FormalConstructorArgumentProofView> {
        match self.shape {
            ConstructorArgumentProofShape::Formal(proof) => {
                Some(FormalConstructorArgumentProofView {
                    source_argument: proof.source_argument,
                    typed_argument: proof.typed_argument,
                    formal: proof.formal,
                    formal_type: proof.formal_type,
                    result_type: proof.result_type,
                })
            }
            _ => None,
        }
    }

    /// Exact ordered receipts for a pair of independently validated nested
    /// constructor arguments. Singleton proofs keep their original shape.
    pub const fn nested_pair(self) -> Option<[NestedConstructorArgumentProof; 2]> {
        match self.shape {
            ConstructorArgumentProofShape::OrderedPair(proof) => {
                match (proof.arguments[0].origin, proof.arguments[1].origin) {
                    (
                        OrderedConstructorArgumentOrigin::NestedNew(first),
                        OrderedConstructorArgumentOrigin::NestedNew(second),
                    ) => Some([first, second]),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Exact ordered source/typed argument roots and their validated origins.
    pub const fn ordered_pair(self) -> Option<OrderedConstructorArgumentPairProofView> {
        match self.shape {
            ConstructorArgumentProofShape::OrderedPair(proof) => {
                Some(OrderedConstructorArgumentPairProofView { proof })
            }
            _ => None,
        }
    }

    /// Ordered outer signature identities retained alongside both branch receipts.
    pub const fn nested_pair_context(self) -> Option<NestedConstructorArgumentPairProofView> {
        match self.shape {
            ConstructorArgumentProofShape::OrderedPair(proof) => match self.nested_pair() {
                Some(arguments) => {
                    Some(NestedConstructorArgumentPairProofView { proof, arguments })
                }
                None => None,
            },
            _ => None,
        }
    }
}

/// Read-only outer formal identities for one validated two-argument receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NestedConstructorArgumentPairProofView {
    proof: OrderedConstructorArgumentPairProof,
    arguments: [NestedConstructorArgumentProof; 2],
}

impl NestedConstructorArgumentPairProofView {
    pub const fn source_method_formals(self) -> [TreeId<Untyped>; 2] {
        self.proof.source_method_formals
    }
    pub const fn method_formals(self) -> [SymbolId; 2] {
        self.proof.method_formals
    }
    pub const fn method_formal_types(self) -> [TypeId; 2] {
        self.proof.method_formal_types
    }
    pub const fn source_constructor_formals(self) -> [TreeId<Untyped>; 2] {
        self.proof.source_constructor_formals
    }
    pub const fn constructor_formals(self) -> [SymbolId; 2] {
        self.proof.constructor_formals
    }
    pub const fn constructor_formal_types(self) -> [TypeId; 2] {
        self.proof.constructor_formal_types
    }
    pub const fn arguments(self) -> [NestedConstructorArgumentProof; 2] {
        self.arguments
    }
}

/// One independently validated entry in an ordered constructor argument pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OrderedConstructorArgumentProofView {
    proof: OrderedConstructorArgumentProof,
}

impl OrderedConstructorArgumentProofView {
    pub const fn source_argument(self) -> TreeId<Untyped> {
        self.proof.source_argument
    }
    pub const fn typed_argument(self) -> TreeId<Typed> {
        self.proof.typed_argument
    }
    pub const fn formal(self) -> Option<FormalConstructorArgumentProofView> {
        match self.proof.origin {
            OrderedConstructorArgumentOrigin::Formal(proof) => {
                Some(FormalConstructorArgumentProofView {
                    source_argument: proof.source_argument,
                    typed_argument: proof.typed_argument,
                    formal: proof.formal,
                    formal_type: proof.formal_type,
                    result_type: proof.result_type,
                })
            }
            OrderedConstructorArgumentOrigin::NestedNew(_) => None,
        }
    }
    pub const fn nested_new(self) -> Option<NestedConstructorArgumentProof> {
        match self.proof.origin {
            OrderedConstructorArgumentOrigin::NestedNew(proof) => Some(proof),
            OrderedConstructorArgumentOrigin::Formal(_) => None,
        }
    }
}

/// Read-only ordered source, typed, formal, and constructor identities for a
/// two-argument constructor application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OrderedConstructorArgumentPairProofView {
    proof: OrderedConstructorArgumentPairProof,
}

impl OrderedConstructorArgumentPairProofView {
    pub const fn source_method_formals(self) -> [TreeId<Untyped>; 2] {
        self.proof.source_method_formals
    }
    pub const fn method_formals(self) -> [SymbolId; 2] {
        self.proof.method_formals
    }
    pub const fn method_formal_types(self) -> [TypeId; 2] {
        self.proof.method_formal_types
    }
    pub const fn source_constructor_formals(self) -> [TreeId<Untyped>; 2] {
        self.proof.source_constructor_formals
    }
    pub const fn constructor_formals(self) -> [SymbolId; 2] {
        self.proof.constructor_formals
    }
    pub const fn constructor_formal_types(self) -> [TypeId; 2] {
        self.proof.constructor_formal_types
    }
    pub const fn arguments(self) -> [OrderedConstructorArgumentProofView; 2] {
        [
            OrderedConstructorArgumentProofView {
                proof: self.proof.arguments[0],
            },
            OrderedConstructorArgumentProofView {
                proof: self.proof.arguments[1],
            },
        ]
    }
}

/// Read-only roots and identities for a validated one-local constructor flow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalValConstructorArgumentProofView {
    proof: LocalValConstructorArgumentProof,
}

impl LocalValConstructorArgumentProofView {
    pub const fn source_block(self) -> TreeId<Untyped> {
        self.proof.source_block
    }
    pub const fn source_declaration(self) -> TreeId<Untyped> {
        self.proof.source_declaration
    }
    pub const fn source_initializer(self) -> TreeId<Untyped> {
        self.proof.source_initializer
    }
    pub const fn source_use(self) -> TreeId<Untyped> {
        self.proof.source_use
    }
    pub const fn typed_block(self) -> TreeId<Typed> {
        self.proof.typed_block
    }
    pub const fn typed_declaration(self) -> TreeId<Typed> {
        self.proof.typed_declaration
    }
    pub const fn typed_initializer(self) -> TreeId<Typed> {
        self.proof.typed_initializer
    }
    pub const fn typed_use(self) -> TreeId<Typed> {
        self.proof.typed_use
    }
    pub const fn symbol(self) -> SymbolId {
        self.proof.symbol
    }
    pub const fn scope(self) -> dotty_core::ScopeId {
        self.proof.scope
    }
    pub const fn local_type(self) -> TypeId {
        self.proof.local_type
    }
    pub const fn result_type(self) -> TypeId {
        self.proof.result_type
    }
    pub const fn initializer_call(self) -> PlannedCallProof {
        self.proof.initializer_call
    }
}

/// Read-only identities and value type retained by a direct formal argument proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormalConstructorArgumentProofView {
    source_argument: TreeId<Untyped>,
    typed_argument: TreeId<Typed>,
    formal: SymbolId,
    formal_type: TypeId,
    result_type: TypeId,
}

impl FormalConstructorArgumentProofView {
    pub const fn source_argument(self) -> TreeId<Untyped> {
        self.source_argument
    }
    pub const fn typed_argument(self) -> TreeId<Typed> {
        self.typed_argument
    }
    pub const fn formal(self) -> SymbolId {
        self.formal
    }
    pub const fn formal_type(self) -> TypeId {
        self.formal_type
    }
    /// Type produced by the original argument expression, independent of the
    /// enclosing method's result type.
    pub const fn result_type(self) -> TypeId {
        self.result_type
    }
}

/// Exact original source and typed identities for one supported one-level
/// nested constructor argument. In a pair receipt, these values preserve the
/// call-site roots independently even when both calls select the same
/// constructor symbol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NestedConstructorArgumentProof {
    source_outer_argument: TreeId<Untyped>,
    typed_outer_argument: TreeId<Typed>,
    source_apply: TreeId<Untyped>,
    typed_apply: TreeId<Typed>,
    source_select: TreeId<Untyped>,
    source_new: TreeId<Untyped>,
    source_argument: TreeId<Untyped>,
    typed_select: TreeId<Typed>,
    typed_new: TreeId<Typed>,
    typed_argument: TreeId<Typed>,
    class: SymbolId,
    constructor: SymbolId,
    formal: SymbolId,
    argument_formal: SymbolId,
    formal_type: TypeId,
    argument_formal_type: TypeId,
    result_type: TypeId,
}

impl NestedConstructorArgumentProof {
    pub const fn source_outer_argument(self) -> TreeId<Untyped> {
        self.source_outer_argument
    }
    pub const fn typed_outer_argument(self) -> TreeId<Typed> {
        self.typed_outer_argument
    }
    pub const fn source_apply(self) -> TreeId<Untyped> {
        self.source_apply
    }
    pub const fn typed_apply(self) -> TreeId<Typed> {
        self.typed_apply
    }
    pub const fn source_select(self) -> TreeId<Untyped> {
        self.source_select
    }
    pub const fn source_new(self) -> TreeId<Untyped> {
        self.source_new
    }
    pub const fn source_argument(self) -> TreeId<Untyped> {
        self.source_argument
    }
    pub const fn typed_select(self) -> TreeId<Typed> {
        self.typed_select
    }
    pub const fn typed_new(self) -> TreeId<Typed> {
        self.typed_new
    }
    pub const fn typed_argument(self) -> TreeId<Typed> {
        self.typed_argument
    }
    pub const fn class(self) -> SymbolId {
        self.class
    }
    pub const fn constructor(self) -> SymbolId {
        self.constructor
    }
    pub const fn formal(self) -> SymbolId {
        self.formal
    }
    /// Exact caller-method formal named by the original inner argument leaf.
    pub const fn argument_formal(self) -> SymbolId {
        self.argument_formal
    }
    pub const fn formal_type(self) -> TypeId {
        self.formal_type
    }
    /// Type of the caller-method formal named by the original inner argument leaf.
    pub const fn argument_formal_type(self) -> TypeId {
        self.argument_formal_type
    }
    pub const fn result_type(self) -> TypeId {
        self.result_type
    }
}

/// Immutable provenance receipt for an exact same-instance helper call used
/// as a constructor argument. Source and typed roots are the original session
/// trees; owner and member are the canonical semantic identities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlannedCallProof {
    source_apply: TreeId<Untyped>,
    source_function: TreeId<Untyped>,
    source_argument: TreeId<Untyped>,
    source_receiver: Option<TreeId<Untyped>>,
    helper_source_tree: TreeId<Untyped>,
    helper_source_rhs: TreeId<Untyped>,
    typed_apply: TreeId<Typed>,
    typed_function: TreeId<Typed>,
    typed_argument: TreeId<Typed>,
    typed_receiver: Option<TreeId<Typed>>,
    helper_typed_rhs: TreeId<Typed>,
    owner: SymbolId,
    method: SymbolId,
    formal: SymbolId,
    helper_formal: SymbolId,
    formal_type: TypeId,
    result_type: TypeId,
}

#[derive(Clone, Copy, Debug)]
enum SourceArgumentShape {
    Formal {
        formal_tree: TreeId<Untyped>,
    },
    NestedNew {
        formal_tree: TreeId<Untyped>,
        select: TreeId<Untyped>,
        new: TreeId<Untyped>,
        type_tree: TreeId<Untyped>,
    },
    PlannedCall {
        formal_tree: TreeId<Untyped>,
        apply: TreeId<Untyped>,
        function: TreeId<Untyped>,
        receiver: Option<TreeId<Untyped>>,
    },
    LocalVal {
        declaration: TreeId<Untyped>,
        initializer: TreeId<Untyped>,
        formal_tree: TreeId<Untyped>,
        apply: TreeId<Untyped>,
        function: TreeId<Untyped>,
        receiver: Option<TreeId<Untyped>>,
    },
}

impl PlannedCallProof {
    pub const fn source_apply(self) -> TreeId<Untyped> {
        self.source_apply
    }
    pub const fn source_function(self) -> TreeId<Untyped> {
        self.source_function
    }
    pub const fn source_argument(self) -> TreeId<Untyped> {
        self.source_argument
    }
    pub const fn source_receiver(self) -> Option<TreeId<Untyped>> {
        self.source_receiver
    }
    pub const fn helper_source_tree(self) -> TreeId<Untyped> {
        self.helper_source_tree
    }
    pub const fn helper_source_rhs(self) -> TreeId<Untyped> {
        self.helper_source_rhs
    }
    pub const fn typed_apply(self) -> TreeId<Typed> {
        self.typed_apply
    }
    pub const fn typed_function(self) -> TreeId<Typed> {
        self.typed_function
    }
    pub const fn typed_argument(self) -> TreeId<Typed> {
        self.typed_argument
    }
    pub const fn typed_receiver(self) -> Option<TreeId<Typed>> {
        self.typed_receiver
    }
    pub const fn helper_typed_rhs(self) -> TreeId<Typed> {
        self.helper_typed_rhs
    }
    pub const fn owner(self) -> SymbolId {
        self.owner
    }
    pub const fn method(self) -> SymbolId {
        self.method
    }
    pub const fn formal(self) -> SymbolId {
        self.formal
    }
    pub const fn helper_formal(self) -> SymbolId {
        self.helper_formal
    }
    pub const fn formal_type(self) -> TypeId {
        self.formal_type
    }
    pub const fn result_type(self) -> TypeId {
        self.result_type
    }
}

impl ValidatedCrossClassConstructorCallProof<'_> {
    /// Frontend source identity shared by all declarations and tree roots.
    pub const fn source_id(&self) -> SourceId {
        self.source_id
    }
    /// Source class that owns the calling method.
    pub const fn caller_class(&self) -> SymbolId {
        self.caller_class
    }
    /// Canonical source method that owns the formal and RHS.
    pub const fn method(&self) -> SymbolId {
        self.method
    }
    /// Exact source formal referenced by the constructor argument.
    pub const fn method_formal(&self) -> SymbolId {
        self.method_formal
    }
    /// Planned class of the original argument type when the formal is a
    /// canonical class `TypeRef` (primitive legacy controls return `None`).
    pub const fn formal_class(&self) -> Option<SymbolId> {
        self.formal_class
    }
    /// Canonical planned class constructed by the method body.
    pub const fn target_class(&self) -> SymbolId {
        self.target_class
    }
    /// Canonical primary constructor selected by the typed `Select`.
    pub const fn constructor(&self) -> SymbolId {
        self.constructor
    }
    /// Original untyped method RHS root.
    pub const fn source_rhs(&self) -> TreeId<Untyped> {
        self.source_rhs
    }
    /// Original untyped constructor `Select` root.
    pub const fn source_select(&self) -> TreeId<Untyped> {
        self.source_select
    }
    /// Original untyped `New` root.
    pub const fn source_new(&self) -> TreeId<Untyped> {
        self.source_new
    }
    /// Original untyped argument root.
    pub const fn source_argument(&self) -> TreeId<Untyped> {
        self.source_argument
    }
    /// Exact typed method RHS retained by the JVM method plan.
    pub const fn typed_rhs(&self) -> TreeId<Typed> {
        self.typed_rhs
    }
    /// Exact typed constructor `Select` root.
    pub const fn typed_select(&self) -> TreeId<Typed> {
        self.typed_select
    }
    /// Exact typed `New` root.
    pub const fn typed_new(&self) -> TreeId<Typed> {
        self.typed_new
    }
    /// Exact typed argument root whose `TermRef` names `method_formal`.
    pub const fn typed_argument(&self) -> TreeId<Typed> {
        self.typed_argument
    }
    /// Opaque sum type identifying the fully validated argument shape.
    pub const fn argument_proof(&self) -> ConstructorArgumentProof {
        self.argument_proof
    }
    /// Exact nested constructor identities when this proof wraps a one-level
    /// constructor application. Retained for source compatibility.
    pub const fn nested_argument(&self) -> Option<NestedConstructorArgumentProof> {
        match self.argument_proof.shape {
            ConstructorArgumentProofShape::NestedNew(proof) => Some(proof),
            _ => None,
        }
    }
    /// Exact ordered roots for a two-argument nested constructor proof.
    pub const fn nested_argument_pair(&self) -> Option<[NestedConstructorArgumentProof; 2]> {
        self.argument_proof.nested_pair()
    }
    /// Exact ordered argument receipts for a two-argument constructor call.
    pub const fn ordered_argument_pair(&self) -> Option<OrderedConstructorArgumentPairProofView> {
        self.argument_proof.ordered_pair()
    }
    /// Canonical semantic type of the method formal and constructor argument.
    pub const fn formal_type(&self) -> TypeId {
        self.formal_type
    }
    /// Canonical type expected by the selected outer constructor. For a
    /// nested proof this is the inner constructed class; `formal_type` stays
    /// the original factory method parameter type.
    pub const fn constructor_formal_type(&self) -> TypeId {
        self.constructor_formal_type
    }
    /// Canonical semantic result type, equal to the constructed target class.
    pub const fn result_type(&self) -> TypeId {
        self.result_type
    }
    /// Exact erased JVM descriptor of the calling method.
    pub fn method_descriptor(&self) -> &str {
        &self.method_descriptor
    }
    /// Exact erased JVM descriptor of the selected primary constructor.
    pub fn constructor_descriptor(&self) -> &str {
        &self.constructor_descriptor
    }
    /// Semantic store, source/typed arenas, and JVM unit borrowed by this
    /// proof. The references are the exact same-session inputs validated here.
    pub fn session_inputs(
        &self,
    ) -> (
        &SemanticStore,
        &AstArena<Untyped>,
        &TypedAst,
        &SourceSemanticIndex,
        &SourceTypedIndex,
        &JvmCompilationUnit<'_>,
    ) {
        (
            self._store,
            self._source_ast,
            self._typed_ast,
            self._source_index,
            self._source_typed_index,
            self._jvm_unit,
        )
    }
}

/// Scan the complete source-ordered JVM batch for the supported constructor
/// argument shapes and validate each against canonical source, typed,
/// semantic, and JVM identities. A body with no candidate root is ignored;
/// once a structural candidate is present, every failed provenance or shape
/// check is returned rather than silently falling back to unproved planning.
pub fn validate_cross_class_constructor_call_proofs<'session>(
    input: CrossClassConstructorCallProofInput<'session>,
) -> Result<
    Vec<ValidatedCrossClassConstructorCallProof<'session>>,
    CrossClassConstructorCallProofError,
> {
    validate_cross_class_constructor_call_proofs_inner(input, None)
}

/// Variant used by the one-session paired planner, which can supply the
/// Typer-owned local binding index without reconstructing local identities.
pub fn validate_cross_class_constructor_call_proofs_with_typer<'session>(
    input: CrossClassConstructorCallProofInput<'session>,
    typer: &SourceTyper<'_>,
) -> Result<
    Vec<ValidatedCrossClassConstructorCallProof<'session>>,
    CrossClassConstructorCallProofError,
> {
    validate_cross_class_constructor_call_proofs_inner(input, Some(typer))
}

fn validate_cross_class_constructor_call_proofs_inner<'session>(
    input: CrossClassConstructorCallProofInput<'session>,
    typer: Option<&SourceTyper<'_>>,
) -> Result<
    Vec<ValidatedCrossClassConstructorCallProof<'session>>,
    CrossClassConstructorCallProofError,
> {
    let CrossClassConstructorCallProofInput {
        source_id,
        source_root,
        store,
        definitions,
        source_ast,
        typed_ast,
        source_index,
        source_typed_index,
        jvm_unit,
    } = input;
    if !std::ptr::eq(jvm_unit.store, store) || !std::ptr::eq(jvm_unit.typed_ast, typed_ast) {
        return Err(CrossClassConstructorCallProofError::SemanticIdentity(
            "JVM unit and source proof must borrow the same semantic and typed stores",
        ));
    }
    if let Some(typer) = typer
        && (!std::ptr::eq(typer.store(), store)
            || !std::ptr::eq(typer.typed_ast(), typed_ast)
            || !std::ptr::eq(typer.source_typed_index(), source_typed_index))
    {
        return Err(CrossClassConstructorCallProofError::TypedBinding(
            "local binding resolver must borrow the exact proof session stores and indexes",
        ));
    }
    let root =
        source_ast
            .try_get(source_root)
            .ok_or(CrossClassConstructorCallProofError::SourceShape(
                "package root is stale",
            ))?;
    let TreeKind::PackageDef(package) = &root.kind else {
        return Err(CrossClassConstructorCallProofError::SourceShape(
            "source root must be a package",
        ));
    };
    let package_owner = source_index.symbol_at(source_id, source_root).ok_or(
        CrossClassConstructorCallProofError::SourceIdentity(
            "package root is not bound to the supplied source identity",
        ),
    )?;
    if !store.symbols.contains(package_owner)
        || store.symbols.get(package_owner).kind != SymbolKind::Package
    {
        return Err(CrossClassConstructorCallProofError::SourceIdentity(
            "source root does not retain its canonical package symbol",
        ));
    }
    if package.stats.len() != jvm_unit.classes.len()
        || package.stats.is_empty()
        || package.stats.len() > 32
    {
        return Err(CrossClassConstructorCallProofError::JvmPlan(
            "JVM batch must contain 1 to 32 source classes exactly once in declaration order",
        ));
    }
    let metadata = jvm_unit.metadata(definitions).map_err(|_| {
        CrossClassConstructorCallProofError::JvmPlan("complete JVM batch metadata is invalid")
    })?;
    let mut proofs = Vec::new();
    let mut seen_methods = std::collections::HashSet::new();
    for (ordinal, &class_tree) in package.stats.iter().enumerate() {
        let TreeKind::TypeDef(class_def) = &source_ast
            .try_get(class_tree)
            .ok_or(CrossClassConstructorCallProofError::SourceShape(
                "source class declaration is stale",
            ))?
            .kind
        else {
            return Err(CrossClassConstructorCallProofError::SourceShape(
                "source package may contain only planned ordinary classes",
            ));
        };
        let caller_class = source_index.symbol_at(source_id, class_tree).ok_or(
            CrossClassConstructorCallProofError::SourceIdentity(
                "class declaration has no canonical symbol",
            ),
        )?;
        if source_index.definition_of(caller_class)
            != Some(SourceDefinition::Canonical {
                source: source_id,
                tree: class_tree,
            })
            || jvm_unit.classes[ordinal].class != caller_class
            || !store.symbols.contains(caller_class)
            || !has_source_origin(store, caller_class, source_id)
            || store.symbols.get(caller_class).kind != SymbolKind::Class
            || store.symbols.get(caller_class).owner != Some(package_owner)
        {
            return Err(CrossClassConstructorCallProofError::SourceIdentity(
                "source and JVM class order must retain the exact canonical class identities",
            ));
        }
        let TreeKind::Template(template) = &source_ast
            .try_get(class_def.rhs)
            .ok_or(CrossClassConstructorCallProofError::SourceShape(
                "class template is stale",
            ))?
            .kind
        else {
            return Err(CrossClassConstructorCallProofError::SourceShape(
                "class must have a template",
            ));
        };
        let caller_plan = unique_constructor_call_class_plan(jvm_unit, caller_class)?;
        for &method_tree in &template.body {
            let Some(TreeKind::DefDef(method_def)) =
                source_ast.try_get(method_tree).map(|node| &node.kind)
            else {
                continue;
            };
            let Some(source_rhs) = method_def.rhs else {
                continue;
            };
            if !is_direct_constructor_call_candidate(source_ast, source_rhs) {
                continue;
            }
            let method = source_index.symbol_at(source_id, method_tree).ok_or(
                CrossClassConstructorCallProofError::SourceIdentity(
                    "candidate method has no canonical source symbol",
                ),
            )?;
            if !seen_methods.insert(method) {
                return Err(CrossClassConstructorCallProofError::SourceIdentity(
                    "a source method cannot receive duplicate constructor-call proofs",
                ));
            }
            let proof = if method_def.value_param_clauses.len() == 1
                && method_def.value_param_clauses[0].len() == 2
            {
                validate_two_argument_constructor_call_candidate(
                    source_id,
                    caller_class,
                    &package.stats,
                    method_tree,
                    method_def,
                    method,
                    source_rhs,
                    store,
                    definitions,
                    source_ast,
                    typed_ast,
                    source_index,
                    source_typed_index,
                    jvm_unit,
                    caller_plan,
                    &metadata,
                )?
            } else {
                validate_constructor_call_candidate(
                    source_id,
                    caller_class,
                    &package.stats,
                    method_tree,
                    method_def,
                    method,
                    source_rhs,
                    store,
                    definitions,
                    source_ast,
                    typed_ast,
                    source_index,
                    source_typed_index,
                    typer,
                    jvm_unit,
                    caller_plan,
                    &metadata,
                )?
            };
            proofs.push(proof);
        }
    }
    for proof in &proofs {
        let argument_proof = proof.argument_proof();
        let planned_call = argument_proof.planned_call().or_else(|| {
            argument_proof
                .local_val()
                .map(LocalValConstructorArgumentProofView::initializer_call)
        });
        let Some(planned_call) = planned_call else {
            continue;
        };
        let helper_proof = proofs
            .iter()
            .find(|candidate| candidate.method() == planned_call.method())
            .ok_or(CrossClassConstructorCallProofError::JvmPlan(
                "planned helper body has no established same-batch constructor proof",
            ))?;
        if planned_call.owner() != proof.caller_class()
            || helper_proof.caller_class() != planned_call.owner()
            || helper_proof.source_id() != proof.source_id()
            || helper_proof.source_rhs() != planned_call.helper_source_rhs()
            || helper_proof.typed_rhs() != planned_call.helper_typed_rhs()
            || helper_proof.method_formal() != planned_call.helper_formal()
            || helper_proof.target_class()
                != match store.types.try_get(planned_call.result_type()) {
                    Some(Type::TypeRef {
                        target: TypeRefTarget::Symbol(class),
                        ..
                    }) => *class,
                    _ => {
                        return Err(CrossClassConstructorCallProofError::TypedBinding(
                            "planned helper result has no canonical class TypeRef",
                        ));
                    }
                }
            || helper_proof.argument_proof().kind() != ConstructorArgumentProofKind::Formal
        {
            return Err(CrossClassConstructorCallProofError::JvmPlan(
                "planned helper must retain its own direct-formal constructor proof in this source batch",
            ));
        }
    }
    Ok(proofs)
}

#[allow(clippy::too_many_arguments)]
fn validate_two_argument_constructor_call_candidate<'session>(
    source_id: SourceId,
    caller_class: SymbolId,
    source_class_trees: &[TreeId<Untyped>],
    method_tree: TreeId<Untyped>,
    method_def: &dotty_core::ast::DefDef<Untyped>,
    method: SymbolId,
    source_rhs: TreeId<Untyped>,
    store: &'session SemanticStore,
    definitions: &'session Definitions,
    source_ast: &'session AstArena<Untyped>,
    typed_ast: &'session TypedAst,
    source_index: &'session SourceSemanticIndex,
    source_typed_index: &'session SourceTypedIndex,
    jvm_unit: &'session JvmCompilationUnit<'session>,
    caller_plan: &JvmClassPlan,
    metadata: &[dotty_jvm::JvmClassMetadata],
) -> Result<ValidatedCrossClassConstructorCallProof<'session>, CrossClassConstructorCallProofError>
{
    use CrossClassConstructorCallProofError as Error;

    let source_error = |message| Error::SourceShape(message);
    let identity_error = |message| Error::SourceIdentity(message);
    let semantic_error = |message| Error::SemanticIdentity(message);
    let typed_error = |message| Error::TypedBinding(message);
    let caller_info = store.symbols.get(caller_class);
    let package_owner = caller_info
        .owner
        .ok_or_else(|| semantic_error("factory class has no package owner"))?;
    if source_index.symbol_at(source_id, method_tree) != Some(method)
        || source_index.definition_of(method)
            != Some(SourceDefinition::Canonical {
                source: source_id,
                tree: method_tree,
            })
        || !store.symbols.contains(method)
        || !has_source_origin(store, method, source_id)
        || store.symbols.get(method).kind != SymbolKind::Method
        || store.symbols.get(method).owner != Some(caller_class)
        || store.symbols.get(method).visibility != dotty_core::Visibility::Public
        || has_any_flag(
            store.symbols.get(method).flags,
            SymbolFlags::ABSTRACT
                | SymbolFlags::SYNTHETIC
                | SymbolFlags::STATIC
                | SymbolFlags::GIVEN
                | SymbolFlags::IMPLICIT
                | SymbolFlags::ERASED
                | SymbolFlags::EXTENSION,
        )
    {
        return Err(identity_error(
            "candidate method is not the canonical public source method",
        ));
    }
    if caller_info.kind != SymbolKind::Class
        || caller_plan.class != caller_class
        || method_def.metadata.visibility.is_some()
        || !method_def.type_params.is_empty()
        || method_def.value_param_clauses.len() != 1
        || method_def.value_param_clauses[0].len() != 2
    {
        return Err(source_error(
            "two-argument candidate must have one plain two-formal clause and a concrete public body",
        ));
    }
    require_source_position(
        source_ast,
        source_id,
        method_tree,
        "factory method declaration has a foreign source position",
    )?;

    let method_formal_trees = [
        method_def.value_param_clauses[0][0],
        method_def.value_param_clauses[0][1],
    ];
    let mut method_formals = [method; 2];
    let mut method_formal_types = [definitions.int; 2];
    let mut method_formal_classes = [None; 2];
    for index in 0..2 {
        require_source_position(
            source_ast,
            source_id,
            method_formal_trees[index],
            "factory formal has a foreign source position",
        )?;
        let TreeKind::ValDef(parameter) = &source_ast
            .try_get(method_formal_trees[index])
            .ok_or_else(|| source_error("factory formal source tree is stale"))?
            .kind
        else {
            return Err(source_error("factory formals must be value parameters"));
        };
        if parameter.rhs.is_some() {
            return Err(source_error("factory formals cannot have default values"));
        }
        let symbol = source_index
            .symbol_at(source_id, method_formal_trees[index])
            .ok_or_else(|| identity_error("factory formal has no canonical source symbol"))?;
        if source_index.definition_of(symbol)
            != Some(SourceDefinition::Canonical {
                source: source_id,
                tree: method_formal_trees[index],
            })
            || !store.symbols.contains(symbol)
            || !has_source_origin(store, symbol, source_id)
            || store.symbols.get(symbol).kind != SymbolKind::Parameter
            || store.symbols.get(symbol).owner != Some(method)
            || !method_formals[..index]
                .iter()
                .all(|earlier| *earlier != symbol)
        {
            return Err(identity_error(
                "factory formals must have distinct canonical parameter identities in source order",
            ));
        }
        let SymbolInfo::Complete(formal_type) = store.symbols.get(symbol).info else {
            return Err(semantic_error("factory formal type is incomplete"));
        };
        let formal_class =
            canonical_class_type_symbol(store, formal_type, package_owner, definitions.no_prefix);
        let reference_formal_is_supported = formal_class.is_some_and(|class| {
            class != caller_class
                && source_index.definition_of(class).is_some_and(|definition| {
                    matches!(definition, SourceDefinition::Canonical { source, .. } if source == source_id)
                })
                && unique_constructor_call_class_plan(jvm_unit, class).is_ok()
                && is_canonical_class_type_ref(
                    store,
                    formal_type,
                    class,
                    package_owner,
                    definitions.no_prefix,
                )
        });
        if formal_type != definitions.int && !reference_formal_is_supported {
            return Err(semantic_error(
                "factory formals must have canonical Int or planned same-package class types",
            ));
        }
        method_formals[index] = symbol;
        method_formal_types[index] = formal_type;
        method_formal_classes[index] = formal_class;
    }
    if method_formal_classes[0] != method_formal_classes[1] {
        return Err(semantic_error(
            "both factory formals must retain the same canonical reference class",
        ));
    }
    let SymbolInfo::Complete(method_type) = store.symbols.get(method).info else {
        return Err(semantic_error("factory method signature is incomplete"));
    };
    let Some(Type::Method(method_signature)) = store.types.try_get(method_type) else {
        return Err(semantic_error(
            "factory method must have one plain method type",
        ));
    };
    if method_signature.kind != dotty_core::MethodKind::Plain
        || method_signature.params.len() != 2
        || method_signature.params[0].ty != method_formal_types[0]
        || method_signature.params[1].ty != method_formal_types[1]
    {
        return Err(semantic_error(
            "factory signature must retain its two ordered reference formals",
        ));
    }

    let TreeKind::Apply(outer_apply) = &source_ast
        .try_get(source_rhs)
        .ok_or_else(|| source_error("factory method RHS is stale"))?
        .kind
    else {
        return Err(source_error(
            "factory body must be the original direct constructor Apply",
        ));
    };
    if outer_apply.kind != ApplyKind::Regular || outer_apply.args.len() != 2 {
        return Err(source_error(
            "outer constructor application must have exactly two regular arguments",
        ));
    }
    let source_select = outer_apply.function;
    let TreeKind::Select(source_selection) = &source_ast
        .try_get(source_select)
        .ok_or_else(|| source_error("outer constructor Select is stale"))?
        .kind
    else {
        return Err(source_error("outer constructor function must be Select"));
    };
    if store.names.resolve(source_selection.name.text()) != "<init>" {
        return Err(source_error(
            "outer constructor call must select the primary constructor",
        ));
    }
    let source_new = source_selection.qualifier;
    let TreeKind::New(source_new_node) = &source_ast
        .try_get(source_new)
        .ok_or_else(|| source_error("outer New is stale"))?
        .kind
    else {
        return Err(source_error("outer constructor receiver must be New"));
    };

    let caller_source = source_index
        .definition_of(caller_class)
        .and_then(|definition| match definition {
            SourceDefinition::Canonical { source, tree } if source == source_id => Some(tree),
            _ => None,
        })
        .ok_or_else(|| {
            identity_error("factory class has no canonical declaration in this source")
        })?;
    let TreeKind::TypeDef(caller_def) = &source_ast
        .try_get(caller_source)
        .ok_or_else(|| identity_error("factory class source tree is stale"))?
        .kind
    else {
        return Err(identity_error("factory source declaration is not a class"));
    };
    let TreeKind::Template(caller_template) = &source_ast
        .try_get(caller_def.rhs)
        .ok_or_else(|| identity_error("factory template is stale"))?
        .kind
    else {
        return Err(identity_error("factory class must have a template"));
    };
    require_source_position(
        source_ast,
        source_id,
        caller_def.rhs,
        "factory template has a foreign source position",
    )?;
    let method_name = store.names.resolve(method_def.name.as_name().text());
    let same_named_methods = caller_template
        .body
        .iter()
        .filter(
            |tree| match source_ast.try_get(**tree).map(|node| &node.kind) {
                Some(TreeKind::DefDef(definition)) => {
                    store.names.resolve(definition.name.as_name().text()) == method_name
                }
                _ => false,
            },
        )
        .count();
    if same_named_methods != 1 {
        return Err(source_error("overloaded factory methods are unsupported"));
    }
    require_source_position(
        source_ast,
        source_id,
        caller_source,
        "factory class source position is foreign",
    )?;
    if caller_info.visibility != dotty_core::Visibility::Public
        || has_any_flag(
            caller_info.flags,
            SymbolFlags::CASE | SymbolFlags::ENUM | SymbolFlags::ABSTRACT | SymbolFlags::SYNTHETIC,
        )
    {
        return Err(semantic_error(
            "factory owner must be a public ordinary concrete class",
        ));
    }
    let typed_rhs = source_typed_index
        .get(source_id, source_rhs)
        .ok_or_else(|| typed_error("factory RHS has no exact typed binding"))?;
    let method_plan = caller_plan
        .methods
        .iter()
        .filter(|plan| plan.method == method)
        .collect::<Vec<_>>();
    if method_plan.len() != 1
        || method_plan[0].parameters != method_formals
        || method_plan[0].body != Some(typed_rhs)
    {
        return Err(Error::JvmPlan(
            "JVM method plan must retain the exact ordered canonical formals and typed RHS",
        ));
    }
    let typed_select = source_typed_index
        .get(source_id, source_select)
        .ok_or_else(|| typed_error("outer Select has no exact typed binding"))?;
    let typed_new = source_typed_index
        .get(source_id, source_new)
        .ok_or_else(|| typed_error("outer New has no exact typed binding"))?;
    let typed_new_tpt = source_typed_index
        .get(source_id, source_new_node.tpt)
        .ok_or_else(|| typed_error("outer New type tree has no exact typed binding"))?;
    let typed_outer_args = [
        source_typed_index
            .get(source_id, outer_apply.args[0])
            .ok_or_else(|| typed_error("first outer argument has no exact typed binding"))?,
        source_typed_index
            .get(source_id, outer_apply.args[1])
            .ok_or_else(|| typed_error("second outer argument has no exact typed binding"))?,
    ];
    if outer_apply.args[0] == outer_apply.args[1] || typed_outer_args[0] == typed_outer_args[1] {
        return Err(typed_error(
            "sibling outer arguments must retain distinct source and typed roots",
        ));
    }
    let typed_root = typed_ast
        .try_get(typed_rhs)
        .ok_or_else(|| typed_error("factory typed RHS is stale"))?;
    let typed_apply = match &typed_root.kind {
        TreeKind::Apply(application) if application.kind == ApplyKind::Regular => application,
        _ => {
            return Err(typed_error(
                "factory typed RHS must remain the original regular Apply",
            ));
        }
    };
    if typed_apply.function != typed_select || typed_apply.args != typed_outer_args {
        return Err(typed_error(
            "typed outer Apply must preserve exact ordered source mappings",
        ));
    }
    let typed_selection = match &typed_ast
        .try_get(typed_select)
        .ok_or_else(|| typed_error("outer typed Select is stale"))?
        .kind
    {
        TreeKind::Select(selection) => selection,
        _ => {
            return Err(typed_error(
                "outer typed constructor function must be Select",
            ));
        }
    };
    if typed_selection.qualifier != typed_new
        || source_typed_index.get(source_id, source_selection.qualifier) != Some(typed_new)
    {
        return Err(typed_error(
            "outer Select must retain the exact mapped New receiver",
        ));
    }
    let typed_new_node = match &typed_ast
        .try_get(typed_new)
        .ok_or_else(|| typed_error("outer typed New is stale"))?
        .kind
    {
        TreeKind::New(new) if new.tpt == typed_new_tpt => new,
        _ => {
            return Err(typed_error(
                "outer New must retain its exact mapped type tree",
            ));
        }
    };
    let _ = typed_new_node;
    let target_type = typed_ast
        .try_get(typed_new_tpt)
        .ok_or_else(|| typed_error("outer typed New type tree is stale"))?
        .ty;
    let target_class = match store.types.try_get(target_type) {
        Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(class),
            ..
        }) => *class,
        _ => {
            return Err(typed_error(
                "Pair New must retain a canonical class TypeRef",
            ));
        }
    };
    if target_class == caller_class
        || !store.symbols.contains(target_class)
        || store.symbols.get(target_class).kind != SymbolKind::Class
        || store.symbols.get(target_class).owner != Some(package_owner)
        || store.symbols.get(target_class).visibility != dotty_core::Visibility::Public
        || has_any_flag(
            store.symbols.get(target_class).flags,
            SymbolFlags::CASE | SymbolFlags::ENUM | SymbolFlags::ABSTRACT | SymbolFlags::SYNTHETIC,
        )
        || !has_source_origin(store, target_class, source_id)
        || source_index.definition_of(target_class).is_none_or(|definition| {
            !matches!(definition, SourceDefinition::Canonical { source, .. } if source == source_id)
        })
    {
        return Err(identity_error("Pair target must be a distinct canonical class in the same source package"));
    }
    if !is_canonical_class_type_ref(
        store,
        target_type,
        target_class,
        package_owner,
        definitions.no_prefix,
    ) {
        return Err(typed_error(
            "Pair New must retain its exact canonical class TypeRef",
        ));
    }
    if !is_canonical_class_type_ref(
        store,
        method_signature.result,
        target_class,
        package_owner,
        definitions.no_prefix,
    ) {
        return Err(semantic_error(
            "factory signature result must retain the exact Pair TypeRef",
        ));
    }
    let target_tree = source_index
        .definition_of(target_class)
        .and_then(|definition| match definition {
            SourceDefinition::Canonical { source, tree } if source == source_id => Some(tree),
            _ => None,
        })
        .ok_or_else(|| {
            identity_error("Pair class declaration is missing from this source batch")
        })?;
    require_source_position(
        source_ast,
        source_id,
        target_tree,
        "Pair class declaration has a foreign source position",
    )?;
    let Some(target_ordinal) = source_class_trees
        .iter()
        .position(|tree| *tree == target_tree)
    else {
        return Err(identity_error(
            "Pair class is outside the exact source package root",
        ));
    };
    if jvm_unit
        .classes
        .get(target_ordinal)
        .is_none_or(|plan| plan.class != target_class)
    {
        return Err(Error::JvmPlan(
            "Pair class must occur at its exact source-order JVM batch position",
        ));
    }
    let TreeKind::TypeDef(target_def) = &source_ast
        .try_get(target_tree)
        .ok_or_else(|| identity_error("Pair class source tree is stale"))?
        .kind
    else {
        return Err(identity_error("Pair source declaration is not a class"));
    };
    let TreeKind::Template(target_template) = &source_ast
        .try_get(target_def.rhs)
        .ok_or_else(|| identity_error("Pair template is stale"))?
        .kind
    else {
        return Err(identity_error("Pair declaration must have a template"));
    };
    require_source_position(
        source_ast,
        source_id,
        target_def.rhs,
        "Pair template has a foreign source position",
    )?;
    if target_template.body.iter().any(|tree| {
        matches!(source_ast.try_get(*tree).map(|node| &node.kind), Some(TreeKind::DefDef(definition)) if store.names.resolve(definition.name.as_name().text()) == "this")
    }) {
        return Err(source_error("secondary Pair constructors are unsupported"));
    }
    let target_constructor = source_index
        .symbol_at(source_id, target_template.constructor)
        .ok_or_else(|| identity_error("Pair primary constructor has no canonical symbol"))?;
    let typed_constructor = match store.types.try_get(
        typed_ast
            .try_get(typed_select)
            .ok_or_else(|| typed_error("outer typed Select is stale"))?
            .ty,
    ) {
        Some(Type::TermRef {
            target: TermRefTarget::Symbol(constructor),
            ..
        }) => *constructor,
        _ => {
            return Err(typed_error(
                "outer Select must retain a canonical constructor TermRef",
            ));
        }
    };
    if typed_constructor != target_constructor
        || !store.symbols.contains(target_constructor)
        || store.symbols.get(target_constructor).kind != SymbolKind::Constructor
        || store.symbols.get(target_constructor).owner != Some(target_class)
        || !has_source_origin(store, target_constructor, source_id)
        || store.symbols.get(target_constructor).visibility != dotty_core::Visibility::Public
        || has_any_flag(
            store.symbols.get(target_constructor).flags,
            SymbolFlags::ABSTRACT | SymbolFlags::SYNTHETIC,
        )
    {
        return Err(semantic_error(
            "outer Select must choose the Pair primary constructor by canonical identity",
        ));
    }
    let TreeKind::DefDef(target_constructor_def) = &source_ast
        .try_get(target_template.constructor)
        .ok_or_else(|| identity_error("Pair constructor source tree is stale"))?
        .kind
    else {
        return Err(identity_error(
            "Pair primary constructor source tree must be DefDef",
        ));
    };
    require_source_position(
        source_ast,
        source_id,
        target_template.constructor,
        "Pair primary constructor has a foreign source position",
    )?;
    let target_formal_trees = target_constructor_def
        .value_param_clauses
        .iter()
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    if !target_constructor_def.type_params.is_empty() || target_formal_trees.len() != 2 {
        return Err(source_error(
            "Pair primary constructor must have exactly two ordinary formals",
        ));
    }
    for (index, formal_tree) in target_formal_trees.iter().copied().enumerate() {
        require_source_position(
            source_ast,
            source_id,
            formal_tree,
            if index == 0 {
                "first Pair constructor formal has a foreign source position"
            } else {
                "second Pair constructor formal has a foreign source position"
            },
        )?;
    }
    let target_formals = [
        source_index
            .derived_symbol_at(target_constructor, source_id, target_formal_trees[0])
            .ok_or_else(|| {
                identity_error("first Pair formal has no canonical constructor identity")
            })?,
        source_index
            .derived_symbol_at(target_constructor, source_id, target_formal_trees[1])
            .ok_or_else(|| {
                identity_error("second Pair formal has no canonical constructor identity")
            })?,
    ];
    let SymbolInfo::Complete(target_signature_type) = store.symbols.get(target_constructor).info
    else {
        return Err(semantic_error("Pair constructor signature is incomplete"));
    };
    let Some(Type::Method(target_signature)) = store.types.try_get(target_signature_type) else {
        return Err(semantic_error(
            "Pair constructor signature must be one method type",
        ));
    };
    if target_signature.kind != dotty_core::MethodKind::Plain || target_signature.params.len() != 2
    {
        return Err(semantic_error(
            "Pair constructor must have one plain two-formal signature",
        ));
    }
    let pair_formal_types = [target_signature.params[0].ty, target_signature.params[1].ty];
    let pair_counter_classes = [
        canonical_class_type_symbol(
            store,
            pair_formal_types[0],
            package_owner,
            definitions.no_prefix,
        ),
        canonical_class_type_symbol(
            store,
            pair_formal_types[1],
            package_owner,
            definitions.no_prefix,
        ),
    ];
    let [Some(counter_class), Some(second_counter_class)] = pair_counter_classes else {
        return Err(semantic_error(
            "Pair constructor formals must be canonical planned class references",
        ));
    };
    if counter_class != second_counter_class
        || counter_class == target_class
        || counter_class == caller_class
    {
        return Err(semantic_error(
            "Pair constructor formals must name the same distinct Counter class",
        ));
    }
    for index in 0..2 {
        let TreeKind::ValDef(parameter) = &source_ast.get(target_formal_trees[index]).kind else {
            return Err(source_error(
                "Pair primary constructor formals must be values",
            ));
        };
        if parameter.rhs.is_some()
            || !store.symbols.contains(target_formals[index])
            || store.symbols.get(target_formals[index]).kind != SymbolKind::Parameter
            || store.symbols.get(target_formals[index]).owner != Some(target_constructor)
            || !has_source_origin(store, target_formals[index], source_id)
            || store.symbols.get(target_formals[index]).info
                != SymbolInfo::Complete(pair_formal_types[index])
            || !is_canonical_class_type_ref(
                store,
                pair_formal_types[index],
                counter_class,
                package_owner,
                definitions.no_prefix,
            )
        {
            return Err(semantic_error(
                "Pair constructor formal order and exact Counter type must match its source declaration",
            ));
        }
    }

    let target_metadata = metadata
        .iter()
        .find(|entry| entry.identity().symbol() == target_class)
        .ok_or(Error::JvmPlan("Pair JVM class metadata is missing"))?;
    let target_constructor_metadata = target_metadata.constructor().ok_or(Error::JvmPlan(
        "Pair primary constructor metadata is missing",
    ))?;
    if target_constructor_metadata.symbol() != target_constructor
        || target_constructor_metadata
            .parameters()
            .iter()
            .map(|parameter| parameter.symbol())
            .collect::<Vec<_>>()
            != target_formals
        || !target_constructor_metadata
            .access_flags()
            .contains(dotty_classfile::access_flags::ACC_PUBLIC)
    {
        return Err(Error::JvmPlan(
            "Pair constructor metadata must preserve exact ordered source formal identities",
        ));
    }
    let caller_metadata = metadata
        .iter()
        .find(|entry| entry.identity().symbol() == caller_class)
        .ok_or(Error::JvmPlan("factory JVM class metadata is missing"))?;
    let method_metadata = caller_metadata
        .methods()
        .iter()
        .find(|entry| entry.symbol() == method)
        .ok_or(Error::JvmPlan("factory JVM method metadata is missing"))?;
    if method_metadata
        .parameters()
        .iter()
        .map(|parameter| parameter.symbol())
        .collect::<Vec<_>>()
        != method_formals
        || !method_metadata
            .access_flags()
            .contains(dotty_classfile::access_flags::ACC_PUBLIC)
        || method_metadata.descriptor().as_str().ends_with(")V")
    {
        return Err(Error::JvmPlan(
            "factory JVM metadata must retain both ordered canonical formals and a Pair result",
        ));
    }

    let direct_formal_arguments = outer_apply.args.iter().all(|argument| {
        matches!(
            source_ast.try_get(*argument).map(|node| &node.kind),
            Some(TreeKind::Ident(_))
        )
    });
    let mut ordered_argument_proofs = Vec::with_capacity(2);
    if direct_formal_arguments {
        for (index, typed_argument) in typed_outer_args.iter().copied().enumerate() {
            let source_argument = outer_apply.args[index];
            require_source_position(
                source_ast,
                source_id,
                source_argument,
                if index == 0 {
                    "first Pair argument has a foreign source position"
                } else {
                    "second Pair argument has a foreign source position"
                },
            )?;
            let TreeKind::Ident(source_ident) = &source_ast
                .try_get(source_argument)
                .ok_or_else(|| source_error("Pair argument source root is stale"))?
                .kind
            else {
                return Err(source_error(
                    "Pair argument must be an original method formal",
                ));
            };
            let typed_argument_node = typed_ast
                .try_get(typed_argument)
                .ok_or_else(|| typed_error("Pair formal argument typed root is stale"))?;
            let TreeKind::Ident(_) = &typed_argument_node.kind else {
                return Err(typed_error("Pair formal argument typed root must be Ident"));
            };
            let formal = match store.types.try_get(typed_argument_node.ty) {
                Some(Type::TermRef {
                    target: TermRefTarget::Symbol(formal),
                    ..
                }) => *formal,
                _ => {
                    return Err(typed_error(
                        "ordered Pair argument must retain its exact caller formal TermRef",
                    ));
                }
            };
            let formal_index = method_formals
                .iter()
                .position(|candidate| *candidate == formal)
                .ok_or_else(|| typed_error("Pair argument TermRef is not a method formal"))?;
            let source_formal_name = match &source_ast
                .try_get(method_formal_trees[formal_index])
                .ok_or_else(|| identity_error("method formal source tree is stale"))?
                .kind
            {
                TreeKind::ValDef(parameter) => parameter.name.as_name().text(),
                _ => return Err(identity_error("method formal source tree is not ValDef")),
            };
            if store.names.resolve(source_ident.name.text())
                != store.names.resolve(source_formal_name)
                || method_formal_trees
                    .iter()
                    .enumerate()
                    .any(|(other_index, tree)| {
                        other_index != formal_index
                            && matches!(
                                source_ast.try_get(*tree).map(|node| &node.kind),
                                Some(TreeKind::ValDef(parameter))
                                    if store.names.resolve(parameter.name.as_name().text())
                                        == store.names.resolve(source_ident.name.text())
                            )
                    })
            {
                return Err(typed_error(
                    "typed Pair argument target must match its unique original source formal",
                ));
            }
            if source_index.definition_of(formal)
                != Some(SourceDefinition::Canonical {
                    source: source_id,
                    tree: method_formal_trees[formal_index],
                })
                || !store.symbols.contains(formal)
                || store.symbols.get(formal).kind != SymbolKind::Parameter
                || store.symbols.get(formal).owner != Some(method)
                || !has_source_origin(store, formal, source_id)
                || store.symbols.get(formal).info
                    != SymbolInfo::Complete(method_formal_types[formal_index])
                || !is_canonical_class_type_ref(
                    store,
                    method_formal_types[formal_index],
                    counter_class,
                    package_owner,
                    definitions.no_prefix,
                )
            {
                return Err(identity_error(
                    "ordered direct argument must retain its exact caller formal and Pair parameter type",
                ));
            }
            if source_typed_index.get(source_id, source_argument) != Some(typed_argument) {
                return Err(typed_error(
                    "ordered Pair argument must retain its exact source-to-typed binding",
                ));
            }
            require_matching_source_typed_positions(
                source_ast,
                typed_ast,
                source_id,
                source_argument,
                typed_argument,
                if index == 0 {
                    "first direct Pair argument"
                } else {
                    "second direct Pair argument"
                },
            )?;
            let formal_proof = FormalConstructorArgumentProof {
                source_argument,
                typed_argument,
                formal,
                formal_type: method_formal_types[formal_index],
                result_type: typed_argument_node.ty,
            };
            ordered_argument_proofs.push(OrderedConstructorArgumentProof {
                source_argument,
                typed_argument,
                origin: OrderedConstructorArgumentOrigin::Formal(formal_proof),
            });
        }
    } else {
        let mut nested_proofs = Vec::with_capacity(2);
        let mut target_formal_types = [definitions.int; 2];
        for index in 0..2 {
            let source_outer_argument = outer_apply.args[index];
            let typed_outer_argument = typed_outer_args[index];
            let TreeKind::Apply(inner_apply) = &source_ast
                .try_get(source_outer_argument)
                .ok_or_else(|| source_error("nested Counter Apply source root is stale"))?
                .kind
            else {
                return Err(source_error(
                    "each Pair argument must be one nested Counter Apply",
                ));
            };
            if inner_apply.kind != ApplyKind::Regular || inner_apply.args.len() != 1 {
                return Err(source_error(
                    "each nested Counter call must have one regular argument",
                ));
            }
            let source_inner_select = inner_apply.function;
            let TreeKind::Select(inner_selection) = &source_ast
                .try_get(source_inner_select)
                .ok_or_else(|| source_error("nested Counter Select source root is stale"))?
                .kind
            else {
                return Err(source_error("nested Counter function must be Select"));
            };
            if store.names.resolve(inner_selection.name.text()) != "<init>" {
                return Err(source_error(
                    "nested Counter call must select its primary constructor",
                ));
            }
            let source_inner_new = inner_selection.qualifier;
            let TreeKind::New(inner_new) = &source_ast
                .try_get(source_inner_new)
                .ok_or_else(|| source_error("nested Counter New source root is stale"))?
                .kind
            else {
                return Err(source_error("nested Counter receiver must be New"));
            };
            let source_leaf = inner_apply.args[0];
            let TreeKind::Ident(leaf_ident) = &source_ast
                .try_get(source_leaf)
                .ok_or_else(|| source_error("nested Counter leaf source root is stale"))?
                .kind
            else {
                return Err(source_error(
                    "nested Counter leaf must be an original factory formal",
                ));
            };
            let leaf_name = store.names.resolve(leaf_ident.name.text());
            let caller_formal_index = method_formal_trees.iter().position(|tree| {
            matches!(&source_ast.get(*tree).kind, TreeKind::ValDef(parameter) if store.names.resolve(parameter.name.as_name().text()) == leaf_name)
        }).ok_or_else(|| source_error("nested Counter leaf must resolve to one caller formal"))?;
            let caller_formal = method_formals[caller_formal_index];
            let source_inner_select_typed = source_typed_index
                .get(source_id, source_inner_select)
                .ok_or_else(|| typed_error("nested Counter Select has no exact typed binding"))?;
            let source_inner_new_typed = source_typed_index
                .get(source_id, source_inner_new)
                .ok_or_else(|| typed_error("nested Counter New has no exact typed binding"))?;
            let source_inner_tpt_typed = source_typed_index
                .get(source_id, inner_new.tpt)
                .ok_or_else(|| {
                    typed_error("nested Counter type tree has no exact typed binding")
                })?;
            let source_leaf_typed = source_typed_index
                .get(source_id, source_leaf)
                .ok_or_else(|| typed_error("nested Counter leaf has no exact typed binding"))?;
            let typed_inner_apply_node = typed_ast
                .try_get(typed_outer_argument)
                .ok_or_else(|| typed_error("nested Counter typed Apply is stale"))?;
            let TreeKind::Apply(typed_inner_apply) = &typed_inner_apply_node.kind else {
                return Err(typed_error("nested Counter typed root must remain Apply"));
            };
            if typed_inner_apply.kind != ApplyKind::Regular
                || typed_inner_apply.function != source_inner_select_typed
                || typed_inner_apply.args != [source_leaf_typed]
                || source_typed_index.get(source_id, source_inner_select)
                    != Some(source_inner_select_typed)
                || source_typed_index.get(source_id, source_inner_new)
                    != Some(source_inner_new_typed)
            {
                return Err(typed_error(
                    "nested Counter typed Apply must preserve exact original Select, New, and leaf roots",
                ));
            }
            let TreeKind::Select(typed_inner_selection) = &typed_ast
                .try_get(source_inner_select_typed)
                .ok_or_else(|| typed_error("nested Counter typed Select is stale"))?
                .kind
            else {
                return Err(typed_error(
                    "nested Counter typed function must remain Select",
                ));
            };
            if typed_inner_selection.qualifier != source_inner_new_typed {
                return Err(typed_error(
                    "nested Counter Select must retain its exact mapped New receiver",
                ));
            }
            let TreeKind::New(typed_inner_new_node) = &typed_ast
                .try_get(source_inner_new_typed)
                .ok_or_else(|| typed_error("nested Counter typed New is stale"))?
                .kind
            else {
                return Err(typed_error("nested Counter typed receiver must remain New"));
            };
            if typed_inner_new_node.tpt != source_inner_tpt_typed {
                return Err(typed_error(
                    "nested Counter New must retain its exact mapped type tree",
                ));
            }
            let inner_class_type = typed_ast
                .try_get(source_inner_tpt_typed)
                .ok_or_else(|| typed_error("nested Counter typed type tree is stale"))?
                .ty;
            if !is_canonical_class_type_ref(
                store,
                inner_class_type,
                counter_class,
                package_owner,
                definitions.no_prefix,
            ) {
                return Err(typed_error(
                    "nested Counter New must retain the exact Pair formal TypeRef",
                ));
            }
            let inner_constructor = match store.types.try_get(
                typed_ast
                    .try_get(source_inner_select_typed)
                    .ok_or_else(|| typed_error("nested Counter typed Select is stale"))?
                    .ty,
            ) {
                Some(Type::TermRef {
                    target: TermRefTarget::Symbol(constructor),
                    ..
                }) => *constructor,
                _ => {
                    return Err(typed_error(
                        "nested Counter Select must retain its canonical constructor TermRef",
                    ));
                }
            };
            if !store.symbols.contains(inner_constructor)
                || store.symbols.get(inner_constructor).kind != SymbolKind::Constructor
                || store.symbols.get(inner_constructor).owner != Some(counter_class)
                || !has_source_origin(store, inner_constructor, source_id)
            {
                return Err(semantic_error(
                    "nested Select must choose a source constructor owned by the exact Counter class",
                ));
            }
            let counter_tree = source_index
                .definition_of(counter_class)
                .and_then(|definition| match definition {
                    SourceDefinition::Canonical { source, tree } if source == source_id => {
                        Some(tree)
                    }
                    _ => None,
                })
                .ok_or_else(|| {
                    identity_error("Counter class has no canonical declaration in this source")
                })?;
            let counter_ordinal = source_class_trees
                .iter()
                .position(|tree| *tree == counter_tree)
                .ok_or_else(|| identity_error("Counter class is outside the exact source batch"))?;
            if jvm_unit
                .classes
                .get(counter_ordinal)
                .is_none_or(|plan| plan.class != counter_class)
            {
                return Err(Error::JvmPlan(
                    "Counter class is absent from its exact source-order JVM batch position",
                ));
            }
            let TreeKind::TypeDef(counter_def) = &source_ast.get(counter_tree).kind else {
                return Err(identity_error("Counter declaration must be a class"));
            };
            require_source_position(
                source_ast,
                source_id,
                counter_tree,
                "Counter class declaration has a foreign source position",
            )?;
            if store.symbols.get(counter_class).visibility != dotty_core::Visibility::Public
                || has_any_flag(
                    store.symbols.get(counter_class).flags,
                    SymbolFlags::CASE
                        | SymbolFlags::ENUM
                        | SymbolFlags::ABSTRACT
                        | SymbolFlags::SYNTHETIC,
                )
            {
                return Err(semantic_error(
                    "Counter target must be a public ordinary concrete class",
                ));
            }
            let TreeKind::Template(counter_template) = &source_ast.get(counter_def.rhs).kind else {
                return Err(identity_error("Counter declaration must have a template"));
            };
            require_source_position(
                source_ast,
                source_id,
                counter_def.rhs,
                "Counter template has a foreign source position",
            )?;
            if counter_template.body.iter().any(|tree| {
            matches!(source_ast.try_get(*tree).map(|node| &node.kind), Some(TreeKind::DefDef(definition)) if store.names.resolve(definition.name.as_name().text()) == "this")
        }) {
            return Err(source_error("secondary Counter constructors are unsupported"));
        }
            let counter_constructor = source_index
                .symbol_at(source_id, counter_template.constructor)
                .ok_or_else(|| {
                    identity_error("Counter primary constructor has no canonical symbol")
                })?;
            if inner_constructor != counter_constructor {
                return Err(semantic_error(
                    "nested Select must choose the exact Counter primary constructor",
                ));
            }
            if store.symbols.get(counter_constructor).visibility != dotty_core::Visibility::Public
                || has_any_flag(
                    store.symbols.get(counter_constructor).flags,
                    SymbolFlags::ABSTRACT | SymbolFlags::SYNTHETIC,
                )
            {
                return Err(semantic_error(
                    "Counter primary constructor must be public and ordinary",
                ));
            }
            let TreeKind::DefDef(counter_ctor_def) =
                &source_ast.get(counter_template.constructor).kind
            else {
                return Err(identity_error(
                    "Counter primary constructor source tree must be DefDef",
                ));
            };
            require_source_position(
                source_ast,
                source_id,
                counter_template.constructor,
                "Counter primary constructor has a foreign source position",
            )?;
            let counter_formal_trees = counter_ctor_def
                .value_param_clauses
                .iter()
                .flatten()
                .copied()
                .collect::<Vec<_>>();
            if !counter_ctor_def.type_params.is_empty() || counter_formal_trees.len() != 1 {
                return Err(source_error(
                    "nested Counter constructor must have one ordinary formal",
                ));
            }
            require_source_position(
                source_ast,
                source_id,
                counter_formal_trees[0],
                "Counter constructor formal has a foreign source position",
            )?;
            let counter_formal = source_index
                .derived_symbol_at(counter_constructor, source_id, counter_formal_trees[0])
                .ok_or_else(|| {
                    identity_error("Counter constructor formal has no canonical identity")
                })?;
            let SymbolInfo::Complete(counter_signature_type) =
                store.symbols.get(counter_constructor).info
            else {
                return Err(semantic_error(
                    "Counter constructor signature is incomplete",
                ));
            };
            let Some(Type::Method(counter_signature)) = store.types.try_get(counter_signature_type)
            else {
                return Err(semantic_error(
                    "Counter constructor signature must be a method type",
                ));
            };
            let SymbolInfo::Complete(counter_formal_type) = store.symbols.get(counter_formal).info
            else {
                return Err(semantic_error(
                    "Counter constructor formal type is incomplete",
                ));
            };
            if counter_signature.kind != dotty_core::MethodKind::Plain
                || counter_signature.params.len() != 1
                || counter_signature.params[0].ty != counter_formal_type
                || counter_formal_type != definitions.int
                || method_formal_types[caller_formal_index] != definitions.int
            {
                return Err(semantic_error(
                    "Counter and caller signatures must retain exact canonical Int formals",
                ));
            }
            let typed_leaf_node = typed_ast
                .try_get(source_leaf_typed)
                .ok_or_else(|| typed_error("nested Counter typed leaf is stale"))?;
            if !matches!(typed_leaf_node.kind, TreeKind::Ident(_))
                || !matches!(store.types.try_get(typed_leaf_node.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }) if *symbol == caller_formal)
            {
                return Err(typed_error(
                    "nested leaf must retain the exact original caller formal TermRef and type",
                ));
            }
            let result_type = typed_inner_apply_node.ty;
            if !matches!(
                store.types.try_get(result_type),
                Some(Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }) if *class == counter_class
            ) {
                return Err(typed_error(
                    "nested Counter Apply result must retain its canonical Counter TypeRef",
                ));
            }
            for (source_tree, typed_tree, description) in [
                (
                    source_outer_argument,
                    typed_outer_argument,
                    "outer Pair argument",
                ),
                (
                    source_inner_select,
                    source_inner_select_typed,
                    "nested Counter Select",
                ),
                (
                    source_inner_new,
                    source_inner_new_typed,
                    "nested Counter New",
                ),
                (
                    inner_new.tpt,
                    source_inner_tpt_typed,
                    "nested Counter type tree",
                ),
                (source_leaf, source_leaf_typed, "nested caller formal leaf"),
            ] {
                require_matching_source_typed_positions(
                    source_ast,
                    typed_ast,
                    source_id,
                    source_tree,
                    typed_tree,
                    description,
                )?;
            }
            require_matching_source_typed_positions(
                source_ast,
                typed_ast,
                source_id,
                source_rhs,
                typed_rhs,
                "factory body",
            )?;
            require_matching_source_typed_positions(
                source_ast,
                typed_ast,
                source_id,
                source_select,
                typed_select,
                "outer Pair Select",
            )?;
            require_matching_source_typed_positions(
                source_ast,
                typed_ast,
                source_id,
                source_new,
                typed_new,
                "outer Pair New",
            )?;
            require_matching_source_typed_positions(
                source_ast,
                typed_ast,
                source_id,
                source_new_node.tpt,
                typed_new_tpt,
                "outer Pair type tree",
            )?;
            require_matching_source_typed_positions(
                source_ast,
                typed_ast,
                source_id,
                source_inner_select,
                source_inner_select_typed,
                "nested Counter Select",
            )?;
            require_matching_source_typed_positions(
                source_ast,
                typed_ast,
                source_id,
                source_inner_new,
                source_inner_new_typed,
                "nested Counter New",
            )?;
            require_matching_source_typed_positions(
                source_ast,
                typed_ast,
                source_id,
                inner_new.tpt,
                source_inner_tpt_typed,
                "nested Counter type tree",
            )?;
            require_matching_source_typed_positions(
                source_ast,
                typed_ast,
                source_id,
                source_leaf,
                source_leaf_typed,
                "nested caller formal leaf",
            )?;
            let counter_metadata = metadata
                .iter()
                .find(|entry| entry.identity().symbol() == counter_class)
                .ok_or(Error::JvmPlan("Counter JVM metadata is missing"))?;
            let counter_ctor_metadata = counter_metadata.constructor().ok_or(Error::JvmPlan(
                "Counter primary constructor metadata is missing",
            ))?;
            if counter_ctor_metadata.symbol() != counter_constructor
                || counter_ctor_metadata
                    .parameters()
                    .iter()
                    .map(|parameter| parameter.symbol())
                    .collect::<Vec<_>>()
                    != [counter_formal]
                || !counter_ctor_metadata
                    .access_flags()
                    .contains(dotty_classfile::access_flags::ACC_PUBLIC)
            {
                return Err(Error::JvmPlan(
                    "Counter metadata must retain its exact public primary constructor formal",
                ));
            }
            nested_proofs.push(NestedConstructorArgumentProof {
                source_outer_argument,
                typed_outer_argument,
                source_apply: source_outer_argument,
                typed_apply: typed_outer_argument,
                source_select: source_inner_select,
                source_new: source_inner_new,
                source_argument: source_leaf,
                typed_select: source_inner_select_typed,
                typed_new: source_inner_new_typed,
                typed_argument: source_leaf_typed,
                class: counter_class,
                constructor: counter_constructor,
                formal: counter_formal,
                argument_formal: caller_formal,
                formal_type: counter_formal_type,
                argument_formal_type: method_formal_types[caller_formal_index],
                result_type,
            });
            target_formal_types[index] = pair_formal_types[index];
        }
        let [nested_first, nested_second] =
            <[NestedConstructorArgumentProof; 2]>::try_from(nested_proofs).map_err(|_| {
                Error::SourceShape("exactly two nested constructor argument proofs are required")
            })?;
        if nested_first.class() != nested_second.class()
            || nested_first.constructor() != nested_second.constructor()
            || nested_first.source_outer_argument() == nested_second.source_outer_argument()
            || nested_first.source_apply() == nested_second.source_apply()
            || nested_first.source_select() == nested_second.source_select()
            || nested_first.source_new() == nested_second.source_new()
            || nested_first.source_argument() == nested_second.source_argument()
            || nested_first.typed_outer_argument() == nested_second.typed_outer_argument()
            || nested_first.typed_apply() == nested_second.typed_apply()
            || nested_first.typed_select() == nested_second.typed_select()
            || nested_first.typed_new() == nested_second.typed_new()
            || nested_first.typed_argument() == nested_second.typed_argument()
        {
            return Err(typed_error(
                "sibling constructor receipts must preserve distinct ordered call-site roots",
            ));
        }
        ordered_argument_proofs.extend([
            OrderedConstructorArgumentProof {
                source_argument: nested_first.source_outer_argument(),
                typed_argument: nested_first.typed_outer_argument(),
                origin: OrderedConstructorArgumentOrigin::NestedNew(nested_first),
            },
            OrderedConstructorArgumentProof {
                source_argument: nested_second.source_outer_argument(),
                typed_argument: nested_second.typed_outer_argument(),
                origin: OrderedConstructorArgumentOrigin::NestedNew(nested_second),
            },
        ]);
    }
    let ordered_argument_proofs = <[OrderedConstructorArgumentProof; 2]>::try_from(
        ordered_argument_proofs,
    )
    .map_err(|_| {
        source_error("exactly two authenticated ordered constructor arguments are required")
    })?;
    let result_type = typed_root.ty;
    if !matches!(
        store.types.try_get(result_type),
        Some(Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }) if *class == target_class
    ) {
        return Err(typed_error(
            "factory result must retain its canonical Pair TypeRef",
        ));
    }
    let method_descriptor = method_metadata.descriptor().as_str();
    let constructor_descriptor = target_constructor_metadata.descriptor().as_str();
    let constructor_formal_type = pair_formal_types[0];
    Ok(ValidatedCrossClassConstructorCallProof {
        _store: store,
        _source_ast: source_ast,
        _typed_ast: typed_ast,
        _source_index: source_index,
        _source_typed_index: source_typed_index,
        _jvm_unit: jvm_unit,
        source_id,
        caller_class,
        method,
        method_formal: method_formals[0],
        formal_class: direct_formal_arguments.then_some(counter_class),
        target_class,
        constructor: target_constructor,
        source_rhs,
        source_select,
        source_new,
        source_argument: outer_apply.args[0],
        typed_rhs,
        typed_select,
        typed_new,
        typed_argument: typed_outer_args[0],
        argument_proof: ConstructorArgumentProof {
            shape: ConstructorArgumentProofShape::OrderedPair(
                OrderedConstructorArgumentPairProof {
                    source_method_formals: method_formal_trees,
                    method_formals,
                    method_formal_types,
                    source_constructor_formals: [target_formal_trees[0], target_formal_trees[1]],
                    constructor_formals: target_formals,
                    constructor_formal_types: pair_formal_types,
                    arguments: ordered_argument_proofs,
                },
            ),
        },
        formal_type: method_formal_types[0],
        constructor_formal_type,
        result_type,
        method_descriptor: method_descriptor.to_owned(),
        constructor_descriptor: constructor_descriptor.to_owned(),
    })
}

fn is_direct_constructor_call_candidate(ast: &AstArena<Untyped>, rhs: TreeId<Untyped>) -> bool {
    fn constructor_application(ast: &AstArena<Untyped>, root: TreeId<Untyped>) -> bool {
        let Some(TreeKind::Apply(application)) = ast.try_get(root).map(|node| &node.kind) else {
            return false;
        };
        let Some(TreeKind::Select(selection)) =
            ast.try_get(application.function).map(|node| &node.kind)
        else {
            return false;
        };
        matches!(
            ast.try_get(selection.qualifier).map(|node| &node.kind),
            Some(TreeKind::New(_))
        )
    }

    match ast.try_get(rhs).map(|node| &node.kind) {
        Some(TreeKind::Apply(_)) => constructor_application(ast, rhs),
        Some(TreeKind::Block(block)) if constructor_application(ast, block.expr) => true,
        Some(TreeKind::Block(block)) => {
            block
                .stats
                .iter()
                .any(|statement| constructor_application(ast, *statement))
                || match ast.try_get(block.expr).map(|node| &node.kind) {
                    Some(TreeKind::Block(nested)) => constructor_application(ast, nested.expr),
                    Some(TreeKind::If(branch)) => {
                        constructor_application(ast, branch.then_branch)
                            || constructor_application(ast, branch.else_branch)
                    }
                    _ => false,
                }
        }
        _ => false,
    }
}

fn require_source_position(
    source_ast: &AstArena<Untyped>,
    source_id: SourceId,
    source_tree: TreeId<Untyped>,
    description: &'static str,
) -> Result<dotty_core::SourceSpan, CrossClassConstructorCallProofError> {
    let position = source_ast
        .try_get(source_tree)
        .and_then(|tree| tree.position)
        .ok_or(CrossClassConstructorCallProofError::SourceIdentity(
            "a canonical source tree is missing its source position",
        ))?;
    if position.source() != source_id {
        return Err(CrossClassConstructorCallProofError::SourceIdentity(
            description,
        ));
    }
    Ok(position)
}

fn has_source_origin(store: &SemanticStore, symbol: SymbolId, source_id: SourceId) -> bool {
    store.symbols.contains(symbol)
        && store.symbols.get(symbol).origin == SymbolOrigin::Source(source_id)
}

fn require_matching_source_typed_positions(
    source_ast: &AstArena<Untyped>,
    typed_ast: &TypedAst,
    source_id: SourceId,
    source_tree: TreeId<Untyped>,
    typed_tree: TreeId<Typed>,
    description: &'static str,
) -> Result<(), CrossClassConstructorCallProofError> {
    let source_position = require_source_position(source_ast, source_id, source_tree, description)?;
    let typed_position = typed_ast
        .try_get(typed_tree)
        .and_then(|tree| tree.position)
        .ok_or(CrossClassConstructorCallProofError::TypedBinding(
            "a canonical typed tree is missing its source position",
        ))?;
    if typed_position != source_position {
        return Err(CrossClassConstructorCallProofError::TypedBinding(
            "source and typed trees do not retain the exact same source position and owner",
        ));
    }
    Ok(())
}

fn unique_constructor_call_class_plan<'a>(
    unit: &'a JvmCompilationUnit<'_>,
    class: SymbolId,
) -> Result<&'a JvmClassPlan, CrossClassConstructorCallProofError> {
    let mut plans = unit.classes.iter().filter(|plan| plan.class == class);
    let plan = plans
        .next()
        .ok_or(CrossClassConstructorCallProofError::JvmPlan(
            "canonical source class is absent from the complete JVM batch",
        ))?;
    if plans.next().is_some() {
        return Err(CrossClassConstructorCallProofError::JvmPlan(
            "canonical source class occurs more than once in the JVM batch",
        ));
    }
    Ok(plan)
}

#[allow(clippy::too_many_arguments)]
fn validate_constructor_call_candidate<'session>(
    source_id: SourceId,
    caller_class: SymbolId,
    source_class_trees: &[TreeId<Untyped>],
    method_tree: TreeId<Untyped>,
    method_def: &dotty_core::ast::DefDef<Untyped>,
    method: SymbolId,
    source_rhs: TreeId<Untyped>,
    store: &'session SemanticStore,
    definitions: &'session Definitions,
    source_ast: &'session AstArena<Untyped>,
    typed_ast: &'session TypedAst,
    source_index: &'session SourceSemanticIndex,
    source_typed_index: &'session SourceTypedIndex,
    typer: Option<&SourceTyper<'_>>,
    jvm_unit: &'session JvmCompilationUnit<'session>,
    caller_plan: &JvmClassPlan,
    metadata: &[dotty_jvm::JvmClassMetadata],
) -> Result<ValidatedCrossClassConstructorCallProof<'session>, CrossClassConstructorCallProofError>
{
    let source_error = |s| CrossClassConstructorCallProofError::SourceShape(s);
    let identity_error = |s| CrossClassConstructorCallProofError::SourceIdentity(s);
    let semantic_error = |s| CrossClassConstructorCallProofError::SemanticIdentity(s);
    let typed_error = |s| CrossClassConstructorCallProofError::TypedBinding(s);
    let caller_source_tree = match source_index.definition_of(caller_class) {
        Some(SourceDefinition::Canonical { source, tree }) if source == source_id => tree,
        _ => return Err(identity_error("caller class has a foreign source origin")),
    };
    require_source_position(
        source_ast,
        source_id,
        caller_source_tree,
        "caller class has a foreign source position",
    )?;
    require_source_position(
        source_ast,
        source_id,
        method_tree,
        "calling method has a foreign source position",
    )?;
    if source_index.symbol_at(source_id, method_tree) != Some(method)
        || source_index.definition_of(method)
            != Some(SourceDefinition::Canonical {
                source: source_id,
                tree: method_tree,
            })
        || !store.symbols.contains(method)
        || !has_source_origin(store, method, source_id)
        || store.symbols.get(method).kind != SymbolKind::Method
        || store.symbols.get(method).owner != Some(caller_class)
        || store.symbols.get(method).visibility != dotty_core::Visibility::Public
        || has_any_flag(
            store.symbols.get(method).flags,
            SymbolFlags::ABSTRACT
                | SymbolFlags::SYNTHETIC
                | SymbolFlags::STATIC
                | SymbolFlags::GIVEN
                | SymbolFlags::IMPLICIT
                | SymbolFlags::ERASED
                | SymbolFlags::EXTENSION,
        )
    {
        return Err(identity_error(
            "candidate method is not the public canonical method owned by its source class",
        ));
    }
    if !method_def.type_params.is_empty()
        || method_def.value_param_clauses.len() != 1
        || method_def.value_param_clauses[0].len() != 1
        || method_def.metadata.visibility.is_some()
    {
        return Err(source_error(
            "candidate method must have one plain formal clause and a concrete public body",
        ));
    }
    let method_formal_tree = method_def.value_param_clauses[0][0];
    require_source_position(
        source_ast,
        source_id,
        method_formal_tree,
        "caller formal has a foreign source position",
    )?;
    let TreeKind::ValDef(formal_def) = &source_ast
        .try_get(method_formal_tree)
        .ok_or_else(|| source_error("method formal source tree is stale"))?
        .kind
    else {
        return Err(source_error(
            "candidate method formal must be a value parameter",
        ));
    };
    if formal_def.rhs.is_some() {
        return Err(source_error(
            "candidate method formal cannot use a default value",
        ));
    }
    let method_formal = source_index
        .symbol_at(source_id, method_formal_tree)
        .ok_or_else(|| identity_error("method formal has no canonical source symbol"))?;
    if source_index.definition_of(method_formal)
        != Some(SourceDefinition::Canonical {
            source: source_id,
            tree: method_formal_tree,
        })
        || !store.symbols.contains(method_formal)
        || !has_source_origin(store, method_formal, source_id)
        || store.symbols.get(method_formal).kind != SymbolKind::Parameter
        || store.symbols.get(method_formal).owner != Some(method)
    {
        return Err(identity_error(
            "method formal is not the canonical parameter owned by this method",
        ));
    }
    let SymbolInfo::Complete(method_type) = store.symbols.get(method).info else {
        return Err(semantic_error("candidate method signature is incomplete"));
    };
    let Some(Type::Method(method_signature)) = store.types.try_get(method_type) else {
        return Err(semantic_error(
            "candidate method must have one plain method type",
        ));
    };
    let SymbolInfo::Complete(formal_type) = store.symbols.get(method_formal).info else {
        return Err(semantic_error("candidate method formal type is incomplete"));
    };
    let package = store
        .symbols
        .get(caller_class)
        .owner
        .ok_or_else(|| semantic_error("caller has no package owner"))?;
    let formal_class =
        canonical_class_type_symbol(store, formal_type, package, definitions.no_prefix);
    if let Some(formal_class) = formal_class
        && (formal_class == caller_class
            || source_index.definition_of(formal_class).is_none_or(
                |definition| !matches!(definition, SourceDefinition::Canonical { source, .. } if source == source_id),
            )
            || unique_constructor_call_class_plan(jvm_unit, formal_class).is_err())
    {
        return Err(semantic_error(
            "reference formal must be a different canonical class planned in this source batch",
        ));
    }
    if method_signature.kind != dotty_core::MethodKind::Plain
        || method_signature.params.len() != 1
        || method_signature.params[0].ty != formal_type
    {
        return Err(semantic_error(
            "method signature must match its one canonical formal",
        ));
    }

    let source_block = match &source_ast
        .try_get(source_rhs)
        .ok_or_else(|| source_error("method RHS source root is stale"))?
        .kind
    {
        TreeKind::Block(block) => Some(block),
        _ => None,
    };
    let source_constructor_apply = source_block.map_or(source_rhs, |block| block.expr);
    let TreeKind::Apply(source_apply) = &source_ast
        .try_get(source_constructor_apply)
        .ok_or_else(|| source_error("constructor expression source root is stale"))?
        .kind
    else {
        return Err(source_error(
            "local constructor Block result must be one direct constructor application",
        ));
    };
    if source_apply.kind != ApplyKind::Regular || source_apply.args.len() != 1 {
        return Err(source_error(
            "constructor application must have one regular argument",
        ));
    }
    let source_select = source_apply.function;
    let TreeKind::Select(source_selection) = &source_ast
        .try_get(source_select)
        .ok_or_else(|| source_error("constructor selection source root is stale"))?
        .kind
    else {
        return Err(source_error("constructor function must be Select"));
    };
    if store.names.resolve(source_selection.name.text()) != "<init>" {
        return Err(source_error(
            "constructor call must select the primary constructor",
        ));
    }
    let source_new = source_selection.qualifier;
    let TreeKind::New(source_new_tree) = &source_ast
        .try_get(source_new)
        .ok_or_else(|| source_error("New source root is stale"))?
        .kind
    else {
        return Err(source_error("constructor receiver must be New"));
    };
    let source_argument = source_apply.args[0];
    let argument_shape = match &source_ast
        .try_get(source_argument)
        .ok_or_else(|| source_error("constructor argument source root is stale"))?
        .kind
    {
        TreeKind::Ident(_) if source_block.is_some() => {
            let block = source_block.expect("guarded by the block-body shape");
            if block.stats.len() != 1 {
                return Err(source_error(
                    "local constructor Block must have exactly one ValDef",
                ));
            }
            let declaration = block.stats[0];
            let TreeKind::ValDef(local) = &source_ast
                .try_get(declaration)
                .ok_or_else(|| source_error("local ValDef source tree is stale"))?
                .kind
            else {
                return Err(source_error(
                    "local constructor Block statistic must be a ValDef",
                ));
            };
            let initializer = local
                .rhs
                .ok_or_else(|| source_error("local constructor ValDef must be initialized"))?;
            let TreeKind::Apply(initializer_apply) = &source_ast
                .try_get(initializer)
                .ok_or_else(|| source_error("local initializer source tree is stale"))?
                .kind
            else {
                return Err(source_error(
                    "local initializer must be one planned helper call",
                ));
            };
            if initializer_apply.kind != ApplyKind::Regular || initializer_apply.args.len() != 1 {
                return Err(source_error(
                    "local initializer must be one regular helper call",
                ));
            }
            let function = initializer_apply.function;
            let function_kind = &source_ast
                .try_get(function)
                .ok_or_else(|| source_error("local initializer function source tree is stale"))?
                .kind;
            let receiver = match function_kind {
                TreeKind::Ident(_) => None,
                TreeKind::Select(selection) => Some(selection.qualifier),
                _ => {
                    return Err(source_error(
                        "local initializer must select one ordinary method",
                    ));
                }
            };
            SourceArgumentShape::LocalVal {
                declaration,
                initializer,
                formal_tree: initializer_apply.args[0],
                apply: initializer,
                function,
                receiver,
            }
        }
        TreeKind::Ident(_) => SourceArgumentShape::Formal {
            formal_tree: source_argument,
        },
        TreeKind::Apply(nested_apply) if nested_apply.kind == ApplyKind::Regular => {
            if nested_apply.args.len() != 1 {
                return Err(source_error(
                    "nested constructor or helper call must have one regular argument",
                ));
            }
            let nested_select = nested_apply.function;
            let function_kind = &source_ast
                .try_get(nested_select)
                .ok_or_else(|| source_error("nested application function source tree is stale"))?
                .kind;
            if let TreeKind::Select(nested_selection) = function_kind
                && store.names.resolve(nested_selection.name.text()) == "<init>"
            {
                let nested_new = nested_selection.qualifier;
                let TreeKind::New(nested_new_tree) = &source_ast
                    .try_get(nested_new)
                    .ok_or_else(|| source_error("nested New source root is stale"))?
                    .kind
                else {
                    return Err(source_error("nested constructor receiver must be New"));
                };
                SourceArgumentShape::NestedNew {
                    formal_tree: nested_apply.args[0],
                    select: nested_select,
                    new: nested_new,
                    type_tree: nested_new_tree.tpt,
                }
            } else if matches!(function_kind, TreeKind::Ident(_) | TreeKind::Select(_)) {
                let receiver = match function_kind {
                    TreeKind::Select(selection) => Some(selection.qualifier),
                    TreeKind::Ident(_) => None,
                    _ => unreachable!(),
                };
                SourceArgumentShape::PlannedCall {
                    formal_tree: nested_apply.args[0],
                    apply: source_argument,
                    function: nested_select,
                    receiver,
                }
            } else {
                return Err(source_error(
                    "planned helper must be one selected ordinary method application",
                ));
            }
        }
        TreeKind::Apply(_) => {
            return Err(source_error(
                "nested constructor or helper application must be regular",
            ));
        }
        _ => {
            return Err(source_error(
                "constructor argument must be the exact caller formal, one nested New, or one planned call",
            ));
        }
    };
    let argument_formal_tree = match argument_shape {
        SourceArgumentShape::Formal { formal_tree }
        | SourceArgumentShape::NestedNew { formal_tree, .. }
        | SourceArgumentShape::PlannedCall { formal_tree, .. }
        | SourceArgumentShape::LocalVal { formal_tree, .. } => formal_tree,
    };
    let TreeKind::Ident(argument_ident) = &source_ast
        .try_get(argument_formal_tree)
        .ok_or_else(|| source_error("constructor argument source root is stale"))?
        .kind
    else {
        return Err(source_error(
            "constructor argument must resolve directly to the caller formal",
        ));
    };
    if store.names.resolve(formal_def.name.as_name().text())
        != store.names.resolve(argument_ident.name.text())
    {
        return Err(source_error(
            "constructor argument must resolve to the exact caller formal",
        ));
    }

    let typed_rhs = source_typed_index
        .get(source_id, source_rhs)
        .ok_or_else(|| typed_error("candidate RHS has no source-to-typed binding"))?;
    let typed_select = source_typed_index
        .get(source_id, source_select)
        .ok_or_else(|| typed_error("constructor selection has no source-to-typed binding"))?;
    let typed_new = source_typed_index
        .get(source_id, source_new)
        .ok_or_else(|| typed_error("New has no source-to-typed binding"))?;
    let typed_new_tpt = source_typed_index
        .get(source_id, source_new_tree.tpt)
        .ok_or_else(|| typed_error("New type tree has no source-to-typed binding"))?;
    let typed_argument = source_typed_index
        .get(source_id, source_argument)
        .ok_or_else(|| typed_error("constructor argument has no source-to-typed binding"))?;
    let typed_formal_argument = source_typed_index
        .get(source_id, argument_formal_tree)
        .ok_or_else(|| typed_error("formal argument has no source-to-typed binding"))?;
    let nested_typed = if let SourceArgumentShape::NestedNew {
        select: nested_select,
        new: nested_new,
        type_tree: nested_new_tpt,
        ..
    } = argument_shape
    {
        Some((
            nested_select,
            nested_new,
            nested_new_tpt,
            source_typed_index
                .get(source_id, nested_select)
                .ok_or_else(|| {
                    typed_error("nested constructor Select has no source-to-typed binding")
                })?,
            source_typed_index
                .get(source_id, nested_new)
                .ok_or_else(|| {
                    typed_error("nested constructor New has no source-to-typed binding")
                })?,
            source_typed_index
                .get(source_id, nested_new_tpt)
                .ok_or_else(|| {
                    typed_error("nested New type tree has no source-to-typed binding")
                })?,
        ))
    } else {
        None
    };
    let planned_parts = match argument_shape {
        SourceArgumentShape::PlannedCall {
            apply,
            function,
            receiver,
            ..
        }
        | SourceArgumentShape::LocalVal {
            apply,
            function,
            receiver,
            ..
        } => Some((apply, function, receiver)),
        _ => None,
    };
    let planned_typed = if let Some((apply, function, receiver)) = planned_parts {
        Some((
            apply,
            function,
            receiver,
            source_typed_index.get(source_id, apply).ok_or_else(|| {
                typed_error("planned helper Apply has no source-to-typed binding")
            })?,
            source_typed_index.get(source_id, function).ok_or_else(|| {
                typed_error("planned helper function has no source-to-typed binding")
            })?,
            receiver
                .map(|tree| {
                    source_typed_index.get(source_id, tree).ok_or_else(|| {
                        typed_error("planned helper receiver has no source-to-typed binding")
                    })
                })
                .transpose()?,
        ))
    } else {
        None
    };
    let typed_root = typed_ast
        .try_get(typed_rhs)
        .ok_or_else(|| typed_error("typed method RHS is stale"))?;
    let typed_constructor_apply = source_typed_index
        .get(source_id, source_constructor_apply)
        .ok_or_else(|| typed_error("constructor expression has no source-to-typed binding"))?;
    let typed_constructor_root = typed_ast
        .try_get(typed_constructor_apply)
        .ok_or_else(|| typed_error("typed constructor expression is stale"))?;
    if let Some(block) = source_block {
        let TreeKind::Block(typed_block) = &typed_root.kind else {
            return Err(typed_error(
                "typed local method RHS must preserve the original Block",
            ));
        };
        let SourceArgumentShape::LocalVal {
            declaration,
            initializer,
            ..
        } = argument_shape
        else {
            return Err(source_error(
                "a constructor Block must use the bounded local-val shape",
            ));
        };
        let typed_declaration = source_typed_index
            .get(source_id, declaration)
            .ok_or_else(|| typed_error("local declaration has no source-to-typed binding"))?;
        let typed_initializer = source_typed_index
            .get(source_id, initializer)
            .ok_or_else(|| typed_error("local initializer has no source-to-typed binding"))?;
        if typed_block.stats != [typed_declaration] || typed_block.expr != typed_constructor_apply {
            return Err(typed_error(
                "typed Block must preserve its one local and final constructor expression",
            ));
        }
        let TreeKind::ValDef(typed_local) = &typed_ast
            .try_get(typed_declaration)
            .ok_or_else(|| typed_error("typed local declaration is stale"))?
            .kind
        else {
            return Err(typed_error("typed local declaration must remain ValDef"));
        };
        if typed_local.rhs != Some(typed_initializer) {
            return Err(typed_error(
                "typed local initializer must retain its exact original root",
            ));
        }
        require_matching_source_typed_positions(
            source_ast,
            typed_ast,
            source_id,
            declaration,
            typed_declaration,
            "local ValDef",
        )?;
        require_matching_source_typed_positions(
            source_ast,
            typed_ast,
            source_id,
            initializer,
            typed_initializer,
            "local initializer",
        )?;
        require_matching_source_typed_positions(
            source_ast,
            typed_ast,
            source_id,
            block.expr,
            typed_constructor_apply,
            "Block result",
        )?;
    } else if typed_constructor_apply != typed_rhs {
        return Err(typed_error(
            "direct constructor body must retain the method RHS root",
        ));
    }
    for (source_tree, typed_tree, description) in [
        (source_rhs, typed_rhs, "method RHS"),
        (source_select, typed_select, "constructor Select"),
        (source_new, typed_new, "constructor New"),
        (source_new_tree.tpt, typed_new_tpt, "New type tree"),
        (source_argument, typed_argument, "constructor argument"),
    ] {
        require_matching_source_typed_positions(
            source_ast,
            typed_ast,
            source_id,
            source_tree,
            typed_tree,
            description,
        )?;
    }
    if let Some((
        source_inner_select,
        source_inner_new,
        source_inner_tpt,
        typed_inner_select,
        typed_inner_new,
        typed_inner_tpt,
    )) = nested_typed
    {
        for (source_tree, typed_tree, description) in [
            (
                source_inner_select,
                typed_inner_select,
                "nested constructor Select",
            ),
            (source_inner_new, typed_inner_new, "nested constructor New"),
            (source_inner_tpt, typed_inner_tpt, "nested New type tree"),
            (
                argument_formal_tree,
                typed_formal_argument,
                "nested constructor argument",
            ),
        ] {
            require_matching_source_typed_positions(
                source_ast,
                typed_ast,
                source_id,
                source_tree,
                typed_tree,
                description,
            )?;
        }
    }
    require_matching_source_typed_positions(
        source_ast,
        typed_ast,
        source_id,
        argument_formal_tree,
        typed_formal_argument,
        "original constructor or helper formal argument",
    )?;
    if let Some((
        source_call,
        source_function,
        source_receiver,
        typed_call,
        typed_function,
        typed_receiver,
    )) = planned_typed
    {
        for (source_tree, typed_tree, description) in [
            (source_call, typed_call, "planned helper Apply"),
            (source_function, typed_function, "planned helper function"),
        ] {
            require_matching_source_typed_positions(
                source_ast,
                typed_ast,
                source_id,
                source_tree,
                typed_tree,
                description,
            )?;
        }
        if let (Some(source_receiver), Some(typed_receiver)) = (source_receiver, typed_receiver) {
            require_matching_source_typed_positions(
                source_ast,
                typed_ast,
                source_id,
                source_receiver,
                typed_receiver,
                "planned helper receiver",
            )?;
        }
    }
    let TreeKind::Apply(typed_apply) = &typed_constructor_root.kind else {
        return Err(typed_error("typed constructor expression is not Apply"));
    };
    if typed_apply.kind != ApplyKind::Regular
        || typed_apply.args != [typed_argument]
        || typed_apply.function != typed_select
        || source_typed_index.get(source_id, source_apply.function) != Some(typed_select)
    {
        return Err(typed_error(
            "typed Apply does not preserve the exact source function and argument roots",
        ));
    }
    let TreeKind::Select(typed_selection) = &typed_ast
        .try_get(typed_select)
        .ok_or_else(|| typed_error("typed constructor selection is stale"))?
        .kind
    else {
        return Err(typed_error("typed constructor function is not Select"));
    };
    if typed_selection.qualifier != typed_new
        || source_typed_index.get(source_id, source_selection.qualifier) != Some(typed_new)
    {
        return Err(typed_error(
            "typed constructor receiver is not the exact mapped New root",
        ));
    }
    let TreeKind::New(typed_new_tree) = &typed_ast
        .try_get(typed_new)
        .ok_or_else(|| typed_error("typed New is stale"))?
        .kind
    else {
        return Err(typed_error("typed constructor receiver is not New"));
    };
    if typed_new_tree.tpt != typed_new_tpt {
        return Err(typed_error(
            "typed New does not retain its exact source type tree",
        ));
    }
    let typed_formal_node = typed_ast
        .try_get(typed_formal_argument)
        .ok_or_else(|| typed_error("typed formal argument is stale"))?;
    let TreeKind::Ident(typed_argument_ident) = &typed_formal_node.kind else {
        return Err(typed_error(
            "typed constructor leaf argument is not an identifier",
        ));
    };
    let local_val_proof_data = if let SourceArgumentShape::LocalVal {
        declaration,
        initializer,
        ..
    } = argument_shape
    {
        let typer = typer.ok_or_else(|| {
            identity_error(
                "local constructor proof requires the original Typer local-binding index",
            )
        })?;
        let TreeKind::ValDef(source_local) = &source_ast
            .try_get(declaration)
            .ok_or_else(|| source_error("local declaration source tree is stale"))?
            .kind
        else {
            return Err(source_error("local declaration source root must be ValDef"));
        };
        if matches!(
            source_ast.try_get(source_local.tpt).map(|tree| &tree.kind),
            Some(TreeKind::TypeTree(_))
        ) {
            return Err(source_error("local val must have an explicit source type"));
        }
        let binding: TypedLocalBinding = typer
            .typed_local_binding_at(source_id, declaration)
            .ok_or_else(|| identity_error("local declaration has no canonical Typer binding"))?;
        let local_symbol = binding.symbol();
        if !store.symbols.contains(local_symbol)
            || !has_source_origin(store, local_symbol, source_id)
            || store.symbols.get(local_symbol).kind != SymbolKind::Local
            || store.symbols.get(local_symbol).owner != Some(method)
            || store
                .symbols
                .get(local_symbol)
                .flags
                .contains(SymbolFlags::MUTABLE)
            || store
                .names
                .resolve(store.symbols.get(local_symbol).name.text())
                != store.names.resolve(source_local.name.as_name().text())
            || method_def
                .value_param_clauses
                .iter()
                .flatten()
                .filter_map(
                    |tree| match source_ast.try_get(*tree).map(|node| &node.kind) {
                        Some(TreeKind::ValDef(parameter)) => Some(parameter.name.as_name().text()),
                        _ => None,
                    },
                )
                .any(|name| {
                    store.names.resolve(name)
                        == store.names.resolve(source_local.name.as_name().text())
                })
        {
            return Err(identity_error(
                "local must be the original immutable method-owned source declaration with an unshadowed name",
            ));
        }
        let local_scope = binding.scope();
        if !store.scopes.contains(local_scope) {
            return Err(identity_error("local binding scope is stale"));
        }
        let scope = store.scopes.get(local_scope);
        if scope.owner != Some(method)
            || scope.lookup(&store.symbols.get(local_symbol).name) != Some(local_symbol)
            || scope.entered_symbols().count() != 1
        {
            return Err(identity_error(
                "local binding must occupy its exact single-entry method-owned block scope",
            ));
        }
        let SymbolInfo::Complete(local_type) = store.symbols.get(local_symbol).info else {
            return Err(semantic_error("local symbol type is incomplete"));
        };
        let local_class = match store.types.try_get(local_type) {
            Some(Type::TypeRef {
                target: TypeRefTarget::Symbol(class),
                ..
            }) if store.symbols.contains(*class)
                && store.symbols.get(*class).kind == SymbolKind::Class =>
            {
                *class
            }
            _ => {
                return Err(semantic_error(
                    "local type must be one canonical class TypeRef",
                ));
            }
        };
        if local_class == caller_class
            || source_index.definition_of(local_class).is_none_or(
                |definition| !matches!(definition, SourceDefinition::Canonical { source, .. } if source == source_id),
            )
            || unique_constructor_call_class_plan(jvm_unit, local_class).is_err()
        {
            return Err(identity_error("local type must be a different canonical class in this source batch"));
        }
        let typed_declaration = binding.declaration();
        let typed_initializer = source_typed_index
            .get(source_id, initializer)
            .ok_or_else(|| typed_error("local initializer has no source-to-typed binding"))?;
        let typed_tpt = source_typed_index
            .get(source_id, source_local.tpt)
            .ok_or_else(|| {
                typed_error("explicit local type tree has no source-to-typed binding")
            })?;
        let TreeKind::ValDef(typed_local) = &typed_ast
            .try_get(typed_declaration)
            .ok_or_else(|| typed_error("canonical typed local declaration is stale"))?
            .kind
        else {
            return Err(typed_error(
                "canonical typed local declaration is not ValDef",
            ));
        };
        if source_typed_index.get(source_id, declaration) != Some(typed_declaration)
            || typed_local.rhs != Some(typed_initializer)
            || typed_local.tpt != typed_tpt
            || typed_ast
                .try_get(typed_tpt)
                .is_none_or(|tree| tree.ty != local_type)
        {
            return Err(typed_error(
                "local binding, explicit TypeRef, initializer and original typed ValDef must agree",
            ));
        }
        let typed_use = typed_argument;
        let typed_use_node = typed_ast
            .try_get(typed_use)
            .ok_or_else(|| typed_error("typed final local Ident is stale"))?;
        let TreeKind::Ident(typed_use_ident) = &typed_use_node.kind else {
            return Err(typed_error("typed final local use must remain Ident"));
        };
        if !matches!(
            store.types.try_get(typed_use_node.ty),
            Some(Type::TermRef { target: TermRefTarget::Symbol(target), .. }) if *target == local_symbol
        ) || store.names.resolve(typed_use_ident.name.text())
            != store
                .names
                .resolve(store.symbols.get(local_symbol).name.text())
        {
            return Err(typed_error(
                "final local Ident must select the exact canonical initialized declaration",
            ));
        }
        let source_use_name = match source_ast.try_get(source_argument).map(|tree| &tree.kind) {
            Some(TreeKind::Ident(ident)) => ident.name,
            _ => {
                return Err(source_error(
                    "final constructor argument must be the original local Ident",
                ));
            }
        };
        if store.names.resolve(source_use_name.text())
            != store
                .names
                .resolve(store.symbols.get(local_symbol).name.text())
        {
            return Err(identity_error(
                "source local Ident does not name its canonical declaration",
            ));
        }
        let declaration_position = require_source_position(
            source_ast,
            source_id,
            declaration,
            "local declaration has a foreign source position",
        )?;
        let initializer_position = require_source_position(
            source_ast,
            source_id,
            initializer,
            "local initializer has a foreign source position",
        )?;
        let use_position = require_source_position(
            source_ast,
            source_id,
            source_argument,
            "local use has a foreign source position",
        )?;
        if declaration_position.span().range().start()
            >= initializer_position.span().range().start()
            || initializer_position.span().range().start() >= use_position.span().range().start()
        {
            return Err(source_error(
                "local declaration and initializer must precede the final local use",
            ));
        }
        require_matching_source_typed_positions(
            source_ast,
            typed_ast,
            source_id,
            source_local.tpt,
            typed_tpt,
            "local declared type",
        )?;
        require_matching_source_typed_positions(
            source_ast,
            typed_ast,
            source_id,
            source_argument,
            typed_use,
            "final local Ident",
        )?;
        Some((
            declaration,
            initializer,
            typed_declaration,
            typed_initializer,
            typed_use,
            local_symbol,
            local_scope,
            local_type,
        ))
    } else {
        None
    };
    require_canonical_caller_formal_seed(
        store.names.resolve(typed_argument_ident.name.text())
            == store.names.resolve(formal_def.name.as_name().text()),
        match store.types.try_get(typed_formal_node.ty) {
            Some(Type::TermRef {
                target: TermRefTarget::Symbol(target),
                ..
            }) => Some(*target),
            _ => None,
        },
        method_formal,
    )?;

    let planned_call_proof = if let (
        Some((source_call, source_function, source_receiver)),
        Some((_, _, _, typed_call, typed_function, typed_receiver)),
    ) = (planned_parts, planned_typed)
    {
        let TreeKind::Apply(source_call_apply) = &source_ast
            .try_get(source_call)
            .ok_or_else(|| source_error("planned helper application source is stale"))?
            .kind
        else {
            return Err(source_error("planned helper source root must be Apply"));
        };
        if source_call_apply.kind != ApplyKind::Regular
            || source_call_apply.args != [argument_formal_tree]
        {
            return Err(source_error(
                "planned helper must have one ordinary argument clause",
            ));
        }
        let typed_function_node = typed_ast
            .try_get(typed_function)
            .ok_or_else(|| typed_error("planned helper typed function is stale"))?;
        let function_shape_matches = match (
            source_ast.try_get(source_function).map(|tree| &tree.kind),
            &typed_function_node.kind,
        ) {
            (Some(TreeKind::Ident(source_ident)), TreeKind::Ident(typed_ident)) => {
                source_receiver.is_none()
                    && typed_receiver.is_none()
                    && store.names.resolve(source_ident.name.text())
                        == store.names.resolve(typed_ident.name.text())
            }
            (Some(TreeKind::Select(source_select)), TreeKind::Select(typed_select)) => {
                source_receiver == Some(source_select.qualifier)
                    && typed_receiver.is_some_and(|receiver| typed_select.qualifier == receiver)
                    && store.names.resolve(source_select.name.text())
                        == store.names.resolve(typed_select.name.text())
            }
            _ => false,
        };
        if !function_shape_matches {
            return Err(typed_error(
                "planned helper typed function must preserve the original Ident or mapped Select receiver edge",
            ));
        }
        let helper = match store.types.try_get(typed_function_node.ty) {
            Some(Type::TermRef {
                target: TermRefTarget::Symbol(helper),
                ..
            }) => *helper,
            _ => {
                return Err(typed_error(
                    "planned helper function must retain a canonical method TermRef",
                ));
            }
        };
        if !store.symbols.contains(helper) {
            return Err(identity_error("planned helper method symbol is stale"));
        }
        let source_function_name = match source_ast.try_get(source_function).map(|tree| &tree.kind)
        {
            Some(TreeKind::Ident(identifier)) => identifier.name,
            Some(TreeKind::Select(selection)) => selection.name,
            _ => {
                return Err(source_error(
                    "planned helper function must be the original Ident or Select",
                ));
            }
        };
        if store.names.resolve(source_function_name.text())
            != store.names.resolve(store.symbols.get(helper).name.text())
        {
            return Err(identity_error(
                "planned helper source name does not match the selected canonical method",
            ));
        }
        let helper_source_tree = match source_index.definition_of(helper) {
            Some(SourceDefinition::Canonical { source, tree }) if source == source_id => tree,
            _ => {
                return Err(identity_error(
                    "planned helper has a foreign source definition",
                ));
            }
        };
        if !store.symbols.contains(helper)
            || !has_source_origin(store, helper, source_id)
            || store.symbols.get(helper).kind != SymbolKind::Method
            || store.symbols.get(helper).owner != Some(caller_class)
            || source_index.symbol_at(source_id, helper_source_tree) != Some(helper)
            || store.symbols.get(helper).visibility != dotty_core::Visibility::Public
            || has_any_flag(
                store.symbols.get(helper).flags,
                SymbolFlags::ABSTRACT
                    | SymbolFlags::SYNTHETIC
                    | SymbolFlags::STATIC
                    | SymbolFlags::GIVEN
                    | SymbolFlags::IMPLICIT
                    | SymbolFlags::ERASED
                    | SymbolFlags::EXTENSION,
            )
        {
            return Err(identity_error(
                "planned helper must be a public canonical instance method owned by the caller class",
            ));
        }
        let caller_source = source_index
            .definition_of(caller_class)
            .and_then(|definition| match definition {
                SourceDefinition::Canonical { source, tree } if source == source_id => Some(tree),
                _ => None,
            })
            .ok_or_else(|| {
                identity_error("planned helper caller class has a foreign definition")
            })?;
        let TreeKind::TypeDef(caller_def) = &source_ast
            .try_get(caller_source)
            .ok_or_else(|| identity_error("planned helper owner source is stale"))?
            .kind
        else {
            return Err(identity_error("planned helper owner source is not a class"));
        };
        let TreeKind::Template(caller_template) = &source_ast
            .try_get(caller_def.rhs)
            .ok_or_else(|| identity_error("planned helper owner template is stale"))?
            .kind
        else {
            return Err(identity_error("planned helper owner has no class template"));
        };
        if !caller_template.body.contains(&helper_source_tree) {
            return Err(identity_error(
                "planned helper is outside the caller class's exact source body",
            ));
        }
        let TreeKind::DefDef(helper_def) = &source_ast
            .try_get(helper_source_tree)
            .ok_or_else(|| identity_error("planned helper source tree is stale"))?
            .kind
        else {
            return Err(identity_error(
                "planned helper source definition is not a method",
            ));
        };
        let Some(helper_source_rhs) = helper_def.rhs else {
            return Err(source_error(
                "planned helper must have an original source body",
            ));
        };
        if !helper_def.type_params.is_empty()
            || helper_def.value_param_clauses.len() != 1
            || helper_def.value_param_clauses[0].len() != 1
            || helper_def.metadata.visibility.is_some()
        {
            return Err(source_error(
                "planned helper must be a monomorphic method with one plain formal clause",
            ));
        }
        if caller_template.body.iter().any(|tree| {
            *tree != helper_source_tree
                && source_index
                    .symbol_at(source_id, *tree)
                    .is_some_and(|candidate| {
                        store.symbols.contains(candidate)
                            && store.symbols.get(candidate).kind == SymbolKind::Method
                            && store.symbols.get(candidate).name == store.symbols.get(helper).name
                    })
        }) {
            return Err(source_error(
                "overloaded planned helper methods are unsupported",
            ));
        }
        let source_receiver_is_this = match source_receiver {
            None => true,
            Some(receiver) => matches!(
                source_ast.try_get(receiver).map(|tree| &tree.kind),
                Some(TreeKind::This(dotty_core::ast::This { qual: None }))
            ),
        };
        if !source_receiver_is_this {
            return Err(source_error(
                "planned helper receiver must be the caller's same-instance this",
            ));
        }
        let helper_formal_tree = helper_def.value_param_clauses[0][0];
        let TreeKind::ValDef(helper_formal_def) = &source_ast
            .try_get(helper_formal_tree)
            .ok_or_else(|| source_error("planned helper formal source tree is stale"))?
            .kind
        else {
            return Err(source_error(
                "planned helper formal must be a value parameter",
            ));
        };
        if helper_formal_def.rhs.is_some() {
            return Err(source_error(
                "planned helper default arguments are unsupported",
            ));
        }
        let helper_formal = source_index
            .symbol_at(source_id, helper_formal_tree)
            .ok_or_else(|| identity_error("planned helper formal has no canonical symbol"))?;
        if source_index.definition_of(helper_formal)
            != Some(SourceDefinition::Canonical {
                source: source_id,
                tree: helper_formal_tree,
            })
            || !store.symbols.contains(helper_formal)
            || !has_source_origin(store, helper_formal, source_id)
            || store.symbols.get(helper_formal).kind != SymbolKind::Parameter
            || store.symbols.get(helper_formal).owner != Some(helper)
        {
            return Err(identity_error(
                "planned helper formal must be the canonical parameter owned by its method",
            ));
        }
        let SymbolInfo::Complete(helper_type) = store.symbols.get(helper).info else {
            return Err(semantic_error("planned helper signature is incomplete"));
        };
        let Some(Type::Method(helper_signature)) = store.types.try_get(helper_type) else {
            return Err(semantic_error(
                "planned helper signature is not a method type",
            ));
        };
        let SymbolInfo::Complete(helper_formal_type) = store.symbols.get(helper_formal).info else {
            return Err(semantic_error("planned helper formal type is incomplete"));
        };
        let helper_formal_type_matches = helper_formal_type == formal_type
            || canonical_class_type_symbol(
                store,
                helper_formal_type,
                package,
                definitions.no_prefix,
            )
            .is_some_and(|helper_class| {
                canonical_class_type_symbol(store, formal_type, package, definitions.no_prefix)
                    == Some(helper_class)
            });
        if helper_signature.kind != dotty_core::MethodKind::Plain
            || helper_signature.params.len() != 1
            || helper_signature.params[0].ty != helper_formal_type
            || !helper_formal_type_matches
        {
            return Err(semantic_error(
                "planned helper must accept the exact caller formal type in one ordinary parameter",
            ));
        }
        let helper_result_class = canonical_class_type_symbol(
            store,
            helper_signature.result,
            package,
            definitions.no_prefix,
        )
        .ok_or_else(|| {
            semantic_error("planned helper must return one canonical same-batch class type")
        })?;
        if helper_result_class == caller_class
            || !store.symbols.contains(helper_result_class)
            || store.symbols.get(helper_result_class).kind != SymbolKind::Class
            || store.symbols.get(helper_result_class).owner != Some(package)
            || !has_source_origin(store, helper_result_class, source_id)
            || source_index
                .definition_of(helper_result_class)
                .is_none_or(|definition| {
                    !matches!(definition, SourceDefinition::Canonical { source, .. } if source == source_id)
                })
            || unique_constructor_call_class_plan(jvm_unit, helper_result_class).is_err()
        {
            return Err(identity_error(
                "planned helper result must be a distinct canonical class in the same JVM unit",
            ));
        }
        let TreeKind::Apply(typed_call_apply) = &typed_ast
            .try_get(typed_call)
            .ok_or_else(|| typed_error("planned helper typed Apply is stale"))?
            .kind
        else {
            return Err(typed_error("planned helper typed call is not Apply"));
        };
        if source_typed_index.get(source_id, source_call) != Some(typed_call)
            || typed_call_apply.kind != ApplyKind::Regular
            || typed_call_apply.function != typed_function
            || typed_call_apply.args != [typed_formal_argument]
            || source_typed_index.get(source_id, source_call) != Some(typed_call)
            || source_typed_index.get(source_id, source_function) != Some(typed_function)
            || source_typed_index.get(source_id, source_call_apply.args[0])
                != Some(typed_formal_argument)
        {
            return Err(typed_error(
                "planned helper Apply must retain the exact mapped function and original formal argument",
            ));
        }
        let (TreeKind::Ident(_) | TreeKind::Select(_)) = &typed_function_node.kind else {
            return Err(typed_error(
                "planned helper typed function must retain its original Ident or Select",
            ));
        };
        let selected_helper = match store.types.try_get(typed_function_node.ty) {
            Some(Type::TermRef {
                prefix,
                target: TermRefTarget::Symbol(symbol),
            }) if *symbol == helper => match store.types.try_get(*prefix) {
                Some(Type::ThisType { class }) if *class == caller_class => helper,
                _ => {
                    return Err(typed_error(
                        "planned helper selection must retain the caller's exact this receiver",
                    ));
                }
            },
            _ => {
                return Err(typed_error(
                    "planned helper selection must retain its canonical method TermRef",
                ));
            }
        };
        if let (Some(source_receiver), Some(typed_receiver)) = (source_receiver, typed_receiver) {
            if source_typed_index.get(source_id, source_receiver) != Some(typed_receiver)
                || !matches!(
                    source_ast.try_get(source_receiver).map(|tree| &tree.kind),
                    Some(TreeKind::This(dotty_core::ast::This { qual: None }))
                )
                || !matches!(
                    typed_ast.try_get(typed_receiver).map(|tree| &tree.kind),
                    Some(TreeKind::This(dotty_core::ast::This { qual: None }))
                )
                || !matches!(
                    typed_ast.try_get(typed_receiver).and_then(|tree| store.types.try_get(tree.ty)),
                    Some(Type::ThisType { class }) if *class == caller_class
                )
            {
                return Err(typed_error(
                    "planned helper explicit receiver must be the exact mapped caller this tree",
                ));
            }
        } else if typed_receiver.is_some() {
            return Err(typed_error(
                "unqualified planned helper must not acquire a synthetic receiver tree",
            ));
        }
        let typed_result_type = typed_ast
            .try_get(typed_call)
            .ok_or_else(|| typed_error("planned helper result tree is stale"))?
            .ty;
        if !is_canonical_class_type_ref(
            store,
            helper_signature.result,
            helper_result_class,
            package,
            definitions.no_prefix,
        ) || !is_canonical_class_type_ref(
            store,
            typed_result_type,
            helper_result_class,
            package,
            definitions.no_prefix,
        ) {
            return Err(typed_error(
                "planned helper call result must retain the exact canonical class TypeRef",
            ));
        }
        let typed_helper_rhs = source_typed_index
            .get(source_id, helper_source_rhs)
            .ok_or_else(|| typed_error("planned helper body has no source-to-typed binding"))?;
        require_source_position(
            source_ast,
            source_id,
            helper_source_tree,
            "planned helper method declaration has a foreign source position",
        )?;
        require_matching_source_typed_positions(
            source_ast,
            typed_ast,
            source_id,
            helper_source_rhs,
            typed_helper_rhs,
            "planned helper body",
        )?;
        let helper_plan = caller_plan
            .methods
            .iter()
            .filter(|plan| plan.method == selected_helper)
            .collect::<Vec<_>>();
        if helper_plan.len() != 1
            || helper_plan[0].parameters != [helper_formal]
            || helper_plan[0].body != Some(typed_helper_rhs)
        {
            return Err(CrossClassConstructorCallProofError::JvmPlan(
                "planned helper JVM method plan must retain the exact body and formal",
            ));
        }
        Some(PlannedCallProof {
            source_apply: source_call,
            source_function,
            source_argument: argument_formal_tree,
            source_receiver,
            helper_source_tree,
            helper_source_rhs,
            typed_apply: typed_call,
            typed_function,
            typed_argument: typed_formal_argument,
            typed_receiver,
            helper_typed_rhs: typed_helper_rhs,
            owner: caller_class,
            method: helper,
            formal: method_formal,
            helper_formal,
            formal_type,
            result_type: typed_result_type,
        })
    } else {
        None
    };

    let typed_new_type = typed_ast
        .try_get(typed_new_tpt)
        .ok_or_else(|| typed_error("typed New type tree is stale"))?
        .ty;
    let target_class = match store.types.try_get(typed_new_type) {
        Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(target),
            ..
        }) => *target,
        _ => {
            return Err(typed_error(
                "New type tree does not retain a canonical class TypeRef",
            ));
        }
    };
    let (nested_argument_proof, argument_type) = if let Some((
        source_inner_select,
        source_inner_new,
        _source_inner_tpt,
        typed_inner_select,
        typed_inner_new,
        typed_inner_tpt,
    )) = nested_typed
    {
        let TreeKind::Apply(inner_apply) = &typed_ast
            .try_get(typed_argument)
            .ok_or_else(|| typed_error("typed nested constructor application is stale"))?
            .kind
        else {
            return Err(typed_error(
                "typed nested constructor argument is not Apply",
            ));
        };
        if inner_apply.kind != ApplyKind::Regular
            || inner_apply.args != [typed_formal_argument]
            || inner_apply.function != typed_inner_select
            || source_typed_index.get(source_id, source_apply.args[0]) != Some(typed_argument)
        {
            return Err(typed_error(
                "typed nested Apply does not preserve its exact source function and formal argument",
            ));
        }
        let TreeKind::Select(inner_selection) = &typed_ast
            .try_get(typed_inner_select)
            .ok_or_else(|| typed_error("typed nested constructor Select is stale"))?
            .kind
        else {
            return Err(typed_error(
                "typed nested constructor function is not Select",
            ));
        };
        let TreeKind::New(inner_new_tree) = &typed_ast
            .try_get(typed_inner_new)
            .ok_or_else(|| typed_error("typed nested New is stale"))?
            .kind
        else {
            return Err(typed_error("typed nested constructor receiver is not New"));
        };
        if inner_selection.qualifier != typed_inner_new
            || source_typed_index.get(source_id, source_inner_select) != Some(typed_inner_select)
            || source_typed_index.get(source_id, source_inner_new) != Some(typed_inner_new)
            || inner_new_tree.tpt != typed_inner_tpt
        {
            return Err(typed_error(
                "typed nested constructor call does not preserve the exact source Select, New, and type roots",
            ));
        }
        let inner_type = typed_ast
            .try_get(typed_inner_tpt)
            .ok_or_else(|| typed_error("typed nested New type tree is stale"))?
            .ty;
        let inner_class = match store.types.try_get(inner_type) {
            Some(Type::TypeRef {
                target: TypeRefTarget::Symbol(class),
                ..
            }) => *class,
            _ => {
                return Err(typed_error(
                    "nested New type tree does not retain a canonical class TypeRef",
                ));
            }
        };
        if inner_class == caller_class || inner_class == target_class || !store.symbols.contains(inner_class)
            || store.symbols.get(inner_class).kind != SymbolKind::Class
            || !has_source_origin(store, inner_class, source_id)
            || source_index.definition_of(inner_class).is_none_or(|definition| !matches!(definition, SourceDefinition::Canonical { source, .. } if source == source_id))
            || unique_constructor_call_class_plan(jvm_unit, inner_class).is_err()
        {
            return Err(identity_error("nested constructor target must be a distinct canonical class planned in the same source batch"));
        }
        if !is_canonical_class_type_ref(
            store,
            inner_type,
            inner_class,
            package,
            definitions.no_prefix,
        ) {
            return Err(typed_error(
                "nested New type tree must retain the exact canonical inner class TypeRef",
            ));
        }
        let inner_constructor = match store.types.try_get(
            typed_ast
                .try_get(typed_inner_select)
                .ok_or_else(|| typed_error("typed nested Select is stale"))?
                .ty,
        ) {
            Some(Type::TermRef {
                target: TermRefTarget::Symbol(constructor),
                ..
            }) => *constructor,
            _ => {
                return Err(typed_error(
                    "typed nested Select does not retain a canonical constructor TermRef",
                ));
            }
        };
        if !store.symbols.contains(inner_constructor)
            || store.symbols.get(inner_constructor).kind != SymbolKind::Constructor
            || store.symbols.get(inner_constructor).owner != Some(inner_class)
            || !has_source_origin(store, inner_constructor, source_id)
        {
            return Err(semantic_error(
                "nested Select must choose a canonical constructor owned by its New class",
            ));
        }
        let inner_source = match source_index.definition_of(inner_class) {
            Some(SourceDefinition::Canonical { source, tree }) if source == source_id => tree,
            _ => {
                return Err(identity_error(
                    "nested constructor class has no canonical declaration in this source unit",
                ));
            }
        };
        let Some(inner_ordinal) = source_class_trees
            .iter()
            .position(|tree| *tree == inner_source)
        else {
            return Err(identity_error(
                "nested constructor class is outside the exact package source root",
            ));
        };
        if jvm_unit
            .classes
            .get(inner_ordinal)
            .is_none_or(|plan| plan.class != inner_class)
        {
            return Err(CrossClassConstructorCallProofError::JvmPlan(
                "nested constructor class is absent from the same ordered JVM batch",
            ));
        }
        let inner_tree = source_ast
            .try_get(inner_source)
            .ok_or_else(|| identity_error("nested class source tree is stale"))?;
        let TreeKind::TypeDef(inner_def) = &inner_tree.kind else {
            return Err(identity_error("nested class source definition is invalid"));
        };
        let inner_template_tree = source_ast
            .try_get(inner_def.rhs)
            .ok_or_else(|| identity_error("nested class template is stale"))?;
        let TreeKind::Template(inner_template) = &inner_template_tree.kind else {
            return Err(identity_error("nested class has no template"));
        };
        require_source_position(
            source_ast,
            source_id,
            inner_source,
            "nested constructor class has a foreign source position",
        )?;
        if !inner_def.metadata.modifiers.is_empty()
            || !inner_def.metadata.annotations.is_empty()
            || !inner_template.parents.is_empty()
            || inner_template.self_val.is_some()
        {
            return Err(source_error(
                "nested constructor class must be ordinary and have no explicit parents or annotations",
            ));
        }
        let canonical_inner_constructor = source_index
            .symbol_at(source_id, inner_template.constructor)
            .ok_or_else(|| identity_error("nested class has no canonical primary constructor"))?;
        if canonical_inner_constructor != inner_constructor
            || source_index.definition_of(inner_constructor)
                != Some(SourceDefinition::Canonical {
                    source: source_id,
                    tree: inner_template.constructor,
                })
        {
            return Err(identity_error(
                "nested Select does not name the exact source primary constructor",
            ));
        }
        if inner_template.body.iter().any(|tree| {
            source_index
                .symbol_at(source_id, *tree)
                .is_some_and(|symbol| {
                    store.symbols.contains(symbol)
                        && store.symbols.get(symbol).kind == SymbolKind::Constructor
                        && symbol != inner_constructor
                })
        }) {
            return Err(source_error(
                "secondary or overloaded nested constructors are unsupported",
            ));
        }
        let inner_plan = unique_constructor_call_class_plan(jvm_unit, inner_class)?;
        let dotty_jvm::JvmClassInitializationPlan::PrimaryConstructor(inner_primary) =
            &inner_plan.initialization
        else {
            return Err(CrossClassConstructorCallProofError::JvmPlan(
                "nested JVM class has no validated primary constructor",
            ));
        };
        if inner_primary.symbol() != inner_constructor {
            return Err(CrossClassConstructorCallProofError::JvmPlan(
                "nested JVM constructor plan does not retain the exact primary constructor",
            ));
        }
        let TreeKind::DefDef(inner_constructor_def) = &source_ast
            .try_get(inner_template.constructor)
            .ok_or_else(|| identity_error("nested primary constructor source is stale"))?
            .kind
        else {
            return Err(identity_error(
                "nested primary constructor source is invalid",
            ));
        };
        require_source_position(
            source_ast,
            source_id,
            inner_template.constructor,
            "nested primary constructor has a foreign source position",
        )?;
        if !inner_constructor_def.type_params.is_empty()
            || inner_constructor_def.value_param_clauses.len() != 1
            || inner_constructor_def.value_param_clauses[0].len() != 1
            || inner_constructor_def.metadata.visibility.is_some()
        {
            return Err(source_error(
                "nested constructor must have one plain non-generic formal clause",
            ));
        }
        let inner_formal_tree = inner_constructor_def.value_param_clauses[0][0];
        require_source_position(
            source_ast,
            source_id,
            inner_formal_tree,
            "nested constructor formal has a foreign source position",
        )?;
        let TreeKind::ValDef(inner_formal_def) = &source_ast
            .try_get(inner_formal_tree)
            .ok_or_else(|| identity_error("nested constructor formal source is stale"))?
            .kind
        else {
            return Err(source_error("nested constructor formal is invalid"));
        };
        if inner_formal_def.rhs.is_some() {
            return Err(source_error(
                "default nested-constructor parameters are unsupported",
            ));
        }
        let inner_formal = source_index
            .derived_symbol_at(inner_constructor, source_id, inner_formal_tree)
            .ok_or_else(|| {
                identity_error(
                    "nested constructor formal is not derived from its exact primary constructor",
                )
            })?;
        if !store.symbols.contains(inner_formal)
            || !has_source_origin(store, inner_formal, source_id)
            || store.symbols.get(inner_formal).kind != SymbolKind::Parameter
            || store.symbols.get(inner_formal).owner != Some(inner_constructor)
        {
            return Err(identity_error(
                "nested constructor formal is not the canonical source parameter owned by its constructor",
            ));
        }
        if inner_primary.parameters() != [inner_formal] {
            return Err(CrossClassConstructorCallProofError::JvmPlan(
                "nested JVM constructor plan does not retain its canonical formal",
            ));
        }
        if store.symbols.get(inner_class).visibility != dotty_core::Visibility::Public
            || has_any_flag(
                store.symbols.get(inner_class).flags,
                SymbolFlags::CASE
                    | SymbolFlags::ENUM
                    | SymbolFlags::ABSTRACT
                    | SymbolFlags::SYNTHETIC,
            )
            || store.symbols.get(inner_constructor).visibility != dotty_core::Visibility::Public
            || has_any_flag(
                store.symbols.get(inner_constructor).flags,
                SymbolFlags::ABSTRACT | SymbolFlags::SYNTHETIC,
            )
        {
            return Err(semantic_error(
                "nested target must have a public ordinary class and public primary constructor",
            ));
        }
        let SymbolInfo::Complete(inner_signature_type) = store.symbols.get(inner_constructor).info
        else {
            return Err(semantic_error("nested constructor signature is incomplete"));
        };
        let Some(Type::Method(inner_signature)) = store.types.try_get(inner_signature_type) else {
            return Err(semantic_error(
                "nested constructor signature is not a method type",
            ));
        };
        let SymbolInfo::Complete(inner_formal_type) = store.symbols.get(inner_formal).info else {
            return Err(semantic_error(
                "nested constructor formal type is incomplete",
            ));
        };
        if let Some(typed_formal_syntax) = source_typed_index.get(source_id, inner_formal_def.tpt) {
            let typed_formal_syntax = typed_ast
                .try_get(typed_formal_syntax)
                .ok_or_else(|| typed_error("nested constructor formal type annotation is stale"))?;
            if typed_formal_syntax.ty != inner_formal_type {
                return Err(typed_error(
                    "nested constructor formal annotation does not match its completed signature",
                ));
            }
        }
        if inner_signature.kind != dotty_core::MethodKind::Plain
            || inner_signature.params.len() != 1
            || inner_signature.params[0].ty != inner_formal_type
            || inner_formal_type != definitions.int
            || formal_type != definitions.int
            || inner_formal_type != formal_type
        {
            return Err(semantic_error(
                "nested constructor and factory formal must both have the canonical Int type",
            ));
        }
        if !matches!(store.types.try_get(typed_formal_node.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }) if *symbol == method_formal)
        {
            return Err(typed_error(
                "nested constructor argument must retain the original factory formal TermRef and exact type",
            ));
        }
        if !matches!(store.types.try_get(typed_ast.try_get(typed_argument).ok_or_else(|| typed_error("typed nested Apply is stale"))?.ty), Some(Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }) if *class == inner_class)
        {
            return Err(typed_error(
                "nested constructor application result must retain its exact inner class TypeRef",
            ));
        }
        (
            Some(NestedConstructorArgumentProof {
                source_outer_argument: source_argument,
                typed_outer_argument: typed_argument,
                source_apply: source_argument,
                typed_apply: typed_argument,
                source_select: source_inner_select,
                source_new: source_inner_new,
                source_argument: argument_formal_tree,
                typed_select: typed_inner_select,
                typed_new: typed_inner_new,
                typed_argument: typed_formal_argument,
                class: inner_class,
                constructor: inner_constructor,
                formal: inner_formal,
                argument_formal: method_formal,
                formal_type: inner_formal_type,
                argument_formal_type: formal_type,
                result_type: typed_ast
                    .try_get(typed_argument)
                    .ok_or_else(|| typed_error("typed nested Apply is stale"))?
                    .ty,
            }),
            inner_type,
        )
    } else if let Some((.., local_type)) = local_val_proof_data {
        let local_symbol = local_val_proof_data.expect("matched local binding").5;
        let typed_local_argument = typed_ast
            .try_get(typed_argument)
            .ok_or_else(|| typed_error("typed constructor local Ident is stale"))?;
        if !matches!(
            store.types.try_get(typed_local_argument.ty),
            Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. })
                if *symbol == local_symbol
        ) {
            return Err(typed_error(
                "constructor local Ident lost its canonical TermRef",
            ));
        }
        (None, local_type)
    } else if let Some(planned_call) = planned_call_proof {
        (None, planned_call.result_type())
    } else {
        if argument_formal_tree != source_argument || typed_formal_argument != typed_argument {
            return Err(typed_error(
                "direct constructor argument mapping is inconsistent",
            ));
        }
        (None, formal_type)
    };
    if target_class == caller_class
        || formal_class == Some(target_class)
        || !store.symbols.contains(target_class)
        || !has_source_origin(store, target_class, source_id)
        || store.symbols.get(target_class).kind != SymbolKind::Class
    {
        return Err(identity_error(
            "constructor target must be another canonical class",
        ));
    }
    let target_source = match source_index.definition_of(target_class) {
        Some(SourceDefinition::Canonical { source, tree }) if source == source_id => tree,
        _ => {
            return Err(identity_error(
                "constructor target class is not declared in this source unit",
            ));
        }
    };
    let Some(target_ordinal) = source_class_trees
        .iter()
        .position(|tree| *tree == target_source)
    else {
        return Err(identity_error(
            "constructor target class is outside the exact package source root",
        ));
    };
    if jvm_unit
        .classes
        .get(target_ordinal)
        .is_none_or(|plan| plan.class != target_class)
    {
        return Err(CrossClassConstructorCallProofError::JvmPlan(
            "selected constructor target is absent from the same ordered JVM batch",
        ));
    }
    let TreeKind::TypeDef(target_def) = &source_ast
        .try_get(target_source)
        .ok_or_else(|| identity_error("target class source tree is stale"))?
        .kind
    else {
        return Err(identity_error("target class source definition is invalid"));
    };
    let TreeKind::Template(target_template) = &source_ast
        .try_get(target_def.rhs)
        .ok_or_else(|| identity_error("target class template source tree is stale"))?
        .kind
    else {
        return Err(identity_error("target class template is invalid"));
    };
    let target_constructor_tree = target_template.constructor;
    require_source_position(
        source_ast,
        source_id,
        target_source,
        "constructor target has a foreign source position",
    )?;
    require_source_position(
        source_ast,
        source_id,
        target_constructor_tree,
        "primary constructor has a foreign source position",
    )?;
    let target_constructor = source_index
        .symbol_at(source_id, target_constructor_tree)
        .ok_or_else(|| {
            identity_error("target primary constructor has no canonical source symbol")
        })?;
    if source_index.definition_of(target_constructor)
        != Some(SourceDefinition::Canonical {
            source: source_id,
            tree: target_constructor_tree,
        })
    {
        return Err(identity_error(
            "selected target constructor is not bound to its exact primary-constructor source tree",
        ));
    }
    let typed_select_type = typed_ast
        .try_get(typed_select)
        .ok_or_else(|| typed_error("typed constructor selection is stale"))?
        .ty;
    let selected_constructor = match store.types.try_get(typed_select_type) {
        Some(Type::TermRef {
            target: TermRefTarget::Symbol(target),
            ..
        }) => *target,
        _ => {
            return Err(typed_error(
                "typed Select does not retain a canonical constructor TermRef",
            ));
        }
    };
    if selected_constructor != target_constructor
        || !store.symbols.contains(target_constructor)
        || !has_source_origin(store, target_constructor, source_id)
        || !has_source_origin(store, selected_constructor, source_id)
        || store.symbols.get(selected_constructor).kind != SymbolKind::Constructor
        || store.symbols.get(selected_constructor).owner != Some(target_class)
    {
        return Err(CrossClassConstructorCallProofError::SemanticIdentity(
            "typed Select must choose the exact target class primary constructor",
        ));
    }
    if target_template.body.iter().any(|tree| {
        source_index
            .symbol_at(source_id, *tree)
            .is_some_and(|symbol| {
                store.symbols.get(symbol).kind == SymbolKind::Constructor
                    && symbol != target_constructor
            })
    }) {
        return Err(source_error(
            "secondary or overloaded constructors are unsupported",
        ));
    }
    if !target_def.metadata.modifiers.is_empty()
        || !target_def.metadata.annotations.is_empty()
        || !target_template.parents.is_empty()
        || target_template.self_val.is_some()
    {
        return Err(source_error(
            "target must be an ordinary class without explicit parents or annotations",
        ));
    }
    let TreeKind::DefDef(target_constructor_def) = &source_ast
        .try_get(target_constructor_tree)
        .ok_or_else(|| identity_error("primary constructor source tree is stale"))?
        .kind
    else {
        return Err(identity_error("primary constructor source is invalid"));
    };
    if !target_constructor_def.type_params.is_empty()
        || target_constructor_def.value_param_clauses.len() != 1
        || target_constructor_def.value_param_clauses[0].len() != 1
        || target_constructor_def.metadata.visibility.is_some()
    {
        return Err(source_error(
            "target constructor must be one plain public non-generic formal clause",
        ));
    }
    let constructor_formal_tree = target_constructor_def.value_param_clauses[0][0];
    require_source_position(
        source_ast,
        source_id,
        constructor_formal_tree,
        "constructor formal has a foreign source position",
    )?;
    let TreeKind::ValDef(constructor_formal_def) = &source_ast
        .try_get(constructor_formal_tree)
        .ok_or_else(|| identity_error("constructor formal source tree is stale"))?
        .kind
    else {
        return Err(source_error("target constructor formal is invalid"));
    };
    if constructor_formal_def.rhs.is_some() {
        return Err(source_error(
            "default constructor parameters are unsupported",
        ));
    }
    let constructor_formal = source_index
        .derived_symbol_at(target_constructor, source_id, constructor_formal_tree)
        .ok_or_else(|| {
            identity_error(
                "target constructor formal is not canonically derived from its primary constructor",
            )
        })?;
    if !has_source_origin(store, constructor_formal, source_id) {
        return Err(identity_error(
            "target constructor formal does not have the supplied source origin",
        ));
    }
    let SymbolInfo::Complete(constructor_type) = store.symbols.get(target_constructor).info else {
        return Err(semantic_error("target constructor signature is incomplete"));
    };
    let Some(Type::Method(constructor_signature)) = store.types.try_get(constructor_type) else {
        return Err(semantic_error(
            "target constructor must have one plain method type",
        ));
    };
    let SymbolInfo::Complete(constructor_formal_type) = store.symbols.get(constructor_formal).info
    else {
        return Err(semantic_error(
            "target constructor formal type is incomplete",
        ));
    };
    let same_reference_type = |left: TypeId, right: TypeId| {
        let left = canonical_class_type_symbol(store, left, package, definitions.no_prefix);
        left.is_some()
            && left == canonical_class_type_symbol(store, right, package, definitions.no_prefix)
    };
    let constructor_argument_type_matches = if let Some(nested) = nested_argument_proof {
        is_canonical_class_type_ref(
            store,
            constructor_formal_type,
            nested.class(),
            package,
            definitions.no_prefix,
        )
    } else if let Some(planned_call) = planned_call_proof {
        let Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(class),
            ..
        }) = store.types.try_get(planned_call.result_type())
        else {
            return Err(typed_error(
                "planned helper result must be a canonical class TypeRef",
            ));
        };
        is_canonical_class_type_ref(
            store,
            constructor_formal_type,
            *class,
            package,
            definitions.no_prefix,
        )
    } else {
        constructor_formal_type == formal_type
            || same_reference_type(constructor_formal_type, formal_type)
    };
    if constructor_signature.kind != dotty_core::MethodKind::Plain
        || constructor_signature.params.len() != 1
        || constructor_signature.params[0].ty != constructor_formal_type
        || !constructor_argument_type_matches
        || ((nested_argument_proof.is_some() || planned_call_proof.is_some())
            && !same_reference_type(constructor_formal_type, argument_type))
    {
        return Err(semantic_error(
            "caller formal and primary constructor formal must have the same exact completed type",
        ));
    }
    if store.symbols.get(target_class).visibility != dotty_core::Visibility::Public
        || has_any_flag(
            store.symbols.get(target_class).flags,
            SymbolFlags::CASE | SymbolFlags::ENUM | SymbolFlags::ABSTRACT | SymbolFlags::SYNTHETIC,
        )
        || store.symbols.get(target_constructor).visibility != dotty_core::Visibility::Public
        || has_any_flag(
            store.symbols.get(target_constructor).flags,
            SymbolFlags::ABSTRACT | SymbolFlags::SYNTHETIC,
        )
    {
        return Err(semantic_error(
            "target must be public, ordinary, concrete, and have a public primary constructor",
        ));
    }
    let caller_symbol = store.symbols.get(caller_class);
    if caller_symbol.visibility != dotty_core::Visibility::Public
        || has_any_flag(
            caller_symbol.flags,
            SymbolFlags::CASE | SymbolFlags::ENUM | SymbolFlags::ABSTRACT | SymbolFlags::SYNTHETIC,
        )
    {
        return Err(semantic_error(
            "caller must be a public ordinary concrete class",
        ));
    }
    if store.symbols.get(target_class).owner != Some(package)
        || store.symbols.get(package).kind != SymbolKind::Package
    {
        return Err(semantic_error(
            "caller and constructor target must share one exact package owner",
        ));
    }
    let mut proof_classes = vec![caller_class, target_class];
    if let Some(nested) = nested_argument_proof {
        proof_classes.push(nested.class());
    }
    for class in proof_classes {
        let scope = factory_class_declarations(store, source_index, class)?;
        if scope.entered_symbols().any(|symbol| {
            store.symbols.contains(symbol)
                && store.symbols.get(symbol).kind == SymbolKind::TypeParameter
                && store.symbols.get(symbol).owner == Some(class)
        }) {
            return Err(source_error(
                "generic caller and target classes are unsupported",
            ));
        }
        let expected_member = match (class, nested_argument_proof) {
            (class, _) if class == caller_class => method,
            (class, _) if class == target_class => target_constructor,
            (class, Some(nested)) if class == nested.class() => nested.constructor(),
            _ => {
                return Err(identity_error(
                    "proof class has no matching canonical member",
                ));
            }
        };
        if !scope
            .entered_symbols()
            .any(|symbol| symbol == expected_member)
        {
            return Err(identity_error(
                "method or constructor is absent from its canonical owner declaration scope",
            ));
        }
    }
    let result_type = method_signature.result;
    if !is_canonical_class_type_ref(
        store,
        result_type,
        target_class,
        package,
        definitions.no_prefix,
    ) {
        return Err(semantic_error(
            "method signature result must retain the exact target class TypeRef",
        ));
    }
    let typed_rhs_type = typed_ast
        .try_get(typed_rhs)
        .ok_or_else(|| typed_error("typed method RHS is stale"))?
        .ty;
    if !matches!(
        store.types.try_get(typed_rhs_type),
        Some(Type::TypeRef { target: TypeRefTarget::Symbol(target), .. }) if *target == target_class
    ) {
        return Err(typed_error(
            "typed method result must retain the exact target class TypeRef",
        ));
    }
    if !is_canonical_class_type_ref(
        store,
        typed_new_type,
        target_class,
        package,
        definitions.no_prefix,
    ) {
        return Err(typed_error(
            "New type tree must retain the exact target class TypeRef",
        ));
    }
    if let Some(typed_result_syntax) = source_typed_index.get(source_id, method_def.tpt) {
        let typed_result_syntax = typed_ast
            .try_get(typed_result_syntax)
            .ok_or_else(|| typed_error("source result annotation binding is stale"))?;
        if !is_canonical_class_type_ref(
            store,
            typed_result_syntax.ty,
            target_class,
            package,
            definitions.no_prefix,
        ) {
            return Err(typed_error(
                "source result annotation does not resolve to the exact target class",
            ));
        }
    }
    if let Some(typed_formal_syntax) = source_typed_index.get(source_id, formal_def.tpt) {
        let typed_formal_syntax = typed_ast
            .try_get(typed_formal_syntax)
            .ok_or_else(|| typed_error("source formal annotation binding is stale"))?;
        if typed_formal_syntax.ty != formal_type {
            return Err(typed_error(
                "source formal type annotation does not match the completed formal type",
            ));
        }
    }
    if let Some(typed_constructor_formal_syntax) =
        source_typed_index.get(source_id, constructor_formal_def.tpt)
    {
        let typed_constructor_formal_syntax = typed_ast
            .try_get(typed_constructor_formal_syntax)
            .ok_or_else(|| typed_error("constructor formal annotation binding is stale"))?;
        if typed_constructor_formal_syntax.ty != constructor_formal_type {
            return Err(typed_error(
                "constructor formal type annotation does not match the selected constructor signature",
            ));
        }
    }
    let method_plan = caller_plan
        .methods
        .iter()
        .filter(|plan| plan.method == method)
        .collect::<Vec<_>>();
    if method_plan.len() != 1
        || method_plan[0].parameters != [method_formal]
        || method_plan[0].body != Some(typed_rhs)
    {
        return Err(CrossClassConstructorCallProofError::JvmPlan(
            "JVM method plan must retain the exact canonical formal and typed RHS",
        ));
    }
    let target_plan = unique_constructor_call_class_plan(jvm_unit, target_class)?;
    let dotty_jvm::JvmClassInitializationPlan::PrimaryConstructor(primary) =
        &target_plan.initialization
    else {
        return Err(CrossClassConstructorCallProofError::JvmPlan(
            "target JVM class has no validated primary constructor",
        ));
    };
    if primary.symbol() != target_constructor || primary.parameters() != [constructor_formal] {
        return Err(CrossClassConstructorCallProofError::JvmPlan(
            "target JVM constructor plan does not retain the canonical primary constructor and formal",
        ));
    }
    let target_metadata = metadata
        .iter()
        .find(|entry| entry.identity().symbol() == target_class)
        .ok_or(CrossClassConstructorCallProofError::JvmPlan(
            "target JVM metadata is missing",
        ))?;
    let caller_metadata = metadata
        .iter()
        .find(|entry| entry.identity().symbol() == caller_class)
        .ok_or(CrossClassConstructorCallProofError::JvmPlan(
            "caller JVM metadata is missing",
        ))?;
    let constructor_metadata =
        target_metadata
            .constructor()
            .ok_or(CrossClassConstructorCallProofError::JvmPlan(
                "target primary constructor metadata is missing",
            ))?;
    let method_metadata = caller_metadata
        .methods()
        .iter()
        .find(|entry| entry.symbol() == method)
        .ok_or(CrossClassConstructorCallProofError::JvmPlan(
            "calling method metadata is missing",
        ))?;
    if constructor_metadata.symbol() != target_constructor
        || constructor_metadata
            .parameters()
            .iter()
            .map(|p| p.symbol())
            .collect::<Vec<_>>()
            != [constructor_formal]
        || !constructor_metadata
            .access_flags()
            .contains(dotty_classfile::access_flags::ACC_PUBLIC)
        || method_metadata
            .parameters()
            .iter()
            .map(|p| p.symbol())
            .collect::<Vec<_>>()
            != [method_formal]
        || !method_metadata
            .access_flags()
            .contains(dotty_classfile::access_flags::ACC_PUBLIC)
    {
        return Err(CrossClassConstructorCallProofError::JvmPlan(
            "JVM metadata must retain public method and constructor identities/formals",
        ));
    }
    let constructor_descriptor = constructor_metadata.descriptor().as_str();
    let method_descriptor = method_metadata.descriptor().as_str();
    let constructor_params = constructor_descriptor
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(")V"))
        .ok_or(CrossClassConstructorCallProofError::JvmPlan(
            "constructor descriptor is not one supported parameter clause",
        ))?;
    let method_parameter_descriptor = if let Some(nested) = nested_argument_proof {
        let inner_metadata = metadata
            .iter()
            .find(|entry| entry.identity().symbol() == nested.class())
            .ok_or(CrossClassConstructorCallProofError::JvmPlan(
                "nested constructor class metadata is missing",
            ))?;
        if store.symbols.get(nested.class()).owner != Some(package) {
            return Err(semantic_error(
                "nested constructor class must share the exact caller package owner",
            ));
        }
        let inner_constructor_metadata =
            inner_metadata
                .constructor()
                .ok_or(CrossClassConstructorCallProofError::JvmPlan(
                    "nested primary constructor metadata is missing",
                ))?;
        if inner_constructor_metadata.symbol() != nested.constructor()
            || inner_constructor_metadata
                .parameters()
                .iter()
                .map(|parameter| parameter.symbol())
                .collect::<Vec<_>>()
                != [nested.formal()]
            || !inner_constructor_metadata
                .access_flags()
                .contains(dotty_classfile::access_flags::ACC_PUBLIC)
        {
            return Err(CrossClassConstructorCallProofError::JvmPlan(
                "nested JVM constructor metadata must retain its canonical constructor and formal",
            ));
        }
        let nested_descriptor = inner_constructor_metadata.descriptor().as_str();
        let nested_params = nested_descriptor
            .strip_prefix('(')
            .and_then(|s| s.strip_suffix(")V"))
            .ok_or(CrossClassConstructorCallProofError::JvmPlan(
                "nested constructor descriptor is not one supported parameter clause",
            ))?;
        let expected_outer_parameter = format!("L{};", inner_metadata.identity().internal_name());
        if constructor_params != expected_outer_parameter {
            return Err(CrossClassConstructorCallProofError::JvmPlan(
                "outer constructor descriptor must consume the exact nested class",
            ));
        }
        nested_params
    } else if let Some(planned_call) = planned_call_proof {
        let helper_result_class = match store.types.try_get(planned_call.result_type()) {
            Some(Type::TypeRef {
                target: TypeRefTarget::Symbol(class),
                ..
            }) => *class,
            _ => {
                return Err(typed_error(
                    "planned helper result must retain a canonical class TypeRef",
                ));
            }
        };
        let helper_metadata = caller_metadata
            .methods()
            .iter()
            .find(|entry| entry.symbol() == planned_call.method())
            .ok_or(CrossClassConstructorCallProofError::JvmPlan(
                "planned helper JVM method metadata is missing",
            ))?;
        let helper_result_metadata = metadata
            .iter()
            .find(|entry| entry.identity().symbol() == helper_result_class)
            .ok_or(CrossClassConstructorCallProofError::JvmPlan(
                "planned helper result class metadata is missing",
            ))?;
        if helper_metadata
            .parameters()
            .iter()
            .map(|parameter| parameter.symbol())
            .collect::<Vec<_>>()
            != [planned_call.helper_formal()]
            || !helper_metadata
                .access_flags()
                .contains(dotty_classfile::access_flags::ACC_PUBLIC)
        {
            return Err(CrossClassConstructorCallProofError::JvmPlan(
                "planned helper JVM metadata must retain its public canonical formal",
            ));
        }
        let caller_parameters = method_descriptor
            .strip_prefix('(')
            .and_then(|descriptor| descriptor.split_once(')'))
            .map(|(parameters, _)| parameters)
            .ok_or(CrossClassConstructorCallProofError::JvmPlan(
                "calling method descriptor has no parameter clause",
            ))?;
        let expected_helper_descriptor = format!(
            "({caller_parameters})L{};",
            helper_result_metadata.identity().internal_name()
        );
        let expected_outer_parameter =
            format!("L{};", helper_result_metadata.identity().internal_name());
        if helper_metadata.descriptor().as_str() != expected_helper_descriptor
            || constructor_params != expected_outer_parameter
        {
            return Err(CrossClassConstructorCallProofError::JvmPlan(
                "planned helper and outer constructor descriptors must preserve the exact result and caller formal",
            ));
        }
        caller_parameters
    } else {
        constructor_params
    };
    let expected_method_descriptor = format!(
        "({method_parameter_descriptor})L{};",
        target_metadata.identity().internal_name()
    );
    if method_descriptor != expected_method_descriptor {
        return Err(CrossClassConstructorCallProofError::JvmPlan(
            "method and constructor descriptors do not preserve the exact argument and result class identities",
        ));
    }
    let argument_proof = ConstructorArgumentProof {
        shape: match nested_argument_proof {
            Some(nested) => ConstructorArgumentProofShape::NestedNew(nested),
            None if let Some((
                source_declaration,
                source_initializer,
                typed_declaration,
                typed_initializer,
                typed_use,
                symbol,
                scope,
                local_type,
            )) = local_val_proof_data =>
            {
                let initializer_call = planned_call_proof.ok_or_else(|| {
                    typed_error("local val initializer lacks its validated planned-call receipt")
                })?;
                let initializer_class = match store.types.try_get(initializer_call.result_type()) {
                    Some(Type::TypeRef {
                        target: TypeRefTarget::Symbol(class),
                        ..
                    }) => *class,
                    _ => {
                        return Err(typed_error(
                            "local initializer result must be a canonical class TypeRef",
                        ));
                    }
                };
                let local_class =
                    canonical_class_type_symbol(store, local_type, package, definitions.no_prefix);
                if local_class != Some(initializer_class)
                    || !is_canonical_class_type_ref(
                        store,
                        local_type,
                        initializer_class,
                        package,
                        definitions.no_prefix,
                    )
                    || !is_canonical_class_type_ref(
                        store,
                        initializer_call.result_type(),
                        initializer_class,
                        package,
                        definitions.no_prefix,
                    )
                {
                    return Err(typed_error(
                        "local val type must exactly match its planned initializer result TypeRef",
                    ));
                }
                ConstructorArgumentProofShape::LocalVal(LocalValConstructorArgumentProof {
                    source_block: source_rhs,
                    source_declaration,
                    source_initializer,
                    source_use: source_argument,
                    typed_block: typed_rhs,
                    typed_declaration,
                    typed_initializer,
                    typed_use,
                    symbol,
                    scope,
                    local_type,
                    result_type: typed_rhs_type,
                    initializer_call,
                })
            }
            None if let Some(planned_call) = planned_call_proof => {
                ConstructorArgumentProofShape::PlannedCall(planned_call)
            }
            None => ConstructorArgumentProofShape::Formal(FormalConstructorArgumentProof {
                source_argument,
                typed_argument,
                formal: method_formal,
                formal_type,
                result_type: argument_type,
            }),
        },
    };
    Ok(ValidatedCrossClassConstructorCallProof {
        _store: store,
        _source_ast: source_ast,
        _typed_ast: typed_ast,
        _source_index: source_index,
        _source_typed_index: source_typed_index,
        _jvm_unit: jvm_unit,
        source_id,
        caller_class,
        method,
        method_formal,
        formal_class,
        target_class,
        constructor: target_constructor,
        source_rhs,
        source_select,
        source_new,
        source_argument,
        typed_rhs,
        typed_select,
        typed_new,
        typed_argument,
        argument_proof,
        formal_type,
        constructor_formal_type,
        result_type,
        method_descriptor: method_descriptor.to_owned(),
        constructor_descriptor: constructor_descriptor.to_owned(),
    })
}

fn factory_class_declarations<'a>(
    store: &'a SemanticStore,
    source_index: &SourceSemanticIndex,
    class: SymbolId,
) -> Result<&'a dotty_core::Scope, CrossClassConstructorCallProofError> {
    if !store.symbols.contains(class) {
        return Err(CrossClassConstructorCallProofError::SourceIdentity(
            "class identity is stale",
        ));
    }
    let symbol = store.symbols.get(class);
    let SymbolInfo::Complete(class_type) = symbol.info else {
        return Err(CrossClassConstructorCallProofError::SemanticIdentity(
            "class info is incomplete",
        ));
    };
    let Some(Type::ClassInfo(info)) = store.types.try_get(class_type) else {
        return Err(CrossClassConstructorCallProofError::SemanticIdentity(
            "class symbol does not carry ClassInfo",
        ));
    };
    if !store.scopes.contains(info.declarations)
        || info.class != class
        || store.scopes.get(info.declarations).owner != Some(class)
        || source_index.scope_of(class) != Some(info.declarations)
    {
        return Err(CrossClassConstructorCallProofError::SourceIdentity(
            "class declaration scope is not associated with its canonical source identity",
        ));
    }
    Ok(store.scopes.get(info.declarations))
}

/// A typed failure while validating the bounded `Reader.read(counter)` proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrossClassMethodRefError {
    /// The untyped source does not have the required two-class shape.
    SourceShape(&'static str),
    /// A source declaration is not bound to the canonical identity in this session.
    SourceIdentity(&'static str),
    /// A class, member, or parameter has an unsupported semantic type or owner.
    SemanticIdentity(&'static str),
    /// A source tree does not map to the exact typed tree being validated.
    TypedBinding(&'static str),
    /// The complete JVM unit does not contain the canonical source plans.
    JvmPlan(&'static str),
}

impl fmt::Display for CrossClassMethodRefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cross-class method reference proof failed: {self:?}")
    }
}

impl std::error::Error for CrossClassMethodRefError {}

/// Borrowed inputs from one frontend/JVM planning session.
///
/// The source root must be the package tree that owns the two planned classes.
/// All IDs are interpreted through these borrowed session stores and indexes.
/// Arena IDs do not authenticate their originating arena, so callers must
/// supply every input from the same frontend session.
pub struct CrossClassMethodRefInput<'session> {
    /// Source identity used by the parser and namer.
    pub source_id: SourceId,
    /// The exact package root from the untyped source arena.
    pub source_root: TreeId<Untyped>,
    /// Canonical symbols and semantic types for this session.
    pub store: &'session SemanticStore,
    /// Canonical primitive definitions for this session.
    pub definitions: &'session Definitions,
    /// Source trees from this session.
    pub source_ast: &'session AstArena<Untyped>,
    /// Typed trees from this session.
    pub typed_ast: &'session TypedAst,
    /// Canonical source declaration and scope mappings from this session.
    pub source_index: &'session SourceSemanticIndex,
    /// Source-to-typed mappings from this session.
    pub source_typed_index: &'session SourceTypedIndex,
    /// JVM plans built from the same semantic store and typed arena.
    pub jvm_unit: &'session JvmCompilationUnit<'session>,
}

/// Validated source/typed identity for `Reader.read(counter: Counter) = counter.get()`.
///
/// This proof is read-only and retains the input borrows so that its IDs and
/// typed roots remain tied to the exact compilation session that established
/// them. It proves only the concrete zero-argument public `Counter.get(): Int`
/// call shape; it does not claim that a TASTy producer can serialize it.
pub struct ValidatedCrossClassMethodRef<'session> {
    _store: &'session SemanticStore,
    _source_ast: &'session AstArena<Untyped>,
    _typed_ast: &'session TypedAst,
    _source_index: &'session SourceSemanticIndex,
    _source_typed_index: &'session SourceTypedIndex,
    _jvm_unit: &'session JvmCompilationUnit<'session>,
    source_id: SourceId,
    counter_class: SymbolId,
    reader_class: SymbolId,
    counter_get: SymbolId,
    reader_read: SymbolId,
    counter_formal: SymbolId,
    source_rhs: TreeId<Untyped>,
    typed_rhs: TreeId<Typed>,
    typed_select: TreeId<Typed>,
    typed_receiver: TreeId<Typed>,
    method_descriptor: &'static str,
}

impl ValidatedCrossClassMethodRef<'_> {
    /// Source file identity shared by both declarations and typed roots.
    pub fn source_id(&self) -> SourceId {
        self.source_id
    }

    /// Canonical planned `Counter` class identity.
    pub fn counter_class(&self) -> SymbolId {
        self.counter_class
    }

    /// Canonical planned `Reader` class identity.
    pub fn reader_class(&self) -> SymbolId {
        self.reader_class
    }

    /// Canonical public `Counter.get(): Int` method identity.
    pub fn counter_get(&self) -> SymbolId {
        self.counter_get
    }

    /// Canonical `Reader.read(counter: Counter): Int` method identity.
    pub fn reader_read(&self) -> SymbolId {
        self.reader_read
    }

    /// Canonical `counter` formal owned by `Reader.read`.
    pub fn counter_formal(&self) -> SymbolId {
        self.counter_formal
    }

    /// Source body root for the call.
    pub fn source_rhs(&self) -> TreeId<Untyped> {
        self.source_rhs
    }

    /// Typed body root for the call.
    pub fn typed_rhs(&self) -> TreeId<Typed> {
        self.typed_rhs
    }

    /// Typed `counter.get` selection node.
    pub fn typed_select(&self) -> TreeId<Typed> {
        self.typed_select
    }

    /// Typed receiver identifier node.
    pub fn typed_receiver(&self) -> TreeId<Typed> {
        self.typed_receiver
    }

    /// JVM method descriptor proven by the supported zero-argument Int result.
    pub fn method_descriptor(&self) -> &'static str {
        self.method_descriptor
    }
}

/// Validate canonical source, typed-tree, declaration-scope, and JVM-plan
/// identity for the bounded `Counter` / `Reader` selected-call shape.
///
/// This operation only inspects borrowed state. It does not mutate the store,
/// allocate symbols, emit classfiles, or construct TASTy bytes.
pub fn validate_cross_class_method_ref(
    input: CrossClassMethodRefInput<'_>,
) -> Result<ValidatedCrossClassMethodRef<'_>, CrossClassMethodRefError> {
    let CrossClassMethodRefInput {
        source_id,
        source_root,
        store,
        definitions,
        source_ast,
        typed_ast,
        source_index,
        source_typed_index,
        jvm_unit,
    } = input;

    if !std::ptr::eq(jvm_unit.store, store) {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "JVM unit uses a different semantic store",
        ));
    }
    if !std::ptr::eq(jvm_unit.typed_ast, typed_ast) {
        return Err(CrossClassMethodRefError::TypedBinding(
            "JVM unit uses a different typed AST",
        ));
    }
    let Some(source_root_tree) = source_ast.try_get(source_root) else {
        return Err(CrossClassMethodRefError::SourceShape(
            "source root is not present in the source AST",
        ));
    };
    let TreeKind::PackageDef(package) = &source_root_tree.kind else {
        return Err(CrossClassMethodRefError::SourceShape(
            "source root must be one package declaration",
        ));
    };
    let [counter_tree, reader_tree] = package.stats.as_slice() else {
        return Err(CrossClassMethodRefError::SourceShape(
            "source package must contain exactly Counter and Reader",
        ));
    };
    let class_name = |tree: TreeId<Untyped>| match source_ast.try_get(tree).map(|tree| &tree.kind) {
        Some(TreeKind::TypeDef(definition)) => {
            store.names.resolve(definition.name.as_name().text())
        }
        _ => "",
    };
    if class_name(*counter_tree) != "Counter" || class_name(*reader_tree) != "Reader" {
        return Err(CrossClassMethodRefError::SourceShape(
            "source declarations must be Counter followed by Reader",
        ));
    }
    validate_plain_class_source(source_ast, *counter_tree)?;
    validate_plain_class_source(source_ast, *reader_tree)?;
    let counter_class = source_index.symbol_at(source_id, *counter_tree).ok_or(
        CrossClassMethodRefError::SourceIdentity("Counter has no canonical source symbol"),
    )?;
    let reader_class = source_index.symbol_at(source_id, *reader_tree).ok_or(
        CrossClassMethodRefError::SourceIdentity("Reader has no canonical source symbol"),
    )?;
    validate_canonical_definition(source_index, source_id, *counter_tree, counter_class)?;
    validate_canonical_definition(source_index, source_id, *reader_tree, reader_class)?;
    if counter_class == reader_class {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "Counter and Reader must be distinct canonical classes",
        ));
    }
    if !store.symbols.contains(counter_class) || !store.symbols.contains(reader_class) {
        return Err(CrossClassMethodRefError::SourceIdentity(
            "class source index contains a stale canonical symbol",
        ));
    }
    if store.symbols.get(counter_class).kind != SymbolKind::Class
        || store.symbols.get(reader_class).kind != SymbolKind::Class
        || store.symbols.get(counter_class).visibility != dotty_core::Visibility::Public
        || store.symbols.get(reader_class).visibility != dotty_core::Visibility::Public
        || has_any_flag(
            store.symbols.get(counter_class).flags,
            SymbolFlags::CASE | SymbolFlags::ENUM | SymbolFlags::ABSTRACT | SymbolFlags::SYNTHETIC,
        )
        || has_any_flag(
            store.symbols.get(reader_class).flags,
            SymbolFlags::CASE | SymbolFlags::ENUM | SymbolFlags::ABSTRACT | SymbolFlags::SYNTHETIC,
        )
    {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "both declarations must be public ordinary classes",
        ));
    }
    let package_owner = store.symbols.get(counter_class).owner.ok_or(
        CrossClassMethodRefError::SemanticIdentity(
            "Counter and Reader must have the exact same package owner",
        ),
    )?;
    if !store.symbols.contains(package_owner) {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "class package owner is stale",
        ));
    }
    if store.symbols.get(reader_class).owner != Some(package_owner)
        || store.symbols.get(package_owner).kind != SymbolKind::Package
    {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "Counter and Reader must have the exact same package owner",
        ));
    }
    let counter_members = class_declarations(store, source_index, counter_class)?;
    let reader_members = class_declarations(store, source_index, reader_class)?;

    let (counter_get_tree, counter_get_definition) = find_method(
        source_ast,
        store,
        *counter_tree,
        "get",
        "Counter must declare exactly one concrete get method",
    )?;
    let (reader_read_tree, reader_read_definition) = find_method(
        source_ast,
        store,
        *reader_tree,
        "read",
        "Reader must declare exactly one concrete read method",
    )?;
    if !counter_get_definition.type_params.is_empty()
        || counter_get_definition.value_param_clauses.len() != 1
        || !counter_get_definition.value_param_clauses[0].is_empty()
        || !reader_read_definition.type_params.is_empty()
        || reader_read_definition.value_param_clauses.len() != 1
        || reader_read_definition.value_param_clauses[0].len() != 1
    {
        return Err(CrossClassMethodRefError::SourceShape(
            "get must be get() and read must have one plain Counter formal",
        ));
    }
    let counter_get = source_index.symbol_at(source_id, counter_get_tree).ok_or(
        CrossClassMethodRefError::SourceIdentity("Counter.get has no canonical source symbol"),
    )?;
    let reader_read = source_index.symbol_at(source_id, reader_read_tree).ok_or(
        CrossClassMethodRefError::SourceIdentity("Reader.read has no canonical source symbol"),
    )?;
    validate_canonical_definition(source_index, source_id, counter_get_tree, counter_get)?;
    validate_canonical_definition(source_index, source_id, reader_read_tree, reader_read)?;
    if !store.symbols.contains(counter_get) || !store.symbols.contains(reader_read) {
        return Err(CrossClassMethodRefError::SourceIdentity(
            "method source index contains a stale canonical symbol",
        ));
    }
    validate_member(store, counter_members, counter_class, counter_get, "get")?;
    validate_member(store, reader_members, reader_class, reader_read, "read")?;
    validate_public_concrete_method(store, counter_get, "Counter.get")?;
    validate_public_concrete_method(store, reader_read, "Reader.read")?;

    let counter_formal_tree = reader_read_definition.value_param_clauses[0][0];
    let counter_formal = source_index
        .symbol_at(source_id, counter_formal_tree)
        .ok_or(CrossClassMethodRefError::SourceIdentity(
            "Reader.read counter formal has no canonical source symbol",
        ))?;
    validate_canonical_definition(source_index, source_id, counter_formal_tree, counter_formal)?;
    if !store.symbols.contains(counter_formal) {
        return Err(CrossClassMethodRefError::SourceIdentity(
            "formal source index contains a stale canonical symbol",
        ));
    }
    if store.symbols.get(counter_formal).kind != SymbolKind::Parameter
        || store.symbols.get(counter_formal).owner != Some(reader_read)
        || store
            .names
            .resolve(store.symbols.get(counter_formal).name.text())
            != "counter"
    {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "counter must be a Reader.read-owned parameter",
        ));
    }
    let SymbolInfo::Complete(formal_type) = store.symbols.get(counter_formal).info else {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "counter formal type is not complete",
        ));
    };
    if !is_canonical_class_type_ref(
        store,
        formal_type,
        counter_class,
        package_owner,
        definitions.no_prefix,
    ) {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "counter formal must refer to the planned Counter class identity",
        ));
    }

    let counter_get_type = complete_method_type(store, counter_get, "Counter.get")?;
    if !counter_get_type.params.is_empty()
        || counter_get_type.result != definitions.int
        || counter_get_type.kind != dotty_core::MethodKind::Plain
    {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "Counter.get must have the exact zero-argument Int signature ()I",
        ));
    }
    let reader_type = complete_method_type(store, reader_read, "Reader.read")?;
    if reader_type.params.len() != 1
        || reader_type.params[0].ty != formal_type
        || reader_type.params[0].erased
        || reader_type.params[0].varargs
        || reader_type.result != definitions.int
        || reader_type.kind != dotty_core::MethodKind::Plain
    {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "Reader.read must have one plain Counter formal and return Int",
        ));
    }

    let source_rhs = reader_read_definition
        .rhs
        .ok_or(CrossClassMethodRefError::SourceShape(
            "Reader.read must have a concrete body",
        ))?;
    let Some(source_rhs_tree) = source_ast.try_get(source_rhs) else {
        return Err(CrossClassMethodRefError::SourceShape(
            "Reader.read body is not present in the source AST",
        ));
    };
    let TreeKind::Apply(source_apply) = &source_rhs_tree.kind else {
        return Err(CrossClassMethodRefError::SourceShape(
            "Reader.read body must be counter.get()",
        ));
    };
    let Some(source_function_tree) = source_ast.try_get(source_apply.function) else {
        return Err(CrossClassMethodRefError::SourceShape(
            "Reader.read selected function is not present in the source AST",
        ));
    };
    let TreeKind::Select(source_select) = &source_function_tree.kind else {
        return Err(CrossClassMethodRefError::SourceShape(
            "Reader.read body must select get on its formal",
        ));
    };
    let Some(source_receiver_tree) = source_ast.try_get(source_select.qualifier) else {
        return Err(CrossClassMethodRefError::SourceShape(
            "Reader.read receiver is not present in the source AST",
        ));
    };
    let TreeKind::Ident(source_receiver) = &source_receiver_tree.kind else {
        return Err(CrossClassMethodRefError::SourceShape(
            "Reader.read receiver must be the counter formal identifier",
        ));
    };
    if store.names.resolve(source_select.name.text()) != "get"
        || store.names.resolve(source_receiver.name.text()) != "counter"
        || !source_apply.args.is_empty()
    {
        return Err(CrossClassMethodRefError::SourceShape(
            "Reader.read body must be the zero-argument counter.get() call",
        ));
    }
    let typed_rhs = source_typed_index.get(source_id, source_rhs).ok_or(
        CrossClassMethodRefError::TypedBinding("Reader.read body has no source-to-typed binding"),
    )?;
    let Some(typed_rhs_tree) = typed_ast.try_get(typed_rhs) else {
        return Err(CrossClassMethodRefError::TypedBinding(
            "Reader.read typed body root is stale",
        ));
    };
    let TreeKind::Apply(typed_apply) = &typed_rhs_tree.kind else {
        return Err(CrossClassMethodRefError::TypedBinding(
            "Reader.read typed body is not an Apply",
        ));
    };
    if typed_apply.kind != ApplyKind::Regular || !typed_apply.args.is_empty() {
        return Err(CrossClassMethodRefError::TypedBinding(
            "typed call must be one plain zero-argument Apply",
        ));
    }
    if source_typed_index.get(source_id, source_apply.function) != Some(typed_apply.function) {
        return Err(CrossClassMethodRefError::TypedBinding(
            "selected method root is not the source-bound typed function",
        ));
    }
    let Some(typed_function_tree) = typed_ast.try_get(typed_apply.function) else {
        return Err(CrossClassMethodRefError::TypedBinding(
            "typed selected function root is stale",
        ));
    };
    let TreeKind::Select(typed_select) = &typed_function_tree.kind else {
        return Err(CrossClassMethodRefError::TypedBinding(
            "typed call function is not a Select",
        ));
    };
    if source_typed_index.get(source_id, source_select.qualifier) != Some(typed_select.qualifier) {
        return Err(CrossClassMethodRefError::TypedBinding(
            "receiver is not the source-bound typed qualifier",
        ));
    }
    let Some(typed_receiver_tree) = typed_ast.try_get(typed_select.qualifier) else {
        return Err(CrossClassMethodRefError::TypedBinding(
            "typed receiver root is stale",
        ));
    };
    let TreeKind::Ident(typed_receiver) = &typed_receiver_tree.kind else {
        return Err(CrossClassMethodRefError::TypedBinding(
            "typed receiver is not an identifier",
        ));
    };
    if store.names.resolve(typed_receiver.name.text()) != "counter"
        || store.names.resolve(typed_select.name.text()) != "get"
        || !matches!(
            store.types.try_get(typed_function_tree.ty),
            Some(Type::TermRef { prefix, target: TermRefTarget::Symbol(target) })
                if *target == counter_get && *prefix == typed_receiver_tree.ty
        )
        || !matches!(
            store.types.try_get(typed_receiver_tree.ty),
            Some(Type::TermRef { prefix, target: TermRefTarget::Symbol(target) })
                if *target == counter_formal && *prefix == definitions.no_prefix
        )
    {
        return Err(CrossClassMethodRefError::TypedBinding(
            "typed receiver and selected method must retain canonical symbol targets",
        ));
    }
    if store.types.try_get(typed_rhs_tree.ty).is_none()
        || store.types.try_get(definitions.int).is_none()
        || typed_rhs_tree.ty != definitions.int
    {
        return Err(CrossClassMethodRefError::TypedBinding(
            "typed call result must be the canonical Int type",
        ));
    }

    let counter_get_rhs =
        counter_get_definition
            .rhs
            .ok_or(CrossClassMethodRefError::SourceShape(
                "Counter.get must have a concrete body",
            ))?;
    if source_ast.try_get(counter_get_rhs).is_none() {
        return Err(CrossClassMethodRefError::SourceShape(
            "Counter.get source body root is stale",
        ));
    }
    let counter_get_body = source_typed_index.get(source_id, counter_get_rhs).ok_or(
        CrossClassMethodRefError::TypedBinding("Counter.get body has no source-to-typed binding"),
    )?;
    if typed_ast.try_get(counter_get_body).is_none() {
        return Err(CrossClassMethodRefError::TypedBinding(
            "Counter.get typed body root is stale",
        ));
    }
    let counter_method_plan = unique_class_plan(jvm_unit, counter_class, "Counter")?;
    let reader_method_plan = unique_class_plan(jvm_unit, reader_class, "Reader")?;
    let counter_get_plan = validate_planned_method(
        jvm_unit,
        counter_method_plan,
        counter_get,
        &[],
        Some(counter_get_body),
        "Counter.get",
    )?;
    validate_planned_method(
        jvm_unit,
        reader_method_plan,
        reader_read,
        &[counter_formal],
        Some(typed_rhs),
        "Reader.read",
    )?;
    let descriptor = counter_get_plan
        .descriptor(store, counter_class, definitions)
        .map_err(|_| {
            CrossClassMethodRefError::JvmPlan(
                "Counter.get JVM descriptor cannot be derived from its canonical plan",
            )
        })?;
    if descriptor.as_str() != "()I" {
        return Err(CrossClassMethodRefError::JvmPlan(
            "Counter.get JVM plan does not have the exact ()I descriptor",
        ));
    }

    Ok(ValidatedCrossClassMethodRef {
        _store: store,
        _source_ast: source_ast,
        _typed_ast: typed_ast,
        _source_index: source_index,
        _source_typed_index: source_typed_index,
        _jvm_unit: jvm_unit,
        source_id,
        counter_class,
        reader_class,
        counter_get,
        reader_read,
        counter_formal,
        source_rhs,
        typed_rhs,
        typed_select: typed_apply.function,
        typed_receiver: typed_select.qualifier,
        method_descriptor: "()I",
    })
}

fn validate_canonical_definition(
    source_index: &SourceSemanticIndex,
    source: SourceId,
    tree: TreeId<Untyped>,
    symbol: SymbolId,
) -> Result<(), CrossClassMethodRefError> {
    if source_index.symbol_at(source, tree) != Some(symbol)
        || source_index.definition_of(symbol) != Some(SourceDefinition::Canonical { source, tree })
    {
        return Err(CrossClassMethodRefError::SourceIdentity(
            "symbol is not the canonical identity for this exact source tree",
        ));
    }
    Ok(())
}

fn validate_plain_class_source(
    source_ast: &AstArena<Untyped>,
    class_tree: TreeId<Untyped>,
) -> Result<(), CrossClassMethodRefError> {
    let Some(class_node) = source_ast.try_get(class_tree) else {
        return Err(CrossClassMethodRefError::SourceShape(
            "class declaration is not present in the source AST",
        ));
    };
    let TreeKind::TypeDef(definition) = &class_node.kind else {
        return Err(CrossClassMethodRefError::SourceShape(
            "top-level declaration must be an ordinary class",
        ));
    };
    let Some(template_node) = source_ast.try_get(definition.rhs) else {
        return Err(CrossClassMethodRefError::SourceShape(
            "class template is not present in the source AST",
        ));
    };
    let TreeKind::Template(template) = &template_node.kind else {
        return Err(CrossClassMethodRefError::SourceShape(
            "class declaration must have an ordinary template",
        ));
    };
    if !template.parents.is_empty() || template.self_val.is_some() {
        return Err(CrossClassMethodRefError::SourceShape(
            "cross-class reference proof excludes explicit parents and self types",
        ));
    }
    let Some(constructor_node) = source_ast.try_get(template.constructor) else {
        return Err(CrossClassMethodRefError::SourceShape(
            "class constructor is not present in the source AST",
        ));
    };
    let TreeKind::DefDef(constructor) = &constructor_node.kind else {
        return Err(CrossClassMethodRefError::SourceShape(
            "class primary constructor must be a source method",
        ));
    };
    if !constructor.type_params.is_empty() {
        return Err(CrossClassMethodRefError::SourceShape(
            "generic classes are outside the cross-class reference proof",
        ));
    }
    Ok(())
}

fn class_declarations<'a>(
    store: &'a SemanticStore,
    source_index: &SourceSemanticIndex,
    class: SymbolId,
) -> Result<&'a dotty_core::Scope, CrossClassMethodRefError> {
    if !store.symbols.contains(class) {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "class identity is stale",
        ));
    }
    let SymbolInfo::Complete(class_type) = store.symbols.get(class).info else {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "class info is not complete",
        ));
    };
    let Some(Type::ClassInfo(info)) = store.types.try_get(class_type) else {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "class symbol does not carry ClassInfo",
        ));
    };
    if !store.scopes.contains(info.declarations)
        || info.class != class
        || store.scopes.get(info.declarations).owner != Some(class)
        || source_index.scope_of(class) != Some(info.declarations)
    {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "class declaration scope is not the canonical source scope",
        ));
    }
    let declarations = store.scopes.get(info.declarations);
    if declarations.entered_symbols().any(|member| {
        if !store.symbols.contains(member) {
            return true;
        }
        let symbol = store.symbols.get(member);
        symbol.kind == SymbolKind::TypeParameter && symbol.owner == Some(class)
    }) {
        return Err(CrossClassMethodRefError::SourceShape(
            "generic classes are outside the cross-class reference proof",
        ));
    }
    Ok(declarations)
}

fn find_method<'a>(
    source_ast: &'a AstArena<Untyped>,
    store: &SemanticStore,
    class_tree: TreeId<Untyped>,
    name: &str,
    reason: &'static str,
) -> Result<(TreeId<Untyped>, &'a dotty_core::ast::DefDef<Untyped>), CrossClassMethodRefError> {
    let Some(class_node) = source_ast.try_get(class_tree) else {
        return Err(CrossClassMethodRefError::SourceShape(
            "class declaration is not present in the source AST",
        ));
    };
    let TreeKind::TypeDef(class_definition) = &class_node.kind else {
        return Err(CrossClassMethodRefError::SourceShape(
            "top-level declaration must be a class",
        ));
    };
    let Some(template_node) = source_ast.try_get(class_definition.rhs) else {
        return Err(CrossClassMethodRefError::SourceShape(
            "class template is not present in the source AST",
        ));
    };
    let TreeKind::Template(template) = &template_node.kind else {
        return Err(CrossClassMethodRefError::SourceShape(
            "class declaration must have a template",
        ));
    };
    let mut matches = Vec::new();
    for tree in template.body.iter().copied() {
        let Some(node) = source_ast.try_get(tree) else {
            return Err(CrossClassMethodRefError::SourceShape(
                "class member is not present in the source AST",
            ));
        };
        if let TreeKind::DefDef(definition) = &node.kind
            && store.names.resolve(definition.name.as_name().text()) == name
        {
            matches.push((tree, definition));
        }
    }
    match matches.as_slice() {
        [(tree, definition)] => Ok((*tree, *definition)),
        _ => Err(CrossClassMethodRefError::SourceShape(reason)),
    }
}

fn validate_member(
    store: &SemanticStore,
    declarations: &dotty_core::Scope,
    owner: SymbolId,
    member: SymbolId,
    name: &str,
) -> Result<(), CrossClassMethodRefError> {
    if !store.symbols.contains(member) {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "method identity is stale",
        ));
    }
    let symbol = store.symbols.get(member);
    if symbol.owner != Some(owner)
        || store.names.resolve(symbol.name.text()) != name
        || declarations.lookup_all(&symbol.name) != [member]
    {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "method owner or declaration-scope membership is not canonical",
        ));
    }
    Ok(())
}

fn validate_public_concrete_method(
    store: &SemanticStore,
    method: SymbolId,
    label: &'static str,
) -> Result<(), CrossClassMethodRefError> {
    if !store.symbols.contains(method) {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "method identity is stale",
        ));
    }
    let symbol = store.symbols.get(method);
    if symbol.kind != SymbolKind::Method
        || symbol.visibility != dotty_core::Visibility::Public
        || has_any_flag(
            symbol.flags,
            SymbolFlags::ABSTRACT
                | SymbolFlags::SYNTHETIC
                | SymbolFlags::ERASED
                | SymbolFlags::STATIC
                | SymbolFlags::EXTENSION
                | SymbolFlags::GIVEN
                | SymbolFlags::IMPLICIT,
        )
    {
        return Err(CrossClassMethodRefError::SemanticIdentity(match label {
            "Counter.get" => "Counter.get must be public, concrete, and source-defined",
            _ => "Reader.read must be public, concrete, and source-defined",
        }));
    }
    Ok(())
}

fn has_any_flag(flags: SymbolFlags, excluded: SymbolFlags) -> bool {
    !flags.intersection(excluded).is_empty()
}

fn is_canonical_class_type_ref(
    store: &SemanticStore,
    ty: dotty_core::TypeId,
    class: SymbolId,
    package: SymbolId,
    no_prefix: dotty_core::TypeId,
) -> bool {
    let Some(Type::TypeRef {
        prefix,
        target: TypeRefTarget::Symbol(target),
    }) = store.types.try_get(ty)
    else {
        return false;
    };
    *target == class
        && matches!(
            store.types.try_get(*prefix),
            Some(Type::TypeRef {
                prefix: package_prefix,
                target: TypeRefTarget::Symbol(package_target),
            }) if *package_prefix == no_prefix && *package_target == package
        )
}

fn canonical_class_type_symbol(
    store: &SemanticStore,
    ty: TypeId,
    package: SymbolId,
    no_prefix: dotty_core::TypeId,
) -> Option<SymbolId> {
    let Some(Type::TypeRef {
        prefix,
        target: TypeRefTarget::Symbol(class),
    }) = store.types.try_get(ty)
    else {
        return None;
    };
    matches!(
        store.types.try_get(*prefix),
        Some(Type::TypeRef {
            prefix: package_prefix,
            target: TypeRefTarget::Symbol(package_target),
        }) if *package_prefix == no_prefix && *package_target == package
    )
    .then_some(*class)
}

fn require_canonical_caller_formal_seed(
    typed_name_matches_formal: bool,
    typed_target: Option<SymbolId>,
    method_formal: SymbolId,
) -> Result<(), CrossClassConstructorCallProofError> {
    if typed_name_matches_formal && typed_target == Some(method_formal) {
        Ok(())
    } else {
        Err(CrossClassConstructorCallProofError::TypedBinding(
            "typed constructor argument does not reference the canonical caller formal",
        ))
    }
}

fn complete_method_type<'a>(
    store: &'a SemanticStore,
    method: SymbolId,
    _label: &'static str,
) -> Result<&'a dotty_core::MethodType, CrossClassMethodRefError> {
    if !store.symbols.contains(method) {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "method identity is stale",
        ));
    }
    let SymbolInfo::Complete(method_type) = store.symbols.get(method).info else {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "method signature is not complete",
        ));
    };
    match store.types.try_get(method_type) {
        Some(Type::Method(method_type)) => Ok(method_type),
        _ => Err(CrossClassMethodRefError::SemanticIdentity(
            "method must have one non-generic plain method type",
        )),
    }
}

fn unique_class_plan<'a>(
    jvm_unit: &'a JvmCompilationUnit<'_>,
    class: SymbolId,
    label: &'static str,
) -> Result<&'a JvmClassPlan, CrossClassMethodRefError> {
    let mut matching = jvm_unit.classes.iter().filter(|plan| plan.class == class);
    let Some(plan) = matching.next() else {
        return Err(CrossClassMethodRefError::JvmPlan(match label {
            "Counter" => "canonical Counter is absent from the complete JVM unit",
            _ => "canonical Reader is absent from the complete JVM unit",
        }));
    };
    if matching.next().is_some() {
        return Err(CrossClassMethodRefError::JvmPlan(
            "a canonical class occurs more than once in the JVM unit",
        ));
    }
    Ok(plan)
}

fn validate_planned_method<'a>(
    jvm_unit: &JvmCompilationUnit<'_>,
    class_plan: &'a JvmClassPlan,
    method: SymbolId,
    parameters: &[SymbolId],
    body: Option<TreeId<Typed>>,
    label: &'static str,
) -> Result<&'a dotty_jvm::JvmMethodPlan, CrossClassMethodRefError> {
    let total_occurrences = jvm_unit
        .classes
        .iter()
        .flat_map(|plan| plan.methods.iter())
        .filter(|plan| plan.method == method)
        .count();
    if total_occurrences == 0 {
        return Err(CrossClassMethodRefError::JvmPlan(match label {
            "Counter.get" => "canonical Counter.get is absent from Counter's JVM plan",
            _ => "canonical Reader.read is absent from Reader's JVM plan",
        }));
    }
    if total_occurrences != 1 {
        return Err(CrossClassMethodRefError::JvmPlan(
            "canonical method must occur exactly once in the complete JVM unit",
        ));
    }
    let mut matching = class_plan
        .methods
        .iter()
        .filter(|plan| plan.method == method);
    let Some(plan) = matching.next() else {
        return Err(CrossClassMethodRefError::JvmPlan(match label {
            "Counter.get" => "canonical Counter.get is absent from Counter's JVM plan",
            _ => "canonical Reader.read is absent from Reader's JVM plan",
        }));
    };
    if matching.next().is_some() || plan.parameters != parameters || plan.body != body {
        return Err(CrossClassMethodRefError::JvmPlan(
            "planned method identity, formal order, or typed body does not match source",
        ));
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::{
        CrossClassConstructorCallProofError, has_any_flag, is_canonical_class_type_ref,
        require_canonical_caller_formal_seed,
    };
    use dotty_core::{Definitions, SemanticStore, SymbolFlags, Type, TypeRefTarget};

    #[test]
    fn wrong_symbol_initializer_seed_is_rejected_before_local_val_receipt_construction() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);

        assert_eq!(
            require_canonical_caller_formal_seed(
                true,
                Some(definitions.any_class),
                definitions.object_class,
            ),
            Err(CrossClassConstructorCallProofError::TypedBinding(
                "typed constructor argument does not reference the canonical caller formal",
            )),
        );
    }

    #[test]
    fn excluded_class_and_method_flags_are_checked_individually() {
        let class_exclusions = [
            SymbolFlags::CASE,
            SymbolFlags::ENUM,
            SymbolFlags::ABSTRACT,
            SymbolFlags::SYNTHETIC,
        ];
        let method_exclusions = [
            SymbolFlags::ABSTRACT,
            SymbolFlags::SYNTHETIC,
            SymbolFlags::ERASED,
            SymbolFlags::STATIC,
            SymbolFlags::EXTENSION,
            SymbolFlags::GIVEN,
            SymbolFlags::IMPLICIT,
        ];

        let class_mask = class_exclusions
            .into_iter()
            .fold(SymbolFlags::EMPTY, SymbolFlags::union);
        let method_mask = method_exclusions
            .into_iter()
            .fold(SymbolFlags::EMPTY, SymbolFlags::union);
        for flag in class_exclusions {
            assert!(has_any_flag(flag, class_mask), "class flag {flag:?}");
        }
        for flag in method_exclusions {
            assert!(has_any_flag(flag, method_mask), "method flag {flag:?}");
        }
        assert!(!has_any_flag(SymbolFlags::EMPTY, class_mask));
        assert!(!has_any_flag(SymbolFlags::EMPTY, method_mask));
    }

    #[test]
    fn canonical_class_type_rejects_an_unrelated_prefix() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let class = definitions.object_class;
        let package = definitions.any_class;
        let package_prefix = store.types.alloc(Type::TypeRef {
            prefix: definitions.no_prefix,
            target: TypeRefTarget::Symbol(package),
        });
        let correct = store.types.alloc(Type::TypeRef {
            prefix: package_prefix,
            target: TypeRefTarget::Symbol(class),
        });
        let wrong_prefix = store.types.alloc(Type::TypeRef {
            prefix: definitions.int,
            target: TypeRefTarget::Symbol(class),
        });

        assert!(is_canonical_class_type_ref(
            &store,
            correct,
            class,
            package,
            definitions.no_prefix
        ));
        assert!(!is_canonical_class_type_ref(
            &store,
            wrong_prefix,
            class,
            package,
            definitions.no_prefix
        ));
    }
}
