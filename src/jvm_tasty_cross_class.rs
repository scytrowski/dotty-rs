//! Read-only source/typed proofs for bounded same-batch cross-class calls used
//! by the JVM/TASTy producer sequence.
//!
//! This validates source and typed provenance only. It does not lower or
//! serialize a cross-class TASTy signature or method body.

use std::fmt;

use dotty_core::ast::{ApplyKind, AstArena, TreeKind, Typed, TypedAst, TypedTreeId, Untyped};
use dotty_core::{
    Definitions, SemanticStore, SourceDefinition, SourceId, SourceSemanticIndex, SourceTypedIndex,
    SymbolFlags, SymbolId, SymbolInfo, SymbolKind, SymbolOrigin, TermRefTarget, TreeId, Type,
    TypeId, TypeRefTarget,
};
use dotty_jvm::{JvmClassPlan, JvmCompilationUnit};
use dotty_tasty_pickler::LocalReferenceAliasBinding;
use dotty_typer::{SourceTyper, TypedLocalBinding};

mod conditional_local_reference;
mod local_receiver_call;
pub use conditional_local_reference::{
    ImmutableLocalConditionalReferenceSelectionProofInput,
    ValidatedImmutableLocalConditionalReferenceSelectionProof,
    validate_immutable_local_conditional_reference_selection_proof,
};
pub use local_receiver_call::{
    ValidatedImmutableLocalCounterSetCallProof,
    ValidatedImmutableLocalCounterSetCallsAfterAliasProof,
    ValidatedImmutableLocalCounterSetCallsProof, validate_immutable_local_counter_set_call_proof,
    validate_immutable_local_counter_set_calls_after_alias_proof,
    validate_immutable_local_counter_set_calls_proof,
};

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
    local_receiver_unit_call: Option<LocalReceiverUnitCallReceipt>,
    local_receiver_unit_calls: Option<LocalReceiverUnitCallsReceipt>,
    local_reference_alias: Option<LocalReferenceAliasBinding>,
    formal_type: TypeId,
    constructor_formal_type: TypeId,
    result_type: TypeId,
    method_descriptor: String,
    constructor_descriptor: String,
}

/// Canonical source/typed roots for the one admitted local-receiver Unit call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalReceiverUnitCallReceipt {
    pub source_apply: TreeId<Untyped>,
    pub typed_apply: TreeId<Typed>,
    pub source_select: TreeId<Untyped>,
    pub typed_select: TreeId<Typed>,
    pub source_receiver: TreeId<Untyped>,
    pub typed_receiver: TreeId<Typed>,
    pub source_argument: TreeId<Untyped>,
    pub typed_argument: TreeId<Typed>,
    pub receiver: SymbolId,
    pub method: SymbolId,
    pub argument_formal: SymbolId,
    pub receiver_scope: dotty_core::ScopeId,
    pub argument_type: TypeId,
    pub result_type: TypeId,
}

/// Exact source-order pair of authenticated local Counter.set calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalReceiverUnitCallsReceipt {
    pub calls: [LocalReceiverUnitCallReceipt; 2],
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
    /// The outer constructor receives one original method formal and one nested call.
    FormalNestedPair,
    /// The outer constructor receives one original formal and one initialized local.
    FormalLocalPair,
    /// The outer constructor receives a same-instance planned method result.
    PlannedCall,
    /// The outer constructor receives one initialized immutable local val.
    LocalVal,
    /// The outer constructor receives two initialized immutable local vals.
    LocalValPair,
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
// These proof variants are intentionally Copy: validation returns immutable
// identity receipts to read-only consumers without allocating or transferring
// the borrowed session stores.
#[allow(clippy::large_enum_variant)]
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
    local_initializers: Option<[ImmutableLocalInitializerReceipt; 2]>,
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
    LocalVal(ImmutableLocalArgumentProof),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ImmutableLocalArgumentProof {
    source_block: TreeId<Untyped>,
    source_declaration: TreeId<Untyped>,
    source_initializer: TreeId<Untyped>,
    source_initializer_select: TreeId<Untyped>,
    source_initializer_new: TreeId<Untyped>,
    source_seed_argument: TreeId<Untyped>,
    source_use: TreeId<Untyped>,
    typed_block: TreeId<Typed>,
    typed_declaration: TreeId<Typed>,
    typed_initializer: TreeId<Typed>,
    typed_initializer_select: TreeId<Typed>,
    typed_initializer_new: TreeId<Typed>,
    typed_seed_argument: TreeId<Typed>,
    typed_use: TreeId<Typed>,
    symbol: SymbolId,
    scope: dotty_core::ScopeId,
    constructor: SymbolId,
    local_type: TypeId,
    result_type: TypeId,
    seed_formal: SymbolId,
    seed_formal_type: TypeId,
    source_type_tree: TreeId<Untyped>,
    typed_type_tree: TreeId<Typed>,
    initializer_result_type: TypeId,
    constructor_formal: SymbolId,
    constructor_formal_type: TypeId,
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

/// Borrowed inputs for the proof-only direct-New immutable-local audit.
/// This remains separate from constructor argument planning: the receipt proves
/// the original local initializer and final local reference without changing
/// producer acceptance or output.
pub struct ImmutableLocalInitializationProofInput<'session> {
    pub source_id: SourceId,
    pub method_tree: TreeId<Untyped>,
    pub store: &'session SemanticStore,
    pub definitions: &'session Definitions,
    pub source_ast: &'session AstArena<Untyped>,
    pub typed_ast: &'session TypedAst,
    pub source_index: &'session SourceSemanticIndex,
    pub source_typed_index: &'session SourceTypedIndex,
    /// Result of `typed_local_binding_at` from the original source Typer.
    pub local_binding: Option<TypedLocalBinding>,
    pub jvm_unit: &'session JvmCompilationUnit<'session>,
}

/// Borrowed inputs for the proof-only aggregate of two immutable local
/// initializers in one original straight-line method Block.
#[derive(Clone, Copy)]
pub struct ImmutableLocalInitializerPairProofInput<'session> {
    pub source_id: SourceId,
    pub method_tree: TreeId<Untyped>,
    pub store: &'session SemanticStore,
    pub definitions: &'session Definitions,
    pub source_ast: &'session AstArena<Untyped>,
    pub typed_ast: &'session TypedAst,
    pub source_index: &'session SourceSemanticIndex,
    pub source_typed_index: &'session SourceTypedIndex,
    /// Original Typer bindings for the first and second source ValDefs.
    pub local_bindings: [Option<TypedLocalBinding>; 2],
    pub jvm_unit: &'session JvmCompilationUnit<'session>,
}

/// Borrowed input for authenticating one immutable local reference alias after
/// the original two initialized Counter locals. The alias binding must come
/// from the same source Typer session as the pair bindings.
#[derive(Clone, Copy)]
pub struct ImmutableLocalReferenceAliasProofInput<'session> {
    pub initializers: ImmutableLocalInitializerPairProofInput<'session>,
    pub alias_binding: Option<TypedLocalBinding>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ImmutableLocalInitializerReceipt {
    declaration: ImmutableLocalDeclarationCore,
    seed_formal: SymbolId,
    seed_formal_type: TypeId,
    initializer_result_type: TypeId,
    constructor_formal: SymbolId,
    constructor_formal_type: TypeId,
}

fn immutable_local_argument_from_initializer(
    source_block: TreeId<Untyped>,
    typed_block: TreeId<Typed>,
    receipt: ImmutableLocalInitializerReceipt,
    source_use: TreeId<Untyped>,
    typed_use: TreeId<Typed>,
    result_type: TypeId,
) -> ImmutableLocalArgumentProof {
    let declaration = receipt.declaration;
    ImmutableLocalArgumentProof {
        source_block,
        source_declaration: declaration.declaration,
        source_initializer: declaration.initializer,
        source_initializer_select: declaration.source_select,
        source_initializer_new: declaration.source_new,
        source_seed_argument: declaration.source_argument,
        source_use,
        typed_block,
        typed_declaration: declaration.typed_declaration,
        typed_initializer: declaration.typed_initializer,
        typed_initializer_select: declaration.typed_select,
        typed_initializer_new: declaration.typed_new,
        typed_seed_argument: declaration.typed_argument,
        typed_use,
        symbol: declaration.symbol,
        scope: declaration.scope,
        constructor: declaration.constructor,
        local_type: declaration.local_type,
        result_type,
        seed_formal: receipt.seed_formal,
        seed_formal_type: receipt.seed_formal_type,
        source_type_tree: declaration.source_type_tree,
        typed_type_tree: declaration.typed_type_tree,
        initializer_result_type: receipt.initializer_result_type,
        constructor_formal: receipt.constructor_formal,
        constructor_formal_type: receipt.constructor_formal_type,
    }
}

/// Opaque all-or-error proof of exactly two original immutable Counter
/// initializers in a single Block. This is a borrowed identity receipt only;
/// it does not validate Pair argument origins or authorize emission.
#[derive(Clone, Copy)]
pub struct ValidatedImmutableLocalInitializerPairProof<'session> {
    _store: &'session SemanticStore,
    _definitions: &'session Definitions,
    _source_ast: &'session AstArena<Untyped>,
    _typed_ast: &'session TypedAst,
    _source_index: &'session SourceSemanticIndex,
    _source_typed_index: &'session SourceTypedIndex,
    _jvm_unit: &'session JvmCompilationUnit<'session>,
    source_id: SourceId,
    method: SymbolId,
    counter: SymbolId,
    source_block: TreeId<Untyped>,
    typed_block: TreeId<Typed>,
    source_final_expression: TreeId<Untyped>,
    typed_final_expression: TreeId<Typed>,
    initializers: [ImmutableLocalInitializerReceipt; 2],
}

/// Read-only proof that a distinct immutable local directly aliases the first
/// original Counter local in the same method Block.
#[derive(Clone, Copy)]
pub struct ValidatedImmutableLocalReferenceAliasProof<'session> {
    initializers: ValidatedImmutableLocalInitializerPairProof<'session>,
    source_declaration: TreeId<Untyped>,
    typed_declaration: TreeId<Typed>,
    symbol: SymbolId,
    scope: dotty_core::ScopeId,
    source_initializer: TreeId<Untyped>,
    typed_initializer: TreeId<Typed>,
    source_type_tree: TreeId<Untyped>,
    typed_type_tree: TreeId<Typed>,
    source_target: TreeId<Untyped>,
    typed_target: TreeId<Typed>,
    target: SymbolId,
    local_type: TypeId,
}

impl<'session> ValidatedImmutableLocalReferenceAliasProof<'session> {
    pub const fn initializers(&self) -> &ValidatedImmutableLocalInitializerPairProof<'session> {
        &self.initializers
    }
    pub const fn source_declaration(&self) -> TreeId<Untyped> {
        self.source_declaration
    }
    pub const fn typed_declaration(&self) -> TreeId<Typed> {
        self.typed_declaration
    }
    pub const fn symbol(&self) -> SymbolId {
        self.symbol
    }
    pub const fn scope(&self) -> dotty_core::ScopeId {
        self.scope
    }
    pub const fn source_initializer(&self) -> TreeId<Untyped> {
        self.source_initializer
    }
    pub const fn typed_initializer(&self) -> TreeId<Typed> {
        self.typed_initializer
    }
    pub const fn source_type_tree(&self) -> TreeId<Untyped> {
        self.source_type_tree
    }
    pub const fn typed_type_tree(&self) -> TreeId<Typed> {
        self.typed_type_tree
    }
    pub const fn source_target(&self) -> TreeId<Untyped> {
        self.source_target
    }
    pub const fn typed_target(&self) -> TreeId<Typed> {
        self.typed_target
    }
    pub const fn target(&self) -> SymbolId {
        self.target
    }
    pub const fn local_type(&self) -> TypeId {
        self.local_type
    }
}

#[derive(Clone, Copy)]
pub struct ImmutableLocalInitializerReceiptView {
    receipt: ImmutableLocalInitializerReceipt,
}

impl ImmutableLocalInitializerReceiptView {
    pub const fn source_declaration(self) -> TreeId<Untyped> {
        self.receipt.declaration.declaration
    }
    pub const fn typed_declaration(self) -> TreeId<Typed> {
        self.receipt.declaration.typed_declaration
    }
    pub const fn symbol(self) -> SymbolId {
        self.receipt.declaration.symbol
    }
    pub const fn scope(self) -> dotty_core::ScopeId {
        self.receipt.declaration.scope
    }
    pub const fn local_type(self) -> TypeId {
        self.receipt.declaration.local_type
    }
    pub const fn source_type_tree(self) -> TreeId<Untyped> {
        self.receipt.declaration.source_type_tree
    }
    pub const fn typed_type_tree(self) -> TreeId<Typed> {
        self.receipt.declaration.typed_type_tree
    }
    pub const fn source_initializer(self) -> TreeId<Untyped> {
        self.receipt.declaration.initializer
    }
    pub const fn typed_initializer(self) -> TreeId<Typed> {
        self.receipt.declaration.typed_initializer
    }
    pub const fn source_select(self) -> TreeId<Untyped> {
        self.receipt.declaration.source_select
    }
    pub const fn typed_select(self) -> TreeId<Typed> {
        self.receipt.declaration.typed_select
    }
    pub const fn source_new(self) -> TreeId<Untyped> {
        self.receipt.declaration.source_new
    }
    pub const fn typed_new(self) -> TreeId<Typed> {
        self.receipt.declaration.typed_new
    }

    pub const fn source_new_type_tree(self) -> TreeId<Untyped> {
        self.receipt.declaration.source_new_type_tree
    }

    pub const fn typed_new_type_tree(self) -> TreeId<Typed> {
        self.receipt.declaration.typed_new_type_tree
    }
    pub const fn source_argument(self) -> TreeId<Untyped> {
        self.receipt.declaration.source_argument
    }
    pub const fn typed_argument(self) -> TreeId<Typed> {
        self.receipt.declaration.typed_argument
    }
    pub const fn constructor(self) -> SymbolId {
        self.receipt.declaration.constructor
    }
    pub const fn seed_formal(self) -> SymbolId {
        self.receipt.seed_formal
    }
    pub const fn seed_formal_type(self) -> TypeId {
        self.receipt.seed_formal_type
    }
    pub const fn initializer_result_type(self) -> TypeId {
        self.receipt.initializer_result_type
    }
    pub const fn constructor_formal(self) -> SymbolId {
        self.receipt.constructor_formal
    }
    pub const fn constructor_formal_type(self) -> TypeId {
        self.receipt.constructor_formal_type
    }
}

impl<'session> ValidatedImmutableLocalInitializerPairProof<'session> {
    pub const fn source_id(&self) -> SourceId {
        self.source_id
    }
    pub const fn method(&self) -> SymbolId {
        self.method
    }
    pub const fn counter(&self) -> SymbolId {
        self.counter
    }
    pub const fn source_block(&self) -> TreeId<Untyped> {
        self.source_block
    }
    pub const fn typed_block(&self) -> TreeId<Typed> {
        self.typed_block
    }
    pub const fn source_final_expression(&self) -> TreeId<Untyped> {
        self.source_final_expression
    }
    pub const fn typed_final_expression(&self) -> TreeId<Typed> {
        self.typed_final_expression
    }
    pub const fn initializers(&self) -> [ImmutableLocalInitializerReceiptView; 2] {
        [
            ImmutableLocalInitializerReceiptView {
                receipt: self.initializers[0],
            },
            ImmutableLocalInitializerReceiptView {
                receipt: self.initializers[1],
            },
        ]
    }
}

/// Opaque proof for one exact immutable method local initialized by its
/// original direct `new Class(formal)` tree before the method's final use.
/// The proof borrows the original one-session stores and exposes source/typed
/// identities only; it does not authorize JVM or TASTy production.
pub struct ValidatedImmutableLocalInitializationProof<'session> {
    _store: &'session SemanticStore,
    _source_ast: &'session AstArena<Untyped>,
    _typed_ast: &'session TypedAst,
    _source_index: &'session SourceSemanticIndex,
    _source_typed_index: &'session SourceTypedIndex,
    _jvm_unit: &'session JvmCompilationUnit<'session>,
    source_id: SourceId,
    method: SymbolId,
    local: SymbolId,
    scope: dotty_core::ScopeId,
    constructor: SymbolId,
    source_block: TreeId<Untyped>,
    source_declaration: TreeId<Untyped>,
    source_type_tree: TreeId<Untyped>,
    source_initializer: TreeId<Untyped>,
    source_initializer_select: TreeId<Untyped>,
    source_initializer_new: TreeId<Untyped>,
    source_seed_argument: TreeId<Untyped>,
    source_final_expression: TreeId<Untyped>,
    source_use: TreeId<Untyped>,
    typed_block: TreeId<Typed>,
    typed_declaration: TreeId<Typed>,
    typed_type_tree: TreeId<Typed>,
    typed_initializer: TreeId<Typed>,
    typed_initializer_select: TreeId<Typed>,
    typed_initializer_new: TreeId<Typed>,
    typed_seed_argument: TreeId<Typed>,
    typed_final_expression: TreeId<Typed>,
    typed_use: TreeId<Typed>,
    local_type: TypeId,
    initializer_result_type: TypeId,
    seed_formal: SymbolId,
    seed_formal_type: TypeId,
    constructor_formal: SymbolId,
    constructor_formal_type: TypeId,
}

