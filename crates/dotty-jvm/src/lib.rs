//! A JVM backend adapter boundary for already-typed compiler semantics.
//!
//! This crate defines and validates a borrowed compilation plan, projects
//! bounded class/method metadata, lowers a small typed expression subset into
//! `dotty-classfile`'s symbolic code builder, and can compose those results
//! into a complete class file when the frontend proves the supported
//! initialization shape. It does not assemble a source frontend, infer
//! identities from names, or mutate the caller's stores. See
//! `docs/jvm-backend.md` for its supported subset and boundary assumptions.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;

use dotty_core::ast::{Tree, TreeKind, Typed, TypedAst};
use dotty_core::definitions::Definitions;
use dotty_core::ids::{SymbolId, TreeId, TypeId};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolFlags, SymbolInfo, SymbolKind};
use dotty_core::types::Type;

mod call_target;
mod descriptor;
mod emission;
mod expression;
mod identity;
mod locals;

/// Maximum number of nested typed expression nodes visited by JVM lowering.
pub const JVM_EXPRESSION_DEPTH_LIMIT: usize = 64;

/// Maximum expanded typed-tree node visits in one JVM lowering call.
///
/// Repeated references to a shared subtree count separately because lowering
/// expands each occurrence into bytecode.
pub const JVM_EXPRESSION_NODE_BUDGET: usize = 4_096;

/// Class-file major version emitted by the current whole-class backend.
pub const JVM_CLASSFILE_MAJOR_VERSION: u16 = 65;

/// JVMS limit for a method's parameter slots, including `this` for instance
/// methods.
pub const JVM_MAX_PARAMETER_SLOTS: usize = 255;

/// A JVM value representation supported by the current projection slice.
///
/// `Int` and `Boolean` have distinct descriptor letters (`I` and `Z`) but
/// both use the JVM's int-like category-one operand representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmType {
    Int,
    Boolean,
}

/// Operand category for the JVM values currently supported here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmValueCategory {
    IntLike,
}

/// A JVM method result. `Void` is only valid as a result, never a parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmReturnType {
    Value(JvmType),
    Void,
}

/// The result of lowering one expression, including its normal fallthrough
/// stack state or the fact that every path terminates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmExpressionFlow {
    /// A value remains on the operand stack at the expression's fallthrough.
    Value {
        /// Semantic primitive represented by the int-like stack value.
        value_type: JvmType,
        /// JVM operand category exposed to parent lowering.
        category: JvmValueCategory,
    },
    /// The expression completes normally and leaves no value on the operand
    /// stack, matching Scala `Unit`.
    Unit,
    /// Every reachable path terminates through a supported explicit return.
    Diverges,
}

/// A standalone lowered expression and its fallthrough contract.
#[derive(Debug)]
pub struct JvmLoweredExpression {
    code: dotty_classfile::bytecode::CodeBuilder,
    flow: JvmExpressionFlow,
}

impl JvmLoweredExpression {
    /// The generated symbolic instructions and labels.
    pub const fn code(&self) -> &dotty_classfile::bytecode::CodeBuilder {
        &self.code
    }

    /// Value, no-value `Unit`, or terminating-flow result for parent lowering.
    pub const fn flow(&self) -> JvmExpressionFlow {
        self.flow
    }

    /// Takes the generated method-local instruction builder.
    pub fn into_code(self) -> dotty_classfile::bytecode::CodeBuilder {
        self.code
    }
}

/// Primitive JVM descriptor and value categories for one method signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmMethodDescriptor {
    descriptor: dotty_classfile::descriptor::MethodDescriptor,
    descriptor_text: String,
    /// Primitive parameters in declaration order.
    parameters: Vec<JvmType>,
    /// Primitive result, or JVM `void` for Scala `Unit`.
    result: JvmReturnType,
}

/// Invocation mode selected by the bounded same-class call-target resolver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmInvocationKind {
    /// Ordinary dynamically dispatched instance invocation.
    Virtual,
}

/// A validated target for a same-class typed instance call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmCallTarget {
    owner_internal_name: String,
    method_name: String,
    descriptor: JvmMethodDescriptor,
    method: SymbolId,
    invocation: JvmInvocationKind,
    result: JvmReturnType,
}

/// A JVM class identity derived from the class symbol and its package owners.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmClassIdentity {
    symbol: SymbolId,
    internal_name: String,
    access_flags: dotty_classfile::access_flags::ClassAccessFlags,
}

/// One canonical parameter's JVM local-variable layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JvmParameterLocal {
    symbol: SymbolId,
    ty: JvmType,
    slot: u16,
    width: u8,
}

/// One ordinary method-local's reserved JVM slot and canonical identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JvmLocalSlot {
    declaration: TreeId<Typed>,
    symbol: SymbolId,
    ty: JvmType,
    slot: u16,
    mutable: bool,
}

/// Deterministic, method-scoped JVM local storage layout.
///
/// Slots are assigned once in typed declaration ID order after the receiver
/// and validated formal parameters. This layout reserves storage only; it
/// does not imply that any local has been initialized.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmLocalLayout {
    method: SymbolId,
    locals: Vec<JvmLocalSlot>,
    by_symbol: std::collections::HashMap<SymbolId, usize>,
    max_locals: u16,
}

/// JVM method identity and parameter layout for an ordinary instance method.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmMethodMetadata {
    symbol: SymbolId,
    name: String,
    descriptor: JvmMethodDescriptor,
    access_flags: dotty_classfile::access_flags::MethodAccessFlags,
    parameters: Vec<JvmParameterLocal>,
}

/// Class identity paired with its selected method metadata in plan order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmClassMetadata {
    identity: JvmClassIdentity,
    methods: Vec<JvmMethodMetadata>,
}

/// Frontend evidence about class initialization for whole-class emission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmClassInitializationPlan {
    /// The frontend proved that an empty `Object.<init>()V` constructor is
    /// equivalent to the source class: no constructor parameters or body,
    /// superclass arguments, fields, or template initializers are present.
    ImplicitNoArg,
    /// Constructor or class initialization is explicit or cannot be proven
    /// safe by the supplied compilation plan.
    Unsupported,
}

/// An owned generated class-file artifact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedClass {
    internal_name: String,
    bytes: Vec<u8>,
}

impl GeneratedClass {
    /// JVM internal name encoded by this artifact.
    pub fn internal_name(&self) -> &str {
        &self.internal_name
    }

    /// Encoded class-file bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Takes ownership of the encoded class-file bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// Why a class symbol is outside the supported top-level class subset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsupportedJvmClassKind {
    Abstract,
    Generic,
    Enum,
    Synthetic,
    Case,
    Sealed,
}

/// A symbol flag with no conservative mapping in this JVM metadata subset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsupportedJvmSymbolFlag {
    Abstract,
    Sealed,
    Case,
    Implicit,
    Given,
    Lazy,
    Mutable,
    Inline,
    Transparent,
    Opaque,
    Extension,
    Static,
    Synthetic,
    JavaDefined,
    Erased,
    Override,
    Enum,
    Exported,
}

