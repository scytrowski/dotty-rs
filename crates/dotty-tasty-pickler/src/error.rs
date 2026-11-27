use std::{error::Error, fmt};

use dotty_core::{
    NameId, ScopeId, SourceId, SymbolId, SymbolInfo, SymbolKind, SymbolOrigin, TreeId, TypeId,
    Typed, Untyped,
};
use dotty_tasty::tasty::{AstBuildError, TastyFileError};

/// Why a canonical symbol cannot be represented by the bounded reference API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceError {
    /// The reference uses the wrong term or type namespace.
    Namespace,
    /// The referenced symbol has an unsupported declaration kind.
    Kind,
    /// The reference does not have the required canonical owner.
    Owner,
    /// More than one declaration could satisfy this reference.
    Ambiguous,
    /// The referenced symbol has no entry in the projection registry.
    NotRegistered,
    /// A local reference targets a declaration outside the local projection.
    NotLocal,
    /// An external reference targets a declaration classified as local.
    NotExternal,
    /// The reference's canonical metadata is inconsistent.
    Metadata,
    /// The declaration's structural tag disagrees with the reference kind.
    DeclarationTag,
}

/// A bounded builtin identity required by the minimal contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    /// The session’s Scala Float builtin identity (raw IEEE-754 binary32 values).
    Float,
    /// The session’s Scala Double builtin identity (raw IEEE-754 binary64 values).
    Double,
    /// The session’s Scala Long builtin identity (signed 64-bit values).
    Long,
    /// The session’s Scala Int builtin identity.
    Int,
    /// The session’s Scala Boolean builtin identity.
    Boolean,
    /// The session’s Scala Unit builtin identity.
    Unit,
    /// The session’s canonical Object builtin identity.
    Object,
}

/// Expected role of a source AST edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceRole {
    /// Package declaration syntax.
    Package,
    /// Package path syntax.
    PackageName,
    /// Class declaration syntax.
    Class,
    /// Class template syntax.
    Template,
    /// Primary-constructor syntax.
    Constructor,
    /// Field declaration syntax.
    Field,
    /// Method declaration syntax.
    Method,
    /// Parameter declaration syntax.
    Parameter,
    /// Source type annotation syntax.
    TypeSyntax,
    /// Declaration body or initializer syntax.
    Body,
}

/// Semantic type role used in a validation failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeRole {
    /// Canonical no-prefix sentinel used for unqualified references.
    NoPrefix,
    /// Canonical builtin type required by this plan.
    Builtin(Builtin),
    /// Class-info signature for the source class.
    ClassInfo,
    /// Primary-constructor method signature.
    ConstructorSignature,
    /// Result type of the primary constructor.
    ConstructorResult,
    /// Method result type.
    MethodResult,
    /// Formal parameter type.
    Parameter,
    /// Type of the typed body expression.
    Body,
}

/// Valid Scala forms intentionally outside the frozen first slice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsupportedFeature {
    /// Package structure is outside the supported source boundary.
    PackageBoundary,
    /// Source provenance includes metadata this producer does not encode.
    SourceMetadata,
    /// Symbol flags or metadata exceed the supported semantic subset.
    SymbolMetadata,
    /// Template metadata exceeds the supported producer subset.
    TemplateMetadata,
    /// The class contains unsupported member forms.
    ClassMembers,
    /// The class has unsupported parent syntax or semantic parents.
    Parents,
    /// An explicit self type is outside this producer subset.
    SelfType,
    /// The declaration has unsupported type parameters.
    TypeParameters,
    /// Parameter clauses exceed this plan's supported shape.
    ParameterClauses,
    /// Constructor statements exceed the supported initialization subset.
    ConstructorBody,
    /// The number of methods exceeds this plan's supported shape.
    MethodCount,
    /// An abstract method cannot provide the complete body required here.
    AbstractMethod,
    /// The body expression is outside the supported producer subset.
    BodyExpression,
    /// The body uses an unsupported primitive operation.
    PrimitiveOperator,
    /// The source result annotation is outside this producer subset.
    ResultTypeSyntax,
    /// The name cannot be represented by this bounded producer.
    Name,
    /// The field shape is outside this producer subset.
    ClassField,
    /// The primary-constructor inputs exceed the supported contract.
    PrimaryConstructorInput,
    /// The field initializer cannot be represented safely by this producer.
    UnsafeFieldInitializer,
}