/// Shared original-declaration invariants used by proof-only immutable local
/// initializer receipts. This intentionally records identities borrowed from
/// the one canonical frontend session; it does not authorize a constructor
/// argument origin or producer plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ImmutableLocalDeclarationCore {
    declaration: TreeId<Untyped>,
    typed_declaration: TreeId<Typed>,
    symbol: SymbolId,
    scope: dotty_core::ScopeId,
    local_type: TypeId,
    source_type_tree: TreeId<Untyped>,
    typed_type_tree: TreeId<Typed>,
    initializer: TreeId<Untyped>,
    typed_initializer: TreeId<Typed>,
    source_select: TreeId<Untyped>,
    typed_select: TreeId<Typed>,
    source_new: TreeId<Untyped>,
    typed_new: TreeId<Typed>,
    source_new_type_tree: TreeId<Untyped>,
    typed_new_type_tree: TreeId<Typed>,
    source_argument: TreeId<Untyped>,
    typed_argument: TreeId<Typed>,
    constructor: SymbolId,
}

#[allow(clippy::too_many_arguments)]
fn validate_immutable_local_declaration_core(
    source_id: SourceId,
    method: SymbolId,
    declaration: TreeId<Untyped>,
    store: &SemanticStore,
    definitions: &Definitions,
    source_ast: &AstArena<Untyped>,
    typed_ast: &TypedAst,
    source_typed_index: &SourceTypedIndex,
    local_binding: Option<TypedLocalBinding>,
    counter: SymbolId,
    package: SymbolId,
    seed_formal: SymbolId,
) -> Result<ImmutableLocalDeclarationCore, CrossClassConstructorCallProofError> {
    let source_error = |s| CrossClassConstructorCallProofError::SourceShape(s);
    let identity_error = |s| CrossClassConstructorCallProofError::SourceIdentity(s);
    let semantic_error = |s| CrossClassConstructorCallProofError::SemanticIdentity(s);
    let typed_error = |s| CrossClassConstructorCallProofError::TypedBinding(s);
    let TreeKind::ValDef(local_def) = &source_ast
        .try_get(declaration)
        .ok_or_else(|| source_error("local declaration tree is stale"))?
        .kind
    else {
        return Err(source_error(
            "factory Block statistic must be the original ValDef",
        ));
    };
    let initializer = local_def
        .rhs
        .ok_or_else(|| source_error("local must have one initializer"))?;
    if matches!(
        source_ast.try_get(local_def.tpt).map(|n| &n.kind),
        Some(TreeKind::TypeTree(_))
    ) {
        return Err(source_error("local must have an explicit source type"));
    }
    let binding = local_binding
        .ok_or_else(|| identity_error("local declaration has no original Typer local binding"))?;
    let local = binding.symbol();
    let typed_declaration = source_typed_index.get(source_id, declaration).ok_or(
        CrossClassConstructorCallProofError::TypedBinding(
            "original ValDef has no source-to-typed binding",
        ),
    )?;
    if binding.declaration() != typed_declaration {
        return Err(identity_error(
            "Typer local binding must name this exact original ValDef",
        ));
    }
    if !store.symbols.contains(local)
        || !has_source_origin(store, local, source_id)
        || store.symbols.get(local).kind != SymbolKind::Local
        || store.symbols.get(local).owner != Some(method)
        || store
            .symbols
            .get(local)
            .flags
            .contains(SymbolFlags::MUTABLE)
    {
        return Err(identity_error(
            "local must be the original immutable method-owned local",
        ));
    }
    let scope_id = binding.scope();
    let local_name = store.symbols.get(local).name;
    if !store.scopes.contains(scope_id) {
        return Err(identity_error("local block scope is stale"));
    }
    let scope = store.scopes.get(scope_id);
    if scope.owner != Some(method) || scope.lookup(&local_name) != Some(local) {
        return Err(identity_error(
            "local must occupy its exact method-owned Block scope",
        ));
    }
    let SymbolInfo::Complete(local_type) = store.symbols.get(local).info else {
        return Err(semantic_error("local type is incomplete"));
    };
    if !is_canonical_class_type_ref(store, local_type, counter, package, definitions.no_prefix) {
        return Err(semantic_error(
            "local explicit type must be the canonical Counter TypeRef",
        ));
    }
    let typed_type_tree = source_typed_index
        .get(source_id, local_def.tpt)
        .ok_or_else(|| typed_error("explicit source type tree has no typed mapping"))?;
    let typed_initializer = source_typed_index
        .get(source_id, initializer)
        .ok_or_else(|| typed_error("original initializer has no typed mapping"))?;
    let TreeKind::Apply(source_apply) = &source_ast
        .try_get(initializer)
        .ok_or_else(|| source_error("local initializer tree is stale"))?
        .kind
    else {
        return Err(source_error(
            "local initializer must be the original direct constructor Apply",
        ));
    };
    if source_apply.kind != ApplyKind::Regular || source_apply.args.len() != 1 {
        return Err(source_error(
            "local initializer must be one regular one-argument Apply",
        ));
    }
    let source_select = source_apply.function;
    let TreeKind::Select(source_selection) = &source_ast
        .try_get(source_select)
        .ok_or_else(|| source_error("initializer Select is stale"))?
        .kind
    else {
        return Err(source_error(
            "initializer function must be the constructor Select",
        ));
    };
    if store.names.resolve(source_selection.name.text()) != "<init>" {
        return Err(source_error(
            "initializer must select the primary Counter constructor",
        ));
    }
    let source_new = source_selection.qualifier;
    let TreeKind::New(source_new_tree) = &source_ast
        .try_get(source_new)
        .ok_or_else(|| source_error("initializer New is stale"))?
        .kind
    else {
        return Err(source_error("initializer constructor receiver must be New"));
    };
    let source_argument = source_apply.args[0];
    let TreeKind::Ident(source_seed) = &source_ast
        .try_get(source_argument)
        .ok_or_else(|| source_error("initializer seed tree is stale"))?
        .kind
    else {
        return Err(source_error(
            "initializer argument must be the original seed Ident",
        ));
    };
    if store.names.resolve(source_seed.name.text())
        != store
            .names
            .resolve(store.symbols.get(seed_formal).name.text())
    {
        return Err(source_error(
            "initializer must use the original seed formal",
        ));
    }
    let typed_select = source_typed_index
        .get(source_id, source_select)
        .ok_or_else(|| typed_error("initializer Select has no typed mapping"))?;
    let typed_new = source_typed_index
        .get(source_id, source_new)
        .ok_or_else(|| typed_error("initializer New has no typed mapping"))?;
    let typed_new_type_tree = source_typed_index
        .get(source_id, source_new_tree.tpt)
        .ok_or_else(|| typed_error("initializer type tree has no typed mapping"))?;
    let typed_argument = source_typed_index
        .get(source_id, source_argument)
        .ok_or_else(|| typed_error("initializer seed has no typed mapping"))?;
    let TreeKind::Apply(typed_apply) = &typed_ast
        .try_get(typed_initializer)
        .ok_or_else(|| typed_error("typed initializer is stale"))?
        .kind
    else {
        return Err(typed_error("typed initializer must remain Apply"));
    };
    if typed_apply.kind != ApplyKind::Regular
        || typed_apply.args != [typed_argument]
        || typed_apply.function != typed_select
    {
        return Err(typed_error(
            "typed initializer Apply must retain exact Select and seed roots",
        ));
    }
    let TreeKind::Select(typed_selection) = &typed_ast
        .try_get(typed_select)
        .ok_or_else(|| typed_error("typed constructor Select is stale"))?
        .kind
    else {
        return Err(typed_error("typed initializer function must remain Select"));
    };
    let TreeKind::New(typed_new_tree) = &typed_ast
        .try_get(typed_new)
        .ok_or_else(|| typed_error("typed New is stale"))?
        .kind
    else {
        return Err(typed_error("typed constructor receiver must remain New"));
    };
    if typed_selection.qualifier != typed_new || typed_new_tree.tpt != typed_new_type_tree {
        return Err(typed_error(
            "typed initializer must preserve exact source Select/New/type roots",
        ));
    }
    if !is_canonical_class_type_ref(
        store,
        typed_ast
            .try_get(typed_new_type_tree)
            .ok_or_else(|| typed_error("typed New type tree is stale"))?
            .ty,
        counter,
        package,
        definitions.no_prefix,
    ) || !is_canonical_class_type_ref(
        store,
        typed_ast
            .try_get(typed_new)
            .ok_or_else(|| typed_error("typed New is stale"))?
            .ty,
        counter,
        package,
        definitions.no_prefix,
    ) {
        return Err(typed_error(
            "typed New must retain the canonical Counter TypeRef",
        ));
    }
    if !matches!(
        store.types.try_get(
            typed_ast
                .try_get(typed_initializer)
                .ok_or_else(|| typed_error("typed initializer is stale"))?
                .ty
        ),
        Some(Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }) if *class == counter
    ) {
        return Err(typed_error(
            "typed initializer result must retain the Counter TypeRef",
        ));
    }
    let constructor = match store.types.try_get(
        typed_ast
            .try_get(typed_select)
            .ok_or_else(|| typed_error("typed Select is stale"))?
            .ty,
    ) {
        Some(Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        }) => *symbol,
        _ => {
            return Err(typed_error(
                "typed constructor Select must retain its canonical TermRef",
            ));
        }
    };
    if !store.symbols.contains(constructor)
        || store.symbols.get(constructor).kind != SymbolKind::Constructor
        || store.symbols.get(constructor).owner != Some(counter)
        || !has_source_origin(store, constructor, source_id)
    {
        return Err(semantic_error(
            "selected constructor must belong to the canonical Counter class",
        ));
    }
    let TreeKind::Ident(typed_seed) = &typed_ast
        .try_get(typed_argument)
        .ok_or_else(|| typed_error("typed seed Ident is stale"))?
        .kind
    else {
        return Err(typed_error("typed initializer seed must remain Ident"));
    };
    let typed_seed_type = typed_ast
        .try_get(typed_argument)
        .ok_or_else(|| typed_error("typed seed Ident is stale"))?
        .ty;
    if store.names.resolve(typed_seed.name.text())
        != store
            .names
            .resolve(store.symbols.get(seed_formal).name.text())
        || !matches!(store.types.try_get(typed_seed_type), Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }) if *symbol == seed_formal)
    {
        return Err(typed_error(
            "typed initializer seed must retain the original Int formal TermRef",
        ));
    }
    let TreeKind::ValDef(typed_local_def) = &typed_ast
        .try_get(typed_declaration)
        .ok_or_else(|| typed_error("typed ValDef is stale"))?
        .kind
    else {
        return Err(typed_error("typed local declaration must remain ValDef"));
    };
    if typed_local_def.rhs != Some(typed_initializer) {
        return Err(typed_error("typed ValDef initializer mismatch"));
    }
    if typed_local_def.tpt != typed_type_tree {
        return Err(typed_error("typed ValDef declared-type edge mismatch"));
    }
    if typed_ast
        .try_get(typed_type_tree)
        .is_none_or(|tree| tree.ty != local_type)
    {
        return Err(typed_error("typed ValDef declared-type identity mismatch"));
    }
    if typed_ast
        .try_get(typed_declaration)
        .is_none_or(|tree| tree.ty != local_type)
    {
        return Err(typed_error(
            "typed local ValDef must retain its canonical Counter TypeId",
        ));
    }
    Ok(ImmutableLocalDeclarationCore {
        declaration,
        typed_declaration,
        symbol: local,
        scope: scope_id,
        local_type,
        source_type_tree: local_def.tpt,
        typed_type_tree,
        initializer,
        typed_initializer,
        source_select,
        typed_select,
        source_new,
        typed_new,
        source_new_type_tree: source_new_tree.tpt,
        typed_new_type_tree,
        source_argument,
        typed_argument,
        constructor,
    })
}