/// The name component that failed the bounded Scala/JVM identifier check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmNameKind {
    PackageSegment,
    Class,
    Method,
}

/// Why a class's semantic class information cannot establish a simple class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidJvmClassInfoKind {
    MissingOrUnfilled,
    Error,
    NotClassInfo,
    WrongClassSymbol,
    MissingDeclarationScope,
    WrongDeclarationScopeOwner,
    InvalidDeclaration,
}

/// Borrowed input for validating one or more class plans.
///
/// The semantic store and typed AST remain owned by the caller. Plans retain
/// canonical symbol identities and roots in that exact AST arena.
pub struct JvmCompilationUnit<'a> {
    /// Canonical semantic symbols and types supplied by the frontend.
    pub store: &'a SemanticStore,
    /// Typed trees supplied by the frontend.
    pub typed_ast: &'a TypedAst,
    /// Classes in deterministic frontend order.
    pub classes: Vec<JvmClassPlan>,
}

/// One canonical class and the method bodies selected for backend work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmClassPlan {
    /// Canonical class symbol in [`JvmCompilationUnit::store`].
    pub class: SymbolId,
    /// Methods in deterministic frontend order.
    pub methods: Vec<JvmMethodPlan>,
    /// Evidence that the synthesized minimal constructor preserves source
    /// initialization behavior.
    pub initialization: JvmClassInitializationPlan,
}

/// A method, its explicitly ordered parameter identities, and typed body root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmMethodPlan {
    /// Canonical method symbol in the semantic store.
    pub method: SymbolId,
    /// Parameter symbols in JVM/source order, supplied by the frontend.
    pub parameters: Vec<SymbolId>,
    /// Ordinary local value declarations in ascending typed-tree ID order.
    pub locals: Vec<JvmLocalBinding>,
    /// Root of this method's body in the supplied typed AST arena.
    /// `None` represents a declaration without a body.
    pub body: Option<TreeId<Typed>>,
}

/// Canonical identity binding for one typed local `val` or `var` declaration.
///
/// Bindings belong to exactly one [`JvmMethodPlan`]. `mutable` records the
/// mutability observed by the frontend; validation checks it against the
/// canonical symbol flags. This contract does not assign local slots or emit
/// initialization instructions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JvmLocalBinding {
    /// Exact typed `ValDef` node in the compilation unit's typed arena.
    pub declaration: TreeId<Typed>,
    /// Existing canonical local symbol in the compilation unit's store.
    pub symbol: SymbolId,
    /// Whether the source declaration is mutable (`var`).
    pub mutable: bool,
}

impl JvmMethodPlan {
    /// Local declarations in deterministic typed-tree ID order.
    pub fn locals(&self) -> &[JvmLocalBinding] {
        &self.locals
    }
}

