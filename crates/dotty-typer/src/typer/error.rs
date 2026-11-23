//! Public typer error and diagnostic payload types.

use std::fmt;

use super::*;

/// A non-fatal issue emitted by a committed typing operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TyperWarning {
    /// Positional arguments were passed to a legacy implicit parameter clause.
    ImplicitClauseWithoutUsing,
}

impl TyperWarning {
    /// Stable identifier for classification independently of presentation.
    pub const fn code(self) -> &'static str {
        match self {
            Self::ImplicitClauseWithoutUsing => "typer.implicit_clause_without_using",
        }
    }
}

/// A warning payload with the severity and byte range assigned by the typer.
pub type TyperDiagnostic = dotty_core::Diagnostic<TyperWarning>;

/// Stable source-shape category for an unsupported pattern root.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PatternKind {
    /// Unqualified identifier pattern.
    Identifier,
    /// Stable qualified selection pattern.
    StableSelection,
    /// Literal constant pattern.
    Literal,
    /// Pattern carrying an explicit type annotation.
    Typed,
    /// Alternative patterns separated by a vertical bar.
    Alternative,
    /// Applied pattern syntax before extractor resolution.
    Application,
    /// Pattern syntax with explicit type arguments.
    TypeApplication,
    /// Named pattern binding.
    Binding,
    /// Resolved extractor-pattern syntax.
    Extractor,
    /// Tuple pattern syntax.
    Tuple,
    /// Infix pattern syntax.
    Infix,
    /// Explicitly parenthesized pattern.
    Parenthesized,
    /// Pattern shape outside the explicitly classified categories.
    Other,
}

/// Callable-shape limitation encountered while resolving `unapply`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtractorMethodShapeIssue {
    NotMethod,
    MethodKind,
    ParameterArity,
    ErasedParameter,
    RepeatedParameter,
    ByNameParameter,
    TrailingClause,
}

/// Unsupported source syntax within an extractor pattern's argument list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtractorPatternArgumentIssue {
    Named,
    SequenceWildcard,
}

/// Unsupported shape of a member used by the Option-like extractor protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtractorResultMemberIssue {
    NotParameterless,
    NotValueType,
    IsEmptyNotBoolean,
}

/// Product-selector metadata encountered while typing an extractor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtractorProductIssue {
    /// The product does not expose the ordered selectors required by the pattern.
    SelectorShape,
    /// The result's relation to `scala.Product` cannot be checked.
    ProductRelationUnsupported,
}

/// Why a tuple pattern could not be lowered through its canonical Scala tuple
/// extractor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TuplePatternResolutionIssue {
    TupleClassNotFound,
    CompanionNotFound,
    CompanionModuleClassUnavailable,
    UnapplyNotFound,
    UnapplyShapeUnsupported,
    TupleTypeUnsupported,
}

/// Why a source field cannot provide a field-initializer expression context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldInitializerContextIssue {
    /// The symbol is not present in the semantic store.
    SymbolMissing,
    /// The symbol is not a field.
    SymbolKind(SymbolKind),
    /// The field is inline; inline field initializers are not supported by
    /// the current initializer-context contract.
    InlineField,
    /// The symbol has no canonical source definition.
    SourceDefinitionMissing,
    /// The source definition belongs to a different source or is derived.
    SourceDefinitionMismatch,
    /// The source tree does not map back to this field symbol.
    SourceTreeSymbolMismatch,
    /// The source tree is outside the current arena.
    SourceTreeMissing,
    /// The source tree is not a `ValDef`.
    SourceTreeKind,
    /// The field has no initializer RHS.
    InitializerMissing,
    /// The initializer RHS is outside the current arena.
    InitializerTreeMissing,
    /// The field has no semantic owner.
    OwnerMissing,
    /// The field is not owned by an ordinary source class.
    OwnerKind {
        /// Semantic owner of the declaration; ownership is distinct from lookup scope.
        owner: SymbolId,
        /// Stable declaration category, independent of flags and visibility.
        kind: SymbolKind,
    },
    /// The enclosing class does not match its source class definition.
    EnclosingClassMalformed {
        /// Semantic owner of the declaration; ownership is distinct from lookup scope.
        owner: SymbolId,
    },
    /// No lexical source context was recorded for the field.
    DeclarationContextMissing,
    /// No separate initializer context was recorded for the field.
    InitializerContextMissing,
    /// The recorded source context is outside the semantic index.
    SourceContextMissing {
        /// Lexical context identity in the source semantic index.
        context: SourceContextId,
    },
    /// The context does not expose the primary-constructor parameter scope
    /// with the enclosing class declaration context as its parent.
    InitializerEnvironmentMalformed {
        /// Semantic owner expected by the lexical source context.
        context_owner: SymbolId,
        /// Lookup-scope identity in the canonical session scope arena.
        context_scope: dotty_core::ScopeId,
        /// Owner actually recorded on the source context scope.
        actual_scope_owner: Option<SymbolId>,
    },
}

impl PatternKind {
    /// Returns the stable diagnostic/audit label for this pattern shape.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Identifier => "identifier",
            Self::StableSelection => "stable selection",
            Self::Literal => "literal",
            Self::Typed => "typed pattern",
            Self::Alternative => "alternative",
            Self::Application => "application pattern",
            Self::TypeApplication => "type application pattern",
            Self::Binding => "binding pattern",
            Self::Extractor => "extractor pattern",
            Self::Tuple => "tuple pattern",
            Self::Infix => "infix pattern",
            Self::Parenthesized => "parenthesized pattern",
            Self::Other => "other pattern",
        }
    }
}

