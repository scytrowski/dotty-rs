use std::fmt;

use dotty_core::ids::{SymbolId, TypeId};
use dotty_core::names::Namespace;
use dotty_core::resolution::ResolutionError;
use dotty_core::symbols::SymbolKind;
use dotty_core::types::TypeRebindError;
use dotty_tasty::tasty::{AstError, TastyFileError};
use dotty_tasty::term::TermError;

/// Structural reason a template parent could not be projected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentTreeReason {
    /// Parent constructor wrappers exceeded the supported traversal bound.
    NestingLimit,
    /// The constructor call has no function child.
    ConstructorHasFunction,
    /// The parent constructor selection is not named `<init>`.
    SelectsConstructor,
    /// The parent constructor selection qualifier is not a `NEW` node.
    SelectsNew,
    /// `NEW` does not have exactly one type-tree child.
    NewArity,
}

/// Structural reason a `SELFDEF` could not be projected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfTypeTreeReason {
    /// The self definition does not have exactly one type-tree child.
    TypeTreeArity,
}

/// Structural reason a definition could not be completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefinitionReason {
    /// A class definition's first child is not its template.
    ClassTemplatePosition,
    /// The method result type is absent after the parameter nodes.
    ResultTypePosition,
    /// The encoded header contains more parameters than the AST nodes.
    HeaderParametersExceedNodes,
    /// An encoded header parameter's kind differs from its AST node.
    HeaderParameterTagMismatch,
    /// The AST contains more parameter nodes than the encoded header.
    ParameterNodesExceedHeader,
    /// An opaque alias does not have exactly one right-hand-side tree.
    OpaqueAliasRhsArity,
    /// Opaque bounds children disagree with their encoded shape.
    OpaqueBoundsShape,
    /// Opaque alias graph traversal exceeded its supported depth.
    OpaqueAliasDepthLimit,
    /// A type alias does not have exactly one right-hand-side tree.
    TypeAliasRhsArity,
    /// A generic opaque alias lambda does not have parameters and a body.
    OpaqueLambdaShape,
    /// Nested opaque alias parameter lambdas are not projected yet.
    NestedOpaqueParameterLambdasUnsupported,
}

/// Structural reason a refined type tree could not be projected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefinedTypeTreeReason {
    /// The refined tree is missing its parent type tree.
    UnexpectedParent,
    /// A refinement stat is outside the supported member-definition subset.
    UnsupportedMemberStat,
}

/// Structural reason a type or type tree could not be projected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeReason {
    MatchTypeShape,
    ChildShapeMismatch,
    OwnerSpaceNameDesignator,
    RefinedTypeShape,
    InvalidReferenceTargetShape,
    BinderChildMismatch,
    EmptyBinder,
    InvalidVarianceMarker,
    AnnotationTailShape,
    AnnotationOfAnnotatedTypeIsNotNode,
    AnnotationPayloadIsNotNode,
    AnnotatedTypeShape,
    IdentifierTypeTreeArity,
    AnnotationApplyFunctionMissing,
    AnnotationApplyArityMismatch,
    AnnotationTypeApplyFunctionMissing,
    AnnotationTypeApplyArityMismatch,
    AnnotationSelectionShape,
    AnnotationNewClassArity,
    NamedArgumentArity,
    AnnotationLiteralNotConstant,
    RecursiveTypeArity,
    ParentTreeDepthLimit,
    TypeTreeDepthLimit,
    LambdaTreeShape,
    LambdaParameterKind,
    RefinedTreeUnexpectedParent,
    IdentifierTermTypeArity,
    TermSelectionQualifierArity,
    QualifiedThisQualifierArity,
    QualifiedThisQualifierKind,
    ExplicitTypeTreeArity,
    ByNameTypeTreeArity,
    AppliedTypeConstructorMissing,
    BoundsTypeTreeArity,
    TypeSelectionQualifierArity,
    SingletonTreeReferenceArity,
    AnnotatedTypeTreeShape,
}

/// Stable semantic category for an unpickling failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnpickleErrorCategory {
    /// The artifact violates a structural or semantic contract.
    MalformedInput,
    /// The artifact is valid, but this semantic projection does not support it.
    UnsupportedFeature,
    /// A symbol or dependency could not be resolved in the available context.
    ResolutionFailure,
    /// The operation is waiting for another phase or pending semantic state.
    Deferred,
    /// A compiler-owned invariant or programming contract failed.
    InternalInvariant,
    /// The stored error payload does not retain enough evidence to choose one
    /// category without consulting the source file, resolver or store state.
    Conditional,
}