/// A plan failed validation against its caller-owned semantic input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JvmCodegenError {
    /// The class ID is outside the supplied symbol table.
    UnknownClass { class: SymbolId },
    /// The method ID is outside the supplied symbol table.
    UnknownMethod { method: SymbolId },
    /// The parameter ID is outside the supplied symbol table.
    UnknownParameter { parameter: SymbolId },
    /// A local symbol ID is outside the supplied symbol table.
    UnknownLocalSymbol { method: SymbolId, symbol: SymbolId },
    /// A symbol has a kind that is not supported for its plan role.
    UnsupportedSymbolKind {
        symbol: SymbolId,
        expected: SymbolRole,
        found: SymbolKind,
    },
    /// A method or parameter is owned by a symbol other than the planned one.
    WrongOwner {
        symbol: SymbolId,
        expected: SymbolId,
        actual: Option<SymbolId>,
    },
    /// The same canonical method appears more than once in the unit.
    DuplicateMethodPlan { method: SymbolId },
    /// The same canonical class appears more than once in the unit.
    DuplicateClassPlan { class: SymbolId },
    /// The same parameter identity appears more than once in a method plan.
    DuplicateParameter {
        parameter: SymbolId,
        method: SymbolId,
    },
    /// The same typed declaration is bound more than once in the unit.
    DuplicateLocalDeclaration {
        declaration: TreeId<Typed>,
        method: SymbolId,
    },
    /// The same canonical local symbol is bound more than once in the unit.
    DuplicateLocalSymbol { symbol: SymbolId, method: SymbolId },
    /// A local binding names a typed node outside the supplied arena.
    UnknownLocalDeclaration {
        method: SymbolId,
        declaration: TreeId<Typed>,
    },
    /// A local binding's typed node is not an ordinary `ValDef`.
    InvalidLocalDeclaration {
        method: SymbolId,
        declaration: TreeId<Typed>,
    },
    /// A local declaration is not reachable from its planned method body.
    LocalDeclarationOutsideMethod {
        method: SymbolId,
        declaration: TreeId<Typed>,
    },
    /// A method parameter was incorrectly supplied as a local declaration.
    LocalIsMethodParameter { method: SymbolId, symbol: SymbolId },
    /// The local binding order is not deterministic by typed declaration ID.
    UnorderedLocalBindings { method: SymbolId },
    /// The typed declaration and canonical local symbol disagree on type.
    LocalTypeMismatch {
        declaration: TreeId<Typed>,
        symbol: SymbolId,
        declaration_type: TypeId,
        symbol_type: TypeId,
    },
    /// Frontend mutability evidence disagrees with canonical symbol flags.
    LocalMutabilityMismatch {
        declaration: TreeId<Typed>,
        symbol: SymbolId,
        recorded_mutable: bool,
        symbol_mutable: bool,
    },
    /// A local declaration has a modifier outside ordinary `val`/`var`.
    UnsupportedLocalFlag {
        symbol: SymbolId,
        flag: UnsupportedJvmLocalFlag,
    },
    /// A planned local `ValDef` has no initializer expression.
    MissingLocalInitializer {
        method: SymbolId,
        declaration: TreeId<Typed>,
    },
    /// A typed local declaration has no canonical binding in the plan.
    LocalDeclarationNotInPlan {
        method: SymbolId,
        declaration: TreeId<Typed>,
    },
    /// A local initializer does not produce its declared primitive value.
    LocalInitializerFlowMismatch {
        method: SymbolId,
        declaration: TreeId<Typed>,
        symbol: SymbolId,
        expected: JvmType,
        found: JvmExpressionFlow,
    },
    /// An initializer reads its local before the corresponding store.
    UninitializedLocalRead {
        tree: TreeId<Typed>,
        symbol: SymbolId,
    },
    /// A local identifier is not backed by the current method's validated layout.
    MissingLocalBinding {
        tree: TreeId<Typed>,
        symbol: SymbolId,
    },
    /// A typed local identifier targets a symbol that is not an owned local.
    InvalidLocalReference {
        tree: TreeId<Typed>,
        symbol: SymbolId,
    },
    /// A local identifier's completed semantic type disagrees with its allocated slot.
    LocalReferenceTypeMismatch {
        tree: TreeId<Typed>,
        symbol: SymbolId,
        expected: JvmType,
        found: Option<JvmType>,
    },
    /// A method plan does not name a typed body root.
    MissingTypedBody { method: SymbolId },
    /// A method body's typed root does not belong to the supplied arena.
    UnknownBodyRoot {
        method: SymbolId,
        tree: TreeId<Typed>,
    },
    /// A typed tree refers to a missing, reserved, or sentinel type.
    InvalidTreeType {
        tree: TreeId<Typed>,
        ty: TypeId,
        reason: InvalidTypeReason,
    },
    /// A semantic symbol has no completed semantic type.
    MissingSemanticInfo { symbol: SymbolId },
    /// A symbol's completed semantic type is missing, reserved, or a sentinel.
    InvalidSemanticType {
        symbol: SymbolId,
        ty: TypeId,
        reason: InvalidTypeReason,
    },
    /// A symbol carries `SymbolInfo::Error` and therefore has no usable type.
    ErrorSemanticInfo { symbol: SymbolId },
    /// A semantic node is outside a future emission subset.
    ///
    /// Validation does not currently choose an emission subset, so this is
    /// reserved for the code-generation stage that follows this contract.
    UnsupportedSemanticNode {
        tree: TreeId<Typed>,
        node: &'static str,
    },
    /// A typed expression uses a missing, sentinel, or error type.
    InvalidExpressionType {
        tree: TreeId<Typed>,
        ty: TypeId,
        reason: InvalidTypeReason,
    },
    /// The typed node is not yet part of the expression-lowering subset.
    UnsupportedTypedExpression {
        tree: TreeId<Typed>,
        node: &'static str,
    },
    /// A typed block's declared type disagrees with the final expression flow.
    BlockResultTypeMismatch {
        tree: TreeId<Typed>,
        declared: TypeId,
        found: JvmExpressionFlow,
    },
    /// A supported integer/boolean literal disagrees with its typed type.
    LiteralTypeMismatch {
        tree: TreeId<Typed>,
        value: JvmType,
        typed: Option<JvmType>,
    },
    /// A typed identifier is not a symbol reference to one of this method's parameters.
    InvalidParameterReference {
        tree: TreeId<Typed>,
        symbol: Option<SymbolId>,
    },
    /// Apply lowering was requested without a validated compilation-unit context.
    JvmCallContextUnavailable {
        tree: TreeId<Typed>,
        method: SymbolId,
    },
    /// A same-class call occurs in a control-flow branch whose verifier frames
    /// cannot represent the temporary reference receiver.
    JvmCallInUnsupportedControlFlow {
        tree: TreeId<Typed>,
        method: SymbolId,
    },
    /// A typed call's argument count disagrees with the selected JVM descriptor.
    JvmCallArityMismatch {
        tree: TreeId<Typed>,
        method: SymbolId,
        expected: usize,
        found: usize,
    },
    /// A lowered argument does not provide the target parameter's primitive value.
    JvmCallArgumentMismatch {
        tree: TreeId<Typed>,
        argument: TreeId<Typed>,
        method: SymbolId,
        index: usize,
        expected: JvmType,
        found: JvmExpressionFlow,
    },
    /// A typed parameter reference has a different primitive type from its local metadata.
    ParameterReferenceTypeMismatch {
        tree: TreeId<Typed>,
        parameter: SymbolId,
        expected: JvmType,
        found: Option<JvmType>,
    },
    /// An instruction could not be appended to the method's code builder.
    CodeBuilder {
        tree: TreeId<Typed>,
        error: dotty_classfile::bytecode::CodeBuilderError,
    },
    /// A conditional expression does not have a Boolean condition.
    InvalidIfCondition {
        tree: TreeId<Typed>,
        found: Option<JvmType>,
    },
    /// A reachable conditional branch does not match the other branch's value type.
    IfBranchTypeMismatch {
        tree: TreeId<Typed>,
        branch: TreeId<Typed>,
        expected: JvmType,
        found: Option<JvmType>,
    },
    /// An If expression's declared type disagrees with its reachable branch values.
    IfResultTypeMismatch {
        tree: TreeId<Typed>,
        expected: Option<JvmType>,
        found: Option<JvmType>,
    },
    /// A typed `Nothing` branch is not backed by supported explicit return flow.
    UnsupportedBottomFlow { tree: TreeId<Typed> },
    /// A Return with an explicit target is outside the current local-return subset.
    UnsupportedReturnTarget {
        tree: TreeId<Typed>,
        from: TreeId<Typed>,
    },
    /// A return value does not match the method's primitive JVM result.
    ReturnValueTypeMismatch {
        tree: TreeId<Typed>,
        expected: JvmReturnType,
        found: Option<JvmType>,
    },
    /// A method body value does not match its descriptor's return type.
    MethodBodyTypeMismatch {
        method: SymbolId,
        tree: TreeId<Typed>,
        expected: JvmReturnType,
        found: Option<JvmType>,
    },
    /// A normally completing no-value expression is incompatible with a
    /// value-returning method body.
    MethodBodyFlowMismatch {
        method: SymbolId,
        tree: TreeId<Typed>,
        expected: JvmReturnType,
        found: JvmExpressionFlow,
    },
    /// A no-value `Unit` flow was attached to a typed tree with a different
    /// semantic type.
    UnitFlowTypeMismatch { tree: TreeId<Typed>, ty: TypeId },
    /// Recursive typed-tree lowering exceeded its fixed depth budget.
    ExpressionDepthExceeded {
        tree: TreeId<Typed>,
        position: Option<dotty_core::source::SourceSpan>,
        limit: usize,
    },
    /// Lowering expanded too many typed-tree occurrences in one call.
    ExpressionNodeBudgetExceeded { tree: TreeId<Typed>, limit: usize },
    /// A method has an unsupported single-clause or monomorphism shape.
    UnsupportedMethodShape {
        method: SymbolId,
        ty: TypeId,
        kind: UnsupportedMethodShapeKind,
    },
    /// A supported primitive parameter's symbol type disagrees with its
    /// corresponding entry in the method's ordered `MethodType`.
    ParameterTypeMismatch {
        method: SymbolId,
        parameter: SymbolId,
        declared: TypeId,
        symbol_info: TypeId,
    },
    /// Scala `Unit` cannot appear in a JVM parameter list.
    UnitParameter {
        method: SymbolId,
        parameter: SymbolId,
        ty: TypeId,
    },
    /// An unsupported semantic type was used for a method value or result.
    UnsupportedJvmType {
        symbol: SymbolId,
        ty: TypeId,
        kind: UnsupportedJvmTypeKind,
    },
    /// A method descriptor exceeds the JVMS 255 parameter-slot limit.
    TooManyParameterSlots { method: SymbolId, slots: usize },
    /// A local symbol is not present in the method's validated local layout.
    UnknownLocalSlot { method: SymbolId, symbol: SymbolId },
    /// The method's local variables exceed the u16 `max_locals` count.
    TooManyLocalSlots { method: SymbolId },
    /// Supplied method metadata or bindings do not match this validated unit.
    LocalLayoutInputMismatch { method: SymbolId },
    /// A parameter modifier is not represented by this primitive-only slice.
    UnsupportedMethodParameter {
        method: SymbolId,
        index: usize,
        kind: UnsupportedMethodParameterKind,
    },
    /// An internally generated descriptor unexpectedly failed the classfile
    /// crate's parser.
    InvalidGeneratedDescriptor {
        method: SymbolId,
        error: dotty_classfile::descriptor::DescriptorError,
    },
    /// A supplied builtin type identity is malformed for this semantic store.
    InvalidBuiltinDefinition {
        builtin: PrimitiveBuiltin,
        ty: TypeId,
    },
    /// The symbol owner is not part of the supported package-only class path.
    UnsupportedOwnerKind {
        class: SymbolId,
        owner: SymbolId,
        found: SymbolKind,
    },
    /// A declared owner identity is not in the supplied semantic store.
    UnknownOwner { class: SymbolId, owner: SymbolId },
    /// The package owner chain is cyclic or has no valid unnamed root.
    InvalidPackageOwnerChain { class: SymbolId, owner: SymbolId },
    /// A class name or package segment is not a supported simple identifier.
    InvalidJvmName {
        symbol: SymbolId,
        kind: JvmNameKind,
        text: String,
    },
    /// The class form is outside the ordinary, concrete, non-generic subset.
    UnsupportedJvmClass {
        class: SymbolId,
        kind: UnsupportedJvmClassKind,
    },
    /// A class or method has visibility other than public.
    UnsupportedJvmVisibility {
        symbol: SymbolId,
        visibility: dotty_core::symbols::Visibility,
    },
    /// A symbol flag has no mapping in the bounded JVM metadata subset.
    UnsupportedJvmSymbolFlag {
        symbol: SymbolId,
        flag: UnsupportedJvmSymbolFlag,
    },
    /// Completed class info is malformed or cannot establish a simple class.
    InvalidJvmClassInfo {
        class: SymbolId,
        ty: Option<TypeId>,
        kind: InvalidJvmClassInfoKind,
    },
    /// A class declaration scope contains a symbol ID outside the symbol table.
    InvalidClassDeclaration { class: SymbolId, symbol: SymbolId },
    /// Two methods on one class project to the same JVM name and descriptor.
    DuplicateJvmMethodIdentity {
        class: SymbolId,
        method: SymbolId,
        previous: SymbolId,
        name: String,
        descriptor: String,
    },
    /// Two class symbols in one compilation unit project to the same internal name.
    DuplicateJvmClassIdentity {
        class: SymbolId,
        previous: SymbolId,
        internal_name: String,
    },
    /// The selected class symbol has no corresponding class plan in the unit.
    MissingClassPlan { class: SymbolId },
    /// The caller method is not part of the supplied current-class plan.
    CallerMethodNotInJvmPlan { class: SymbolId, method: SymbolId },
    /// A canonical method reference is not selected for JVM emission.
    MethodNotInJvmPlan {
        tree: TreeId<Typed>,
        class: SymbolId,
        method: SymbolId,
    },
    /// A typed call site or its function expression has an unsupported shape.
    UnsupportedJvmCallShape {
        tree: TreeId<Typed>,
        kind: UnsupportedJvmCallShapeKind,
    },
    /// The function tree does not carry a canonical method-symbol reference.
    UnresolvedJvmCallTarget { tree: TreeId<Typed> },
    /// The typed call result disagrees with the selected method descriptor.
    JvmCallResultTypeMismatch {
        tree: TreeId<Typed>,
        method: SymbolId,
        expected: JvmReturnType,
        found: Option<JvmReturnType>,
    },
    /// The class plan cannot prove that synthesizing the minimal constructor is safe.
    UnsupportedConstructor { class: SymbolId },
    /// The class's parent list is not exactly its canonical `Object` type.
    UnsupportedJvmSuperclass {
        class: SymbolId,
        parent: Option<TypeId>,
    },
    /// The class declares a self type outside this emission subset.
    UnsupportedJvmSelfType { class: SymbolId, self_type: TypeId },
    /// A declared field, initializer, nested type, or other member is outside
    /// the single-class emission subset.
    UnsupportedJvmClassMember {
        class: SymbolId,
        member: SymbolId,
        kind: SymbolKind,
    },
    /// A selected method is owned by the class but absent from its declaration scope.
    MethodNotInClassDeclarations { class: SymbolId, method: SymbolId },
    /// ClassFileBuilder rejected staged output; `method` is present when the
    /// failing operation was associated with a selected semantic method.
    ClassFileBuild {
        class: SymbolId,
        method: Option<SymbolId>,
        error: dotty_classfile::class_file_builder::ClassFileBuildError,
    },
    /// The fixed synthesized constructor could not be built symbolically.
    ConstructorCodeBuilder {
        class: SymbolId,
        error: dotty_classfile::bytecode::CodeBuilderError,
    },
}