/// A recoverable failure while projecting or completing source semantics.
#[derive(Debug)]
pub enum TyperError {
    /// A symbol is not present in the semantic store.
    UnknownSymbol {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// No source provenance is indexed for this symbol.
    SourceProvenanceMissing {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// A declaration that needs lexical lookup has no source lexical context.
    DeclarationContextMissing {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// A source context ID is outside the naming result that produced it.
    SourceContextMissing {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Index of the lexical source context in this source semantic index.
        context_index: u32,
    },
    /// A source context chain or package identity is inconsistent while
    /// collecting lexical package candidates.
    LexicalPackageCandidateInvalid {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Index of the lexical source context in this source semantic index.
        context_index: Option<u32>,
        /// Package identity or path associated with this declaration.
        package: Option<SymbolId>,
        /// Detailed reason for rejecting this input or dependency.
        reason: &'static str,
    },
    /// The requested expression-context owner is not present in the semantic store.
    ExpressionOwnerMissing {
        /// Semantic owner of the declaration; ownership is distinct from lookup scope.
        owner: SymbolId,
    },
    /// This declaration kind cannot own an expression body context.
    ExpressionOwnerKindUnsupported {
        /// Semantic owner of the declaration; ownership is distinct from lookup scope.
        owner: SymbolId,
        /// Stable declaration category, independent of flags and visibility.
        kind: SymbolKind,
    },
    /// A method or constructor has no indexed lexical scope for its body.
    ExpressionMethodScopeMissing {
        /// Semantic owner of the declaration; ownership is distinct from lookup scope.
        owner: SymbolId,
    },
    /// The recorded method body scope is owned by a different symbol.
    ExpressionMethodScopeOwnerMismatch {
        /// Semantic owner of the declaration; ownership is distinct from lookup scope.
        owner: SymbolId,
        /// Lookup-scope identity in the canonical session scope arena.
        scope: dotty_core::ScopeId,
        /// Observed declaration identity in the caller’s symbol table. None records a missing
        /// binding or owner.
        actual: Option<SymbolId>,
    },
    /// A source context stored on an expression owner is outside this index.
    ExpressionOwnerSourceContextMissing {
        /// Semantic owner of the declaration; ownership is distinct from lookup scope.
        owner: SymbolId,
        /// Lexical context identity in the source semantic index.
        context: SourceContextId,
    },
    /// No source context is indexed for this expression owner.
    ExpressionOwnerDeclarationContextMissing {
        /// Semantic owner of the declaration; ownership is distinct from lookup scope.
        owner: SymbolId,
    },
    /// A source field cannot provide a validated initializer context.
    FieldInitializerContextInvalid {
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: Option<u32>,
        /// Specific structural or semantic reason for the enclosing rejection.
        issue: FieldInitializerContextIssue,
    },
    /// Source `var` syntax and semantic mutability disagree for an inferred field.
    FieldMutabilityMismatch {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Whether the original source declaration is mutable.
        source_mutable: bool,
        /// Whether the canonical semantic symbol is mutable.
        semantic_mutable: bool,
    },
    /// A local expression scope is outside the semantic store's scope arena.
    ExpressionLocalScopeMissing {
        /// Lookup-scope identity in the canonical session scope arena.
        scope: dotty_core::ScopeId,
    },
    /// A local-scope handle is malformed for this typer's stack.
    ExpressionLocalScopeStackMissing {
        /// Expression-scope identity in this typer session.
        stack: ExpressionScopeId,
    },
    /// A local-scope handle was created by a different `SourceTyper`.
    ExpressionLocalScopeStackForeign {
        /// Expression-scope identity in this typer session.
        stack: ExpressionScopeId,
    },
    /// The type name may come from an import form not supported by this pass.
    UnsupportedImportContext {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Index of the lexical source context in this source semantic index.
        context_index: u32,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        import_tree_index: u32,
    },
    /// An import qualifier does not resolve to an entered package or scope.
    ImportQualifierNotFound {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        import_tree_index: u32,
    },
    /// A local import qualifier did not resolve to a supported stable prefix.
    ImportQualifierNotStable {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        import_tree_index: u32,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// An import tree has an unexpected AST shape or stale child reference.
    MalformedSourceImport {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        import_tree_index: u32,
    },
    /// Completion for this semantic declaration category is not implemented.
    UnsupportedSymbolCompletion {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Stable declaration category, independent of flags and visibility.
        kind: SymbolKind,
    },
    /// A source class-like declaration has no declaration scope from naming.
    MissingClassScope {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// A class-like symbol already contains an incompatible complete type.
    MalformedClassInfo {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Recorded signature or completion state of the canonical declaration.
        info: TypeId,
    },
    /// A two-field source case class does not have the bounded Product2
    /// projection field types `Int` and `Boolean` in that order.
    UnsupportedProduct2FieldTypes {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// An explicit-parent Product1 case class does not have its bounded `Int`
    /// source field type.
    UnsupportedProduct1FieldType {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// The anonymous template exceeds the supported empty source marker-trait shape.
    AnonymousClassInstantiationDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// The projected type does not resolve to a named class-like symbol.
    NewTargetNotClass {
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
    },
    /// Traits cannot be instantiated with `new`.
    TraitInstantiation {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// Abstract classes cannot be instantiated with `new`.
    AbstractClassInstantiation {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// Module classes and objects cannot be instantiated with `new`.
    ModuleInstantiation {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// Packages cannot be instantiated with `new`.
    PackageInstantiation {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// A type parameter is not proven to be an instantiable concrete class.
    TypeParameterInstantiation {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// Class metadata needed for construction is absent or incomplete.
    NewClassInfoUnavailable {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// Constructor lookup normalization failed or exceeded its bound.
    ConstructorLookupNormalization {
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: crate::types::TypeNormalizeError,
    },
    /// The exact `<init>` scope bucket contains a non-constructor or invalid symbol.
    MalformedConstructorBucket {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// A constructor bucket entry has a non-callable or invalid completed type.
    MalformedConstructorCandidate {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Semantic callable type whose parameter clauses are being inspected.
        callable: TypeId,
    },
    /// A template parent is not a type or a supported constructor-call shape.
    /// Constructor application found no constructor candidate for its class.
    ConstructorApplicationUnavailable {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
    },
    /// Constructor application has multiple candidates; overload selection is deferred.
    ConstructorOverloadResolutionDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Candidates examined by lookup; preserves rejection details when available.
        candidates: Vec<SymbolId>,
    },
    /// No direct constructor candidate accepts the current argument list.
    ConstructorApplicationNoApplicable {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Candidates examined by lookup; preserves rejection details when available.
        candidates: Vec<(SymbolId, OverloadRejection)>,
    },
    /// Several direct constructors apply, but no unique most-specific candidate exists.
    AmbiguousConstructorApplication {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Candidates examined by lookup; preserves rejection details when available.
        candidates: Vec<SymbolId>,
    },
    /// An unsupported direct constructor could compete with otherwise applicable candidates.
    ConstructorOverloadResolutionRequiresUnsupportedCandidate {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Declaration considered by lookup or overload resolution.
        candidate: SymbolId,
    },
    /// Generic constructor application is deferred to explicit/inferred type arguments.
    ConstructorPolymorphicApplicationDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// Explicit class arguments require a polymorphic primary constructor.
    ConstructorCallableNotPolymorphic {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Constructor declaration identity being inspected in the supplied session.
        constructor: SymbolId,
        /// Type of the constructed instance.
        instance_type: TypeId,
    },
    /// Explicit constructor type arguments do not match the constructor binder.
    ConstructorTypeArgumentArityMismatch {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Constructor declaration identity being inspected in the supplied session.
        constructor: SymbolId,
        /// Required argument or parameter count.
        expected: usize,
        /// Observed argument or parameter count.
        actual: usize,
    },
    /// The constructor owner is inconsistent with the class in the typed `New`.
    ConstructorInstanceClassMismatch {
        /// Constructor declaration identity being inspected in the supplied session.
        constructor: SymbolId,
        /// Semantic owner of the declaration; ownership is distinct from lookup scope.
        owner: SymbolId,
        /// Class instantiated by the constructor application.
        instance_class: SymbolId,
    },
    /// Binder-exact constructor Poly instantiation failed.
    ConstructorPolyInstantiationFailed {
        /// Constructor declaration identity being inspected in the supplied session.
        constructor: SymbolId,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: dotty_core::TypeRebindError,
    },
    /// The instantiated constructor result disagrees with the typed `New` type.
    ConstructorResultTypeMismatch {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Constructor declaration identity being inspected in the supplied session.
        constructor: SymbolId,
        /// Recorded result type of the constructor signature.
        constructor_result: TypeId,
        /// Type of the constructed instance.
        instance_type: TypeId,
    },
    /// The supported subtype relation cannot compare the constructor result
    /// with the typed `New` instance type.
    ConstructorResultTypeCheckUnsupported {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Constructor declaration identity being inspected in the supplied session.
        constructor: SymbolId,
        /// Recorded result type of the constructor signature.
        constructor_result: TypeId,
        /// Type of the constructed instance.
        instance_type: TypeId,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: Box<TypeRelationError>,
    },
    /// An explicit constructor type argument violates one side of its bounds.
    ConstructorTypeArgumentBoundViolation {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Constructor declaration identity being inspected in the supplied session.
        constructor: SymbolId,
        /// Zero-based position in the corresponding ordered parameter, argument, or clause list.
        parameter_index: usize,
        /// Semantic type of the argument being checked.
        argument: TypeId,
        /// Semantic bound in the canonical type arena.
        bound: TypeId,
        /// Bound or producer side to which this failure applies.
        side: TypeArgumentBoundSide,
    },
    /// A constructor Poly parameter uses bounds not supported by this typer.
    UnsupportedConstructorTypeArgumentBounds {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Constructor declaration identity being inspected in the supplied session.
        constructor: SymbolId,
        /// Zero-based position in the corresponding ordered parameter, argument, or clause list.
        parameter_index: usize,
        /// Lower and upper bound representation for this parameter.
        bounds: TypeId,
    },
    /// The supported subtype relation cannot check a constructor type argument bound.
    ConstructorTypeArgumentBoundCheckUnsupported {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Constructor declaration identity being inspected in the supplied session.
        constructor: SymbolId,
        /// Zero-based position in the corresponding ordered parameter, argument, or clause list.
        parameter_index: usize,
        /// Semantic type of the argument being checked.
        argument: TypeId,
        /// Semantic bound in the canonical type arena.
        bound: TypeId,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: Box<TypeRelationError>,
    },
    /// A generic primary constructor parameter was not constrained by its arguments.
    UnconstrainedConstructorTypeParameter {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Constructor declaration identity being inspected in the supplied session.
        constructor: SymbolId,
        /// Zero-based position in the corresponding ordered parameter, argument, or clause list.
        parameter_index: usize,
    },
    /// A raw generic `New` selected a secondary constructor whose owner type
    /// parameters cannot yet be inferred by overload resolution.
    UnsupportedRawGenericSecondaryConstructorInference {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Constructor declaration identity being inspected in the supplied session.
        constructor: SymbolId,
        /// Semantic owner of the declaration; ownership is distinct from lookup scope.
        owner: SymbolId,
    },
    /// Constructor arguments constrain one Poly parameter to non-equivalent types.
    ConflictingConstructorInferenceConstraints {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Constructor declaration identity being inspected in the supplied session.
        constructor: SymbolId,
        /// Zero-based position in the corresponding ordered parameter, argument, or clause list.
        parameter_index: usize,
        /// First type encountered while checking this relationship.
        first: TypeId,
        /// Second type encountered while checking this relationship.
        second: TypeId,
    },
    /// A constructor formal/actual pair is outside supported structural inference.
    UnsupportedConstructorInferenceShape {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Constructor declaration identity being inspected in the supplied session.
        constructor: SymbolId,
        /// Zero-based position in the corresponding ordered parameter, argument, or clause list.
        parameter_index: Option<usize>,
        /// Formal parameter identity; this is not inferred from its name.
        formal: TypeId,
        /// Observed semantic type identity in the caller’s type arena.
        actual: TypeId,
    },
    /// A raw generic `New` could not be assigned a sound inferred instance type.
    UnableToFinalizeRawGenericNewInstanceType {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Constructor declaration identity being inspected in the supplied session.
        constructor: SymbolId,
        /// Result type in the same arena; further parameter clauses are represented by nested
        /// callable types.
        result: TypeId,
    },
    /// The source class parent tree has an invalid structural shape.
    MalformedClassParent {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// A direct parent application selected a private constructor.
    ParentConstructorInaccessible {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Constructor declaration identity being inspected in the supplied session.
        constructor: SymbolId,
    },
    /// A parent constructor argument reads the child receiver before `super` initializes it.
    UninitializedParentArgumentReceiver {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Canonical symbol used as the call receiver.
        receiver: SymbolId,
    },
    /// A projected parent does not resolve to a class or trait declaration.
    UnresolvedParentClassKind {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: Option<SymbolId>,
    },
    /// Higher-kinded source type parameter completion is deferred.
    HigherKindedTypeParameterDeferred {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// Higher-kinded source type alias completion is deferred.
    HigherKindedTypeAliasDeferred {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// A type alias RHS cannot be represented by source alias bounds.
    InvalidCompletedBounds {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
    },
    /// Opaque alias completion is deferred until opaque visibility is modeled.
    OpaqueAliasDeferred {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// The inferred-result method is already being completed through its RHS.
    RecursiveInferredMethodResult {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// An inferred source field is already being completed through its RHS.
    RecursiveInferredFieldType {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// An inferred source field's widened RHS type is not a valid field value type.
    InvalidInferredFieldType {
        /// Field declaration identity being inspected in the supplied session.
        field: SymbolId,
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Inferred type produced while checking this construct.
        inferred: TypeId,
    },
    /// Inferred source field completion exceeded its bounded dependency depth.
    InferredFieldTypeDepthExceeded {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Configured maximum recursive traversal depth.
        max_depth: usize,
    },
    /// An inferred-result method has no body from which to obtain a result.
    InferredMethodResultRightHandSideMissing {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// The typed method body widened to a type that cannot be a method result.
    InvalidInferredMethodResult {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Inferred type produced while checking this construct.
        inferred: TypeId,
    },
    /// Extension signature normalization for right-associative methods is deferred.
    RightAssociativeExtensionDeferred {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// Right-associative infix lowering requires Scala's right-operand rewrite.
    RightAssociativeInfixDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Operator spelling or canonical name used by the operation.
        operator: dotty_core::Name,
    },
    /// An infix operator was not represented in the term namespace.
    InfixOperatorMustBeTerm {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Operator spelling or canonical name used by the operation.
        operator: dotty_core::Name,
    },
    /// A source PrefixOp has a spelling outside Scala's four unary operators.
    UnsupportedPrefixOperator {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Operator spelling or canonical name used by the operation.
        operator: dotty_core::Name,
    },
    /// Scala prefix syntax cannot call a unary method declared with `()`.
    PrefixMethodNeedsArgumentList {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Name carried by the declaration or reference.
        name: dotty_core::Name,
    },
    /// A polymorphic unary selection requires result-directed inference.
    PrefixPolymorphicDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Name carried by the declaration or reference.
        name: dotty_core::Name,
    },
    /// The source annotation is outside the parser's ordinary constructor shape.
    MalformedSourceAnnotation {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// The annotation's exact class identity or annotation ancestry is unavailable.
    SourceAnnotationClassDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Class identity in the canonical session symbol table.
        class: Option<SymbolId>,
    },
    /// The resolved class does not derive from the canonical annotation base.
    SourceAnnotationNotAnnotationClass {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
    },
    /// An argument is not a supported source literal constant.
    SourceAnnotationArgumentNotConstant {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// The same named annotation argument appears more than once.
    SourceAnnotationDuplicateNamedArgument {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Name carried by the declaration or reference.
        name: dotty_core::TermName,
    },
    /// Constructor overloads or signature semantics exceed the first slice.
    SourceAnnotationConstructorDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
    },
    /// The source arguments do not match the resolved constructor's parameters.
    SourceAnnotationConstructorArgumentMismatch {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
    },
    /// A constant cannot yet be checked against its constructor parameter type.
    SourceAnnotationArgumentTypeDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
    },
    /// A source method parameter tree has no symbol for this method owner.
    MethodParameterSymbolMissing {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        parameter_tree_index: u32,
    },
    /// An explicitly typed local method uses a signature form outside the
    /// current plain, single-clause subset.
    LocalMethodSignatureDeferred {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Valid language feature outside the supported implementation subset.
        feature: &'static str,
    },
    /// A local method has no source-written result type; result inference is
    /// handled by a later typer increment.
    LocalMethodInferredResultDeferred {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// A method type-parameter tree has no symbol for this method owner.
    MethodTypeParameterSymbolMissing {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        parameter_tree_index: u32,
    },
    /// Extension method prefix parameter metadata is absent from the source index.
    ExtensionPrefixClausesMissing {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// A method clause has malformed parameter kinds or inconsistent flags.
    MalformedMethodClause {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        method_tree_index: u32,
        /// Zero-based position in the corresponding ordered parameter, argument, or clause list.
        clause_index: usize,
    },
    /// A repeated parameter appears in an implicit or contextual clause.
    RepeatedParameterClauseUnsupported {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        method_tree_index: u32,
        /// Zero-based position in the corresponding ordered parameter, argument, or clause list.
        clause_index: usize,
        /// Zero-based position in the corresponding ordered parameter, argument, or clause list.
        parameter_index: usize,
        /// Whether this parameter clause is ordinary, implicit, or contextual.
        kind: MethodKind,
    },
    /// A repeated parameter is not the final parameter in its clause.
    RepeatedParameterNotFinal {
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        method_tree_index: u32,
        /// Zero-based position in the corresponding ordered parameter, argument, or clause list.
        clause_index: usize,
        /// Zero-based position in the corresponding ordered parameter, argument, or clause list.
        parameter_index: usize,
    },
    /// A repeated parameter can only be projected while validating its method signature.
    RepeatedParameterSignatureContextMissing {
        /// Parameter identity in the canonical session symbol table.
        parameter: SymbolId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        parameter_tree_index: u32,
    },
    /// A constructor is not owned by a class, trait, or module class.
    ConstructorOwnerNotClassLike {
        /// Constructor declaration identity being inspected in the supplied session.
        constructor: SymbolId,
        /// Semantic owner of the declaration; ownership is distinct from lookup scope.
        owner: Option<SymbolId>,
    },
    /// A completed constructor owner does not contain canonical ClassInfo.
    MalformedConstructorOwnerInfo {
        /// Constructor declaration identity being inspected in the supplied session.
        constructor: SymbolId,
        /// Semantic owner of the declaration; ownership is distinct from lookup scope.
        owner: SymbolId,
        /// Recorded signature or completion state of the canonical declaration.
        info: TypeId,
    },
    /// A constructor owner's source definition does not have a template.
    MalformedConstructorOwner {
        /// Constructor declaration identity being inspected in the supplied session.
        constructor: SymbolId,
        /// Semantic owner of the declaration; ownership is distinct from lookup scope.
        owner: SymbolId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// Scala 3 secondary constructors cannot declare their own type parameters.
    SecondaryConstructorTypeParametersUnsupported {
        /// Constructor declaration identity being inspected in the supplied session.
        constructor: SymbolId,
    },
    /// A deferred completion belongs to a future completion engine.
    DeferredSymbolCompletion {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// A previous fatal semantic failure has already been recorded.
    SymbolAlreadyErrored {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// A source type-tree form is not supported yet.
    UnsupportedTypeTree {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// AST node category encountered at the failure site.
        tree_kind: &'static str,
    },
    /// Recursive source type-tree projection exceeded its bounded depth.
    SourceTypeProjectionDepthExceeded {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Configured maximum recursive traversal depth.
        max_depth: usize,
    },
    /// A declaration has no source-written type and must not be inferred here.
    MissingDeclaredType {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Optional source location; absent for nodes without source provenance.
        position: Option<SourceSpan>,
    },
    /// The source tree does not have the definition shape expected by its symbol.
    MalformedSourceAst {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Stable declaration category, independent of flags and visibility.
        kind: SymbolKind,
    },
    /// The source tree's declaration category does not agree with the symbol.
    SymbolSourceKindMismatch {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Stable declaration category, independent of flags and visibility.
        kind: SymbolKind,
    },
    /// A referenced tree ID is outside the supplied arena.
    TreeOutsideArena {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// A simple type identifier did not resolve in its declaration scope.
    TypeNameNotFound {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Name carried by the declaration or reference.
        name: dotty_core::Name,
        /// Optional source location; absent for nodes without source provenance.
        position: Option<SourceSpan>,
    },
    /// A source name exists, but its semantic symbol cannot denote a type.
    WrongTypeNameKind {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Name carried by the declaration or reference.
        name: dotty_core::Name,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Stable declaration category, independent of flags and visibility.
        kind: SymbolKind,
        /// Optional source location; absent for nodes without source provenance.
        position: Option<SourceSpan>,
    },
    /// The requested function class is beyond the source typer's bounded support.
    UnsupportedSourceFunctionArity {
        /// Structural or semantic category of the item.
        kind: SourceFunctionKind,
        /// Number of arguments, parameters, or components in the inspected shape.
        arity: usize,
        /// Largest supported argument or parameter count.
        max_arity: usize,
    },
    /// The configured semantic resolver could not materialize a canonical function class.
    SourceFunctionClassNotFound {
        /// Structural or semantic category of the item.
        kind: SourceFunctionKind,
        /// Number of arguments, parameters, or components in the inspected shape.
        arity: usize,
    },
    /// The external semantic symbol resolver could not answer soundly.
    SymbolResolution {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: ResolutionError,
    },
    /// Resolving the canonical Java String type failed during type widening.
    StringTypeResolution {
        /// Underlying typed failure; inspect it for the more specific cause.
        error: ResolutionError,
    },
    /// The lexical scope exposes multiple possible type declarations.
    AmbiguousTypeName {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Name carried by the declaration or reference.
        name: dotty_core::Name,
        /// Optional source location; absent for nodes without source provenance.
        position: Option<SourceSpan>,
    },
    /// The type-tree cache was already bound to a different type.
    DuplicateSourceTypeCacheEntry {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Previously recorded value retained by the failed operation.
        existing: TypeId,
        /// New value rejected because it conflicts with the existing entry.
        attempted: TypeId,
    },
    /// A member's declaration-relative this type could not be adapted to its receiver.
    MemberPrefixRebinding {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Semantic prefix through which the member is selected.
        prefix: TypeId,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: dotty_core::types::TypeRebindError,
    },
    /// A future completion operation could not rebind a semantic type.
    TypeRebinding {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: dotty_core::TypeRebindError,
    },
    /// A source class has no canonical AST provenance from which to recover
    /// its class type-parameter order.
    SourceClassTypeParametersProvenanceMissing {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// The source class, template, or primary constructor has an invalid AST
    /// shape for class type-parameter recovery.
    MalformedSourceClassTypeParameters {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// A class type-parameter tree has no canonical symbol mapping.
    ClassTypeParameterSymbolMissing {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// A receiver's nominal constructor differs from the expected class.
    ReceiverDoesNotDenoteExpectedClass {
        /// Required declaration identity in the caller’s symbol table.
        expected: SymbolId,
        /// Observed declaration identity in the caller’s symbol table. None records a missing
        /// binding or owner.
        actual: Option<SymbolId>,
    },
    /// The arguments on a source receiver do not match its source class arity.
    ReceiverGenericArityMismatch {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Required argument or parameter count.
        expected: usize,
        /// Observed argument or parameter count.
        actual: usize,
    },
    /// The applied target of constructor discovery has the wrong class arity.
    ConstructorTargetGenericArityMismatch {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Required argument or parameter count.
        expected: usize,
        /// Observed argument or parameter count.
        actual: usize,
    },
    /// A generic source class was used without receiver type arguments.
    RawGenericSourceReceiverUnsupported {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
        /// Required count required by this shape.
        expected: usize,
    },
    /// External generic argument order is not modeled yet.
    ExternalGenericInstantiationDeferred {
        /// Class declaration identity being inspected in the supplied session.
        class: SymbolId,
    },
    /// A completed member info could not be obtained for adaptation.
    MemberTypeUnavailable {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// The bounded, symbol-exact semantic substitution failed.
    TypeSubstitution(dotty_core::TypeRebindError),
    /// Receiver normalization failed before a nominal view could be read.
    TypeNormalization(crate::types::TypeNormalizeError),
    /// A source tree already has a different typed replacement recorded.
    ConflictingTypedExpression {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Previously recorded value retained by the failed operation.
        existing: u32,
        /// New value rejected because it conflicts with the existing entry.
        attempted: u32,
    },
    /// A source PatDef already has a different one-to-many typed expansion.
    PatDefExpansionConflict {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// Pattern typing introduced user bindings even though a PatDef's binder
    /// inventory reported a different count.
    PatDefBinderInventoryConflict {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Required count required by this shape.
        expected: usize,
        /// Observed count recorded in this shape.
        actual: usize,
    },
    /// A multi-binding PatDef exceeds the supported canonical TupleN bound.
    PatDefAggregateArityDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Number of arguments, parameters, or components in the inspected shape.
        arity: usize,
        /// Largest value supported by this operation.
        max_supported: usize,
    },
    /// A local pattern definition is outside the bounded PatDef subset.
    LocalPatDefDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Stable diagnostic label for the unsupported syntax or type shape.
        kind: &'static str,
    },
    /// String constants do not have a canonical source type in this session yet.
    StringLiteralTypingDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// Null constants do not have a canonical source type in this session yet.
    NullLiteralTypingDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// An integer spelling is invalid or outside the supported Scala Int range.
    IntegerLiteralOutOfRange {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Original source spelling retained for the diagnostic.
        spelling: String,
    },
    /// A decimal or exponent literal is not representable as an IEEE Double.
    FloatingLiteralInvalid {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Original source spelling retained for the diagnostic.
        spelling: String,
    },
    /// The selected `this` qualifier is not an enclosing class-like owner.
    ThisOwnerNotEnclosing {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Qualifier of the selected member or reference.
        qualifier: Option<dotty_core::Name>,
        /// Semantic owner of the declaration; ownership is distinct from lookup scope.
        owner: SymbolId,
    },
    /// The semantic owner chain used by `this` contains a cycle.
    ThisOwnerCycle {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Semantic owner of the declaration; ownership is distinct from lookup scope.
        owner: SymbolId,
    },
    /// No term declaration named this expression was visible.
    TermNameNotFound {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Name carried by the declaration or reference.
        name: dotty_core::Name,
        /// Optional source location; absent for nodes without source provenance.
        position: Option<SourceSpan>,
    },
    /// More than one term declaration matched in the same lexical scope.
    AmbiguousTermReference {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Name carried by the declaration or reference.
        name: dotty_core::Name,
        /// Optional source location; absent for nodes without source provenance.
        position: Option<SourceSpan>,
    },
    /// A method reference has several overloads and is not being selected yet.
    OverloadedReferenceDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Name carried by the declaration or reference.
        name: dotty_core::Name,
    },
    /// An object term needs a module receiver model that this slice does not have.
    ObjectTermReferenceDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// A source object has no matching derived ModuleClass identity.
    ObjectModuleClassUnavailable {
        /// Module value identity in the canonical session symbol table.
        object: SymbolId,
    },
    /// The configured resolver rejected or could not validate an external
    /// Object's module identity.
    ObjectModuleClassResolution {
        /// Module value identity in the canonical session symbol table.
        object: SymbolId,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: ResolutionError,
    },
    /// The resolved term symbol kind is outside the supported value subset.
    UnsupportedTermReference {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Stable declaration category, independent of flags and visibility.
        kind: SymbolKind,
    },
    /// No member with this term name exists on the receiver.
    MemberNotFound {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Semantic receiver type for the member or intrinsic call.
        receiver: TypeId,
        /// Name carried by the declaration or reference.
        name: dotty_core::Name,
    },
    /// Universal equality is defined, but its two operand types are
    /// statically disjoint under the supported Scala 3.9 equality rules.
    IncompatibleEqualityOperands {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Left operand of the enclosing relation or syntax node.
        left: TypeId,
        /// Right operand of the enclosing relation or syntax node.
        right: TypeId,
    },
    /// Several member candidates remain after lookup; overload resolution is deferred.
    OverloadedSelectionDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Name carried by the declaration or reference.
        name: dotty_core::Name,
    },
    /// A regular application callee does not widen to a method type.
    ApplicationCalleeNotMethod {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
    },
    /// A supported polymorphic application leaves one type parameter without
    /// constraints from its term arguments.
    UnconstrainedTypeParameter {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Enclosing type binder whose parameters are referenced.
        binder: TypeId,
        /// Zero-based position in the corresponding ordered parameter, argument, or clause list.
        parameter_index: usize,
    },
    /// Two actual arguments constrain one Poly parameter to non-equivalent types.
    ConflictingInferenceConstraints {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Enclosing type binder whose parameters are referenced.
        binder: TypeId,
        /// Zero-based position in the corresponding ordered parameter, argument, or clause list.
        parameter_index: usize,
        /// First type encountered while checking this relationship.
        first: TypeId,
        /// Second type encountered while checking this relationship.
        second: TypeId,
    },
    /// A generic application uses a formal/actual shape outside the structural
    /// inference rules supported by this typer increment.
    UnsupportedInferenceShape {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Enclosing type binder whose parameters are referenced.
        binder: TypeId,
        /// Zero-based position in the corresponding ordered parameter, argument, or clause list.
        parameter_index: Option<usize>,
        /// Formal parameter identity; this is not inferred from its name.
        formal: TypeId,
        /// Observed semantic type identity in the caller’s type arena.
        actual: TypeId,
    },
    /// A unique polymorphic callee is not the supported one-clause Poly/Method form.
    UnsupportedPolymorphicApplicationShape {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Enclosing type binder whose parameters are referenced.
        binder: TypeId,
    },
    /// Candidate competition includes a polymorphic method and needs general
    /// generic overload resolution.
    GenericOverloadResolutionDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// Inferred arguments violate the declared bounds of a Poly parameter.
    InferredTypeArgumentBoundViolation {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Zero-based position in the corresponding ordered parameter, argument, or clause list.
        parameter_index: usize,
        /// Semantic type of the argument being checked.
        argument: TypeId,
        /// Semantic bound in the canonical type arena.
        bound: TypeId,
        /// Bound or producer side to which this failure applies.
        side: TypeArgumentBoundSide,
    },
    /// The inferred argument uses bounds this typer cannot validate soundly.
    UnsupportedInferredTypeArgumentBounds {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Zero-based position in the corresponding ordered parameter, argument, or clause list.
        parameter_index: usize,
        /// Semantic type of the argument being checked.
        argument: TypeId,
        /// Lower and upper bound representation for this parameter.
        bounds: TypeId,
    },
    /// A subtype relation needed to validate an inferred argument's bounds is
    /// outside the current relation subset.
    InferredTypeArgumentBoundCheckUnsupported {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Zero-based position in the corresponding ordered parameter, argument, or clause list.
        parameter_index: usize,
        /// Semantic type of the argument being checked.
        argument: TypeId,
        /// Semantic bound in the canonical type arena.
        bound: TypeId,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: Box<TypeRelationError>,
    },
    /// An explicit type application callee does not widen to a Poly type.
    ExplicitTypeApplicationCalleeNotPoly {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
    },
    /// An explicit type application has a different number of type arguments
    /// than the callee's Poly binder.
    ExplicitTypeApplicationArityMismatch {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Required argument or parameter count.
        expected: usize,
        /// Observed argument or parameter count.
        actual: usize,
    },
    /// An explicit type argument violates one side of its declared bounds.
    ExplicitTypeArgumentBoundViolation {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Zero-based position in the corresponding ordered parameter, argument, or clause list.
        parameter_index: usize,
        /// Semantic type of the argument being checked.
        argument: TypeId,
        /// Semantic bound in the canonical type arena.
        bound: TypeId,
        /// Bound or producer side to which this failure applies.
        side: TypeArgumentBoundSide,
    },
    /// The Poly parameter uses bounds not supported by explicit application.
    UnsupportedExplicitTypeArgumentBounds {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Zero-based position in the corresponding ordered parameter, argument, or clause list.
        parameter_index: usize,
        /// Lower and upper bound representation for this parameter.
        bounds: TypeId,
    },
    /// The subtype relation could not decide an explicit type argument bound.
    ExplicitTypeArgumentBoundCheckUnsupported {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Zero-based position in the corresponding ordered parameter, argument, or clause list.
        parameter_index: usize,
        /// Semantic type of the argument being checked.
        argument: TypeId,
        /// Semantic bound in the canonical type arena.
        bound: TypeId,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: Box<TypeRelationError>,
    },
    /// Type application on an overloaded callee remains deferred.
    OverloadedTypeApplicationDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// Binder-exact semantic Poly instantiation failed.
    PolyInstantiation(dotty_core::TypeRebindError),
    /// A projected source type form cannot be represented as a typed type tree.
    TypeArgumentTreeCannotBeReified {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// AST node category encountered at the failure site.
        tree_kind: &'static str,
    },
    /// A projected type ascription tree cannot be represented by a typed TypeTree.
    TypeAscriptionTreeCannotBeReified {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// AST node category encountered at the failure site.
        tree_kind: &'static str,
    },
    /// The application syntax does not match the method clause kind.
    ApplicationMethodKindMismatch {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Ordinary or explicit contextual application syntax.
        application_kind: ApplyKind,
        /// Ordinary, implicit, or contextual method-clause kind.
        method_kind: MethodKind,
    },
    /// Raw generic constructor inference currently supports only plain clauses.
    UnsupportedApplicationMethodKind {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Whether this parameter clause is ordinary, implicit, or contextual.
        kind: MethodKind,
    },
    /// No eligible contextual value conforms to the formal parameter.
    MissingContextualArgument {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Required semantic type identity in the caller’s type arena.
        expected: TypeId,
    },
    /// A recursive provider revisited the same goal.
    DivergentContextualArgument {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Required semantic type identity in the caller’s type arena.
        expected: TypeId,
    },
    /// Context argument search exceeded its nesting or work budget.
    ContextualSearchLimit {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// Multiple contextual values at the same priority conform to the formal.
    AmbiguousContextualArgument {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Required semantic type identity in the caller’s type arena.
        expected: TypeId,
        /// Maximal candidates in search order. A synthetic ID may expire on rollback.
        candidates: Vec<SymbolId>,
        /// Owner and member spellings in candidate order, retained across rollback.
        candidate_names: Vec<(Option<Name>, Name)>,
    },
    /// A contextual provider supplied extensions, but their overload resolution failed.
    ExtensionOverload {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Interned text identity and term/type namespace of the declaration or reference.
        name: Name,
        /// Contextual argument provider selected or examined during search.
        provider: SymbolId,
        /// Underlying failure that prevented this operation from completing.
        cause: Box<TyperError>,
    },
    /// A provider of the selected extension could not obtain its contextual argument.
    ExtensionProviderDependency {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Interned text identity and term/type namespace of the declaration or reference.
        name: Name,
        /// Contextual argument provider selected or examined during search.
        provider: SymbolId,
        /// Underlying failure that prevented this operation from completing.
        cause: Box<TyperError>,
    },
    /// A conversion exposing the selected member could not obtain its contextual argument.
    MemberConversionDependency {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Interned text identity and term/type namespace of the declaration or reference.
        name: Name,
        /// Contextual argument provider selected or examined during search.
        provider: SymbolId,
        /// Underlying failure that prevented this operation from completing.
        cause: Box<TyperError>,
    },
    /// Multiple eligible conversions expose the requested term on their result.
    AmbiguousMemberConversion {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Semantic receiver type for the member or intrinsic call.
        receiver: TypeId,
        /// Interned text identity and term/type namespace of the declaration or reference.
        name: Name,
        /// Candidates examined by lookup; preserves rejection details when available.
        candidates: Vec<SymbolId>,
    },
    /// A call requires contextual argument insertion or implicit search.
    UsingApplicationDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// The application supplies a different number of arguments than the method.
    ApplicationArityMismatch {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Required argument or parameter count.
        expected: usize,
        /// Observed argument or parameter count.
        actual: usize,
    },
    /// An erased parameter cannot be supplied by ordinary expression typing.
    ErasedApplicationParameterDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Zero-based position in the corresponding ordered parameter, argument, or clause list.
        parameter_index: usize,
    },
    /// A repeated parameter requires sequence adaptation outside this slice.
    VarargsApplicationParameterDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Zero-based position in the corresponding ordered parameter, argument, or clause list.
        parameter_index: usize,
    },
    /// A repeated parameter reference needs its concrete sequence type.
    VarargsParameterReferenceDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// A by-name parameter requires delayed evaluation outside this slice.
    ByNameApplicationParameterDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Zero-based position in the corresponding ordered parameter, argument, or clause list.
        parameter_index: usize,
    },
    /// The method result refers to a term parameter of the method being called.
    DependentMethodApplicationDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Enclosing type binder whose parameters are referenced.
        binder: TypeId,
        /// Result type in the same arena; further parameter clauses are represented by nested
        /// callable types.
        result: TypeId,
    },
    /// An argument's widened type does not conform to the corresponding formal.
    ApplicationArgumentTypeMismatch {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Zero-based position in the corresponding ordered parameter, argument, or clause list.
        argument_index: usize,
        /// Argument type at failure. Rollback can invalidate or reuse this slot.
        actual: TypeId,
        /// Required type at failure, with the same rollback restrictions as `actual`.
        expected: TypeId,
        /// Owned argument type description captured before rollback.
        actual_description: Box<str>,
        /// Owned parameter type description captured before rollback.
        expected_description: Box<str>,
    },
    /// The supported conformance relation cannot decide an argument comparison.
    ApplicationArgumentConformanceUnsupported {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Zero-based position in the corresponding ordered parameter, argument, or clause list.
        argument_index: usize,
        /// Observed semantic type identity in the caller’s type arena.
        actual: TypeId,
        /// Required semantic type identity in the caller’s type arena.
        expected: TypeId,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: Box<TypeRelationError>,
    },
    /// The widened expression type does not conform to its expected type.
    ExpectedExpressionTypeMismatch {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Observed semantic type identity in the caller’s type arena.
        actual: TypeId,
        /// Required semantic type identity in the caller’s type arena.
        expected: TypeId,
    },
    /// The supported conformance relation cannot decide an expected-type comparison.
    ExpectedExpressionConformanceUnsupported {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Observed semantic type identity in the caller’s type arena.
        actual: TypeId,
        /// Required semantic type identity in the caller’s type arena.
        expected: TypeId,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: Box<TypeRelationError>,
    },
    /// An assignment's typed left-hand side is not a direct symbol reference.
    AssignmentLhsNotAssignable {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        lhs_tree_index: u32,
        /// Expression category encountered at the failure site.
        expression_kind: &'static str,
    },
    /// An assignment target is a value-like declaration without MUTABLE.
    AssignmentTargetImmutable {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Binding or control-flow destination selected by this node.
        target: SymbolId,
    },
    /// This semantic symbol category is not writable by ordinary assignment.
    AssignmentTargetKindUnsupported {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Binding or control-flow destination selected by this node.
        target: SymbolId,
        /// Stable declaration category, independent of flags and visibility.
        kind: SymbolKind,
    },
    /// The writable type of the selected declaration could not be recovered.
    WritableAssignmentTypeUnavailable {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Binding or control-flow destination selected by this node.
        target: SymbolId,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: Box<TyperError>,
    },
    /// The condition of an if expression does not conform to canonical Boolean.
    IfConditionTypeMismatch {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Observed semantic type identity in the caller’s type arena.
        actual: TypeId,
        /// Required semantic type identity in the caller’s type arena.
        expected: TypeId,
    },
    /// The supported relation cannot decide if-condition conformance.
    IfConditionConformanceUnsupported {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Observed semantic type identity in the caller’s type arena.
        actual: TypeId,
        /// Required semantic type identity in the caller’s type arena.
        expected: TypeId,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: Box<TypeRelationError>,
    },
    /// A branch expression type could not be widened for the minimal join.
    IfBranchTypeCannotBeWidened {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        branch_tree_index: u32,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: Box<TyperError>,
    },
    /// The minimal branch join could not soundly decide a subtype relation.
    IfBranchJoinUnsupported {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Left operand of the enclosing relation or syntax node.
        left: TypeId,
        /// Right operand of the enclosing relation or syntax node.
        right: TypeId,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: Box<TypeRelationError>,
    },
    /// An if expression references a missing condition or branch tree.
    IfChildTreeOutsideArena {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        child_tree_index: u32,
        /// Role of this item in the source or semantic validation contract.
        role: &'static str,
    },
    /// The condition of a while expression does not conform to canonical Boolean.
    WhileConditionTypeMismatch {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Observed semantic type identity in the caller’s type arena.
        actual: TypeId,
        /// Required semantic type identity in the caller’s type arena.
        expected: TypeId,
    },
    /// The supported relation cannot decide while-condition conformance.
    WhileConditionConformanceUnsupported {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Observed semantic type identity in the caller’s type arena.
        actual: TypeId,
        /// Required semantic type identity in the caller’s type arena.
        expected: TypeId,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: Box<TypeRelationError>,
    },
    /// A while expression references a missing condition or body tree.
    WhileChildTreeOutsideArena {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        child_tree_index: u32,
        /// Role of this item in the source or semantic validation contract.
        role: &'static str,
    },
    /// A return is not nested in a source method supported by this increment.
    ReturnOutsideSupportedMethod {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Semantic owner of the declaration; ownership is distinct from lookup scope.
        owner: SymbolId,
    },
    /// The enclosing method's source definition cannot be recovered safely.
    ReturnMethodProvenanceMalformed {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// Returns cannot participate in inferred method-result computation yet.
    ReturnInInferredResultMethodDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// A return target is missing or is not a method/label reference tree.
    MalformedReturnTarget {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        target_tree_index: u32,
    },
    /// Labeled or non-local return targets are outside the current source slice.
    NonLocalReturnDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        target_tree_index: u32,
    },
    /// The explicitly typed return value does not conform to the method result.
    ReturnExpressionTypeMismatch {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Observed semantic type identity in the caller’s type arena.
        actual: TypeId,
        /// Required semantic type identity in the caller’s type arena.
        expected: TypeId,
    },
    /// The supported relation cannot decide return-value conformance.
    ReturnExpressionConformanceUnsupported {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Observed semantic type identity in the caller’s type arena.
        actual: TypeId,
        /// Required semantic type identity in the caller’s type arena.
        expected: TypeId,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: Box<TypeRelationError>,
    },
    /// The owner chain used by a return is cyclic or references a missing owner.
    MalformedReturnOwnerChain {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Semantic owner of the declaration; ownership is distinct from lookup scope.
        owner: SymbolId,
    },
    /// The enclosing method signature has an invalid methodic result chain.
    MalformedReturnMethodSignature {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Optional generic signature or semantic callable signature.
        signature: TypeId,
    },
    /// Every overload rejected the supplied arguments; reasons are kept per candidate.
    OverloadApplicationNoApplicable {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Candidates examined by lookup; preserves rejection details when available.
        candidates: Vec<(SymbolId, OverloadRejection)>,
    },
    /// Several supported candidates apply, but this increment has not selected a unique best one.
    AmbiguousOverloadApplication {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Candidates examined by lookup; preserves rejection details when available.
        candidates: Vec<SymbolId>,
    },
    /// An unsupported overload could compete with supported candidates.
    OverloadResolutionRequiresUnsupportedCandidate {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Declaration considered by lookup or overload resolution.
        candidate: SymbolId,
    },
    /// A method symbol has an incomplete or unsupported callable shape.
    MalformedOverloadCandidate {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Semantic callable type whose parameter clauses are being inspected.
        callable: TypeId,
    },
    /// The application name bucket mixes methods and non-method terms.
    MixedApplicationCandidateKinds {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Candidates examined by lookup; preserves rejection details when available.
        candidates: Vec<SymbolId>,
    },
    /// A type selection is outside expression typing.
    TypeSelectionInExpression {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// Explicit `super` is limited to an unqualified direct class parent and
    /// one public, concrete, monomorphic method declared by that parent.
    UnsupportedSuper {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Detailed reason for rejecting this input or dependency.
        reason: &'static str,
    },
    /// Member discovery failed with a typed lookup error.
    MemberLookup(Box<MemberLookupError>),
    /// The source expression form is outside this issue's supported subset.
    UnsupportedExpression {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Expression category encountered at the failure site.
        expression_kind: &'static str,
    },
    /// A function literal parameter is outside the explicitly typed subset.
    UnsupportedFunctionLiteralParameter {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Detailed reason for rejecting this input or dependency.
        reason: &'static str,
    },
    /// A function literal references an outer value requiring closure capture.
    FunctionLiteralCaptureUnsupported {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: Option<SymbolId>,
    },
    /// The source pattern root is not supported by the current pattern subset.
    UnsupportedPattern {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Source shape of the pattern that could not be typed.
        pattern_kind: PatternKind,
    },
    /// The tuple pattern's canonical TupleN class, companion, or extractor
    /// could not be resolved or instantiated soundly.
    TuplePatternResolutionDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Number of arguments, parameters, or components in the inspected shape.
        arity: usize,
        /// Specific structural or semantic reason for the enclosing rejection.
        issue: TuplePatternResolutionIssue,
    },
    /// The selector and canonical tuple input type are disjoint under the
    /// supported bounded relation.
    TuplePatternTypeMismatch {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Scrutinee expression or member selector for this construct.
        selector: TypeId,
        /// Semantic tuple type in the canonical type arena.
        tuple_type: TypeId,
    },
    /// The selector's relation to the canonical tuple input could not be
    /// decided by the bounded type relation.
    TuplePatternRelationDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Scrutinee expression or member selector for this construct.
        selector: TypeId,
        /// Semantic tuple type in the canonical type arena.
        tuple_type: TypeId,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: Box<TypeRelationError>,
    },
    /// General infix extractor lowering remains deferred.
    InfixPatternDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// Pattern alternatives cannot introduce bindings in any branch.
    PatternBindingInAlternative {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        branch_tree_index: u32,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        binding_tree_index: u32,
    },
    /// The bounded relation could not compute a common type for alternative branches.
    PatternAlternativeJoinUnsupported {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Left operand of the enclosing relation or syntax node.
        left: TypeId,
        /// Right operand of the enclosing relation or syntax node.
        right: TypeId,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: Box<TypeRelationError>,
    },
    /// A simple extractor identifier did not resolve in the current context.
    ExtractorQualifierNotFound {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Interned text identity and term/type namespace of the declaration or reference.
        name: Name,
    },
    /// A selected extractor qualifier member does not exist.
    ExtractorQualifierMemberNotFound {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Interned text identity and term/type namespace of the declaration or reference.
        name: Name,
    },
    /// A selected extractor qualifier resolves to multiple members.
    ExtractorQualifierMemberAmbiguous {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Interned text identity and term/type namespace of the declaration or reference.
        name: Name,
    },
    /// The extractor application function uses an unsupported source-tree shape.
    ExtractorQualifierShapeUnsupported {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// The resolved qualifier is not a symbol-backed value reference.
    ExtractorQualifierNotValueLike {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Type of the member-selection qualifier.
        qualifier_type: Option<TypeId>,
    },
    /// The resolved extractor qualifier is mutable or otherwise unstable.
    ExtractorQualifierNotStable {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Type of the member-selection qualifier.
        qualifier_type: TypeId,
    },
    /// No member named `unapply` exists on the extractor value type.
    ExtractorUnapplyNotFound {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Extractor declaration being resolved for this pattern.
        extractor: SymbolId,
    },
    /// Several `unapply` declarations remain; overload selection is deferred.
    ExtractorUnapplyOverloaded {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Extractor declaration being resolved for this pattern.
        extractor: SymbolId,
        /// Candidates examined by lookup; preserves rejection details when available.
        candidates: Vec<SymbolId>,
    },
    /// Generic `unapply` methods are outside this extractor increment.
    ExtractorUnapplyPolymorphic {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Extractor method identity used for the pattern.
        unapply: SymbolId,
        /// Semantic callable type whose parameter clauses are being inspected.
        callable: TypeId,
    },
    /// The method does not have the supported one-parameter plain shape.
    ExtractorUnapplyShapeUnsupported {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Extractor method identity used for the pattern.
        unapply: SymbolId,
        /// Semantic callable type whose parameter clauses are being inspected.
        callable: TypeId,
        /// Specific structural or semantic reason for the enclosing rejection.
        issue: ExtractorMethodShapeIssue,
    },
    /// The selector is not proven to conform to the unapply input type.
    ExtractorPatternConstraintDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Scrutinee expression or member selector for this construct.
        selector: TypeId,
        /// Input type passed to normalization or the enclosing operation.
        input: TypeId,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: Option<Box<TypeRelationError>>,
    },
    /// The unapply result is malformed or methodic before result protocols apply.
    UnsupportedExtractorResultProtocol {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Extractor method identity used for the pattern.
        unapply: SymbolId,
        /// Result type in the same arena; further parameter clauses are represented by nested
        /// callable types.
        result: TypeId,
    },
    /// An extractor pattern has an unsupported number of nested patterns.
    ExtractorPatternArityUnsupported {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Extractor method identity used for the pattern.
        unapply: SymbolId,
        /// Observed argument or parameter count.
        actual: usize,
    },
    /// A Boolean-result extractor pattern must not bind nested patterns.
    BooleanExtractorPatternArityUnsupported {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Extractor method identity used for the pattern.
        unapply: SymbolId,
        /// Observed argument or parameter count.
        actual: usize,
    },
    /// A required Option-like result member is absent.
    ExtractorResultMemberNotFound {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Result type in the same arena; further parameter clauses are represented by nested
        /// callable types.
        result: TypeId,
        /// Namespace-aware member name used for lookup.
        member: Name,
    },
    /// A required Option-like result member has multiple visible declarations.
    ExtractorResultMemberOverloaded {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Result type in the same arena; further parameter clauses are represented by nested
        /// callable types.
        result: TypeId,
        /// Namespace-aware member name used for lookup.
        member: Name,
        /// Candidates examined by lookup; preserves rejection details when available.
        candidates: Vec<SymbolId>,
    },
    /// A required Option-like result member is not a supported parameterless value.
    ExtractorResultMemberUnsupported {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Result type in the same arena; further parameter clauses are represented by nested
        /// callable types.
        result: TypeId,
        /// Namespace-aware member name used for lookup.
        member: Name,
        /// Type of the selected member.
        member_type: TypeId,
        /// Specific structural or semantic reason for the enclosing rejection.
        issue: ExtractorResultMemberIssue,
    },
    /// The product result does not expose selectors matching the source arity.
    ExtractorProductSelectorCountMismatch {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Extractor method identity used for the pattern.
        unapply: SymbolId,
        /// Result type in the same arena; further parameter clauses are represented by nested
        /// callable types.
        result: TypeId,
        /// Import selectors or selected names in source order.
        selectors: Vec<dotty_core::Name>,
    },
    /// The extractor result does not expose the supported product selector protocol.
    UnsupportedExtractorProductProtocol {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Extractor method identity used for the pattern.
        unapply: SymbolId,
        /// Result type in the same arena; further parameter clauses are represented by nested
        /// callable types.
        result: TypeId,
        /// Specific structural or semantic reason for the enclosing rejection.
        issue: ExtractorProductIssue,
    },
    /// Named arguments and sequence wildcards are not accepted by this plan.
    ExtractorPatternArgumentUnsupported {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        argument_tree_index: u32,
        /// Specific structural or semantic reason for the enclosing rejection.
        issue: ExtractorPatternArgumentIssue,
    },
    /// An identifier pattern is not a well-formed term-name pattern.
    MalformedVariablePattern {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// An explicit Bind currently supports only a wildcard body.
    UnsupportedBindPatternBody {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// A literal or stable value pattern cannot match the selector prototype.
    PatternTypeMismatch {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Observed semantic type identity in the caller’s type arena.
        actual: TypeId,
        /// Scrutinee expression or member selector for this construct.
        selector: TypeId,
    },
    /// The bounded type relation cannot decide pattern/selector compatibility.
    PatternTypeRelationDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Observed semantic type identity in the caller’s type arena.
        actual: TypeId,
        /// Scrutinee expression or member selector for this construct.
        selector: TypeId,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: Box<TypeRelationError>,
    },
    /// The selector and source typed-pattern type cannot match under the current relation.
    TypedPatternTypeMismatch {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Scrutinee expression or member selector for this construct.
        selector: TypeId,
        /// Type used to check the pattern.
        pattern_type: TypeId,
    },
    /// The bounded relation cannot decide whether a typed pattern can match.
    TypedPatternRelationDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Scrutinee expression or member selector for this construct.
        selector: TypeId,
        /// Type used to check the pattern.
        pattern_type: TypeId,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: Box<TypeRelationError>,
    },
    /// Runtime testing for the projected typed-pattern type is not represented yet.
    TypedPatternRuntimeTestDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Type used to check the pattern.
        pattern_type: TypeId,
        /// Detailed reason for rejecting this input or dependency.
        reason: &'static str,
    },
    /// A stable-pattern root does not retain a well-formed selected term symbol.
    MalformedStablePatternTarget {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// A resolved pattern value is mutable or otherwise not a stable term.
    UnstablePatternValue {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// A source case tree is missing or does not have the required `CaseDef` shape.
    MalformedCaseDef {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Category actually encountered in the input.
        actual_kind: &'static str,
    },
    /// A source pattern binding is malformed or does not match the requested name.
    MalformedPatternBinding {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// The wildcard spelling cannot introduce a pattern-bound symbol.
    WildcardPatternBindingRejected {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// Pattern symbols may only be entered while a case scope is active.
    PatternBindingOutsideCaseScope {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// A repeated source binding was presented with conflicting case ownership.
    PatternBindingScopeConflict {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Scope already associated with this binding.
        existing_scope: dotty_core::ScopeId,
        /// Scope requested by the rejected binding.
        attempted_scope: dotty_core::ScopeId,
    },
    /// Two pattern bindings with the same name occur in one case scope.
    DuplicatePatternBinding {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Name carried by the declaration or reference.
        name: dotty_core::Name,
    },
    /// A source Match has no cases, so no result type can be derived.
    EmptyMatchCases {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// The selector type could not be adapted to a pattern prototype.
    MatchSelectorTypeCannotBeAdapted {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: Box<TyperError>,
    },
    /// A case body's type could not be widened for the bounded result join.
    MatchCaseResultTypeCannotBeWidened {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        case_tree_index: u32,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: Box<TyperError>,
    },
    /// The bounded result join cannot decide a relation between two cases.
    MatchCaseJoinUnsupported {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Left operand of the enclosing relation or syntax node.
        left: TypeId,
        /// Right operand of the enclosing relation or syntax node.
        right: TypeId,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: Box<TypeRelationError>,
    },
    /// A block statement changes the local declaration or import environment.
    LocalBlockDeclarationDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Stable diagnostic label for the unsupported syntax or type shape.
        kind: &'static str,
    },
    /// A local value modifier has no bounded symbol policy or its semantics
    /// are intentionally deferred to a later typing increment.
    LocalValueModifierDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Source modifiers retained on this syntax node.
        modifiers: Vec<dotty_core::ast::Modifier>,
        /// Classifier result for the input shape being rejected.
        classification: &'static str,
    },
    /// A local value carries source visibility or annotations not preserved by
    /// the bounded typed local-value representation.
    LocalValueMetadataDeferred {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Source or semantic visibility of the declaration.
        visibility: Option<dotty_core::ast::VisibilitySyntax>,
        /// Number of annotation entries encountered.
        annotation_count: usize,
    },
    /// An inferred local initializer widens to a type that cannot be a value info.
    InvalidInferredLocalValueType {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Inferred type produced while checking this construct.
        inferred: TypeId,
    },
    /// A local value declaration has no initializer.
    LocalValueRightHandSideMissing {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// A local value initializer refers to the local value currently being initialized.
    RecursiveLocalValueInitializer {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// A recursive local lazy value requires inference/initialization
    /// semantics beyond the bounded declaration typer.
    RecursiveLazyLocalValueInitializer {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
    },
    /// Completing a forward chain of inferred local lazy values exceeded the
    /// bounded completion depth.
    InferredLocalLazyTypeDepthExceeded {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Configured maximum recursive traversal depth.
        max_depth: usize,
    },
    /// Local value declarations are only supported as statements in a block.
    LocalValueOutsideBlock {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
    },
    /// Two local values with the same name occur in one block scope.
    DuplicateLocalValue {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Name carried by the declaration or reference.
        name: dotty_core::Name,
    },
    /// A local value's widened initializer type does not conform to its annotation.
    LocalValueTypeMismatch {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Observed semantic type identity in the caller’s type arena.
        actual: TypeId,
        /// Required semantic type identity in the caller’s type arena.
        expected: TypeId,
    },
    /// The supported conformance relation cannot decide a local value comparison.
    LocalValueConformanceUnsupported {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Observed semantic type identity in the caller’s type arena.
        actual: TypeId,
        /// Required semantic type identity in the caller’s type arena.
        expected: TypeId,
        /// Underlying typed failure; inspect it for the more specific cause.
        error: Box<TypeRelationError>,
    },
    /// The semantic type is not yet supported by expression widening.
    ExpressionTypeCannotBeWidened {
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
    },
    /// A term reference designates a symbol category that is not widenable.
    TermReferenceCannotBeWidened {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Stable declaration category, independent of flags and visibility.
        kind: SymbolKind,
    },
    /// The receiver prefix does not contain the referenced member symbol.
    TermReferencePrefixMismatch {
        /// Declaration identity involved in the operation, relative to the caller’s symbol table.
        symbol: SymbolId,
        /// Semantic prefix through which the member is selected.
        prefix: TypeId,
    },
    /// A selection qualifier cannot be preserved as a stable reference path.
    UnstableSelectionPrefix {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Type of the member-selection qualifier.
        qualifier_type: TypeId,
    },
    /// A singleton type's reference is outside the supported stable path forms.
    UnsupportedSingletonReference {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Method-handle invocation category or referenced semantic kind.
        reference_kind: &'static str,
    },
    /// A literal singleton uses a constant kind not supported by source syntax.
    UnsupportedSingletonLiteralKind {
        /// Source buffer identity in the current compilation session.
        source: SourceId,
        /// Zero-based index of the relevant tree in the original untyped source arena.
        tree_index: u32,
        /// Source literal category involved in the rejection.
        literal_kind: &'static str,
    },
}

/// Which side of an ordinary type-parameter bound an explicit argument broke.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeArgumentBoundSide {
    /// The argument was not above the declared lower bound.
    Lower,
    /// The argument was not below the declared upper bound.
    Upper,
}

impl fmt::Display for TyperError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for TyperError {}

#[cfg(test)]
mod tests {
    use super::PatternKind;

    #[test]
    fn pattern_kind_labels_are_stable_and_descriptive() {
        assert_eq!(PatternKind::Identifier.as_str(), "identifier");
        assert_eq!(PatternKind::Literal.as_str(), "literal");
        assert_eq!(PatternKind::Typed.as_str(), "typed pattern");
        assert_eq!(PatternKind::Alternative.as_str(), "alternative");
        assert_eq!(PatternKind::Application.as_str(), "application pattern");
        assert_eq!(PatternKind::Other.as_str(), "other pattern");
    }
}