/// Why semantic unpickling of a TASTy file failed.
///
/// Malformed or unsupported TASTy input is always reported through this type;
/// the unpickler never lowers an unsupported semantic shape to a placeholder.
/// Variants are introduced only when an unpickling pass needs them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnpickleError {
    /// The TASTy file itself failed a structural query, such as building the
    /// AST address index.
    Tasty(TastyFileError),
    /// An AST node could not be decoded into its structured form.
    Ast(AstError),
    /// A second symbol was entered for a definition address that already has
    /// one, which would break the address-identity invariant.
    DuplicateDefinition { address: u32 },
    /// A second declaration scope was entered for a symbol that already owns
    /// one.
    DuplicateScope { symbol: SymbolId },
    /// A class-like symbol was discovered with a companion that conflicts
    /// with a link already published on either endpoint.
    ConflictingCompanion {
        symbol: SymbolId,
        existing: SymbolId,
        candidate: SymbolId,
    },
    /// A name reference does not resolve to a name-table entry, or resolves
    /// through an unreasonably deep (possibly cyclic) chain of entries.
    InvalidNameReference { reference: u32 },
    /// A name-table entry with a tag this unpickler does not interpret.
    UnsupportedName { reference: u32 },
    /// The qualifier of a `private[X]` / `protected[X]` modifier has a tree
    /// shape this unpickler does not read. `tag` is the qualifier node's tag.
    /// Package names and references to enclosing definitions are supported.
    UnsupportedQualifier { tag: u8 },
    /// A reference points at an address that is not the start of a visible AST
    /// node (out of range, or inside another node's payload), or at one that
    /// has no entered symbol. `from` is the referring definition.
    InvalidReferenceTarget { from: u32, to: u32 },
    /// The qualifier of a `private[Q]` / `protected[Q]` modifier at
    /// `definition` is not a package, class, trait or object that encloses
    /// the qualified definition (or is the definition itself).
    InvalidQualifier { definition: u32 },
    /// A `PACKAGE` node whose path is neither a direct package reference
    /// (`TERMREFpkg`) nor a `SHAREDtype` link to one, which are the only forms
    /// this unpickler reads.
    UnsupportedPackagePath { address: u32 },
    /// No definition node exists at an address the tree walk expected one.
    MissingDefinition { address: u32 },
    /// A second `TypeId` was recorded for a type-node address that already
    /// has one, which would break the address-identity invariant.
    DuplicateType { address: u32 },
    /// The type tree at `address` is a form the projection does not build a
    /// type for yet (Milestone 5a). Distinct from `UnsupportedType`, which is
    /// about type *wire nodes*; the tree is never guessed from its syntax.
    UnsupportedTypeTree { address: u32, tag: u8 },
    /// The term tree at `address` is a form the term-`tpe` projection does not
    /// build a type for (Milestone 5b): a `SELECTtpt` qualifier or
    /// `SINGLETONtpt` reference that is not a path.
    UnsupportedTermTree { address: u32, tag: u8 },
    /// The class `symbol` entered for the definition at `address` has no
    /// declaration scope in this unit's index, so no `ClassInfo` can name it
    /// (Milestone 5d1). Pass 1 allocates one for every template.
    MissingClassScope { address: u32, symbol: SymbolId },
    /// The template parent at `address` (tag `tag`) is a wrapper form
    /// `type_of_parent` does not read (Milestone 5d1).
    UnsupportedParentTree { address: u32, tag: u8 },
    /// The template parent at `address` has the right wrapper but not the
    /// shape it requires (Milestone 5d1).
    MalformedParentTree {
        address: u32,
        reason: ParentTreeReason,
    },
    /// The self definition at `address` does not have the shape
    /// `SELFDEF name type_tree` (Milestone 5d1).
    InvalidSelfTypeTree {
        address: u32,
        reason: SelfTypeTreeReason,
    },
    /// The definition at `address` has a header that does not agree with its
    /// nodes (a parameter clause sequence that cannot be aligned with the
    /// parameter nodes, or a missing result type).
    MalformedDefinition {
        address: u32,
        reason: DefinitionReason,
    },
    /// The method whose parameter is at `address` has a parameter modifier
    /// (`INLINE`, `TRACKED`, `INTO`: `tag`) whose upstream adaptation
    /// (`MethodType.fromSymbols`) `dotty-core` cannot represent. The method is
    /// not completed rather than completed with it dropped.
    UnsupportedMethodParameterSemantics { address: u32, tag: u8 },
    /// The constructor at `address` has no entered owner, or its owner is
    /// not `Class`, `Trait` or `ModuleClass` (Milestone 5d2a). `owner` is the
    /// entered owner, if any.
    ConstructorOwnerNotClassLike {
        address: u32,
        owner: Option<SymbolId>,
    },
    /// The constructor at `address`'s owner already holds
    /// `SymbolInfo::Complete(info)`, but `info` is not the `ClassInfo` class
    /// completion always publishes for it (naming `owner` itself, with the
    /// canonical `no_prefix`). Constructor completion never triggers class
    /// completion, but a completed owner's info is a real invariant, checked
    /// rather than trusted (Milestone 5d2a).
    MalformedOwnerClassInfo {
        address: u32,
        owner: SymbolId,
        info: TypeId,
    },
    /// The `LAMBDAtpt` at `address` was reached from two different owners
    /// (through a `SHAREDterm`), and its type parameters are owned once: it
    /// is not projected rather than given a second owner.
    SharedLambdaOwnerConflict { address: u32 },
    /// The `REFINEDtpt` at `address` was reached from two different owners
    /// (through a `SHAREDterm`); its synthetic refinement class has one fixed
    /// owner, the same policy `SharedLambdaOwnerConflict` follows (Milestone
    /// 5d2b).
    SharedRefinementOwnerConflict { address: u32 },
    /// An immediate statement of the `REFINEDtpt` at `address` is not a
    /// `TYPEDEF`, `VALDEF` or `DEFDEF` (Milestone 5d2b): an unsupported
    /// refinement stat, never silently dropped.
    UnsupportedRefinementStat { address: u32, tag: u8 },
    /// Projecting the `REFINEDtpt` at `address` found no synthetic refinement
    /// class entered for it: pass 1 either did not run or did not reach this
    /// address (Milestone 5d2b).
    MissingRefinementClass { address: u32 },
    /// The symbol entered at the `REFINEDtpt` address `address` exists but is
    /// not the synthetic refinement class pass 1 makes there (wrong kind or
    /// name), or its `SymbolInfo` is not the complete, parent-less
    /// `ClassInfo` pass 1 publishes for it (Milestone 5d2b).
    InvalidRefinementClass { address: u32, symbol: SymbolId },
    /// The synthetic refinement class at `address` has no declaration scope
    /// recorded for it, which pass 1 always allocates (Milestone 5d2b).
    MissingRefinementScope { address: u32, symbol: SymbolId },
    /// The `REFINEDtpt` at `address` has a shape structured decoding does not
    /// agree with (its indexed children disagree with its wire shape), or a
    /// member's projected `Refined.info` reason `reason` describes (Milestone
    /// 5d2b).
    MalformedRefinedTypeTree {
        address: u32,
        reason: RefinedTypeTreeReason,
    },
    /// Two immediate stats of the `REFINEDtpt` at `address` declare the same
    /// `name` in the same namespace: Dotty rejects an overloaded refinement
    /// member, so the second declaration is refused rather than shadowing or
    /// stacking with the first (Milestone 5d2b).
    UnsupportedRefinementOverload { address: u32, name: String },
    /// Closing a `REFINEDtpt`'s projected chain over its synthetic class's
    /// `ThisType` (`close_over_this`) failed at `address`.
    CloseOverThis {
        address: u32,
        error: dotty_core::TypeRebindError,
    },
    /// Abstracting the parameter symbols of the binder written at `address`
    /// into `ParamRef`s failed.
    ParameterAbstraction {
        address: u32,
        error: dotty_core::TypeRebindError,
    },
    /// The qualifier of the selection at `address` is an unstable singleton (a
    /// method, a mutable member, a constructor), which Dotty would widen
    /// (`widenIfUnstable`). The model has no widening, so the selection is
    /// refused rather than made on the unstable singleton.
    UnstableSelectQualifier { address: u32, qualifier: TypeId },
    /// The `SINGLETONtpt` at `address` refers to a term whose type is not a
    /// stable singleton: `ty` is what the reference denotes.
    InvalidSingletonTypeTree { address: u32, ty: TypeId },
    /// No symbol was entered for the definition address given to completion.
    MissingEnteredSymbol { address: u32 },
    /// The symbol at `address` is of a kind whose completion is a later
    /// milestone (methods, constructors, classes, traits, modules).
    UnsupportedSymbolCompletion { address: u32, kind: SymbolKind },
    /// The symbol at `address` is of a kind pass 1 never indexes an
    /// `ANNOTATION` tail for (a package, entered through `enter_package`
    /// rather than `enter_symbol`): `complete_symbol_annotations` and
    /// `complete_symbols_annotations` are for `TYPEDEF`/`VALDEF`/`DEFDEF`/
    /// `TYPEPARAM`/`PARAM` addresses only.
    UnsupportedAnnotationCompletion { address: u32, kind: SymbolKind },
    /// The defining owner of an opaque alias is not a class-like symbol.
    OpaqueAliasOwnerNotClassLike {
        address: u32,
        owner: SymbolId,
        kind: SymbolKind,
    },
    /// The opaque alias owner is missing from this unit and has no completed
    /// class info in the shared semantic store.
    OpaqueAliasOwnerNotEntered { address: u32, owner: SymbolId },
    /// An opaque alias would add a second same-named refinement to its owner.
    OpaqueAliasCycle { address: u32 },
    /// The symbol at `address` already holds `SymbolInfo::Deferred`, which
    /// this milestone has no way to force.
    SymbolCompletionDeferred { address: u32 },
    /// The symbol at `address` already holds `SymbolInfo::Error`.
    SymbolInfoError { address: u32 },
    /// A type parameter or type definition whose right-hand side cannot be
    /// bounds: `ty` is its projected type.
    InvalidCompletedBounds { address: u32, ty: TypeId },
    /// The type node at `address` has a tag the type pass does not decode
    /// yet (a later increment) or that is not a type at all. The node is
    /// never lowered to a placeholder type.
    UnsupportedType { tag: u8, address: u32 },
    /// The type node at `from` refers to the definition at `to`, which is a
    /// visible AST node but has no symbol entered by pass 1 (for example a
    /// local definition, or a definition in another unit).
    MissingReferencedSymbol { from: u32, to: u32 },
    /// The type node at `from` refers to the definition at `to`, which has an
    /// entered symbol of the wrong kind: a `TYPEREF*` must name a type-namespace
    /// symbol, a `TERMREF*` a term-namespace one, and `THIS` a class.
    InvalidReferenceKind { from: u32, to: u32 },
    /// The `TYPEREFpkg` / `TERMREFpkg` node at `address` names a package
    /// that has not been entered into the package registry. Resolving
    /// packages outside the entered units belongs to the future resolver.
    UnresolvedPackage { address: u32, package: String },
    /// The name-based reference at `address` is well formed and its prefix is
    /// understood, but neither the entered state nor the resolver holds a
    /// member `name` of `namespace` in it. This is not evidence the member
    /// does not exist.
    UnresolvedMember {
        address: u32,
        prefix: TypeId,
        /// The explicit declaration space of a `TYPEREFin` / `TERMREFin`,
        /// where the member was searched instead of in `prefix`.
        space: Option<TypeId>,
        name: String,
        namespace: Namespace,
    },
    /// The prefix holds `candidates` symbols under the requested name and
    /// namespace (overloads), and the reference does not say which. Never
    /// resolved by taking the first.
    AmbiguousMember {
        address: u32,
        prefix: TypeId,
        /// As in [`Self::UnresolvedMember`].
        space: Option<TypeId>,
        name: String,
        candidates: usize,
    },
    /// The term reference at `address` carries a signature (`SIGNED` /
    /// `TARGETSIGNED`), and selecting a member by signature is not
    /// implemented. The signature is neither stripped nor ignored.
    UnsupportedSignedReference { address: u32, name: String },
    /// The compound type node at `address` has a shape the semantic model
    /// cannot express: its indexed children disagree with its wire shape.
    MalformedType { address: u32, reason: TypeReason },
    /// The `ANNOTATEDtype` at `address` carries a full annotation tree, whose
    /// root node at `annotation_address` has tag `tag`. The annotated type is
    /// understood; the tree is not, and is never dropped to fit the compact
    /// form.
    UnsupportedAnnotationTree {
        address: u32,
        annotation_address: u32,
        tag: u8,
    },
    /// The `ANNOTATEDtype` at `address` has a compact annotation whose type,
    /// `annotation_type`, is neither a `TypeRef` nor an `Applied` type, which
    /// is all Dotty's `CompactAnnotation` accepts (a `SHAREDtype` payload can
    /// reach any type, or a binder still being decoded).
    InvalidCompactAnnotationType {
        address: u32,
        annotation_type: TypeId,
    },
    /// The full annotation at `annotation_address` inside the `ANNOTATEDtype`
    /// at `address` is a constructor call with a part this pass does not read
    /// (`tag` names it): a spine node that is not `APPLY`, `TYPEAPPLY`, the
    /// constructor `SELECTin` or `NEW`, or a class tree that is not a type.
    UnsupportedAnnotationConstructor {
        address: u32,
        annotation_address: u32,
        tag: u8,
    },
    /// An argument of the annotation on the `ANNOTATEDtype` at `address` is
    /// a form the annotation value model does not hold (only literals and class
    /// literals are). It is never dropped or evaluated.
    UnsupportedAnnotationArgument {
        address: u32,
        argument_address: u32,
        tag: u8,
    },
    /// The type of the full annotation on the `ANNOTATEDtype` at `address`,
    /// `annotation_type`, is neither a `TypeRef` nor an `Applied` type.
    InvalidAnnotationType {
        address: u32,
        annotation_type: TypeId,
    },
    /// The `TYPEBOUNDS` node at `address` carries variance markers for the
    /// lambda `target`, which is still being decoded, so it cannot be rebound
    /// yet. (A marker on a bound that is not a lambda is left alone, as
    /// Dotty's `readVariances` does.)
    BoundsVarianceTargetPending { address: u32, target: TypeId },
    /// The `TYPEBOUNDS` node at `address` carries `actual` variance markers,
    /// but the lambda `lambda` it applies to has `expected` parameters.
    BoundsVarianceArityMismatch {
        address: u32,
        lambda: TypeId,
        expected: usize,
        actual: usize,
    },
    /// Rebinding the lambda under the `TYPEBOUNDS` at `address` failed.
    RebindFailed {
        address: u32,
        error: TypeRebindError,
    },
    /// The `METHODtype` at `address` ends with a modifier, `tag`, that a method
    /// type does not use (only `IMPLICIT` and `GIVEN` do).
    InvalidMethodModifier { address: u32, tag: u8 },
    /// The `PARAMtype` at `from` names `binder`, which is not the start of a
    /// visible AST node.
    InvalidBinderReference { from: u32, binder: u32 },
    /// The `PARAMtype` at `from` names a node whose type, `binder`, is not a
    /// binder (a `TypeLambda`, `Poly` or `Method`).
    InvalidBinderKind { from: u32, binder: TypeId },
    /// The `PARAMtype` at `address` names parameter `index` of `binder`, which
    /// has only `arity` parameters.
    InvalidParameterIndex {
        address: u32,
        binder: TypeId,
        index: u32,
        arity: usize,
    },
    /// Parameter `index` of `binder` has an info, `bounds`, that is not a
    /// bounds type (`Bounds` or `AliasingBounds`).
    InvalidTypeParameterBounds {
        binder: TypeId,
        index: u32,
        bounds: TypeId,
    },
    /// The prefix of the name-based reference at `address` is a form whose
    /// members cannot be looked up here, and the resolver did not know it
    /// either.
    UnsupportedResolutionPrefix { address: u32, prefix: TypeId },
    /// The declaration space (the owner) of the `TYPEREFin` / `TERMREFin` at
    /// `address` has no declaration-scope semantics here, or is a binder still
    /// being decoded, and the resolver did not know it either. Distinct from
    /// [`Self::UnsupportedResolutionPrefix`]: the prefix is how the reference
    /// is viewed, the space is where its declaration is.
    UnsupportedResolutionSpace { address: u32, space: TypeId },
    /// The prefix of the `TYPEREFin` at `address` is not a legal prefix (an
    /// unstable singleton), which Dotty wraps in a `QualSkolemType`. The
    /// semantic model has none, so the reference is not decoded.
    IllegalTypePrefix { address: u32, prefix: TypeId },
    /// The resolver failed on the reference at `address`, or answered with a
    /// symbol that cannot be the one requested.
    ResolverFailure {
        address: u32,
        error: ResolutionError,
    },
}

