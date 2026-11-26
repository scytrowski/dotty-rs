//! A JVM backend adapter boundary for already-typed compiler semantics.
//!
//! This crate defines and validates a borrowed compilation plan, projects
//! bounded class/method metadata, lowers a small typed expression subset into
//! `dotty-classfile`'s symbolic code builder, and can compose those results
//! into a complete class file when the frontend proves the supported
//! initialization shape. It does not assemble a source frontend, infer
//! identities from names, or mutate the caller's stores. See
//! `docs/jvm-backend.md` for its supported subset and boundary assumptions.

#![deny(missing_docs)]
#![doc = include_str!("guide.md")]

use std::collections::HashSet;
use std::error::Error;
use std::fmt;

use dotty_core::ast::{AstArena, Tree, TreeKind, Typed, TypedAst, Untyped};
use dotty_core::definitions::Definitions;
use dotty_core::ids::{SourceId, SymbolId, TreeId, TypeId};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolFlags, SymbolInfo, SymbolKind};
use dotty_core::types::Type;

mod boolean_ops;
mod call_target;
mod constructor;
mod constructor_call;
mod constructor_initializers;
mod context_result;
mod defaults;
mod descriptor;
pub use constructor::{
    JvmConstructorAccessorBinding, JvmConstructorArgumentBinding, JvmConstructorIssue,
    JvmConstructorMetadata, JvmDirectSuperConstructorPlan, JvmOrderedReferenceConstructorFormals,
    JvmPrimaryConstructorPlan,
};
pub use constructor_initializers::{
    JvmConstructorAccessorInitializer, JvmConstructorFieldInitializer,
    JvmConstructorFieldInitializerOrigin, JvmConstructorFieldInitializerPlan,
    JvmConstructorInitializerInstruction,
};
mod emission;
pub use emission::PreparedJvmEmission;
mod expression;
mod identity;
mod locals;
mod primitive_ops;
mod reference;
mod reference_identity;

pub use boolean_ops::{
    JvmBooleanIntrinsic, JvmBooleanIntrinsicError, JvmBooleanRhsEvaluation,
    resolve_boolean_intrinsic,
};
pub use primitive_ops::{JvmIntIntrinsic, JvmIntIntrinsicError, resolve_int_intrinsic};
pub use reference_identity::{
    JvmReferenceIdentityError, JvmReferenceIdentityOperator, resolve_reference_identity_operator,
};

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

/// A JVM value representation supported by the current metadata projection.
///
/// `Int` and `Boolean` have distinct descriptor letters (`I` and `Z`) but
/// both use the JVM's int-like category-one operand representation. `Float`
/// is a separate category-one value with descriptor `F`. `Long` and `Double`
/// are distinct category-two values with descriptors `J` and `D`. The current expression
/// subset supports Long literals, parameters, ordinary locals, assignments,
/// returns, and private instance fields with primary-constructor
/// initialization. Checked planned calls, Long constructor arguments,
/// Long-valued `If` joins, and Long local updates across supported `While`
/// loops are also supported. Long arithmetic, comparisons, static Long calls,
/// and category-two stack manipulation remain unsupported. References retain
/// their exact planned class identity.
/// Unit-aware method and field metadata can encode those identities;
/// store-only projectors remain narrower.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmType {
    /// Scala Int lowered as a JVM int value.
    Int,
    /// Scala Boolean lowered using the JVM int-like verifier category.
    Boolean,
    /// A category-one JVM `float` value.
    Float,
    /// A category-two JVM `long` value.
    Long,
    /// A category-two JVM `double` value.
    Double,
    /// A category-one reference to one exact class in a validated compilation unit.
    Reference(SymbolId),
}

/// Operand category for the JVM values currently represented here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmValueCategory {
    /// JVM verifier category shared by supported int and Boolean values.
    IntLike,
    /// A JVM category-one `float` value, distinct from int-like values.
    Float,
    /// A JVM category-two `long` value.
    Long,
    /// A JVM category-two `double` value, distinct from `long`.
    Double,
    /// A JVM object reference; it occupies one operand-stack/local slot.
    Reference,
}

/// A JVM method result. `Void` is only valid as a result, never a parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmReturnType {
    /// Method returns the specified supported JVM value type.
    Value(JvmType),
    /// Method returns no JVM value, corresponding to supported Unit results.
    Void,
}

/// The result of lowering one expression, including its normal fallthrough
/// stack state or the fact that every path terminates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmExpressionFlow {
    /// A value remains on the operand stack at the expression's fallthrough.
    Value {
        /// Exact primitive or planned class identity represented on the stack.
        value_type: JvmType,
        /// JVM operand category exposed to parent lowering.
        category: JvmValueCategory,
    },
    /// A JVM verifier `Null` value whose source expression has an approved
    /// reference context, but which has not acquired a class identity.
    Null,
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

/// Validated JVM descriptor and value categories for one method signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmMethodDescriptor {
    descriptor: dotty_classfile::descriptor::MethodDescriptor,
    descriptor_text: String,
    /// Primitive and planned-reference parameters in declaration order.
    parameters: Vec<JvmType>,
    /// Primitive or planned-reference result, or JVM `void` for Scala `Unit`.
    result: JvmReturnType,
}

/// Invocation mode selected by the bounded planned-call resolvers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmInvocationKind {
    /// Ordinary dynamically dispatched instance invocation.
    Virtual,
    /// Statically selected direct-parent call from an unqualified `super`.
    SpecialSuper,
}

/// JVM invocation mode for a validated object constructor call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmConstructorInvocationKind {
    /// Constructor calls use `invokespecial` after object allocation.
    Special,
}

/// A validated target for a typed instance call on a planned class receiver.
/// Its symbolic owner is the class declaring the selected method; the
/// receiver can be that class or its validated direct subclass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmCallTarget {
    prior_application: Option<TreeId<Typed>>,
    owner_internal_name: String,
    method_name: String,
    descriptor: JvmMethodDescriptor,
    method: SymbolId,
    invocation: JvmInvocationKind,
    result: JvmReturnType,
    receiver_tree: Option<TreeId<Typed>>,
    receiver_class: SymbolId,
}

/// Exact validated constructor target for a typed `new C(args)` application.
///
/// Resolution is read-only. It supplies the canonical class and constructor
/// identities, descriptor, access flags, and argument types needed by a later
/// allocation emitter; it does not create bytecode or constant-pool entries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmConstructorCallTarget {
    owner_class: SymbolId,
    owner_internal_name: String,
    constructor: SymbolId,
    descriptor: JvmMethodDescriptor,
    access_flags: dotty_classfile::access_flags::MethodAccessFlags,
    argument_types: Vec<JvmType>,
    result_type: JvmType,
}

/// A JVM class identity derived from the class symbol and its package owners.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmClassIdentity {
    symbol: SymbolId,
    internal_name: String,
    access_flags: dotty_classfile::access_flags::ClassAccessFlags,
}

/// Validated direct-superclass relation for one canonical planned class.
///
/// The source symbols remain authoritative inside `dotty-jvm`; the two names
/// are checked projections for class-file consumers. Constructed only by a
/// validated [`JvmClassHierarchy`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmSuperclassPlan {
    class: SymbolId,
    parent: SymbolId,
    class_internal_name: String,
    parent_internal_name: String,
}

/// Read-only, deterministic direct-superclass proofs for one compilation unit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JvmClassHierarchy {
    superclasses: Vec<JvmSuperclassPlan>,
}

/// A validated superclass edge projected only to JVM internal names.
///
/// This is the Scala-independent view intended for a later class-file
/// verifier. It contains no source symbols or mutable registry reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmInternalNameHierarchyEdge {
    child_internal_name: String,
    parent_internal_name: String,
}

/// A checked JVM reference representation for one exact source sibling union.
///
/// The original semantic union and expected source type remain available for
/// auditing; this proof does not rewrite either type or enable bytecode
/// lowering by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JvmSiblingUnionProjection {
    source_type: TypeId,
    expected_type: TypeId,
    source_arms: [SymbolId; 2],
    representation: JvmType,
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
    pattern: Option<(dotty_core::ScopeId, TreeId<Typed>)>,
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
    context_result: Option<context_result::ContextResult>,
    symbol: SymbolId,
    name: String,
    descriptor: JvmMethodDescriptor,
    access_flags: dotty_classfile::access_flags::MethodAccessFlags,
    parameters: Vec<JvmParameterLocal>,
}

/// Read-only effective method identity for a planned receiver class.
///
/// Inherited entries retain the canonical symbol and declaring class from the
/// parent. An overriding entry points at the child's distinct declaration;
/// this metadata never copies a method body into a child class plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmMethodDispatchMetadata {
    receiver_class: SymbolId,
    declaring_class: SymbolId,
    method: SymbolId,
    name: String,
    descriptor: JvmMethodDescriptor,
}

/// Getter or setter role of an effective JVM accessor method.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JvmAccessorDispatchRole {
    /// Public generated getter for a Scala `val` or `var`.
    Getter,
    /// Public generated setter for a Scala `var`.
    Setter,
}

/// Canonical source identity behind a JVM method-shaped declaration.
///
/// Ordinary source methods retain their `Method` symbol; generated accessors
/// retain the source `Field` and whether the entry is its getter or setter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmBinaryMethodOrigin {
    /// A declared Scala method.
    Method(SymbolId),
    /// A generated method derived from a source field.
    Accessor {
        /// Canonical source field that owns this accessor.
        field: SymbolId,
        /// Whether this is the getter or mutable setter entrypoint.
        role: JvmAccessorDispatchRole,
    },
}

/// Which accessor-related inherited binary declaration collision occurred.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmAccessorCollisionKind {
    /// A child source method occupies an inherited generated accessor slot.
    SourceMethodRedefinesInheritedAccessor,
    /// A child generated getter or setter occupies an inherited source method slot.
    GeneratedAccessorRedefinesInheritedMethod,
    /// A child generated getter or setter occupies an inherited final method slot.
    GeneratedAccessorRedefinesInheritedFinalMethod,
    /// A child generated getter or setter occupies an inherited accessor slot.
    GeneratedAccessorRedefinesInheritedAccessor,
    /// An accessor-related declaration has the inherited JVM parameter shape
    /// but an incompatible result descriptor.
    IncompatibleInheritedAccessorResult,
}

/// Read-only effective dispatch identity for one generated accessor method.
///
/// Accessors retain their canonical source field as provenance instead of
/// inventing a source method symbol. Conflicting declarations remain separate
/// entries so later validation can inspect the collision without treating it
/// as an override.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmAccessorDispatchMetadata {
    receiver_class: SymbolId,
    declaring_class: SymbolId,
    field: SymbolId,
    role: JvmAccessorDispatchRole,
    name: String,
    descriptor: JvmMethodDescriptor,
    access_flags: dotty_classfile::access_flags::MethodAccessFlags,
}

/// Class identity paired with declared method metadata and an effective
/// dispatch view. `methods` remains in class-plan order and excludes inherited
/// declarations; unit-aware projections populate `dispatch_methods`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmClassMetadata {
    identity: JvmClassIdentity,
    superclass: Option<JvmSuperclassPlan>,
    fields: Vec<JvmFieldMetadata>,
    constructor: Option<JvmConstructorMetadata>,
    accessor_bindings: Vec<JvmConstructorAccessorBinding>,
    accessors: Vec<JvmAccessorMetadata>,
    methods: Vec<JvmMethodMetadata>,
    dispatch_methods: Vec<JvmMethodDispatchMetadata>,
    dispatch_accessors: Vec<JvmAccessorDispatchMetadata>,
}

/// A frontend-issued binding from one canonical field to its exact typed
/// initializer root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JvmFieldPlan {
    /// Canonical class-owned field symbol in the compilation unit store.
    pub field: SymbolId,
    /// Source access form retained by the frontend when it builds this plan.
    pub source_visibility: JvmFieldSourceVisibility,
    /// Exact typed RHS that the frontend associated with `field`.
    pub initializer: Option<TreeId<Typed>>,
}

/// Source visibility distinctions needed by the JVM field subset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmFieldSourceVisibility {
    /// Unqualified `private`, which retains the existing emitted field path.
    PlainPrivate,
    /// Object-private `private[this]`, which shares `Visibility::Private` in
    /// the semantic store but has a distinct Scala accessor/storage contract.
    PrivateThis,
    /// An ordinary class-body declaration with no explicit visibility
    /// modifier, paired with checked source and typed-RHS provenance.
    PlainPublic(JvmPublicBodyFieldSourceProof),
}

/// Checked immutable provenance for an ordinary public class-body field.
///
/// Construct this record from the exact source AST, semantic index, typed
/// index, and store used by the compilation unit. Its private fields prevent a
/// caller from changing the canonical declaration or RHS after validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JvmPublicBodyFieldSourceProof {
    class: SymbolId,
    field: SymbolId,
    source: SourceId,
    declaration: TreeId<Untyped>,
    template_position: usize,
    mutable: bool,
    rhs: TreeId<Untyped>,
    initializer: TreeId<Typed>,
}