impl ValidatedImmutableLocalInitializationProof<'_> {
    pub const fn source_id(&self) -> SourceId {
        self.source_id
    }
    pub const fn method(&self) -> SymbolId {
        self.method
    }
    pub const fn local(&self) -> SymbolId {
        self.local
    }
    pub const fn scope(&self) -> dotty_core::ScopeId {
        self.scope
    }
    pub const fn constructor(&self) -> SymbolId {
        self.constructor
    }
    pub const fn source_block(&self) -> TreeId<Untyped> {
        self.source_block
    }
    pub const fn source_declaration(&self) -> TreeId<Untyped> {
        self.source_declaration
    }
    pub const fn source_type_tree(&self) -> TreeId<Untyped> {
        self.source_type_tree
    }
    pub const fn source_initializer(&self) -> TreeId<Untyped> {
        self.source_initializer
    }
    pub const fn source_initializer_select(&self) -> TreeId<Untyped> {
        self.source_initializer_select
    }
    pub const fn source_initializer_new(&self) -> TreeId<Untyped> {
        self.source_initializer_new
    }
    pub const fn source_seed_argument(&self) -> TreeId<Untyped> {
        self.source_seed_argument
    }
    pub const fn source_final_expression(&self) -> TreeId<Untyped> {
        self.source_final_expression
    }
    pub const fn source_use(&self) -> TreeId<Untyped> {
        self.source_use
    }
    pub const fn typed_block(&self) -> TreeId<Typed> {
        self.typed_block
    }
    pub const fn typed_declaration(&self) -> TreeId<Typed> {
        self.typed_declaration
    }
    pub const fn typed_type_tree(&self) -> TreeId<Typed> {
        self.typed_type_tree
    }
    pub const fn typed_initializer(&self) -> TreeId<Typed> {
        self.typed_initializer
    }
    pub const fn typed_initializer_select(&self) -> TreeId<Typed> {
        self.typed_initializer_select
    }
    pub const fn typed_initializer_new(&self) -> TreeId<Typed> {
        self.typed_initializer_new
    }
    pub const fn typed_seed_argument(&self) -> TreeId<Typed> {
        self.typed_seed_argument
    }
    pub const fn typed_final_expression(&self) -> TreeId<Typed> {
        self.typed_final_expression
    }
    pub const fn typed_use(&self) -> TreeId<Typed> {
        self.typed_use
    }
    pub const fn local_type(&self) -> TypeId {
        self.local_type
    }
    pub const fn initializer_result_type(&self) -> TypeId {
        self.initializer_result_type
    }
    pub const fn seed_formal(&self) -> SymbolId {
        self.seed_formal
    }
    pub const fn seed_formal_type(&self) -> TypeId {
        self.seed_formal_type
    }
    pub const fn constructor_formal(&self) -> SymbolId {
        self.constructor_formal
    }
    pub const fn constructor_formal_type(&self) -> TypeId {
        self.constructor_formal_type
    }
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
                match (proof.arguments[0].origin, proof.arguments[1].origin) {
                    (
                        OrderedConstructorArgumentOrigin::NestedNew(_),
                        OrderedConstructorArgumentOrigin::NestedNew(_),
                    ) => ConstructorArgumentProofKind::NestedNewPair,
                    (
                        OrderedConstructorArgumentOrigin::Formal(_),
                        OrderedConstructorArgumentOrigin::Formal(_),
                    ) => ConstructorArgumentProofKind::DirectFormalPair,
                    (
                        OrderedConstructorArgumentOrigin::Formal(_),
                        OrderedConstructorArgumentOrigin::LocalVal(_),
                    )
                    | (
                        OrderedConstructorArgumentOrigin::LocalVal(_),
                        OrderedConstructorArgumentOrigin::Formal(_),
                    ) => ConstructorArgumentProofKind::FormalLocalPair,
                    (
                        OrderedConstructorArgumentOrigin::LocalVal(_),
                        OrderedConstructorArgumentOrigin::LocalVal(_),
                    ) => ConstructorArgumentProofKind::LocalValPair,
                    _ => ConstructorArgumentProofKind::FormalNestedPair,
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
            OrderedConstructorArgumentOrigin::LocalVal(_) => None,
        }
    }
    pub const fn nested_new(self) -> Option<NestedConstructorArgumentProof> {
        match self.proof.origin {
            OrderedConstructorArgumentOrigin::NestedNew(proof) => Some(proof),
            OrderedConstructorArgumentOrigin::Formal(_) => None,
            OrderedConstructorArgumentOrigin::LocalVal(_) => None,
        }
    }
    pub const fn local_val(self) -> Option<ImmutableLocalArgumentProofView> {
        match self.proof.origin {
            OrderedConstructorArgumentOrigin::LocalVal(proof) => {
                Some(ImmutableLocalArgumentProofView { proof })
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImmutableLocalArgumentProofView {
    proof: ImmutableLocalArgumentProof,
}

impl ImmutableLocalArgumentProofView {
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
    pub const fn constructor(self) -> SymbolId {
        self.proof.constructor
    }
    pub const fn local_type(self) -> TypeId {
        self.proof.local_type
    }
    pub const fn result_type(self) -> TypeId {
        self.proof.result_type
    }
    pub const fn seed_formal(self) -> SymbolId {
        self.proof.seed_formal
    }
    pub const fn seed_formal_type(self) -> TypeId {
        self.proof.seed_formal_type
    }
    pub const fn constructor_formal(self) -> SymbolId {
        self.proof.constructor_formal
    }
    pub const fn constructor_formal_type(self) -> TypeId {
        self.proof.constructor_formal_type
    }
    pub const fn source_initializer_select(self) -> TreeId<Untyped> {
        self.proof.source_initializer_select
    }
    pub const fn source_initializer_new(self) -> TreeId<Untyped> {
        self.proof.source_initializer_new
    }
    pub const fn source_seed_argument(self) -> TreeId<Untyped> {
        self.proof.source_seed_argument
    }
    pub const fn typed_initializer_select(self) -> TreeId<Typed> {
        self.proof.typed_initializer_select
    }
    pub const fn typed_initializer_new(self) -> TreeId<Typed> {
        self.proof.typed_initializer_new
    }
    pub const fn typed_seed_argument(self) -> TreeId<Typed> {
        self.proof.typed_seed_argument
    }
    pub const fn source_type_tree(self) -> TreeId<Untyped> {
        self.proof.source_type_tree
    }
    pub const fn typed_type_tree(self) -> TreeId<Typed> {
        self.proof.typed_type_tree
    }
    pub const fn initializer_result_type(self) -> TypeId {
        self.proof.initializer_result_type
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
    /// Original immutable-local initializer receipts in source declaration order.
    /// This remains distinct from `arguments()`, whose positions may repeat an alias.
    pub const fn local_initializers(self) -> Option<[ImmutableLocalInitializerReceiptView; 2]> {
        match self.proof.local_initializers {
            Some(receipts) => Some([
                ImmutableLocalInitializerReceiptView {
                    receipt: receipts[0],
                },
                ImmutableLocalInitializerReceiptView {
                    receipt: receipts[1],
                },
            ]),
            None => None,
        }
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
    /// Canonical local-receiver Unit statement, when the exact three-statement
    /// local-pair envelope was independently authenticated.
    pub const fn local_receiver_unit_call(&self) -> Option<LocalReceiverUnitCallReceipt> {
        self.local_receiver_unit_call
    }

    /// Authenticated ordered pair of non-final local Counter.set statements.
    pub const fn local_receiver_unit_calls(&self) -> Option<LocalReceiverUnitCallsReceipt> {
        self.local_receiver_unit_calls
    }
    /// Exact canonical source/typed evidence for the third local in the
    /// bounded alias-plus-two-calls factory shape.
    pub const fn local_reference_alias(&self) -> Option<LocalReferenceAliasBinding> {
        self.local_reference_alias
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
                    typer,
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
    typer: Option<&SourceTyper<'_>>,
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
    let method_scope_id = source_index
        .scope_of(method)
        .ok_or_else(|| identity_error("factory method has no registered source scope"))?;
    if !store.scopes.contains(method_scope_id) {
        return Err(identity_error("factory method source scope is stale"));
    }
    let method_scope = store.scopes.get(method_scope_id);
    if method_scope.owner != Some(method) {
        return Err(identity_error(
            "factory method source scope has a noncanonical owner",
        ));
    }
    if method_scope.entered_symbols().count() != method_formals.len()
        || method_formals.iter().any(|formal| {
            let name = &store.symbols.get(*formal).name;
            method_scope.lookup_all(name) != [*formal]
        })
    {
        return Err(identity_error(
            "factory method scope must contain the exact canonical formal entries",
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

    let (source_apply_tree, source_block) =
        match source_ast.try_get(source_rhs).map(|node| &node.kind) {
            Some(TreeKind::Apply(_)) => (source_rhs, None),
            Some(TreeKind::Block(block))
                if (1..=2).contains(&block.stats.len())
                    || ((3..=4).contains(&block.stats.len())
                        && block.stats[2..].iter().all(|stat| {
                            matches!(
                                source_ast.try_get(*stat).map(|node| &node.kind),
                                Some(TreeKind::Apply(_))
                            )
                        }))
                    || (block.stats.len() == 5
                        && matches!(
                            source_ast.try_get(block.stats[2]).map(|node| &node.kind),
                            Some(TreeKind::ValDef(_))
                        )
                        && block.stats[3..].iter().all(|stat| {
                            matches!(
                                source_ast.try_get(*stat).map(|node| &node.kind),
                                Some(TreeKind::Apply(_))
                            )
                        })) =>
            {
                (block.expr, Some(block))
            }
            _ => {
                return Err(source_error(
                    "factory body must be a direct Apply or a bounded local Pair block",
                ));
            }
        };
    let typer_for_locals = if source_block.is_some() {
        Some(typer.ok_or_else(|| {
            typed_error("local Pair call requires the original Typer binding resolver")
        })?)
    } else {
        None
    };
    let immutable_local = if let Some(block) = source_block.filter(|block| block.stats.len() == 1) {
        let declaration = block.stats[0];
        if !matches!(
            source_ast.try_get(declaration).map(|node| &node.kind),
            Some(TreeKind::ValDef(_))
        ) {
            return Err(source_error(
                "local Pair call block must contain one val declaration",
            ));
        }
        Some(validate_immutable_local_initialization_proof(
            ImmutableLocalInitializationProofInput {
                source_id,
                method_tree,
                store,
                definitions,
                source_ast,
                typed_ast,
                source_index,
                source_typed_index,
                local_binding: typer_for_locals
                    .ok_or_else(|| typed_error("one-local Pair call requires its original Typer"))?
                    .typed_local_binding_at(source_id, declaration),
                jvm_unit,
            },
        )?)
    } else {
        None
    };
    let mut local_receiver_unit_call = None;
    let mut local_receiver_unit_calls = None;
    let mut local_reference_alias = None;
    let immutable_local_pair = if let Some(block) =
        source_block.filter(|block| matches!(block.stats.len(), 2..=5))
    {
        if block.stats.iter().any(|declaration| {
            !matches!(
                source_ast.try_get(*declaration).map(|node| &node.kind),
                Some(TreeKind::ValDef(_))
            )
        }) && (block.stats.len() < 3
            || block.stats[..2].iter().any(|declaration| {
                !matches!(
                    source_ast.try_get(*declaration).map(|node| &node.kind),
                    Some(TreeKind::ValDef(_))
                )
            })
            || block.stats[2..].iter().any(|stat| {
                !matches!(
                    source_ast.try_get(*stat).map(|node| &node.kind),
                    Some(TreeKind::Apply(_)) | Some(TreeKind::ValDef(_))
                )
            }))
        {
            return Err(source_error(
                "local Pair call block must contain two vals, an optional alias, then Counter.set calls",
            ));
        }
        let typer = typer_for_locals
            .ok_or_else(|| typed_error("two-local Pair call requires its original Typer"))?;
        let input = ImmutableLocalInitializerPairProofInput {
            source_id,
            method_tree,
            store,
            definitions,
            source_ast,
            typed_ast,
            source_index,
            source_typed_index,
            local_bindings: [
                typer.typed_local_binding_at(source_id, block.stats[0]),
                typer.typed_local_binding_at(source_id, block.stats[1]),
            ],
            jvm_unit,
        };
        if block.stats.len() == 5 {
            let calls = validate_immutable_local_counter_set_calls_after_alias_proof(
                ImmutableLocalReferenceAliasProofInput {
                    initializers: input,
                    alias_binding: typer.typed_local_binding_at(source_id, block.stats[2]),
                },
            )?;
            let alias = calls.alias();
            local_reference_alias = Some(LocalReferenceAliasBinding {
                source_block: alias.initializers.source_block(),
                typed_block: alias.initializers.typed_block(),
                source_declaration: alias.source_declaration,
                typed_declaration: alias.typed_declaration,
                source_initializer: alias.source_initializer,
                typed_initializer: alias.typed_initializer,
                source_type_tree: alias.source_type_tree,
                typed_type_tree: alias.typed_type_tree,
                source_target: alias.source_target,
                typed_target: alias.typed_target,
                local: alias.symbol,
                target: alias.target,
                scope: alias.scope,
                local_type: alias.local_type,
            });
            let pair = *alias.initializers();
            let [first, second] = calls.calls();
            local_receiver_unit_calls = Some(LocalReceiverUnitCallsReceipt {
                calls: [first, second].map(|call| LocalReceiverUnitCallReceipt {
                    source_apply: call.source_call(),
                    typed_apply: call.typed_call(),
                    source_select: call.source_select(),
                    typed_select: call.typed_select(),
                    source_receiver: call.source_receiver(),
                    typed_receiver: call.typed_receiver(),
                    source_argument: call.source_argument(),
                    typed_argument: call.typed_argument(),
                    receiver: call.receiver(),
                    method: call.method(),
                    argument_formal: call.formal(),
                    receiver_scope: call.receiver_scope(),
                    argument_type: call.argument_type(),
                    result_type: call.result_type(),
                }),
            });
            Some(pair)
        } else if block.stats.len() == 3 {
            let call = validate_immutable_local_counter_set_call_proof(input)?;
            let pair = *call.initializer_pair();
            local_receiver_unit_call = Some(LocalReceiverUnitCallReceipt {
                source_apply: call.source_call(),
                typed_apply: call.typed_call(),
                source_select: call.source_select(),
                typed_select: call.typed_select(),
                source_receiver: call.source_receiver(),
                typed_receiver: call.typed_receiver(),
                source_argument: call.source_argument(),
                typed_argument: call.typed_argument(),
                receiver: call.receiver(),
                method: call.method(),
                argument_formal: call.formal(),
                receiver_scope: call.receiver_scope(),
                argument_type: call.argument_type(),
                result_type: call.result_type(),
            });
            Some(pair)
        } else if block.stats.len() == 4 {
            let calls = validate_immutable_local_counter_set_calls_proof(input)?;
            let pair = *calls.initializer_pair();
            let [first, second] = calls.calls();
            local_receiver_unit_calls = Some(LocalReceiverUnitCallsReceipt {
                calls: [first, second].map(|call| LocalReceiverUnitCallReceipt {
                    source_apply: call.source_call(),
                    typed_apply: call.typed_call(),
                    source_select: call.source_select(),
                    typed_select: call.typed_select(),
                    source_receiver: call.source_receiver(),
                    typed_receiver: call.typed_receiver(),
                    source_argument: call.source_argument(),
                    typed_argument: call.typed_argument(),
                    receiver: call.receiver(),
                    method: call.method(),
                    argument_formal: call.formal(),
                    receiver_scope: call.receiver_scope(),
                    argument_type: call.argument_type(),
                    result_type: call.result_type(),
                }),
            });
            Some(pair)
        } else {
            Some(validate_immutable_local_initializer_pair_proof(input)?)
        }
    } else {
        None
    };
    let TreeKind::Apply(outer_apply) = &source_ast
        .try_get(source_apply_tree)
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
    let typed_outer_args = map_ordered_constructor_argument_bindings(
        source_typed_index,
        source_id,
        [outer_apply.args[0], outer_apply.args[1]],
    )?;
    let typed_apply_tree = source_typed_index
        .get(source_id, source_apply_tree)
        .ok_or_else(|| typed_error("final constructor Apply has no exact typed binding"))?;
    if source_block.is_some() {
        require_matching_source_typed_positions(
            source_ast,
            typed_ast,
            source_id,
            source_apply_tree,
            typed_apply_tree,
            "final Pair constructor Apply",
        )?;
    }
    let typed_root = typed_ast
        .try_get(typed_apply_tree)
        .ok_or_else(|| typed_error("factory typed final expression is stale"))?;
    let typed_apply = match &typed_root.kind {
        TreeKind::Apply(application) if application.kind == ApplyKind::Regular => application,
        _ => {
            return Err(typed_error(
                "factory typed RHS must remain the original regular Apply",
            ));
        }
    };
    if typed_apply.args.len() != 2 {
        return Err(typed_error(
            "typed outer Apply must preserve exact ordered source mappings",
        ));
    }
    if typed_apply.function != typed_select {
        return Err(typed_error(
            "typed outer Apply must preserve exact ordered source mappings",
        ));
    }
    validate_ordered_constructor_argument_bindings(
        source_typed_index,
        source_id,
        [outer_apply.args[0], outer_apply.args[1]],
        [typed_apply.args[0], typed_apply.args[1]],
    )?;
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
    for (source_tree, typed_tree, description) in [
        (source_rhs, typed_rhs, "factory body"),
        (source_select, typed_select, "outer Pair Select"),
        (source_new, typed_new, "outer Pair New"),
        (source_new_node.tpt, typed_new_tpt, "outer Pair type tree"),
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
    let legacy_pair_signature = method_formal_classes[0] == method_formal_classes[1];
    let formal_nested_signature = method_formal_classes[0] == Some(counter_class)
        && method_formal_types[1] == definitions.int;
    if !legacy_pair_signature && !formal_nested_signature {
        return Err(semantic_error(
            "factory formals must retain a supported ordered pair signature",
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

    let direct_formal_arguments = outer_apply.args.iter().copied().zip(typed_outer_args).all(
        |(source_argument, typed_argument)| {
            matches!(
                source_ast.try_get(source_argument).map(|node| &node.kind),
                Some(TreeKind::Ident(_))
            ) && matches!(
                typed_ast.try_get(typed_argument).and_then(|node| store.types.try_get(node.ty)),
                Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. })
                    if method_formals.contains(symbol)
            )
        },
    );
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
        for (index, typed_outer_argument) in typed_outer_args.iter().copied().enumerate() {
            let source_outer_argument = outer_apply.args[index];
            if immutable_local_pair.is_some()
                && !matches!(
                    source_ast
                        .try_get(source_outer_argument)
                        .map(|node| &node.kind),
                    Some(TreeKind::Ident(_))
                )
            {
                return Err(source_error(
                    "two-local Pair arguments must be direct original local Idents",
                ));
            }
            if matches!(
                source_ast
                    .try_get(source_outer_argument)
                    .map(|node| &node.kind),
                Some(TreeKind::Ident(_))
            ) {
                require_source_position(
                    source_ast,
                    source_id,
                    source_outer_argument,
                    if index == 0 {
                        "first Pair argument has a foreign source position"
                    } else {
                        "second Pair argument has a foreign source position"
                    },
                )?;
                let TreeKind::Ident(source_ident) = &source_ast
                    .try_get(source_outer_argument)
                    .ok_or_else(|| source_error("Pair argument source root is stale"))?
                    .kind
                else {
                    unreachable!("the source argument was checked as Ident")
                };
                let typed_argument_node = typed_ast
                    .try_get(typed_outer_argument)
                    .ok_or_else(|| typed_error("Pair formal argument typed root is stale"))?;
                if !matches!(typed_argument_node.kind, TreeKind::Ident(_)) {
                    return Err(typed_error("Pair formal argument typed root must be Ident"));
                }
                if let Some(local) = immutable_local.as_ref()
                    && matches!(
                        store.types.try_get(typed_argument_node.ty),
                        Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. })
                            if *symbol == local.local()
                    )
                {
                    if local.source_use() != source_outer_argument
                        || local.typed_use() != typed_outer_argument
                    {
                        return Err(typed_error(
                            "local Pair argument must preserve the exact original source and typed use roots",
                        ));
                    }
                    let TreeKind::Ident(local_ident) = &source_ast.get(source_outer_argument).kind
                    else {
                        unreachable!("the local receipt authenticates an Ident use")
                    };
                    let target = match store.types.try_get(typed_argument_node.ty) {
                        Some(Type::TermRef {
                            target: TermRefTarget::Symbol(target),
                            ..
                        }) => *target,
                        _ => {
                            return Err(typed_error(
                                "local Pair argument must retain its exact local TermRef",
                            ));
                        }
                    };
                    if target != local.local()
                        || store.names.resolve(local_ident.name.text())
                            != store
                                .names
                                .resolve(store.symbols.get(local.local()).name.text())
                        || store.symbols.get(local.local()).info
                            != SymbolInfo::Complete(local.local_type())
                    {
                        return Err(typed_error(
                            "ordered local Pair argument must retain its exact initialized local identity and type",
                        ));
                    }
                    let proof = ImmutableLocalArgumentProof {
                        source_block: local.source_block(),
                        source_declaration: local.source_declaration(),
                        source_initializer: local.source_initializer(),
                        source_use: local.source_use(),
                        typed_block: local.typed_block(),
                        typed_declaration: local.typed_declaration(),
                        typed_initializer: local.typed_initializer(),
                        typed_use: local.typed_use(),
                        symbol: local.local(),
                        scope: local.scope(),
                        constructor: local.constructor(),
                        local_type: local.local_type(),
                        result_type: typed_argument_node.ty,
                        seed_formal: local.seed_formal(),
                        seed_formal_type: local.seed_formal_type(),
                        source_initializer_select: local.source_initializer_select(),
                        source_initializer_new: local.source_initializer_new(),
                        source_seed_argument: local.source_seed_argument(),
                        typed_initializer_select: local.typed_initializer_select(),
                        typed_initializer_new: local.typed_initializer_new(),
                        typed_seed_argument: local.typed_seed_argument(),
                        source_type_tree: local.source_type_tree(),
                        typed_type_tree: local.typed_type_tree(),
                        initializer_result_type: local.initializer_result_type(),
                        constructor_formal: local.constructor_formal(),
                        constructor_formal_type: local.constructor_formal_type(),
                    };
                    ordered_argument_proofs.push(OrderedConstructorArgumentProof {
                        source_argument: source_outer_argument,
                        typed_argument: typed_outer_argument,
                        origin: OrderedConstructorArgumentOrigin::LocalVal(proof),
                    });
                    continue;
                }
                if let Some(local_pair) = immutable_local_pair.as_ref() {
                    if !matches!(typed_argument_node.kind, TreeKind::Ident(_)) {
                        return Err(typed_error(
                            "two-local Pair arguments must retain direct typed local Idents",
                        ));
                    }
                    let target = match store.types.try_get(typed_argument_node.ty) {
                        Some(Type::TermRef {
                            target: TermRefTarget::Symbol(target),
                            ..
                        }) => *target,
                        _ => {
                            return Err(typed_error(
                                "local Pair argument must retain its exact local TermRef",
                            ));
                        }
                    };
                    if let Some(receipt) = local_pair
                        .initializers
                        .iter()
                        .copied()
                        .find(|receipt| receipt.declaration.symbol == target)
                    {
                        let TreeKind::Ident(local_ident) = &source_ast
                            .try_get(source_outer_argument)
                            .ok_or_else(|| source_error("local Pair use is stale"))?
                            .kind
                        else {
                            return Err(source_error("local Pair argument must be an Ident"));
                        };
                        let local_name = store.symbols.get(receipt.declaration.symbol).name;
                        if store.names.resolve(local_ident.name.text())
                            != store.names.resolve(local_name.text())
                            || store.symbols.get(receipt.declaration.symbol).owner != Some(method)
                            || store.symbols.get(receipt.declaration.symbol).info
                                != SymbolInfo::Complete(receipt.declaration.local_type)
                        {
                            return Err(typed_error(
                                "ordered local Pair argument must retain its exact initialized local identity and type",
                            ));
                        }
                        if source_typed_index.get(source_id, source_outer_argument)
                            != Some(typed_outer_argument)
                        {
                            return Err(typed_error(
                                "local Pair argument must retain its exact source-to-typed binding",
                            ));
                        }
                        require_matching_source_typed_positions(
                            source_ast,
                            typed_ast,
                            source_id,
                            source_outer_argument,
                            typed_outer_argument,
                            if index == 0 {
                                "first initialized local Pair argument"
                            } else {
                                "second initialized local Pair argument"
                            },
                        )?;
                        let proof = immutable_local_argument_from_initializer(
                            local_pair.source_block,
                            local_pair.typed_block,
                            receipt,
                            source_outer_argument,
                            typed_outer_argument,
                            typed_argument_node.ty,
                        );
                        ordered_argument_proofs.push(OrderedConstructorArgumentProof {
                            source_argument: source_outer_argument,
                            typed_argument: typed_outer_argument,
                            origin: OrderedConstructorArgumentOrigin::LocalVal(proof),
                        });
                        continue;
                    }
                    return Err(typed_error(
                        "each two-local Pair argument must resolve to one of its original local receipts",
                    ));
                }
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
                if source_typed_index.get(source_id, source_outer_argument)
                    != Some(typed_outer_argument)
                {
                    return Err(typed_error(
                        "ordered Pair argument must retain its exact source-to-typed binding",
                    ));
                }
                require_matching_source_typed_positions(
                    source_ast,
                    typed_ast,
                    source_id,
                    source_outer_argument,
                    typed_outer_argument,
                    if index == 0 {
                        "first direct Pair argument"
                    } else {
                        "second direct Pair argument"
                    },
                )?;
                let formal_proof = FormalConstructorArgumentProof {
                    source_argument: source_outer_argument,
                    typed_argument: typed_outer_argument,
                    formal,
                    formal_type: method_formal_types[formal_index],
                    result_type: typed_argument_node.ty,
                };
                ordered_argument_proofs.push(OrderedConstructorArgumentProof {
                    source_argument: source_outer_argument,
                    typed_argument: typed_outer_argument,
                    origin: OrderedConstructorArgumentOrigin::Formal(formal_proof),
                });
                continue;
            }
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
            let nested_proof = NestedConstructorArgumentProof {
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
            };
            nested_proofs.push(nested_proof);
            ordered_argument_proofs.push(OrderedConstructorArgumentProof {
                source_argument: source_outer_argument,
                typed_argument: typed_outer_argument,
                origin: OrderedConstructorArgumentOrigin::NestedNew(nested_proof),
            });
        }
        if let [nested_first, nested_second] = nested_proofs.as_slice()
            && (nested_first.class() != nested_second.class()
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
                || nested_first.typed_argument() == nested_second.typed_argument())
        {
            return Err(typed_error(
                "sibling constructor receipts must preserve distinct ordered call-site roots",
            ));
        }
    }
    if formal_nested_signature
        && ordered_argument_proofs
            .iter()
            .filter(|argument| {
                matches!(argument.origin, OrderedConstructorArgumentOrigin::Formal(_))
            })
            .count()
            != 1
    {
        return Err(source_error(
            "mixed Pair arguments must contain exactly one formal and one nested Counter call",
        ));
    }
    let ordered_argument_proofs = <[OrderedConstructorArgumentProof; 2]>::try_from(
        ordered_argument_proofs,
    )
    .map_err(|_| {
        source_error("exactly two authenticated ordered constructor arguments are required")
    })?;
    let formal_class = ordered_argument_proofs
        .iter()
        .any(|argument| matches!(argument.origin, OrderedConstructorArgumentOrigin::Formal(_)))
        .then_some(counter_class);
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
        formal_class,
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
                    local_initializers: immutable_local_pair.map(|proof| proof.initializers),
                },
            ),
        },
        local_receiver_unit_call,
        local_receiver_unit_calls,
        local_reference_alias,
        formal_type: method_formal_types[0],
        constructor_formal_type,
        result_type,
        method_descriptor: method_descriptor.to_owned(),
        constructor_descriptor: constructor_descriptor.to_owned(),
    })
}

/// Resolve the canonical typed roots for two source-ordered constructor
/// arguments. Keeping this check separate lets callers exercise the argument
/// provenance validator without replacing a Typer-owned session index.
pub fn map_ordered_constructor_argument_bindings(
    source_typed_index: &SourceTypedIndex,
    source_id: SourceId,
    source_arguments: [TreeId<Untyped>; 2],
) -> Result<[TypedTreeId; 2], CrossClassConstructorCallProofError> {
    let typed_error = |message| CrossClassConstructorCallProofError::TypedBinding(message);
    let typed_arguments = [
        source_typed_index
            .get(source_id, source_arguments[0])
            .ok_or_else(|| typed_error("first outer argument has no exact typed binding"))?,
        source_typed_index
            .get(source_id, source_arguments[1])
            .ok_or_else(|| typed_error("second outer argument has no exact typed binding"))?,
    ];
    if source_arguments[0] == source_arguments[1] || typed_arguments[0] == typed_arguments[1] {
        return Err(typed_error(
            "sibling outer arguments must retain distinct source and typed roots",
        ));
    }
    Ok(typed_arguments)
}

/// Check that source argument edges retain their exact original order in the
/// typed Apply. This is shared by the proof path and focused provenance tests.
pub fn validate_ordered_constructor_argument_bindings(
    source_typed_index: &SourceTypedIndex,
    source_id: SourceId,
    source_arguments: [TreeId<Untyped>; 2],
    typed_arguments: [TypedTreeId; 2],
) -> Result<(), CrossClassConstructorCallProofError> {
    let expected =
        map_ordered_constructor_argument_bindings(source_typed_index, source_id, source_arguments)?;
    if expected != typed_arguments {
        return Err(CrossClassConstructorCallProofError::TypedBinding(
            "typed outer Apply must preserve exact ordered source mappings",
        ));
    }
    Ok(())
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

/// Validate the bounded original immutable-local initializer used by the
/// issue #2065 audit. This is intentionally not wired into constructor
/// argument planning or either producer.
pub fn validate_immutable_local_initialization_proof<'session>(
    input: ImmutableLocalInitializationProofInput<'session>,
) -> Result<ValidatedImmutableLocalInitializationProof<'session>, CrossClassConstructorCallProofError>
{
    let source_error = |s| CrossClassConstructorCallProofError::SourceShape(s);
    let identity_error = |s| CrossClassConstructorCallProofError::SourceIdentity(s);
    let semantic_error = |s| CrossClassConstructorCallProofError::SemanticIdentity(s);
    let typed_error = |s| CrossClassConstructorCallProofError::TypedBinding(s);
    let ImmutableLocalInitializationProofInput {
        source_id,
        method_tree,
        store,
        definitions,
        source_ast,
        typed_ast,
        source_index,
        source_typed_index,
        local_binding,
        jvm_unit,
    } = input;
    if !std::ptr::eq(jvm_unit.store, store) || !std::ptr::eq(jvm_unit.typed_ast, typed_ast) {
        return Err(CrossClassConstructorCallProofError::SemanticIdentity(
            "JVM unit and source proof must borrow the same semantic and typed stores",
        ));
    }
    let method_node = source_ast
        .try_get(method_tree)
        .ok_or_else(|| source_error("factory method tree is stale"))?;
    let TreeKind::DefDef(method_def) = &method_node.kind else {
        return Err(source_error("factory source root must be a method"));
    };
    require_source_position(
        source_ast,
        source_id,
        method_tree,
        "factory method has a foreign source position",
    )?;
    let method = source_index
        .symbol_at(source_id, method_tree)
        .ok_or_else(|| identity_error("factory method has no canonical source symbol"))?;
    if source_index.definition_of(method)
        != Some(SourceDefinition::Canonical {
            source: source_id,
            tree: method_tree,
        })
        || !store.symbols.contains(method)
        || !has_source_origin(store, method, source_id)
        || store.symbols.get(method).kind != SymbolKind::Method
    {
        return Err(identity_error(
            "factory method is not the original canonical method",
        ));
    }
    if !method_def.type_params.is_empty()
        || method_def.value_param_clauses.len() != 1
        || method_def.value_param_clauses[0].len() != 2
    {
        return Err(source_error(
            "factory method must have exactly two ordinary formals",
        ));
    }
    let owner = store
        .symbols
        .get(method)
        .owner
        .ok_or_else(|| identity_error("factory method has no source class owner"))?;
    if !store.symbols.contains(owner)
        || store.symbols.get(owner).kind != SymbolKind::Class
        || !has_source_origin(store, owner, source_id)
        || source_index.definition_of(owner).is_none_or(
            |definition| !matches!(definition, SourceDefinition::Canonical { source, .. } if source == source_id),
        )
    {
        return Err(identity_error("factory method owner must be its canonical source class"));
    }
    let package = store
        .symbols
        .get(owner)
        .owner
        .ok_or_else(|| identity_error("factory class has no package owner"))?;
    let mut formals = Vec::with_capacity(2);
    for tree in &method_def.value_param_clauses[0] {
        let TreeKind::ValDef(formal) = &source_ast
            .try_get(*tree)
            .ok_or_else(|| source_error("factory formal tree is stale"))?
            .kind
        else {
            return Err(source_error("factory parameters must be value formals"));
        };
        if formal.rhs.is_some() {
            return Err(source_error("factory formals cannot have defaults"));
        }
        if matches!(
            source_ast.try_get(formal.tpt).map(|node| &node.kind),
            Some(TreeKind::TypeTree(_))
        ) {
            return Err(source_error(
                "factory formals must retain explicit source types",
            ));
        }
        let symbol = source_index
            .symbol_at(source_id, *tree)
            .ok_or_else(|| identity_error("factory formal has no canonical source symbol"))?;
        if !store.symbols.contains(symbol)
            || !has_source_origin(store, symbol, source_id)
            || store.symbols.get(symbol).kind != SymbolKind::Parameter
            || store.symbols.get(symbol).owner != Some(method)
        {
            return Err(identity_error(
                "factory formal is not owned by the canonical method",
            ));
        }
        let SymbolInfo::Complete(ty) = store.symbols.get(symbol).info else {
            return Err(semantic_error("factory formal type is incomplete"));
        };
        require_source_position(
            source_ast,
            source_id,
            *tree,
            "factory formal has a foreign source span",
        )?;
        formals.push((symbol, ty, formal.name.as_name()));
    }
    let SymbolInfo::Complete(method_ty) = store.symbols.get(method).info else {
        return Err(semantic_error("factory method signature is incomplete"));
    };
    let Some(Type::Method(signature)) = store.types.try_get(method_ty) else {
        return Err(semantic_error(
            "factory method must have one plain signature",
        ));
    };
    if signature.kind != dotty_core::MethodKind::Plain
        || signature.params.len() != 2
        || signature.params[0].ty != formals[0].1
        || signature.params[1].ty != formals[1].1
    {
        return Err(semantic_error(
            "factory signature must preserve its two canonical formals",
        ));
    }
    let counter = canonical_class_type_symbol(store, formals[0].1, package, definitions.no_prefix)
        .ok_or_else(|| {
            semantic_error("first factory formal must be a canonical Counter class TypeRef")
        })?;
    if formals[1].1 != definitions.int {
        return Err(semantic_error(
            "seed formal must have the canonical Int type",
        ));
    }
    if !has_source_origin(store, counter, source_id)
        || store.symbols.get(counter).kind != SymbolKind::Class
        || source_index.definition_of(counter).is_none_or(
            |d| !matches!(d, SourceDefinition::Canonical { source, .. } if source == source_id),
        )
    {
        return Err(identity_error(
            "Counter must be a canonical class in the original source batch",
        ));
    }
    unique_constructor_call_class_plan(jvm_unit, owner)?;
    unique_constructor_call_class_plan(jvm_unit, counter)?;
    let rhs = method_def
        .rhs
        .ok_or_else(|| source_error("factory method must have a body"))?;
    let source_block_node = source_ast
        .try_get(rhs)
        .ok_or_else(|| source_error("factory body is stale"))?;
    let TreeKind::Block(block) = &source_block_node.kind else {
        return Err(source_error("factory body must be the original Block"));
    };
    if block.stats.len() != 1 {
        return Err(source_error(
            "factory Block must contain exactly one local ValDef",
        ));
    }
    let declaration = block.stats[0];
    let TreeKind::ValDef(local_def) = &source_ast
        .try_get(declaration)
        .ok_or_else(|| source_error("local declaration tree is stale"))?
        .kind
    else {
        return Err(source_error(
            "factory Block statistic must be the original ValDef",
        ));
    };
    if local_def.rhs.is_none() {
        return Err(source_error("local must have one initializer"));
    }
    if matches!(
        source_ast.try_get(local_def.tpt).map(|n| &n.kind),
        Some(TreeKind::TypeTree(_))
    ) {
        return Err(source_error("local must have an explicit source type"));
    }
    let binding = local_binding
        .ok_or_else(|| identity_error("local declaration has no original Typer local binding"))?;
    let local = binding.symbol();
    let scope_id = binding.scope();
    if binding.declaration()
        != source_typed_index.get(source_id, declaration).ok_or({
            CrossClassConstructorCallProofError::TypedBinding(
                "original ValDef has no source-to-typed binding",
            )
        })?
    {
        return Err(identity_error(
            "Typer local binding must name this exact original ValDef",
        ));
    }
    if !store.symbols.contains(local)
        || !has_source_origin(store, local, source_id)
        || store.symbols.get(local).kind != SymbolKind::Local
        || store.symbols.get(local).owner != Some(method)
        || store
            .symbols
            .get(local)
            .flags
            .contains(SymbolFlags::MUTABLE)
    {
        return Err(identity_error(
            "local must be the original immutable method-owned local",
        ));
    }
    let local_name = store.symbols.get(local).name;
    if store.names.resolve(local_name.text())
        != store.names.resolve(local_def.name.as_name().text())
        || formals.iter().any(|(_, _, name)| {
            store.names.resolve(name.text()) == store.names.resolve(local_name.text())
        })
    {
        return Err(identity_error(
            "local must retain its source name without shadowing a formal",
        ));
    }
    if !store.scopes.contains(scope_id) {
        return Err(identity_error("local block scope is stale"));
    }
    let scope = store.scopes.get(scope_id);
    if scope.owner != Some(method)
        || scope.lookup(&local_name) != Some(local)
        || scope.entered_symbols().count() != 1
    {
        return Err(identity_error(
            "local must occupy its exact single-entry method-owned Block scope",
        ));
    }
    let SymbolInfo::Complete(local_type) = store.symbols.get(local).info else {
        return Err(semantic_error("local type is incomplete"));
    };
    if !is_canonical_class_type_ref(store, local_type, counter, package, definitions.no_prefix) {
        return Err(semantic_error(
            "local explicit type must be the canonical Counter TypeRef",
        ));
    }
    let initializer = local_def.rhs.unwrap();
    let TreeKind::Apply(init_apply) = &source_ast
        .try_get(initializer)
        .ok_or_else(|| source_error("local initializer tree is stale"))?
        .kind
    else {
        return Err(source_error(
            "local initializer must be the original direct constructor Apply",
        ));
    };
    if init_apply.kind != ApplyKind::Regular || init_apply.args.len() != 1 {
        return Err(source_error(
            "local initializer must be one regular one-argument Apply",
        ));
    }
    let source_select = init_apply.function;
    let TreeKind::Select(selection) = &source_ast
        .try_get(source_select)
        .ok_or_else(|| source_error("initializer Select is stale"))?
        .kind
    else {
        return Err(source_error(
            "initializer function must be the constructor Select",
        ));
    };
    if store.names.resolve(selection.name.text()) != "<init>" {
        return Err(source_error(
            "initializer must select the primary Counter constructor",
        ));
    }
    let source_new = selection.qualifier;
    let TreeKind::New(new_tree) = &source_ast
        .try_get(source_new)
        .ok_or_else(|| source_error("initializer New is stale"))?
        .kind
    else {
        return Err(source_error("initializer constructor receiver must be New"));
    };
    let source_argument = init_apply.args[0];
    let TreeKind::Ident(seed_ident) = &source_ast
        .try_get(source_argument)
        .ok_or_else(|| source_error("initializer seed tree is stale"))?
        .kind
    else {
        return Err(source_error(
            "initializer argument must be the original seed Ident",
        ));
    };
    if store.names.resolve(seed_ident.name.text()) != store.names.resolve(formals[1].2.text()) {
        return Err(source_error(
            "initializer must use the original seed formal",
        ));
    }
    let typed_block = source_typed_index
        .get(source_id, rhs)
        .ok_or_else(|| typed_error("original Block has no typed mapping"))?;
    let typed_decl = source_typed_index
        .get(source_id, declaration)
        .ok_or_else(|| typed_error("original ValDef has no typed mapping"))?;
    let typed_init = source_typed_index
        .get(source_id, initializer)
        .ok_or_else(|| typed_error("original initializer has no typed mapping"))?;
    let typed_select = source_typed_index
        .get(source_id, source_select)
        .ok_or_else(|| typed_error("initializer Select has no typed mapping"))?;
    let typed_new = source_typed_index
        .get(source_id, source_new)
        .ok_or_else(|| typed_error("initializer New has no typed mapping"))?;
    let typed_new_tpt = source_typed_index
        .get(source_id, new_tree.tpt)
        .ok_or_else(|| typed_error("initializer type tree has no typed mapping"))?;
    let typed_seed = source_typed_index
        .get(source_id, source_argument)
        .ok_or_else(|| typed_error("initializer seed has no typed mapping"))?;
    let typed_use = source_typed_index
        .get(source_id, block.expr)
        .ok_or_else(|| typed_error("final Block expression has no typed mapping"))?;
    let TreeKind::Block(typed_block_tree) = &typed_ast
        .try_get(typed_block)
        .ok_or_else(|| typed_error("typed Block is stale"))?
        .kind
    else {
        return Err(typed_error(
            "original source Block must remain a typed Block",
        ));
    };
    if typed_block_tree.stats != [typed_decl] || typed_block_tree.expr != typed_use {
        return Err(typed_error(
            "typed Block must preserve exactly the original ValDef and final expression",
        ));
    }
    let TreeKind::ValDef(typed_local_def) = &typed_ast
        .try_get(typed_decl)
        .ok_or_else(|| typed_error("typed ValDef is stale"))?
        .kind
    else {
        return Err(typed_error("typed local declaration must remain ValDef"));
    };
    if typed_local_def.rhs != Some(typed_init)
        || typed_local_def.tpt
            != source_typed_index
                .get(source_id, local_def.tpt)
                .ok_or_else(|| typed_error("explicit source type tree has no typed mapping"))?
        || typed_ast
            .try_get(typed_local_def.tpt)
            .is_none_or(|t| t.ty != local_type)
    {
        return Err(typed_error(
            "typed ValDef must preserve the original explicit type and initializer",
        ));
    }
    let TreeKind::Apply(typed_init_apply) = &typed_ast
        .try_get(typed_init)
        .ok_or_else(|| typed_error("typed initializer is stale"))?
        .kind
    else {
        return Err(typed_error("typed initializer must remain Apply"));
    };
    if typed_init_apply.kind != ApplyKind::Regular
        || typed_init_apply.args != [typed_seed]
        || typed_init_apply.function != typed_select
    {
        return Err(typed_error(
            "typed initializer Apply must retain exact Select and seed roots",
        ));
    }
    let TreeKind::Select(typed_selection) = &typed_ast
        .try_get(typed_select)
        .ok_or_else(|| typed_error("typed constructor Select is stale"))?
        .kind
    else {
        return Err(typed_error("typed initializer function must remain Select"));
    };
    let TreeKind::New(typed_new_tree) = &typed_ast
        .try_get(typed_new)
        .ok_or_else(|| typed_error("typed New is stale"))?
        .kind
    else {
        return Err(typed_error("typed constructor receiver must remain New"));
    };
    if typed_selection.qualifier != typed_new
        || typed_new_tree.tpt != typed_new_tpt
        || typed_init_apply.function != source_typed_index.get(source_id, source_select).unwrap()
    {
        return Err(typed_error(
            "typed initializer must preserve exact source Select/New/type roots",
        ));
    }
    let typed_counter_type = typed_ast
        .try_get(typed_new_tpt)
        .ok_or_else(|| typed_error("typed New type tree is stale"))?
        .ty;
    if !is_canonical_class_type_ref(
        store,
        typed_counter_type,
        counter,
        package,
        definitions.no_prefix,
    ) {
        return Err(typed_error(
            "typed New type tree must retain the canonical Counter TypeRef",
        ));
    }
    if !is_canonical_class_type_ref(
        store,
        typed_ast
            .try_get(typed_new)
            .ok_or_else(|| typed_error("typed New is stale"))?
            .ty,
        counter,
        package,
        definitions.no_prefix,
    ) {
        return Err(typed_error(
            "typed New result must retain the canonical Counter TypeRef",
        ));
    }
    if !matches!(
        store.types.try_get(
            typed_ast
                .try_get(typed_init)
                .ok_or_else(|| typed_error("typed initializer is stale"))?
                .ty
        ),
        Some(Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }) if *class == counter
    ) {
        return Err(typed_error(
            "typed initializer result must retain the Counter TypeRef",
        ));
    }
    let constructor = match store.types.try_get(
        typed_ast
            .try_get(typed_select)
            .ok_or_else(|| typed_error("typed Select is stale"))?
            .ty,
    ) {
        Some(Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        }) => *symbol,
        _ => {
            return Err(typed_error(
                "typed constructor Select must retain its canonical TermRef",
            ));
        }
    };
    if !store.symbols.contains(constructor)
        || store.symbols.get(constructor).kind != SymbolKind::Constructor
        || store.symbols.get(constructor).owner != Some(counter)
        || !has_source_origin(store, constructor, source_id)
    {
        return Err(semantic_error(
            "selected constructor must belong to the canonical Counter class",
        ));
    }
    let counter_source = match source_index.definition_of(counter) {
        Some(SourceDefinition::Canonical { source, tree }) if source == source_id => tree,
        _ => return Err(identity_error("Counter source definition is not canonical")),
    };
    let TreeKind::TypeDef(counter_def) = &source_ast
        .try_get(counter_source)
        .ok_or_else(|| identity_error("Counter definition tree is stale"))?
        .kind
    else {
        return Err(identity_error("Counter source definition must be TypeDef"));
    };
    let TreeKind::Template(counter_template) = &source_ast
        .try_get(counter_def.rhs)
        .ok_or_else(|| identity_error("Counter template is stale"))?
        .kind
    else {
        return Err(identity_error("Counter definition must have a Template"));
    };
    if source_index.symbol_at(source_id, counter_template.constructor) != Some(constructor) {
        return Err(identity_error(
            "selected constructor must be the original Counter primary constructor",
        ));
    }
    let TreeKind::DefDef(constructor_def) = &source_ast
        .try_get(counter_template.constructor)
        .ok_or_else(|| identity_error("primary constructor tree is stale"))?
        .kind
    else {
        return Err(identity_error(
            "Counter primary constructor source tree must be DefDef",
        ));
    };
    if constructor_def.value_param_clauses.len() != 1
        || constructor_def.value_param_clauses[0].len() != 1
    {
        return Err(source_error(
            "Counter primary constructor must have one Int formal",
        ));
    }
    let ctor_formal = source_index
        .derived_symbol_at(
            constructor,
            source_id,
            constructor_def.value_param_clauses[0][0],
        )
        .ok_or_else(|| identity_error("Counter constructor formal is not canonically derived"))?;
    let SymbolInfo::Complete(ctor_ty) = store.symbols.get(constructor).info else {
        return Err(semantic_error(
            "Counter constructor signature is incomplete",
        ));
    };
    let Some(Type::Method(ctor_sig)) = store.types.try_get(ctor_ty) else {
        return Err(semantic_error(
            "Counter constructor must have a method signature",
        ));
    };
    if !store.symbols.contains(ctor_formal)
        || store.symbols.get(ctor_formal).owner != Some(constructor)
        || store.symbols.get(ctor_formal).kind != SymbolKind::Parameter
        || !matches!(store.symbols.get(ctor_formal).info, SymbolInfo::Complete(t) if t == definitions.int)
        || ctor_sig.kind != dotty_core::MethodKind::Plain
        || ctor_sig.params.len() != 1
        || ctor_sig.params[0].ty != definitions.int
    {
        return Err(semantic_error(
            "selected Counter constructor must have the canonical Int signature",
        ));
    }
    let TreeKind::Ident(typed_seed_ident) = &typed_ast
        .try_get(typed_seed)
        .ok_or_else(|| typed_error("typed seed Ident is stale"))?
        .kind
    else {
        return Err(typed_error("typed initializer seed must remain Ident"));
    };
    if store.names.resolve(typed_seed_ident.name.text()) != store.names.resolve(formals[1].2.text())
        || !matches!(store.types.try_get(typed_ast.try_get(typed_seed).unwrap().ty), Some(Type::TermRef { target: TermRefTarget::Symbol(s), .. }) if *s == formals[1].0)
    {
        return Err(typed_error(
            "typed initializer seed must retain the original Int formal TermRef",
        ));
    }
    let TreeKind::Apply(final_apply) = &source_ast
        .try_get(block.expr)
        .ok_or_else(|| source_error("final Block expression is stale"))?
        .kind
    else {
        return Err(source_error(
            "final Block expression must be the original Pair constructor Apply",
        ));
    };
    if final_apply.kind != ApplyKind::Regular || final_apply.args.len() != 2 {
        return Err(source_error(
            "final Pair constructor call must have two regular arguments",
        ));
    }
    let typed_final = typed_ast
        .try_get(typed_use)
        .ok_or_else(|| typed_error("final expression typed tree is stale"))?;
    let TreeKind::Apply(typed_final_apply) = &typed_final.kind else {
        return Err(typed_error(
            "final Pair constructor expression must remain Apply",
        ));
    };
    if typed_final_apply.args.len() != 2 || typed_final_apply.kind != ApplyKind::Regular {
        return Err(typed_error(
            "typed final constructor must retain two regular arguments",
        ));
    }
    let source_outer_select = final_apply.function;
    let TreeKind::Select(source_outer_selection) = &source_ast
        .try_get(source_outer_select)
        .ok_or_else(|| source_error("final Pair constructor Select is stale"))?
        .kind
    else {
        return Err(source_error(
            "final Pair constructor function must be Select",
        ));
    };
    if store.names.resolve(source_outer_selection.name.text()) != "<init>" {
        return Err(source_error(
            "final application must select the Pair primary constructor",
        ));
    }
    let source_outer_new = source_outer_selection.qualifier;
    let TreeKind::New(source_pair_new) = &source_ast
        .try_get(source_outer_new)
        .ok_or_else(|| source_error("final Pair New is stale"))?
        .kind
    else {
        return Err(source_error("final constructor receiver must be New Pair"));
    };
    let typed_outer_select = source_typed_index
        .get(source_id, source_outer_select)
        .ok_or_else(|| typed_error("final Pair Select has no original typed mapping"))?;
    let typed_outer_new = source_typed_index
        .get(source_id, source_outer_new)
        .ok_or_else(|| typed_error("final Pair New has no original typed mapping"))?;
    if typed_final_apply.function != typed_outer_select {
        return Err(typed_error(
            "typed final Apply must preserve its original Pair Select",
        ));
    }
    let TreeKind::Select(typed_outer_selection) = &typed_ast
        .try_get(typed_outer_select)
        .ok_or_else(|| typed_error("typed final Pair Select is stale"))?
        .kind
    else {
        return Err(typed_error("typed final Pair function must remain Select"));
    };
    let TreeKind::New(typed_pair_new) = &typed_ast
        .try_get(typed_outer_new)
        .ok_or_else(|| typed_error("typed final Pair New is stale"))?
        .kind
    else {
        return Err(typed_error(
            "typed final constructor receiver must remain New",
        ));
    };
    let typed_pair_tpt = source_typed_index
        .get(source_id, source_pair_new.tpt)
        .ok_or_else(|| typed_error("final Pair type tree has no original typed mapping"))?;
    if typed_outer_selection.qualifier != typed_outer_new || typed_pair_new.tpt != typed_pair_tpt {
        return Err(typed_error(
            "typed final Pair call must preserve the exact New and type trees",
        ));
    }
    let pair_type = typed_ast
        .try_get(typed_pair_tpt)
        .ok_or_else(|| typed_error("typed Pair type tree is stale"))?
        .ty;
    let pair_class = match store.types.try_get(pair_type) {
        Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(class),
            ..
        }) => *class,
        _ => {
            return Err(typed_error(
                "final New type must retain a canonical Pair class TypeRef",
            ));
        }
    };
    if pair_class == owner
        || pair_class == counter
        || !has_source_origin(store, pair_class, source_id)
        || store.symbols.get(pair_class).kind != SymbolKind::Class
        || source_index.definition_of(pair_class).is_none_or(
            |d| !matches!(d, SourceDefinition::Canonical { source, .. } if source == source_id),
        )
        || !is_canonical_class_type_ref(
            store,
            pair_type,
            pair_class,
            package,
            definitions.no_prefix,
        )
    {
        return Err(identity_error(
            "Pair must be a different canonical class in the original source batch",
        ));
    }
    if !is_canonical_class_type_ref(
        store,
        typed_ast
            .try_get(typed_outer_new)
            .ok_or_else(|| typed_error("typed final Pair New is stale"))?
            .ty,
        pair_class,
        package,
        definitions.no_prefix,
    ) {
        return Err(typed_error(
            "typed final Pair New result must retain the canonical Pair TypeRef",
        ));
    }
    let typed_final_class = match store.types.try_get(typed_final.ty) {
        Some(Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(class),
        }) if *prefix == definitions.no_prefix => Some(*class),
        _ => canonical_class_type_symbol(store, typed_final.ty, package, definitions.no_prefix),
    };
    if typed_final_class != Some(pair_class)
        || canonical_class_type_symbol(store, signature.result, package, definitions.no_prefix)
            != Some(pair_class)
    {
        return Err(typed_error(
            "typed final Apply must retain the method's canonical Pair result type",
        ));
    }
    unique_constructor_call_class_plan(jvm_unit, pair_class)?;
    let pair_constructor = match store.types.try_get(
        typed_ast
            .try_get(typed_outer_select)
            .ok_or_else(|| typed_error("typed Pair Select is stale"))?
            .ty,
    ) {
        Some(Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        }) => *symbol,
        _ => {
            return Err(typed_error(
                "typed Pair Select must retain its constructor TermRef",
            ));
        }
    };
    if !store.symbols.contains(pair_constructor)
        || store.symbols.get(pair_constructor).kind != SymbolKind::Constructor
        || store.symbols.get(pair_constructor).owner != Some(pair_class)
    {
        return Err(semantic_error(
            "final Select must choose the constructor owned by Pair",
        ));
    }
    let TreeKind::TypeDef(pair_def) = &source_ast
        .try_get(
            source_index
                .definition_of(pair_class)
                .and_then(|d| match d {
                    SourceDefinition::Canonical { tree, .. } => Some(tree),
                    _ => None,
                })
                .ok_or_else(|| identity_error("Pair source definition is missing"))?,
        )
        .ok_or_else(|| identity_error("Pair source tree is stale"))?
        .kind
    else {
        return Err(identity_error("Pair source definition must be TypeDef"));
    };
    let TreeKind::Template(pair_template) = &source_ast
        .try_get(pair_def.rhs)
        .ok_or_else(|| identity_error("Pair template is stale"))?
        .kind
    else {
        return Err(identity_error(
            "Pair source definition must contain a Template",
        ));
    };
    if source_index.symbol_at(source_id, pair_template.constructor) != Some(pair_constructor) {
        return Err(identity_error(
            "final call must select Pair's original primary constructor",
        ));
    }
    let TreeKind::DefDef(pair_ctor_def) = &source_ast
        .try_get(pair_template.constructor)
        .ok_or_else(|| identity_error("Pair primary constructor tree is stale"))?
        .kind
    else {
        return Err(identity_error("Pair primary constructor must be DefDef"));
    };
    if pair_ctor_def.value_param_clauses.len() != 1
        || pair_ctor_def.value_param_clauses[0].len() != 2
    {
        return Err(source_error(
            "Pair primary constructor must have exactly two formals",
        ));
    }
    let SymbolInfo::Complete(pair_ctor_type) = store.symbols.get(pair_constructor).info else {
        return Err(semantic_error("Pair constructor signature is incomplete"));
    };
    let Some(Type::Method(pair_ctor_signature)) = store.types.try_get(pair_ctor_type) else {
        return Err(semantic_error(
            "Pair constructor signature must be a method type",
        ));
    };
    if pair_ctor_signature.kind != dotty_core::MethodKind::Plain
        || pair_ctor_signature.params.len() != 2
        || pair_ctor_signature.params.iter().any(|p| {
            canonical_class_type_symbol(store, p.ty, package, definitions.no_prefix)
                != Some(counter)
        })
    {
        return Err(semantic_error(
            "Pair primary constructor must take exactly two Counter formals",
        ));
    }
    if signature.result != pair_type
        && canonical_class_type_symbol(store, signature.result, package, definitions.no_prefix)
            != Some(pair_class)
    {
        return Err(semantic_error(
            "factory method result must retain the canonical Pair type",
        ));
    }
    let mut local_arg = None;
    let mut formal_arg = None;
    for arg in final_apply.args.iter().copied() {
        let TreeKind::Ident(ident) = &source_ast
            .try_get(arg)
            .ok_or_else(|| source_error("final constructor argument tree is stale"))?
            .kind
        else {
            return Err(source_error(
                "final Pair constructor arguments must be original Ident leaves",
            ));
        };
        let normalized = store.names.resolve(ident.name.text());
        if normalized == store.names.resolve(local_name.text()) {
            if local_arg.replace(arg).is_some() {
                return Err(source_error(
                    "final Pair call duplicates the local reference",
                ));
            }
        } else if normalized == store.names.resolve(formals[0].2.text()) {
            if formal_arg.replace(arg).is_some() {
                return Err(source_error(
                    "final Pair call duplicates the existing formal",
                ));
            }
        } else {
            return Err(source_error(
                "final Pair arguments must be the existing formal and exact local",
            ));
        }
    }
    let local_arg = local_arg
        .ok_or_else(|| source_error("final Pair call must use the original local Ident"))?;
    let formal_arg = formal_arg.ok_or_else(|| {
        source_error("final Pair call must preserve the original existing formal")
    })?;
    let typed_local_use = source_typed_index
        .get(source_id, local_arg)
        .ok_or_else(|| typed_error("final local Ident has no original typed mapping"))?;
    let typed_formal_use = source_typed_index
        .get(source_id, formal_arg)
        .ok_or_else(|| typed_error("final existing formal has no original typed mapping"))?;
    let expected_typed_args = final_apply
        .args
        .iter()
        .map(|source_arg| {
            source_typed_index
                .get(source_id, *source_arg)
                .ok_or_else(|| {
                    typed_error("final constructor argument has no original typed mapping")
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if typed_final_apply.args != expected_typed_args {
        return Err(typed_error(
            "typed final Apply must match the original source argument order and roots",
        ));
    }
    if !matches!(store.types.try_get(typed_ast.try_get(typed_local_use)
        .ok_or_else(|| typed_error("typed local use is stale"))?.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(s), .. }) if *s == local)
        || !matches!(store.types.try_get(typed_ast.try_get(typed_formal_use)
        .ok_or_else(|| typed_error("typed existing formal is stale"))?.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(s), .. }) if *s == formals[0].0)
    {
        return Err(typed_error(
            "final arguments must retain exact local and formal TermRefs",
        ));
    }
    let source_seed_pos = require_source_position(
        source_ast,
        source_id,
        source_argument,
        "seed argument has foreign source span",
    )?;
    let source_decl_pos = require_source_position(
        source_ast,
        source_id,
        declaration,
        "local ValDef has foreign source span",
    )?;
    let source_init_pos = require_source_position(
        source_ast,
        source_id,
        initializer,
        "initializer has foreign source span",
    )?;
    let source_use_pos = require_source_position(
        source_ast,
        source_id,
        local_arg,
        "final local use has foreign source span",
    )?;
    if source_decl_pos.span().range().start() >= source_seed_pos.span().range().start()
        || source_init_pos.span().range().start() >= source_use_pos.span().range().start()
    {
        return Err(source_error(
            "local declaration and initialization must precede the final use",
        ));
    }
    for (s, t, desc) in [
        (rhs, typed_block, "Block"),
        (declaration, typed_decl, "ValDef"),
        (initializer, typed_init, "initializer"),
        (source_argument, typed_seed, "seed"),
        (local_arg, typed_local_use, "local use"),
    ] {
        require_matching_source_typed_positions(source_ast, typed_ast, source_id, s, t, desc)?;
    }
    for (s, t, desc) in [
        (local_def.tpt, typed_local_def.tpt, "local declared type"),
        (
            source_select,
            typed_select,
            "initializer constructor Select",
        ),
        (source_new, typed_new, "initializer New"),
        (new_tree.tpt, typed_new_tpt, "initializer New type"),
        (source_outer_select, typed_outer_select, "final Pair Select"),
        (source_outer_new, typed_outer_new, "final Pair New"),
        (source_pair_new.tpt, typed_pair_tpt, "final Pair type"),
        (formal_arg, typed_formal_use, "final existing formal"),
    ] {
        require_matching_source_typed_positions(source_ast, typed_ast, source_id, s, t, desc)?;
    }
    // Keep the original wrapper's error ordering and exact diagnostics above,
    // then capture the shared borrowed declaration/initializer contract used
    // by both one-local and two-local receipts.
    let local_core = validate_immutable_local_declaration_core(
        source_id,
        method,
        declaration,
        store,
        definitions,
        source_ast,
        typed_ast,
        source_typed_index,
        Some(binding),
        counter,
        package,
        formals[1].0,
    )?;
    Ok(ValidatedImmutableLocalInitializationProof {
        _store: store,
        _source_ast: source_ast,
        _typed_ast: typed_ast,
        _source_index: source_index,
        _source_typed_index: source_typed_index,
        _jvm_unit: jvm_unit,
        source_id,
        method,
        local: local_core.symbol,
        scope: local_core.scope,
        constructor: local_core.constructor,
        source_block: rhs,
        source_declaration: local_core.declaration,
        source_type_tree: local_core.source_type_tree,
        source_initializer: local_core.initializer,
        source_initializer_select: local_core.source_select,
        source_initializer_new: local_core.source_new,
        source_seed_argument: local_core.source_argument,
        source_final_expression: block.expr,
        source_use: local_arg,
        typed_block,
        typed_declaration: local_core.typed_declaration,
        typed_type_tree: local_core.typed_type_tree,
        typed_initializer: local_core.typed_initializer,
        typed_initializer_select: local_core.typed_select,
        typed_initializer_new: local_core.typed_new,
        typed_seed_argument: local_core.typed_argument,
        typed_final_expression: typed_use,
        typed_use: typed_local_use,
        local_type: local_core.local_type,
        initializer_result_type: typed_ast
            .try_get(typed_init)
            .ok_or_else(|| typed_error("typed initializer became stale"))?
            .ty,
        seed_formal: formals[1].0,
        seed_formal_type: formals[1].1,
        constructor_formal: ctor_formal,
        constructor_formal_type: definitions.int,
    })
}

fn original_initializer_seed_formal(
    source_id: SourceId,
    declaration: TreeId<Untyped>,
    source_ast: &AstArena<Untyped>,
    typed_ast: &TypedAst,
    source_typed_index: &SourceTypedIndex,
    method_formals: &[SymbolId],
    store: &SemanticStore,
) -> Result<SymbolId, CrossClassConstructorCallProofError> {
    let source_error = |message| CrossClassConstructorCallProofError::SourceShape(message);
    let typed_error = |message| CrossClassConstructorCallProofError::TypedBinding(message);
    let TreeKind::ValDef(local) = &source_ast
        .try_get(declaration)
        .ok_or_else(|| source_error("local declaration tree is stale"))?
        .kind
    else {
        return Err(source_error(
            "factory Block statistic must be the original ValDef",
        ));
    };
    let initializer = local
        .rhs
        .ok_or_else(|| source_error("local must have one initializer"))?;
    let TreeKind::Apply(apply) = &source_ast
        .try_get(initializer)
        .ok_or_else(|| source_error("local initializer tree is stale"))?
        .kind
    else {
        return Err(source_error(
            "local initializer must be the original direct constructor Apply",
        ));
    };
    if apply.kind != ApplyKind::Regular || apply.args.len() != 1 {
        return Err(source_error(
            "local initializer must be one regular one-argument Apply",
        ));
    }
    let source_argument = apply.args[0];
    if !matches!(
        source_ast.try_get(source_argument).map(|tree| &tree.kind),
        Some(TreeKind::Ident(_))
    ) {
        return Err(source_error(
            "initializer argument must be the original seed Ident",
        ));
    }
    let typed_argument = source_typed_index
        .get(source_id, source_argument)
        .ok_or_else(|| typed_error("initializer seed has no typed mapping"))?;
    let TreeKind::Ident(_) = &typed_ast
        .try_get(typed_argument)
        .ok_or_else(|| typed_error("typed seed Ident is stale"))?
        .kind
    else {
        return Err(typed_error("typed initializer seed must remain Ident"));
    };
    let Some(Type::TermRef {
        target: TermRefTarget::Symbol(formal),
        ..
    }) = store.types.try_get(
        typed_ast
            .try_get(typed_argument)
            .ok_or_else(|| typed_error("typed seed Ident is stale"))?
            .ty,
    )
    else {
        return Err(source_error(
            "initializer must use the original seed formal",
        ));
    };
    if !method_formals.contains(formal) {
        return Err(source_error(
            "initializer must use the original seed formal",
        ));
    }
    Ok(*formal)
}

#[allow(clippy::too_many_arguments)]
/// Validate exactly two original, ordered immutable local initializers from one
/// straight-line source Block. The returned pair is created only after both
/// local receipts and their shared scope/order invariants have passed.
pub fn validate_immutable_local_initializer_pair_proof<'session>(
    input: ImmutableLocalInitializerPairProofInput<'session>,
) -> Result<
    ValidatedImmutableLocalInitializerPairProof<'session>,
    CrossClassConstructorCallProofError,