impl fmt::Display for UnpickleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tasty(error) => write!(formatter, "invalid TASTy file: {error}"),
            Self::Ast(error) => write!(formatter, "invalid TASTy AST node: {error}"),
            Self::DuplicateDefinition { address } => write!(
                formatter,
                "a symbol was already entered for the definition at address {address}"
            ),
            Self::InvalidNameReference { reference } => {
                write!(formatter, "invalid name reference {reference}")
            }
            Self::UnsupportedName { reference } => {
                write!(formatter, "unsupported name entry at reference {reference}")
            }
            Self::UnsupportedQualifier { tag } => write!(
                formatter,
                "access qualifier with tag {tag} is not a package or enclosing definition"
            ),
            Self::InvalidReferenceTarget { from, to } => write!(
                formatter,
                "definition at address {from} refers to address {to}, which is not a valid target"
            ),
            Self::InvalidQualifier { definition } => write!(
                formatter,
                "the access qualifier of the definition at address {definition} does not enclose it"
            ),
            Self::UnsupportedPackagePath { address } => write!(
                formatter,
                "package at address {address} has an unsupported path form"
            ),
            Self::MissingDefinition { address } => {
                write!(formatter, "no definition node at address {address}")
            }
            Self::DuplicateType { address } => write!(
                formatter,
                "a type was already recorded for the type node at address {address}"
            ),
            Self::UnsupportedTypeTree { address, tag } => write!(
                formatter,
                "the type tree at address {address} has tag {tag}, which is not projected yet"
            ),
            Self::UnsupportedTermTree { address, tag } => write!(
                formatter,
                "the term tree at address {address} has tag {tag}, which is not projected"
            ),
            Self::MissingClassScope { address, symbol } => write!(
                formatter,
                "the class {} defined at address {address} has no declaration scope",
                symbol.index()
            ),
            Self::UnsupportedParentTree { address, tag } => write!(
                formatter,
                "the template parent at address {address} has tag {tag}, which is not a parent form this pass reads"
            ),
            Self::MalformedParentTree { address, reason } => write!(
                formatter,
                "the template parent at address {address} is malformed: {reason}"
            ),
            Self::InvalidSelfTypeTree { address, reason } => write!(
                formatter,
                "the self definition at address {address} is malformed: {reason}"
            ),
            Self::MalformedDefinition { address, reason } => write!(
                formatter,
                "the definition at address {address} is malformed: {reason}"
            ),
            Self::UnsupportedMethodParameterSemantics { address, tag } => write!(
                formatter,
                "the method parameter at address {address} has the modifier with tag {tag}, whose method-type adaptation is not modeled"
            ),
            Self::ConstructorOwnerNotClassLike { address, owner } => write!(
                formatter,
                "the constructor at address {address} has owner {}, which is not a class, trait or module class",
                owner.map_or("none".to_owned(), |owner| owner.index().to_string())
            ),
            Self::MalformedOwnerClassInfo { address, owner, .. } => write!(
                formatter,
                "the constructor at address {address}'s owner {} is Complete but not its own canonical ClassInfo",
                owner.index()
            ),
            Self::SharedLambdaOwnerConflict { address } => write!(
                formatter,
                "the lambda type tree at address {address} is shared by definitions with different owners"
            ),
            Self::SharedRefinementOwnerConflict { address } => write!(
                formatter,
                "the refined type tree at address {address} is shared by definitions with different owners"
            ),
            Self::UnsupportedRefinementStat { address, tag } => write!(
                formatter,
                "the refinement stat at address {address} has tag {tag}, which is not a supported refinement member"
            ),
            Self::MissingRefinementClass { address } => write!(
                formatter,
                "the refined type tree at address {address} has no synthetic refinement class entered for it"
            ),
            Self::InvalidRefinementClass { address, symbol } => write!(
                formatter,
                "the symbol {} entered at refined type tree address {address} is not its synthetic refinement class",
                symbol.index()
            ),
            Self::MissingRefinementScope { address, symbol } => write!(
                formatter,
                "the synthetic refinement class {} at address {address} has no declaration scope",
                symbol.index()
            ),
            Self::MalformedRefinedTypeTree { address, reason } => write!(
                formatter,
                "the refined type tree at address {address} is malformed: {reason}"
            ),
            Self::UnsupportedRefinementOverload { address, name } => write!(
                formatter,
                "the refined type tree at address {address} declares '{name}' more than once"
            ),
            Self::CloseOverThis { address, error } => write!(
                formatter,
                "closing the refined type tree at address {address} over its synthetic class failed: {error}"
            ),
            Self::ParameterAbstraction { address, error } => write!(
                formatter,
                "abstracting the parameters of the binder at address {address} failed: {error}"
            ),
            Self::UnstableSelectQualifier { address, .. } => write!(
                formatter,
                "the selection at address {address} has an unstable qualifier, which needs widening this model does not have"
            ),
            Self::InvalidSingletonTypeTree { address, .. } => write!(
                formatter,
                "the singleton type tree at address {address} does not refer to a stable singleton"
            ),
            Self::MissingEnteredSymbol { address } => write!(
                formatter,
                "no symbol was entered for the definition at address {address}"
            ),
            Self::UnsupportedSymbolCompletion { address, kind } => write!(
                formatter,
                "the {kind:?} at address {address} cannot be completed yet"
            ),
            Self::UnsupportedAnnotationCompletion { address, kind } => write!(
                formatter,
                "the {kind:?} at address {address} has no annotation tail to complete"
            ),
            Self::OpaqueAliasOwnerNotClassLike { address, kind, .. } => write!(
                formatter,
                "the opaque type alias at address {address} is owned by a non-class-like {kind:?}"
            ),
            Self::OpaqueAliasOwnerNotEntered { address, owner } => write!(
                formatter,
                "the opaque type alias at address {address} has owner symbol {} without entered or completed class info",
                owner.index()
            ),
            Self::OpaqueAliasCycle { address } => write!(
                formatter,
                "completing the opaque type alias at address {address} would create a cyclic owner refinement"
            ),
            Self::SymbolCompletionDeferred { address } => write!(
                formatter,
                "the symbol at address {address} has a deferred completion"
            ),
            Self::SymbolInfoError { address } => {
                write!(
                    formatter,
                    "the symbol at address {address} has an error info"
                )
            }
            Self::InvalidCompletedBounds { address, ty } => write!(
                formatter,
                "the type {ty:?} of the definition at address {address} cannot be bounds"
            ),
            Self::UnsupportedType { tag, address } => write!(
                formatter,
                "the type node at address {address} has tag {tag}, which is not decoded yet"
            ),
            Self::MissingReferencedSymbol { from, to } => write!(
                formatter,
                "the type node at address {from} refers to address {to}, which has no entered symbol"
            ),
            Self::InvalidReferenceKind { from, to } => write!(
                formatter,
                "the type node at address {from} refers to the definition at address {to}, which is the wrong kind of symbol for it"
            ),
            Self::UnresolvedPackage { address, package } => write!(
                formatter,
                "the package reference at address {address} names package `{package}`, which has not been entered"
            ),
            Self::UnresolvedMember {
                address,
                name,
                namespace,
                ..
            } => write!(
                formatter,
                "the reference at address {address} names {namespace:?} member `{name}`, which is not in its prefix"
            ),
            Self::AmbiguousMember {
                address,
                name,
                candidates,
                ..
            } => write!(
                formatter,
                "the reference at address {address} names `{name}`, which has {candidates} candidates in its prefix"
            ),
            Self::UnsupportedSignedReference { address, name } => write!(
                formatter,
                "the reference at address {address} to `{name}` carries a signature, which is not supported yet"
            ),
            Self::UnsupportedAnnotationTree {
                address,
                annotation_address,
                tag,
            } => write!(
                formatter,
                "the annotated type at address {address} has a full annotation tree (tag {tag} at address {annotation_address}), which is not decoded yet"
            ),
            Self::InvalidCompactAnnotationType { address, .. } => write!(
                formatter,
                "the compact annotation of the annotated type at address {address} is not a type reference or an applied type"
            ),
            Self::UnsupportedAnnotationConstructor {
                address,
                annotation_address,
                tag,
            } => write!(
                formatter,
                "the annotation at address {annotation_address} of the annotated type at address {address} has a constructor part with tag {tag}, which is not decoded"
            ),
            Self::UnsupportedAnnotationArgument {
                address,
                argument_address,
                tag,
            } => write!(
                formatter,
                "the annotated type at address {address} has an annotation argument at address {argument_address} with tag {tag}, which is not decoded"
            ),
            Self::InvalidAnnotationType { address, .. } => write!(
                formatter,
                "the annotation of the annotated type at address {address} is not a type reference or an applied type"
            ),
            Self::MalformedType { address, reason } => write!(
                formatter,
                "the type at address {address} is malformed: {reason}"
            ),
            Self::BoundsVarianceTargetPending { address, .. } => write!(
                formatter,
                "the bounds at address {address} carry variance markers for a lambda that is still being decoded"
            ),
            Self::BoundsVarianceArityMismatch {
                address,
                expected,
                actual,
                ..
            } => write!(
                formatter,
                "the bounds at address {address} carry {actual} variance markers for a lambda with {expected} parameters"
            ),
            Self::RebindFailed { address, error } => write!(
                formatter,
                "the variances of the bounds at address {address} could not be applied: {error}"
            ),
            Self::InvalidMethodModifier { address, tag } => write!(
                formatter,
                "the method type at address {address} carries modifier {tag}, which a method type does not use"
            ),
            Self::InvalidBinderReference { from, binder } => write!(
                formatter,
                "the parameter type at address {from} names address {binder}, which is not a node"
            ),
            Self::InvalidBinderKind { from, .. } => write!(
                formatter,
                "the parameter type at address {from} names a type that is not a binder"
            ),
            Self::InvalidParameterIndex {
                address,
                index,
                arity,
                ..
            } => write!(
                formatter,
                "the parameter type at address {address} names parameter {index} of a binder with {arity} parameters"
            ),
            Self::InvalidTypeParameterBounds { index, .. } => write!(
                formatter,
                "the info of type parameter {index} is not a bounds type"
            ),
            Self::UnsupportedResolutionPrefix { address, .. } => write!(
                formatter,
                "the reference at address {address} has a prefix whose members cannot be looked up"
            ),
            Self::UnsupportedResolutionSpace { address, .. } => write!(
                formatter,
                "the reference at address {address} has a declaration space whose declarations cannot be looked up"
            ),
            Self::IllegalTypePrefix { address, .. } => write!(
                formatter,
                "the type reference at address {address} has a prefix that is not a legal prefix, which needs a qualifier skolem this model does not have"
            ),
            Self::ResolverFailure { address, error } => write!(
                formatter,
                "the resolver failed on the reference at address {address}: {error}"
            ),
            Self::DuplicateScope { symbol } => write!(
                formatter,
                "a declaration scope was already entered for symbol {}",
                symbol.index()
            ),
            Self::ConflictingCompanion {
                symbol,
                existing,
                candidate,
            } => write!(
                formatter,
                "symbol {} already links to companion {}, conflicting with {}",
                symbol.index(),
                existing.index(),
                candidate.index()
            ),
        }
    }
}