impl JvmPublicBodyFieldSourceProof {
    /// Validates that `declaration` is the canonical unqualified class-body
    /// field and that `initializer` is the typed image of its exact RHS.
    #[allow(clippy::too_many_arguments)]
    pub fn from_source(
        class: SymbolId,
        field: SymbolId,
        source: SourceId,
        declaration: TreeId<Untyped>,
        initializer: TreeId<Typed>,
        ast: &AstArena<Untyped>,
        index: &dotty_core::SourceSemanticIndex,
        source_typed_index: &dotty_core::SourceTypedIndex,
        typed_ast: &dotty_core::ast::TypedAst,
        store: &SemanticStore,
    ) -> Result<Self, JvmCodegenError> {
        let invalid = || JvmCodegenError::InvalidPublicBodyFieldSourceProof { field };
        let class_symbol = store
            .symbols
            .contains(class)
            .then(|| store.symbols.get(class))
            .ok_or(JvmCodegenError::UnknownClass { class })?;
        if class_symbol.kind != SymbolKind::Class {
            return Err(JvmCodegenError::UnsupportedSymbolKind {
                symbol: class,
                expected: SymbolRole::Class,
                found: class_symbol.kind,
            });
        }
        let field_symbol = store
            .symbols
            .contains(field)
            .then(|| store.symbols.get(field))
            .ok_or(JvmCodegenError::UnknownField { field })?;
        if field_symbol.kind != SymbolKind::Field {
            return Err(JvmCodegenError::UnsupportedSymbolKind {
                symbol: field,
                expected: SymbolRole::Field,
                found: field_symbol.kind,
            });
        }
        if field_symbol.origin != dotty_core::SymbolOrigin::Source(source) {
            return Err(invalid());
        }
        if field_symbol.owner != Some(class) {
            return Err(JvmCodegenError::WrongOwner {
                symbol: field,
                expected: class,
                actual: field_symbol.owner,
            });
        }
        if field_symbol.visibility != dotty_core::symbols::Visibility::Public {
            return Err(JvmCodegenError::UnsupportedJvmVisibility {
                symbol: field,
                visibility: field_symbol.visibility,
            });
        }
        if index.symbol_at(source, declaration) != Some(field)
            || index.definition_of(field)
                != Some(dotty_core::SourceDefinition::Canonical {
                    source,
                    tree: declaration,
                })
        {
            return Err(invalid());
        }
        let TreeKind::ValDef(definition) = &ast.try_get(declaration).ok_or_else(invalid)?.kind
        else {
            return Err(invalid());
        };
        let Some(rhs) = definition.rhs else {
            return Err(JvmCodegenError::MissingFieldInitializer { field });
        };
        let source_rhs = ast.try_get(rhs).ok_or_else(invalid)?;
        let typed_rhs = typed_ast
            .try_get(initializer)
            .ok_or(JvmCodegenError::UnknownFieldInitializer { field, initializer })?;
        if definition.metadata.visibility.is_some()
            || source_typed_index.get(source, rhs) != Some(initializer)
            || source_rhs.position.is_none()
            || source_rhs.position != typed_rhs.position
        {
            return Err(invalid());
        }
        let mutable = definition
            .metadata
            .modifiers
            .contains(&dotty_core::ast::Modifier::Var);
        if field_symbol.flags.contains(SymbolFlags::MUTABLE) != mutable {
            return Err(invalid());
        }
        let Some(dotty_core::SourceDefinition::Canonical {
            source: class_source,
            tree: class_tree,
        }) = index.definition_of(class)
        else {
            return Err(invalid());
        };
        if class_symbol.origin != dotty_core::SymbolOrigin::Source(source) {
            return Err(invalid());
        }
        let TreeKind::TypeDef(class_definition) =
            &ast.try_get(class_tree).ok_or_else(invalid)?.kind
        else {
            return Err(invalid());
        };
        let TreeKind::Template(template) =
            &ast.try_get(class_definition.rhs).ok_or_else(invalid)?.kind
        else {
            return Err(invalid());
        };
        if class_source != source || !template.body.contains(&declaration) {
            return Err(invalid());
        }
        let template_position = template
            .body
            .iter()
            .position(|tree| *tree == declaration)
            .ok_or_else(invalid)?;
        Ok(Self {
            class,
            field,
            source,
            declaration,
            template_position,
            mutable,
            rhs,
            initializer,
        })
    }

    /// Canonical owning class symbol.
    pub const fn class(self) -> SymbolId {
        self.class
    }

    /// Canonical source field symbol.
    pub const fn field(self) -> SymbolId {
        self.field
    }

    /// Source compilation unit containing the declaration.
    pub const fn source(self) -> SourceId {
        self.source
    }

    /// Exact source `ValDef` declaration tree.
    pub const fn declaration(self) -> TreeId<Untyped> {
        self.declaration
    }

    /// Zero-based source order within the containing template body.
    pub const fn template_position(self) -> usize {
        self.template_position
    }

    /// Whether the source declaration is `var` rather than `val`.
    pub const fn mutable(self) -> bool {
        self.mutable
    }

    /// Exact source RHS tree.
    pub const fn rhs(self) -> TreeId<Untyped> {
        self.rhs
    }

    /// Exact typed initializer root bound to the source RHS.
    pub const fn initializer(self) -> TreeId<Typed> {
        self.initializer
    }
}

/// Validated JVM metadata for one private primitive or exact-reference instance field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmFieldMetadata {
    symbol: SymbolId,
    name: String,
    descriptor: String,
    value_type: JvmType,
    access_flags: dotty_classfile::access_flags::FieldAccessFlags,
    owner: SymbolId,
    initializer: TreeId<Typed>,
}

/// Audited source origin of one JVM accessor projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmAccessorOrigin {
    /// A class-owned field derived from the exact primary-constructor formal.
    ConstructorParameter(JvmConstructorAccessorBinding),
    /// An ordinary class-body field with its exact source and typed RHS proof.
    ClassBodyField(JvmPublicBodyFieldSourceProof),
}

/// Checked JVM-only backing-field and method signatures for one source
/// accessor. The origin retains the distinct proof required by constructor
/// parameters and class-body fields. This projection does not synthesize
/// source symbols or emit class-file members.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmAccessorMetadata {
    origin: JvmAccessorOrigin,
    value_type: JvmType,
    field_name: String,
    field_descriptor: String,
    field_access_flags: dotty_classfile::access_flags::FieldAccessFlags,
    getter: JvmAccessorMethodMetadata,
    setter: Option<JvmAccessorMethodMetadata>,
}

/// JVM-only signature metadata for an accessor entrypoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmAccessorMethodMetadata {
    name: String,
    descriptor: JvmMethodDescriptor,
    access_flags: dotty_classfile::access_flags::MethodAccessFlags,
}

/// Frontend evidence about class initialization for whole-class emission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JvmClassInitializationPlan {
    /// Validated source constructor proof used to emit its exact primary
    /// constructor and source-ordered checked field initializers.
    PrimaryConstructor(JvmPrimaryConstructorPlan),
    /// The frontend proved that a synthesized public no-argument constructor
    /// is equivalent to the source class: no constructor parameters/body,
    /// parent arguments, or non-field template initializers are present.
    /// Planned field initializers remain separately source-ordered, and the
    /// unit hierarchy supplies the validated direct-parent call.
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
    /// This validation path rejects the abstract declaration marker.
    Abstract,
    /// This validation path rejects the generic class requiring unsupported type parameters.
    Generic,
    /// This validation path rejects the enum declaration marker.
    Enum,
    /// This validation path rejects the synthetic declaration marker.
    Synthetic,
    /// This validation path rejects the case declaration marker.
    Case,
    /// This validation path rejects the sealed declaration marker.
    Sealed,
}

/// Why a canonical class parent cannot be accepted in the bounded unit graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmSuperclassIssue {
    /// The class lacks the single supported direct superclass.
    MissingOrMultipleParents,
    /// The parent type is not a canonical class reference in this session.
    NonCanonicalParentType,
    /// The parent symbol is not an accepted superclass declaration kind.
    WrongParentKind,
    /// The class names itself as its superclass.
    SelfParent,
    /// The source superclass is absent from the complete compilation plan.
    ParentNotPlanned,
    /// The superclass has a shape outside this backend's supported subset.
    ParentUnsupportedShape,
    /// The selected superclass forbids extension.
    ParentIsFinal,
    /// The selected superclass is not publicly accessible.
    ParentNotPublic,
    /// The superclass has an unsupported explicit self type.
    ParentHasSelfType,
    /// The planned superclass graph contains a cycle.
    InheritanceCycle,
    /// The superclass chain exceeds the configured traversal bound.
    InheritanceTooDeep,
    /// The hierarchy cannot reach the required canonical Object root.
    MissingObjectRoot,
    /// The required superclass constructor is absent or inaccessible.
    ParentConstructorUnavailable,
}

/// A symbol flag with no conservative mapping in this JVM metadata subset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsupportedJvmSymbolFlag {
    /// This validation path rejects the abstract declaration marker.
    Abstract,
    /// This validation path rejects the sealed declaration marker.
    Sealed,
    /// This validation path rejects the case declaration marker.
    Case,
    /// This validation path rejects the implicit declaration marker.
    Implicit,
    /// This validation path rejects the given declaration marker.
    Given,
    /// This validation path rejects the lazy declaration marker.
    Lazy,
    /// This validation path rejects the mutable symbol flag.
    Mutable,
    /// This validation path rejects the inline declaration marker.
    Inline,
    /// This validation path rejects the transparent declaration marker.
    Transparent,
    /// This validation path rejects the opaque declaration marker.
    Opaque,
    /// This validation path rejects the extension declaration marker.
    Extension,
    /// This validation path rejects the static declaration marker.
    Static,
    /// This validation path rejects the synthetic declaration marker.
    Synthetic,
    /// This validation path rejects the Java-defined symbol flag.
    JavaDefined,
    /// This validation path rejects the erased declaration marker.
    Erased,
    /// This validation path rejects the override declaration marker.
    Override,
    /// This validation path rejects the enum declaration marker.
    Enum,
    /// This validation path rejects the export-generated symbol flag.
    Exported,
}

/// The name component that failed the bounded Scala/JVM identifier check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmNameKind {
    /// One source package segment being encoded as a JVM name.
    PackageSegment,
    /// Class name being encoded as a JVM internal name.
    Class,
    /// Field name being encoded for class-file metadata.
    Field,
    /// Method name being encoded for class-file metadata.
    Method,
}

/// Why a class's semantic class information cannot establish a simple class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidJvmClassInfoKind {
    /// The class signature is missing or its type slot is unfilled.
    MissingOrUnfilled,
    /// Class signature completion recorded an error.
    Error,
    /// The completed signature is not a class-info type.
    NotClassInfo,
    /// The class-info payload identifies a different class.
    WrongClassSymbol,
    /// The class declaration scope is absent from this session.
    MissingDeclarationScope,
    /// The declaration scope belongs to another owner.
    WrongDeclarationScopeOwner,
    /// A class member fails the canonical declaration contract.
    InvalidDeclaration,
}

/// Why accessor call-site metadata no longer agrees with its source field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmAccessorCallMetadataIssue {
    /// More than one accessor metadata entry claims the same field.
    DuplicateField,
    /// The accessor or field belongs to a different class.
    OwnerMismatch,
    /// The receiver class has no planned metadata.
    ReceiverClassMissing,
    /// No effective getter is recorded for this field.
    EffectiveGetterMissing,
    /// The effective getter differs from the selected accessor.
    EffectiveGetterMismatch,
    /// No effective setter is recorded for this field.
    EffectiveSetterMissing,
    /// The effective setter differs from the selected accessor.
    EffectiveSetterMismatch,
    /// The required canonical source-to-typed binding is absent.
    BindingMissing,
    /// The body proof identifies a different backing field.
    BodyFieldProofMismatch,
    /// The field and accessor value types disagree.
    ValueTypeMismatch,
    /// Backing-field flags disagree with the accessor contract.
    BackingFieldFlagsMismatch,
    /// The getter signature does not match the field type.
    GetterSignatureMismatch,
    /// The setter signature does not accept the field type and return Unit.
    SetterSignatureMismatch,
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
    /// Fields in deterministic frontend source declaration order.
    pub fields: Vec<JvmFieldPlan>,
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
    /// Exact typed `ValDef` or pattern `Bind` node in the compilation unit's typed arena.
    pub declaration: TreeId<Typed>,
    /// Existing canonical local symbol in the compilation unit's store.
    pub symbol: SymbolId,
    /// Whether the source declaration is mutable (`var`).
    pub mutable: bool,
    literal_given: Option<defaults::LiteralGiven>,
    pattern: Option<(dotty_core::ScopeId, TreeId<Typed>)>,
}

impl JvmLocalBinding {
    /// Records an ordinary eager local value or variable.
    pub const fn ordinary(declaration: TreeId<Typed>, symbol: SymbolId, mutable: bool) -> Self {
        Self {
            declaration,
            symbol,
            mutable,
            literal_given: None,
            pattern: None,
        }
    }
    /// Records an immutable pattern binder with its canonical case scope and
    /// exact typed CaseDef. The case must bind this declaration as its root
    /// pattern; all IDs must belong to the unit's one semantic session.
    pub const fn pattern(
        declaration: TreeId<Typed>,
        symbol: SymbolId,
        scope: dotty_core::ScopeId,
        case: TreeId<Typed>,
    ) -> Self {
        Self {
            declaration,
            symbol,
            mutable: false,
            literal_given: None,
            pattern: Some((scope, case)),
        }
    }