> {
    validate_immutable_local_initializer_pair_proof_inner(input, 0, false)
}

/// Authenticate the supported immutable direct-reference alias of the first
/// original Counter local. This proof deliberately leaves the following
/// Counter.set statements and Pair construction outside its receipt.
pub fn validate_immutable_local_reference_alias_proof<'session>(
    input: ImmutableLocalReferenceAliasProofInput<'session>,
) -> Result<ValidatedImmutableLocalReferenceAliasProof<'session>, CrossClassConstructorCallProofError>
{
    let pair = validate_immutable_local_initializer_pair_proof_inner(input.initializers, 2, true)?;
    validate_immutable_local_reference_alias_from_pair(pair, input.alias_binding)
}

#[allow(clippy::too_many_arguments)]
fn validate_immutable_local_initializer_pair_proof_inner<'session>(
    input: ImmutableLocalInitializerPairProofInput<'session>,
    counter_set_call_count: usize,
    alias_prefix: bool,
) -> Result<
    ValidatedImmutableLocalInitializerPairProof<'session>,
    CrossClassConstructorCallProofError,
> {
    let source_error = |s| CrossClassConstructorCallProofError::SourceShape(s);
    let identity_error = |s| CrossClassConstructorCallProofError::SourceIdentity(s);
    let semantic_error = |s| CrossClassConstructorCallProofError::SemanticIdentity(s);
    let typed_error = |s| CrossClassConstructorCallProofError::TypedBinding(s);
    let ImmutableLocalInitializerPairProofInput {
        source_id,
        method_tree,
        store,
        definitions,
        source_ast,
        typed_ast,
        source_index,
        source_typed_index,
        local_bindings,
        jvm_unit,
    } = input;
    if !std::ptr::eq(jvm_unit.store, store) || !std::ptr::eq(jvm_unit.typed_ast, typed_ast) {
        return Err(semantic_error(
            "JVM unit and source proof must borrow the same semantic and typed stores",
        ));
    }
    let TreeKind::DefDef(method_def) = &source_ast
        .try_get(method_tree)
        .ok_or_else(|| source_error("factory method tree is stale"))?
        .kind
    else {
        return Err(source_error("factory source root must be a method"));
    };
    require_source_position(
        source_ast,
        source_id,
        method_tree,
        "factory method has a foreign source position",
    )?;
    let method = source_index
        .symbol_at(source_id, method_tree)
        .ok_or_else(|| identity_error("factory method has no canonical source symbol"))?;
    if source_index.definition_of(method)
        != Some(SourceDefinition::Canonical {
            source: source_id,
            tree: method_tree,
        })
        || !store.symbols.contains(method)
        || !has_source_origin(store, method, source_id)
        || store.symbols.get(method).kind != SymbolKind::Method
    {
        return Err(identity_error(
            "factory method is not the original canonical method",
        ));
    }
    if !method_def.type_params.is_empty()
        || method_def.value_param_clauses.len() != 1
        || method_def.value_param_clauses[0].len() != 2
    {
        return Err(source_error(
            "factory method must have exactly two ordinary Int formals",
        ));
    }
    let owner = store
        .symbols
        .get(method)
        .owner
        .ok_or_else(|| identity_error("factory method has no source class owner"))?;
    if !store.symbols.contains(owner)
        || store.symbols.get(owner).kind != SymbolKind::Class
        || !has_source_origin(store, owner, source_id)
        || source_index.definition_of(owner).is_none_or(
            |d| !matches!(d, SourceDefinition::Canonical { source, .. } if source == source_id),
        )
    {
        return Err(identity_error(
            "factory method owner must be its canonical source class",
        ));
    }
    let package = store
        .symbols
        .get(owner)
        .owner
        .ok_or_else(|| identity_error("factory class has no package owner"))?;
    let SymbolInfo::Complete(method_type) = store.symbols.get(method).info else {
        return Err(semantic_error("factory method signature is incomplete"));
    };
    let Some(Type::Method(signature)) = store.types.try_get(method_type) else {
        return Err(semantic_error(
            "factory method must have one plain signature",
        ));
    };
    if signature.kind != dotty_core::MethodKind::Plain
        || signature.params.len() != 2
        || signature
            .params
            .iter()
            .any(|param| param.ty != definitions.int)
    {
        return Err(semantic_error(
            "factory signature must preserve its two canonical Int formals",
        ));
    }
    let mut formals = Vec::with_capacity(2);
    for source_formal in &method_def.value_param_clauses[0] {
        let TreeKind::ValDef(formal_def) = &source_ast
            .try_get(*source_formal)
            .ok_or_else(|| source_error("factory formal tree is stale"))?
            .kind
        else {
            return Err(source_error("factory parameters must be value formals"));
        };
        if formal_def.rhs.is_some() {
            return Err(source_error("factory formals cannot have defaults"));
        }
        if matches!(
            source_ast.try_get(formal_def.tpt).map(|tree| &tree.kind),
            Some(TreeKind::TypeTree(_))
        ) {
            return Err(source_error(
                "factory formals must retain explicit source types",
            ));
        }
        let formal = source_index
            .symbol_at(source_id, *source_formal)
            .ok_or_else(|| identity_error("factory formal has no canonical source symbol"))?;
        if !store.symbols.contains(formal)
            || !has_source_origin(store, formal, source_id)
            || store.symbols.get(formal).kind != SymbolKind::Parameter
            || store.symbols.get(formal).owner != Some(method)
        {
            return Err(identity_error(
                "factory formal is not owned by the canonical method",
            ));
        }
        if !matches!(store.symbols.get(formal).info, SymbolInfo::Complete(ty) if ty == definitions.int)
        {
            return Err(semantic_error(
                "factory formals must retain the canonical Int type",
            ));
        }
        require_source_position(
            source_ast,
            source_id,
            *source_formal,
            "factory formal has a foreign source span",
        )?;
        formals.push(formal);
    }
    let rhs = method_def
        .rhs
        .ok_or_else(|| source_error("factory method must have a body"))?;
    let TreeKind::Block(block) = &source_ast
        .try_get(rhs)
        .ok_or_else(|| source_error("factory body is stale"))?
        .kind
    else {
        return Err(source_error("factory body must be the original Block"));
    };
    let expected_stats = 2 + usize::from(alias_prefix) + counter_set_call_count;
    if block.stats.len() != expected_stats {
        return Err(source_error(match counter_set_call_count {
            0 if !alias_prefix => "factory Block must contain exactly two local ValDefs",
            1 => "factory Block must contain exactly two local ValDefs and one Counter.set call",
            2 => "factory Block must contain exactly two local ValDefs and two Counter.set calls",
            _ => "factory Block has an unsupported number of Counter.set calls",
        }));
    }
    if block.stats[0] == block.stats[1] {
        return Err(identity_error(
            "two local declarations must have distinct original source roots",
        ));
    }
    if alias_prefix
        && !matches!(
            source_ast.try_get(block.stats[2]).map(|node| &node.kind),
            Some(TreeKind::ValDef(_))
        )
    {
        return Err(source_error(
            "third factory Block statistic must be the immutable local alias ValDef",
        ));
    }
    if block.stats[2 + usize::from(alias_prefix)..]
        .iter()
        .any(|stat| {
            !matches!(
                source_ast.try_get(*stat).map(|node| &node.kind),
                Some(TreeKind::Apply(_))
            )
        })
    {
        return Err(source_error(if counter_set_call_count == 1 {
            "third factory Block statistic must be the original Counter.set Apply"
        } else {
            "factory call statistics must be original Apply trees"
        }));
    }
    let typed_block = source_typed_index
        .get(source_id, rhs)
        .ok_or_else(|| typed_error("original Block has no typed mapping"))?;
    let typed_final_expression = source_typed_index
        .get(source_id, block.expr)
        .ok_or_else(|| typed_error("final Block expression has no typed mapping"))?;
    let typed_declarations = [
        source_typed_index
            .get(source_id, block.stats[0])
            .ok_or_else(|| typed_error("first ValDef has no typed mapping"))?,
        source_typed_index
            .get(source_id, block.stats[1])
            .ok_or_else(|| typed_error("second ValDef has no typed mapping"))?,
    ];
    if typed_declarations[0] == typed_declarations[1] {
        return Err(typed_error(
            "two source ValDefs must retain distinct typed roots",
        ));
    }
    let TreeKind::Block(typed_block_tree) = &typed_ast
        .try_get(typed_block)
        .ok_or_else(|| typed_error("typed Block is stale"))?
        .kind
    else {
        return Err(typed_error("original source Block must remain typed Block"));
    };
    if typed_block_tree.stats.get(..2) != Some(typed_declarations.as_slice())
        || typed_block_tree.stats.len() != expected_stats
        || typed_block_tree.expr != typed_final_expression
    {
        return Err(typed_error(
            "typed Block must preserve both original ValDefs and final expression",
        ));
    }
    let TreeKind::Apply(final_apply) = &source_ast
        .try_get(block.expr)
        .ok_or_else(|| source_error("final Block expression is stale"))?
        .kind
    else {
        return Err(source_error(
            "final Block expression must be the enclosing constructor Apply",
        ));
    };
    if final_apply.kind != ApplyKind::Regular {
        return Err(source_error(
            "final Block expression must be one regular constructor Apply",
        ));
    }
    let source_final_select = final_apply.function;
    let TreeKind::Select(source_final_selection) = &source_ast
        .try_get(source_final_select)
        .ok_or_else(|| source_error("final constructor Select is stale"))?
        .kind
    else {
        return Err(source_error(
            "final constructor Apply function must be Select",
        ));
    };
    if store.names.resolve(source_final_selection.name.text()) != "<init>" {
        return Err(source_error(
            "final Block expression must select a primary constructor",
        ));
    }
    let source_final_new = source_final_selection.qualifier;
    let TreeKind::New(source_final_new_tree) = &source_ast
        .try_get(source_final_new)
        .ok_or_else(|| source_error("final constructor New is stale"))?
        .kind
    else {
        return Err(source_error(
            "final constructor Select qualifier must be New",
        ));
    };
    if final_apply.args.len() != 2 {
        return Err(source_error(
            "final constructor Apply must have exactly two arguments",
        ));
    }
    let typed_final_select = source_typed_index
        .get(source_id, source_final_select)
        .ok_or_else(|| typed_error("final constructor Select has no typed mapping"))?;
    let typed_final_new = source_typed_index
        .get(source_id, source_final_new)
        .ok_or_else(|| typed_error("final constructor New has no typed mapping"))?;
    let typed_final_new_type = source_typed_index
        .get(source_id, source_final_new_tree.tpt)
        .ok_or_else(|| typed_error("final constructor type tree has no typed mapping"))?;
    let TreeKind::Apply(typed_final_apply) = &typed_ast
        .try_get(typed_final_expression)
        .ok_or_else(|| typed_error("typed final expression is stale"))?
        .kind
    else {
        return Err(typed_error("final source Apply must remain a typed Apply"));
    };
    if typed_final_apply.kind != ApplyKind::Regular
        || typed_final_apply.args.len() != 2
        || typed_final_apply.function != typed_final_select
    {
        return Err(typed_error(
            "typed final constructor Apply must retain its original Select edge",
        ));
    }
    let TreeKind::Select(typed_final_selection) = &typed_ast
        .try_get(typed_final_select)
        .ok_or_else(|| typed_error("typed final constructor Select is stale"))?
        .kind
    else {
        return Err(typed_error(
            "typed final constructor function must remain Select",
        ));
    };
    let TreeKind::New(typed_final_new_tree) = &typed_ast
        .try_get(typed_final_new)
        .ok_or_else(|| typed_error("typed final constructor New is stale"))?
        .kind
    else {
        return Err(typed_error(
            "typed final constructor receiver must remain New",
        ));
    };
    if typed_final_selection.qualifier != typed_final_new
        || typed_final_new_tree.tpt != typed_final_new_type
    {
        return Err(typed_error(
            "typed final constructor must preserve exact Select/New/type edges",
        ));
    }
    let Type::TypeRef {
        target: TypeRefTarget::Symbol(result_class),
        ..
    } = store
        .types
        .try_get(signature.result)
        .ok_or_else(|| semantic_error("factory result type is stale"))?
    else {
        return Err(semantic_error(
            "factory result type must be a canonical class TypeRef",
        ));
    };
    if !store.symbols.contains(*result_class)
        || store.symbols.get(*result_class).kind != SymbolKind::Class
        || !has_source_origin(store, *result_class, source_id)
        || source_index.definition_of(*result_class).is_none_or(|definition| {
            !matches!(definition, SourceDefinition::Canonical { source, .. } if source == source_id)
        })
        || !is_canonical_class_type_ref(
            store,
            signature.result,
            *result_class,
            package,
            definitions.no_prefix,
        )
    {
        return Err(identity_error(
            "factory result class must be canonical in the original source package",
        ));
    }
    let typed_final_new_type_value = typed_ast
        .try_get(typed_final_new_type)
        .ok_or_else(|| typed_error("typed final constructor type tree is stale"))?
        .ty;
    let typed_final_new_value = typed_ast
        .try_get(typed_final_new)
        .ok_or_else(|| typed_error("typed final constructor New is stale"))?
        .ty;
    let typed_final_apply_value = typed_ast
        .try_get(typed_final_expression)
        .ok_or_else(|| typed_error("typed final constructor Apply is stale"))?
        .ty;
    if !is_canonical_class_type_ref(
        store,
        typed_final_new_type_value,
        *result_class,
        package,
        definitions.no_prefix,
    ) || !matches!(
        store.types.try_get(typed_final_new_value),
        Some(Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }) if *class == *result_class
    ) || !matches!(
        store.types.try_get(typed_final_apply_value),
        Some(Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }) if *class == *result_class
    ) {
        return Err(typed_error(
            "typed final constructor must preserve the canonical method result class",
        ));
    }
    let final_constructor = match store.types.try_get(
        typed_ast
            .try_get(typed_final_select)
            .ok_or_else(|| typed_error("typed final constructor Select is stale"))?
            .ty,
    ) {
        Some(Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        }) => *symbol,
        _ => {
            return Err(typed_error(
                "typed final constructor Select must retain its canonical TermRef",
            ));
        }
    };
    let result_class_tree = match source_index.definition_of(*result_class) {
        Some(SourceDefinition::Canonical { source, tree }) if source == source_id => tree,
        _ => {
            return Err(identity_error(
                "factory result class is not canonical source",
            ));
        }
    };
    let TreeKind::TypeDef(result_class_def) = &source_ast
        .try_get(result_class_tree)
        .ok_or_else(|| identity_error("factory result class tree is stale"))?
        .kind
    else {
        return Err(identity_error("factory result source root must be TypeDef"));
    };
    let TreeKind::Template(result_class_template) = &source_ast
        .try_get(result_class_def.rhs)
        .ok_or_else(|| identity_error("factory result class template is stale"))?
        .kind
    else {
        return Err(identity_error("factory result class must have Template"));
    };
    let expected_final_constructor = source_index
        .symbol_at(source_id, result_class_template.constructor)
        .ok_or_else(|| identity_error("factory result constructor is not canonical"))?;
    if final_constructor != expected_final_constructor
        || !store.symbols.contains(final_constructor)
        || store.symbols.get(final_constructor).kind != SymbolKind::Constructor
        || store.symbols.get(final_constructor).owner != Some(*result_class)
    {
        return Err(semantic_error(
            "typed final Apply must select the original result-class constructor",
        ));
    }
    let bindings = [
        local_bindings[0].ok_or_else(|| {
            identity_error("first local declaration has no original Typer binding")
        })?,
        local_bindings[1].ok_or_else(|| {
            identity_error("second local declaration has no original Typer binding")
        })?,
    ];
    let local_symbols = [bindings[0].symbol(), bindings[1].symbol()];
    if local_symbols[0] == local_symbols[1] {
        return Err(identity_error(
            "two local declarations must have distinct original symbols",
        ));
    }
    if local_symbols
        .iter()
        .any(|symbol| !store.symbols.contains(*symbol))
    {
        return Err(identity_error(
            "Typer local binding names a stale or foreign symbol",
        ));
    }
    let first_type = match store.symbols.get(local_symbols[0]).info {
        SymbolInfo::Complete(ty) => ty,
        _ => return Err(semantic_error("first local type is incomplete")),
    };
    let final_constructor_definition = source_ast
        .try_get(result_class_template.constructor)
        .ok_or_else(|| identity_error("factory result constructor tree is stale"))?;
    let TreeKind::DefDef(final_constructor_source) = &final_constructor_definition.kind else {
        return Err(identity_error(
            "factory result primary constructor must be a source DefDef",
        ));
    };
    if !final_constructor_source.type_params.is_empty()
        || final_constructor_source.value_param_clauses.len() != 1
        || final_constructor_source.value_param_clauses[0].len() != 2
    {
        return Err(source_error(
            "result primary constructor must have exactly two ordinary formals",
        ));
    }
    let SymbolInfo::Complete(final_constructor_type) =
        store.symbols.get(expected_final_constructor).info
    else {
        return Err(semantic_error(
            "result primary constructor signature is incomplete",
        ));
    };
    let Some(Type::Method(final_constructor_signature)) =
        store.types.try_get(final_constructor_type)
    else {
        return Err(semantic_error(
            "result primary constructor must have a method signature",
        ));
    };
    let counter = canonical_class_type_symbol(store, first_type, package, definitions.no_prefix)
        .ok_or_else(|| semantic_error("first local type must be a canonical Counter TypeRef"))?;
    if !has_source_origin(store, counter, source_id)
        || store.symbols.get(counter).kind != SymbolKind::Class
        || source_index.definition_of(counter).is_none_or(
            |d| !matches!(d, SourceDefinition::Canonical { source, .. } if source == source_id),
        )
    {
        return Err(identity_error(
            "Counter must be a canonical class in the original source batch",
        ));
    }
    if final_constructor_signature.kind != dotty_core::MethodKind::Plain
        || final_constructor_signature.params.len() != 2
        || final_constructor_signature.params.iter().any(|parameter| {
            canonical_class_type_symbol(store, parameter.ty, package, definitions.no_prefix)
                != Some(counter)
        })
    {
        return Err(semantic_error(
            "result primary constructor must take exactly two canonical Counter formals",
        ));
    }
    if !matches!(
        source_ast.try_get(block.stats[0]).map(|node| &node.kind),
        Some(TreeKind::ValDef(_))
    ) || !matches!(
        source_ast.try_get(block.stats[1]).map(|node| &node.kind),
        Some(TreeKind::ValDef(_))
    ) {
        return Err(source_error(
            "factory Block statistics must be the original ValDefs",
        ));
    }
    let first_name = store.symbols.get(local_symbols[0]).name;
    let second_name = store.symbols.get(local_symbols[1]).name;
    if store.names.resolve(first_name.text()) == store.names.resolve(second_name.text())
        || formals.iter().any(|formal| {
            store.symbols.get(*formal).name == first_name
                || store.symbols.get(*formal).name == second_name
        })
    {
        return Err(identity_error(
            "two locals must have distinct names without shadowing method formals",
        ));
    }
    let seed_formals = [
        original_initializer_seed_formal(
            source_id,
            block.stats[0],
            source_ast,
            typed_ast,
            source_typed_index,
            &formals,
            store,
        )?,
        original_initializer_seed_formal(
            source_id,
            block.stats[1],
            source_ast,
            typed_ast,
            source_typed_index,
            &formals,
            store,
        )?,
    ];
    if seed_formals[0] == seed_formals[1] {
        return Err(source_error(
            "two local initializers must use distinct original method formals",
        ));
    }
    let first_core = validate_immutable_local_declaration_core(
        source_id,
        method,
        block.stats[0],
        store,
        definitions,
        source_ast,
        typed_ast,
        source_typed_index,
        Some(bindings[0]),
        counter,
        package,
        seed_formals[0],
    )?;
    let second_core = validate_immutable_local_declaration_core(
        source_id,
        method,
        block.stats[1],
        store,
        definitions,
        source_ast,
        typed_ast,
        source_typed_index,
        Some(bindings[1]),
        counter,
        package,
        seed_formals[1],
    )?;
    if first_core.scope != second_core.scope {
        return Err(identity_error(
            "both immutable locals must belong to the same original Block scope",
        ));
    }
    let scope = store.scopes.get(first_core.scope);
    let scope_entries = scope.entered_symbols().collect::<Vec<_>>();
    if scope.owner != Some(method)
        || scope_entries.len() != 2 + usize::from(alias_prefix)
        || !scope_entries.contains(&local_symbols[0])
        || !scope_entries.contains(&local_symbols[1])
    {
        return Err(identity_error(
            "both locals must occupy the exact two-entry method-owned Block scope",
        ));
    }
    let first_decl_pos = require_source_position(
        source_ast,
        source_id,
        block.stats[0],
        "first local ValDef has a foreign source span",
    )?;
    let second_decl_pos = require_source_position(
        source_ast,
        source_id,
        block.stats[1],
        "second local ValDef has a foreign source span",
    )?;
    let first_init_pos = require_source_position(
        source_ast,
        source_id,
        first_core.initializer,
        "first local initializer has a foreign source span",
    )?;
    let second_init_pos = require_source_position(
        source_ast,
        source_id,
        second_core.initializer,
        "second local initializer has a foreign source span",
    )?;
    let final_pos = require_source_position(
        source_ast,
        source_id,
        block.expr,
        "final Block expression has a foreign source span",
    )?;
    if first_decl_pos.span().range().start() >= second_decl_pos.span().range().start()
        || first_decl_pos.span().range().start() >= first_init_pos.span().range().start()
        || second_decl_pos.span().range().start() >= second_init_pos.span().range().start()
        || first_init_pos.span().range().end() > second_decl_pos.span().range().start()
        || second_init_pos.span().range().end() > final_pos.span().range().start()
    {
        return Err(source_error(
            "two local initializers must be sequential and precede the final expression",
        ));
    }
    let constructor_tree = match source_index.definition_of(counter) {
        Some(SourceDefinition::Canonical { source, tree }) if source == source_id => tree,
        _ => return Err(identity_error("Counter source definition is not canonical")),
    };
    let TreeKind::TypeDef(counter_def) = &source_ast
        .try_get(constructor_tree)
        .ok_or_else(|| identity_error("Counter definition tree is stale"))?
        .kind
    else {
        return Err(identity_error("Counter source definition must be TypeDef"));
    };
    let TreeKind::Template(counter_template) = &source_ast
        .try_get(counter_def.rhs)
        .ok_or_else(|| identity_error("Counter template is stale"))?
        .kind
    else {
        return Err(identity_error("Counter definition must have a Template"));
    };
    let constructor = source_index
        .symbol_at(source_id, counter_template.constructor)
        .ok_or_else(|| identity_error("Counter primary constructor has no canonical symbol"))?;
    if first_core.constructor != constructor || second_core.constructor != constructor {
        return Err(identity_error(
            "both local initializers must select the original Counter primary constructor",
        ));
    }
    let TreeKind::DefDef(constructor_def) = &source_ast
        .try_get(counter_template.constructor)
        .ok_or_else(|| identity_error("Counter primary constructor tree is stale"))?
        .kind
    else {
        return Err(identity_error(
            "Counter primary constructor source tree must be DefDef",
        ));
    };
    if constructor_def.value_param_clauses.len() != 1
        || constructor_def.value_param_clauses[0].len() != 1
    {
        return Err(source_error(
            "Counter primary constructor must have one Int formal",
        ));
    }
    let constructor_formal = source_index
        .derived_symbol_at(
            constructor,
            source_id,
            constructor_def.value_param_clauses[0][0],
        )
        .ok_or_else(|| identity_error("Counter constructor formal is not canonically derived"))?;
    let SymbolInfo::Complete(constructor_type) = store.symbols.get(constructor).info else {
        return Err(semantic_error(
            "Counter constructor signature is incomplete",
        ));
    };
    let Some(Type::Method(constructor_signature)) = store.types.try_get(constructor_type) else {
        return Err(semantic_error(
            "Counter constructor must have a method signature",
        ));
    };
    if !store.symbols.contains(constructor_formal)
        || store.symbols.get(constructor_formal).owner != Some(constructor)
        || store.symbols.get(constructor_formal).kind != SymbolKind::Parameter
        || !matches!(store.symbols.get(constructor_formal).info, SymbolInfo::Complete(ty) if ty == definitions.int)
        || constructor_signature.kind != dotty_core::MethodKind::Plain
        || constructor_signature.params.len() != 1
        || constructor_signature.params[0].ty != definitions.int
    {
        return Err(semantic_error(
            "Counter primary constructor must retain its canonical Int signature",
        ));
    }
    unique_constructor_call_class_plan(jvm_unit, owner)?;
    unique_constructor_call_class_plan(jvm_unit, counter)?;
    let first_initializer_result_type = typed_ast
        .try_get(first_core.typed_initializer)
        .ok_or_else(|| typed_error("first typed initializer became stale"))?
        .ty;
    let second_initializer_result_type = typed_ast
        .try_get(second_core.typed_initializer)
        .ok_or_else(|| typed_error("second typed initializer became stale"))?
        .ty;
    let receipts = [
        ImmutableLocalInitializerReceipt {
            declaration: first_core,
            seed_formal: seed_formals[0],
            seed_formal_type: definitions.int,
            initializer_result_type: first_initializer_result_type,
            constructor_formal,
            constructor_formal_type: definitions.int,
        },
        ImmutableLocalInitializerReceipt {
            declaration: second_core,
            seed_formal: seed_formals[1],
            seed_formal_type: definitions.int,
            initializer_result_type: second_initializer_result_type,
            constructor_formal,
            constructor_formal_type: definitions.int,
        },
    ];
    for core in [first_core, second_core] {
        for (source_tree, typed_tree, label) in [
            (core.declaration, core.typed_declaration, "local ValDef"),
            (core.source_type_tree, core.typed_type_tree, "local type"),
            (core.initializer, core.typed_initializer, "initializer"),
            (core.source_select, core.typed_select, "constructor Select"),
            (core.source_new, core.typed_new, "constructor New"),
            (
                core.source_new_type_tree,
                core.typed_new_type_tree,
                "constructor New type",
            ),
            (core.source_argument, core.typed_argument, "seed formal"),
        ] {
            require_matching_source_typed_positions(
                source_ast,
                typed_ast,
                source_id,
                source_tree,
                typed_tree,
                label,
            )?;
        }
    }
    require_matching_source_typed_positions(
        source_ast,
        typed_ast,
        source_id,
        rhs,
        typed_block,
        "Block",
    )?;
    require_matching_source_typed_positions(
        source_ast,
        typed_ast,
        source_id,
        block.expr,
        typed_final_expression,
        "final Block expression",
    )?;
    for (source_tree, typed_tree, description) in [
        (
            source_final_select,
            typed_final_select,
            "final constructor Select",
        ),
        (source_final_new, typed_final_new, "final constructor New"),
        (
            source_final_new_tree.tpt,
            typed_final_new_type,
            "final constructor New type",
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
    Ok(ValidatedImmutableLocalInitializerPairProof {
        _store: store,
        _definitions: definitions,
        _source_ast: source_ast,
        _typed_ast: typed_ast,
        _source_index: source_index,
        _source_typed_index: source_typed_index,
        _jvm_unit: jvm_unit,
        source_id,
        method,
        counter,
        source_block: rhs,
        typed_block,
        source_final_expression: block.expr,
        typed_final_expression,
        initializers: receipts,
    })
}

fn validate_immutable_local_reference_alias_from_pair<'session>(
    pair: ValidatedImmutableLocalInitializerPairProof<'session>,
    alias_binding: Option<TypedLocalBinding>,
) -> Result<ValidatedImmutableLocalReferenceAliasProof<'session>, CrossClassConstructorCallProofError>
{
    let source_error = |s| CrossClassConstructorCallProofError::SourceShape(s);
    let identity_error = |s| CrossClassConstructorCallProofError::SourceIdentity(s);
    let semantic_error = |s| CrossClassConstructorCallProofError::SemanticIdentity(s);
    let typed_error = |s| CrossClassConstructorCallProofError::TypedBinding(s);
    let TreeKind::Block(block) = &pair
        ._source_ast
        .try_get(pair.source_block)
        .ok_or_else(|| source_error("original local Block is stale"))?
        .kind
    else {
        return Err(source_error("original local Block must remain a Block"));
    };
    let source_declaration = block.stats[2];
    let TreeKind::ValDef(alias_def) = &pair
        ._source_ast
        .try_get(source_declaration)
        .ok_or_else(|| source_error("alias declaration tree is stale"))?
        .kind
    else {
        return Err(source_error(
            "third factory Block statistic must be the immutable local alias ValDef",
        ));
    };
    let source_initializer = alias_def
        .rhs
        .ok_or_else(|| source_error("alias local must have one initializer"))?;
    let TreeKind::Ident(source_alias_ident) = &pair
        ._source_ast
        .try_get(source_initializer)
        .ok_or_else(|| source_error("alias initializer tree is stale"))?
        .kind
    else {
        return Err(source_error(
            "alias initializer must be the direct original local Ident",
        ));
    };
    let binding = alias_binding
        .ok_or_else(|| identity_error("alias declaration has no original Typer local binding"))?;
    let typed_declaration = pair
        ._source_typed_index
        .get(pair.source_id, source_declaration)
        .ok_or_else(|| typed_error("alias ValDef has no source-to-typed binding"))?;
    if binding.declaration() != typed_declaration {
        return Err(identity_error(
            "Typer alias binding must name this exact original ValDef",
        ));
    }
    let source_type_tree = alias_def.tpt;
    if matches!(
        pair._source_ast
            .try_get(source_type_tree)
            .map(|tree| &tree.kind),
        Some(TreeKind::TypeTree(_))
    ) {
        return Err(source_error(
            "alias must retain its explicit source Counter type",
        ));
    }
    let typed_type_tree = pair
        ._source_typed_index
        .get(pair.source_id, source_type_tree)
        .ok_or_else(|| typed_error("alias explicit type tree has no source-to-typed mapping"))?;
    let typed_initializer = pair
        ._source_typed_index
        .get(pair.source_id, source_initializer)
        .ok_or_else(|| typed_error("alias Ident has no source-to-typed binding"))?;
    let typed_target = typed_initializer;
    let TreeKind::Ident(typed_alias_ident) = &pair
        ._typed_ast
        .try_get(typed_target)
        .ok_or_else(|| typed_error("typed alias target is stale"))?
        .kind
    else {
        return Err(typed_error("typed alias target must remain Ident"));
    };
    if source_alias_ident.name != typed_alias_ident.name {
        return Err(typed_error(
            "source and typed alias Ident roots must preserve the same original reference",
        ));
    }
    let target = match pair._store.types.try_get(
        pair._typed_ast
            .try_get(typed_target)
            .ok_or_else(|| typed_error("typed alias target is stale"))?
            .ty,
    ) {
        Some(Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        }) => *symbol,
        _ => {
            return Err(typed_error(
                "typed alias target must retain its canonical local TermRef",
            ));
        }
    };
    let initializers = pair.initializers();
    let method_tree = match pair._source_index.definition_of(pair.method) {
        Some(SourceDefinition::Canonical { source, tree }) if source == pair.source_id => tree,
        _ => return Err(identity_error("factory method is not canonically indexed")),
    };
    let TreeKind::DefDef(method_def) = &pair
        ._source_ast
        .try_get(method_tree)
        .ok_or_else(|| identity_error("factory method tree is stale"))?
        .kind
    else {
        return Err(identity_error("factory method source root is not DefDef"));
    };
    let first_formal = pair
        ._source_index
        .symbol_at(pair.source_id, method_def.value_param_clauses[0][0])
        .ok_or_else(|| identity_error("first factory formal has no canonical source symbol"))?;
    let first_initializer_index = initializers
        .iter()
        .position(|initializer| initializer.seed_formal() == first_formal)
        .ok_or_else(|| {
            identity_error("no original local initializer uses the first Pair argument")
        })?;
    let original = initializers[first_initializer_index].symbol();
    let other = initializers[1 - first_initializer_index].symbol();
    let original_name = pair._store.symbols.get(original).name;
    if source_alias_ident.name != original_name || typed_alias_ident.name != original_name {
        return Err(typed_error(
            "source and typed alias Idents must agree with the authenticated original local",
        ));
    }
    let TreeKind::Apply(source_pair_apply) = &pair
        ._source_ast
        .try_get(pair.source_final_expression)
        .ok_or_else(|| source_error("final Pair Apply is stale"))?
        .kind
    else {
        return Err(source_error(
            "final expression must remain the original Pair Apply",
        ));
    };
    if source_pair_apply.args.len() != 2 {
        return Err(source_error(
            "final Pair Apply must retain exactly two arguments",
        ));
    }
    let source_pair_first_argument = source_pair_apply.args[0];
    let TreeKind::Ident(source_pair_first_ident) = &pair
        ._source_ast
        .try_get(source_pair_first_argument)
        .ok_or_else(|| source_error("first final Pair argument is stale"))?
        .kind
    else {
        return Err(source_error(
            "first final Pair argument must be the original local Ident",
        ));
    };
    let typed_pair_first_argument = pair
        ._source_typed_index
        .get(pair.source_id, source_pair_first_argument)
        .ok_or_else(|| typed_error("first final Pair argument has no typed mapping"))?;
    let TreeKind::Apply(typed_pair_apply) = &pair
        ._typed_ast
        .try_get(pair.typed_final_expression)
        .ok_or_else(|| typed_error("typed final Pair Apply is stale"))?
        .kind
    else {
        return Err(typed_error("typed final expression must remain Pair Apply"));
    };
    if typed_pair_apply.args.len() != 2 || typed_pair_apply.args[0] != typed_pair_first_argument {
        return Err(typed_error(
            "typed Pair arg[0] must retain its exact source mapping",
        ));
    }
    let TreeKind::Ident(typed_pair_first_ident) = &pair
        ._typed_ast
        .try_get(typed_pair_first_argument)
        .ok_or_else(|| typed_error("typed first final Pair argument is stale"))?
        .kind
    else {
        return Err(typed_error(
            "typed first final Pair argument must remain Ident",
        ));
    };
    let pair_first_target = match pair._store.types.try_get(
        pair._typed_ast
            .try_get(typed_pair_first_argument)
            .ok_or_else(|| typed_error("typed first final Pair argument is stale"))?
            .ty,
    ) {
        Some(Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        }) => *symbol,
        _ => {
            return Err(typed_error(
                "typed first final Pair argument must retain its local TermRef",
            ));
        }
    };
    if pair_first_target != original {
        return Err(identity_error(
            "alias target must be the canonical first final Pair argument local",
        ));
    }
    if source_pair_first_ident.name != typed_pair_first_ident.name {
        return Err(typed_error(
            "source and typed first Pair argument Idents must preserve the same reference",
        ));
    }
    if source_pair_first_ident.name != original_name || typed_pair_first_ident.name != original_name
    {
        return Err(typed_error(
            "source and typed first Pair argument Idents must agree with the authenticated original local",
        ));
    }
    let symbol = binding.symbol();
    if target != original {
        return Err(identity_error(
            "alias initializer must reference the canonical first original local",
        ));
    }
    if symbol == original || symbol == other || !pair._store.symbols.contains(symbol) {
        return Err(identity_error(
            "alias must be a distinct original local symbol",
        ));
    }
    let local_info = pair._store.symbols.get(symbol);
    if local_info.kind != SymbolKind::Local
        || local_info.owner != Some(pair.method)
        || local_info.flags.contains(SymbolFlags::MUTABLE)
        || !has_source_origin(pair._store, symbol, pair.source_id)
    {
        return Err(identity_error(
            "alias must be the original immutable method-owned local",
        ));
    }
    let scope = binding.scope();
    if scope != initializers[0].scope()
        || scope != initializers[1].scope()
        || !pair._store.scopes.contains(scope)
    {
        return Err(identity_error(
            "alias must belong to the original method Block scope",
        ));
    }
    let scope_info = pair._store.scopes.get(scope);
    let entries = scope_info.entered_symbols().collect::<Vec<_>>();
    if scope_info.owner != Some(pair.method)
        || entries.len() != 3
        || ![original, other, symbol]
            .iter()
            .all(|local| entries.contains(local))
    {
        return Err(identity_error(
            "the three original locals must occupy the exact method-owned Block scope",
        ));
    }
    let local_type = match local_info.info {
        SymbolInfo::Complete(ty) => ty,
        _ => return Err(semantic_error("alias local type is incomplete")),
    };
    let package = pair
        ._store
        .symbols
        .get(pair.method)
        .owner
        .and_then(|owner| pair._store.symbols.get(owner).owner)
        .ok_or_else(|| identity_error("factory method has no canonical source package"))?;
    if !is_canonical_class_type_ref(
        pair._store,
        local_type,
        pair.counter,
        package,
        pair._definitions.no_prefix,
    ) {
        return Err(semantic_error(
            "alias and original locals must retain the same completed Counter type",
        ));
    }
    let source_position = require_source_position(
        pair._source_ast,
        pair.source_id,
        source_declaration,
        "alias ValDef has a foreign source span",
    )?;
    let initializer_position = require_source_position(
        pair._source_ast,
        pair.source_id,
        source_initializer,
        "alias Ident has a foreign source span",
    )?;
    let second = initializers[1];
    let second_initializer_position = require_source_position(
        pair._source_ast,
        pair.source_id,
        second.source_initializer(),
        "second local initializer has a foreign source span",
    )?;
    let first_call_position = require_source_position(
        pair._source_ast,
        pair.source_id,
        block.stats[3],
        "first deferred Counter.set statement has a foreign source span",
    )?;
    if second_initializer_position.span().range().end() > source_position.span().range().start()
        || source_position.span().range().start() >= initializer_position.span().range().start()
        || initializer_position.span().range().end() > first_call_position.span().range().start()
    {
        return Err(source_error(
            "alias must be initialized after the original locals and before later statements",
        ));
    }
    require_matching_source_typed_positions(
        pair._source_ast,
        pair._typed_ast,
        pair.source_id,
        source_declaration,
        typed_declaration,
        "alias ValDef",
    )?;
    require_matching_source_typed_positions(
        pair._source_ast,
        pair._typed_ast,
        pair.source_id,
        source_initializer,
        typed_initializer,
        "alias Ident",
    )?;
    require_matching_source_typed_positions(
        pair._source_ast,
        pair._typed_ast,
        pair.source_id,
        source_type_tree,
        typed_type_tree,
        "alias explicit type tree",
    )?;
    require_matching_source_typed_positions(
        pair._source_ast,
        pair._typed_ast,
        pair.source_id,
        source_pair_first_argument,
        typed_pair_first_argument,
        "first final Pair argument",
    )?;
    let typed_block = pair
        ._source_typed_index
        .get(pair.source_id, pair.source_block)
        .ok_or_else(|| typed_error("original Block has no typed mapping"))?;
    let TreeKind::Block(typed_block_tree) = &pair
        ._typed_ast
        .try_get(typed_block)
        .ok_or_else(|| typed_error("typed Block is stale"))?
        .kind
    else {
        return Err(typed_error("source Block must remain a typed Block"));
    };
    if typed_block_tree.stats.get(2) != Some(&typed_declaration) {
        return Err(typed_error(
            "typed Block must preserve the exact alias declaration at its source position",
        ));
    }
    let (typed_alias_type, typed_annotation_type) = match &pair
        ._typed_ast
        .try_get(typed_declaration)
        .ok_or_else(|| typed_error("typed alias ValDef is stale"))?
        .kind
    {
        TreeKind::ValDef(value)
            if value.rhs == Some(typed_initializer) && value.tpt == typed_type_tree =>
        {
            (
                pair._typed_ast.get(typed_declaration).ty,
                pair._typed_ast
                    .try_get(typed_type_tree)
                    .ok_or_else(|| typed_error("typed alias explicit type tree is stale"))?
                    .ty,
            )
        }
        TreeKind::ValDef(_) => {
            return Err(typed_error(
                "typed alias ValDef must preserve its exact initializer and type-tree edges",
            ));
        }
        _ => return Err(typed_error("typed alias declaration must remain ValDef")),
    };
    if !is_canonical_class_type_ref(
        pair._store,
        typed_alias_type,
        pair.counter,
        package,
        pair._definitions.no_prefix,
    ) || !is_canonical_class_type_ref(
        pair._store,
        typed_annotation_type,
        pair.counter,
        package,
        pair._definitions.no_prefix,
    ) || !matches!(pair._store.types.try_get(pair._typed_ast.get(typed_target).ty),
            Some(Type::TermRef { target: TermRefTarget::Symbol(target_symbol), .. }) if *target_symbol == original)
    {
        return Err(typed_error(
            "typed alias and target must retain the exact canonical Counter/local identities",
        ));
    }
    Ok(ValidatedImmutableLocalReferenceAliasProof {
        initializers: pair,
        source_declaration,
        typed_declaration,
        symbol,
        scope,
        source_initializer,
        typed_initializer,
        source_type_tree,
        typed_type_tree,
        source_target: source_initializer,
        typed_target,
        target: original,
        local_type,
    })
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
        local_receiver_unit_call: None,
        local_receiver_unit_calls: None,
        local_reference_alias: None,
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