impl std::error::Error for UnpickleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Tasty(error) => Some(error),
            Self::Ast(error) => Some(error),
            Self::RebindFailed { error, .. } => Some(error),
            Self::DuplicateDefinition { .. }
            | Self::DuplicateScope { .. }
            | Self::ConflictingCompanion { .. }
            | Self::InvalidNameReference { .. }
            | Self::UnsupportedName { .. }
            | Self::UnsupportedQualifier { .. }
            | Self::InvalidReferenceTarget { .. }
            | Self::InvalidQualifier { .. }
            | Self::UnsupportedPackagePath { .. }
            | Self::MissingDefinition { .. }
            | Self::DuplicateType { .. }
            | Self::UnsupportedType { .. }
            | Self::UnsupportedTypeTree { .. }
            | Self::UnsupportedTermTree { .. }
            | Self::UnstableSelectQualifier { .. }
            | Self::SharedLambdaOwnerConflict { .. }
            | Self::SharedRefinementOwnerConflict { .. }
            | Self::UnsupportedRefinementStat { .. }
            | Self::MissingRefinementClass { .. }
            | Self::InvalidRefinementClass { .. }
            | Self::MissingRefinementScope { .. }
            | Self::MalformedRefinedTypeTree { .. }
            | Self::UnsupportedRefinementOverload { .. }
            | Self::CloseOverThis { .. }
            | Self::MalformedDefinition { .. }
            | Self::MissingClassScope { .. }
            | Self::UnsupportedParentTree { .. }
            | Self::MalformedParentTree { .. }
            | Self::InvalidSelfTypeTree { .. }
            | Self::UnsupportedMethodParameterSemantics { .. }
            | Self::ConstructorOwnerNotClassLike { .. }
            | Self::MalformedOwnerClassInfo { .. }
            | Self::ParameterAbstraction { .. }
            | Self::InvalidSingletonTypeTree { .. }
            | Self::MissingEnteredSymbol { .. }
            | Self::UnsupportedSymbolCompletion { .. }
            | Self::UnsupportedAnnotationCompletion { .. }
            | Self::OpaqueAliasOwnerNotClassLike { .. }
            | Self::OpaqueAliasOwnerNotEntered { .. }
            | Self::OpaqueAliasCycle { .. }
            | Self::SymbolCompletionDeferred { .. }
            | Self::SymbolInfoError { .. }
            | Self::InvalidCompletedBounds { .. }
            | Self::MissingReferencedSymbol { .. }
            | Self::InvalidReferenceKind { .. }
            | Self::UnresolvedPackage { .. }
            | Self::UnresolvedMember { .. }
            | Self::AmbiguousMember { .. }
            | Self::UnsupportedSignedReference { .. }
            | Self::MalformedType { .. }
            | Self::UnsupportedAnnotationTree { .. }
            | Self::InvalidCompactAnnotationType { .. }
            | Self::UnsupportedAnnotationConstructor { .. }
            | Self::UnsupportedAnnotationArgument { .. }
            | Self::InvalidAnnotationType { .. }
            | Self::BoundsVarianceTargetPending { .. }
            | Self::BoundsVarianceArityMismatch { .. }
            | Self::InvalidMethodModifier { .. }
            | Self::InvalidBinderReference { .. }
            | Self::InvalidBinderKind { .. }
            | Self::InvalidParameterIndex { .. }
            | Self::InvalidTypeParameterBounds { .. }
            | Self::UnsupportedResolutionPrefix { .. }
            | Self::UnsupportedResolutionSpace { .. }
            | Self::IllegalTypePrefix { .. }
            | Self::ResolverFailure { .. } => None,
        }
    }
}

impl From<TastyFileError> for UnpickleError {
    fn from(error: TastyFileError) -> Self {
        Self::Tasty(error)
    }
}

impl From<AstError> for UnpickleError {
    fn from(error: AstError) -> Self {
        Self::Ast(error)
    }
}