    /// Canonical scope and typed CaseDef for a pattern binder, or None for an
    /// ordinary local. Validation checks the symbol, scope and pattern edge.
    pub const fn pattern_case(self) -> Option<(dotty_core::ScopeId, TreeId<Typed>)> {
        self.pattern
    }
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
    /// The contextual getter, its copied formals or its body disagree with the source plan.
    InvalidContextualDefaultGetter {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// A planned constant local given disagrees with its source or typed declaration.
    InvalidLiteralGiven {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// A primary constructor plan or its source evidence is inconsistent.
    InvalidPrimaryConstructor {
        /// Constructor declaration identity being inspected in the supplied session.
        constructor: SymbolId,
        /// Specific structural or semantic reason for the enclosing rejection.
        issue: JvmConstructorIssue,
    },
    /// A constructor application has a supported shape but mismatched class identities.
    JvmConstructorCallClassMismatch {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Required declaration identity in the caller’s symbol table.
        expected: SymbolId,
        /// Observed declaration identity in the caller’s symbol table. None records a missing
        /// binding or owner.
        found: Option<SymbolId>,
    },
    /// The selected constructor belongs to a different class than the typed `new` target.
    JvmConstructorCallWrongOwner {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Constructor declaration identity being inspected in the supplied session.
        constructor: SymbolId,
        /// Required declaration identity in the caller’s symbol table.
        expected: SymbolId,
        /// Observed declaration identity in the caller’s symbol table. None records a missing
        /// binding or owner.
        actual: Option<SymbolId>,
    },
    /// The selected canonical constructor is not present in approved constructor metadata.
    JvmConstructorCallNotApproved {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Constructor declaration identity being inspected in the supplied session.
        constructor: SymbolId,
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
    },
    /// Constructor access is not available from the caller class in this subset.
    InaccessibleJvmConstructor {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Constructor declaration identity being inspected in the supplied session.
        constructor: SymbolId,
        /// Enclosing class of the call site.
        caller_class: SymbolId,
        /// Class owning the member being checked.
        owner_class: SymbolId,
    },
    /// The call does not supply the exact constructor descriptor arity.
    JvmConstructorCallArityMismatch {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Constructor declaration identity being inspected in the supplied session.
        constructor: SymbolId,
        /// Required argument or parameter count.
        expected: usize,
        /// Observed argument or parameter count.
        found: usize,
    },
    /// One argument's completed primitive type differs from its constructor slot.
    JvmConstructorCallArgumentTypeMismatch {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Exact typed argument tree involved in the failed check.
        argument: TreeId<Typed>,
        /// Required JVM value or control-flow representation.
        expected: JvmType,
        /// Observed JVM value or control-flow representation. None means no representation was
        /// available.
        found: Option<JvmType>,
    },
    /// The typed application result does not identify the allocated class.
    JvmConstructorCallResultTypeMismatch {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Required declaration identity in the caller’s symbol table.
        expected: SymbolId,
        /// Observed declaration identity in the caller’s symbol table. None records a missing
        /// binding or owner.
        found: Option<SymbolId>,
    },
    /// A constructor argument is outside the supported branch-free primitive subset.
    UnsupportedJvmConstructorArgument {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Exact typed argument tree involved in the failed check.
        argument: TreeId<Typed>,
    },
    /// Constructor application lowering was requested without a planned unit.
    JvmConstructorCallContextUnavailable {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// A method containing an allocation also requires branch-aware verifier frames.
    JvmObjectCreationInUnsupportedControlFlow {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// The selected constructor is not a proven plain no-argument or planned primary call.
    UnsupportedJvmConstructorCall {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Structural or semantic category of the item.
        kind: JvmConstructorCallShapeError,
    },
    /// The class ID is outside the supplied symbol table.
    UnknownClass {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
    },
    /// The method ID is outside the supplied symbol table.
    UnknownMethod {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// A method was requested from a class plan that does not contain it.
    MethodNotInJvmPlanForBody {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// The field ID is outside the supplied symbol table.
    UnknownField {
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
    },
    /// A public-body field plan lacks matching immutable source evidence.
    InvalidPublicBodyFieldSourceProof {
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
    },
    /// Public-body field plans do not retain source template order.
    OutOfOrderPublicBodyFieldPlan {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
    },
    /// Field plans do not retain initializer source order across visibility forms.
    OutOfOrderFieldPlan {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
    },
    /// A planned field initializer has no source position for order validation.
    FieldInitializerSourcePositionUnavailable {
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
    },
    /// Planned field initializers come from different source files.
    FieldInitializersFromDifferentSources {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
    },
    /// The parameter ID is outside the supplied symbol table.
    UnknownParameter {
        /// Parameter identity in the canonical session symbol table.
        parameter: SymbolId,
    },
    /// A local symbol ID is outside the supplied symbol table.
    UnknownLocalSymbol {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// A symbol has a kind that is not supported for its plan role.
    UnsupportedSymbolKind {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Required declaration role in this compilation plan.
        expected: SymbolRole,
        /// Observed declaration kind, independent of flags or visibility.
        found: SymbolKind,
    },
    /// A method or parameter is owned by a symbol other than the planned one.
    WrongOwner {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Required declaration identity in the caller’s symbol table.
        expected: SymbolId,
        /// Observed declaration identity in the caller’s symbol table. None records a missing
        /// binding or owner.
        actual: Option<SymbolId>,
    },
    /// The same canonical method appears more than once in the unit.
    DuplicateMethodPlan {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// The same canonical field appears more than once in a class plan.
    DuplicateFieldPlan {
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
    },
    /// The same canonical class appears more than once in the unit.
    DuplicateClassPlan {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
    },
    /// The same parameter identity appears more than once in a method plan.
    DuplicateParameter {
        /// Parameter identity in the canonical session symbol table.
        parameter: SymbolId,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// The same typed declaration is bound more than once in the unit.
    DuplicateLocalDeclaration {
        /// Declaration identity or syntax node involved in this check.
        declaration: TreeId<Typed>,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// The same canonical local symbol is bound more than once in the unit.
    DuplicateLocalSymbol {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// A local binding names a typed node outside the supplied arena.
    UnknownLocalDeclaration {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Declaration identity or syntax node involved in this check.
        declaration: TreeId<Typed>,
    },
    /// A local binding's typed node is not an ordinary `ValDef`.
    InvalidLocalDeclaration {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Declaration identity or syntax node involved in this check.
        declaration: TreeId<Typed>,
    },
    /// A local declaration is not reachable from its planned method body.
    LocalDeclarationOutsideMethod {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Declaration identity or syntax node involved in this check.
        declaration: TreeId<Typed>,
    },
    /// A method parameter was incorrectly supplied as a local declaration.
    LocalIsMethodParameter {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// The local binding order is not deterministic by typed declaration ID.
    UnorderedLocalBindings {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// The typed declaration and canonical local symbol disagree on type.
    LocalTypeMismatch {
        /// Declaration identity or syntax node involved in this check.
        declaration: TreeId<Typed>,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Type of the declaration being checked.
        declaration_type: TypeId,
        /// Semantic type recorded for the canonical symbol.
        symbol_type: TypeId,
    },
    /// Frontend mutability evidence disagrees with canonical symbol flags.
    LocalMutabilityMismatch {
        /// Declaration identity or syntax node involved in this check.
        declaration: TreeId<Typed>,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Mutability recorded in the previously validated metadata.
        recorded_mutable: bool,
        /// Whether the symbol carries the mutable flag.
        symbol_mutable: bool,
    },
    /// A local declaration has a modifier outside ordinary `val`/`var`.
    UnsupportedLocalFlag {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Unsupported flag that caused the rejection.
        flag: UnsupportedJvmLocalFlag,
    },
    /// A planned local `ValDef` has no initializer expression.
    MissingLocalInitializer {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Declaration identity or syntax node involved in this check.
        declaration: TreeId<Typed>,
    },
    /// A typed local declaration has no canonical binding in the plan.
    LocalDeclarationNotInPlan {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Declaration identity or syntax node involved in this check.
        declaration: TreeId<Typed>,
    },
    /// A local initializer does not produce its declared primitive value.
    LocalInitializerFlowMismatch {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Declaration identity or syntax node involved in this check.
        declaration: TreeId<Typed>,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Required JVM value or control-flow representation.
        expected: JvmType,
        /// Observed JVM value or control-flow representation.
        found: JvmExpressionFlow,
    },
    /// A typed assignment does not have the canonical Unit result type.
    AssignmentResultTypeMismatch {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
    },
    /// The assignment target is not an identifier for an owned planned local.
    InvalidAssignmentTarget {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Assignment target tree.
        lhs: TreeId<Typed>,
    },
    /// The assignment target is an ordinary local that is not mutable.
    ImmutableLocalAssignment {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// The assignment target has not been initialized on every reachable path.
    UninitializedLocalAssignment {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// The right hand side does not produce the target's primitive value type.
    AssignmentValueFlowMismatch {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Required JVM value or control-flow representation.
        expected: JvmType,
        /// Observed JVM value or control-flow representation.
        found: JvmExpressionFlow,
    },
    /// The assignment target is an immutable current-instance field.
    ImmutableFieldAssignment {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
    },
    /// Semantic mutability and validated JVM field metadata disagree.
    JvmFieldMutabilityMismatch {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
        /// Whether the symbol carries the mutable flag.
        symbol_mutable: bool,
        /// Whether the source metadata records a mutable declaration.
        metadata_mutable: bool,
    },
    /// An initializer reads its local before the corresponding store.
    UninitializedLocalRead {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// A local identifier is not backed by the current method's validated layout.
    MissingLocalBinding {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// A typed local identifier targets a symbol that is not an owned local.
    InvalidLocalReference {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// A local identifier's completed semantic type disagrees with its allocated slot.
    LocalReferenceTypeMismatch {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Required JVM value or control-flow representation.
        expected: JvmType,
        /// Observed JVM value or control-flow representation. None means no representation was
        /// available.
        found: Option<JvmType>,
    },
    /// A method plan does not name a typed body root.
    MissingTypedBody {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// A planned typed initializer is outside the supplied arena.
    UnknownFieldInitializer {
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
        /// Typed initializer root in the canonical session arena.
        initializer: TreeId<Typed>,
    },
    /// A field identifier is not present in the selected class field plan.
    FieldNotInJvmPlan {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
    },
    /// A field read was attempted without a validated class and method plan.
    JvmFieldReadContextUnavailable {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// A typed field read has a missing target symbol.
    UnknownJvmFieldReference {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
    },
    /// A field selection does not use the current class's unqualified `this`.
    UnsupportedJvmFieldReceiver {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
    },
    /// Validated constructor-accessor ABI metadata disagrees with the source field.
    InvalidJvmAccessorCallMetadata {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
        /// Semantic owner of the declaration; ownership is distinct from lookup scope.
        owner: SymbolId,
        /// Specific structural or semantic reason for the enclosing rejection.
        issue: JvmAccessorCallMetadataIssue,
    },
    /// A field read is outside the currently modeled CFG stack flow.
    JvmFieldReadInUnsupportedControlFlow {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
    },
    /// A field write is outside the currently modeled CFG stack flow.
    JvmFieldWriteInUnsupportedControlFlow {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
    },
    /// A field write was attempted without a validated class and method plan.
    JvmFieldWriteContextUnavailable {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// The typed field reference prefix does not name the current instance.
    InvalidJvmFieldReferencePrefix {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
    },
    /// A field initializer's `TermRef` points outside the semantic symbol table.
    UnknownFieldInitializerSymbol {
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
        /// Typed initializer root in the canonical session arena.
        initializer: TreeId<Typed>,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// The declared field has no typed initializer.
    MissingFieldInitializer {
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
    },
    /// The canonical class declaration scope does not contain this field.
    FieldNotInClassScope {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
    },
    /// The canonical field type and its typed initializer disagree.
    FieldInitializerTypeMismatch {
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
        /// Typed initializer root in the canonical session arena.
        initializer: TreeId<Typed>,
        /// Required JVM value or control-flow representation.
        expected: JvmType,
        /// Observed JVM value or control-flow representation. None means no representation was
        /// available.
        found: Option<JvmType>,
    },
    /// A reference field initializer is outside the exact parameter-copy or bounded direct-new forms.
    UnsupportedJvmReferenceFieldInitializer {
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
        /// Typed initializer root in the canonical session arena.
        initializer: TreeId<Typed>,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: Option<SymbolId>,
    },
    /// Constructor initializer lowering requires a validated primary constructor plan.
    ConstructorInitializerContextUnavailable {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
    },
    /// The checked accessor metadata no longer matches its exact constructor
    /// formal layout while building executable constructor stores.
    InvalidJvmAccessorInitializer {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
        /// Parameter identity in the canonical session symbol table.
        parameter: SymbolId,
        /// Specific structural or semantic reason for the enclosing rejection.
        issue: JvmAccessorInitializerIssue,
    },
    /// A field initializer reads a field before its source-ordered store.
    FieldInitializerReadsUninitializedField {
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Field referenced by the initializer or accessor body.
        referenced_field: SymbolId,
    },
    /// A typed initializer tree is outside the supported literal/reference/intrinsic subset.
    UnsupportedConstructorFieldInitializer {
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Syntax-node category encountered at the failure site.
        node: &'static str,
    },
    /// The source field modifier has no supported JVM field mapping.
    UnsupportedJvmFieldFlag {
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
        /// Unsupported flag that caused the rejection.
        flag: UnsupportedJvmFieldFlag,
    },
    /// Source visibility has the same canonical visibility but another ABI form.
    UnsupportedJvmFieldSourceVisibility {
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
        /// Source or semantic visibility of the declaration.
        visibility: JvmFieldSourceVisibility,
    },
    /// Two distinct planned fields have the same class-file field name.
    DuplicateJvmFieldName {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
        /// Previously selected declaration with the conflicting identity.
        previous: SymbolId,
        /// Recorded name spelling used by the enclosing declaration or validation rule.
        name: String,
    },
    /// Actual class-file field emission is outside this metadata-only increment.
    FieldEmissionUnsupported {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
    },
    /// Field metadata needs a typed AST to verify its planned RHS root.
    FieldTypedAstRequired {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
    },
    /// A method body's typed root does not belong to the supplied arena.
    UnknownBodyRoot {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
    },
    /// A context-function result has unsupported captures or inconsistent declaration links.
    InvalidContextFunctionResult {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
    },
    /// A typed tree refers to a missing, reserved, or sentinel type.
    InvalidTreeType {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
        /// Detailed reason for rejecting this input or dependency.
        reason: InvalidTypeReason,
    },
    /// A semantic symbol has no completed semantic type.
    MissingSemanticInfo {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// A symbol's completed semantic type is missing, reserved, or a sentinel.
    InvalidSemanticType {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
        /// Detailed reason for rejecting this input or dependency.
        reason: InvalidTypeReason,
    },
    /// A symbol carries `SymbolInfo::Error` and therefore has no usable type.
    ErrorSemanticInfo {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// A semantic node is outside a future emission subset.
    ///
    /// Validation does not currently choose an emission subset, so this is
    /// reserved for the code-generation stage that follows this contract.
    UnsupportedSemanticNode {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Syntax-node category encountered at the failure site.
        node: &'static str,
    },
    /// A typed expression uses a missing, sentinel, or error type.
    InvalidExpressionType {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
        /// Detailed reason for rejecting this input or dependency.
        reason: InvalidTypeReason,
    },
    /// A reference type is outside the bounded, planned-class subset.
    UnsupportedJvmReferenceType {
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
        /// Structural or semantic category of the item.
        kind: UnsupportedJvmReferenceTypeKind,
    },
    /// A requested reference projection names a missing or sentinel type slot.
    InvalidReferenceType {
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
        /// Detailed reason for rejecting this input or dependency.
        reason: InvalidTypeReason,
    },
    /// A reference target is not one of this compilation unit's class plans.
    ReferenceClassNotInJvmPlan {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
    },
    /// Two reference values identify different canonical classes.
    ReferenceTypeMismatch {
        /// Required declaration identity in the caller’s symbol table.
        expected: SymbolId,
        /// Observed declaration identity in the caller’s symbol table.
        found: SymbolId,
    },
    /// A source union cannot be represented by the supplied checked expected class.
    UnsupportedSiblingUnionProjection {
        /// Source-level semantic type before JVM lowering.
        source_type: TypeId,
        /// Type required by the enclosing conformance check.
        expected_type: TypeId,
    },
    /// A reference-local operation is outside the supported straight-line subset.
    ReferenceLocalInstructionUnsupported {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
    },
    /// Reserved for compatibility; validated mutable planned-class locals
    /// support initialized reads and straight-line assignments.
    UnsupportedMutableReferenceLocal {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// Reference local operations require reference-aware CFG analysis.
    ReferenceLocalInUnsupportedControlFlow {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// Reference method descriptors are deliberately outside this subset.
    ReferenceMethodDescriptorUnsupported {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Value type recorded by the enclosing validation rule.
        ty: JvmType,
    },
    /// Legacy error retained for callers that construct an unsupported descriptor directly.
    ReferenceMethodReturnUnsupported {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Value type recorded by the enclosing validation rule.
        ty: JvmType,
    },
    /// The typed node is not yet part of the expression-lowering subset.
    UnsupportedTypedExpression {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Syntax-node category encountered at the failure site.
        node: &'static str,
    },
    /// A typed block's declared type disagrees with the final expression flow.
    BlockResultTypeMismatch {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Explicitly declared type used by the check.
        declared: TypeId,
        /// Observed JVM value or control-flow representation.
        found: JvmExpressionFlow,
    },
    /// A supported integer/boolean literal disagrees with its typed type.
    LiteralTypeMismatch {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// JVM value type produced by the inspected expression.
        value: JvmType,
        /// JVM value type obtained from the typed tree, when available.
        typed: Option<JvmType>,
    },
    /// A typed identifier is not a symbol reference to one of this method's parameters.
    InvalidParameterReference {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: Option<SymbolId>,
    },
    /// A method-parameter `TermRef` carries a prefix other than canonical `NoPrefix`.
    InvalidParameterReferencePrefix {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Parameter identity in the canonical session symbol table.
        parameter: SymbolId,
        /// Semantic prefix through which the member is selected.
        prefix: TypeId,
    },
    /// A reference parameter needs a validated compilation-unit class plan.
    ReferenceParameterRequiresPlan {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Parameter identity in the canonical session symbol table.
        parameter: SymbolId,
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
    },
    /// A typed `this` does not identify the current planned instance class.
    InvalidCurrentThisReference {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Required declaration identity in the caller’s symbol table.
        expected: SymbolId,
        /// Observed declaration identity in the caller’s symbol table. None records a missing
        /// binding or owner.
        found: Option<SymbolId>,
    },
    /// A qualified `this` value is outside the current expression subset.
    QualifiedThisValueUnsupported {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
    },
    /// A reference value was requested without validated current-class metadata.
    JvmReferenceValueContextUnavailable {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// Apply lowering was requested without a validated compilation-unit context.
    JvmCallContextUnavailable {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// A typed call that claims a canonical Int intrinsic has inconsistent semantics.
    IntIntrinsicResolution {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: JvmIntIntrinsicError,
    },
    /// A resolved Int operand did not lower to one JVM Int value.
    IntIntrinsicOperandFlowMismatch {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Operand of the prefix or postfix operator.
        operand: TreeId<Typed>,
        /// Index into the enclosing table or parameter list, using that structure’s indexing
        /// convention.
        index: usize,
        /// Observed JVM value or control-flow representation.
        found: JvmExpressionFlow,
    },
    /// A typed tree that claims a canonical Boolean intrinsic has inconsistent semantics.
    BooleanIntrinsicResolution {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: JvmBooleanIntrinsicError,
    },
    /// A resolved Boolean operand did not lower to one JVM Boolean value.
    BooleanIntrinsicOperandFlowMismatch {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Operand of the prefix or postfix operator.
        operand: TreeId<Typed>,
        /// Index into the enclosing table or parameter list, using that structure’s indexing
        /// convention.
        index: usize,
        /// Observed JVM value or control-flow representation.
        found: JvmExpressionFlow,
    },
    /// A typed application that selects canonical AnyRef identity has inconsistent semantics.
    ReferenceIdentityResolution {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: JvmReferenceIdentityError,
    },
    /// A reference identity operand did not lower to a planned reference or canonical Null.
    ReferenceIdentityOperandFlowMismatch {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Operand of the prefix or postfix operator.
        operand: TreeId<Typed>,
        /// Index into the enclosing table or parameter list, using that structure’s indexing
        /// convention.
        index: usize,
        /// Observed JVM value or control-flow representation.
        found: JvmExpressionFlow,
    },
    /// A virtual call occurs in a control-flow branch whose verifier frames
    /// cannot represent its temporary reference receiver.
    JvmCallInUnsupportedControlFlow {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// A typed call's argument count disagrees with the selected JVM descriptor.
    JvmCallArityMismatch {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Required argument or parameter count.
        expected: usize,
        /// Observed argument or parameter count.
        found: usize,
    },
    /// A lowered argument does not provide the target parameter's primitive value.
    JvmCallArgumentMismatch {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Exact typed argument tree involved in the failed check.
        argument: TreeId<Typed>,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Index into the enclosing table or parameter list, using that structure’s indexing
        /// convention.
        index: usize,
        /// Required JVM value or control-flow representation.
        expected: JvmType,
        /// Observed JVM value or control-flow representation.
        found: JvmExpressionFlow,
    },
    /// Legacy error retained for API compatibility; planned virtual calls now validate references.
    ReferenceCallArgumentUnsupported {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Index into the enclosing table or parameter list, using that structure’s indexing
        /// convention.
        index: usize,
        /// Canonical class symbol whose builtin type is being checked.
        ty: SymbolId,
    },
    /// A typed parameter reference has a different primitive type from its local metadata.
    ParameterReferenceTypeMismatch {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Parameter identity in the canonical session symbol table.
        parameter: SymbolId,
        /// Required JVM value or control-flow representation.
        expected: JvmType,
        /// Observed JVM value or control-flow representation. None means no representation was
        /// available.
        found: Option<JvmType>,
    },
    /// An instruction could not be appended to the method's code builder.
    CodeBuilder {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: dotty_classfile::bytecode::CodeBuilderError,
    },
    /// A conditional expression does not have a Boolean condition.
    InvalidIfCondition {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Observed JVM value or control-flow representation. None means no representation was
        /// available.
        found: Option<JvmType>,
    },
    /// A while condition does not produce a Boolean value on normal flow.
    InvalidWhileCondition {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Observed JVM value or control-flow representation. None means no representation was
        /// available.
        found: Option<JvmType>,
    },
    /// A typed while node does not carry the canonical Unit result type.
    WhileResultTypeMismatch {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
    },
    /// A reachable conditional branch does not match the other branch's value type.
    IfBranchTypeMismatch {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Typed branch root involved in control-flow validation.
        branch: TreeId<Typed>,
        /// Required JVM value or control-flow representation.
        expected: JvmType,
        /// Observed JVM value or control-flow representation. None means no representation was
        /// available.
        found: Option<JvmType>,
    },
    /// An If expression's declared type disagrees with its reachable branch values.
    IfResultTypeMismatch {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Required JVM value or control-flow representation. None means no representation was
        /// available.
        expected: Option<JvmType>,
        /// Observed JVM value or control-flow representation. None means no representation was
        /// available.
        found: Option<JvmType>,
    },
    /// A typed `Nothing` branch is not backed by supported explicit return flow.
    UnsupportedBottomFlow {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
    },
    /// A Return with an explicit target is outside the current local-return subset.
    UnsupportedReturnTarget {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Return target or starting point of the enclosing relation.
        from: TreeId<Typed>,
    },
    /// A return value does not match the method's primitive JVM result.
    ReturnValueTypeMismatch {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Required JVM value or control-flow representation.
        expected: JvmReturnType,
        /// Observed JVM value or control-flow representation. None means no representation was
        /// available.
        found: Option<JvmType>,
    },
    /// A method body value does not match its descriptor's return type.
    MethodBodyTypeMismatch {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Required JVM value or control-flow representation.
        expected: JvmReturnType,
        /// Observed JVM value or control-flow representation. None means no representation was
        /// available.
        found: Option<JvmType>,
    },
    /// A normally completing no-value expression is incompatible with a
    /// value-returning method body.
    MethodBodyFlowMismatch {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Required JVM value or control-flow representation.
        expected: JvmReturnType,
        /// Observed JVM value or control-flow representation.
        found: JvmExpressionFlow,
    },
    /// A no-value `Unit` flow was attached to a typed tree with a different
    /// semantic type.
    UnitFlowTypeMismatch {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
    },
    /// Recursive typed-tree lowering exceeded its fixed depth budget.
    ExpressionDepthExceeded {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Optional source location; absent for nodes without source provenance.
        position: Option<dotty_core::source::SourceSpan>,
        /// Configured upper bound that the operation exceeded.
        limit: usize,
    },
    /// Lowering expanded too many typed-tree occurrences in one call.
    ExpressionNodeBudgetExceeded {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Configured upper bound that the operation exceeded.
        limit: usize,
    },
    /// A method has an unsupported single-clause or monomorphism shape.
    UnsupportedMethodShape {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
        /// Structural or semantic category of the item.
        kind: UnsupportedMethodShapeKind,
    },
    /// A supported primitive parameter's symbol type disagrees with its
    /// corresponding entry in the method's ordered `MethodType`.
    ParameterTypeMismatch {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Parameter identity in the canonical session symbol table.
        parameter: SymbolId,
        /// Explicitly declared type used by the check.
        declared: TypeId,
        /// Type stored as the declaration signature.
        symbol_info: TypeId,
    },
    /// Scala `Unit` cannot appear in a JVM parameter list.
    UnitParameter {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Parameter identity in the canonical session symbol table.
        parameter: SymbolId,
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
    },
    /// An unsupported semantic type was used for a method value or result.
    UnsupportedJvmType {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
        /// Structural or semantic category of the item.
        kind: UnsupportedJvmTypeKind,
    },
    /// A method descriptor exceeds the JVMS 255 parameter-slot limit.
    TooManyParameterSlots {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// JVM local or stack slots required by the operation.
        slots: usize,
    },
    /// A local symbol is not present in the method's validated local layout.
    UnknownLocalSlot {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// The method's local variables exceed the u16 `max_locals` count.
    TooManyLocalSlots {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// Supplied method metadata or bindings do not match this validated unit.
    LocalLayoutInputMismatch {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// A parameter modifier is not represented by this primitive-only slice.
    UnsupportedMethodParameter {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Index into the enclosing table or parameter list, using that structure’s indexing
        /// convention.
        index: usize,
        /// Structural or semantic category of the item.
        kind: UnsupportedMethodParameterKind,
    },
    /// An internally generated descriptor unexpectedly failed the classfile
    /// crate's parser.
    InvalidGeneratedDescriptor {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: dotty_classfile::descriptor::DescriptorError,
    },
    /// A supplied builtin type identity is malformed for this semantic store.
    InvalidBuiltinDefinition {
        /// Compiler-known builtin whose canonical identity is required.
        builtin: PrimitiveBuiltin,
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
    },
    /// A canonical primitive `TypeRef` has a prefix other than the exact
    /// shared `NoPrefix` type required by the supported descriptor slice.
    InvalidPrimitiveTypePrefix {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
        /// Semantic prefix through which the member is selected.
        prefix: TypeId,
        /// Compiler-known builtin whose canonical identity is required.
        builtin: PrimitiveBuiltin,
    },
    /// The symbol owner is not part of the supported package-only class path.
    UnsupportedOwnerKind {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Semantic owner of the declaration; ownership is distinct from lookup scope.
        owner: SymbolId,
        /// Observed declaration kind, independent of flags or visibility.
        found: SymbolKind,
    },
    /// A declared owner identity is not in the supplied semantic store.
    UnknownOwner {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Semantic owner of the declaration; ownership is distinct from lookup scope.
        owner: SymbolId,
    },
    /// The package owner chain is cyclic or has no valid unnamed root.
    InvalidPackageOwnerChain {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Semantic owner of the declaration; ownership is distinct from lookup scope.
        owner: SymbolId,
    },
    /// A class name or package segment is not a supported simple identifier.
    InvalidJvmName {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Structural or semantic category of the item.
        kind: JvmNameKind,
        /// Interned original spelling or decoded textual payload.
        text: String,
    },
    /// The class form is outside the ordinary, concrete, non-generic subset.
    UnsupportedJvmClass {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Structural or semantic category of the item.
        kind: UnsupportedJvmClassKind,
    },
    /// A class or method has visibility other than public.
    UnsupportedJvmVisibility {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Source or semantic visibility of the declaration.
        visibility: dotty_core::symbols::Visibility,
    },
    /// A symbol flag has no mapping in the bounded JVM metadata subset.
    UnsupportedJvmSymbolFlag {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Unsupported flag that caused the rejection.
        flag: UnsupportedJvmSymbolFlag,
    },
    /// Completed class info is malformed or cannot establish a simple class.
    InvalidJvmClassInfo {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Semantic type recorded for the inspected node or declaration.
        ty: Option<TypeId>,
        /// Structural or semantic category of the item.
        kind: InvalidJvmClassInfoKind,
    },
    /// A class declaration scope contains a symbol ID outside the symbol table.
    InvalidClassDeclaration {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// Two methods on one class project to the same JVM name and descriptor.
    DuplicateJvmMethodIdentity {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Previously selected declaration with the conflicting identity.
        previous: SymbolId,
        /// Recorded name spelling used by the enclosing declaration or validation rule.
        name: String,
        /// Erased JVM descriptor used for member identity or validation.
        descriptor: String,
    },
    /// A generated accessor name and descriptor collide with an emitted or
    /// generated method. `previous` is its method symbol or source field ID.
    DuplicateJvmAccessorMethodIdentity {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
        /// Previously selected declaration with the conflicting identity.
        previous: SymbolId,
        /// Recorded name spelling used by the enclosing declaration or validation rule.
        name: String,
        /// Erased JVM descriptor used for member identity or validation.
        descriptor: String,
    },
    /// A declared method and an inherited accessor, or a declared accessor
    /// and inherited method/accessor, share one exact JVM method identity.
    /// The descriptor is absent only for a conservative same-name check
    /// against an unplanned source method.
    InheritedJvmAccessorCollision {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Structural or semantic category of the item.
        kind: JvmAccessorCollisionKind,
        /// Provenance of this declaration, artifact, or argument binding.
        origin: JvmBinaryMethodOrigin,
        /// Parent type or declaration involved in this relationship.
        parent: SymbolId,
        /// Provenance of the inherited method being checked.
        parent_origin: JvmBinaryMethodOrigin,
        /// Recorded name spelling used by the enclosing declaration or validation rule.
        name: String,
        /// Erased JVM descriptor used for member identity or validation.
        descriptor: Option<String>,
        /// Inherited descriptor when checking a result-only mismatch.
        parent_descriptor: Option<String>,
    },
    /// A descendant cannot add new generated accessor storage above inherited
    /// accessors in the currently supported emission subset.
    UnsupportedJvmAccessorInheritance {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Parent type or declaration involved in this relationship.
        parent: SymbolId,
    },
    /// Two class symbols in one compilation unit project to the same internal name.
    DuplicateJvmClassIdentity {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Previously selected declaration with the conflicting identity.
        previous: SymbolId,
        /// JVM internal class name using slash-separated package segments.
        internal_name: String,
    },
    /// The selected class symbol has no corresponding class plan in the unit.
    MissingClassPlan {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
    },
    /// The caller method is not part of the supplied current-class plan.
    CallerMethodNotInJvmPlan {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// A canonical method reference is not selected for JVM emission.
    MethodNotInJvmPlan {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// A typed call site or its function expression has an unsupported shape.
    UnsupportedJvmCallShape {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Structural or semantic category of the item.
        kind: UnsupportedJvmCallShapeKind,
    },
    /// The function tree does not carry a canonical method-symbol reference.
    UnresolvedJvmCallTarget {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
    },
    /// The typed call result disagrees with the selected method descriptor.
    JvmCallResultTypeMismatch {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Required JVM value or control-flow representation.
        expected: JvmReturnType,
        /// Observed JVM value or control-flow representation. None means no representation was
        /// available.
        found: Option<JvmReturnType>,
    },
    /// Lowering the selected receiver did not produce the exact planned class value.
    JvmCallReceiverFlowMismatch {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Required declaration identity in the caller’s symbol table.
        expected: SymbolId,
        /// Observed JVM value or control-flow representation.
        found: JvmExpressionFlow,
    },
    /// The class plan cannot prove that synthesizing the minimal constructor is safe.
    UnsupportedConstructor {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
    },
    /// The class's parent list is not exactly its canonical `Object` type.
    UnsupportedJvmSuperclass {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Parent type or declaration involved in this relationship.
        parent: Option<TypeId>,
    },
    /// A source class parent is not proven by the bounded canonical unit graph.
    InvalidJvmSuperclass {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Parent type or declaration involved in this relationship.
        parent: Option<SymbolId>,
        /// Specific structural or semantic reason for the enclosing rejection.
        issue: JvmSuperclassIssue,
    },
    /// A planned child method collides with a non-private final method
    /// declared by its validated direct parent. The descriptor is absent
    /// when the parent's method is unplanned and the backend rejects by name.
    FinalJvmMethodOverride {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Parent type or declaration involved in this relationship.
        parent: SymbolId,
        /// Overridden or inherited method identity, when available.
        parent_method: SymbolId,
        /// Recorded name spelling used by the enclosing declaration or validation rule.
        name: String,
        /// Erased JVM descriptor used for member identity or validation.
        descriptor: Option<String>,
    },
    /// The source declares an override that is absent, incompatible, or
    /// outside the bounded planned-class contract.
    InvalidJvmOverride {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Parent type or declaration involved in this relationship.
        parent: SymbolId,
        /// Overridden or inherited method identity, when available.
        parent_method: Option<SymbolId>,
        /// Specific structural or semantic reason for the enclosing rejection.
        issue: JvmOverrideIssue,
    },
    /// The class declares a self type outside this emission subset.
    UnsupportedJvmSelfType {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Explicit self type, when present.
        self_type: TypeId,
    },
    /// A declared field, initializer, nested type, or other member is outside
    /// the single-class emission subset.
    UnsupportedJvmClassMember {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Member identity or name involved in this check.
        member: SymbolId,
        /// Stable declaration category, independent of flags and visibility.
        kind: SymbolKind,
    },
    /// A selected method is owned by the class but absent from its declaration scope.
    MethodNotInClassDeclarations {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// ClassFileBuilder rejected staged output; `method` is present when the
    /// failing operation was associated with a selected semantic method.
    ClassFileBuild {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Method identity in the canonical session symbol table.
        method: Option<SymbolId>,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: dotty_classfile::class_file_builder::ClassFileBuildError,
    },
    /// The fixed synthesized constructor could not be built symbolically.
    ConstructorCodeBuilder {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: dotty_classfile::bytecode::CodeBuilderError,
    },
    /// A generated constructor accessor getter could not be built symbolically.
    AccessorMethodCodeBuilder {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: dotty_classfile::bytecode::CodeBuilderError,
    },
}

/// Why a constructor accessor cannot be copied into its checked backing field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmAccessorInitializerIssue {
    /// No constructor local has the exact derived formal identity.
    MissingParameter,
    /// The formal identity is not at the accessor's recorded source ordinal.
    SourceOrdinalMismatch,
    /// The formal's projected JVM type differs from the accessor field type.
    ValueTypeMismatch,
    /// The local width differs from the projected JVM category width.
    SlotWidthMismatch,
    /// The formal is duplicated or the source order is not strictly increasing.
    DuplicateOrUnorderedBinding,
}

/// Reason a source method cannot form an exact inherited JVM override.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmOverrideIssue {
    /// The source uses `override` but the direct parent declares no same-name method.
    NoParentDeclaration,
    /// A same-name declaration exists, but it was not included in the parent plan.
    ParentMethodUnplanned,
    /// The only candidate is private or otherwise inaccessible.
    InaccessibleParentMethod,
    /// The method's JVM descriptor or semantic MethodType differs.
    SignatureMismatch,
    /// The child attempts to reduce the public parent's visibility.
    NarrowerVisibility,
    /// The matching declaration is public, but the child lacks source `override` evidence.
    OverrideModifierRequired,
}

/// Unsupported typed constructor application shape or metadata relation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmConstructorCallShapeError {
    /// The constructor expression is not a value application.
    NotApplication,
    /// The constructor uses unsupported explicit type arguments.
    TypeApplication,
    /// The constructor uses an unsupported explicit contextual argument clause.
    UsingApplication,
    /// The application callee is not a constructor member selection.
    FunctionNotSelect,
    /// The selected constructor is not qualified by a new expression.
    QualifierNotNew,
    /// The new expression does not name the canonical planned class.
    TypeTreeNotCanonicalClass,
    /// The selected constructor differs from the class's canonical constructor.
    ConstructorReferenceNotCanonical,
    /// The selected constructor is absent from its owner's declaration scope.
    ConstructorNotInClassScope,
    /// The constructor initialization plan is outside the supported call subset.
    UnsupportedInitializationPlan,
    /// The retained evidence for implicit no-argument construction is inconsistent.
    InvalidImplicitNoArgProof,
    /// A constructor argument has an unsupported JVM type.
    UnsupportedArgumentType,
}

/// Semantic role required by a compilation plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolRole {
    /// Class declaration being validated.
    Class,
    /// Constructor declaration being validated.
    Constructor,
    /// Field declaration being validated.
    Field,
    /// Method declaration being validated.
    Method,
    /// Formal parameter declaration being validated.
    Parameter,
    /// Local binding being validated.
    Local,
}

/// Unsupported semantic modifier for an ordinary JVM local binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsupportedJvmLocalFlag {
    /// This validation path rejects the lazy declaration marker.
    Lazy,
    /// This validation path rejects the given declaration marker.
    Given,
    /// This validation path rejects the implicit declaration marker.
    Implicit,
    /// This validation path rejects the erased declaration marker.
    Erased,
    /// This validation path rejects the inline declaration marker.
    Inline,
}

/// Unsupported source modifier on a planned JVM field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsupportedJvmFieldFlag {
    /// This validation path rejects the abstract declaration marker.
    Abstract,
    /// This validation path rejects the sealed declaration marker.
    Sealed,
    /// This validation path rejects the case declaration marker.
    Case,
    /// This validation path rejects the implicit declaration marker.
    Implicit,
    /// This validation path rejects the given declaration marker.
    Given,
    /// This validation path rejects the lazy declaration marker.
    Lazy,
    /// This validation path rejects the inline declaration marker.
    Inline,
    /// This validation path rejects the transparent declaration marker.
    Transparent,
    /// This validation path rejects the opaque declaration marker.
    Opaque,
    /// This validation path rejects the extension declaration marker.
    Extension,
    /// This validation path rejects the static declaration marker.
    Static,
    /// This validation path rejects the synthetic declaration marker.
    Synthetic,
    /// This validation path rejects the Java-defined symbol flag.
    JavaDefined,
    /// This validation path rejects the erased declaration marker.
    Erased,
    /// This validation path rejects the override declaration marker.
    Override,
    /// This validation path rejects the enum declaration marker.
    Enum,
    /// This validation path rejects the export-generated symbol flag.
    Exported,
}

/// Why a typed semantic slot cannot be consumed as an actual type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidTypeReason {
    /// The type slot is absent or has not been filled.
    MissingOrUnfilled,
    /// The type is the absence-of-type sentinel.
    NoType,
    /// The type records a previous typing failure.
    Error,
    /// A no-prefix sentinel appears where a value type is required.
    NoPrefix,
}

/// Unsupported method-type shape for this descriptor projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsupportedMethodShapeKind {
    /// The method has unsupported polymorphic type parameters.
    Polymorphic,
    /// The method has unsupported additional curried clauses.
    NestedCurriedClause,
    /// The method clause is implicit or contextual on this lowering path.
    NonPlainClause,
    /// The method type and canonical parameter list have different lengths.
    ParameterCountMismatch,
    /// The declaration's signature is not a method type.
    NotMethodType,
}

/// Why a typed call is outside the supported planned-call subset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsupportedJvmCallShapeKind {
    /// The supplied call root is not a term application.
    NotApplication,
    /// A parameterless source default getter cannot take an explicit argument clause.
    UnexpectedGetterApplication,
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
    /// A receiver is not an immutable local or a validated direct constructor result.
    UnsupportedReferenceReceiver,
    /// The receiver's mutability disagrees with its canonical local plan.
    MutableOrUnplannedReferenceReceiver,
    /// A selected member's `this` receiver is not the current class.
    NonCurrentThis,
    /// A selected member uses a qualified `this` expression.
    QualifiedThisReceiver,
    /// A nested reference-call receiver points back to an active call tree.
    RecursiveReceiver,
    /// The canonical term reference has a prefix outside this call subset.
    UnsupportedReceiverPrefix,
    /// The target has a Float or Double argument/result before FP call lowering is added.
    FloatingPointNotLowered,
    /// A super call is qualified or its qualifier is not the current `this`.
    UnsupportedSuperQualifier,
    /// The typed `SuperType` does not match the caller's proved direct parent.
    SuperTypeDoesNotMatchHierarchy,
}

/// Unsupported semantic type form in this descriptor projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsupportedJvmTypeKind {
    /// Type aliases are not lowered by this validation path.
    Alias,
    /// The type depends on an unsupported type parameter.
    TypeParameter,
    /// The type is an unreduced or unsupported match type.
    MatchType,
    /// Structural refinements are outside this lowering subset.
    RefinedType,
    /// The union type is outside this lowering subset.
    Union,
    /// The intersection type is outside this lowering subset.
    Intersection,
    /// A Scala module value type is outside this lowering subset.
    ScalaObject,
    /// This reference-class type is not accepted by this validation path.
    ReferenceClass,
    /// A name-designed type reference lacks a supported canonical class identity.
    NamedReference,
    /// A term reference is not accepted as a lowered value type here.
    TermReference,
    /// A nested method type appears where a value type is required.
    NestedMethod,
    /// Array lowering is outside this validation path's supported subset.
    Array,
    /// By-name parameter lowering is unsupported here.
    ByName,
    /// Repeated-parameter lowering is unsupported here.
    Repeated,
    /// Type bounds appear where a concrete lowered type is required.
    Bounds,
    /// The singleton constant is outside the supported primitive subset.
    NonPrimitiveConstant,
    /// A semantic type outside the explicitly supported categories was encountered.
    Other,
}

/// Why a semantic type cannot be represented as a planned class reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsupportedJvmReferenceTypeKind {
    /// The reference is not a canonical symbol-designed type reference.
    NotCanonicalTypeRef,
    /// The reference prefix is outside the supported package shape.
    UnsupportedPrefix,
    /// The referenced declaration is not an accepted class kind.
    UnsupportedSymbolKind,
    /// This validation path requires a planned source class rather than an external class.
    ExternalClass,
    /// A module class is outside this reference-type subset.
    ObjectClass,
    /// Anonymous class references are unsupported by this validation path.
    AnonymousClass,
    /// Generic class references require unsupported type substitution.
    GenericClass,
    /// The class cannot be established as deriving from canonical Object.
    NotObjectDerived,
    /// The class metadata fails the supported reference-type contract.
    UnsupportedClassMetadata,
}

/// Primitive definition identity being read from [`Definitions`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrimitiveBuiltin {
    /// The session’s Scala Int builtin identity.
    Int,
    /// The session’s Scala Boolean builtin identity.
    Boolean,
    /// The session’s Scala Float builtin identity.
    Float,
    /// The session’s Scala Long builtin identity.
    Long,
    /// The session’s Scala Double builtin identity.
    Double,
    /// The session’s Scala Unit builtin identity.
    Unit,
}

/// An unsupported method parameter modifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsupportedMethodParameterKind {
    /// The supplied ordered parameter is not the formal of this contextual clause.
    ClauseBinding,
    /// Erased parameters require unsupported runtime argument adaptation.
    Erased,
    /// Repeated or variable-arity parameters are outside this lowering subset.
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

    /// Derives this class, constructor, and selected instance-method metadata.
    /// This store-only entry point supports primitive constructor parameters
    /// because it cannot prove a complete unit for reference signatures. It
    /// also supports only empty field plans; use
    /// [`Self::metadata_with_typed_ast`] when planned fields are present.
    pub fn metadata(
        &self,
        store: &SemanticStore,
        definitions: &Definitions,
    ) -> Result<JvmClassMetadata, JvmCodegenError> {
        identity::class_metadata(self, store, None, definitions, None, None, None)
    }

    /// Derives class, primitive-field, constructor, and selected method
    /// metadata while validating each frontend-issued field/RHS binding
    /// against this exact typed AST. Reference-valued fields and constructor
    /// parameters require [`JvmCompilationUnit::metadata`], which can prove
    /// that their classes are planned in the same emission unit.
    pub fn metadata_with_typed_ast(
        &self,
        store: &SemanticStore,
        typed_ast: &TypedAst,
        definitions: &Definitions,
    ) -> Result<JvmClassMetadata, JvmCodegenError> {
        identity::class_metadata(self, store, Some(typed_ast), definitions, None, None, None)
    }
}

impl JvmClassMetadata {
    /// Explicit primary constructor metadata, when the frontend supplied a plan.
    pub fn constructor(&self) -> Option<&JvmConstructorMetadata> {
        self.constructor.as_ref()
    }

    /// Semantically validated source field to constructor-formal bindings for
    /// the class's primary-constructor accessors. These are proof records for
    /// later JVM-only planning, not generated fields or methods.
    pub fn accessor_bindings(&self) -> &[JvmConstructorAccessorBinding] {
        &self.accessor_bindings
    }

    /// JVM-only backing-field and accessor-method signatures, in source order.
    /// Emission uses these own-class entries only after validating the bounded
    /// hierarchy and constructor proofs.
    pub fn accessors(&self) -> &[JvmAccessorMetadata] {
        &self.accessors
    }
    /// Class identity, preserving its canonical semantic symbol.
    pub const fn identity(&self) -> &JvmClassIdentity {
        &self.identity
    }

    /// Validated direct parent for unit-aware metadata; absent for the legacy
    /// store-only class projection that has no whole-unit hierarchy proof.
    pub fn superclass(&self) -> Option<&JvmSuperclassPlan> {
        self.superclass.as_ref()
    }

    /// Selected method metadata in the class plan's order.
    pub fn methods(&self) -> &[JvmMethodMetadata] {
        &self.methods
    }

    /// Effective public methods reachable on this receiver through the
    /// validated direct-parent edge. Inherited entries retain their original
    /// owner and symbol; this does not add them to [`Self::methods`]. The
    /// store-only [`JvmClassPlan::metadata`] projection has no unit hierarchy,
    /// so its dispatch view is empty.
    pub fn dispatch_methods(&self) -> &[JvmMethodDispatchMetadata] {
        &self.dispatch_methods
    }

    /// Effective generated getter/setter methods reachable on this receiver.
    ///
    /// Entries retain the declaring class and canonical source field. Getter
    /// and setter roles are distinct, inherited entries are never copied into
    /// [`Self::accessors`]. The view supplies evidence to the bounded emission
    /// gate; it does not independently authorize class-file emission.
    pub fn dispatch_accessors(&self) -> &[JvmAccessorDispatchMetadata] {
        &self.dispatch_accessors
    }

    /// Validated instance field metadata in the class plan's source order.
    pub fn fields(&self) -> &[JvmFieldMetadata] {
        &self.fields
    }
}

impl JvmSuperclassPlan {
    /// Canonical child class identity.
    pub const fn class(&self) -> SymbolId {
        self.class
    }

    /// Canonical immediate parent identity.
    pub const fn parent(&self) -> SymbolId {
        self.parent
    }

    /// Checked JVM internal name of the child.
    pub fn class_internal_name(&self) -> &str {
        &self.class_internal_name
    }

    /// Checked JVM internal name of the immediate parent.
    pub fn parent_internal_name(&self) -> &str {
        &self.parent_internal_name
    }
}

impl JvmClassHierarchy {
    /// Direct-superclass proofs in deterministic compilation-unit plan order.
    pub fn superclasses(&self) -> &[JvmSuperclassPlan] {
        &self.superclasses
    }

    /// Returns the nearest common planned ancestor of two planned classes.
    ///
    /// The validated hierarchy has one superclass per class, so the nearest
    /// common ancestor is unique. The built-in `java/lang/Object` root is not
    /// returned: if it is the only common class, or either input is not part
    /// of this hierarchy snapshot, this returns `None`. The query uses only
    /// canonical [`SymbolId`] edges; it does not compare printed names or
    /// modify the hierarchy.
    pub fn nearest_common_non_object_ancestor(
        &self,
        left: SymbolId,
        right: SymbolId,
    ) -> Option<SymbolId> {
        let left_ancestors = self.planned_ancestors(left)?;
        let right_ancestors = self.planned_ancestors(right)?;
        left_ancestors
            .into_iter()
            .find(|candidate| right_ancestors.contains(candidate))
    }

    fn planned_ancestors(&self, class: SymbolId) -> Option<Vec<SymbolId>> {
        let mut ancestors = Vec::new();
        let mut current = class;
        loop {
            if ancestors.contains(&current) {
                return None;
            }
            let edge = self
                .superclasses
                .iter()
                .find(|edge| edge.class == current)?;
            ancestors.push(current);
            if !self
                .superclasses
                .iter()
                .any(|candidate| candidate.class == edge.parent)
            {
                break;
            }
            current = edge.parent;
        }
        Some(ancestors)
    }

    /// Returns the Scala-independent hierarchy edges for class-file consumers.
    pub fn internal_name_edges(&self) -> Vec<JvmInternalNameHierarchyEdge> {
        self.superclasses
            .iter()
            .map(|parent| JvmInternalNameHierarchyEdge {
                child_internal_name: parent.class_internal_name.clone(),
                parent_internal_name: parent.parent_internal_name.clone(),
            })
            .collect()
    }
}

impl JvmSiblingUnionProjection {
    /// Original source `Type::Or` identity; the semantic store is not changed.
    pub const fn source_type(self) -> TypeId {
        self.source_type
    }

    /// Canonical expected source type that authorized the projection.
    pub const fn expected_type(self) -> TypeId {
        self.expected_type
    }

    /// The two exact canonical source arm symbols, in their original order.
    pub const fn source_arms(self) -> [SymbolId; 2] {
        self.source_arms
    }

    /// Checked JVM representation of the expected common class.
    pub const fn representation(self) -> JvmType {
        self.representation
    }
}

impl JvmInternalNameHierarchyEdge {
    /// JVM internal name on the subclass side of this edge.
    pub fn child_internal_name(&self) -> &str {
        &self.child_internal_name
    }

    /// JVM internal name on the superclass side of this edge.
    pub fn parent_internal_name(&self) -> &str {
        &self.parent_internal_name
    }
}

impl JvmFieldMetadata {
    /// Canonical field symbol used to derive this metadata.
    pub const fn symbol(&self) -> SymbolId {
        self.symbol
    }

    /// JVM field name projected from the canonical symbol.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Exact JVM field descriptor (`I`, `Z`, or `L<planned-class>;`).
    pub fn descriptor(&self) -> &str {
        &self.descriptor
    }

    /// Exact semantic value type retained alongside the checked descriptor.
    pub const fn value_type(&self) -> JvmType {
        self.value_type
    }

    /// JVM access flags (`private`, with `final` for immutable Scala vals).
    pub const fn access_flags(&self) -> dotty_classfile::access_flags::FieldAccessFlags {
        self.access_flags
    }

    /// Canonical owning class symbol.
    pub const fn owner(&self) -> SymbolId {
        self.owner
    }

    /// Exact typed initializer root validated for this field.
    pub const fn initializer(&self) -> TreeId<Typed> {
        self.initializer
    }
}

impl JvmAccessorMetadata {
    /// Audited provenance used to derive the JVM projection.
    pub const fn origin(&self) -> JvmAccessorOrigin {
        self.origin
    }

    /// Constructor binding, present only for a primary-constructor accessor.
    pub const fn binding(&self) -> Option<JvmConstructorAccessorBinding> {
        match self.origin {
            JvmAccessorOrigin::ConstructorParameter(binding) => Some(binding),
            JvmAccessorOrigin::ClassBodyField(_) => None,
        }
    }

    /// Source field identity; no synthetic field symbol is introduced.
    pub const fn source_field(&self) -> SymbolId {
        match self.origin {
            JvmAccessorOrigin::ConstructorParameter(binding) => binding.field(),
            JvmAccessorOrigin::ClassBodyField(proof) => proof.field(),
        }
    }

    /// Constructor-owned derived formal identity, absent for body fields.
    pub const fn constructor_parameter(&self) -> Option<SymbolId> {
        match self.origin {
            JvmAccessorOrigin::ConstructorParameter(binding) => Some(binding.parameter()),
            JvmAccessorOrigin::ClassBodyField(_) => None,
        }
    }

    /// Canonical class that owns the source field and JVM backing storage.
    pub const fn owner(&self) -> SymbolId {
        match self.origin {
            JvmAccessorOrigin::ConstructorParameter(binding) => binding.class(),
            JvmAccessorOrigin::ClassBodyField(proof) => proof.class(),
        }
    }

    /// Source ordinal: constructor parameter order or template-body position.
    pub const fn source_ordinal(&self) -> usize {
        match self.origin {
            JvmAccessorOrigin::ConstructorParameter(binding) => binding.source_ordinal(),
            JvmAccessorOrigin::ClassBodyField(proof) => proof.template_position(),
        }
    }

    /// Whether the source accessor is a mutable `var`.
    pub const fn is_mutable(&self) -> bool {
        match self.origin {
            JvmAccessorOrigin::ConstructorParameter(binding) => binding.is_mutable(),
            JvmAccessorOrigin::ClassBodyField(proof) => proof.mutable(),
        }
    }

    /// Exact checked JVM value type shared by the field, getter and setter.
    pub const fn value_type(&self) -> JvmType {
        self.value_type
    }

    /// Source-name-derived JVM backing field name proven by the pinned oracle.
    pub fn field_name(&self) -> &str {
        &self.field_name
    }

    /// JVM descriptor of the private backing field.
    pub fn field_descriptor(&self) -> &str {
        &self.field_descriptor
    }

    /// JVM-private flags, with `final` for immutable `val` storage.
    pub const fn field_access_flags(&self) -> dotty_classfile::access_flags::FieldAccessFlags {
        self.field_access_flags
    }

    /// Public getter signature for this accessor.
    pub const fn getter(&self) -> &JvmAccessorMethodMetadata {
        &self.getter
    }

    /// Public setter signature for a mutable `var`; absent for a `val`.
    pub const fn setter(&self) -> Option<&JvmAccessorMethodMetadata> {
        self.setter.as_ref()
    }
}

impl JvmAccessorMethodMetadata {
    /// Exact JVM member name from the pinned Scala 3.9.0 oracle.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Validated descriptor for the accessor method.
    pub const fn descriptor(&self) -> &JvmMethodDescriptor {
        &self.descriptor
    }

    /// Public JVM access flags observed in the pinned compiler output.
    pub const fn access_flags(&self) -> dotty_classfile::access_flags::MethodAccessFlags {
        self.access_flags
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

    /// Exact typed expression used as the receiver, or `None` for implicit `this`.
    pub const fn receiver_tree(&self) -> Option<TreeId<Typed>> {
        self.receiver_tree
    }

    /// Canonical class identity that must be present on the operand stack.
    pub const fn receiver_class(&self) -> SymbolId {
        self.receiver_class
    }
}

impl JvmConstructorCallTarget {
    /// Canonical planned class that owns the selected constructor.
    pub const fn owner_class(&self) -> SymbolId {
        self.owner_class
    }

    /// JVM internal name of the constructor owner.
    pub fn owner_internal_name(&self) -> &str {
        &self.owner_internal_name
    }

    /// Canonical constructor selected by the typed `TermRef`.
    pub const fn constructor_symbol(&self) -> SymbolId {
        self.constructor
    }

    /// Exact validated constructor descriptor, including `()V` for implicit no-arg.
    pub const fn descriptor(&self) -> &JvmMethodDescriptor {
        &self.descriptor
    }

    /// Validated public/private JVM constructor access flags.
    pub const fn access_flags(&self) -> dotty_classfile::access_flags::MethodAccessFlags {
        self.access_flags
    }

    /// Supported argument types in typed source order.
    pub fn argument_types(&self) -> &[JvmType] {
        &self.argument_types
    }

    /// Exact class-reference type produced by the application.
    pub const fn result_type(&self) -> JvmType {
        self.result_type
    }

    /// Allocation always dispatches through the JVM's special constructor call.
    pub const fn invocation_kind(&self) -> JvmConstructorInvocationKind {
        JvmConstructorInvocationKind::Special
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

    /// Conservative flags for a planned ordinary instance method.
    pub const fn access_flags(&self) -> dotty_classfile::access_flags::MethodAccessFlags {
        self.access_flags
    }

    /// Formal parameter identities and local slots in JVM/source order.
    pub fn parameters(&self) -> &[JvmParameterLocal] {
        &self.parameters
    }
}

impl JvmMethodDispatchMetadata {
    /// Receiver class whose effective method view contains this entry.
    pub const fn receiver_class(&self) -> SymbolId {
        self.receiver_class
    }

    /// Class that declares the canonical method symbol.
    pub const fn declaring_class(&self) -> SymbolId {
        self.declaring_class
    }

    /// Canonical method symbol; inherited entries keep the parent's identity.
    pub const fn method(&self) -> SymbolId {
        self.method
    }

    /// JVM-safe method name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Exact JVM method descriptor.
    pub const fn descriptor(&self) -> &JvmMethodDescriptor {
        &self.descriptor
    }
}

impl JvmAccessorDispatchMetadata {
    /// Receiver class whose effective accessor view contains this entry.
    pub const fn receiver_class(&self) -> SymbolId {
        self.receiver_class
    }

    /// Class that declares the source field and generated accessor method.
    pub const fn declaring_class(&self) -> SymbolId {
        self.declaring_class
    }

    /// Canonical source field symbol; no source method symbol is fabricated.
    pub const fn field(&self) -> SymbolId {
        self.field
    }

    /// Whether the entry denotes the getter or setter role.
    pub const fn role(&self) -> JvmAccessorDispatchRole {
        self.role
    }

    /// JVM-safe generated accessor method name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Exact JVM method descriptor.
    pub const fn descriptor(&self) -> &JvmMethodDescriptor {
        &self.descriptor
    }

    /// Validated method access flags.
    pub const fn access_flags(&self) -> dotty_classfile::access_flags::MethodAccessFlags {
        self.access_flags
    }
}

/// Lowers the supported value-producing leaves from the supplied typed AST.
///
/// Int, Boolean, and Long literals become category-correct constant
/// instructions.
/// Parameter identifiers are resolved only through their canonical
/// `TermRefTarget::Symbol` and this method's parameter metadata. Int and
/// Boolean leaves produce [`JvmValueCategory::IntLike`]; Long leaves produce
/// [`JvmValueCategory::Long`]. The method does not add a return instruction
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
/// Int/Boolean/Long values, canonical Int/Boolean intrinsics, the `Unit`
/// literal, ordered blocks, and nested `If`/local `Return` nodes are supported.
/// Intrinsics resolve only by their canonical semantic symbols. `Value` leaves
/// one correctly typed value on the JVM stack; `Unit` completes normally with
/// no stack value; `Diverges` has no reachable fallthrough. Blocks discard unused primitive
/// values with category-correct stack operations where supported and stop
/// lowering after a definitely diverging statement. Long-valued branch joins
/// are supported; discarded Long statements remain unsupported because the
/// standalone API does not emit category-two stack manipulation.
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
/// error. Int and Boolean values use `ireturn`, Long values use `lreturn`, a
/// normally completing Unit body gets `return`, and a body whose paths all
/// terminate gets no additional epilogue.
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
/// 65 and contains only the planned public instance methods, the validated
/// constructor, checked backing fields, and no TASTy or other Scala ABI metadata.
pub fn emit_class(
    unit: &JvmCompilationUnit<'_>,
    class_symbol: SymbolId,
    definitions: &Definitions,
) -> Result<GeneratedClass, JvmCodegenError> {
    emission::emit_class(unit, class_symbol, definitions)
}

/// Emits one selected class with an explicit Scala 3 TASTy sidecar link.
///
/// This opt-in path writes the supplied, already validated link identifier as
/// the class-level `TASTY` attribute. It does not produce or validate the
/// sidecar itself; callers coordinating a pair must validate the encoded
/// TASTy header and class bytes together. [`emit_class`] remains unchanged and
/// emits no Scala metadata.
pub fn emit_class_with_tasty_link(
    unit: &JvmCompilationUnit<'_>,
    class_symbol: SymbolId,
    definitions: &Definitions,
    tasty_link: dotty_classfile::tasty_link::TastyLinkId,
) -> Result<GeneratedClass, JvmCodegenError> {
    emission::emit_class_with_tasty_link(unit, class_symbol, definitions, tasty_link)
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
    /// Pattern scope and typed CaseDef preserved from the validated local plan.
    /// None identifies an ordinary ValDef local.
    pub const fn pattern_case(self) -> Option<(dotty_core::ScopeId, TreeId<Typed>)> {
        self.pattern
    }

    /// Exact typed ValDef or pattern Bind that supplied this canonical binding.
    pub const fn declaration(self) -> TreeId<Typed> {
        self.declaration
    }

    /// Canonical local symbol used as the identity for this slot.
    pub const fn symbol(self) -> SymbolId {
        self.symbol
    }

    /// Completed primitive or planned reference type stored in this category-one slot.
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
    /// Exact reference descriptors require a validated `JvmCompilationUnit`
    /// and are available through its `metadata` projection instead.
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
    /// Projects a completed canonical type reference to an exact class in
    /// this validated emission unit.
    ///
    /// This does not erase to `java/lang/Object`, follow aliases, or project
    /// external, generic, anonymous, deeper-inheritance, or otherwise
    /// unsupported class shapes. The result can describe expression/local
    /// values and unit-aware method metadata, but it does not enable arbitrary
    /// reference bytecode emission.
    pub fn reference_type(
        &self,
        ty: TypeId,
        definitions: &Definitions,
    ) -> Result<JvmType, JvmCodegenError> {
        reference::project_type(self, ty, definitions)
    }

    /// Proves that an exact two-arm sibling `Type::Or` can use an explicitly
    /// expected planned class as its JVM reference representation.
    ///
    /// The expected class must be the nearest non-Object common ancestor of
    /// both canonical planned arms. This only returns projection evidence; it
    /// does not rewrite the typed AST, infer an expected type, or lower control
    /// flow. Standalone unions remain unsupported by [`Self::reference_type`].
    pub fn project_sibling_union_to_expected_type(
        &self,
        source_type: TypeId,
        expected_type: TypeId,
        definitions: &Definitions,
    ) -> Result<JvmSiblingUnionProjection, JvmCodegenError> {
        reference::project_sibling_union_to_expected_type(
            self,
            source_type,
            expected_type,
            definitions,
        )
    }

    /// Projects a completed local value type to its exact primitive or
    /// validated planned-class representation.
    pub(crate) fn local_value_type(
        &self,
        symbol: SymbolId,
        ty: TypeId,
        definitions: &Definitions,
        metadata: Option<&[JvmClassMetadata]>,
    ) -> Result<JvmType, JvmCodegenError> {
        match metadata {
            Some(metadata) => reference::project_local_type_with_metadata(
                self,
                symbol,
                ty,
                definitions,
                Some(metadata),
            ),
            None => reference::project_local_type(self, symbol, ty, definitions),
        }
    }

    /// Projects all planned class, field, constructor, and method metadata in
    /// plan order.
    ///
    /// Duplicate JVM class names and duplicate `(method name, descriptor)`
    /// pairs are rejected before the metadata is returned.
    pub fn metadata(
        &self,
        definitions: &Definitions,
    ) -> Result<Vec<JvmClassMetadata>, JvmCodegenError> {
        identity::unit_metadata(self, definitions)
    }

    /// Builds the validated direct-superclass proof for every planned class.
    ///
    /// The graph accepts Object-rooted classes and one planned subclass hop.
    /// It is an owned snapshot: the semantic store is never modified, and
    /// names are projected only after canonical parent identities validate.
    pub fn class_hierarchy(
        &self,
        definitions: &Definitions,
    ) -> Result<JvmClassHierarchy, JvmCodegenError> {
        identity::unit_hierarchy(self, definitions)
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

    /// Lowers one explicitly planned method body using this unit's validated
    /// class, field, and method metadata. This method-level path supports
    /// planned reference values and the validated direct `Derived` to
    /// `Base` assignment for arguments, immutable local initializers, and
    /// returns. It can also lower private primitive field reads and mutable
    /// field assignments; it does not enable arbitrary `Object` upcasts.
    /// [`emit_class`] installs only fields covered by a validated
    /// primary constructor.
    pub fn lower_method_body(
        &self,
        class: SymbolId,
        method: SymbolId,
        definitions: &Definitions,
    ) -> Result<dotty_classfile::bytecode::CodeBuilder, JvmCodegenError> {
        self.validate()?;
        let plan = self
            .classes
            .iter()
            .find(|plan| plan.class == class)
            .ok_or(JvmCodegenError::MissingClassPlan { class })?;
        let all_metadata = self.metadata(definitions)?;
        let metadata = all_metadata
            .iter()
            .find(|metadata| metadata.identity().symbol() == class)
            .ok_or(JvmCodegenError::MissingClassPlan { class })?;
        let method_plan = plan
            .methods
            .iter()
            .find(|candidate| candidate.method == method)
            .ok_or(JvmCodegenError::MethodNotInJvmPlanForBody { class, method })?;
        let method_metadata = metadata
            .methods()
            .iter()
            .find(|candidate| candidate.symbol() == method)
            .ok_or(JvmCodegenError::MethodNotInJvmPlanForBody { class, method })?;
        let body = method_plan
            .body
            .ok_or(JvmCodegenError::MissingTypedBody { method })?;
        expression::lower_planned_method_body(
            self.into(),
            metadata,
            definitions,
            &all_metadata,
            method_metadata,
            body,
        )
    }

    /// Lowers checked private and public-body field initializers into
    /// source-ordered constructor fragments. Primary-constructor parameter
    /// reads use the validated constructor slots; an implicit no-arg plan has
    /// no fabricated constructor metadata.
    ///
    /// The returned plan excludes the superclass constructor call and final
    /// `return`; callers must place its instructions after the validated
    /// direct-parent `<init>()V` call.
    /// It supports the bounded literal, parameter, intrinsic, planned-reference
    /// and prior-field-read subset. Unsupported expressions and self/forward
    /// field reads return typed errors without modifying the unit.
    pub fn lower_constructor_field_initializers(
        &self,
        class: SymbolId,
        definitions: &Definitions,
    ) -> Result<JvmConstructorFieldInitializerPlan, JvmCodegenError> {
        constructor_initializers::lower(self, class, definitions)
    }

    /// Resolves a typed ordinary call to a planned virtual target.
    ///
    /// `call` identifies the complete typed application or the bare selection
    /// of a registered parameterless default getter. The callee must be a
    /// canonical method reference in an `Ident` or a
    /// `Select` whose receiver has an exact planned class identity. Its selected
    /// method must be declared by that class or its validated direct parent and
    /// appear unchanged in the receiver's effective dispatch metadata.
    /// Resolution uses the selected method `SymbolId` and this unit's explicit
    /// emission plan; it does not evaluate arguments, load a receiver, or emit
    /// instructions.
    pub fn resolve_same_class_call(
        &self,
        caller_class: SymbolId,
        caller_method: SymbolId,
        call: TreeId<Typed>,
        definitions: &Definitions,
    ) -> Result<JvmCallTarget, JvmCodegenError> {
        call_target::resolve_same_class_call(self, caller_class, caller_method, call, definitions)
    }

    /// Resolves a typed instance call on `this`, a reference parameter, a
    /// validated direct `new`, an immutable initialized local, or a completed
    /// reference-valued planned call with an exact planned class identity.
    ///
    /// The selected `TermRef` symbol, receiver class, owner declaration scope,
    /// class/method plans, visibility, descriptor, and call result must all
    /// agree. An inherited target must be the exact planned public method
    /// declared by the receiver's validated direct parent. This read-only
    /// resolver emits no instructions; method-body lowering checks direct
    /// subclass assignability for arguments, immutable local initializers,
    /// and returns while evaluating each receiver once before arguments in
    /// source order. Other reference conversions remain unsupported.
    pub fn resolve_planned_instance_call(
        &self,
        caller_class: SymbolId,
        caller_method: SymbolId,
        call: TreeId<Typed>,
        definitions: &Definitions,
    ) -> Result<JvmCallTarget, JvmCodegenError> {
        call_target::resolve_instance_call(
            self.into(),
            caller_class,
            caller_method,
            call,
            definitions,
            JvmInvocationKind::Virtual,
            None,
        )
    }

    /// Resolves an explicitly typed `super.method(args)` call to the canonical
    /// method declared by this class's direct parent.
    ///
    /// The complete `Apply` tree must select an unqualified `Super(This)`;
    /// the `SuperType`, selected `TermRefTarget::Symbol`, hierarchy edge,
    /// parent declaration, public instance method plan, descriptor, and result
    /// type must agree. This is a read-only proof and never falls back to
    /// ordinary virtual/name-based resolution.
    pub fn resolve_direct_super_call(
        &self,
        caller_class: SymbolId,
        caller_method: SymbolId,
        call: TreeId<Typed>,
        definitions: &Definitions,
    ) -> Result<JvmCallTarget, JvmCodegenError> {
        call_target::resolve_instance_call(
            self.into(),
            caller_class,
            caller_method,
            call,
            definitions,
            JvmInvocationKind::SpecialSuper,
            None,
        )
    }

    /// Resolves a typed `Apply(Select(New(...), <init>), args)` to the exact
    /// constructor approved by this compilation unit.
    ///
    /// The resolver checks class/constructor identity, initialization proof,
    /// accessibility, argument order and primitive types. It returns metadata
    /// for a future allocation emitter and emits no allocation or invocation
    /// instructions itself.
    pub fn resolve_constructor_call(
        &self,
        caller_class: SymbolId,
        caller_method: SymbolId,
        apply_tree: TreeId<Typed>,
        definitions: &Definitions,
    ) -> Result<JvmConstructorCallTarget, JvmCodegenError> {
        constructor_call::resolve(self, caller_class, caller_method, apply_tree, definitions)
    }

    /// Lowers the receiver for a validated same-class instance call.
    ///
    /// The complete call tree is first resolved through
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
        let all_metadata = self.metadata(definitions)?;
        let class_metadata = all_metadata
            .iter()
            .find(|metadata| metadata.identity().symbol() == caller_class)
            .ok_or(JvmCodegenError::MissingClassPlan {
                class: caller_class,
            })?;
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
            class_metadata,
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
        let mut seen_fields = HashSet::new();
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
            if let JvmClassInitializationPlan::PrimaryConstructor(constructor) =
                &class_plan.initialization
            {
                constructor.validate_binding(self.store, class_plan.class)?;
                self.validate_symbol_type(constructor.symbol())?;
                if let Some(super_constructor) = constructor.direct_super_constructor() {
                    super_constructor.validate_typed_binding(
                        self.store,
                        self.typed_ast,
                        class_plan.class,
                        constructor.symbol(),
                    )?;
                    self.validate_symbol_type(super_constructor.parent_class())?;
                    self.validate_symbol_type(super_constructor.parent_constructor())?;
                }
                for parameter in constructor.parameters() {
                    self.validate_symbol_type(*parameter)?;
                }
            }

            let mut class_field_symbols = HashSet::new();
            let mut last_public_body_position = None;
            let mut last_field_source_position: Option<(SourceId, u32)> = None;
            let mixed_visibility_fields = class_plan.fields.iter().any(|field| {
                matches!(
                    field.source_visibility,
                    JvmFieldSourceVisibility::PlainPublic(_)
                )
            }) && class_plan.fields.iter().any(|field| {
                matches!(
                    field.source_visibility,
                    JvmFieldSourceVisibility::PlainPrivate
                )
            });
            for field_plan in &class_plan.fields {
                if !class_field_symbols.insert(field_plan.field)
                    || !seen_fields.insert(field_plan.field)
                {
                    return Err(JvmCodegenError::DuplicateFieldPlan {
                        field: field_plan.field,
                    });
                }
                let field = self
                    .store
                    .symbols
                    .contains(field_plan.field)
                    .then(|| self.store.symbols.get(field_plan.field))
                    .ok_or(JvmCodegenError::UnknownField {
                        field: field_plan.field,
                    })?;
                if field.kind != SymbolKind::Field {
                    return Err(JvmCodegenError::UnsupportedSymbolKind {
                        symbol: field_plan.field,
                        expected: SymbolRole::Field,
                        found: field.kind,
                    });
                }
                if field.owner != Some(class_plan.class) {
                    return Err(JvmCodegenError::WrongOwner {
                        symbol: field_plan.field,
                        expected: class_plan.class,
                        actual: field.owner,
                    });
                }
                let expected_visibility = match field_plan.source_visibility {
                    JvmFieldSourceVisibility::PlainPrivate
                    | JvmFieldSourceVisibility::PrivateThis => {
                        dotty_core::symbols::Visibility::Private
                    }
                    JvmFieldSourceVisibility::PlainPublic(_) => {
                        dotty_core::symbols::Visibility::Public
                    }
                };
                if field.visibility != expected_visibility {
                    return Err(JvmCodegenError::UnsupportedJvmVisibility {
                        symbol: field_plan.field,
                        visibility: field.visibility,
                    });
                }
                match field_plan.source_visibility {
                    JvmFieldSourceVisibility::PlainPrivate => {}
                    JvmFieldSourceVisibility::PrivateThis => {
                        return Err(JvmCodegenError::UnsupportedJvmFieldSourceVisibility {
                            field: field_plan.field,
                            visibility: field_plan.source_visibility,
                        });
                    }
                    JvmFieldSourceVisibility::PlainPublic(proof)
                        if proof.class != class_plan.class
                            || proof.field != field_plan.field
                            || Some(proof.initializer) != field_plan.initializer =>
                    {
                        return Err(JvmCodegenError::InvalidPublicBodyFieldSourceProof {
                            field: field_plan.field,
                        });
                    }
                    JvmFieldSourceVisibility::PlainPublic(proof) => {
                        if proof.mutable != field.flags.contains(SymbolFlags::MUTABLE) {
                            return Err(JvmCodegenError::InvalidPublicBodyFieldSourceProof {
                                field: field_plan.field,
                            });
                        }
                        if last_public_body_position
                            .is_some_and(|position| position >= proof.template_position)
                        {
                            return Err(JvmCodegenError::OutOfOrderPublicBodyFieldPlan {
                                class: class_plan.class,
                                field: field_plan.field,
                            });
                        }
                        last_public_body_position = Some(proof.template_position);
                    }
                }
                identity::validate_field_flags(field_plan.field, field.flags)?;
                let class_info = identity::class_info(self.store, class_plan.class)?;
                if !self
                    .store
                    .scopes
                    .get(class_info.declarations)
                    .entered_symbols()
                    .any(|declaration| declaration == field_plan.field)
                {
                    return Err(JvmCodegenError::FieldNotInClassScope {
                        class: class_plan.class,
                        field: field_plan.field,
                    });
                }
                self.validate_symbol_type(field_plan.field)?;
                let initializer =
                    field_plan
                        .initializer
                        .ok_or(JvmCodegenError::MissingFieldInitializer {
                            field: field_plan.field,
                        })?;
                let initializer_tree = self.typed_ast.try_get(initializer).ok_or(
                    JvmCodegenError::UnknownFieldInitializer {
                        field: field_plan.field,
                        initializer,
                    },
                )?;
                if mixed_visibility_fields {
                    let source_position = initializer_tree
                        .position
                        .map(|position| (position.source(), position.span().range().start()))
                        .ok_or(JvmCodegenError::FieldInitializerSourcePositionUnavailable {
                            field: field_plan.field,
                        })?;
                    if let Some((previous_source, previous_start)) = last_field_source_position {
                        if previous_source != source_position.0 {
                            return Err(JvmCodegenError::FieldInitializersFromDifferentSources {
                                class: class_plan.class,
                                field: field_plan.field,
                            });
                        }
                        if previous_start >= source_position.1 {
                            return Err(JvmCodegenError::OutOfOrderFieldPlan {
                                class: class_plan.class,
                                field: field_plan.field,
                            });
                        }
                    }
                    last_field_source_position = Some(source_position);
                }
                validate_tree_type(self.store, initializer, initializer_tree)?;
            }

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
                    let valid_pattern = binding.pattern.is_some_and(|(scope, case)| {
                        matches!(&declaration.kind, TreeKind::Bind(bind) if !bind.given && bind.name.is_term()
                            && self.store.symbols.contains(binding.symbol)
                            && bind.name == self.store.symbols.get(binding.symbol).name)
                            && matches!(self.store.types.try_get(declaration.ty), Some(dotty_core::Type::TermRef { prefix, target: dotty_core::TermRefTarget::Symbol(target) })
                                if *target == binding.symbol && matches!(self.store.types.try_get(*prefix), Some(dotty_core::Type::NoPrefix)))
                            && matches!(self.typed_ast.try_get(case).map(|n| &n.kind), Some(TreeKind::CaseDef(node)) if node.pattern == binding.declaration)
                            && typed_tree_contains(self.typed_ast, body, case)
                            && self.store.scopes.contains(scope) && self.store.symbols.contains(binding.symbol)
                            && self.store.scopes.get(scope).owner == Some(method_plan.method)
                            && self.store.scopes.get(scope).lookup_all(&self.store.symbols.get(binding.symbol).name) == [binding.symbol]
                            && !binding.mutable && binding.literal_given.is_none()
                    });
                    if !(binding.pattern.is_none()
                        && matches!(declaration.kind, TreeKind::ValDef(_))
                        || valid_pattern)
                    {
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
                    let literal_given = binding.literal_given.is_some_and(|proof| {
                        proof.matches(binding, method_plan.method, self.typed_ast, self.store)
                    });
                    if binding.literal_given.is_some() && !literal_given {
                        return Err(JvmCodegenError::InvalidLiteralGiven {
                            symbol: binding.symbol,
                        });
                    }
                    for (flag, required_flag) in [
                        (UnsupportedJvmLocalFlag::Lazy, SymbolFlags::LAZY),
                        (UnsupportedJvmLocalFlag::Given, SymbolFlags::GIVEN),
                        (UnsupportedJvmLocalFlag::Implicit, SymbolFlags::IMPLICIT),
                        (UnsupportedJvmLocalFlag::Erased, SymbolFlags::ERASED),
                        (UnsupportedJvmLocalFlag::Inline, SymbolFlags::INLINE),
                    ] {
                        if local.flags.contains(required_flag)
                            && !(literal_given
                                && matches!(
                                    flag,
                                    UnsupportedJvmLocalFlag::Given | UnsupportedJvmLocalFlag::Lazy
                                ))
                        {
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
                    if !valid_pattern && declaration.ty != symbol_type {
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
    any_typed_tree(ast, root, |tree| tree == sought)
}

fn any_typed_tree(
    ast: &TypedAst,
    root: TreeId<Typed>,
    mut visit: impl FnMut(TreeId<Typed>) -> bool,
) -> bool {
    let mut pending = vec![root];
    let mut visited = HashSet::new();
    while let Some(id) = pending.pop() {
        if !visited.insert(id) {
            continue;
        }
        if visit(id) {
            return true;
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