/// Semantic role required by a compilation plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolRole {
    Class,
    Method,
    Parameter,
    Local,
}

/// Unsupported semantic modifier for an ordinary JVM local binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsupportedJvmLocalFlag {
    Lazy,
    Given,
    Implicit,
    Erased,
    Inline,
}

/// Why a typed semantic slot cannot be consumed as an actual type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidTypeReason {
    MissingOrUnfilled,
    NoType,
    Error,
    NoPrefix,
}

/// Unsupported method-type shape for this descriptor projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsupportedMethodShapeKind {
    Polymorphic,
    NestedCurriedClause,
    NonPlainClause,
    ParameterCountMismatch,
    NotMethodType,
}

/// Why a typed call is outside same-class virtual target resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsupportedJvmCallShapeKind {
    /// The supplied call root is not a term application.
    NotApplication,
    /// The application uses a contextual/implicit argument clause.
    UsingClause,
    /// The function is explicitly type-applied.
    TypeApplication,
    /// The function is itself an application (a curried/nested call).
    NestedApplication,
    /// The function is not a supported typed identifier or selection.
    UnsupportedFunctionTree,
    /// A selected member's receiver is not a `this` tree.
    NonThisReceiver,
    /// A selected member's `this` receiver is not the current class.
    NonCurrentThis,
    /// A selected member uses a qualified `this` expression.
    QualifiedThisReceiver,
    /// The canonical term reference has a prefix outside this call subset.
    UnsupportedReceiverPrefix,
}

/// Unsupported semantic type form in this descriptor projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsupportedJvmTypeKind {
    Alias,
    TypeParameter,
    MatchType,
    RefinedType,
    Union,
    Intersection,
    ScalaObject,
    ReferenceClass,
    NamedReference,
    TermReference,
    NestedMethod,
    Array,
    ByName,
    Repeated,
    Bounds,
    NonPrimitiveConstant,
    Other,
}

/// Primitive definition identity being read from [`Definitions`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrimitiveBuiltin {
    Int,
    Boolean,
    Unit,
}

/// An unsupported method parameter modifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsupportedMethodParameterKind {
    Erased,
    Varargs,
}

impl fmt::Display for JvmCodegenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JVM compilation input is invalid: {self:?}")
    }
}

impl Error for JvmCodegenError {}