impl UnpickleError {
    /// Returns a stable machine-readable code. Codes are explicit API values:
    /// they never depend on formatted text, IDs, or TASTy addresses.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Tasty(_) => "unpickler.tasty",
            Self::Ast(_) => "unpickler.ast",
            Self::DuplicateDefinition { .. } => "unpickler.duplicate_definition",
            Self::DuplicateScope { .. } => "unpickler.duplicate_scope",
            Self::ConflictingCompanion { .. } => "unpickler.conflicting_companion",
            Self::InvalidNameReference { .. } => "unpickler.invalid_name_reference",
            Self::UnsupportedName { .. } => "unpickler.unsupported_name",
            Self::UnsupportedQualifier { .. } => "unpickler.unsupported_qualifier",
            Self::InvalidReferenceTarget { .. } => "unpickler.invalid_reference_target",
            Self::InvalidQualifier { .. } => "unpickler.invalid_qualifier",
            Self::UnsupportedPackagePath { .. } => "unpickler.unsupported_package_path",
            Self::MissingDefinition { .. } => "unpickler.missing_definition",
            Self::DuplicateType { .. } => "unpickler.duplicate_type",
            Self::UnsupportedTypeTree { .. } => "unpickler.unsupported_type_tree",
            Self::UnsupportedTermTree { .. } => "unpickler.unsupported_term_tree",
            Self::MissingClassScope { .. } => "unpickler.missing_class_scope",
            Self::UnsupportedParentTree { .. } => "unpickler.unsupported_parent_tree",
            Self::MalformedParentTree { reason, .. } => reason.code(),
            Self::InvalidSelfTypeTree { reason, .. } => reason.code(),
            Self::MalformedDefinition { reason, .. } => reason.code(),
            Self::UnsupportedMethodParameterSemantics { .. } => {
                "unpickler.unsupported_method_parameter_semantics"
            }
            Self::ConstructorOwnerNotClassLike { .. } => {
                "unpickler.constructor_owner_not_class_like"
            }
            Self::MalformedOwnerClassInfo { .. } => "unpickler.malformed_owner_class_info",
            Self::SharedLambdaOwnerConflict { .. } => "unpickler.shared_lambda_owner_conflict",
            Self::SharedRefinementOwnerConflict { .. } => {
                "unpickler.shared_refinement_owner_conflict"
            }
            Self::UnsupportedRefinementStat { .. } => "unpickler.unsupported_refinement_stat",
            Self::MissingRefinementClass { .. } => "unpickler.missing_refinement_class",
            Self::InvalidRefinementClass { .. } => "unpickler.invalid_refinement_class",
            Self::MissingRefinementScope { .. } => "unpickler.missing_refinement_scope",
            Self::MalformedRefinedTypeTree { reason, .. } => reason.code(),
            Self::UnsupportedRefinementOverload { .. } => {
                "unpickler.unsupported_refinement_overload"
            }
            Self::CloseOverThis { .. } => "unpickler.close_over_this",
            Self::ParameterAbstraction { .. } => "unpickler.parameter_abstraction",
            Self::UnstableSelectQualifier { .. } => "unpickler.unstable_select_qualifier",
            Self::InvalidSingletonTypeTree { .. } => "unpickler.invalid_singleton_type_tree",
            Self::MissingEnteredSymbol { .. } => "unpickler.missing_entered_symbol",
            Self::UnsupportedSymbolCompletion { .. } => "unpickler.unsupported_symbol_completion",
            Self::UnsupportedAnnotationCompletion { .. } => {
                "unpickler.unsupported_annotation_completion"
            }
            Self::OpaqueAliasOwnerNotClassLike { .. } => {
                "unpickler.opaque_alias_owner_not_class_like"
            }
            Self::OpaqueAliasOwnerNotEntered { .. } => "unpickler.opaque_alias_owner_not_entered",
            Self::OpaqueAliasCycle { .. } => "unpickler.opaque_alias_cycle",
            Self::SymbolCompletionDeferred { .. } => "unpickler.symbol_completion_deferred",
            Self::SymbolInfoError { .. } => "unpickler.symbol_info_error",
            Self::InvalidCompletedBounds { .. } => "unpickler.invalid_completed_bounds",
            Self::UnsupportedType { .. } => "unpickler.unsupported_type",
            Self::MissingReferencedSymbol { .. } => "unpickler.missing_referenced_symbol",
            Self::InvalidReferenceKind { .. } => "unpickler.invalid_reference_kind",
            Self::UnresolvedPackage { .. } => "unpickler.unresolved_package",
            Self::UnresolvedMember { .. } => "unpickler.unresolved_member",
            Self::AmbiguousMember { .. } => "unpickler.ambiguous_member",
            Self::UnsupportedSignedReference { .. } => "unpickler.unsupported_signed_reference",
            Self::MalformedType { reason, .. } => reason.code(),
            Self::UnsupportedAnnotationTree { .. } => "unpickler.unsupported_annotation_tree",
            Self::InvalidCompactAnnotationType { .. } => {
                "unpickler.invalid_compact_annotation_type"
            }
            Self::UnsupportedAnnotationConstructor { .. } => {
                "unpickler.unsupported_annotation_constructor"
            }
            Self::UnsupportedAnnotationArgument { .. } => {
                "unpickler.unsupported_annotation_argument"
            }
            Self::InvalidAnnotationType { .. } => "unpickler.invalid_annotation_type",
            Self::BoundsVarianceTargetPending { .. } => "unpickler.bounds_variance_target_pending",
            Self::BoundsVarianceArityMismatch { .. } => "unpickler.bounds_variance_arity_mismatch",
            Self::RebindFailed { .. } => "unpickler.rebind_failed",
            Self::InvalidMethodModifier { .. } => "unpickler.invalid_method_modifier",
            Self::InvalidBinderReference { .. } => "unpickler.invalid_binder_reference",
            Self::InvalidBinderKind { .. } => "unpickler.invalid_binder_kind",
            Self::InvalidParameterIndex { .. } => "unpickler.invalid_parameter_index",
            Self::InvalidTypeParameterBounds { .. } => "unpickler.invalid_type_parameter_bounds",
            Self::UnsupportedResolutionPrefix { .. } => "unpickler.unsupported_resolution_prefix",
            Self::UnsupportedResolutionSpace { .. } => "unpickler.unsupported_resolution_space",
            Self::IllegalTypePrefix { .. } => "unpickler.illegal_type_prefix",
            Self::ResolverFailure { .. } => "unpickler.resolver_failure",
        }
    }

    /// Classifies the error without inspecting its rendered message. Cases
    /// whose public payload omits the evidence needed for a definitive result
    /// are explicitly returned as [`UnpickleErrorCategory::Conditional`].
    pub fn category(&self) -> UnpickleErrorCategory {
        use UnpickleErrorCategory as Category;
        match self {
            Self::Tasty(error) => tasty_category(error),
            Self::Ast(error) => ast_category(error),
            Self::DuplicateDefinition { .. }
            | Self::DuplicateScope { .. }
            | Self::ConflictingCompanion { .. }
            | Self::DuplicateType { .. }
            | Self::MissingClassScope { .. }
            | Self::MalformedOwnerClassInfo { .. }
            | Self::MissingRefinementClass { .. }
            | Self::InvalidRefinementClass { .. }
            | Self::MissingRefinementScope { .. }
            | Self::MissingEnteredSymbol { .. }
            | Self::SymbolInfoError { .. } => Category::InternalInvariant,
            Self::CloseOverThis { error, .. }
            | Self::ParameterAbstraction { error, .. }
            | Self::RebindFailed { error, .. } => type_rebind_category(*error),
            Self::InvalidNameReference { .. }
            | Self::InvalidReferenceTarget { .. }
            | Self::ConstructorOwnerNotClassLike { .. }
            | Self::OpaqueAliasOwnerNotClassLike { .. }
            | Self::OpaqueAliasOwnerNotEntered { .. }
            | Self::UnsupportedType { .. }
            | Self::InvalidCompactAnnotationType { .. }
            | Self::InvalidAnnotationType { .. } => Category::Conditional,
            Self::UnsupportedName { .. }
            | Self::UnsupportedQualifier { .. }
            | Self::UnsupportedPackagePath { .. }
            | Self::UnsupportedTypeTree { .. }
            | Self::UnsupportedTermTree { .. }
            | Self::UnsupportedParentTree { .. }
            | Self::UnsupportedMethodParameterSemantics { .. }
            | Self::SharedLambdaOwnerConflict { .. }
            | Self::SharedRefinementOwnerConflict { .. }
            | Self::UnsupportedRefinementStat { .. }
            | Self::UnsupportedRefinementOverload { .. }
            | Self::UnstableSelectQualifier { .. }
            | Self::UnsupportedAnnotationCompletion { .. }
            | Self::UnsupportedSignedReference { .. }
            | Self::UnsupportedAnnotationTree { .. }
            | Self::UnsupportedAnnotationConstructor { .. }
            | Self::UnsupportedAnnotationArgument { .. }
            | Self::UnsupportedResolutionPrefix { .. }
            | Self::UnsupportedResolutionSpace { .. }
            | Self::IllegalTypePrefix { .. } => Category::UnsupportedFeature,
            Self::MalformedParentTree { reason, .. } => reason.category(),
            Self::InvalidSelfTypeTree { .. } => Category::MalformedInput,
            Self::MalformedDefinition { reason, .. } => reason.category(),
            Self::MalformedRefinedTypeTree { reason, .. } => reason.category(),
            Self::MalformedType { reason, .. } => reason.category(),
            Self::MissingDefinition { .. }
            | Self::InvalidQualifier { .. }
            | Self::InvalidSingletonTypeTree { .. }
            | Self::InvalidCompletedBounds { .. }
            | Self::InvalidReferenceKind { .. }
            | Self::BoundsVarianceArityMismatch { .. }
            | Self::InvalidMethodModifier { .. }
            | Self::InvalidBinderReference { .. }
            | Self::InvalidBinderKind { .. }
            | Self::InvalidParameterIndex { .. }
            | Self::InvalidTypeParameterBounds { .. } => Category::MalformedInput,
            Self::UnresolvedPackage { .. }
            | Self::UnresolvedMember { .. }
            | Self::AmbiguousMember { .. }
            | Self::ResolverFailure { .. }
            | Self::MissingReferencedSymbol { .. } => Category::ResolutionFailure,
            Self::UnsupportedSymbolCompletion { .. }
            | Self::SymbolCompletionDeferred { .. }
            | Self::BoundsVarianceTargetPending { .. } => Category::Deferred,
            Self::OpaqueAliasCycle { .. } => Category::MalformedInput,
        }
    }
}