/// Bounded validation and semantic projection work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitKind {
    /// Maximum owner-chain traversal depth.
    OwnerDepth,
    /// Maximum recursive expression traversal depth.
    ExpressionDepth,
    /// Maximum number of projected items.
    Items,
    /// Maximum UTF-8 byte length for a name.
    NameBytes,
    /// Maximum number of name-table entries.
    Names,
    /// Maximum recursive semantic type traversal depth.
    TypeDepth,
    /// Maximum encoded artifact size in bytes.
    FileBytes,
}

/// Typed failures, with canonical identities rather than parsed display strings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickleError {
    /// A failed projection context cannot publish or accept further work.
    ProjectionAborted,
    /// A semantic type graph contains an unsupported cycle.
    CyclicType {
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
    },
    /// A canonical symbol cannot be represented by the bounded reference API.
    Reference {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Detailed reason for rejecting this input or dependency.
        reason: ReferenceError,
    },
    /// The structural AST builder rejected the projected shape.
    Construction {
        /// Underlying failure that prevented this operation from completing.
        cause: Box<AstBuildError>,
    },
    /// A symbol ID is absent from the supplied semantic store.
    InvalidSymbol {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// A type ID is absent from the supplied type arena.
    InvalidType {
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
    },
    /// A type slot has been reserved but not filled.
    IncompleteType {
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
    },
    /// A scope ID is absent from the supplied scope arena.
    InvalidScope {
        /// Lookup-scope identity in the canonical session scope arena.
        scope: ScopeId,
    },
    /// A name ID is absent from the supplied name interner.
    InvalidName {
        /// Interned text identity in the supplied name table.
        name: NameId,
    },
    /// A source tree ID is absent from the supplied untyped arena.
    InvalidSourceTree {
        /// Source tree identity in its original session arena.
        tree: TreeId<Untyped>,
    },
    /// A typed tree ID is absent from the supplied typed arena.
    InvalidTypedTree {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
    },
    /// An expression root is reused where the producer requires distinct occurrence identity.
    RepeatedExpression {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
    },
    /// A declaration has no successfully completed signature.
    IncompleteSymbol {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Recorded signature or completion state of the canonical declaration.
        info: SymbolInfo,
    },
    /// The canonical primitive method has an unexpected receiver type.
    PrimitiveOperatorReceiver {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Required semantic type identity in the caller’s type arena.
        expected: TypeId,
        /// Observed semantic type identity in the caller’s type arena.
        actual: TypeId,
    },
    /// The canonical primitive method has an unexpected parameter count or clause shape.
    PrimitiveOperatorArity {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Required argument or parameter count. None means the signature has no supported count.
        expected: Option<usize>,
        /// Observed argument or parameter count. None means the signature has no supported count.
        actual: Option<usize>,
    },
    /// A primitive method parameter has an unexpected canonical type.
    PrimitiveOperatorParameter {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Zero-based position in the ordered parameter or argument list.
        index: usize,
        /// Required semantic type identity in the caller’s type arena.
        expected: TypeId,
        /// Observed semantic type identity in the caller’s type arena.
        actual: TypeId,
    },
    /// The primitive method result differs from its canonical result type.
    PrimitiveOperatorResult {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Required semantic type identity in the caller’s type arena.
        expected: TypeId,
        /// Observed semantic type identity in the caller’s type arena.
        actual: TypeId,
    },
    /// The source tree does not map to the expected canonical symbol.
    SourceBinding {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Source tree identity in its original session arena.
        tree: TreeId<Untyped>,
        /// Required declaration identity in the caller’s symbol table.
        expected: SymbolId,
        /// Observed declaration identity in the caller’s symbol table. None records a missing
        /// binding or owner.
        actual: Option<SymbolId>,
    },
    /// A symbol lacks its required original source definition.
    SourceDefinition {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// A declaration originates in a different source or origin kind.
    SourceOrigin {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Required source buffer identity in the compilation session.
        expected: SourceId,
        /// Observed source or binary provenance of the declaration.
        actual: SymbolOrigin,
    },
    /// A source span belongs to a different source identity.
    SourceSpan {
        /// Required source buffer identity in the compilation session.
        expected: SourceId,
        /// Observed source buffer identity in the compilation session.
        actual: SourceId,
    },
    /// The supplied source path is not an accepted normalized relative path.
    InvalidSourceName,
    /// A canonical declaration has the wrong symbol kind.
    Kind {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Required declaration kind, independent of flags or visibility.
        expected: SymbolKind,
        /// Observed declaration kind, independent of flags or visibility.
        actual: SymbolKind,
    },
    /// A canonical declaration has the wrong semantic owner.
    Owner {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Required declaration identity in the caller’s symbol table. None records a missing
        /// binding or owner.
        expected: Option<SymbolId>,
        /// Observed declaration identity in the caller’s symbol table. None records a missing
        /// binding or owner.
        actual: Option<SymbolId>,
    },
    /// The semantic owner chain contains a cycle.
    OwnerCycle {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// The owner's declaration scope disagrees with the canonical plan.
    ScopeMembers {
        /// Semantic owner of the declaration; ownership is distinct from lookup scope.
        owner: SymbolId,
    },
    /// The semantic declaration name differs from its source spelling.
    NameMismatch {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Source tree identity in its original session arena.
        tree: TreeId<Untyped>,
    },
    /// The source tree does not have the required structural role.
    SourceShape {
        /// Source tree identity in its original session arena.
        tree: TreeId<Untyped>,
        /// Required structural role of the original source tree.
        expected: SourceRole,
    },
    /// The plan does not preserve original class-member order.
    SourceOrder {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
    },
    /// A declaration identity appears more than once in the plan.
    DuplicateDeclaration {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// The plan does not preserve canonical parameter order within a clause.
    ParameterOrder {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Parameter-clause category or position involved in this check.
        clause: usize,
    },
    /// The plan contains a valid feature outside this producer subset.
    Unsupported {
        /// Valid language feature outside the supported implementation subset.
        feature: UnsupportedFeature,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: Option<SymbolId>,
    },
    /// The type graph does not satisfy its required semantic role.
    MalformedType {
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
        /// Role of this item in the source or semantic validation contract.
        role: TypeRole,
    },
    /// The type reference is outside the supported prefix or designator subset.
    UnsupportedTypeReference {
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
    },
    /// A required type differs from this session's canonical builtin.
    BuiltinIdentity {
        /// Compiler-known builtin whose canonical identity is required.
        builtin: Builtin,
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
    },
    /// A planned method lacks the complete typed body required for emission.
    MissingBody {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// The body is not the exact typed root recorded for its source RHS.
    BodyBinding {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Source tree identity in its original session arena.
        tree: TreeId<Untyped>,
        /// Exact typed root required by the original source binding.
        expected: TreeId<Typed>,
        /// Typed root actually recorded for the source binding; absent when no binding exists.
        actual: Option<TreeId<Typed>>,
    },
    /// The typed body has a type outside the required result contract.
    BodyType {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
    },
    /// The source integer literal and typed constant disagree.
    BodySourceValue {
        /// Exact source tree root in the canonical session arena.
        source_tree: TreeId<Untyped>,
        /// Exact typed tree root in the canonical session arena.
        typed_tree: TreeId<Typed>,
        /// Integer value in the original source literal.
        expected: i32,
        /// Integer constant value recorded on the typed tree.
        actual: i32,
    },
    /// The typed body lacks the required source span.
    BodySpan {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
    },
    /// The field type differs from the declared canonical type.
    FieldType {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Required semantic type identity in the caller’s type arena.
        expected: TypeId,
        /// Observed semantic type identity in the caller’s type arena.
        actual: TypeId,
    },
    /// The initializer is not the exact typed root recorded for its source RHS.
    FieldInitializerBinding {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Exact source tree root in the canonical session arena.
        source_tree: TreeId<Untyped>,
        /// Exact typed root required by the original source binding.
        expected: TreeId<Typed>,
        /// Typed root actually recorded for the source binding; absent when no binding exists.
        actual: Option<TreeId<Typed>>,
    },
    /// The source and typed field initializer spans disagree.
    FieldInitializerSpan {
        /// Exact source tree root in the canonical session arena.
        source_tree: TreeId<Untyped>,
        /// Exact typed tree root in the canonical session arena.
        typed_tree: TreeId<Typed>,
    },
    /// The source and typed method body spans disagree.
    MethodBodySpan {
        /// Exact source tree root in the canonical session arena.
        source_tree: TreeId<Untyped>,
        /// Exact typed tree root in the canonical session arena.
        typed_tree: TreeId<Typed>,
    },
    /// The configured validation or projection work bound was exceeded.
    LimitExceeded {
        /// Structural or semantic category of the item.
        kind: LimitKind,
        /// Configured upper bound that the operation exceeded.
        limit: usize,
    },
    /// Preserves nested codec errors during projection and provisional assembly.
    Encoding {
        /// Underlying failure that prevented this operation from completing.
        cause: Box<TastyFileError>,
    },
}

impl fmt::Display for PickleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl Error for PickleError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Encoding { cause } => Some(cause.as_ref()),
            Self::Construction { cause } => Some(cause.as_ref()),
            _ => None,
        }
    }
}