impl JvmClassIdentity {
    /// Canonical class symbol retained by this projected identity.
    pub const fn symbol(&self) -> SymbolId {
        self.symbol
    }

    /// JVM internal class name, such as `Answer` or `example/Answer`.
    pub fn internal_name(&self) -> &str {
        &self.internal_name
    }

    /// Conservative class-file access flags for a public ordinary class.
    pub const fn access_flags(&self) -> dotty_classfile::access_flags::ClassAccessFlags {
        self.access_flags
    }
}

impl JvmClassPlan {
    /// Derives this class's JVM internal name from its canonical package-owner
    /// chain and validates its public, concrete, non-generic class shape.
    pub fn identity(&self, store: &SemanticStore) -> Result<JvmClassIdentity, JvmCodegenError> {
        identity::class_identity(store, self.class)
    }

    /// Derives this class and its selected public instance-method metadata.
    pub fn metadata(
        &self,
        store: &SemanticStore,
        definitions: &Definitions,
    ) -> Result<JvmClassMetadata, JvmCodegenError> {
        identity::class_metadata(self, store, definitions)
    }
}

impl JvmClassMetadata {
    /// Class identity, preserving its canonical semantic symbol.
    pub const fn identity(&self) -> &JvmClassIdentity {
        &self.identity
    }

    /// Selected method metadata in the class plan's order.
    pub fn methods(&self) -> &[JvmMethodMetadata] {
        &self.methods
    }
}

impl JvmCallTarget {
    /// JVM internal name of the selected method's owner.
    pub fn owner_internal_name(&self) -> &str {
        &self.owner_internal_name
    }

    /// Unmangled JVM method name projected from the selected canonical symbol.
    pub fn method_name(&self) -> &str {
        &self.method_name
    }

    /// Exact JVM method descriptor projected for the selected symbol.
    pub const fn descriptor(&self) -> &JvmMethodDescriptor {
        &self.descriptor
    }

    /// Canonical selected method identity.
    pub const fn method_symbol(&self) -> SymbolId {
        self.method
    }

    /// Invocation mode for the resolved target.
    pub const fn invocation_kind(&self) -> JvmInvocationKind {
        self.invocation
    }

    /// Result contract, including `Void` for Scala `Unit`.
    pub const fn result(&self) -> JvmReturnType {
        self.result
    }
}

impl JvmMethodMetadata {
    /// Canonical method symbol used to derive this metadata.
    pub const fn symbol(&self) -> SymbolId {
        self.symbol
    }

    /// JVM-safe, unmangled method name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Validated JVM method descriptor projected from semantic types.
    pub const fn descriptor(&self) -> &JvmMethodDescriptor {
        &self.descriptor
    }

    /// Conservative flags for a public ordinary instance method.
    pub const fn access_flags(&self) -> dotty_classfile::access_flags::MethodAccessFlags {
        self.access_flags
    }

    /// Formal parameter identities and local slots in JVM/source order.
    pub fn parameters(&self) -> &[JvmParameterLocal] {
        &self.parameters
    }
}

/// Lowers the supported value-producing leaves from the supplied typed AST.
///
/// Integer and Boolean literals become an integer constant instruction.
/// Parameter identifiers are resolved only through their canonical
/// `TermRefTarget::Symbol` and this method's parameter metadata. Both produce
/// [`JvmValueCategory::IntLike`]. The method does not add a return instruction
/// or any other method epilogue. Unsupported expressions return a typed error.
/// The builder is only updated after successful validation and instruction
/// construction.
pub fn lower_value_expression(
    store: &SemanticStore,
    typed_ast: &TypedAst,
    definitions: &Definitions,
    method: &JvmMethodMetadata,
    tree: TreeId<Typed>,
    code: &mut dotty_classfile::bytecode::CodeBuilder,
) -> Result<JvmValueCategory, JvmCodegenError> {
    expression::lower_value_expression(store, typed_ast, definitions, method, tree, code)
}

/// Lowers one supported typed expression without appending a method epilogue.
///
/// Integer/Boolean leaves, the `Unit` literal, ordered blocks, and nested
/// `If`/local `Return` nodes are supported. `Value` leaves one correctly typed
/// value on the JVM stack; `Unit` completes normally with no stack value;
/// `Diverges` has no reachable fallthrough. Blocks discard unused primitive
/// values with `pop` and stop lowering after a definitely diverging statement.
/// The expression is built into a fresh method-local instruction builder, so
/// an error never exposes partial code. This standalone API has no local
/// binding layout; local `Ident` nodes return [`JvmCodegenError::MissingLocalBinding`].
pub fn lower_expression(
    store: &SemanticStore,
    typed_ast: &TypedAst,
    definitions: &Definitions,
    method: &JvmMethodMetadata,
    tree: TreeId<Typed>,
) -> Result<JvmLoweredExpression, JvmCodegenError> {
    expression::lower_expression(store, typed_ast, definitions, method, tree)
}

/// Lowers a complete method body and adds only the descriptor-appropriate
/// terminal return required by its reachable fallthrough.
///
/// The returned builder is method-local and is discarded on any lowering
/// error. Int and Boolean values use `ireturn`; a normally completing Unit body
/// gets `return`; a body whose paths all terminate gets no additional epilogue.
pub fn lower_method_body(
    store: &SemanticStore,
    typed_ast: &TypedAst,
    definitions: &Definitions,
    method: &JvmMethodMetadata,
    body: TreeId<Typed>,
) -> Result<dotty_classfile::bytecode::CodeBuilder, JvmCodegenError> {
    expression::lower_method_body(store, typed_ast, definitions, method, body)
}

/// Emits one selected ordinary class as a complete JVM class-file artifact.
///
/// The class must have an explicit [`JvmClassPlan`] in `unit`. Its
/// initialization plan must prove an implicit no-argument constructor is
/// equivalent to the source class. The output is pinned to class-file version
/// 65 and contains only the planned public instance methods, a minimal public
/// constructor, and no TASTy or other Scala ABI metadata.
pub fn emit_class(
    unit: &JvmCompilationUnit<'_>,
    class_symbol: SymbolId,
    definitions: &Definitions,
) -> Result<GeneratedClass, JvmCodegenError> {
    emission::emit_class(unit, class_symbol, definitions)
}

impl JvmParameterLocal {
    /// Canonical parameter symbol supplied by the class plan.
    pub const fn symbol(self) -> SymbolId {
        self.symbol
    }

    /// JVM type currently assigned to this local.
    pub const fn ty(self) -> JvmType {
        self.ty
    }

    /// First JVM local slot occupied by this parameter.
    pub const fn slot(self) -> u16 {
        self.slot
    }

    /// Number of JVM local slots occupied by this parameter.
    pub const fn width(self) -> u8 {
        self.width
    }
}

impl JvmLocalSlot {
    /// Exact typed `ValDef` that supplied this canonical binding.
    pub const fn declaration(self) -> TreeId<Typed> {
        self.declaration
    }

    /// Canonical local symbol used as the identity for this slot.
    pub const fn symbol(self) -> SymbolId {
        self.symbol
    }