fn type_rebind_category(error: TypeRebindError) -> UnpickleErrorCategory {
    match error {
        TypeRebindError::TooDeep { .. } => UnpickleErrorCategory::UnsupportedFeature,
        TypeRebindError::DuplicateParameterSymbol { .. }
        | TypeRebindError::DuplicateSubstitutionSymbol { .. } => {
            UnpickleErrorCategory::InternalInvariant
        }
        _ => UnpickleErrorCategory::Conditional,
    }
}

impl ParentTreeReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::NestingLimit => "a parent constructor call nests too deeply",
            Self::ConstructorHasFunction => "a parent constructor call has a function",
            Self::SelectsConstructor => "a parent constructor selection does not select <init>",
            Self::SelectsNew => "a parent constructor selection is not on NEW",
            Self::NewArity => "NEW has one type tree",
        }
    }

    fn code(self) -> &'static str {
        match self {
            Self::NestingLimit => "unpickler.malformed_parent_tree.nesting_limit",
            Self::ConstructorHasFunction => {
                "unpickler.malformed_parent_tree.constructor_has_function"
            }
            Self::SelectsConstructor => "unpickler.malformed_parent_tree.selects_constructor",
            Self::SelectsNew => "unpickler.malformed_parent_tree.selects_new",
            Self::NewArity => "unpickler.malformed_parent_tree.new_arity",
        }
    }

    fn category(self) -> UnpickleErrorCategory {
        match self {
            Self::NestingLimit => UnpickleErrorCategory::UnsupportedFeature,
            _ => UnpickleErrorCategory::MalformedInput,
        }
    }
}

impl fmt::Display for ParentTreeReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl SelfTypeTreeReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::TypeTreeArity => "a self definition has one type tree",
        }
    }

    fn code(self) -> &'static str {
        match self {
            Self::TypeTreeArity => "unpickler.invalid_self_type_tree.type_tree_arity",
        }
    }
}

impl fmt::Display for SelfTypeTreeReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl DefinitionReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::ClassTemplatePosition => "a class definition's first child is its template",
            Self::ResultTypePosition => "a method has a result type after its parameters",
            Self::HeaderParametersExceedNodes => "more header parameters than parameter nodes",
            Self::HeaderParameterTagMismatch => "a header parameter disagrees with its node",
            Self::ParameterNodesExceedHeader => "more parameter nodes than header parameters",
            Self::OpaqueAliasRhsArity => "an opaque type alias has one right-hand-side tree",
            Self::OpaqueBoundsShape => "opaque bounds children do not match their encoded shape",
            Self::OpaqueAliasDepthLimit => {
                "an opaque alias dependency graph exceeds the recursion limit"
            }
            Self::TypeAliasRhsArity => "a type alias has one right-hand-side tree",
            Self::OpaqueLambdaShape => "a generic opaque alias lambda has parameters and a body",
            Self::NestedOpaqueParameterLambdasUnsupported => {
                "nested opaque alias parameter lambdas are unsupported"
            }
        }
    }

    fn code(self) -> &'static str {
        match self {
            Self::ClassTemplatePosition => "unpickler.malformed_definition.class_template_position",
            Self::ResultTypePosition => "unpickler.malformed_definition.result_type_position",
            Self::HeaderParametersExceedNodes => {
                "unpickler.malformed_definition.header_parameters_exceed_nodes"
            }
            Self::HeaderParameterTagMismatch => {
                "unpickler.malformed_definition.header_parameter_tag_mismatch"
            }
            Self::ParameterNodesExceedHeader => {
                "unpickler.malformed_definition.parameter_nodes_exceed_header"
            }
            Self::OpaqueAliasRhsArity => "unpickler.malformed_definition.opaque_alias_rhs_arity",
            Self::OpaqueBoundsShape => "unpickler.malformed_definition.opaque_bounds_shape",
            Self::OpaqueAliasDepthLimit => {
                "unpickler.malformed_definition.opaque_alias_depth_limit"
            }
            Self::TypeAliasRhsArity => "unpickler.malformed_definition.type_alias_rhs_arity",
            Self::OpaqueLambdaShape => "unpickler.malformed_definition.opaque_lambda_shape",
            Self::NestedOpaqueParameterLambdasUnsupported => {
                "unpickler.malformed_definition.nested_opaque_parameter_lambdas"
            }
        }
    }

    fn category(self) -> UnpickleErrorCategory {
        match self {
            Self::OpaqueAliasDepthLimit | Self::NestedOpaqueParameterLambdasUnsupported => {
                UnpickleErrorCategory::UnsupportedFeature
            }
            _ => UnpickleErrorCategory::MalformedInput,
        }
    }
}

impl fmt::Display for DefinitionReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl RefinedTypeTreeReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::UnexpectedParent => "a refined type tree has a parent",
            Self::UnsupportedMemberStat => "a refinement stat is not a supported member definition",
        }
    }

    fn code(self) -> &'static str {
        match self {
            Self::UnexpectedParent => "unpickler.malformed_refined_type_tree.unexpected_parent",
            Self::UnsupportedMemberStat => {
                "unpickler.malformed_refined_type_tree.unsupported_member_stat"
            }
        }
    }

    fn category(self) -> UnpickleErrorCategory {
        match self {
            Self::UnexpectedParent => UnpickleErrorCategory::MalformedInput,
            Self::UnsupportedMemberStat => UnpickleErrorCategory::UnsupportedFeature,
        }
    }
}

impl fmt::Display for RefinedTypeTreeReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl TypeReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::MatchTypeShape => "a match type has a bound and a scrutinee",
            Self::ChildShapeMismatch => "the node's children do not match its shape",
            Self::OwnerSpaceNameDesignator => {
                "an owner-space reference resolved to a name designator"
            }
            Self::RefinedTypeShape => "a refined type has a parent and a refinement",
            Self::InvalidReferenceTargetShape => "not a direct or symbol reference",
            Self::BinderChildMismatch => "the binder's children disagree with its parameters",
            Self::EmptyBinder => "the binder has no parameters",
            Self::InvalidVarianceMarker => "a bounds tail holds a marker that is not a variance",
            Self::AnnotationTailShape => {
                "an ANNOTATION tail entry has a tycon and a full annotation"
            }
            Self::AnnotationOfAnnotatedTypeIsNotNode => {
                "the annotation of an annotated type is not a node"
            }
            Self::AnnotationPayloadIsNotNode => "the annotation is not a node",
            Self::AnnotatedTypeShape => "an annotated type has a parent and an annotation",
            Self::IdentifierTypeTreeArity => "an identifier type tree has one type",
            Self::AnnotationApplyFunctionMissing => "an application has a function",
            Self::AnnotationApplyArityMismatch => {
                "an application's arguments do not match its shape"
            }
            Self::AnnotationTypeApplyFunctionMissing => "a type application has a function",
            Self::AnnotationTypeApplyArityMismatch => {
                "a type application's arguments do not match its shape"
            }
            Self::AnnotationSelectionShape => "a selection has a qualifier and an owner",
            Self::AnnotationNewClassArity => "a `new` has one class",
            Self::NamedArgumentArity => "a named argument has one value",
            Self::AnnotationLiteralNotConstant => {
                "an annotation argument literal is not a constant"
            }
            Self::RecursiveTypeArity => "a recursive type has exactly one child",
            Self::ParentTreeDepthLimit => "a parent tree nests too deeply",
            Self::TypeTreeDepthLimit => "a type tree nests too deeply",
            Self::LambdaTreeShape => "a lambda type tree has type parameters and a body",
            Self::LambdaParameterKind => "a lambda type tree parameter is not a type parameter",
            Self::RefinedTreeUnexpectedParent => "a refined type tree has a parent",
            Self::IdentifierTermTypeArity => "an identifier has one type",
            Self::TermSelectionQualifierArity => "a selection has one qualifier",
            Self::QualifiedThisQualifierArity => "a qualified this has one qualifier",
            Self::QualifiedThisQualifierKind => {
                "the qualifier of a qualified this is not a type identifier"
            }
            Self::ExplicitTypeTreeArity => "an explicit type tree has one child",
            Self::ByNameTypeTreeArity => "a by-name type tree has one child",
            Self::AppliedTypeConstructorMissing => "an applied type tree has a constructor",
            Self::BoundsTypeTreeArity => "a bounds type tree has one to three children",
            Self::TypeSelectionQualifierArity => "a type selection has one qualifier",
            Self::SingletonTreeReferenceArity => "a singleton type tree has one reference",
            Self::AnnotatedTypeTreeShape => {
                "an annotated type tree has a type tree and an annotation"
            }
        }
    }

    fn code(self) -> &'static str {
        match self {
            Self::MatchTypeShape => "unpickler.malformed_type.match_type_shape",
            Self::ChildShapeMismatch => "unpickler.malformed_type.child_shape_mismatch",
            Self::OwnerSpaceNameDesignator => {
                "unpickler.malformed_type.owner_space_name_designator"
            }
            Self::RefinedTypeShape => "unpickler.malformed_type.refined_type_shape",
            Self::InvalidReferenceTargetShape => {
                "unpickler.malformed_type.invalid_reference_target_shape"
            }
            Self::BinderChildMismatch => "unpickler.malformed_type.binder_child_mismatch",
            Self::EmptyBinder => "unpickler.malformed_type.empty_binder",
            Self::InvalidVarianceMarker => "unpickler.malformed_type.invalid_variance_marker",
            Self::AnnotationTailShape => "unpickler.malformed_type.annotation_tail_shape",
            Self::AnnotationOfAnnotatedTypeIsNotNode => {
                "unpickler.malformed_type.annotation_of_annotated_type_is_not_node"
            }
            Self::AnnotationPayloadIsNotNode => {
                "unpickler.malformed_type.annotation_payload_is_not_node"
            }
            Self::AnnotatedTypeShape => "unpickler.malformed_type.annotated_type_shape",
            Self::IdentifierTypeTreeArity => "unpickler.malformed_type.identifier_type_tree_arity",
            Self::AnnotationApplyFunctionMissing => {
                "unpickler.malformed_type.annotation_apply_function_missing"
            }
            Self::AnnotationApplyArityMismatch => {
                "unpickler.malformed_type.annotation_apply_arity_mismatch"
            }
            Self::AnnotationTypeApplyFunctionMissing => {
                "unpickler.malformed_type.annotation_type_apply_function_missing"
            }
            Self::AnnotationTypeApplyArityMismatch => {
                "unpickler.malformed_type.annotation_type_apply_arity_mismatch"
            }
            Self::AnnotationSelectionShape => "unpickler.malformed_type.annotation_selection_shape",
            Self::AnnotationNewClassArity => "unpickler.malformed_type.annotation_new_class_arity",
            Self::NamedArgumentArity => "unpickler.malformed_type.named_argument_arity",
            Self::AnnotationLiteralNotConstant => {
                "unpickler.malformed_type.annotation_literal_not_constant"
            }
            Self::RecursiveTypeArity => "unpickler.malformed_type.recursive_type_arity",
            Self::ParentTreeDepthLimit => "unpickler.malformed_type.parent_tree_depth_limit",
            Self::TypeTreeDepthLimit => "unpickler.malformed_type.type_tree_depth_limit",
            Self::LambdaTreeShape => "unpickler.malformed_type.lambda_tree_shape",
            Self::LambdaParameterKind => "unpickler.malformed_type.lambda_parameter_kind",
            Self::RefinedTreeUnexpectedParent => {
                "unpickler.malformed_type.refined_tree_unexpected_parent"
            }
            Self::IdentifierTermTypeArity => "unpickler.malformed_type.identifier_term_type_arity",
            Self::TermSelectionQualifierArity => {
                "unpickler.malformed_type.term_selection_qualifier_arity"
            }
            Self::QualifiedThisQualifierArity => {
                "unpickler.malformed_type.qualified_this_qualifier_arity"
            }
            Self::QualifiedThisQualifierKind => {
                "unpickler.malformed_type.qualified_this_qualifier_kind"
            }
            Self::ExplicitTypeTreeArity => "unpickler.malformed_type.explicit_type_tree_arity",
            Self::ByNameTypeTreeArity => "unpickler.malformed_type.by_name_type_tree_arity",
            Self::AppliedTypeConstructorMissing => {
                "unpickler.malformed_type.applied_type_constructor_missing"
            }
            Self::BoundsTypeTreeArity => "unpickler.malformed_type.bounds_type_tree_arity",
            Self::TypeSelectionQualifierArity => {
                "unpickler.malformed_type.type_selection_qualifier_arity"
            }
            Self::SingletonTreeReferenceArity => {
                "unpickler.malformed_type.singleton_tree_reference_arity"
            }
            Self::AnnotatedTypeTreeShape => "unpickler.malformed_type.annotated_type_tree_shape",
        }
    }

    fn category(self) -> UnpickleErrorCategory {
        match self {
            Self::OwnerSpaceNameDesignator
            | Self::ParentTreeDepthLimit
            | Self::TypeTreeDepthLimit => UnpickleErrorCategory::UnsupportedFeature,
            _ => UnpickleErrorCategory::MalformedInput,
        }
    }
}

