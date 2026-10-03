//! Public typer error and diagnostic payload types.

use std::fmt;

use super::*;

/// Stable source-shape category for an unsupported pattern root.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PatternKind {
    Identifier,
    StableSelection,
    Literal,
    Typed,
    Alternative,
    Application,
    TypeApplication,
    Binding,
    Extractor,
    Tuple,
    Infix,
    Parenthesized,
    Other,
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
        symbol: SymbolId,
    },
    /// No source provenance is indexed for this symbol.
    SourceProvenanceMissing {
        symbol: SymbolId,
    },
    /// A declaration that needs lexical lookup has no source lexical context.
    DeclarationContextMissing {
        symbol: SymbolId,
    },
    /// A source context ID is outside the naming result that produced it.
    SourceContextMissing {
        source: SourceId,
        tree_index: u32,
        context_index: u32,
    },
    /// The requested expression-context owner is not present in the semantic store.
    ExpressionOwnerMissing {
        owner: SymbolId,
    },
    /// This declaration kind cannot own an expression body context.
    ExpressionOwnerKindUnsupported {
        owner: SymbolId,
        kind: SymbolKind,
    },
    /// A method or constructor has no indexed lexical scope for its body.
    ExpressionMethodScopeMissing {
        owner: SymbolId,
    },
    /// The recorded method body scope is owned by a different symbol.
    ExpressionMethodScopeOwnerMismatch {
        owner: SymbolId,
        scope: dotty_core::ScopeId,
        actual: Option<SymbolId>,
    },
    /// A source context stored on an expression owner is outside this index.
    ExpressionOwnerSourceContextMissing {
        owner: SymbolId,
        context: SourceContextId,
    },
    /// No source context is indexed for this expression owner.
    ExpressionOwnerDeclarationContextMissing {
        owner: SymbolId,
    },
    /// A local expression scope is outside the semantic store's scope arena.
    ExpressionLocalScopeMissing {
        scope: dotty_core::ScopeId,
    },
    /// A local-scope handle is malformed for this typer's stack.
    ExpressionLocalScopeStackMissing {
        stack: ExpressionScopeId,
    },
    /// A local-scope handle was created by a different `SourceTyper`.
    ExpressionLocalScopeStackForeign {
        stack: ExpressionScopeId,
    },
    /// The type name may come from an import form not supported by this pass.
    UnsupportedImportContext {
        source: SourceId,
        context_index: u32,
        import_tree_index: u32,
    },
    /// An import qualifier does not resolve to an entered package or scope.
    ImportQualifierNotFound {
        source: SourceId,
        import_tree_index: u32,
    },
    /// A local import qualifier did not resolve to a supported stable prefix.
    ImportQualifierNotStable {
        source: SourceId,
        import_tree_index: u32,
        symbol: SymbolId,
    },
    /// An import tree has an unexpected AST shape or stale child reference.
    MalformedSourceImport {
        source: SourceId,
        import_tree_index: u32,
    },
    /// Completion for this semantic declaration category is not implemented.
    UnsupportedSymbolCompletion {
        symbol: SymbolId,
        kind: SymbolKind,
    },
    /// A source class-like declaration has no declaration scope from naming.
    MissingClassScope {
        symbol: SymbolId,
    },
    /// A class-like symbol already contains an incompatible complete type.
    MalformedClassInfo {
        symbol: SymbolId,
        info: TypeId,
    },
    /// A `new` type tree is a template; anonymous-class lowering is deferred.
    AnonymousClassInstantiationDeferred {
        source: SourceId,
        tree_index: u32,
    },
    /// The projected type does not resolve to a named class-like symbol.
    NewTargetNotClass {
        ty: TypeId,
    },
    /// Traits cannot be instantiated with `new`.
    TraitInstantiation {
        symbol: SymbolId,
    },
    /// Abstract classes cannot be instantiated with `new`.
    AbstractClassInstantiation {
        symbol: SymbolId,
    },
    /// Module classes and objects cannot be instantiated with `new`.
    ModuleInstantiation {
        symbol: SymbolId,
    },
    /// Packages cannot be instantiated with `new`.
    PackageInstantiation {
        symbol: SymbolId,
    },
    /// A type parameter is not proven to be an instantiable concrete class.
    TypeParameterInstantiation {
        symbol: SymbolId,
    },
    /// Class metadata needed for construction is absent or incomplete.
    NewClassInfoUnavailable {
        symbol: SymbolId,
    },
    /// Constructor lookup normalization failed or exceeded its bound.
    ConstructorLookupNormalization {
        ty: TypeId,
        error: crate::types::TypeNormalizeError,
    },
    /// The exact `<init>` scope bucket contains a non-constructor or invalid symbol.
    MalformedConstructorBucket {
        class: SymbolId,
        symbol: SymbolId,
    },
    /// A constructor bucket entry has a non-callable or invalid completed type.
    MalformedConstructorCandidate {
        symbol: SymbolId,
        callable: TypeId,
    },
    /// A template parent is not a type or a supported constructor-call shape.
    /// Constructor application found no constructor candidate for its class.
    ConstructorApplicationUnavailable {
        class: SymbolId,
    },
    /// Constructor application has multiple candidates; overload selection is deferred.
    ConstructorOverloadResolutionDeferred {
        source: SourceId,
        tree_index: u32,
        class: SymbolId,
        candidates: Vec<SymbolId>,
    },
    /// No direct constructor candidate accepts the current argument list.
    ConstructorApplicationNoApplicable {
        source: SourceId,
        tree_index: u32,
        class: SymbolId,
        candidates: Vec<(SymbolId, OverloadRejection)>,
    },
    /// Several direct constructors apply, but no unique most-specific candidate exists.
    AmbiguousConstructorApplication {
        source: SourceId,
        tree_index: u32,
        class: SymbolId,
        candidates: Vec<SymbolId>,
    },
    /// An unsupported direct constructor could compete with otherwise applicable candidates.
    ConstructorOverloadResolutionRequiresUnsupportedCandidate {
        source: SourceId,
        tree_index: u32,
        class: SymbolId,
        candidate: SymbolId,
    },
    /// Generic constructor application is deferred to explicit/inferred type arguments.
    ConstructorPolymorphicApplicationDeferred {
        source: SourceId,
        tree_index: u32,
    },
    /// Explicit class arguments require a polymorphic primary constructor.
    ConstructorCallableNotPolymorphic {
        source: SourceId,
        tree_index: u32,
        constructor: SymbolId,
        instance_type: TypeId,
    },
    /// Explicit constructor type arguments do not match the constructor binder.
    ConstructorTypeArgumentArityMismatch {
        source: SourceId,
        tree_index: u32,
        constructor: SymbolId,
        expected: usize,
        actual: usize,
    },
    /// The constructor owner is inconsistent with the class in the typed `New`.
    ConstructorInstanceClassMismatch {
        constructor: SymbolId,
        owner: SymbolId,
        instance_class: SymbolId,
    },
    /// Binder-exact constructor Poly instantiation failed.
    ConstructorPolyInstantiationFailed {
        constructor: SymbolId,
        error: dotty_core::TypeRebindError,
    },
    /// The instantiated constructor result disagrees with the typed `New` type.
    ConstructorResultTypeMismatch {
        source: SourceId,
        tree_index: u32,
        constructor: SymbolId,
        constructor_result: TypeId,
        instance_type: TypeId,
    },
    /// The supported subtype relation cannot compare the constructor result
    /// with the typed `New` instance type.
    ConstructorResultTypeCheckUnsupported {
        source: SourceId,
        tree_index: u32,
        constructor: SymbolId,
        constructor_result: TypeId,
        instance_type: TypeId,
        error: Box<TypeRelationError>,
    },
    /// An explicit constructor type argument violates one side of its bounds.
    ConstructorTypeArgumentBoundViolation {
        source: SourceId,
        tree_index: u32,
        constructor: SymbolId,
        parameter_index: usize,
        argument: TypeId,
        bound: TypeId,
        side: TypeArgumentBoundSide,
    },
    /// A constructor Poly parameter uses bounds not supported by this typer.
    UnsupportedConstructorTypeArgumentBounds {
        source: SourceId,
        tree_index: u32,
        constructor: SymbolId,
        parameter_index: usize,
        bounds: TypeId,
    },
    /// The supported subtype relation cannot check a constructor type argument bound.
    ConstructorTypeArgumentBoundCheckUnsupported {
        source: SourceId,
        tree_index: u32,
        constructor: SymbolId,
        parameter_index: usize,
        argument: TypeId,
        bound: TypeId,
        error: Box<TypeRelationError>,
    },
    /// A generic primary constructor parameter was not constrained by its arguments.
    UnconstrainedConstructorTypeParameter {
        source: SourceId,
        tree_index: u32,
        constructor: SymbolId,
        parameter_index: usize,
    },
    /// A raw generic `New` selected a secondary constructor whose owner type
    /// parameters cannot yet be inferred by overload resolution.
    UnsupportedRawGenericSecondaryConstructorInference {
        source: SourceId,
        tree_index: u32,
        constructor: SymbolId,
        owner: SymbolId,
    },
    /// Constructor arguments constrain one Poly parameter to non-equivalent types.
    ConflictingConstructorInferenceConstraints {
        source: SourceId,
        tree_index: u32,
        constructor: SymbolId,
        parameter_index: usize,
        first: TypeId,
        second: TypeId,
    },
    /// A constructor formal/actual pair is outside supported structural inference.
    UnsupportedConstructorInferenceShape {
        source: SourceId,
        tree_index: u32,
        constructor: SymbolId,
        parameter_index: Option<usize>,
        formal: TypeId,
        actual: TypeId,
    },
    /// A raw generic `New` could not be assigned a sound inferred instance type.
    UnableToFinalizeRawGenericNewInstanceType {
        source: SourceId,
        tree_index: u32,
        constructor: SymbolId,
        result: TypeId,
    },
    MalformedClassParent {
        source: SourceId,
        tree_index: u32,
    },
    /// A projected parent does not resolve to a class or trait declaration.
    UnresolvedParentClassKind {
        source: SourceId,
        tree_index: u32,
        symbol: Option<SymbolId>,
    },
    /// Higher-kinded source type parameter completion is deferred.
    HigherKindedTypeParameterDeferred {
        symbol: SymbolId,
        tree_index: u32,
    },
    /// Higher-kinded source type alias completion is deferred.
    HigherKindedTypeAliasDeferred {
        symbol: SymbolId,
        tree_index: u32,
    },
    /// A type alias RHS cannot be represented by source alias bounds.
    InvalidCompletedBounds {
        source: SourceId,
        tree_index: u32,
        ty: TypeId,
    },
    /// Opaque alias completion is deferred until opaque visibility is modeled.
    OpaqueAliasDeferred {
        symbol: SymbolId,
        tree_index: u32,
    },
    /// The inferred-result method is already being completed through its RHS.
    RecursiveInferredMethodResult {
        symbol: SymbolId,
    },
    /// An inferred-result method has no body from which to obtain a result.
    InferredMethodResultRightHandSideMissing {
        symbol: SymbolId,
        tree_index: u32,
    },
    /// The typed method body widened to a type that cannot be a method result.
    InvalidInferredMethodResult {
        symbol: SymbolId,
        tree_index: u32,
        inferred: TypeId,
    },
    /// Extension signature normalization for right-associative methods is deferred.
    RightAssociativeExtensionDeferred {
        symbol: SymbolId,
        tree_index: u32,
    },
    /// Right-associative infix lowering requires Scala's right-operand rewrite.
    RightAssociativeInfixDeferred {
        source: SourceId,
        tree_index: u32,
        operator: dotty_core::Name,
    },
    /// An infix operator was not represented in the term namespace.
    InfixOperatorMustBeTerm {
        source: SourceId,
        tree_index: u32,
        operator: dotty_core::Name,
    },
    /// A source method parameter tree has no symbol for this method owner.
    MethodParameterSymbolMissing {
        method: SymbolId,
        parameter_tree_index: u32,
    },
    /// An explicitly typed local method uses a signature form outside the
    /// current plain, single-clause subset.
    LocalMethodSignatureDeferred {
        symbol: SymbolId,
        tree_index: u32,
        feature: &'static str,
    },
    /// A local method has no source-written result type; result inference is
    /// handled by a later typer increment.
    LocalMethodInferredResultDeferred {
        symbol: SymbolId,
        tree_index: u32,
    },
    /// A method type-parameter tree has no symbol for this method owner.
    MethodTypeParameterSymbolMissing {
        method: SymbolId,
        parameter_tree_index: u32,
    },
    /// Extension method prefix parameter metadata is absent from the source index.
    ExtensionPrefixClausesMissing {
        method: SymbolId,
    },
    /// A method clause has malformed parameter kinds or inconsistent flags.
    MalformedMethodClause {
        method: SymbolId,
        method_tree_index: u32,
        clause_index: usize,
    },
    /// A constructor is not owned by a class, trait, or module class.
    ConstructorOwnerNotClassLike {
        constructor: SymbolId,
        owner: Option<SymbolId>,
    },
    /// A completed constructor owner does not contain canonical ClassInfo.
    MalformedConstructorOwnerInfo {
        constructor: SymbolId,
        owner: SymbolId,
        info: TypeId,
    },
    /// A constructor owner's source definition does not have a template.
    MalformedConstructorOwner {
        constructor: SymbolId,
        owner: SymbolId,
        tree_index: u32,
    },
    /// Scala 3 secondary constructors cannot declare their own type parameters.
    SecondaryConstructorTypeParametersUnsupported {
        constructor: SymbolId,
    },
    /// A deferred completion belongs to a future completion engine.
    DeferredSymbolCompletion {
        symbol: SymbolId,
    },
    /// A previous fatal semantic failure has already been recorded.
    SymbolAlreadyErrored {
        symbol: SymbolId,
    },
    /// A source type-tree form is not supported yet.
    UnsupportedTypeTree {
        source: SourceId,
        tree_index: u32,
        tree_kind: &'static str,
    },
    /// A declaration has no source-written type and must not be inferred here.
    MissingDeclaredType {
        source: SourceId,
        tree_index: u32,
        position: Option<SourceSpan>,
    },
    /// The source tree does not have the definition shape expected by its symbol.
    MalformedSourceAst {
        source: SourceId,
        tree_index: u32,
        symbol: SymbolId,
        kind: SymbolKind,
    },
    /// The source tree's declaration category does not agree with the symbol.
    SymbolSourceKindMismatch {
        source: SourceId,
        tree_index: u32,
        symbol: SymbolId,
        kind: SymbolKind,
    },
    /// A referenced tree ID is outside the supplied arena.
    TreeOutsideArena {
        source: SourceId,
        tree_index: u32,
    },
    /// A simple type identifier did not resolve in its declaration scope.
    TypeNameNotFound {
        source: SourceId,
        tree_index: u32,
        name: dotty_core::Name,
        position: Option<SourceSpan>,
    },
    /// A source name exists, but its semantic symbol cannot denote a type.
    WrongTypeNameKind {
        source: SourceId,
        tree_index: u32,
        name: dotty_core::Name,
        symbol: SymbolId,
        kind: SymbolKind,
        position: Option<SourceSpan>,
    },
    /// The external semantic symbol resolver could not answer soundly.
    SymbolResolution {
        source: SourceId,
        tree_index: u32,
        error: ResolutionError,
    },
    /// The lexical scope exposes multiple possible type declarations.
    AmbiguousTypeName {
        source: SourceId,
        tree_index: u32,
        name: dotty_core::Name,
        position: Option<SourceSpan>,
    },
    /// The type-tree cache was already bound to a different type.
    DuplicateSourceTypeCacheEntry {
        source: SourceId,
        tree_index: u32,
        existing: TypeId,
        attempted: TypeId,
    },
    /// A future completion operation could not rebind a semantic type.
    TypeRebinding {
        source: SourceId,
        tree_index: u32,
        error: dotty_core::TypeRebindError,
    },
    /// A source class has no canonical AST provenance from which to recover
    /// its class type-parameter order.
    SourceClassTypeParametersProvenanceMissing {
        symbol: SymbolId,
    },
    /// The source class, template, or primary constructor has an invalid AST
    /// shape for class type-parameter recovery.
    MalformedSourceClassTypeParameters {
        class: SymbolId,
        tree_index: u32,
    },
    /// A class type-parameter tree has no canonical symbol mapping.
    ClassTypeParameterSymbolMissing {
        class: SymbolId,
        tree_index: u32,
    },
    /// A receiver's nominal constructor differs from the expected class.
    ReceiverDoesNotDenoteExpectedClass {
        expected: SymbolId,
        actual: Option<SymbolId>,
    },
    /// The arguments on a source receiver do not match its source class arity.
    ReceiverGenericArityMismatch {
        class: SymbolId,
        expected: usize,
        actual: usize,
    },
    /// The applied target of constructor discovery has the wrong class arity.
    ConstructorTargetGenericArityMismatch {
        class: SymbolId,
        expected: usize,
        actual: usize,
    },
    /// A generic source class was used without receiver type arguments.
    RawGenericSourceReceiverUnsupported {
        class: SymbolId,
        expected: usize,
    },
    /// External generic argument order is not modeled yet.
    ExternalGenericInstantiationDeferred {
        class: SymbolId,
    },
    /// A completed member info could not be obtained for adaptation.
    MemberTypeUnavailable {
        symbol: SymbolId,
    },
    /// The bounded, symbol-exact semantic substitution failed.
    TypeSubstitution(dotty_core::TypeRebindError),
    /// Receiver normalization failed before a nominal view could be read.
    TypeNormalization(crate::types::TypeNormalizeError),
    /// A source tree already has a different typed replacement recorded.
    ConflictingTypedExpression {
        source: SourceId,
        tree_index: u32,
        existing: u32,
        attempted: u32,
    },
    /// String constants do not have a canonical source type in this session yet.
    StringLiteralTypingDeferred {
        source: SourceId,
        tree_index: u32,
    },
    /// Null constants do not have a canonical source type in this session yet.
    NullLiteralTypingDeferred {
        source: SourceId,
        tree_index: u32,
    },
    /// An integer spelling is invalid or outside the supported Scala Int range.
    IntegerLiteralOutOfRange {
        source: SourceId,
        tree_index: u32,
        spelling: String,
    },
    /// A decimal or exponent literal is not representable as an IEEE Double.
    FloatingLiteralInvalid {
        source: SourceId,
        tree_index: u32,
        spelling: String,
    },
    /// The selected `this` qualifier is not an enclosing class-like owner.
    ThisOwnerNotEnclosing {
        source: SourceId,
        tree_index: u32,
        qualifier: Option<dotty_core::Name>,
        owner: SymbolId,
    },
    /// The semantic owner chain used by `this` contains a cycle.
    ThisOwnerCycle {
        source: SourceId,
        owner: SymbolId,
    },
    /// No term declaration named this expression was visible.
    TermNameNotFound {
        source: SourceId,
        tree_index: u32,
        name: dotty_core::Name,
        position: Option<SourceSpan>,
    },
    /// More than one term declaration matched in the same lexical scope.
    AmbiguousTermReference {
        source: SourceId,
        tree_index: u32,
        name: dotty_core::Name,
        position: Option<SourceSpan>,
    },
    /// A method reference has several overloads and is not being selected yet.
    OverloadedReferenceDeferred {
        source: SourceId,
        tree_index: u32,
        name: dotty_core::Name,
    },
    /// An object term needs a module receiver model that this slice does not have.
    ObjectTermReferenceDeferred {
        source: SourceId,
        tree_index: u32,
        symbol: SymbolId,
    },
    /// A source object has no matching derived ModuleClass identity.
    ObjectModuleClassUnavailable {
        object: SymbolId,
    },
    /// The resolved term symbol kind is outside the supported value subset.
    UnsupportedTermReference {
        source: SourceId,
        tree_index: u32,
        symbol: SymbolId,
        kind: SymbolKind,
    },
    /// No member with this term name exists on the receiver.
    MemberNotFound {
        source: SourceId,
        tree_index: u32,
        receiver: TypeId,
        name: dotty_core::Name,
    },
    /// Several member candidates remain after lookup; overload resolution is deferred.
    OverloadedSelectionDeferred {
        source: SourceId,
        tree_index: u32,
        name: dotty_core::Name,
    },
    /// A regular application callee does not widen to a method type.
    ApplicationCalleeNotMethod {
        source: SourceId,
        tree_index: u32,
        ty: TypeId,
    },
    /// A supported polymorphic application leaves one type parameter without
    /// constraints from its term arguments.
    UnconstrainedTypeParameter {
        source: SourceId,
        tree_index: u32,
        binder: TypeId,
        parameter_index: usize,
    },
    /// Two actual arguments constrain one Poly parameter to non-equivalent types.
    ConflictingInferenceConstraints {
        source: SourceId,
        tree_index: u32,
        binder: TypeId,
        parameter_index: usize,
        first: TypeId,
        second: TypeId,
    },
    /// A generic application uses a formal/actual shape outside the structural
    /// inference rules supported by this typer increment.
    UnsupportedInferenceShape {
        source: SourceId,
        tree_index: u32,
        binder: TypeId,
        parameter_index: Option<usize>,
        formal: TypeId,
        actual: TypeId,
    },
    /// A unique polymorphic callee is not the supported one-clause Poly/Method form.
    UnsupportedPolymorphicApplicationShape {
        source: SourceId,
        tree_index: u32,
        binder: TypeId,
    },
    /// Candidate competition includes a polymorphic method and needs general
    /// generic overload resolution.
    GenericOverloadResolutionDeferred {
        source: SourceId,
        tree_index: u32,
    },
    /// Inferred arguments violate the declared bounds of a Poly parameter.
    InferredTypeArgumentBoundViolation {
        source: SourceId,
        tree_index: u32,
        parameter_index: usize,
        argument: TypeId,
        bound: TypeId,
        side: TypeArgumentBoundSide,
    },
    /// The inferred argument uses bounds this typer cannot validate soundly.
    UnsupportedInferredTypeArgumentBounds {
        source: SourceId,
        tree_index: u32,
        parameter_index: usize,
        argument: TypeId,
        bounds: TypeId,
    },
    /// A subtype relation needed to validate an inferred argument's bounds is
    /// outside the current relation subset.
    InferredTypeArgumentBoundCheckUnsupported {
        source: SourceId,
        tree_index: u32,
        parameter_index: usize,
        argument: TypeId,
        bound: TypeId,
        error: Box<TypeRelationError>,
    },
    /// An explicit type application callee does not widen to a Poly type.
    ExplicitTypeApplicationCalleeNotPoly {
        source: SourceId,
        tree_index: u32,
        ty: TypeId,
    },
    /// An explicit type application has a different number of type arguments
    /// than the callee's Poly binder.
    ExplicitTypeApplicationArityMismatch {
        source: SourceId,
        tree_index: u32,
        expected: usize,
        actual: usize,
    },
    /// An explicit type argument violates one side of its declared bounds.
    ExplicitTypeArgumentBoundViolation {
        source: SourceId,
        tree_index: u32,
        parameter_index: usize,
        argument: TypeId,
        bound: TypeId,
        side: TypeArgumentBoundSide,
    },
    /// The Poly parameter uses bounds not supported by explicit application.
    UnsupportedExplicitTypeArgumentBounds {
        source: SourceId,
        tree_index: u32,
        parameter_index: usize,
        bounds: TypeId,
    },
    /// The subtype relation could not decide an explicit type argument bound.
    ExplicitTypeArgumentBoundCheckUnsupported {
        source: SourceId,
        tree_index: u32,
        parameter_index: usize,
        argument: TypeId,
        bound: TypeId,
        error: Box<TypeRelationError>,
    },
    /// Type application on an overloaded callee remains deferred.
    OverloadedTypeApplicationDeferred {
        source: SourceId,
        tree_index: u32,
    },
    /// Binder-exact semantic Poly instantiation failed.
    PolyInstantiation(dotty_core::TypeRebindError),
    /// A projected source type form cannot be represented as a typed type tree.
    TypeArgumentTreeCannotBeReified {
        source: SourceId,
        tree_index: u32,
        tree_kind: &'static str,
    },
    /// A projected type ascription tree cannot be represented by a typed TypeTree.
    TypeAscriptionTreeCannotBeReified {
        source: SourceId,
        tree_index: u32,
        tree_kind: &'static str,
    },
    /// The application syntax does not match the method clause kind.
    ApplicationMethodKindMismatch {
        source: SourceId,
        tree_index: u32,
        application_kind: ApplyKind,
        method_kind: MethodKind,
    },
    /// Raw generic constructor inference currently supports only plain clauses.
    UnsupportedApplicationMethodKind {
        source: SourceId,
        tree_index: u32,
        kind: MethodKind,
    },
    /// A call requires contextual argument insertion or implicit search.
    UsingApplicationDeferred {
        source: SourceId,
        tree_index: u32,
    },
    /// The application supplies a different number of arguments than the method.
    ApplicationArityMismatch {
        source: SourceId,
        tree_index: u32,
        expected: usize,
        actual: usize,
    },
    /// An erased parameter cannot be supplied by ordinary expression typing.
    ErasedApplicationParameterDeferred {
        source: SourceId,
        tree_index: u32,
        parameter_index: usize,
    },
    /// A repeated parameter requires sequence adaptation outside this slice.
    VarargsApplicationParameterDeferred {
        source: SourceId,
        tree_index: u32,
        parameter_index: usize,
    },
    /// A repeated parameter reference needs its concrete sequence type.
    VarargsParameterReferenceDeferred {
        source: SourceId,
        tree_index: u32,
        symbol: SymbolId,
    },
    /// A by-name parameter requires delayed evaluation outside this slice.
    ByNameApplicationParameterDeferred {
        source: SourceId,
        tree_index: u32,
        parameter_index: usize,
    },
    /// The method result refers to a term parameter of the method being called.
    DependentMethodApplicationDeferred {
        source: SourceId,
        tree_index: u32,
        binder: TypeId,
        result: TypeId,
    },
    /// An argument's widened type does not conform to the corresponding formal.
    ApplicationArgumentTypeMismatch {
        source: SourceId,
        tree_index: u32,
        argument_index: usize,
        actual: TypeId,
        expected: TypeId,
    },
    /// The supported conformance relation cannot decide an argument comparison.
    ApplicationArgumentConformanceUnsupported {
        source: SourceId,
        tree_index: u32,
        argument_index: usize,
        actual: TypeId,
        expected: TypeId,
        error: Box<TypeRelationError>,
    },
    /// The widened expression type does not conform to its expected type.
    ExpectedExpressionTypeMismatch {
        source: SourceId,
        tree_index: u32,
        actual: TypeId,
        expected: TypeId,
    },
    /// The supported conformance relation cannot decide an expected-type comparison.
    ExpectedExpressionConformanceUnsupported {
        source: SourceId,
        tree_index: u32,
        actual: TypeId,
        expected: TypeId,
        error: Box<TypeRelationError>,
    },
    /// An assignment's typed left-hand side is not a direct symbol reference.
    AssignmentLhsNotAssignable {
        source: SourceId,
        tree_index: u32,
        lhs_tree_index: u32,
        expression_kind: &'static str,
    },
    /// An assignment target is a value-like declaration without MUTABLE.
    AssignmentTargetImmutable {
        source: SourceId,
        tree_index: u32,
        target: SymbolId,
    },
    /// This semantic symbol category is not writable by ordinary assignment.
    AssignmentTargetKindUnsupported {
        source: SourceId,
        tree_index: u32,
        target: SymbolId,
        kind: SymbolKind,
    },
    /// The writable type of the selected declaration could not be recovered.
    WritableAssignmentTypeUnavailable {
        source: SourceId,
        tree_index: u32,
        target: SymbolId,
        error: Box<TyperError>,
    },
    /// The condition of an if expression does not conform to canonical Boolean.
    IfConditionTypeMismatch {
        source: SourceId,
        tree_index: u32,
        actual: TypeId,
        expected: TypeId,
    },
    /// The supported relation cannot decide if-condition conformance.
    IfConditionConformanceUnsupported {
        source: SourceId,
        tree_index: u32,
        actual: TypeId,
        expected: TypeId,
        error: Box<TypeRelationError>,
    },
    /// A branch expression type could not be widened for the minimal join.
    IfBranchTypeCannotBeWidened {
        source: SourceId,
        tree_index: u32,
        branch_tree_index: u32,
        error: Box<TyperError>,
    },
    /// The minimal branch join could not soundly decide a subtype relation.
    IfBranchJoinUnsupported {
        source: SourceId,
        tree_index: u32,
        left: TypeId,
        right: TypeId,
        error: Box<TypeRelationError>,
    },
    /// An if expression references a missing condition or branch tree.
    IfChildTreeOutsideArena {
        source: SourceId,
        tree_index: u32,
        child_tree_index: u32,
        role: &'static str,
    },
    /// The condition of a while expression does not conform to canonical Boolean.
    WhileConditionTypeMismatch {
        source: SourceId,
        tree_index: u32,
        actual: TypeId,
        expected: TypeId,
    },
    /// The supported relation cannot decide while-condition conformance.
    WhileConditionConformanceUnsupported {
        source: SourceId,
        tree_index: u32,
        actual: TypeId,
        expected: TypeId,
        error: Box<TypeRelationError>,
    },
    /// A while expression references a missing condition or body tree.
    WhileChildTreeOutsideArena {
        source: SourceId,
        tree_index: u32,
        child_tree_index: u32,
        role: &'static str,
    },
    /// A return is not nested in a source method supported by this increment.
    ReturnOutsideSupportedMethod {
        source: SourceId,
        tree_index: u32,
        owner: SymbolId,
    },
    /// The enclosing method's source definition cannot be recovered safely.
    ReturnMethodProvenanceMalformed {
        source: SourceId,
        tree_index: u32,
        method: SymbolId,
    },
    /// Returns cannot participate in inferred method-result computation yet.
    ReturnInInferredResultMethodDeferred {
        source: SourceId,
        tree_index: u32,
        method: SymbolId,
    },
    /// A return target is missing or is not a method/label reference tree.
    MalformedReturnTarget {
        source: SourceId,
        tree_index: u32,
        target_tree_index: u32,
    },
    /// Labeled or non-local return targets are outside the current source slice.
    NonLocalReturnDeferred {
        source: SourceId,
        tree_index: u32,
        target_tree_index: u32,
    },
    /// The explicitly typed return value does not conform to the method result.
    ReturnExpressionTypeMismatch {
        source: SourceId,
        tree_index: u32,
        actual: TypeId,
        expected: TypeId,
    },
    /// The supported relation cannot decide return-value conformance.
    ReturnExpressionConformanceUnsupported {
        source: SourceId,
        tree_index: u32,
        actual: TypeId,
        expected: TypeId,
        error: Box<TypeRelationError>,
    },
    /// The owner chain used by a return is cyclic or references a missing owner.
    MalformedReturnOwnerChain {
        source: SourceId,
        tree_index: u32,
        owner: SymbolId,
    },
    /// The enclosing method signature has an invalid methodic result chain.
    MalformedReturnMethodSignature {
        source: SourceId,
        tree_index: u32,
        method: SymbolId,
        signature: TypeId,
    },
    /// Every overload rejected the supplied arguments; reasons are kept per candidate.
    OverloadApplicationNoApplicable {
        source: SourceId,
        tree_index: u32,
        candidates: Vec<(SymbolId, OverloadRejection)>,
    },
    /// Several supported candidates apply, but this increment has not selected a unique best one.
    AmbiguousOverloadApplication {
        source: SourceId,
        tree_index: u32,
        candidates: Vec<SymbolId>,
    },
    /// An unsupported overload could compete with supported candidates.
    OverloadResolutionRequiresUnsupportedCandidate {
        source: SourceId,
        tree_index: u32,
        candidate: SymbolId,
    },
    /// A method symbol has an incomplete or unsupported callable shape.
    MalformedOverloadCandidate {
        symbol: SymbolId,
        callable: TypeId,
    },
    /// The application name bucket mixes methods and non-method terms.
    MixedApplicationCandidateKinds {
        source: SourceId,
        tree_index: u32,
        candidates: Vec<SymbolId>,
    },
    /// A type selection is outside expression typing.
    TypeSelectionInExpression {
        source: SourceId,
        tree_index: u32,
    },
    /// Member discovery failed with a typed lookup error.
    MemberLookup(Box<MemberLookupError>),
    /// The source expression form is outside this issue's supported subset.
    UnsupportedExpression {
        source: SourceId,
        tree_index: u32,
        expression_kind: &'static str,
    },
    /// The source pattern root is not supported by the current pattern subset.
    UnsupportedPattern {
        source: SourceId,
        tree_index: u32,
        pattern_kind: PatternKind,
    },
    /// A source case tree is missing or does not have the required `CaseDef` shape.
    MalformedCaseDef {
        source: SourceId,
        tree_index: u32,
        actual_kind: &'static str,
    },
    /// Guard typing is not part of the current wildcard case subset.
    MatchGuardDeferred {
        source: SourceId,
        tree_index: u32,
    },
    /// A source pattern binding is malformed or does not match the requested name.
    MalformedPatternBinding {
        source: SourceId,
        tree_index: u32,
    },
    /// The wildcard spelling cannot introduce a pattern-bound symbol.
    WildcardPatternBindingRejected {
        source: SourceId,
        tree_index: u32,
    },
    /// Pattern symbols may only be entered while a case scope is active.
    PatternBindingOutsideCaseScope {
        source: SourceId,
        tree_index: u32,
    },
    /// A repeated source binding was presented with conflicting case ownership.
    PatternBindingScopeConflict {
        source: SourceId,
        tree_index: u32,
        existing_scope: dotty_core::ScopeId,
        attempted_scope: dotty_core::ScopeId,
    },
    /// Two pattern bindings with the same name occur in one case scope.
    DuplicatePatternBinding {
        source: SourceId,
        tree_index: u32,
        name: dotty_core::Name,
    },
    /// A source Match has no cases, so no result type can be derived.
    EmptyMatchCases {
        source: SourceId,
        tree_index: u32,
    },
    /// The selector type could not be adapted to a pattern prototype.
    MatchSelectorTypeCannotBeAdapted {
        source: SourceId,
        tree_index: u32,
        error: Box<TyperError>,
    },
    /// A case body's type could not be widened for the bounded result join.
    MatchCaseResultTypeCannotBeWidened {
        source: SourceId,
        tree_index: u32,
        case_tree_index: u32,
        error: Box<TyperError>,
    },
    /// The bounded result join cannot decide a relation between two cases.
    MatchCaseJoinUnsupported {
        source: SourceId,
        tree_index: u32,
        left: TypeId,
        right: TypeId,
        error: Box<TypeRelationError>,
    },
    /// A block statement changes the local declaration or import environment.
    LocalBlockDeclarationDeferred {
        source: SourceId,
        tree_index: u32,
        kind: &'static str,
    },
    /// An inferred local initializer widens to a type that cannot be a value info.
    InvalidInferredLocalValueType {
        source: SourceId,
        tree_index: u32,
        inferred: TypeId,
    },
    /// A local value declaration has no initializer.
    LocalValueRightHandSideMissing {
        source: SourceId,
        tree_index: u32,
    },
    /// A local value initializer refers to the local value currently being initialized.
    RecursiveLocalValueInitializer {
        source: SourceId,
        tree_index: u32,
        symbol: SymbolId,
    },
    /// Local value declarations are only supported as statements in a block.
    LocalValueOutsideBlock {
        source: SourceId,
        tree_index: u32,
    },
    /// Two local values with the same name occur in one block scope.
    DuplicateLocalValue {
        source: SourceId,
        tree_index: u32,
        name: dotty_core::Name,
    },
    /// A local value's widened initializer type does not conform to its annotation.
    LocalValueTypeMismatch {
        source: SourceId,
        tree_index: u32,
        actual: TypeId,
        expected: TypeId,
    },
    /// The supported conformance relation cannot decide a local value comparison.
    LocalValueConformanceUnsupported {
        source: SourceId,
        tree_index: u32,
        actual: TypeId,
        expected: TypeId,
        error: Box<TypeRelationError>,
    },
    /// The semantic type is not yet supported by expression widening.
    ExpressionTypeCannotBeWidened {
        ty: TypeId,
    },
    /// A term reference designates a symbol category that is not widenable.
    TermReferenceCannotBeWidened {
        symbol: SymbolId,
        kind: SymbolKind,
    },
    /// The receiver prefix does not contain the referenced member symbol.
    TermReferencePrefixMismatch {
        symbol: SymbolId,
        prefix: TypeId,
    },
    /// A selection qualifier cannot be preserved as a stable reference path.
    UnstableSelectionPrefix {
        source: SourceId,
        tree_index: u32,
        qualifier_type: TypeId,
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