    /// Completed primitive type stored in this category-one slot.
    pub const fn ty(self) -> JvmType {
        self.ty
    }

    /// JVM local-variable slot reserved for this declaration.
    pub const fn slot(self) -> u16 {
        self.slot
    }

    /// Whether the source declaration is mutable.
    pub const fn is_mutable(self) -> bool {
        self.mutable
    }
}

impl JvmLocalLayout {
    /// Method whose receiver, parameters, and locals define this layout.
    pub const fn method(&self) -> SymbolId {
        self.method
    }

    /// Locals in deterministic typed declaration ID order.
    pub fn locals(&self) -> &[JvmLocalSlot] {
        &self.locals
    }

    /// JVMS `max_locals` count, including receiver and formal parameters.
    pub const fn max_locals(&self) -> u16 {
        self.max_locals
    }

    /// Finds a local only by its canonical symbol identity.
    pub fn local(&self, symbol: SymbolId) -> Result<&JvmLocalSlot, JvmCodegenError> {
        self.by_symbol
            .get(&symbol)
            .and_then(|index| self.locals.get(*index))
            .ok_or(JvmCodegenError::UnknownLocalSlot {
                method: self.method,
                symbol,
            })
    }
}

impl JvmMethodPlan {
    /// Extracts this method's primitive-only JVM signature from canonical
    /// semantic types and this plan's explicitly ordered parameter symbols.
    ///
    /// The class, method, and parameter symbol kinds and ownership are checked
    /// before reading completed type information. The signature accepts one
    /// ordinary monomorphic parameter clause containing only `Int` and
    /// `Boolean`, with `Int`, `Boolean`, or result-only `Unit` as its result.
    pub fn descriptor(
        &self,
        store: &SemanticStore,
        owner: SymbolId,
        definitions: &Definitions,
    ) -> Result<JvmMethodDescriptor, JvmCodegenError> {
        descriptor::method_descriptor(self, store, owner, definitions)
    }
}