impl fmt::Display for TypeReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

fn tasty_category(error: &TastyFileError) -> UnpickleErrorCategory {
    use UnpickleErrorCategory::{MalformedInput, UnsupportedFeature};
    match error {
        TastyFileError::Header(
            dotty_tasty::tasty::HeaderError::UnsupportedVersion { .. }
            | dotty_tasty::tasty::HeaderError::IncompatibleVersion { .. },
        ) => UnsupportedFeature,
        TastyFileError::Asts(error) => ast_category(error),
        _ => MalformedInput,
    }
}

fn ast_category(error: &AstError) -> UnpickleErrorCategory {
    use UnpickleErrorCategory::{InternalInvariant, MalformedInput, UnsupportedFeature};
    match error {
        AstError::UnsupportedCategory { .. }
        | AstError::UnsupportedRelocation { .. }
        | AstError::RecursionLimit { .. } => UnsupportedFeature,
        AstError::RelocationDidNotConverge { .. } => InternalInvariant,
        AstError::Term(error) => term_category(error),
        AstError::Read(_)
        | AstError::InvalidTag { .. }
        | AstError::UnexpectedTag { .. }
        | AstError::InvalidAstReference { .. }
        | AstError::AstNodeCountMismatch { .. } => MalformedInput,
    }
}

