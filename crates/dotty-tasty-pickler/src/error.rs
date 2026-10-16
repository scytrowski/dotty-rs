use std::{error::Error, fmt};

use dotty_core::{
    NameId, ScopeId, SourceId, SymbolId, SymbolInfo, SymbolKind, SymbolOrigin, TreeId, TypeId,
    Typed, Untyped,
};
use dotty_tasty::tasty::{AstBuildError, TastyFileError};

/// Why a canonical symbol cannot be represented by the bounded reference API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceError {
    Namespace,
    Kind,
    Owner,
    Ambiguous,
    NotRegistered,
    NotLocal,
    NotExternal,
    Metadata,
    DeclarationTag,
}

/// A bounded builtin identity required by the minimal contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    Int,
    Unit,
    Object,
}

/// Expected role of a source AST edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceRole {
    Package,
    PackageName,
    Class,
    Template,
    Constructor,
    Method,
    Parameter,
    TypeSyntax,
    Body,
}

/// Semantic type role used in a validation failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeRole {
    NoPrefix,
    Builtin(Builtin),
    ClassInfo,
    ConstructorSignature,
    ConstructorResult,
    MethodResult,
    Parameter,
    Body,
}

/// Valid Scala forms intentionally outside the frozen first slice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsupportedFeature {
    PackageBoundary,
    SourceMetadata,
    SymbolMetadata,
    TemplateMetadata,
    ClassMembers,
    Parents,
    SelfType,
    TypeParameters,
    ParameterClauses,
    ConstructorBody,
    MethodCount,
    AbstractMethod,
    BodyExpression,
    ResultTypeSyntax,
    Name,
}

/// Bounded validation and semantic projection work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitKind {
    OwnerDepth,
    Items,
    NameBytes,
    Names,
    TypeDepth,
    FileBytes,
}

/// Typed failures, with canonical identities rather than parsed display strings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickleError {
    /// A failed projection context cannot publish or accept further work.
    ProjectionAborted,
    CyclicType {
        ty: TypeId,
    },
    Reference {
        symbol: SymbolId,
        reason: ReferenceError,
    },
    Construction {
        cause: Box<AstBuildError>,
    },
    InvalidSymbol {
        symbol: SymbolId,
    },
    InvalidType {
        ty: TypeId,
    },
    IncompleteType {
        ty: TypeId,
    },
    InvalidScope {
        scope: ScopeId,
    },
    InvalidName {
        name: NameId,
    },
    InvalidSourceTree {
        tree: TreeId<Untyped>,
    },
    InvalidTypedTree {
        tree: TreeId<Typed>,
    },
    IncompleteSymbol {
        symbol: SymbolId,
        info: SymbolInfo,
    },
    SourceBinding {
        source: SourceId,
        tree: TreeId<Untyped>,
        expected: SymbolId,
        actual: Option<SymbolId>,
    },
    SourceDefinition {
        symbol: SymbolId,
    },
    SourceOrigin {
        symbol: SymbolId,
        expected: SourceId,
        actual: SymbolOrigin,
    },
    SourceSpan {
        expected: SourceId,
        actual: SourceId,
    },
    InvalidSourceName,
    Kind {
        symbol: SymbolId,
        expected: SymbolKind,
        actual: SymbolKind,
    },
    Owner {
        symbol: SymbolId,
        expected: Option<SymbolId>,
        actual: Option<SymbolId>,
    },
    OwnerCycle {
        symbol: SymbolId,
    },
    ScopeMembers {
        owner: SymbolId,
    },
    NameMismatch {
        symbol: SymbolId,
        tree: TreeId<Untyped>,
    },
    SourceShape {
        tree: TreeId<Untyped>,
        expected: SourceRole,
    },
    SourceOrder {
        class: SymbolId,
    },
    DuplicateDeclaration {
        symbol: SymbolId,
    },
    ParameterOrder {
        method: SymbolId,
        clause: usize,
    },
    Unsupported {
        feature: UnsupportedFeature,
        symbol: Option<SymbolId>,
    },
    MalformedType {
        ty: TypeId,
        role: TypeRole,
    },
    UnsupportedTypeReference {
        ty: TypeId,
    },
    BuiltinIdentity {
        builtin: Builtin,
        ty: TypeId,
    },
    MissingBody {
        method: SymbolId,
    },
    BodyBinding {
        source: SourceId,
        tree: TreeId<Untyped>,
        expected: TreeId<Typed>,
        actual: Option<TreeId<Typed>>,
    },
    BodyType {
        tree: TreeId<Typed>,
        ty: TypeId,
    },
    BodySourceValue {
        source_tree: TreeId<Untyped>,
        typed_tree: TreeId<Typed>,
        expected: i32,
        actual: i32,
    },
    BodySpan {
        tree: TreeId<Typed>,
    },
    LimitExceeded {
        kind: LimitKind,
        limit: usize,
    },
    /// Preserves nested codec errors during projection and provisional assembly.
    Encoding {
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