impl JvmCompilationUnit<'_> {
    /// Projects all planned class identities and method metadata in plan order.
    ///
    /// Duplicate JVM class names and duplicate `(method name, descriptor)`
    /// pairs are rejected before the metadata is returned.
    pub fn metadata(
        &self,
        definitions: &Definitions,
    ) -> Result<Vec<JvmClassMetadata>, JvmCodegenError> {
        identity::unit_metadata(self, definitions)
    }

    /// Builds the deterministic JVM local layout for one planned method.
    ///
    /// The complete unit is validated first, then locals are projected from
    /// their canonical semantic types and allocated in typed declaration ID
    /// order. Reservation does not establish definite initialization.
    pub fn local_layout(
        &self,
        class: SymbolId,
        method: SymbolId,
        definitions: &Definitions,
    ) -> Result<JvmLocalLayout, JvmCodegenError> {
        locals::method_local_layout(self, class, method, definitions)
    }

    /// Allocates locals for supplied metadata and bindings from this unit.
    ///
    /// The metadata and binding slice must exactly match a method plan in this
    /// unit. They are checked against the current semantic store and typed AST
    /// before any layout is returned.
    pub fn local_layout_for(
        &self,
        method: &JvmMethodMetadata,
        bindings: &[JvmLocalBinding],
        definitions: &Definitions,
    ) -> Result<JvmLocalLayout, JvmCodegenError> {
        locals::method_local_layout_for(self, method, bindings, definitions)
    }

    /// Resolves a typed ordinary call to a planned same-class virtual target.
    ///
    /// `call` must identify the complete typed `Apply` tree so the resolver can
    /// reject contextual application and validate the call result. Its
    /// function must be a canonical method reference in an `Ident` or a
    /// `Select` whose receiver is the current class's `this`. Resolution uses
    /// the selected method `SymbolId` and this unit's explicit emission plan;
    /// it does not evaluate arguments, load a receiver, or emit instructions.
    pub fn resolve_same_class_call(
        &self,
        caller_class: SymbolId,
        caller_method: SymbolId,
        call: TreeId<Typed>,
        definitions: &Definitions,
    ) -> Result<JvmCallTarget, JvmCodegenError> {
        call_target::resolve_same_class_call(self, caller_class, caller_method, call, definitions)
    }

    /// Lowers the receiver for a validated same-class instance call.
    ///
    /// The complete `Apply` tree is first resolved through
    /// [`Self::resolve_same_class_call`]. This proves an implicit identifier
    /// names a planned instance method, while an explicit selection must use
    /// the current class's unqualified `this`. On success, the returned
    /// method-local builder contains exactly one `aload(0)` instruction. The
    /// caller must append argument evaluation after this builder so the
    /// receiver is pushed before call arguments. No call or arguments are
    /// emitted here, and an error returns no partial builder.
    pub fn lower_same_class_call_receiver(
        &self,
        caller_class: SymbolId,
        caller_method: SymbolId,
        call: TreeId<Typed>,
        definitions: &Definitions,
    ) -> Result<dotty_classfile::bytecode::CodeBuilder, JvmCodegenError> {
        let class_plan = self
            .classes
            .iter()
            .find(|plan| plan.class == caller_class)
            .ok_or(JvmCodegenError::MissingClassPlan {
                class: caller_class,
            })?;
        let class_metadata = class_plan.metadata(self.store, definitions)?;
        let caller_metadata = class_metadata
            .methods()
            .iter()
            .find(|method| method.symbol() == caller_method)
            .ok_or(JvmCodegenError::CallerMethodNotInJvmPlan {
                class: caller_class,
                method: caller_method,
            })?;
        let target =
            self.resolve_same_class_call(caller_class, caller_method, call, definitions)?;
        expression::lower_current_receiver(
            self.typed_ast,
            &class_metadata,
            caller_metadata,
            call,
            &target,
        )
    }

    /// Validates the plan without mutating or copying semantic state.
    ///
    /// The first error follows class order, method order, parameter order,
    /// then typed-arena allocation order. Symbol identity is always the
    /// canonical [`SymbolId`]; equal names never substitute for identity.
    pub fn validate(&self) -> Result<(), JvmCodegenError> {
        let mut seen_classes = HashSet::new();
        let mut seen_methods = HashSet::new();
        let mut seen_local_declarations = HashSet::new();
        let mut seen_local_symbols = HashSet::new();

        for class_plan in &self.classes {
            if !seen_classes.insert(class_plan.class) {
                return Err(JvmCodegenError::DuplicateClassPlan {
                    class: class_plan.class,
                });
            }

            let class = self
                .store
                .symbols
                .contains(class_plan.class)
                .then(|| self.store.symbols.get(class_plan.class))
                .ok_or(JvmCodegenError::UnknownClass {
                    class: class_plan.class,
                })?;
            if class.kind != SymbolKind::Class {
                return Err(JvmCodegenError::UnsupportedSymbolKind {
                    symbol: class_plan.class,
                    expected: SymbolRole::Class,
                    found: class.kind,
                });
            }
            self.validate_symbol_type(class_plan.class)?;

            for method_plan in &class_plan.methods {
                if !seen_methods.insert(method_plan.method) {
                    return Err(JvmCodegenError::DuplicateMethodPlan {
                        method: method_plan.method,
                    });
                }

                let method = self
                    .store
                    .symbols
                    .contains(method_plan.method)
                    .then(|| self.store.symbols.get(method_plan.method))
                    .ok_or(JvmCodegenError::UnknownMethod {
                        method: method_plan.method,
                    })?;
                if method.kind != SymbolKind::Method {
                    return Err(JvmCodegenError::UnsupportedSymbolKind {
                        symbol: method_plan.method,
                        expected: SymbolRole::Method,
                        found: method.kind,
                    });
                }
                if method.owner != Some(class_plan.class) {
                    return Err(JvmCodegenError::WrongOwner {
                        symbol: method_plan.method,
                        expected: class_plan.class,
                        actual: method.owner,
                    });
                }
                self.validate_symbol_type(method_plan.method)?;

                let mut seen_parameters = HashSet::new();
                for parameter_id in &method_plan.parameters {
                    if !seen_parameters.insert(*parameter_id) {
                        return Err(JvmCodegenError::DuplicateParameter {
                            parameter: *parameter_id,
                            method: method_plan.method,
                        });
                    }
                    let parameter = self
                        .store
                        .symbols
                        .contains(*parameter_id)
                        .then(|| self.store.symbols.get(*parameter_id))
                        .ok_or(JvmCodegenError::UnknownParameter {
                            parameter: *parameter_id,
                        })?;
                    if parameter.kind != SymbolKind::Parameter {
                        return Err(JvmCodegenError::UnsupportedSymbolKind {
                            symbol: *parameter_id,
                            expected: SymbolRole::Parameter,
                            found: parameter.kind,
                        });
                    }
                    if parameter.owner != Some(method_plan.method) {
                        return Err(JvmCodegenError::WrongOwner {
                            symbol: *parameter_id,
                            expected: method_plan.method,
                            actual: parameter.owner,
                        });
                    }
                    self.validate_symbol_type(*parameter_id)?;
                }

                let body = method_plan.body.ok_or(JvmCodegenError::MissingTypedBody {
                    method: method_plan.method,
                })?;
                if self.typed_ast.try_get(body).is_none() {
                    return Err(JvmCodegenError::UnknownBodyRoot {
                        method: method_plan.method,
                        tree: body,
                    });
                }

                let mut previous_declaration_index = None;
                for binding in &method_plan.locals {
                    if !seen_local_declarations.insert(binding.declaration) {
                        return Err(JvmCodegenError::DuplicateLocalDeclaration {
                            declaration: binding.declaration,
                            method: method_plan.method,
                        });
                    }
                    if !seen_local_symbols.insert(binding.symbol) {
                        return Err(JvmCodegenError::DuplicateLocalSymbol {
                            symbol: binding.symbol,
                            method: method_plan.method,
                        });
                    }
                    if previous_declaration_index
                        .is_some_and(|previous| previous >= binding.declaration.index())
                    {
                        return Err(JvmCodegenError::UnorderedLocalBindings {
                            method: method_plan.method,
                        });
                    }
                    previous_declaration_index = Some(binding.declaration.index());

                    let declaration = self.typed_ast.try_get(binding.declaration).ok_or(
                        JvmCodegenError::UnknownLocalDeclaration {
                            method: method_plan.method,
                            declaration: binding.declaration,
                        },
                    )?;
                    if !matches!(declaration.kind, TreeKind::ValDef(_)) {
                        return Err(JvmCodegenError::InvalidLocalDeclaration {
                            method: method_plan.method,
                            declaration: binding.declaration,
                        });
                    }
                    if !typed_tree_contains(self.typed_ast, body, binding.declaration) {
                        return Err(JvmCodegenError::LocalDeclarationOutsideMethod {
                            method: method_plan.method,
                            declaration: binding.declaration,
                        });
                    }
                    validate_tree_type(self.store, binding.declaration, declaration)?;

                    if method_plan.parameters.contains(&binding.symbol) {
                        return Err(JvmCodegenError::LocalIsMethodParameter {
                            method: method_plan.method,
                            symbol: binding.symbol,
                        });
                    }
                    let local = self
                        .store
                        .symbols
                        .contains(binding.symbol)
                        .then(|| self.store.symbols.get(binding.symbol))
                        .ok_or(JvmCodegenError::UnknownLocalSymbol {
                            method: method_plan.method,
                            symbol: binding.symbol,
                        })?;
                    if local.kind != SymbolKind::Local {
                        return Err(JvmCodegenError::UnsupportedSymbolKind {
                            symbol: binding.symbol,
                            expected: SymbolRole::Local,
                            found: local.kind,
                        });
                    }
                    if local.owner != Some(method_plan.method) {
                        return Err(JvmCodegenError::WrongOwner {
                            symbol: binding.symbol,
                            expected: method_plan.method,
                            actual: local.owner,
                        });
                    }
                    for (flag, required_flag) in [
                        (UnsupportedJvmLocalFlag::Lazy, SymbolFlags::LAZY),
                        (UnsupportedJvmLocalFlag::Given, SymbolFlags::GIVEN),
                        (UnsupportedJvmLocalFlag::Implicit, SymbolFlags::IMPLICIT),
                        (UnsupportedJvmLocalFlag::Erased, SymbolFlags::ERASED),
                        (UnsupportedJvmLocalFlag::Inline, SymbolFlags::INLINE),
                    ] {
                        if local.flags.contains(required_flag) {
                            return Err(JvmCodegenError::UnsupportedLocalFlag {
                                symbol: binding.symbol,
                                flag,
                            });
                        }
                    }
                    self.validate_symbol_type(binding.symbol)?;
                    let symbol_type = match self.store.symbols.info(binding.symbol) {
                        SymbolInfo::Complete(ty) => *ty,
                        SymbolInfo::Missing | SymbolInfo::Deferred(_) => {
                            return Err(JvmCodegenError::MissingSemanticInfo {
                                symbol: binding.symbol,
                            });
                        }
                        SymbolInfo::Error => {
                            return Err(JvmCodegenError::ErrorSemanticInfo {
                                symbol: binding.symbol,
                            });
                        }
                    };
                    if declaration.ty != symbol_type {
                        return Err(JvmCodegenError::LocalTypeMismatch {
                            declaration: binding.declaration,
                            symbol: binding.symbol,
                            declaration_type: declaration.ty,
                            symbol_type,
                        });
                    }
                    let symbol_mutable = local.flags.contains(SymbolFlags::MUTABLE);
                    if binding.mutable != symbol_mutable {
                        return Err(JvmCodegenError::LocalMutabilityMismatch {
                            declaration: binding.declaration,
                            symbol: binding.symbol,
                            recorded_mutable: binding.mutable,
                            symbol_mutable,
                        });
                    }
                }
            }
        }

        for (tree_id, tree) in self.typed_ast.iter() {
            validate_tree_type(self.store, tree_id, tree)?;
        }
        Ok(())
    }

    fn validate_symbol_type(&self, symbol: SymbolId) -> Result<(), JvmCodegenError> {
        match self.store.symbols.info(symbol) {
            SymbolInfo::Missing | SymbolInfo::Deferred(_) => {
                Err(JvmCodegenError::MissingSemanticInfo { symbol })
            }
            SymbolInfo::Error => Err(JvmCodegenError::ErrorSemanticInfo { symbol }),
            SymbolInfo::Complete(ty) => validate_type(self.store, *ty).map_err(|reason| {
                JvmCodegenError::InvalidSemanticType {
                    symbol,
                    ty: *ty,
                    reason,
                }
            }),
        }
    }
}