fn term_category(error: &TermError) -> UnpickleErrorCategory {
    match error {
        TermError::UnsupportedCategory { .. } | TermError::RecursionLimit { .. } => {
            UnpickleErrorCategory::UnsupportedFeature
        }
        TermError::Read(_) | TermError::InvalidTag { .. } | TermError::InvalidConstant { .. } => {
            UnpickleErrorCategory::MalformedInput
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_tasty::tasty::HeaderError;

    #[test]
    fn tasty_file_errors_convert_and_expose_their_source() {
        let source = TastyFileError::Header(HeaderError::InvalidMagic {
            actual: [0, 0, 0, 0],
        });
        let error = UnpickleError::from(source.clone());

        assert_eq!(error, UnpickleError::Tasty(source));
        assert!(std::error::Error::source(&error).is_some());
        assert!(error.to_string().starts_with("invalid TASTy file"));
    }

    #[test]
    fn duplicate_definition_names_the_address_and_has_no_source() {
        let error = UnpickleError::DuplicateDefinition { address: 46 };

        assert!(error.to_string().contains("address 46"));
        assert!(std::error::Error::source(&error).is_none());
    }

    #[test]
    fn ast_errors_convert_and_expose_their_source() {
        let source = AstError::InvalidTag { tag: 1, offset: 7 };
        let error = UnpickleError::from(source.clone());

        assert_eq!(error, UnpickleError::Ast(source));
        assert!(std::error::Error::source(&error).is_some());
        assert!(error.to_string().starts_with("invalid TASTy AST node"));
    }

    #[test]
    fn nested_decode_causes_keep_malformed_and_unsupported_categories_distinct() {
        let unsupported_version =
            UnpickleError::Tasty(TastyFileError::Header(HeaderError::UnsupportedVersion {
                major: 28,
                minor: 10,
                experimental: 0,
            }));
        assert_eq!(
            unsupported_version.category(),
            UnpickleErrorCategory::UnsupportedFeature
        );
        assert_eq!(unsupported_version.code(), "unpickler.tasty");

        let unsupported_ast = UnpickleError::Ast(AstError::UnsupportedCategory {
            tag: 255,
            offset: 12,
        });
        assert_eq!(
            unsupported_ast.category(),
            UnpickleErrorCategory::UnsupportedFeature
        );

        let malformed_ast = UnpickleError::Ast(AstError::InvalidTag {
            tag: 255,
            offset: 12,
        });
        assert_eq!(
            malformed_ast.category(),
            UnpickleErrorCategory::MalformedInput
        );

        let nested_term_limit = UnpickleError::Tasty(TastyFileError::Asts(AstError::Term(
            TermError::RecursionLimit {
                offset: 12,
                limit: 256,
            },
        )));
        assert_eq!(
            nested_term_limit.category(),
            UnpickleErrorCategory::UnsupportedFeature
        );
        assert_eq!(
            UnpickleError::Ast(AstError::Term(TermError::RecursionLimit {
                offset: 12,
                limit: 256,
            }))
            .category(),
            nested_term_limit.category()
        );
    }

    #[test]
    fn nested_type_rebind_limits_and_invariants_keep_their_categories() {
        let mut store = dotty_core::SemanticStore::new();
        let type_id = store.types.alloc(dotty_core::Type::NoType);
        let symbol = store.symbols.alloc(dotty_core::symbols::Symbol {
            name: dotty_core::Name::new(
                store.names.intern("parameter"),
                dotty_core::Namespace::Term,
            ),
            owner: None,
            kind: SymbolKind::Value,
            flags: dotty_core::symbols::SymbolFlags::EMPTY,
            visibility: dotty_core::symbols::Visibility::Public,
            info: dotty_core::symbols::SymbolInfo::Missing,
            origin: dotty_core::symbols::SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: dotty_core::symbols::SymbolLinks::default(),
        });
        let too_deep = TypeRebindError::TooDeep { id: type_id };
        let duplicate_parameter = TypeRebindError::DuplicateParameterSymbol { symbol };

        for error in [
            UnpickleError::CloseOverThis {
                address: 4,
                error: too_deep,
            },
            UnpickleError::ParameterAbstraction {
                address: 4,
                error: too_deep,
            },
        ] {
            assert_eq!(error.category(), UnpickleErrorCategory::UnsupportedFeature);
        }

        for error in [
            UnpickleError::CloseOverThis {
                address: 4,
                error: duplicate_parameter,
            },
            UnpickleError::ParameterAbstraction {
                address: 4,
                error: duplicate_parameter,
            },
        ] {
            assert_eq!(error.category(), UnpickleErrorCategory::InternalInvariant);
        }
    }

    #[test]
    fn typed_reason_families_have_distinct_stable_codes() {
        let mut codes = std::collections::BTreeSet::new();
        let mut add = |error: UnpickleError| {
            let code = error.code();
            assert!(code.starts_with("unpickler."));
            assert!(codes.insert(code), "duplicate code {code}");
        };

        for reason in [
            ParentTreeReason::NestingLimit,
            ParentTreeReason::ConstructorHasFunction,
            ParentTreeReason::SelectsConstructor,
            ParentTreeReason::SelectsNew,
            ParentTreeReason::NewArity,
        ] {
            add(UnpickleError::MalformedParentTree { address: 4, reason });
        }
        assert_eq!(
            ParentTreeReason::SelectsConstructor.to_string(),
            "a parent constructor selection does not select <init>"
        );
        assert_eq!(
            ParentTreeReason::SelectsNew.to_string(),
            "a parent constructor selection is not on NEW"
        );
        add(UnpickleError::InvalidSelfTypeTree {
            address: 4,
            reason: SelfTypeTreeReason::TypeTreeArity,
        });
        for reason in [
            DefinitionReason::ClassTemplatePosition,
            DefinitionReason::ResultTypePosition,
            DefinitionReason::HeaderParametersExceedNodes,
            DefinitionReason::HeaderParameterTagMismatch,
            DefinitionReason::ParameterNodesExceedHeader,
            DefinitionReason::OpaqueAliasRhsArity,
            DefinitionReason::OpaqueBoundsShape,
            DefinitionReason::OpaqueAliasDepthLimit,
            DefinitionReason::TypeAliasRhsArity,
            DefinitionReason::OpaqueLambdaShape,
            DefinitionReason::NestedOpaqueParameterLambdasUnsupported,
        ] {
            add(UnpickleError::MalformedDefinition { address: 4, reason });
        }
        for reason in [
            RefinedTypeTreeReason::UnexpectedParent,
            RefinedTypeTreeReason::UnsupportedMemberStat,
        ] {
            add(UnpickleError::MalformedRefinedTypeTree { address: 4, reason });
        }
        for reason in [
            TypeReason::MatchTypeShape,
            TypeReason::ChildShapeMismatch,
            TypeReason::OwnerSpaceNameDesignator,
            TypeReason::RefinedTypeShape,
            TypeReason::InvalidReferenceTargetShape,
            TypeReason::BinderChildMismatch,
            TypeReason::EmptyBinder,
            TypeReason::InvalidVarianceMarker,
            TypeReason::AnnotationTailShape,
            TypeReason::AnnotationOfAnnotatedTypeIsNotNode,
            TypeReason::AnnotationPayloadIsNotNode,
            TypeReason::AnnotatedTypeShape,
            TypeReason::IdentifierTypeTreeArity,
            TypeReason::AnnotationApplyFunctionMissing,
            TypeReason::AnnotationApplyArityMismatch,
            TypeReason::AnnotationTypeApplyFunctionMissing,
            TypeReason::AnnotationTypeApplyArityMismatch,
            TypeReason::AnnotationSelectionShape,
            TypeReason::AnnotationNewClassArity,
            TypeReason::NamedArgumentArity,
            TypeReason::AnnotationLiteralNotConstant,
            TypeReason::RecursiveTypeArity,
            TypeReason::ParentTreeDepthLimit,
            TypeReason::TypeTreeDepthLimit,
            TypeReason::LambdaTreeShape,
            TypeReason::LambdaParameterKind,
            TypeReason::RefinedTreeUnexpectedParent,
            TypeReason::IdentifierTermTypeArity,
            TypeReason::TermSelectionQualifierArity,
            TypeReason::QualifiedThisQualifierArity,
            TypeReason::QualifiedThisQualifierKind,
            TypeReason::ExplicitTypeTreeArity,
            TypeReason::ByNameTypeTreeArity,
            TypeReason::AppliedTypeConstructorMissing,
            TypeReason::BoundsTypeTreeArity,
            TypeReason::TypeSelectionQualifierArity,
            TypeReason::SingletonTreeReferenceArity,
            TypeReason::AnnotatedTypeTreeShape,
        ] {
            add(UnpickleError::MalformedType { address: 4, reason });
        }
        assert_eq!(codes.len(), 57);
    }

    #[test]
    fn categories_distinguish_malformed_unsupported_resolution_deferred_and_invariant() {
        use UnpickleErrorCategory as Category;
        let mut store = dotty_core::SemanticStore::new();
        let prefix = store.types.alloc(dotty_core::types::Type::NoType);

        assert_eq!(
            UnpickleError::MalformedType {
                address: 1,
                reason: TypeReason::ChildShapeMismatch,
            }
            .category(),
            Category::MalformedInput
        );
        assert_eq!(
            UnpickleError::MalformedType {
                address: 1,
                reason: TypeReason::TypeTreeDepthLimit,
            }
            .category(),
            Category::UnsupportedFeature
        );
        assert_eq!(
            UnpickleError::UnresolvedMember {
                address: 1,
                prefix,
                space: None,
                name: "missing".to_owned(),
                namespace: Namespace::Type,
            }
            .category(),
            Category::ResolutionFailure
        );
        assert_eq!(
            UnpickleError::UnsupportedSymbolCompletion {
                address: 1,
                kind: SymbolKind::Class,
            }
            .category(),
            Category::Deferred
        );
        assert_eq!(
            UnpickleError::DuplicateDefinition { address: 1 }.category(),
            Category::InternalInvariant
        );
        assert_eq!(
            UnpickleError::InvalidNameReference {
                reference: u32::MAX
            }
            .category(),
            Category::Conditional
        );
    }

    #[test]
    fn classification_and_codes_do_not_dereference_rolled_back_ids() {
        let mut store = dotty_core::SemanticStore::new();
        let checkpoint = store.checkpoint();
        let invalidated_binder = store.types.alloc(dotty_core::types::Type::NoType);
        store.rollback_to(checkpoint);
        let error = UnpickleError::InvalidBinderKind {
            from: 32,
            binder: invalidated_binder,
        };

        assert_eq!(error.code(), "unpickler.invalid_binder_kind");
        assert_eq!(error.category(), UnpickleErrorCategory::MalformedInput);
    }

    #[test]
    fn typed_reasons_keep_the_audited_mixed_categories() {
        use UnpickleErrorCategory::{MalformedInput, UnsupportedFeature};

        assert_eq!(
            UnpickleError::MalformedParentTree {
                address: 1,
                reason: ParentTreeReason::NestingLimit,
            }
            .category(),
            UnsupportedFeature
        );
        assert_eq!(
            UnpickleError::MalformedDefinition {
                address: 1,
                reason: DefinitionReason::OpaqueAliasDepthLimit,
            }
            .category(),
            UnsupportedFeature
        );
        assert_eq!(
            UnpickleError::MalformedDefinition {
                address: 1,
                reason: DefinitionReason::NestedOpaqueParameterLambdasUnsupported,
            }
            .category(),
            UnsupportedFeature
        );
        assert_eq!(
            UnpickleError::MalformedRefinedTypeTree {
                address: 1,
                reason: RefinedTypeTreeReason::UnsupportedMemberStat,
            }
            .category(),
            UnsupportedFeature
        );
        assert_eq!(
            UnpickleError::MalformedType {
                address: 1,
                reason: TypeReason::OwnerSpaceNameDesignator,
            }
            .category(),
            UnsupportedFeature
        );
        assert_eq!(
            UnpickleError::MalformedType {
                address: 1,
                reason: TypeReason::ParentTreeDepthLimit,
            }
            .category(),
            UnsupportedFeature
        );
        assert_eq!(
            UnpickleError::MalformedType {
                address: 1,
                reason: TypeReason::MatchTypeShape,
            }
            .category(),
            MalformedInput
        );
    }

    #[test]
    fn typed_reasons_preserve_readable_display_text() {
        let error = UnpickleError::MalformedType {
            address: 19,
            reason: TypeReason::QualifiedThisQualifierKind,
        };

        assert_eq!(
            error.to_string(),
            "the type at address 19 is malformed: the qualifier of a qualified this is not a type identifier"
        );
    }

    #[test]
    fn stable_codes_ignore_addresses_and_tags_while_errors_keep_their_payloads() {
        let first = UnpickleError::UnsupportedTypeTree {
            address: 17,
            tag: 84,
        };
        let second = UnpickleError::UnsupportedTypeTree {
            address: 99,
            tag: 85,
        };
        assert_eq!(first.code(), "unpickler.unsupported_type_tree");
        assert_eq!(first.code(), second.code());
        assert_eq!(first.category(), UnpickleErrorCategory::UnsupportedFeature);
        assert!(matches!(
            first,
            UnpickleError::UnsupportedTypeTree {
                address: 17,
                tag: 84
            }
        ));

        let resolver_cause = dotty_core::ResolutionError::Ambiguous { candidates: 3 };
        let error = UnpickleError::ResolverFailure {
            address: 41,
            error: resolver_cause.clone(),
        };
        assert_eq!(error.code(), "unpickler.resolver_failure");
        assert_eq!(error.category(), UnpickleErrorCategory::ResolutionFailure);
        assert!(
            matches!(error, UnpickleError::ResolverFailure { address: 41, error } if error == resolver_cause)
        );
    }
}