fn validate_tree_type(
    store: &SemanticStore,
    tree_id: TreeId<Typed>,
    tree: &Tree<Typed>,
) -> Result<(), JvmCodegenError> {
    validate_type(store, tree.ty).map_err(|reason| JvmCodegenError::InvalidTreeType {
        tree: tree_id,
        ty: tree.ty,
        reason,
    })
}

fn validate_type(store: &SemanticStore, ty: TypeId) -> Result<(), InvalidTypeReason> {
    match store.types.try_get(ty) {
        None => Err(InvalidTypeReason::MissingOrUnfilled),
        Some(Type::NoType) => Err(InvalidTypeReason::NoType),
        Some(Type::Error(_)) => Err(InvalidTypeReason::Error),
        Some(Type::NoPrefix) => Err(InvalidTypeReason::NoPrefix),
        Some(_) => Ok(()),
    }
}

fn typed_tree_contains(ast: &TypedAst, root: TreeId<Typed>, sought: TreeId<Typed>) -> bool {
    let mut pending = vec![root];
    let mut visited = HashSet::new();
    while let Some(id) = pending.pop() {
        if id == sought {
            return true;
        }
        if !visited.insert(id) {
            continue;
        }
        let Some(tree) = ast.try_get(id) else {
            continue;
        };
        match &tree.kind {
            TreeKind::Ident(_)
            | TreeKind::This(_)
            | TreeKind::Literal(_)
            | TreeKind::TypeTree(_) => {}
            TreeKind::Select(node) => pending.push(node.qualifier),
            TreeKind::Super(node) => pending.push(node.qual),
            TreeKind::Apply(node) => {
                pending.push(node.function);
                pending.extend(node.args.iter().copied());
            }
            TreeKind::TypeApply(node) => {
                pending.push(node.function);
                pending.extend(node.args.iter().copied());
            }
            TreeKind::New(node) => pending.push(node.tpt),
            TreeKind::Typed(node) => pending.extend([node.expr, node.tpt]),
            TreeKind::NamedArg(node) => pending.push(node.arg),
            TreeKind::Assign(node) => pending.extend([node.lhs, node.rhs]),
            TreeKind::Block(node) => {
                pending.extend(node.stats.iter().copied());
                pending.push(node.expr);
            }
            TreeKind::If(node) => pending.extend([node.cond, node.then_branch, node.else_branch]),
            TreeKind::Match(node) => {
                pending.push(node.selector);
                pending.extend(node.cases.iter().copied());
            }
            TreeKind::CaseDef(node) => {
                pending.extend([node.pattern, node.body]);
                pending.extend(node.guard);
            }
            TreeKind::Return(node) => pending.extend(node.expr.into_iter().chain(node.from)),
            TreeKind::While(node) => pending.extend([node.cond, node.body]),
            TreeKind::Try(node) => {
                pending.push(node.expr);
                pending.extend(node.cases.iter().copied());
                pending.extend(node.finalizer);
            }
            TreeKind::Closure(node) => {
                pending.extend(node.env.iter().copied());
                pending.push(node.method);
                pending.extend(node.tpt);
            }
            TreeKind::ValDef(node) => {
                pending.push(node.tpt);
                pending.extend(node.rhs);
            }
            TreeKind::DefDef(node) => {
                pending.extend(node.type_params.iter().copied());
                pending.extend(node.value_param_clauses.iter().flatten().copied());
                pending.push(node.tpt);
                pending.extend(node.rhs);
            }
            TreeKind::TypeDef(node) => pending.push(node.rhs),
            TreeKind::Template(node) => {
                pending.push(node.constructor);
                pending.extend(node.parents.iter().copied());
                pending.extend(node.self_val);
                pending.extend(node.body.iter().copied());
            }
            TreeKind::PackageDef(node) => {
                pending.push(node.name);
                pending.extend(node.stats.iter().copied());
            }
            TreeKind::Import(node) => {
                pending.push(node.expr);
                pending.extend(node.selectors.iter().flat_map(|selector| selector.renamed));
                pending.extend(node.selectors.iter().flat_map(|selector| selector.bound));
            }
            TreeKind::Export(node) => {
                pending.push(node.expr);
                pending.extend(node.selectors.iter().flat_map(|selector| selector.renamed));
                pending.extend(node.selectors.iter().flat_map(|selector| selector.bound));
            }
            TreeKind::SingletonTypeTree(node) => pending.push(node.reference),
            TreeKind::AppliedTypeTree(node) => {
                pending.push(node.tpt);
                pending.extend(node.args.iter().copied());
            }
            TreeKind::RefinedTypeTree(node) => {
                pending.push(node.tpt);
                pending.extend(node.refinements.iter().copied());
            }
            TreeKind::LambdaTypeTree(node) => {
                pending.extend(node.type_params.iter().copied());
                pending.push(node.body);
            }
            TreeKind::MatchTypeTree(node) => {
                pending.extend(node.bound);
                pending.push(node.selector);
                pending.extend(node.cases.iter().copied());
            }
            TreeKind::ByNameTypeTree(node) => pending.push(node.result),
            TreeKind::TypeBoundsTree(node) => {
                pending.extend(node.low);
                pending.extend(node.high);
                pending.extend(node.alias);
            }
            TreeKind::Bind(node) => pending.push(node.body),
            TreeKind::Alternative(node) => pending.extend(node.alternatives.iter().copied()),
            TreeKind::UnApply(node) => {
                pending.push(node.function);
                pending.extend(node.implicits.iter().copied());
                pending.extend(node.patterns.iter().copied());
            }
            TreeKind::Annotated(node) => pending.extend([node.expr, node.annotation]),
            TreeKind::Quote(node) => {
                pending.push(node.body);
                pending.extend(node.tags.iter().copied());
            }
            TreeKind::Splice(node) => pending.push(node.expr),
            TreeKind::QuotePattern(node) => {
                pending.extend(node.bindings.iter().copied());
                pending.extend([node.body, node.quotes]);
            }
            TreeKind::SplicePattern(node) => {
                pending.push(node.body);
                pending.extend(node.type_args.iter().copied());
                pending.extend(node.args.iter().copied());
            }
            TreeKind::Inlined(node) => {
                pending.extend(node.bindings.iter().copied());
                pending.push(node.expansion);
            }
            TreeKind::PhaseSpecific(never) => match *never {},
        }
    }
    false
}

#[cfg(test)]
mod tests;
