//! Source declaration completion driver.

use std::fmt;

use dotty_core::ast::{
    ApplyKind, Ident, NumberKind, NumberLiteral, TreeKind, TypeBoundsTree, TypedAstBuilder,
    UntypedNode,
};
use dotty_core::types::{
    ClassInfo, MethodKind, MethodParamSpec, MethodType, TermRefTarget, Type, TypeParamSpec,
    TypeRefTarget, method_type_from_symbols, poly_type_from_symbols,
};
use dotty_core::{
    AstArena, Definitions, MemberRequest, MemberSelector, MemberSpace, NoResolver, Packages,
    ResolutionError, SemanticStore, SourceId, SourceSpan, SymbolFlags, SymbolId, SymbolInfo,
    SymbolKind, SymbolOrigin, SymbolResolver, TreeId, TypeId, Typed, Untyped,
};
use dotty_namer::{SourceContextId, SourceDefinition, SourceSemanticIndex};

use crate::{SourceTypeIndex, SourceTypedIndex};

#[path = "lookup/mod.rs"]
mod lookup;
#[path = "substitution.rs"]
mod substitution;
#[path = "types/subtype.rs"]
mod subtype;

pub use lookup::{MAX_MEMBER_LOOKUP_DEPTH, MemberCandidate, MemberLookupError};
pub use subtype::{MAX_TYPE_RELATION_DEPTH, MAX_TYPE_RELATION_VIEWS, TypeRelationError};

/// A recoverable failure while projecting or completing source semantics.
#[derive(Debug)]
pub enum TyperError {
    /// A symbol is not present in the semantic store.
    UnknownSymbol { symbol: SymbolId },
    /// The namer did not retain source provenance for this symbol.
    SourceProvenanceMissing { symbol: SymbolId },
    /// A declaration that needs lexical lookup has no namer context.
    DeclarationContextMissing { symbol: SymbolId },
    /// A source context ID is outside the naming result that produced it.
    SourceContextMissing {
        source: SourceId,
        tree_index: u32,
        context_index: u32,
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
    /// An import tree has an unexpected AST shape or stale child reference.
    MalformedSourceImport {
        source: SourceId,
        import_tree_index: u32,
    },
    /// Completion for this semantic declaration category is not implemented.
    UnsupportedSymbolCompletion { symbol: SymbolId, kind: SymbolKind },
    /// A source class-like declaration has no declaration scope from naming.
    MissingClassScope { symbol: SymbolId },
    /// A class-like symbol already contains an incompatible complete type.
    MalformedClassInfo { symbol: SymbolId, info: TypeId },
    /// A template parent is not a type or a supported constructor-call shape.
    MalformedClassParent { source: SourceId, tree_index: u32 },
    /// A projected parent does not resolve to a class or trait declaration.
    UnresolvedParentClassKind {
        source: SourceId,
        tree_index: u32,
        symbol: Option<SymbolId>,
    },
    /// Higher-kinded source type parameter completion is deferred.
    HigherKindedTypeParameterDeferred { symbol: SymbolId, tree_index: u32 },
    /// Higher-kinded source type alias completion is deferred.
    HigherKindedTypeAliasDeferred { symbol: SymbolId, tree_index: u32 },
    /// A type alias RHS cannot be represented by source alias bounds.
    InvalidCompletedBounds {
        source: SourceId,
        tree_index: u32,
        ty: TypeId,
    },
    /// Opaque alias completion is deferred until opaque visibility is modeled.
    OpaqueAliasDeferred { symbol: SymbolId, tree_index: u32 },
    /// Method result inference is deferred to expression typing.
    InferredMethodResultDeferred { symbol: SymbolId, tree_index: u32 },
    /// Extension signature normalization for right-associative methods is deferred.
    RightAssociativeExtensionDeferred { symbol: SymbolId, tree_index: u32 },
    /// A source method parameter tree has no symbol for this method owner.
    MethodParameterSymbolMissing {
        method: SymbolId,
        parameter_tree_index: u32,
    },
    /// Extension method prefix parameter metadata is absent from the source index.
    ExtensionPrefixClausesMissing { method: SymbolId },
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
    /// Source identities needed to apply a generic owner to a secondary
    /// constructor result are not represented as constructor parameters.
    GenericSecondaryConstructorDeferred {
        constructor: SymbolId,
        owner: SymbolId,
    },
    /// Secondary-constructor type parameters do not yet have source symbols.
    SecondaryConstructorTypeParametersDeferred { constructor: SymbolId },
    /// A deferred completion belongs to a future completion engine.
    DeferredSymbolCompletion { symbol: SymbolId },
    /// A previous fatal semantic failure has already been recorded.
    SymbolAlreadyErrored { symbol: SymbolId },
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
    TreeOutsideArena { source: SourceId, tree_index: u32 },
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
    SourceClassTypeParametersProvenanceMissing { symbol: SymbolId },
    /// The source class, template, or primary constructor has an invalid AST
    /// shape for class type-parameter recovery.
    MalformedSourceClassTypeParameters { class: SymbolId, tree_index: u32 },
    /// A class type-parameter tree has no canonical symbol mapping.
    ClassTypeParameterSymbolMissing { class: SymbolId, tree_index: u32 },
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
    /// A generic source class was used without receiver type arguments.
    RawGenericSourceReceiverUnsupported { class: SymbolId, expected: usize },
    /// External generic argument order is not modeled yet.
    ExternalGenericInstantiationDeferred { class: SymbolId },
    /// A completed member info could not be obtained for adaptation.
    MemberTypeUnavailable { symbol: SymbolId },
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
    StringLiteralTypingDeferred { source: SourceId, tree_index: u32 },
    /// Null constants do not have a canonical source type in this session yet.
    NullLiteralTypingDeferred { source: SourceId, tree_index: u32 },
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
    ThisOwnerCycle { source: SourceId, owner: SymbolId },
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
    ObjectModuleClassUnavailable { object: SymbolId },
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
    /// Applying a polymorphic method requires explicit application or inference.
    PolymorphicMethodApplicationDeferred { source: SourceId, tree_index: u32 },
    /// Only plain method clauses are supported by ordinary application.
    UnsupportedApplicationMethodKind {
        source: SourceId,
        tree_index: u32,
        kind: MethodKind,
    },
    /// `using` application requires contextual argument insertion.
    UsingApplicationDeferred { source: SourceId, tree_index: u32 },
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
    MalformedOverloadCandidate { symbol: SymbolId, callable: TypeId },
    /// The application name bucket mixes methods and non-method terms.
    MixedApplicationCandidateKinds {
        source: SourceId,
        tree_index: u32,
        candidates: Vec<SymbolId>,
    },
    /// A type selection is outside expression typing.
    TypeSelectionInExpression { source: SourceId, tree_index: u32 },
    /// Member discovery failed with a typed lookup error.
    MemberLookup(Box<MemberLookupError>),
    /// The source expression form is outside this issue's supported subset.
    UnsupportedExpression {
        source: SourceId,
        tree_index: u32,
        expression_kind: &'static str,
    },
    /// The semantic type is not yet supported by expression widening.
    ExpressionTypeCannotBeWidened { ty: TypeId },
    /// A term reference designates a symbol category that is not widenable.
    TermReferenceCannotBeWidened { symbol: SymbolId, kind: SymbolKind },
    /// The receiver prefix does not contain the referenced member symbol.
    TermReferencePrefixMismatch { symbol: SymbolId, prefix: TypeId },
    /// A selection qualifier cannot be preserved as a stable reference path.
    UnstableSelectionPrefix {
        source: SourceId,
        tree_index: u32,
        qualifier_type: TypeId,
    },
}

/// Why one overload was excluded from an application candidate set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OverloadRejection {
    WrongArity {
        expected: usize,
        actual: usize,
    },
    ArgumentNonConformance {
        argument_index: usize,
        actual: TypeId,
        expected: TypeId,
    },
    UnsupportedSemantics,
}

impl fmt::Display for TyperError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for TyperError {}

/// The lexical source scope and semantic owner used to type an expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExpressionContext {
    /// Lexical context for term name and import lookup.
    pub lexical: SourceContextId,
    /// Semantic declaration that owns the expression, used for `this`.
    pub owner: SymbolId,
}

/// Completes source declaration signatures against a shared semantic session.
pub struct SourceTyper<'a> {
    arena: &'a AstArena<Untyped>,
    source: SourceId,
    index: &'a SourceSemanticIndex,
    store: &'a mut SemanticStore,
    definitions: Definitions,
    packages: &'a Packages,
    resolver: Box<dyn SymbolResolver + 'a>,
    type_index: SourceTypeIndex,
    typed_arena: AstArena<Typed>,
    typed_index: SourceTypedIndex,
}

#[derive(Clone, Copy)]
struct SourceTreeLocation {
    tree_index: u32,
    position: Option<SourceSpan>,
}

#[derive(Clone, Copy)]
enum ImportSelection {
    Explicit,
    Wildcard,
}

#[derive(Clone, Copy)]
struct ApplicationCandidate {
    symbol: SymbolId,
    callable: TypeId,
    member: Option<MemberCandidate>,
}

#[derive(Clone, Copy)]
struct TypedArgument {
    typed: TreeId<Typed>,
    own_type: TypeId,
    widened_type: TypeId,
}

fn overload_arity_rejection(method: &MethodType, actual: usize) -> Option<usize> {
    match method.params.iter().position(|parameter| parameter.varargs) {
        Some(varargs_index) if varargs_index + 1 == method.params.len() => {
            (actual < varargs_index).then_some(varargs_index)
        }
        Some(_) => None,
        None => (actual != method.params.len()).then_some(method.params.len()),
    }
}

fn supported_override_signature(method: &MethodType, store: &SemanticStore) -> bool {
    method.kind == MethodKind::Plain
        && method.params.iter().all(|parameter| {
            !parameter.erased
                && !parameter.varargs
                && !matches!(store.types.try_get(parameter.ty), Some(Type::ByName { .. }))
        })
}

struct ResolvedApplicationFunction {
    typed: TreeId<Typed>,
    callable: TypeId,
    arguments: Vec<TypedArgument>,
}

#[derive(Clone, Copy)]
enum ApplicationFunctionShape {
    Ident(Ident),
    Select {
        selection: dotty_core::ast::Select<Untyped>,
        qualifier: TreeId<Typed>,
        receiver_type: TypeId,
    },
}

#[derive(Clone, Copy)]
struct SourceImport {
    tree: TreeId<Untyped>,
    context: SourceContextId,
    parent: Option<SourceContextId>,
}

#[derive(Clone)]
enum MethodClauseSpec {
    Types(Vec<TypeParamSpec>),
    Terms(Vec<MethodParamSpec>, MethodKind),
}

/// Resolves source-written type names without making the type-tree dispatcher
/// depend on the details of lexical scopes, imports, or semantic lookups.
struct SourceNameResolver<'typer, 'store> {
    typer: &'typer mut SourceTyper<'store>,
}

impl SourceNameResolver<'_, '_> {
    fn resolve_type_name(
        &mut self,
        name: dotty_core::Name,
        context: SourceContextId,
        location: SourceTreeLocation,
    ) -> Result<TypeId, TyperError> {
        let target =
            self.typer
                .lookup_type_symbol(name, context, location.tree_index, location.position)?;
        if let Some(target) = target {
            let prefix = self.typer.type_symbol_prefix(target);
            return Ok(self.typer.store.types.alloc(Type::TypeRef {
                prefix,
                target: TypeRefTarget::Symbol(target),
            }));
        }
        if let Some(builtin) = self
            .typer
            .definitions
            .source_builtin_type(self.typer.store, name)
        {
            return Ok(builtin);
        }
        if let Some(symbol) = self.typer.lookup_term_candidate_for_type_name(
            name,
            context,
            location.tree_index,
            location.position,
        )? {
            return Err(TyperError::WrongTypeNameKind {
                source: self.typer.source,
                tree_index: location.tree_index,
                name,
                symbol,
                kind: self.typer.store.symbols.get(symbol).kind,
                position: location.position,
            });
        }
        Err(TyperError::TypeNameNotFound {
            source: self.typer.source,
            tree_index: location.tree_index,
            name,
            position: location.position,
        })
    }

    fn resolve_type_member(
        &mut self,
        qualifier_tree: TreeId<Untyped>,
        name: dotty_core::Name,
        context: SourceContextId,
        location: SourceTreeLocation,
    ) -> Result<TypeId, TyperError> {
        self.typer.type_of_selected_tpt(
            qualifier_tree,
            name,
            context,
            location.tree_index,
            location.position,
        )
    }
}

impl<'a> SourceTyper<'a> {
    /// Creates a driver using caller-owned, already-bootstrapped definitions.
    pub fn new(
        arena: &'a AstArena<Untyped>,
        source: SourceId,
        index: &'a SourceSemanticIndex,
        store: &'a mut SemanticStore,
        definitions: Definitions,
        packages: &'a Packages,
    ) -> Self {
        Self {
            arena,
            source,
            index,
            store,
            definitions,
            packages,
            resolver: Box::new(NoResolver),
            type_index: SourceTypeIndex::default(),
            typed_arena: AstArena::new(),
            typed_index: SourceTypedIndex::new(),
        }
    }

    /// Uses an external semantic resolver after source and session lookup.
    ///
    /// The resolver supplies symbols already present in the semantic store;
    /// it does not perform classpath IO through the typer.
    pub fn with_resolver(mut self, resolver: Box<dyn SymbolResolver + 'a>) -> Self {
        self.resolver = resolver;
        self
    }

    /// Types one supported source expression into this driver's typed arena.
    /// Repeated typing of a source tree returns its existing typed identity.
    /// Failed attempts roll back typed nodes and semantic type allocations.
    pub fn type_expression(
        &mut self,
        tree: TreeId<Untyped>,
        context: ExpressionContext,
    ) -> Result<TreeId<Typed>, TyperError> {
        if let Some(typed) = self.typed_index.get(self.source, tree) {
            return Ok(typed);
        }
        let ast_checkpoint = self.typed_arena.checkpoint();
        let store_checkpoint = self.store.checkpoint();
        let type_index_checkpoint = self.type_index.checkpoint();
        let mut info_journal = Vec::new();
        let mut new_mappings = Vec::new();
        let result =
            self.type_expression_inner(tree, context, &mut info_journal, &mut new_mappings);
        match result {
            Ok(typed) => Ok(typed),
            Err(error) => {
                for (symbol, previous) in info_journal.into_iter().rev() {
                    if self.store.symbols.contains(symbol) {
                        self.store.symbols.set_info(symbol, previous);
                    }
                }
                self.typed_arena.rollback_to(ast_checkpoint);
                self.store.rollback_to(store_checkpoint);
                self.type_index.restore(type_index_checkpoint);
                for (source, source_tree) in new_mappings.into_iter().rev() {
                    self.typed_index.remove(source, source_tree);
                }
                Err(error)
            }
        }
    }

    /// Returns the value type represented by a typed expression's own type.
    ///
    /// Literal expressions preserve their exact `Type::Constant` type in the
    /// typed AST; consumers use this operation when they need the canonical
    /// builtin value type. A member reference is adapted to its receiver view.
    /// Adapting a reference may complete source symbols and allocate semantic
    /// types; failures roll those changes back.
    pub fn widen_expression_type(&mut self, ty: TypeId) -> Result<TypeId, TyperError> {
        let store_checkpoint = self.store.checkpoint();
        let type_index_checkpoint = self.type_index.checkpoint();
        let mut info_journal = Vec::new();
        let result = self.widen_expression_type_journaled(ty, &mut info_journal, 0);
        if result.is_err() {
            for (symbol, previous) in info_journal.into_iter().rev() {
                if self.store.symbols.contains(symbol) {
                    self.store.symbols.set_info(symbol, previous);
                }
            }
            self.store.rollback_to(store_checkpoint);
            self.type_index.restore(type_index_checkpoint);
        }
        result
    }

    fn widen_expression_type_journaled(
        &mut self,
        ty: TypeId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        depth: usize,
    ) -> Result<TypeId, TyperError> {
        if depth >= crate::types::MAX_TYPE_NORMALIZATION_DEPTH {
            return Err(TyperError::TypeNormalization(
                crate::types::TypeNormalizeError::TooDeep,
            ));
        }
        let Some(expression_type) = self.store.types.try_get(ty).cloned() else {
            return Err(TyperError::TypeNormalization(
                if self.store.types.contains(ty) {
                    crate::types::TypeNormalizeError::UnfilledType { ty }
                } else {
                    crate::types::TypeNormalizeError::InvalidType { ty }
                },
            ));
        };
        use dotty_core::Constant;
        match expression_type {
            Type::Constant(value) => match value {
                Constant::Unit => Ok(self.definitions.unit),
                Constant::Boolean(_) => Ok(self.definitions.boolean),
                Constant::Byte(_) => Ok(self.definitions.byte),
                Constant::Short(_) => Ok(self.definitions.short),
                Constant::Char(_) => Ok(self.definitions.char),
                Constant::Int(_) => Ok(self.definitions.int),
                Constant::Long(_) => Ok(self.definitions.long),
                Constant::FloatBits(_) => Ok(self.definitions.float),
                Constant::DoubleBits(_) => Ok(self.definitions.double),
                Constant::String(_)
                | Constant::StringUtf16(_)
                | Constant::Null
                | Constant::Class(_) => Err(TyperError::ExpressionTypeCannotBeWidened { ty }),
            },
            Type::TermRef { prefix, target } => {
                let TermRefTarget::Symbol(symbol) = target else {
                    return Err(TyperError::ExpressionTypeCannotBeWidened { ty });
                };
                if !self.store.symbols.contains(symbol) {
                    return Err(TyperError::UnknownSymbol { symbol });
                }
                let kind = self.store.symbols.get(symbol).kind;
                if kind == SymbolKind::Object {
                    let module_class = self.source_module_class_of_object(symbol)?;
                    let module_prefix = if prefix == self.definitions.no_prefix {
                        self.type_symbol_prefix(module_class)
                    } else {
                        prefix
                    };
                    return Ok(self.store.types.alloc(Type::TypeRef {
                        prefix: module_prefix,
                        target: TypeRefTarget::Symbol(module_class),
                    }));
                }
                if !matches!(
                    kind,
                    SymbolKind::Field
                        | SymbolKind::Value
                        | SymbolKind::Variable
                        | SymbolKind::Parameter
                        | SymbolKind::Method
                ) {
                    return Err(TyperError::TermReferenceCannotBeWidened { symbol, kind });
                }
                if prefix == self.definitions.no_prefix {
                    self.completed_expression_symbol_info(symbol, info_journal)
                } else {
                    let receiver =
                        self.widen_expression_type_journaled(prefix, info_journal, depth + 1)?;
                    let receiver = self.this_type_receiver_view(receiver)?;
                    let name = self.store.symbols.get(symbol).name;
                    let candidates = self
                        .lookup_members_journaled(receiver, name, info_journal)
                        .map_err(|error| TyperError::MemberLookup(Box::new(error)))?;
                    let candidate = candidates
                        .into_iter()
                        .find(|candidate| candidate.symbol == symbol)
                        .ok_or(TyperError::TermReferencePrefixMismatch {
                            symbol,
                            prefix: receiver,
                        })?;
                    self.member_type_on_journaled(&candidate, info_journal)
                }
            }
            Type::NoType | Type::NoPrefix | Type::Error(_) => {
                Err(TyperError::ExpressionTypeCannotBeWidened { ty })
            }
            Type::Repeated { .. } => Err(TyperError::ExpressionTypeCannotBeWidened { ty }),
            _ => Ok(ty),
        }
    }

    fn type_expression_inner(
        &mut self,
        tree: TreeId<Untyped>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        if let Some(typed) = self.typed_index.get(self.source, tree) {
            return Ok(typed);
        }
        let Some(source_tree) = self.arena.try_get(tree).cloned() else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: tree.index(),
            });
        };
        let typed = match source_tree.kind {
            TreeKind::Literal(literal) => {
                self.literal_type(&literal.value, tree.index())?;
                let ty = self
                    .store
                    .types
                    .alloc(Type::Constant(literal.value.clone()));
                Ok(TypedAstBuilder::new(&mut self.typed_arena).literal(
                    literal.value,
                    ty,
                    source_tree.position,
                ))
            }
            TreeKind::PhaseSpecific(UntypedNode::Number(number)) => {
                let value = self.type_number_literal(number, tree.index())?;
                let ty = self.store.types.alloc(Type::Constant(value.clone()));
                Ok(TypedAstBuilder::new(&mut self.typed_arena).literal(
                    value,
                    ty,
                    source_tree.position,
                ))
            }
            TreeKind::This(this) => {
                let class = self.enclosing_this_owner(this.qual, context.owner, tree.index())?;
                let ty = self.store.types.alloc(Type::ThisType { class });
                Ok(TypedAstBuilder::new(&mut self.typed_arena).this(
                    this.qual,
                    ty,
                    source_tree.position,
                ))
            }
            TreeKind::Ident(ident) => {
                let symbol = self.resolve_expression_term(
                    ident.name,
                    context.lexical,
                    tree.index(),
                    source_tree.position,
                )?;
                let ty = self.expression_type_of_symbol(
                    symbol,
                    context.owner,
                    tree.index(),
                    info_journal,
                )?;
                Ok(
                    TypedAstBuilder::new(&mut self.typed_arena).ident_with_backquoted(
                        ident.name,
                        ident.backquoted,
                        ty,
                        source_tree.position,
                    ),
                )
            }
            TreeKind::Apply(application) => {
                if application.kind == ApplyKind::Using {
                    return Err(TyperError::UsingApplicationDeferred {
                        source: self.source,
                        tree_index: tree.index(),
                    });
                }
                let resolved_function = self.resolve_overloaded_application_function(
                    application.function,
                    &application.args,
                    context,
                    tree.index(),
                    info_journal,
                    new_mappings,
                )?;
                let (function, callable, typed_arguments) =
                    if let Some(resolved) = resolved_function {
                        (resolved.typed, resolved.callable, Some(resolved.arguments))
                    } else {
                        let function = self.type_expression_inner(
                            application.function,
                            context,
                            info_journal,
                            new_mappings,
                        )?;
                        let function_type = self.typed_arena.get(function).ty;
                        let callable =
                            self.widen_expression_type_journaled(function_type, info_journal, 0)?;
                        (function, callable, None)
                    };
                let method = match self.store.types.try_get(callable) {
                    Some(Type::Method(method)) => method.clone(),
                    Some(Type::Poly(_)) => {
                        return Err(TyperError::PolymorphicMethodApplicationDeferred {
                            source: self.source,
                            tree_index: tree.index(),
                        });
                    }
                    _ => {
                        return Err(TyperError::ApplicationCalleeNotMethod {
                            source: self.source,
                            tree_index: tree.index(),
                            ty: callable,
                        });
                    }
                };
                if method.kind != MethodKind::Plain {
                    return Err(TyperError::UnsupportedApplicationMethodKind {
                        source: self.source,
                        tree_index: tree.index(),
                        kind: method.kind,
                    });
                }
                if application.args.len() != method.params.len() {
                    return Err(TyperError::ApplicationArityMismatch {
                        source: self.source,
                        tree_index: tree.index(),
                        expected: method.params.len(),
                        actual: application.args.len(),
                    });
                }
                for (parameter_index, parameter) in method.params.iter().enumerate() {
                    if parameter.erased {
                        return Err(TyperError::ErasedApplicationParameterDeferred {
                            source: self.source,
                            tree_index: tree.index(),
                            parameter_index,
                        });
                    }
                    if parameter.varargs {
                        return Err(TyperError::VarargsApplicationParameterDeferred {
                            source: self.source,
                            tree_index: tree.index(),
                            parameter_index,
                        });
                    }
                    if matches!(
                        self.store.types.try_get(parameter.ty),
                        Some(Type::ByName { .. })
                    ) {
                        return Err(TyperError::ByNameApplicationParameterDeferred {
                            source: self.source,
                            tree_index: tree.index(),
                            parameter_index,
                        });
                    }
                }
                if self.type_contains_param_ref(method.result, callable)? {
                    return Err(TyperError::DependentMethodApplicationDeferred {
                        source: self.source,
                        tree_index: tree.index(),
                        binder: callable,
                        result: method.result,
                    });
                }
                let mut arguments = Vec::with_capacity(application.args.len());
                for (argument_index, (argument_tree, parameter)) in
                    application.args.iter().zip(&method.params).enumerate()
                {
                    let (argument, actual) = if let Some(typed_arguments) = &typed_arguments {
                        let typed_argument = typed_arguments[argument_index];
                        debug_assert_eq!(
                            self.typed_arena.get(typed_argument.typed).ty,
                            typed_argument.own_type
                        );
                        (typed_argument.typed, typed_argument.widened_type)
                    } else {
                        let argument = self.type_expression_inner(
                            *argument_tree,
                            context,
                            info_journal,
                            new_mappings,
                        )?;
                        let argument_type = self.typed_arena.get(argument).ty;
                        let actual =
                            self.widen_expression_type_journaled(argument_type, info_journal, 0)?;
                        (argument, actual)
                    };
                    match self.conforms(actual, parameter.ty) {
                        Ok(true) => {}
                        Ok(false) => {
                            return Err(TyperError::ApplicationArgumentTypeMismatch {
                                source: self.source,
                                tree_index: tree.index(),
                                argument_index,
                                actual,
                                expected: parameter.ty,
                            });
                        }
                        Err(error) => {
                            return Err(TyperError::ApplicationArgumentConformanceUnsupported {
                                source: self.source,
                                tree_index: tree.index(),
                                argument_index,
                                actual,
                                expected: parameter.ty,
                                error: Box::new(error),
                            });
                        }
                    }
                    arguments.push(argument);
                }
                Ok(TypedAstBuilder::new(&mut self.typed_arena).apply_with_kind(
                    function,
                    arguments,
                    application.kind,
                    method.result,
                    source_tree.position,
                ))
            }
            TreeKind::Select(selection) => {
                if !selection.name.is_term() {
                    return Err(TyperError::TypeSelectionInExpression {
                        source: self.source,
                        tree_index: tree.index(),
                    });
                }
                let qualifier = self.type_expression_inner(
                    selection.qualifier,
                    context,
                    info_journal,
                    new_mappings,
                )?;
                let receiver_type = self.typed_arena.get(qualifier).ty;
                self.require_stable_selection_prefix(receiver_type, tree.index())?;
                let receiver =
                    self.widen_expression_type_journaled(receiver_type, info_journal, 0)?;
                let receiver = self.this_type_receiver_view(receiver)?;
                let candidates = self
                    .lookup_members_journaled(receiver, selection.name, info_journal)
                    .map_err(|error| TyperError::MemberLookup(Box::new(error)))?;
                let candidate = match candidates.as_slice() {
                    [] => {
                        return Err(TyperError::MemberNotFound {
                            source: self.source,
                            tree_index: tree.index(),
                            receiver,
                            name: selection.name,
                        });
                    }
                    [candidate] => candidate,
                    _ => {
                        return Err(TyperError::OverloadedSelectionDeferred {
                            source: self.source,
                            tree_index: tree.index(),
                            name: selection.name,
                        });
                    }
                };
                let ty = self.store.types.alloc(Type::TermRef {
                    prefix: receiver_type,
                    target: TermRefTarget::Symbol(candidate.symbol),
                });
                Ok(TypedAstBuilder::new(&mut self.typed_arena).select(
                    qualifier,
                    selection.name,
                    selection.backquoted,
                    ty,
                    source_tree.position,
                ))
            }
            _ => Err(TyperError::UnsupportedExpression {
                source: self.source,
                tree_index: tree.index(),
                expression_kind: tree_kind_name(&source_tree.kind),
            }),
        }?;
        self.typed_index
            .insert(self.source, tree, typed)
            .map_err(|error| TyperError::ConflictingTypedExpression {
                source: error.source,
                tree_index: error.untyped.index(),
                existing: error.existing.index(),
                attempted: error.attempted.index(),
            })?;
        new_mappings.push((self.source, tree));
        Ok(typed)
    }

    fn type_contains_param_ref(&self, root: TypeId, binder: TypeId) -> Result<bool, TyperError> {
        fn visit(
            typer: &SourceTyper<'_>,
            ty: TypeId,
            binder: TypeId,
            depth: usize,
            seen: &mut std::collections::HashSet<TypeId>,
        ) -> Result<bool, TyperError> {
            if depth >= crate::types::MAX_TYPE_NORMALIZATION_DEPTH {
                return Err(TyperError::TypeNormalization(
                    crate::types::TypeNormalizeError::TooDeep,
                ));
            }
            if !seen.insert(ty) {
                return Ok(false);
            }
            let Some(node) = typer.store.types.try_get(ty) else {
                return Err(TyperError::TypeNormalization(
                    if typer.store.types.contains(ty) {
                        crate::types::TypeNormalizeError::UnfilledType { ty }
                    } else {
                        crate::types::TypeNormalizeError::InvalidType { ty }
                    },
                ));
            };
            if matches!(node, Type::ParamRef { binder: found, .. } if *found == binder) {
                return Ok(true);
            }
            let mut children = Vec::new();
            match node {
                Type::TermRef { prefix, .. } | Type::TypeRef { prefix, .. } => {
                    children.push(*prefix);
                }
                Type::SuperType {
                    this_type,
                    super_type,
                } => children.extend([*this_type, *super_type]),
                Type::Applied { tycon, args } => {
                    children.push(*tycon);
                    children.extend(args.iter().copied());
                }
                Type::Bounds { low, high } => children.extend([*low, *high]),
                Type::AliasingBounds { alias }
                | Type::ByName { result: alias }
                | Type::Flexible { underlying: alias }
                | Type::Recursive { parent: alias }
                | Type::Wildcard { bounds: alias }
                | Type::JavaArray { element: alias }
                | Type::Repeated { element: alias } => children.push(*alias),
                Type::And { left, right } | Type::Or { left, right } => {
                    children.extend([*left, *right]);
                }
                Type::Refined { parent, info, .. } => children.extend([*parent, *info]),
                Type::Method(method) => {
                    children.extend(method.params.iter().map(|parameter| parameter.ty));
                    children.push(method.result);
                }
                Type::Poly(poly) => {
                    children.extend(poly.params.iter().map(|parameter| parameter.bounds));
                    children.push(poly.result);
                }
                Type::TypeLambda(lambda) => {
                    children.extend(lambda.params.iter().map(|parameter| parameter.bounds));
                    children.push(lambda.result);
                }
                Type::MatchCase { pattern, result } => children.extend([*pattern, *result]),
                Type::Annotated { underlying, .. } => children.push(*underlying),
                Type::ParamRef { .. }
                | Type::NoType
                | Type::Error(_)
                | Type::NoPrefix
                | Type::ThisType { .. }
                | Type::Constant(_)
                | Type::RecThis { .. } => {}
                Type::Match(match_type) => {
                    children.extend([match_type.bound, match_type.scrutinee]);
                    children.extend(match_type.cases.iter().copied());
                }
                Type::ClassInfo(info) => {
                    children.push(info.prefix);
                    children.extend(info.parents.iter().copied());
                    children.extend(info.self_type);
                }
            }
            for child in children {
                if visit(typer, child, binder, depth + 1, seen)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }

        visit(self, root, binder, 0, &mut std::collections::HashSet::new())
    }

    fn resolve_overloaded_application_function(
        &mut self,
        function_tree: TreeId<Untyped>,
        argument_trees: &[TreeId<Untyped>],
        context: ExpressionContext,
        application_tree_index: u32,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<Option<ResolvedApplicationFunction>, TyperError> {
        let Some(function_node) = self.arena.try_get(function_tree).cloned() else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: function_tree.index(),
            });
        };
        let (mut candidates, function_shape) = match function_node.kind {
            TreeKind::Ident(ident) => {
                let candidates = self.expression_term_candidates(
                    ident.name,
                    context.lexical,
                    function_tree.index(),
                    function_node.position,
                )?;
                if candidates.len() <= 1 {
                    return Ok(None);
                }
                if candidates
                    .iter()
                    .any(|symbol| self.store.symbols.get(*symbol).kind != SymbolKind::Method)
                {
                    return Err(TyperError::MixedApplicationCandidateKinds {
                        source: self.source,
                        tree_index: application_tree_index,
                        candidates,
                    });
                }
                (
                    candidates
                        .into_iter()
                        .map(|symbol| {
                            let callable =
                                self.completed_expression_symbol_info(symbol, info_journal)?;
                            self.validate_overload_callable(symbol, callable)?;
                            Ok(ApplicationCandidate {
                                symbol,
                                callable,
                                member: None,
                            })
                        })
                        .collect::<Result<Vec<_>, TyperError>>()?,
                    ApplicationFunctionShape::Ident(ident),
                )
            }
            TreeKind::Select(selection) if selection.name.is_term() => {
                let qualifier = self.type_expression_inner(
                    selection.qualifier,
                    context,
                    info_journal,
                    new_mappings,
                )?;
                let receiver_type = self.typed_arena.get(qualifier).ty;
                self.require_stable_selection_prefix(receiver_type, function_tree.index())?;
                let receiver =
                    self.widen_expression_type_journaled(receiver_type, info_journal, 0)?;
                let receiver_view = self.this_type_receiver_view(receiver)?;
                let members = self
                    .lookup_overload_members_journaled(receiver_view, selection.name, info_journal)
                    .map_err(|error| TyperError::MemberLookup(Box::new(error)))?;
                if members.len() <= 1 {
                    return Ok(None);
                }
                if members
                    .iter()
                    .any(|member| self.store.symbols.get(member.symbol).kind != SymbolKind::Method)
                {
                    return Err(TyperError::MixedApplicationCandidateKinds {
                        source: self.source,
                        tree_index: application_tree_index,
                        candidates: members.iter().map(|member| member.symbol).collect(),
                    });
                }
                let candidates = members
                    .into_iter()
                    .map(|member| {
                        let callable = self.member_type_on_journaled(&member, info_journal)?;
                        self.validate_overload_callable(member.symbol, callable)?;
                        Ok(ApplicationCandidate {
                            symbol: member.symbol,
                            callable,
                            member: Some(member),
                        })
                    })
                    .collect::<Result<Vec<_>, TyperError>>()?;
                (
                    candidates,
                    ApplicationFunctionShape::Select {
                        selection,
                        qualifier,
                        receiver_type,
                    },
                )
            }
            _ => return Ok(None),
        };

        let mut arguments = Vec::with_capacity(argument_trees.len());
        for argument_tree in argument_trees {
            let typed =
                self.type_expression_inner(*argument_tree, context, info_journal, new_mappings)?;
            let own_type = self.typed_arena.get(typed).ty;
            let widened_type = self.widen_expression_type_journaled(own_type, info_journal, 0)?;
            arguments.push(TypedArgument {
                typed,
                own_type,
                widened_type,
            });
        }

        let mut completed_relation_types = std::collections::HashSet::new();
        for argument in &arguments {
            self.complete_overload_relation_type(
                argument.widened_type,
                info_journal,
                &mut completed_relation_types,
                0,
            )?;
        }
        for candidate in &candidates {
            if let Some(Type::Method(method)) = self.store.types.try_get(candidate.callable) {
                let parameter_types: Vec<_> = method.params.iter().map(|param| param.ty).collect();
                for parameter_type in parameter_types {
                    self.complete_overload_relation_type(
                        parameter_type,
                        info_journal,
                        &mut completed_relation_types,
                        0,
                    )?;
                }
            }
        }

        self.remove_overridden_overload_candidates(&mut candidates, application_tree_index)?;

        let winner =
            self.choose_overload_candidate(&mut candidates, &arguments, application_tree_index)?;
        let function_type = match function_shape {
            ApplicationFunctionShape::Ident(ident) => {
                let ty = self.expression_type_of_symbol(
                    winner.symbol,
                    context.owner,
                    function_tree.index(),
                    info_journal,
                )?;
                TypedAstBuilder::new(&mut self.typed_arena).ident_with_backquoted(
                    ident.name,
                    ident.backquoted,
                    ty,
                    function_node.position,
                )
            }
            ApplicationFunctionShape::Select {
                selection,
                qualifier,
                receiver_type,
            } => {
                if winner
                    .member
                    .is_some_and(|member| member.symbol != winner.symbol)
                {
                    return Err(TyperError::MalformedOverloadCandidate {
                        symbol: winner.symbol,
                        callable: winner.callable,
                    });
                }
                let ty = self.store.types.alloc(Type::TermRef {
                    prefix: receiver_type,
                    target: TermRefTarget::Symbol(winner.symbol),
                });
                TypedAstBuilder::new(&mut self.typed_arena).select(
                    qualifier,
                    selection.name,
                    selection.backquoted,
                    ty,
                    function_node.position,
                )
            }
        };
        self.typed_index
            .insert(self.source, function_tree, function_type)
            .map_err(|error| TyperError::ConflictingTypedExpression {
                source: error.source,
                tree_index: error.untyped.index(),
                existing: error.existing.index(),
                attempted: error.attempted.index(),
            })?;
        new_mappings.push((self.source, function_tree));
        Ok(Some(ResolvedApplicationFunction {
            typed: function_type,
            callable: winner.callable,
            arguments,
        }))
    }

    fn validate_overload_callable(
        &self,
        symbol: SymbolId,
        callable: TypeId,
    ) -> Result<(), TyperError> {
        match self.store.types.try_get(callable) {
            Some(Type::Method(_) | Type::Poly(_)) => Ok(()),
            _ => Err(TyperError::MalformedOverloadCandidate { symbol, callable }),
        }
    }

    fn complete_overload_relation_type(
        &mut self,
        ty: TypeId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        seen: &mut std::collections::HashSet<TypeId>,
        depth: usize,
    ) -> Result<(), TyperError> {
        if depth >= crate::types::MAX_TYPE_NORMALIZATION_DEPTH {
            return Err(TyperError::TypeNormalization(
                crate::types::TypeNormalizeError::TooDeep,
            ));
        }
        if !seen.insert(ty) {
            return Ok(());
        }
        let Some(node) = self.store.types.try_get(ty).cloned() else {
            return Err(TyperError::TypeNormalization(
                if self.store.types.contains(ty) {
                    crate::types::TypeNormalizeError::UnfilledType { ty }
                } else {
                    crate::types::TypeNormalizeError::InvalidType { ty }
                },
            ));
        };
        let mut children = Vec::new();
        match node {
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                prefix,
            } => {
                if !self.store.symbols.contains(symbol) {
                    return Err(TyperError::UnknownSymbol { symbol });
                }
                children.push(prefix);
                if matches!(
                    self.store.symbols.get(symbol).kind,
                    SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
                ) && self.is_current_source_symbol(symbol)
                    && matches!(*self.store.symbols.info(symbol), SymbolInfo::Missing)
                {
                    self.complete_symbol_inner(symbol, info_journal)?;
                }
                if let SymbolInfo::Complete(info) = *self.store.symbols.info(symbol)
                    && let Some(Type::ClassInfo(class_info)) = self.store.types.try_get(info)
                {
                    children.extend(class_info.parents.iter().copied());
                }
            }
            Type::Applied { tycon, args } => {
                children.push(tycon);
                children.extend(args);
            }
            Type::TermRef { prefix, .. } => children.push(prefix),
            Type::SuperType {
                this_type,
                super_type,
            } => children.extend([this_type, super_type]),
            Type::Bounds { low, high } => children.extend([low, high]),
            Type::AliasingBounds { alias }
            | Type::ByName { result: alias }
            | Type::Flexible { underlying: alias }
            | Type::Recursive { parent: alias }
            | Type::Wildcard { bounds: alias }
            | Type::JavaArray { element: alias }
            | Type::Repeated { element: alias }
            | Type::Annotated {
                underlying: alias, ..
            } => children.push(alias),
            Type::And { left, right } | Type::Or { left, right } => {
                children.extend([left, right]);
            }
            _ => {}
        }
        for child in children {
            self.complete_overload_relation_type(child, info_journal, seen, depth + 1)?;
        }
        Ok(())
    }

    fn remove_overridden_overload_candidates(
        &mut self,
        candidates: &mut Vec<ApplicationCandidate>,
        tree_index: u32,
    ) -> Result<(), TyperError> {
        let mut overridden = vec![false; candidates.len()];
        for derived_index in 0..candidates.len() {
            let Some(derived_member) = candidates[derived_index].member else {
                continue;
            };
            let Some(Type::Method(derived_method)) = self
                .store
                .types
                .try_get(candidates[derived_index].callable)
                .cloned()
            else {
                continue;
            };
            if !supported_override_signature(&derived_method, self.store)
                || self.type_contains_param_ref(
                    derived_method.result,
                    candidates[derived_index].callable,
                )?
            {
                continue;
            }

            for base_index in 0..candidates.len() {
                if base_index == derived_index || overridden[base_index] {
                    continue;
                }
                let Some(base_member) = candidates[base_index].member else {
                    continue;
                };
                if derived_member.declaring_class == base_member.declaring_class {
                    continue;
                }
                let Some(Type::Method(base_method)) = self
                    .store
                    .types
                    .try_get(candidates[base_index].callable)
                    .cloned()
                else {
                    continue;
                };
                if !supported_override_signature(&base_method, self.store)
                    || self.type_contains_param_ref(
                        base_method.result,
                        candidates[base_index].callable,
                    )?
                    || derived_method.params.len() != base_method.params.len()
                {
                    continue;
                }

                let mut same_parameters = true;
                for (derived, base) in derived_method.params.iter().zip(&base_method.params) {
                    let derived_subtype = self.is_subtype(derived.ty, base.ty).map_err(|_| {
                        TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                            source: self.source,
                            tree_index,
                            candidate: candidates[derived_index].symbol,
                        }
                    })?;
                    if !derived_subtype {
                        same_parameters = false;
                        break;
                    }
                    let base_subtype = self.is_subtype(base.ty, derived.ty).map_err(|_| {
                        TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                            source: self.source,
                            tree_index,
                            candidate: candidates[base_index].symbol,
                        }
                    })?;
                    if !base_subtype {
                        same_parameters = false;
                        break;
                    }
                }
                if !same_parameters {
                    continue;
                }

                let derived_view = derived_member.receiver_view;
                let base_view = base_member.receiver_view;
                let derived_is_subtype =
                    self.is_subtype(derived_view, base_view).map_err(|_| {
                        TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                            source: self.source,
                            tree_index,
                            candidate: candidates[derived_index].symbol,
                        }
                    })?;
                if !derived_is_subtype {
                    continue;
                }
                let base_is_subtype = self.is_subtype(base_view, derived_view).map_err(|_| {
                    TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                        source: self.source,
                        tree_index,
                        candidate: candidates[base_index].symbol,
                    }
                })?;
                if !base_is_subtype {
                    overridden[base_index] = true;
                }
            }
        }
        let mut index = 0;
        candidates.retain(|_| {
            let keep = !overridden[index];
            index += 1;
            keep
        });
        Ok(())
    }

    fn choose_overload_candidate(
        &mut self,
        candidates: &mut [ApplicationCandidate],
        arguments: &[TypedArgument],
        tree_index: u32,
    ) -> Result<ApplicationCandidate, TyperError> {
        let mut applicable = Vec::new();
        let mut rejected = Vec::new();
        for candidate in candidates.iter().copied() {
            let method = match self.store.types.try_get(candidate.callable) {
                Some(Type::Method(method)) => method.clone(),
                Some(Type::Poly(poly)) => {
                    let rejection = match self.store.types.try_get(poly.result) {
                        Some(Type::Method(method))
                            if let Some(expected) =
                                overload_arity_rejection(method, arguments.len()) =>
                        {
                            OverloadRejection::WrongArity {
                                expected,
                                actual: arguments.len(),
                            }
                        }
                        _ => {
                            return Err(
                                TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                                    source: self.source,
                                    tree_index,
                                    candidate: candidate.symbol,
                                },
                            );
                        }
                    };
                    rejected.push((candidate.symbol, rejection));
                    continue;
                }
                _ => {
                    return Err(TyperError::MalformedOverloadCandidate {
                        symbol: candidate.symbol,
                        callable: candidate.callable,
                    });
                }
            };
            if let Some(expected) = overload_arity_rejection(&method, arguments.len()) {
                rejected.push((
                    candidate.symbol,
                    OverloadRejection::WrongArity {
                        expected,
                        actual: arguments.len(),
                    },
                ));
                continue;
            }
            let mut argument_rejection = None;
            let mut unsupported_relation = false;
            for (argument_index, (argument, parameter)) in
                arguments.iter().zip(&method.params).enumerate()
            {
                match self.conforms(argument.widened_type, parameter.ty) {
                    Ok(true) => {}
                    Ok(false) => {
                        argument_rejection = Some(OverloadRejection::ArgumentNonConformance {
                            argument_index,
                            actual: argument.widened_type,
                            expected: parameter.ty,
                        });
                        break;
                    }
                    Err(_) => {
                        unsupported_relation = true;
                        break;
                    }
                }
            }
            if let Some(rejection) = argument_rejection {
                rejected.push((candidate.symbol, rejection));
                continue;
            }
            if unsupported_relation {
                return Err(TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                    source: self.source,
                    tree_index,
                    candidate: candidate.symbol,
                });
            }
            let unsupported = method.kind != MethodKind::Plain
                || method.params.iter().any(|param| {
                    param.erased
                        || param.varargs
                        || matches!(
                            self.store.types.try_get(param.ty),
                            Some(Type::ByName { .. })
                        )
                })
                || self.type_contains_param_ref(method.result, candidate.callable)?;
            if unsupported {
                return Err(TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                    source: self.source,
                    tree_index,
                    candidate: candidate.symbol,
                });
            }
            applicable.push(candidate);
        }
        match applicable.as_slice() {
            [candidate] => Ok(*candidate),
            [] => Err(TyperError::OverloadApplicationNoApplicable {
                source: self.source,
                tree_index,
                candidates: rejected,
            }),
            many => {
                let mut most_specific = Vec::new();
                for candidate in many {
                    let Some(Type::Method(method)) = self.store.types.try_get(candidate.callable)
                    else {
                        return Err(TyperError::MalformedOverloadCandidate {
                            symbol: candidate.symbol,
                            callable: candidate.callable,
                        });
                    };
                    let candidate_params: Vec<_> =
                        method.params.iter().map(|parameter| parameter.ty).collect();
                    let mut dominates_all = true;
                    for other in many {
                        if candidate.symbol == other.symbol {
                            continue;
                        }
                        let Some(Type::Method(other_method)) =
                            self.store.types.try_get(other.callable)
                        else {
                            return Err(TyperError::MalformedOverloadCandidate {
                                symbol: other.symbol,
                                callable: other.callable,
                            });
                        };
                        let other_params: Vec<_> = other_method
                            .params
                            .iter()
                            .map(|parameter| parameter.ty)
                            .collect();
                        let mut strictly_more_specific = false;
                        for (candidate_parameter, other_parameter) in
                            candidate_params.iter().zip(&other_params)
                        {
                            let candidate_is_subtype = self
                                .is_subtype(*candidate_parameter, *other_parameter)
                                .map_err(|_| {
                                    TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                                        source: self.source,
                                        tree_index,
                                        candidate: candidate.symbol,
                                    }
                                })?;
                            if !candidate_is_subtype {
                                dominates_all = false;
                                break;
                            }
                            let other_is_subtype = self
                                .is_subtype(*other_parameter, *candidate_parameter)
                                .map_err(|_| {
                                    TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                                        source: self.source,
                                        tree_index,
                                        candidate: other.symbol,
                                    }
                                })?;
                            strictly_more_specific |= !other_is_subtype;
                        }
                        if !dominates_all || !strictly_more_specific {
                            dominates_all = false;
                            break;
                        }
                    }
                    if dominates_all {
                        most_specific.push(*candidate);
                    }
                }
                match most_specific.as_slice() {
                    [candidate] => Ok(*candidate),
                    _ => Err(TyperError::AmbiguousOverloadApplication {
                        source: self.source,
                        tree_index,
                        candidates: many.iter().map(|candidate| candidate.symbol).collect(),
                    }),
                }
            }
        }
    }

    fn enclosing_this_owner(
        &self,
        qualifier: Option<dotty_core::Name>,
        expression_owner: SymbolId,
        tree_index: u32,
    ) -> Result<SymbolId, TyperError> {
        use std::collections::HashSet;
        let mut current = Some(expression_owner);
        let mut seen = HashSet::new();
        while let Some(symbol) = current {
            if !self.store.symbols.contains(symbol) {
                return Err(TyperError::UnknownSymbol { symbol });
            }
            if !seen.insert(symbol) {
                return Err(TyperError::ThisOwnerCycle {
                    source: self.source,
                    owner: symbol,
                });
            }
            let declaration = self.store.symbols.get(symbol);
            if matches!(
                declaration.kind,
                SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
            ) && qualifier.is_none_or(|name| declaration.name == name)
            {
                return Ok(symbol);
            }
            current = declaration.owner;
        }
        Err(TyperError::ThisOwnerNotEnclosing {
            source: self.source,
            tree_index,
            qualifier,
            owner: expression_owner,
        })
    }

    fn require_stable_selection_prefix(
        &self,
        qualifier_type: TypeId,
        tree_index: u32,
    ) -> Result<(), TyperError> {
        let stable = match self.store.types.try_get(qualifier_type) {
            Some(Type::ThisType { .. } | Type::Constant(_)) => true,
            Some(Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            }) if self.store.symbols.contains(*symbol) => {
                let declaration = self.store.symbols.get(*symbol);
                let by_name = matches!(
                    declaration.info,
                    SymbolInfo::Complete(info)
                        if matches!(self.store.types.try_get(info), Some(Type::ByName { .. }))
                );
                matches!(
                    declaration.kind,
                    SymbolKind::Parameter
                        | SymbolKind::Field
                        | SymbolKind::Value
                        | SymbolKind::Object
                ) && !declaration.flags.contains(SymbolFlags::MUTABLE)
                    && !by_name
                    && (declaration.kind != SymbolKind::Object
                        || self.source_module_class_of_object(*symbol).is_ok())
            }
            _ => false,
        };
        if stable {
            Ok(())
        } else {
            Err(TyperError::UnstableSelectionPrefix {
                source: self.source,
                tree_index,
                qualifier_type,
            })
        }
    }

    fn resolve_expression_term(
        &mut self,
        name: dotty_core::Name,
        context: SourceContextId,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<SymbolId, TyperError> {
        let candidates = self.expression_term_candidates(name, context, tree_index, position)?;
        self.unique_expression_term(&candidates, name, tree_index, position)
    }

    /// Resolves the highest-precedence lexical/import bucket without choosing
    /// among method overloads. Ordinary expression references continue to use
    /// `resolve_expression_term` and defer when that bucket has several
    /// methods; Apply uses the full bucket for applicability filtering.
    fn expression_term_candidates(
        &mut self,
        name: dotty_core::Name,
        context: SourceContextId,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<Vec<SymbolId>, TyperError> {
        let contexts = self.source_context_chain(context, tree_index)?;
        for context_id in &contexts {
            let source_context = self.index.source_context(*context_id);
            let candidates = self
                .store
                .scopes
                .get(source_context.lexical_scope)
                .lookup_all(&name)
                .to_vec();
            if !candidates.is_empty() {
                return Ok(candidates);
            }
            for selection in [ImportSelection::Explicit, ImportSelection::Wildcard] {
                let candidates = self.lookup_import_candidates(
                    &[*context_id],
                    name,
                    false,
                    selection,
                    SourceTreeLocation {
                        tree_index,
                        position,
                    },
                )?;
                if !candidates.is_empty() {
                    return Ok(candidates);
                }
            }
        }
        Err(TyperError::TermNameNotFound {
            source: self.source,
            tree_index,
            name,
            position,
        })
    }

    fn unique_expression_term(
        &self,
        candidates: &[SymbolId],
        name: dotty_core::Name,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<SymbolId, TyperError> {
        match candidates {
            [symbol] => Ok(*symbol),
            many if many
                .iter()
                .all(|symbol| self.store.symbols.get(*symbol).kind == SymbolKind::Method) =>
            {
                Err(TyperError::OverloadedReferenceDeferred {
                    source: self.source,
                    tree_index,
                    name,
                })
            }
            _ => Err(TyperError::AmbiguousTermReference {
                source: self.source,
                tree_index,
                name,
                position,
            }),
        }
    }

    fn expression_type_of_symbol(
        &mut self,
        symbol: SymbolId,
        expression_owner: SymbolId,
        tree_index: u32,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        if !self.store.symbols.contains(symbol) {
            return Err(TyperError::UnknownSymbol { symbol });
        }
        let kind = self.store.symbols.get(symbol).kind;
        match kind {
            SymbolKind::Parameter
            | SymbolKind::Field
            | SymbolKind::Value
            | SymbolKind::Variable
            | SymbolKind::Method => {}
            SymbolKind::Object => {
                if self.source_module_class_of_object(symbol).is_err() {
                    return Err(TyperError::ObjectTermReferenceDeferred {
                        source: self.source,
                        tree_index,
                        symbol,
                    });
                }
            }
            _ => {
                return Err(TyperError::UnsupportedTermReference {
                    source: self.source,
                    tree_index,
                    symbol,
                    kind,
                });
            }
        }
        if kind != SymbolKind::Object {
            let info = self.completed_expression_symbol_info(symbol, info_journal)?;
            if matches!(self.store.types.try_get(info), Some(Type::Repeated { .. })) {
                return Err(TyperError::VarargsParameterReferenceDeferred {
                    source: self.source,
                    tree_index,
                    symbol,
                });
            }
        }
        let prefix =
            self.expression_term_prefix(symbol, expression_owner, tree_index, info_journal)?;
        Ok(self.store.types.alloc(Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(symbol),
        }))
    }

    fn completed_expression_symbol_info(
        &mut self,
        symbol: SymbolId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        match *self.store.symbols.info(symbol) {
            SymbolInfo::Complete(ty) => Ok(ty),
            SymbolInfo::Missing => self.complete_symbol_inner(symbol, info_journal),
            SymbolInfo::Deferred(_) => Err(TyperError::DeferredSymbolCompletion { symbol }),
            SymbolInfo::Error => Err(TyperError::SymbolAlreadyErrored { symbol }),
        }
    }

    fn expression_term_prefix(
        &mut self,
        symbol: SymbolId,
        expression_owner: SymbolId,
        tree_index: u32,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let owner = self.store.symbols.get(symbol).owner;
        if !owner.is_some_and(|owner| {
            matches!(
                self.store.symbols.get(owner).kind,
                SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
            )
        }) {
            return Ok(self.definitions.no_prefix);
        }
        let current_class = self.enclosing_this_owner(None, expression_owner, tree_index)?;
        let prefix = self.store.types.alloc(Type::ThisType {
            class: current_class,
        });
        if owner == Some(current_class) {
            return Ok(prefix);
        }
        let mut enclosing = self.store.symbols.get(current_class).owner;
        let mut seen = std::collections::HashSet::new();
        while let Some(class) = enclosing {
            if !self.store.symbols.contains(class) {
                return Err(TyperError::UnknownSymbol { symbol: class });
            }
            if !seen.insert(class) {
                return Err(TyperError::ThisOwnerCycle {
                    source: self.source,
                    owner: class,
                });
            }
            let declaration = self.store.symbols.get(class);
            enclosing = declaration.owner;
            if Some(class) == owner
                && matches!(
                    declaration.kind,
                    SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
                )
            {
                return Ok(self.store.types.alloc(Type::ThisType { class }));
            }
        }
        let name = self.store.symbols.get(symbol).name;
        match self.lookup_members_journaled(prefix, name, info_journal) {
            Ok(candidates)
                if candidates
                    .iter()
                    .any(|candidate| candidate.symbol == symbol) =>
            {
                Ok(prefix)
            }
            Ok(_) => Ok(self.definitions.no_prefix),
            Err(MemberLookupError::ClassInfoUnavailable { symbol, .. })
                if !self.is_current_source_symbol(symbol) =>
            {
                // Member lookup can reach an uncompleted external `Object`
                // parent after proving no current-source path to this symbol.
                Ok(self.definitions.no_prefix)
            }
            Err(error) => Err(TyperError::MemberLookup(Box::new(error))),
        }
    }

    fn source_module_class_of_object(&self, object: SymbolId) -> Result<SymbolId, TyperError> {
        if !self.store.symbols.contains(object) {
            return Err(TyperError::UnknownSymbol { symbol: object });
        }
        let declaration = self.store.symbols.get(object);
        if declaration.kind != SymbolKind::Object {
            return Err(TyperError::ObjectModuleClassUnavailable { object });
        }
        let Some(owner) = declaration.owner else {
            return Err(TyperError::ObjectModuleClassUnavailable { object });
        };
        let Some(SourceDefinition::Canonical { source, tree }) = self.index.definition_of(object)
        else {
            return Err(TyperError::ObjectModuleClassUnavailable { object });
        };
        if source != self.source {
            return Err(TyperError::ObjectModuleClassUnavailable { object });
        }
        let Some(module_class) = self.index.derived_symbol_at(owner, source, tree) else {
            return Err(TyperError::ObjectModuleClassUnavailable { object });
        };
        if !self.store.symbols.contains(module_class)
            || self.store.symbols.get(module_class).kind != SymbolKind::ModuleClass
            || self.index.definition_of(module_class)
                != Some(SourceDefinition::Derived { source, tree })
        {
            return Err(TyperError::ObjectModuleClassUnavailable { object });
        }
        Ok(module_class)
    }

    fn literal_type(
        &self,
        value: &dotty_core::Constant,
        tree_index: u32,
    ) -> Result<TypeId, TyperError> {
        use dotty_core::Constant;
        match value {
            Constant::Unit => Ok(self.definitions.unit),
            Constant::Boolean(_) => Ok(self.definitions.boolean),
            Constant::Byte(_) => Ok(self.definitions.byte),
            Constant::Short(_) => Ok(self.definitions.short),
            Constant::Char(_) => Ok(self.definitions.char),
            Constant::Int(_) => Ok(self.definitions.int),
            Constant::Long(_) => Ok(self.definitions.long),
            Constant::FloatBits(_) => Ok(self.definitions.float),
            Constant::DoubleBits(_) => Ok(self.definitions.double),
            Constant::String(_) | Constant::StringUtf16(_) => {
                Err(TyperError::StringLiteralTypingDeferred {
                    source: self.source,
                    tree_index,
                })
            }
            Constant::Null => Err(TyperError::NullLiteralTypingDeferred {
                source: self.source,
                tree_index,
            }),
            Constant::Class(_) => Err(TyperError::UnsupportedExpression {
                source: self.source,
                tree_index,
                expression_kind: "class constant",
            }),
        }
    }

    fn type_number_literal(
        &self,
        number: NumberLiteral,
        tree_index: u32,
    ) -> Result<dotty_core::Constant, TyperError> {
        use dotty_core::Constant;
        let spelling = self.store.names.resolve(number.text).to_owned();
        let digits = spelling.replace('_', "");
        match number.kind {
            NumberKind::Whole(radix) => {
                let unsigned = digits
                    .strip_prefix("0x")
                    .or_else(|| digits.strip_prefix("0X"))
                    .or_else(|| digits.strip_prefix("0b"))
                    .or_else(|| digits.strip_prefix("0B"));
                let parsed = if radix == 10 {
                    digits.parse::<i32>().ok()
                } else if (2..=36).contains(&radix) {
                    unsigned
                        .and_then(|digits| u32::from_str_radix(digits, radix).ok())
                        .map(|bits| bits as i32)
                } else {
                    None
                };
                parsed
                    .map(Constant::Int)
                    .ok_or(TyperError::IntegerLiteralOutOfRange {
                        source: self.source,
                        tree_index,
                        spelling,
                    })
            }
            NumberKind::Decimal | NumberKind::Floating => digits
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .map(Constant::double)
                .ok_or(TyperError::FloatingLiteralInvalid {
                    source: self.source,
                    tree_index,
                    spelling,
                }),
        }
    }

    /// Returns this driver's source type cache.
    pub fn source_type_index(&self) -> &SourceTypeIndex {
        &self.type_index
    }

    /// Completes a source declaration, rolling back this call's mutations on failure.
    ///
    /// Source classes, traits, and module classes publish [`Type::ClassInfo`]
    /// using the declaration scope allocated by the namer. Class and module
    /// classes receive the canonical `java.lang.Object` parent when no real
    /// class parent is present. Scala 3.9 TASTy also records `Object` as the
    /// parent of a trait with no explicit parent.
    pub fn complete_symbol(&mut self, symbol: SymbolId) -> Result<TypeId, TyperError> {
        self.run_atomic(|typer, info_journal| {
            if !typer.store.symbols.contains(symbol) {
                return Err(TyperError::UnknownSymbol { symbol });
            }
            match *typer.store.symbols.info(symbol) {
                SymbolInfo::Complete(ty) => {
                    if matches!(
                        typer.store.symbols.get(symbol).kind,
                        SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
                    ) {
                        typer.validate_existing_class_info(symbol, ty)?;
                    }
                    return Ok(ty);
                }
                SymbolInfo::Missing => {}
                SymbolInfo::Deferred(_) => {
                    return Err(TyperError::DeferredSymbolCompletion { symbol });
                }
                SymbolInfo::Error => return Err(TyperError::SymbolAlreadyErrored { symbol }),
            }
            typer.complete_symbol_inner(symbol, info_journal)
        })
    }

    fn run_atomic<T>(
        &mut self,
        operation: impl FnOnce(&mut Self, &mut Vec<(SymbolId, SymbolInfo)>) -> Result<T, TyperError>,
    ) -> Result<T, TyperError> {
        let checkpoint = self.store.checkpoint();
        let cache_checkpoint = self.type_index.checkpoint();
        let mut info_journal = Vec::new();
        let result = operation(self, &mut info_journal);
        if result.is_err() {
            for (changed, previous) in info_journal.into_iter().rev() {
                if self.store.symbols.contains(changed) {
                    self.store.symbols.set_info(changed, previous);
                }
            }
            self.store.rollback_to(checkpoint);
            self.type_index.restore(cache_checkpoint);
        }
        result
    }

    fn complete_symbol_inner(
        &mut self,
        symbol: SymbolId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let kind = self.store.symbols.get(symbol).kind;
        let definition = self
            .index
            .definition_of(symbol)
            .ok_or(TyperError::SourceProvenanceMissing { symbol })?;
        let (source, tree) = match definition {
            SourceDefinition::Canonical { source, tree }
            | SourceDefinition::Derived { source, tree } => (source, tree),
        };
        if source != self.source {
            return Err(TyperError::SourceProvenanceMissing { symbol });
        }
        if self.index.declaration_context_of(symbol).is_none() {
            return Err(TyperError::DeclarationContextMissing { symbol });
        }

        let Some(source_tree) = self.arena.try_get(tree) else {
            return Err(TyperError::TreeOutsideArena {
                source,
                tree_index: tree.index(),
            });
        };
        let type_tree = match (kind, &source_tree.kind) {
            (
                SymbolKind::Field
                | SymbolKind::Value
                | SymbolKind::Variable
                | SymbolKind::Parameter,
                TreeKind::ValDef(definition),
            ) => Some(definition.tpt),
            (SymbolKind::Method | SymbolKind::Constructor, TreeKind::DefDef(_)) => None,
            (
                SymbolKind::TypeParameter
                | SymbolKind::TypeAlias
                | SymbolKind::Class
                | SymbolKind::Trait,
                TreeKind::TypeDef(_),
            ) => None,
            // Namer records both object term and derived module class against
            // the original parser-only `ModuleDef`; keep their semantic
            // dispatch distinct even though their source tree is shared.
            (
                SymbolKind::Object | SymbolKind::ModuleClass,
                TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::ModuleDef(_)),
            ) => None,
            _ => {
                return Err(TyperError::SymbolSourceKindMismatch {
                    source,
                    tree_index: tree.index(),
                    symbol,
                    kind,
                });
            }
        };
        let type_definition_rhs = match &source_tree.kind {
            TreeKind::TypeDef(definition) => Some(definition.rhs),
            _ => None,
        };
        let method_definition = match &source_tree.kind {
            TreeKind::DefDef(definition) => Some(definition.clone()),
            _ => None,
        };

        match kind {
            SymbolKind::Field
            | SymbolKind::Value
            | SymbolKind::Variable
            | SymbolKind::Parameter => {
                let Some(tpt) = type_tree else {
                    return Err(TyperError::MalformedSourceAst {
                        source,
                        tree_index: tree.index(),
                        symbol,
                        kind,
                    });
                };
                let (tpt, repeated_parameter) = if kind == SymbolKind::Parameter {
                    match self.arena.try_get(tpt).map(|node| &node.kind) {
                        Some(TreeKind::PhaseSpecific(UntypedNode::PostfixOp(postfix)))
                            if self.store.names.resolve(postfix.op.text()) == "*" =>
                        {
                            (postfix.operand, true)
                        }
                        _ => (tpt, false),
                    }
                } else {
                    (tpt, false)
                };
                let context = self
                    .index
                    .declaration_context_of(symbol)
                    .ok_or(TyperError::DeclarationContextMissing { symbol })?;
                let element_type = self.type_of_tpt(tpt, context)?;
                let ty = if repeated_parameter {
                    self.store.types.alloc(Type::Repeated {
                        element: element_type,
                    })
                } else {
                    element_type
                };
                let previous = *self.store.symbols.info(symbol);
                info_journal.push((symbol, previous));
                self.store
                    .symbols
                    .set_info(symbol, SymbolInfo::Complete(ty));
                Ok(ty)
            }
            SymbolKind::TypeParameter => {
                let rhs = type_definition_rhs.ok_or(TyperError::SymbolSourceKindMismatch {
                    source,
                    tree_index: tree.index(),
                    symbol,
                    kind,
                })?;
                let context = self
                    .index
                    .declaration_context_of(symbol)
                    .ok_or(TyperError::DeclarationContextMissing { symbol })?;
                self.complete_type_parameter(symbol, rhs, context, info_journal)
            }
            SymbolKind::TypeAlias => {
                let rhs = type_definition_rhs.ok_or(TyperError::SymbolSourceKindMismatch {
                    source,
                    tree_index: tree.index(),
                    symbol,
                    kind,
                })?;
                let context = self
                    .index
                    .declaration_context_of(symbol)
                    .ok_or(TyperError::DeclarationContextMissing { symbol })?;
                self.complete_type_alias(symbol, rhs, tree.index(), context, info_journal)
            }
            SymbolKind::Method => {
                let definition = method_definition.ok_or(TyperError::SymbolSourceKindMismatch {
                    source,
                    tree_index: tree.index(),
                    symbol,
                    kind,
                })?;
                let info = self.complete_method_signature(
                    symbol,
                    tree.index(),
                    &definition,
                    info_journal,
                )?;
                let previous = *self.store.symbols.info(symbol);
                info_journal.push((symbol, previous));
                self.store
                    .symbols
                    .set_info(symbol, SymbolInfo::Complete(info));
                Ok(info)
            }
            SymbolKind::Constructor => {
                let definition = method_definition.ok_or(TyperError::SymbolSourceKindMismatch {
                    source,
                    tree_index: tree.index(),
                    symbol,
                    kind,
                })?;
                let info =
                    self.complete_constructor_signature(symbol, tree, &definition, info_journal)?;
                let previous = *self.store.symbols.info(symbol);
                info_journal.push((symbol, previous));
                self.store
                    .symbols
                    .set_info(symbol, SymbolInfo::Complete(info));
                Ok(info)
            }
            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass => {
                self.complete_class_info(symbol, kind, tree, info_journal)
            }
            SymbolKind::Object => Err(TyperError::UnsupportedSymbolCompletion { symbol, kind }),
            SymbolKind::Package | SymbolKind::Local => {
                Err(TyperError::UnsupportedSymbolCompletion { symbol, kind })
            }
        }
    }

    fn validate_existing_class_info(&self, symbol: SymbolId, ty: TypeId) -> Result<(), TyperError> {
        let expected_scope = if self.index.definition_of(symbol).is_some() {
            Some(
                self.index
                    .scope_of(symbol)
                    .ok_or(TyperError::MissingClassScope { symbol })?,
            )
        } else {
            None
        };
        match self.store.types.get(ty) {
            Type::ClassInfo(info)
                if info.class == symbol
                    && info.prefix == self.definitions.no_prefix
                    && expected_scope.is_none_or(|scope| info.declarations == scope) =>
            {
                Ok(())
            }
            _ => Err(TyperError::MalformedClassInfo { symbol, info: ty }),
        }
    }

    fn complete_class_info(
        &mut self,
        symbol: SymbolId,
        kind: SymbolKind,
        source_tree: TreeId<Untyped>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let source_node = self
            .arena
            .try_get(source_tree)
            .ok_or(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: source_tree.index(),
            })?;
        let template = match (kind, &source_node.kind) {
            (SymbolKind::Class | SymbolKind::Trait, TreeKind::TypeDef(definition)) => {
                let Some(node) = self.arena.try_get(definition.rhs) else {
                    return Err(TyperError::TreeOutsideArena {
                        source: self.source,
                        tree_index: definition.rhs.index(),
                    });
                };
                let TreeKind::Template(template) = &node.kind else {
                    return Err(TyperError::MalformedSourceAst {
                        source: self.source,
                        tree_index: definition.rhs.index(),
                        symbol,
                        kind,
                    });
                };
                template.clone()
            }
            (SymbolKind::ModuleClass, TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module))) => {
                let Some(node) = self.arena.try_get(module.template) else {
                    return Err(TyperError::TreeOutsideArena {
                        source: self.source,
                        tree_index: module.template.index(),
                    });
                };
                let TreeKind::Template(template) = &node.kind else {
                    return Err(TyperError::MalformedSourceAst {
                        source: self.source,
                        tree_index: module.template.index(),
                        symbol,
                        kind,
                    });
                };
                template.clone()
            }
            _ => {
                return Err(TyperError::SymbolSourceKindMismatch {
                    source: self.source,
                    tree_index: source_tree.index(),
                    symbol,
                    kind,
                });
            }
        };

        let class_context = self
            .index
            .declaration_context_of(symbol)
            .ok_or(TyperError::DeclarationContextMissing { symbol })?;
        let constructor_node =
            self.arena
                .try_get(template.constructor)
                .ok_or(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: template.constructor.index(),
                })?;
        let TreeKind::DefDef(constructor) = &constructor_node.kind else {
            return Err(TyperError::MalformedSourceAst {
                source: self.source,
                tree_index: template.constructor.index(),
                symbol,
                kind,
            });
        };

        let mut type_context = class_context;
        for parameter_tree in &constructor.type_params {
            let Some(parameter_node) = self.arena.try_get(*parameter_tree) else {
                return Err(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: parameter_tree.index(),
                });
            };
            if !matches!(parameter_node.kind, TreeKind::TypeDef(_)) {
                return Err(TyperError::MalformedSourceAst {
                    source: self.source,
                    tree_index: parameter_tree.index(),
                    symbol,
                    kind,
                });
            }
            let parameter = self.index.symbol_at(self.source, *parameter_tree).ok_or(
                TyperError::MethodParameterSymbolMissing {
                    method: symbol,
                    parameter_tree_index: parameter_tree.index(),
                },
            )?;
            if self.store.symbols.get(parameter).kind != SymbolKind::TypeParameter {
                return Err(TyperError::MalformedSourceAst {
                    source: self.source,
                    tree_index: parameter_tree.index(),
                    symbol,
                    kind,
                });
            }
            if self.store.symbols.get(parameter).owner != Some(symbol) {
                return Err(TyperError::MalformedSourceAst {
                    source: self.source,
                    tree_index: parameter_tree.index(),
                    symbol,
                    kind,
                });
            }
            if let Some(context) = self.index.declaration_context_of(parameter) {
                type_context = context;
            }
            self.complete_signature_parameter(parameter, info_journal)?;
        }

        let mut parents = Vec::with_capacity(template.parents.len().max(1));
        for parent in &template.parents {
            parents.push(self.project_parent_type(*parent, type_context, 0)?);
        }
        let first_parent_is_trait = match (parents.first(), template.parents.first()) {
            (Some(parent_type), Some(parent_tree)) => {
                self.parent_is_trait(*parent_type, parent_tree.index(), info_journal)?
            }
            _ => false,
        };
        if parents.is_empty() || first_parent_is_trait {
            parents.insert(0, self.definitions.object_type);
        }

        let self_type = match template.self_val {
            Some(self_tree) => {
                let Some(node) = self.arena.try_get(self_tree) else {
                    return Err(TyperError::TreeOutsideArena {
                        source: self.source,
                        tree_index: self_tree.index(),
                    });
                };
                let TreeKind::ValDef(self_definition) = &node.kind else {
                    return Err(TyperError::MalformedSourceAst {
                        source: self.source,
                        tree_index: self_tree.index(),
                        symbol,
                        kind,
                    });
                };
                let Some(type_tree) = self.arena.try_get(self_definition.tpt) else {
                    return Err(TyperError::TreeOutsideArena {
                        source: self.source,
                        tree_index: self_definition.tpt.index(),
                    });
                };
                if matches!(type_tree.kind, TreeKind::TypeTree(_)) {
                    // `self =>` has a source binder but imposes no additional
                    // self-type constraint. The parser represents its absent
                    // type with a synthetic TypeTree; ClassInfo encodes this
                    // default as `None`.
                    None
                } else {
                    Some(self.type_of_tpt_inner(self_definition.tpt, type_context)?)
                }
            }
            None => None,
        };

        let declarations = self
            .index
            .scope_of(symbol)
            .ok_or(TyperError::MissingClassScope { symbol })?;
        let info = self.store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: self.definitions.no_prefix,
            class: symbol,
            parents,
            declarations,
            self_type,
        }));
        let previous = *self.store.symbols.info(symbol);
        info_journal.push((symbol, previous));
        self.store
            .symbols
            .set_info(symbol, SymbolInfo::Complete(info));
        Ok(info)
    }

    fn project_parent_type(
        &mut self,
        tree: TreeId<Untyped>,
        context: SourceContextId,
        depth: usize,
    ) -> Result<TypeId, TyperError> {
        if depth > 256 {
            return Err(TyperError::MalformedClassParent {
                source: self.source,
                tree_index: tree.index(),
            });
        }
        let Some(node) = self.arena.try_get(tree) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: tree.index(),
            });
        };
        match &node.kind {
            TreeKind::Apply(application) => {
                self.project_parent_type(application.function, context, depth + 1)
            }
            TreeKind::Block(block) => self.project_parent_type(block.expr, context, depth + 1),
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => {
                self.project_parent_type(parens.inner, context, depth + 1)
            }
            TreeKind::New(new) => self.type_of_tpt_inner(new.tpt, context),
            TreeKind::TypeApply(application) => {
                let repeated_arguments =
                    self.parent_constructor_has_applied_tpt(application.function, depth + 1);
                let tycon = self.project_parent_type(application.function, context, depth + 1)?;
                if repeated_arguments {
                    return Ok(tycon);
                }
                let mut args = Vec::with_capacity(application.args.len());
                for argument in &application.args {
                    args.push(self.type_of_tpt_inner(*argument, context)?);
                }
                Ok(self.store.types.alloc(Type::Applied { tycon, args }))
            }
            TreeKind::Select(selection)
                if self.store.names.resolve(selection.name.text()) == "<init>" =>
            {
                let Some(qualifier) = self.arena.try_get(selection.qualifier) else {
                    return Err(TyperError::TreeOutsideArena {
                        source: self.source,
                        tree_index: selection.qualifier.index(),
                    });
                };
                let TreeKind::New(new) = &qualifier.kind else {
                    return Err(TyperError::MalformedClassParent {
                        source: self.source,
                        tree_index: tree.index(),
                    });
                };
                self.type_of_tpt_inner(new.tpt, context)
            }
            _ => self.type_of_tpt_inner(tree, context),
        }
    }

    fn parent_constructor_has_applied_tpt(&self, tree: TreeId<Untyped>, depth: usize) -> bool {
        if depth > 256 {
            return false;
        }
        let Some(node) = self.arena.try_get(tree) else {
            return false;
        };
        match &node.kind {
            TreeKind::Apply(application) => {
                self.parent_constructor_has_applied_tpt(application.function, depth + 1)
            }
            TreeKind::Block(block) => {
                self.parent_constructor_has_applied_tpt(block.expr, depth + 1)
            }
            TreeKind::TypeApply(application) => {
                self.parent_constructor_has_applied_tpt(application.function, depth + 1)
            }
            TreeKind::Select(selection)
                if self.store.names.resolve(selection.name.text()) == "<init>" =>
            {
                let Some(qualifier) = self.arena.try_get(selection.qualifier) else {
                    return false;
                };
                let TreeKind::New(new) = &qualifier.kind else {
                    return false;
                };
                self.is_applied_type_tree(new.tpt, depth + 1)
            }
            TreeKind::New(new) => self.is_applied_type_tree(new.tpt, depth + 1),
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => {
                self.parent_constructor_has_applied_tpt(parens.inner, depth + 1)
            }
            _ => false,
        }
    }

    fn is_applied_type_tree(&self, tree: TreeId<Untyped>, depth: usize) -> bool {
        if depth > 256 {
            return false;
        }
        let Some(node) = self.arena.try_get(tree) else {
            return false;
        };
        match &node.kind {
            TreeKind::AppliedTypeTree(_) => true,
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => {
                self.is_applied_type_tree(parens.inner, depth + 1)
            }
            _ => false,
        }
    }

    fn parent_type_symbol(&self, ty: TypeId) -> Option<SymbolId> {
        match self.store.types.get(ty) {
            Type::TypeRef { target, .. } => target.symbol(),
            Type::Applied { tycon, .. } => self.parent_type_symbol(*tycon),
            _ => None,
        }
    }

    fn parent_is_trait(
        &mut self,
        ty: TypeId,
        tree_index: u32,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<bool, TyperError> {
        let mut current_type = ty;
        let mut visited = Vec::new();
        for _ in 0..256 {
            let symbol = self.parent_type_symbol(current_type);
            let Some(symbol) = symbol else {
                return Err(TyperError::UnresolvedParentClassKind {
                    source: self.source,
                    tree_index,
                    symbol: None,
                });
            };
            if !self.store.symbols.contains(symbol) {
                return Err(TyperError::UnknownSymbol { symbol });
            }
            if visited.contains(&symbol) {
                return Err(TyperError::UnresolvedParentClassKind {
                    source: self.source,
                    tree_index,
                    symbol: Some(symbol),
                });
            }
            visited.push(symbol);
            match self.store.symbols.get(symbol).kind {
                SymbolKind::Trait => return Ok(true),
                SymbolKind::Class | SymbolKind::ModuleClass => return Ok(false),
                SymbolKind::TypeAlias => {
                    let alias_info = match *self.store.symbols.info(symbol) {
                        SymbolInfo::Complete(info) => info,
                        SymbolInfo::Missing
                            if self.index.definition_of(symbol).is_some_and(|definition| {
                                matches!(
                                    definition,
                                    SourceDefinition::Canonical { source, .. }
                                        | SourceDefinition::Derived { source, .. }
                                        if source == self.source
                                )
                            }) =>
                        {
                            self.complete_symbol_inner(symbol, info_journal)?
                        }
                        SymbolInfo::Missing => {
                            return Err(TyperError::UnresolvedParentClassKind {
                                source: self.source,
                                tree_index,
                                symbol: Some(symbol),
                            });
                        }
                        SymbolInfo::Deferred(_) => {
                            return Err(TyperError::DeferredSymbolCompletion { symbol });
                        }
                        SymbolInfo::Error => {
                            return Err(TyperError::SymbolAlreadyErrored { symbol });
                        }
                    };
                    let Type::AliasingBounds { alias } = self.store.types.get(alias_info) else {
                        return Err(TyperError::UnresolvedParentClassKind {
                            source: self.source,
                            tree_index,
                            symbol: Some(symbol),
                        });
                    };
                    current_type = *alias;
                }
                _ => {
                    return Err(TyperError::UnresolvedParentClassKind {
                        source: self.source,
                        tree_index,
                        symbol: Some(symbol),
                    });
                }
            }
        }
        Err(TyperError::UnresolvedParentClassKind {
            source: self.source,
            tree_index,
            symbol: self.parent_type_symbol(current_type),
        })
    }

    fn complete_type_alias(
        &mut self,
        symbol: SymbolId,
        rhs: TreeId<Untyped>,
        declaration_tree_index: u32,
        context: SourceContextId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        if self
            .store
            .symbols
            .get(symbol)
            .flags
            .contains(SymbolFlags::OPAQUE)
        {
            return Err(TyperError::OpaqueAliasDeferred {
                symbol,
                tree_index: declaration_tree_index,
            });
        }
        let Some(rhs_node) = self.arena.try_get(rhs) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: rhs.index(),
            });
        };
        let info = match &rhs_node.kind {
            TreeKind::LambdaTypeTree(_) => {
                return Err(TyperError::HigherKindedTypeAliasDeferred {
                    symbol,
                    tree_index: rhs.index(),
                });
            }
            TreeKind::TypeBoundsTree(bounds) => {
                let bounds = *bounds;
                if let Some(alias) = bounds.alias {
                    let projected = self.type_of_tpt_inner(alias, context)?;
                    self.alias_bounds_for_type(projected, alias.index())?
                } else {
                    self.project_type_bounds(&bounds, context)?
                }
            }
            _ => {
                let projected = self.type_of_tpt_inner(rhs, context)?;
                self.alias_bounds_for_type(projected, rhs.index())?
            }
        };
        let previous = *self.store.symbols.info(symbol);
        info_journal.push((symbol, previous));
        self.store
            .symbols
            .set_info(symbol, SymbolInfo::Complete(info));
        Ok(info)
    }

    /// Source-side equivalent of Dotty's `toBounds`: genuine bounds and
    /// aliases keep their distinction, ordinary types become aliases, and
    /// methodic/by-name types are rejected.
    fn alias_bounds_for_type(&mut self, ty: TypeId, tree_index: u32) -> Result<TypeId, TyperError> {
        dotty_core::types::to_bounds(&mut self.store.types, ty).map_err(|_| {
            TyperError::InvalidCompletedBounds {
                source: self.source,
                tree_index,
                ty,
            }
        })
    }

    fn complete_type_parameter(
        &mut self,
        symbol: SymbolId,
        rhs: TreeId<Untyped>,
        context: SourceContextId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let Some(rhs_node) = self.arena.try_get(rhs) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: rhs.index(),
            });
        };
        let bounds_tree = match &rhs_node.kind {
            TreeKind::TypeBoundsTree(_) => rhs,
            TreeKind::PhaseSpecific(UntypedNode::ContextBounds(context_bounds)) => {
                // Context-bound evidence remains represented in the source AST
                // for later lowering; this stage completes only the ordinary
                // bounds carried by the wrapper.
                context_bounds.bounds
            }
            TreeKind::LambdaTypeTree(_) => {
                return Err(TyperError::HigherKindedTypeParameterDeferred {
                    symbol,
                    tree_index: rhs.index(),
                });
            }
            _ => {
                return Err(TyperError::UnsupportedTypeTree {
                    source: self.source,
                    tree_index: rhs.index(),
                    tree_kind: tree_kind_name(&rhs_node.kind),
                });
            }
        };
        let Some(bounds_node) = self.arena.try_get(bounds_tree) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: bounds_tree.index(),
            });
        };
        let TreeKind::TypeBoundsTree(bounds) = &bounds_node.kind else {
            return Err(TyperError::UnsupportedTypeTree {
                source: self.source,
                tree_index: bounds_tree.index(),
                tree_kind: tree_kind_name(&bounds_node.kind),
            });
        };
        let bounds = *bounds;
        if bounds.alias.is_some() {
            return Err(TyperError::UnsupportedTypeTree {
                source: self.source,
                tree_index: bounds_tree.index(),
                tree_kind: "aliased type parameter bounds",
            });
        }
        let info = self.project_type_bounds(&bounds, context)?;
        let previous = *self.store.symbols.info(symbol);
        info_journal.push((symbol, previous));
        self.store
            .symbols
            .set_info(symbol, SymbolInfo::Complete(info));
        Ok(info)
    }

    fn project_type_bounds(
        &mut self,
        bounds: &TypeBoundsTree<Untyped>,
        context: SourceContextId,
    ) -> Result<TypeId, TyperError> {
        let low = if let Some(low) = bounds.low {
            self.type_of_tpt_inner(low, context)?
        } else {
            self.definitions.nothing_type
        };
        let high = if let Some(high) = bounds.high {
            self.type_of_tpt_inner(high, context)?
        } else {
            self.definitions.any_type
        };
        Ok(self.store.types.alloc(Type::Bounds { low, high }))
    }

    fn complete_method_signature(
        &mut self,
        method: SymbolId,
        method_tree_index: u32,
        definition: &dotty_core::ast::DefDef<Untyped>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let method_name = self.store.names.resolve(definition.name.as_name().text());
        let is_extension = self
            .store
            .symbols
            .get(method)
            .flags
            .contains(SymbolFlags::EXTENSION);
        if is_extension && method_name.ends_with(':') {
            return Err(TyperError::RightAssociativeExtensionDeferred {
                symbol: method,
                tree_index: method_tree_index,
            });
        }
        let declaration_context = self
            .index
            .declaration_context_of(method)
            .ok_or(TyperError::DeclarationContextMissing { symbol: method })?;
        let mut clauses = Vec::new();
        let prefix_clauses = if is_extension {
            Some(
                self.index
                    .extension_prefix_clauses(method)
                    .ok_or(TyperError::ExtensionPrefixClausesMissing { method })?
                    .to_vec(),
            )
        } else {
            None
        };
        let first_signature_parameter = prefix_clauses
            .as_ref()
            .and_then(|clauses| clauses.iter().flatten().next().map(|tree| (*tree, true)))
            .or_else(|| definition.type_params.first().map(|tree| (*tree, false)))
            .or_else(|| {
                definition
                    .value_param_clauses
                    .iter()
                    .flatten()
                    .next()
                    .map(|tree| (*tree, false))
            });
        let signature_parameter_context = if let Some((tree, derived)) = first_signature_parameter {
            let parameter = self.method_parameter_symbol(method, tree, derived)?;
            self.index
                .declaration_context_of(parameter)
                .ok_or(TyperError::DeclarationContextMissing { symbol: parameter })?
        } else {
            declaration_context
        };
        if let Some(prefix_clauses) = prefix_clauses {
            for (clause_index, trees) in prefix_clauses.iter().enumerate() {
                clauses.push(self.extension_prefix_clause(
                    method,
                    method_tree_index,
                    clause_index,
                    trees,
                    info_journal,
                )?);
            }
        }
        if !definition.type_params.is_empty() {
            clauses.push(MethodClauseSpec::Types(self.type_parameter_specs(
                method,
                &definition.type_params,
                false,
                info_journal,
                method_tree_index,
                clauses.len(),
            )?));
        }
        for trees in &definition.value_param_clauses {
            let clause_index = clauses.len();
            let (parameters, kind) = self.method_parameter_specs(
                method,
                trees,
                false,
                method_tree_index,
                clause_index,
                info_journal,
            )?;
            clauses.push(MethodClauseSpec::Terms(parameters, kind));
        }

        let Some(result_node) = self.arena.try_get(definition.tpt) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: definition.tpt.index(),
            });
        };
        if matches!(&result_node.kind, TreeKind::TypeTree(_)) {
            return Err(TyperError::InferredMethodResultDeferred {
                symbol: method,
                tree_index: definition.tpt.index(),
            });
        }
        let mut signature = self.type_of_tpt_inner(definition.tpt, signature_parameter_context)?;
        for clause in clauses.into_iter().rev() {
            signature = match clause {
                MethodClauseSpec::Types(parameters) => {
                    poly_type_from_symbols(self.store, &parameters, signature).map_err(|error| {
                        TyperError::TypeRebinding {
                            source: self.source,
                            tree_index: method_tree_index,
                            error,
                        }
                    })?
                }
                MethodClauseSpec::Terms(parameters, kind) => {
                    method_type_from_symbols(self.store, &parameters, signature, kind).map_err(
                        |error| TyperError::TypeRebinding {
                            source: self.source,
                            tree_index: method_tree_index,
                            error,
                        },
                    )?
                }
            };
        }
        Ok(signature)
    }

    fn complete_constructor_signature(
        &mut self,
        constructor: SymbolId,
        constructor_tree: TreeId<Untyped>,
        definition: &dotty_core::ast::DefDef<Untyped>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let owner = self.constructor_owner(constructor)?;
        let owner_constructor = self.owner_primary_constructor_tree(constructor, owner)?;
        let is_primary = owner_constructor == Some(constructor_tree);

        if !is_primary && !definition.type_params.is_empty() {
            return Err(TyperError::SecondaryConstructorTypeParametersDeferred { constructor });
        }
        if !is_primary
            && definition.type_params.is_empty()
            && !self.owner_type_parameters(constructor, owner)?.is_empty()
        {
            return Err(TyperError::GenericSecondaryConstructorDeferred { constructor, owner });
        }

        let mut clauses = Vec::new();
        if !definition.type_params.is_empty() {
            let clause_index = clauses.len();
            clauses.push(MethodClauseSpec::Types(self.type_parameter_specs(
                constructor,
                &definition.type_params,
                is_primary,
                info_journal,
                constructor_tree.index(),
                clause_index,
            )?));
        }
        for trees in &definition.value_param_clauses {
            let clause_index = clauses.len();
            let (parameters, kind) = self.method_parameter_specs(
                constructor,
                trees,
                is_primary,
                constructor_tree.index(),
                clause_index,
                info_journal,
            )?;
            clauses.push(MethodClauseSpec::Terms(parameters, kind));
        }
        let clauses = Self::normalize_constructor_clauses(&clauses);
        let mut result = self.constructor_effective_result(owner, &clauses);
        for clause in clauses.into_iter().rev() {
            result = match clause {
                MethodClauseSpec::Types(parameters) => {
                    poly_type_from_symbols(self.store, &parameters, result).map_err(|error| {
                        TyperError::TypeRebinding {
                            source: self.source,
                            tree_index: constructor_tree.index(),
                            error,
                        }
                    })?
                }
                MethodClauseSpec::Terms(parameters, kind) => {
                    method_type_from_symbols(self.store, &parameters, result, kind).map_err(
                        |error| TyperError::TypeRebinding {
                            source: self.source,
                            tree_index: constructor_tree.index(),
                            error,
                        },
                    )?
                }
            };
        }
        Ok(result)
    }

    fn constructor_owner(&self, constructor: SymbolId) -> Result<SymbolId, TyperError> {
        let owner = self.store.symbols.get(constructor).owner;
        let Some(owner) = owner.filter(|owner| {
            self.store.symbols.contains(*owner)
                && matches!(
                    self.store.symbols.get(*owner).kind,
                    SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
                )
        }) else {
            return Err(TyperError::ConstructorOwnerNotClassLike { constructor, owner });
        };
        if let SymbolInfo::Complete(info) = *self.store.symbols.info(owner)
            && !matches!(
                self.store.types.get(info),
                Type::ClassInfo(class_info)
                    if class_info.class == owner && class_info.prefix == self.definitions.no_prefix
            )
        {
            return Err(TyperError::MalformedConstructorOwnerInfo {
                constructor,
                owner,
                info,
            });
        }
        Ok(owner)
    }

    fn owner_primary_constructor_tree(
        &self,
        constructor: SymbolId,
        owner: SymbolId,
    ) -> Result<Option<TreeId<Untyped>>, TyperError> {
        let definition = self
            .index
            .definition_of(owner)
            .ok_or(TyperError::SourceProvenanceMissing { symbol: owner })?;
        let (source, tree) = match definition {
            SourceDefinition::Canonical { source, tree }
            | SourceDefinition::Derived { source, tree } => (source, tree),
        };
        if source != self.source {
            return Err(TyperError::SourceProvenanceMissing { symbol: owner });
        }
        let Some(node) = self.arena.try_get(tree) else {
            return Err(TyperError::TreeOutsideArena {
                source,
                tree_index: tree.index(),
            });
        };
        let template_tree = match &node.kind {
            TreeKind::TypeDef(definition) => definition.rhs,
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(definition)) => definition.template,
            _ => {
                return Err(TyperError::MalformedConstructorOwner {
                    constructor,
                    owner,
                    tree_index: tree.index(),
                });
            }
        };
        let Some(template_node) = self.arena.try_get(template_tree) else {
            return Err(TyperError::TreeOutsideArena {
                source,
                tree_index: template_tree.index(),
            });
        };
        match &template_node.kind {
            TreeKind::Template(template) => Ok(Some(template.constructor)),
            _ => Err(TyperError::MalformedConstructorOwner {
                constructor,
                owner,
                tree_index: template_tree.index(),
            }),
        }
    }

    fn owner_type_parameters(
        &self,
        constructor: SymbolId,
        owner: SymbolId,
    ) -> Result<Vec<TreeId<Untyped>>, TyperError> {
        let Some(owner_constructor) = self.owner_primary_constructor_tree(constructor, owner)?
        else {
            return Ok(Vec::new());
        };
        let Some(node) = self.arena.try_get(owner_constructor) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: owner_constructor.index(),
            });
        };
        match &node.kind {
            TreeKind::DefDef(definition) => Ok(definition.type_params.clone()),
            _ => Err(TyperError::MalformedConstructorOwner {
                constructor,
                owner,
                tree_index: owner_constructor.index(),
            }),
        }
    }

    fn constructor_effective_result(
        &mut self,
        owner: SymbolId,
        clauses: &[MethodClauseSpec],
    ) -> TypeId {
        let owner_ref = self
            .store
            .types
            .alloc(Type::type_ref(self.definitions.no_prefix, owner));
        match clauses.first() {
            Some(MethodClauseSpec::Types(parameters)) => {
                let args = parameters
                    .iter()
                    .map(|parameter| {
                        self.store
                            .types
                            .alloc(Type::type_ref(self.definitions.no_prefix, parameter.symbol))
                    })
                    .collect();
                self.store.types.alloc(Type::Applied {
                    tycon: owner_ref,
                    args,
                })
            }
            _ => owner_ref,
        }
    }

    fn normalize_constructor_clauses(clauses: &[MethodClauseSpec]) -> Vec<MethodClauseSpec> {
        match clauses.split_first() {
            Some((MethodClauseSpec::Types(parameters), rest)) => {
                let mut normalized = vec![MethodClauseSpec::Types(parameters.clone())];
                normalized.extend(Self::normalize_constructor_clauses(rest));
                normalized
            }
            Some((MethodClauseSpec::Terms(parameters, MethodKind::Implicit), _))
                if !parameters.is_empty() =>
            {
                let mut normalized = vec![MethodClauseSpec::Terms(Vec::new(), MethodKind::Plain)];
                normalized.extend(clauses.iter().cloned());
                normalized
            }
            _ => {
                let all_contextual = clauses.iter().all(|clause| match clause {
                    MethodClauseSpec::Types(_) => true,
                    MethodClauseSpec::Terms(parameters, MethodKind::Contextual) => {
                        !parameters.is_empty()
                    }
                    MethodClauseSpec::Terms(_, MethodKind::Plain | MethodKind::Implicit) => false,
                });
                let mut normalized = clauses.to_vec();
                if all_contextual {
                    normalized.push(MethodClauseSpec::Terms(Vec::new(), MethodKind::Plain));
                }
                normalized
            }
        }
    }

    fn extension_prefix_clause(
        &mut self,
        method: SymbolId,
        method_tree_index: u32,
        clause_index: usize,
        trees: &[TreeId<Untyped>],
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<MethodClauseSpec, TyperError> {
        let Some(first_tree) = trees.first() else {
            return Ok(MethodClauseSpec::Terms(Vec::new(), MethodKind::Plain));
        };
        let Some(first_node) = self.arena.try_get(*first_tree) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: first_tree.index(),
            });
        };
        match &first_node.kind {
            TreeKind::TypeDef(_) => Ok(MethodClauseSpec::Types(self.type_parameter_specs(
                method,
                trees,
                true,
                info_journal,
                method_tree_index,
                clause_index,
            )?)),
            TreeKind::ValDef(_) => {
                let (parameters, kind) = self.method_parameter_specs(
                    method,
                    trees,
                    true,
                    method_tree_index,
                    clause_index,
                    info_journal,
                )?;
                Ok(MethodClauseSpec::Terms(parameters, kind))
            }
            _ => Err(TyperError::MalformedMethodClause {
                method,
                method_tree_index,
                clause_index,
            }),
        }
    }

    fn type_parameter_specs(
        &mut self,
        method: SymbolId,
        trees: &[TreeId<Untyped>],
        derived: bool,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        method_tree_index: u32,
        clause_index: usize,
    ) -> Result<Vec<TypeParamSpec>, TyperError> {
        let mut parameters = Vec::with_capacity(trees.len());
        for tree in trees {
            let Some(node) = self.arena.try_get(*tree) else {
                return Err(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: tree.index(),
                });
            };
            let TreeKind::TypeDef(definition) = &node.kind else {
                return Err(TyperError::MalformedMethodClause {
                    method,
                    method_tree_index,
                    clause_index,
                });
            };
            let symbol = self.method_parameter_symbol(method, *tree, derived)?;
            if self.store.symbols.get(symbol).kind != SymbolKind::TypeParameter {
                return Err(TyperError::MalformedMethodClause {
                    method,
                    method_tree_index,
                    clause_index,
                });
            }
            let bounds = self.complete_signature_parameter(symbol, info_journal)?;
            parameters.push(TypeParamSpec {
                symbol,
                name: definition.name,
                bounds,
                declared_variance: None,
            });
        }
        Ok(parameters)
    }

    fn method_parameter_specs(
        &mut self,
        method: SymbolId,
        trees: &[TreeId<Untyped>],
        derived: bool,
        method_tree_index: u32,
        clause_index: usize,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<(Vec<MethodParamSpec>, MethodKind), TyperError> {
        let mut parameters = Vec::with_capacity(trees.len());
        let mut clause_kind = None;
        for tree in trees {
            let Some(node) = self.arena.try_get(*tree) else {
                return Err(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: tree.index(),
                });
            };
            let TreeKind::ValDef(definition) = &node.kind else {
                return Err(TyperError::MalformedMethodClause {
                    method,
                    method_tree_index,
                    clause_index,
                });
            };
            let symbol = self.method_parameter_symbol(method, *tree, derived)?;
            if self.store.symbols.get(symbol).kind != SymbolKind::Parameter {
                return Err(TyperError::MalformedMethodClause {
                    method,
                    method_tree_index,
                    clause_index,
                });
            }
            let flags = self.store.symbols.get(symbol).flags;
            let given = flags.contains(SymbolFlags::GIVEN);
            let implicit = flags.contains(SymbolFlags::IMPLICIT);
            if given && implicit {
                return Err(TyperError::MalformedMethodClause {
                    method,
                    method_tree_index,
                    clause_index,
                });
            }
            let parameter_kind = if given {
                MethodKind::Contextual
            } else if implicit {
                MethodKind::Implicit
            } else {
                MethodKind::Plain
            };
            if clause_kind.is_some_and(|kind| kind != parameter_kind) {
                return Err(TyperError::MalformedMethodClause {
                    method,
                    method_tree_index,
                    clause_index,
                });
            }
            clause_kind = Some(parameter_kind);
            let repeated_parameter = matches!(
                self.arena.try_get(definition.tpt).map(|node| &node.kind),
                Some(TreeKind::PhaseSpecific(UntypedNode::PostfixOp(postfix)))
                    if self.store.names.resolve(postfix.op.text()) == "*"
            );
            let completed_parameter_type =
                self.complete_signature_parameter(symbol, info_journal)?;
            let ty = if repeated_parameter {
                match self.store.types.try_get(completed_parameter_type) {
                    Some(Type::Repeated { element }) => *element,
                    _ => {
                        return Err(TyperError::MalformedMethodClause {
                            method,
                            method_tree_index,
                            clause_index,
                        });
                    }
                }
            } else {
                completed_parameter_type
            };
            parameters.push(MethodParamSpec {
                symbol,
                name: definition.name,
                ty,
                erased: flags.contains(SymbolFlags::ERASED),
                varargs: repeated_parameter,
            });
        }
        Ok((parameters, clause_kind.unwrap_or(MethodKind::Plain)))
    }

    fn method_parameter_symbol(
        &self,
        method: SymbolId,
        tree: TreeId<Untyped>,
        derived: bool,
    ) -> Result<SymbolId, TyperError> {
        let symbol = if derived {
            self.index.derived_symbol_at(method, self.source, tree)
        } else {
            self.index.symbol_at(self.source, tree)
        };
        symbol.ok_or(TyperError::MethodParameterSymbolMissing {
            method,
            parameter_tree_index: tree.index(),
        })
    }

    fn complete_signature_parameter(
        &mut self,
        symbol: SymbolId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        if !self.store.symbols.contains(symbol) {
            return Err(TyperError::UnknownSymbol { symbol });
        }
        match *self.store.symbols.info(symbol) {
            SymbolInfo::Complete(ty) => Ok(ty),
            SymbolInfo::Missing => self.complete_symbol_inner(symbol, info_journal),
            SymbolInfo::Deferred(_) => Err(TyperError::DeferredSymbolCompletion { symbol }),
            SymbolInfo::Error => Err(TyperError::SymbolAlreadyErrored { symbol }),
        }
    }

    /// Projects a source type tree using the declaration context from the namer.
    pub fn type_of_tpt(
        &mut self,
        tree: TreeId<Untyped>,
        context: SourceContextId,
    ) -> Result<TypeId, TyperError> {
        self.run_atomic(|typer, _| typer.type_of_tpt_inner(tree, context))
    }

    fn type_of_tpt_inner(
        &mut self,
        tree: TreeId<Untyped>,
        context: SourceContextId,
    ) -> Result<TypeId, TyperError> {
        if let Some(ty) = self.type_index.type_at(self.source, tree) {
            return Ok(ty);
        }
        let Some(source_tree) = self.arena.try_get(tree) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: tree.index(),
            });
        };
        if matches!(&source_tree.kind, TreeKind::TypeTree(_)) {
            return Err(TyperError::MissingDeclaredType {
                source: self.source,
                tree_index: tree.index(),
                position: source_tree.position,
            });
        }
        if let TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) = &source_tree.kind {
            let ty = self.type_of_tpt_inner(parens.inner, context)?;
            if let Err(existing) = self.type_index.insert(self.source, tree, ty) {
                return Err(TyperError::DuplicateSourceTypeCacheEntry {
                    source: self.source,
                    tree_index: tree.index(),
                    existing,
                    attempted: ty,
                });
            }
            return Ok(ty);
        }
        if let TreeKind::ByNameTypeTree(by_name) = &source_tree.kind {
            let result = self.type_of_tpt_inner(by_name.result, context)?;
            let ty = self.store.types.alloc(Type::ByName { result });
            if let Err(existing) = self.type_index.insert(self.source, tree, ty) {
                return Err(TyperError::DuplicateSourceTypeCacheEntry {
                    source: self.source,
                    tree_index: tree.index(),
                    existing,
                    attempted: ty,
                });
            }
            return Ok(ty);
        }
        if let TreeKind::AppliedTypeTree(applied) = &source_tree.kind {
            let tycon = self.type_of_tpt_inner(applied.tpt, context)?;
            let mut args = Vec::with_capacity(applied.args.len());
            for argument in &applied.args {
                args.push(self.type_of_tpt_inner(*argument, context)?);
            }
            let ty = self.store.types.alloc(Type::Applied { tycon, args });
            if let Err(existing) = self.type_index.insert(self.source, tree, ty) {
                return Err(TyperError::DuplicateSourceTypeCacheEntry {
                    source: self.source,
                    tree_index: tree.index(),
                    existing,
                    attempted: ty,
                });
            }
            return Ok(ty);
        }
        if let TreeKind::Select(select) = &source_tree.kind {
            let ty = SourceNameResolver { typer: self }.resolve_type_member(
                select.qualifier,
                select.name,
                context,
                SourceTreeLocation {
                    tree_index: tree.index(),
                    position: source_tree.position,
                },
            )?;
            if let Err(existing) = self.type_index.insert(self.source, tree, ty) {
                return Err(TyperError::DuplicateSourceTypeCacheEntry {
                    source: self.source,
                    tree_index: tree.index(),
                    existing,
                    attempted: ty,
                });
            }
            return Ok(ty);
        }
        let TreeKind::Ident(Ident { name, .. }) = &source_tree.kind else {
            return Err(TyperError::UnsupportedTypeTree {
                source: self.source,
                tree_index: tree.index(),
                tree_kind: tree_kind_name(&source_tree.kind),
            });
        };
        if !name.is_type() {
            return Err(TyperError::UnsupportedTypeTree {
                source: self.source,
                tree_index: tree.index(),
                tree_kind: "term identifier",
            });
        }
        if self.index.try_source_context(context).is_none() {
            return Err(TyperError::SourceContextMissing {
                source: self.source,
                tree_index: tree.index(),
                context_index: context.index(),
            });
        }
        let position = source_tree.position;
        let ty = SourceNameResolver { typer: self }.resolve_type_name(
            *name,
            context,
            SourceTreeLocation {
                tree_index: tree.index(),
                position,
            },
        )?;
        if let Err(existing) = self.type_index.insert(self.source, tree, ty) {
            return Err(TyperError::DuplicateSourceTypeCacheEntry {
                source: self.source,
                tree_index: tree.index(),
                existing,
                attempted: ty,
            });
        }
        Ok(ty)
    }

    pub(super) fn type_symbol_prefix(&mut self, symbol: SymbolId) -> TypeId {
        let Some(owner) = self.store.symbols.get(symbol).owner else {
            return self.definitions.no_prefix;
        };
        match self.store.symbols.get(owner).kind {
            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass => {
                self.store.types.alloc(Type::ThisType { class: owner })
            }
            SymbolKind::Package => self.package_type_prefix(owner),
            _ => self.definitions.no_prefix,
        }
    }

    fn package_type_prefix(&mut self, package: SymbolId) -> TypeId {
        self.store.types.alloc(Type::TypeRef {
            prefix: self.definitions.no_prefix,
            target: TypeRefTarget::Symbol(package),
        })
    }

    fn type_of_selected_tpt(
        &mut self,
        qualifier_tree: TreeId<Untyped>,
        name: dotty_core::Name,
        context: SourceContextId,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<TypeId, TyperError> {
        if !name.is_type() {
            return Err(TyperError::UnsupportedTypeTree {
                source: self.source,
                tree_index,
                tree_kind: "term selection",
            });
        }
        let Some(qualifier) =
            self.resolve_qualifier_symbol(qualifier_tree, context, tree_index, position)?
        else {
            return Err(TyperError::TypeNameNotFound {
                source: self.source,
                tree_index,
                name,
                position,
            });
        };
        let scopes = self.scopes_of(qualifier);
        let type_candidates: Vec<_> = scopes
            .iter()
            .flat_map(|scope| {
                self.store
                    .scopes
                    .get(*scope)
                    .lookup_all(&name)
                    .iter()
                    .copied()
            })
            .collect();
        if let Some(symbol) =
            self.unique_type_candidate(&type_candidates, name, tree_index, position)?
        {
            let prefix = self.type_symbol_prefix(symbol);
            return Ok(self.store.types.alloc(Type::TypeRef {
                prefix,
                target: TypeRefTarget::Symbol(symbol),
            }));
        }
        let term_name = dotty_core::Name::new(name.text(), dotty_core::Namespace::Term);
        let term_candidates: Vec<_> = scopes
            .iter()
            .flat_map(|scope| {
                self.store
                    .scopes
                    .get(*scope)
                    .lookup_all(&term_name)
                    .iter()
                    .copied()
            })
            .collect();
        if let Some(symbol) =
            self.unique_symbol_candidate(&term_candidates, name, tree_index, position)?
        {
            return Err(TyperError::WrongTypeNameKind {
                source: self.source,
                tree_index,
                name,
                symbol,
                kind: self.store.symbols.get(symbol).kind,
                position,
            });
        }
        let prefix = self.type_prefix_for_qualifier(qualifier);
        let request = MemberRequest {
            prefix,
            name,
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };
        let external = self
            .resolver
            .resolve_member(self.store, &request)
            .map_err(|error| TyperError::SymbolResolution {
                source: self.source,
                tree_index,
                error,
            })?;
        if let Some(symbol) = external {
            if !self.store.symbols.contains(symbol) {
                return Err(TyperError::UnknownSymbol { symbol });
            }
            if matches!(
                self.store.symbols.get(symbol).kind,
                SymbolKind::Class
                    | SymbolKind::Trait
                    | SymbolKind::ModuleClass
                    | SymbolKind::TypeParameter
                    | SymbolKind::TypeAlias
            ) {
                let prefix = self.type_symbol_prefix(symbol);
                return Ok(self.store.types.alloc(Type::TypeRef {
                    prefix,
                    target: TypeRefTarget::Symbol(symbol),
                }));
            }
            return Err(TyperError::WrongTypeNameKind {
                source: self.source,
                tree_index,
                name,
                symbol,
                kind: self.store.symbols.get(symbol).kind,
                position,
            });
        }
        Err(TyperError::TypeNameNotFound {
            source: self.source,
            tree_index,
            name,
            position,
        })
    }

    fn resolve_qualifier_symbol(
        &mut self,
        tree: TreeId<Untyped>,
        context: SourceContextId,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<Option<SymbolId>, TyperError> {
        let Some(node) = self.arena.try_get(tree) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: tree.index(),
            });
        };
        match &node.kind {
            TreeKind::Ident(ident) => {
                if let Some(symbol) =
                    self.lookup_context_symbol(ident.name, context, tree_index, position)?
                {
                    return Ok(Some(symbol));
                }
                let segment = self.store.names.resolve(ident.name.text()).to_owned();
                self.resolve_external_package(&[segment], tree_index)
            }
            TreeKind::Select(select) => {
                let Some(qualifier) =
                    self.resolve_qualifier_symbol(select.qualifier, context, tree_index, position)?
                else {
                    return Ok(None);
                };
                let mut candidates = Vec::new();
                for scope in self.scopes_of(qualifier) {
                    if let Some(symbol) =
                        self.unique_scoped_symbol(scope, select.name, tree_index, position)?
                    {
                        candidates.push(symbol);
                    }
                }
                candidates.sort_by_key(|symbol| symbol.index());
                candidates.dedup();
                if let Some(symbol) =
                    self.unique_symbol_candidate(&candidates, select.name, tree_index, position)?
                {
                    return Ok(Some(symbol));
                }
                if self.store.symbols.get(qualifier).kind == SymbolKind::Package {
                    let mut path = self.package_path(qualifier);
                    path.push(self.store.names.resolve(select.name.text()).to_owned());
                    if let Some(package) = self.resolve_external_package(&path, tree_index)? {
                        return Ok(Some(package));
                    }
                }
                let prefix = self.type_prefix_for_qualifier(qualifier);
                let request = MemberRequest {
                    prefix,
                    name: select.name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                };
                self.resolver
                    .resolve_member(self.store, &request)
                    .map_err(|error| TyperError::SymbolResolution {
                        source: self.source,
                        tree_index,
                        error,
                    })
            }
            _ => Err(TyperError::UnsupportedTypeTree {
                source: self.source,
                tree_index,
                tree_kind: tree_kind_name(&node.kind),
            }),
        }
    }

    fn resolve_external_package(
        &mut self,
        path: &[String],
        tree_index: u32,
    ) -> Result<Option<SymbolId>, TyperError> {
        let segments: Vec<_> = path.iter().map(String::as_str).collect();
        let package = self
            .resolver
            .resolve_package(self.store, &segments)
            .map_err(|error| TyperError::SymbolResolution {
                source: self.source,
                tree_index,
                error,
            })?;
        if let Some(symbol) = package {
            if !self.store.symbols.contains(symbol) {
                return Err(TyperError::UnknownSymbol { symbol });
            }
            if self.store.symbols.get(symbol).kind != SymbolKind::Package {
                return Err(TyperError::SymbolResolution {
                    source: self.source,
                    tree_index,
                    error: ResolutionError::Malformed {
                        reason: "package resolution returned a non-package symbol".to_owned(),
                    },
                });
            }
        }
        Ok(package)
    }

    fn package_path(&self, package: SymbolId) -> Vec<String> {
        let mut path = Vec::new();
        let mut current = Some(package);
        while let Some(symbol) = current {
            let entry = self.store.symbols.get(symbol);
            if entry.kind != SymbolKind::Package {
                break;
            }
            let name = self.store.names.resolve(entry.name.text());
            if !name.is_empty() {
                path.push(name.to_owned());
            }
            current = entry.owner;
        }
        path.reverse();
        path
    }

    fn type_prefix_for_qualifier(&mut self, symbol: SymbolId) -> TypeId {
        match self.store.symbols.get(symbol).kind {
            SymbolKind::Package => self.package_type_prefix(symbol),
            SymbolKind::Object => {
                let owner = self.store.symbols.get(symbol).owner;
                let module_class = owner
                    .and_then(|owner| self.index.definition_of(symbol).map(|_| owner))
                    .and_then(|owner| {
                        self.index
                            .definition_of(symbol)
                            .and_then(|definition| match definition {
                                SourceDefinition::Canonical { source, tree }
                                | SourceDefinition::Derived { source, tree }
                                    if source == self.source =>
                                {
                                    self.index.derived_symbol_at(owner, source, tree)
                                }
                                _ => None,
                            })
                    });
                if let Some(module_class) = module_class {
                    self.store.types.alloc(Type::ThisType {
                        class: module_class,
                    })
                } else {
                    self.definitions.no_prefix
                }
            }
            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass => {
                let prefix = self.type_symbol_prefix(symbol);
                self.store.types.alloc(Type::TypeRef {
                    prefix,
                    target: TypeRefTarget::Symbol(symbol),
                })
            }
            _ => self.definitions.no_prefix,
        }
    }

    fn lookup_type_symbol(
        &mut self,
        name: dotty_core::Name,
        context: SourceContextId,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<Option<SymbolId>, TyperError> {
        let contexts = self.source_context_chain(context, tree_index)?;
        let mut package_candidates = Vec::new();
        for context_id in &contexts {
            let source_context = self.index.source_context(*context_id);
            let candidates = self
                .store
                .scopes
                .get(source_context.lexical_scope)
                .lookup_all(&name);
            if self.store.symbols.get(source_context.owner).kind == SymbolKind::Package {
                let (current_unit, other_units): (Vec<_>, Vec<_>) =
                    candidates.iter().copied().partition(|symbol| {
                        self.store.symbols.get(*symbol).origin == SymbolOrigin::Source(self.source)
                    });
                if let Some(symbol) =
                    self.unique_type_candidate(&current_unit, name, tree_index, position)?
                {
                    return Ok(Some(symbol));
                }
                package_candidates.extend(other_units);
            } else if let Some(symbol) =
                self.unique_type_candidate(candidates, name, tree_index, position)?
            {
                return Ok(Some(symbol));
            }
        }
        for selection in [ImportSelection::Explicit, ImportSelection::Wildcard] {
            if let Some(symbol) = self.lookup_imports(
                &contexts,
                name,
                true,
                selection,
                SourceTreeLocation {
                    tree_index,
                    position,
                },
            )? {
                return Ok(Some(symbol));
            }
        }
        self.unique_type_candidate(&package_candidates, name, tree_index, position)
    }

    fn source_context_chain(
        &self,
        context: SourceContextId,
        tree_index: u32,
    ) -> Result<Vec<SourceContextId>, TyperError> {
        let mut contexts = Vec::new();
        let mut current = Some(context);
        while let Some(context_id) = current {
            let Some(source_context) = self.index.try_source_context(context_id) else {
                return Err(TyperError::SourceContextMissing {
                    source: self.source,
                    tree_index,
                    context_index: context_id.index(),
                });
            };
            contexts.push(context_id);
            current = source_context.parent;
        }
        Ok(contexts)
    }

    fn lookup_imported_symbols(
        &mut self,
        source_import: SourceImport,
        wanted: dotty_core::Name,
        type_only: bool,
        selection: ImportSelection,
        location: SourceTreeLocation,
    ) -> Result<Vec<SymbolId>, TyperError> {
        let Some(node) = self.arena.try_get(source_import.tree) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: source_import.tree.index(),
            });
        };
        let TreeKind::Import(import) = &node.kind else {
            return Err(TyperError::MalformedSourceImport {
                source: self.source,
                import_tree_index: source_import.tree.index(),
            });
        };

        let wildcard = self.store.names.get("*");
        let hidden = self.store.names.get("_");
        let mut hidden_names = Vec::new();
        for selector in &import.selectors {
            if let Some(renamed) = selector.renamed {
                let Some(rename_tree) = self.arena.try_get(renamed) else {
                    return Err(TyperError::MalformedSourceImport {
                        source: self.source,
                        import_tree_index: source_import.tree.index(),
                    });
                };
                if !matches!(rename_tree.kind, TreeKind::Ident(_)) {
                    return Err(TyperError::MalformedSourceImport {
                        source: self.source,
                        import_tree_index: source_import.tree.index(),
                    });
                }
                hidden_names.push(selector.imported.text());
            }
        }
        let mut relevant = Vec::new();
        for selector in &import.selectors {
            if Some(selector.imported.text()) == wildcard {
                if matches!(selection, ImportSelection::Explicit)
                    || hidden_names.contains(&wanted.text())
                {
                    continue;
                }
                relevant.push((selector.imported, true));
                continue;
            }
            if matches!(selection, ImportSelection::Wildcard) {
                continue;
            }
            if selector.bound.is_some() {
                continue;
            }
            let public_name = if let Some(renamed) = selector.renamed {
                let Some(rename_tree) = self.arena.try_get(renamed) else {
                    return Err(TyperError::MalformedSourceImport {
                        source: self.source,
                        import_tree_index: source_import.tree.index(),
                    });
                };
                let TreeKind::Ident(ident) = &rename_tree.kind else {
                    return Err(TyperError::MalformedSourceImport {
                        source: self.source,
                        import_tree_index: source_import.tree.index(),
                    });
                };
                ident.name.text()
            } else {
                selector.imported.text()
            };
            if public_name == wanted.text() && Some(public_name) != hidden {
                relevant.push((selector.imported, false));
            }
        }
        if relevant.is_empty() {
            return Ok(Vec::new());
        }

        let qualifier_context = source_import.parent.unwrap_or(source_import.context);
        let Some(qualifier) = self.import_qualifier_symbol(
            import.expr,
            qualifier_context,
            source_import.tree.index(),
            location.position,
        )?
        else {
            return Err(TyperError::ImportQualifierNotFound {
                source: self.source,
                import_tree_index: source_import.tree.index(),
            });
        };
        let scopes = self.scopes_of(qualifier);
        let mut matches = Vec::new();
        for (imported, is_wildcard) in relevant {
            let name = if is_wildcard {
                wanted
            } else if type_only {
                dotty_core::Name::new(imported.text(), dotty_core::Namespace::Type)
            } else {
                imported
            };
            let mut found_locally = false;
            for scope in &scopes {
                if type_only {
                    let candidates = self.store.scopes.get(*scope).lookup_all(&name);
                    found_locally |= !candidates.is_empty();
                    matches.extend_from_slice(candidates);
                } else {
                    let candidates = self.scoped_symbols(*scope, name);
                    found_locally |= !candidates.is_empty();
                    matches.extend(candidates);
                }
            }
            if !found_locally {
                let request = MemberRequest {
                    prefix: self.type_prefix_for_qualifier(qualifier),
                    name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                };
                if let Some(symbol) =
                    self.resolver
                        .resolve_member(self.store, &request)
                        .map_err(|error| TyperError::SymbolResolution {
                            source: self.source,
                            tree_index: location.tree_index,
                            error,
                        })?
                {
                    if !self.store.symbols.contains(symbol) {
                        return Err(TyperError::UnknownSymbol { symbol });
                    }
                    matches.push(symbol);
                }
            }
        }
        matches.sort_by_key(|symbol| symbol.index());
        matches.dedup();
        self.deduplicate_import_candidates(&mut matches, type_only);
        if type_only {
            Ok(self
                .unique_type_candidate(&matches, wanted, location.tree_index, location.position)?
                .into_iter()
                .collect())
        } else {
            Ok(matches)
        }
    }

    fn import_qualifier_symbol(
        &mut self,
        qualifier: TreeId<Untyped>,
        context: SourceContextId,
        import_tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<Option<SymbolId>, TyperError> {
        self.resolve_qualifier_symbol(qualifier, context, import_tree_index, position)
    }

    fn lookup_context_symbol(
        &mut self,
        name: dotty_core::Name,
        context: SourceContextId,
        import_tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<Option<SymbolId>, TyperError> {
        let contexts = self.source_context_chain(context, import_tree_index)?;
        for context_id in &contexts {
            let source_context = self.index.source_context(*context_id);
            if let Some(symbol) = self.unique_scoped_symbol(
                source_context.lexical_scope,
                name,
                import_tree_index,
                position,
            )? {
                return Ok(Some(symbol));
            }
        }
        for selection in [ImportSelection::Explicit, ImportSelection::Wildcard] {
            if let Some(symbol) = self.lookup_imports(
                &contexts,
                name,
                false,
                selection,
                SourceTreeLocation {
                    tree_index: import_tree_index,
                    position,
                },
            )? {
                return Ok(Some(symbol));
            }
        }
        Ok(None)
    }

    /// Looks up all imports at the same lexical depth together. Imports in one
    /// scope have equal precedence, so distinct matching symbols are
    /// ambiguous regardless of their source order.
    fn lookup_imports(
        &mut self,
        contexts: &[SourceContextId],
        name: dotty_core::Name,
        type_only: bool,
        selection: ImportSelection,
        location: SourceTreeLocation,
    ) -> Result<Option<SymbolId>, TyperError> {
        let candidates =
            self.lookup_import_candidates(contexts, name, type_only, selection, location)?;
        if candidates.is_empty() {
            return Ok(None);
        }
        if type_only {
            self.unique_type_candidate(&candidates, name, location.tree_index, location.position)
        } else {
            self.unique_symbol_candidate(&candidates, name, location.tree_index, location.position)
        }
    }

    fn lookup_import_candidates(
        &mut self,
        contexts: &[SourceContextId],
        name: dotty_core::Name,
        type_only: bool,
        selection: ImportSelection,
        location: SourceTreeLocation,
    ) -> Result<Vec<SymbolId>, TyperError> {
        let mut scopes: Vec<(dotty_core::ScopeId, Vec<SourceImport>)> = Vec::new();
        for context_id in contexts {
            let source_context = self.index.source_context(*context_id);
            let Some(import_tree) = source_context.import else {
                continue;
            };
            let import = SourceImport {
                tree: import_tree,
                context: *context_id,
                parent: source_context.parent,
            };
            if let Some(scope_index) = scopes
                .iter()
                .position(|(scope, _)| *scope == source_context.lexical_scope)
            {
                scopes[scope_index].1.push(import);
            } else {
                scopes.push((source_context.lexical_scope, vec![import]));
            }
        }
        for (_, imports) in scopes {
            let mut candidates = Vec::new();
            for source_import in imports {
                candidates.extend(self.lookup_imported_symbols(
                    source_import,
                    name,
                    type_only,
                    selection,
                    location,
                )?);
            }
            candidates.sort_by_key(|symbol| symbol.index());
            candidates.dedup();
            self.deduplicate_import_candidates(&mut candidates, type_only);
            if !candidates.is_empty() {
                return Ok(candidates);
            }
        }
        Ok(Vec::new())
    }

    /// Returns the underlying symbol for a fully known chain of type aliases.
    /// Unknown or structurally described aliases remain distinct so lookup
    /// never guesses that two incomplete types are equivalent.
    fn imported_type_target(&self, symbol: SymbolId) -> Option<SymbolId> {
        let mut current = symbol;
        let mut seen = Vec::new();
        loop {
            if seen.contains(&current) {
                return None;
            }
            seen.push(current);
            if self.store.symbols.get(current).kind != SymbolKind::TypeAlias {
                return Some(current);
            }
            let SymbolInfo::Complete(info) = self.store.symbols.info(current) else {
                return None;
            };
            let Type::AliasingBounds { alias } = self.store.types.get(*info) else {
                return None;
            };
            let Type::TypeRef {
                target: TypeRefTarget::Symbol(target),
                ..
            } = self.store.types.get(*alias)
            else {
                return None;
            };
            current = *target;
        }
    }

    fn deduplicate_import_candidates(&self, candidates: &mut Vec<SymbolId>, type_only: bool) {
        if !type_only {
            return;
        }
        let mut unique = Vec::with_capacity(candidates.len());
        for candidate in candidates.iter().copied() {
            let canonical = self.imported_type_target(candidate);
            let already_present = canonical.is_some_and(|canonical| {
                unique
                    .iter()
                    .any(|existing| self.imported_type_target(*existing) == Some(canonical))
            });
            if !already_present {
                unique.push(candidate);
            }
        }
        *candidates = unique;
    }

    fn unique_scoped_symbol(
        &self,
        scope: dotty_core::ScopeId,
        name: dotty_core::Name,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<Option<SymbolId>, TyperError> {
        let candidates = self.scoped_symbols(scope, name);
        self.unique_symbol_candidate(&candidates, name, tree_index, position)
    }

    fn scoped_symbols(&self, scope: dotty_core::ScopeId, name: dotty_core::Name) -> Vec<SymbolId> {
        let direct = self.store.scopes.get(scope).lookup_all(&name);
        if !direct.is_empty() {
            return direct.to_vec();
        }
        let alternate = dotty_core::Name::new(
            name.text(),
            match name.namespace() {
                dotty_core::Namespace::Term => dotty_core::Namespace::Type,
                dotty_core::Namespace::Type => dotty_core::Namespace::Term,
            },
        );
        self.store.scopes.get(scope).lookup_all(&alternate).to_vec()
    }

    fn lookup_term_candidate_for_type_name(
        &mut self,
        name: dotty_core::Name,
        context: SourceContextId,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<Option<SymbolId>, TyperError> {
        let term_name = dotty_core::Name::new(name.text(), dotty_core::Namespace::Term);
        let contexts = self.source_context_chain(context, tree_index)?;
        for context_id in &contexts {
            let source_context = self.index.source_context(*context_id);
            let candidates = self
                .store
                .scopes
                .get(source_context.lexical_scope)
                .lookup_all(&term_name);
            if !candidates.is_empty() {
                return self.unique_symbol_candidate(candidates, name, tree_index, position);
            }
        }
        for selection in [ImportSelection::Explicit, ImportSelection::Wildcard] {
            if let Some(symbol) = self.lookup_imports(
                &contexts,
                name,
                false,
                selection,
                SourceTreeLocation {
                    tree_index,
                    position,
                },
            )? && !matches!(
                self.store.symbols.get(symbol).kind,
                SymbolKind::Class
                    | SymbolKind::Trait
                    | SymbolKind::ModuleClass
                    | SymbolKind::TypeParameter
                    | SymbolKind::TypeAlias
            ) {
                return Ok(Some(symbol));
            }
        }
        Ok(None)
    }

    fn unique_type_candidate(
        &self,
        candidates: &[SymbolId],
        name: dotty_core::Name,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<Option<SymbolId>, TyperError> {
        let type_candidates: Vec<_> = candidates
            .iter()
            .copied()
            .filter(|symbol| {
                matches!(
                    self.store.symbols.get(*symbol).kind,
                    SymbolKind::Class
                        | SymbolKind::Trait
                        | SymbolKind::ModuleClass
                        | SymbolKind::TypeParameter
                        | SymbolKind::TypeAlias
                )
            })
            .collect();
        if type_candidates.is_empty() {
            match candidates {
                [symbol] => {
                    let kind = self.store.symbols.get(*symbol).kind;
                    Err(TyperError::WrongTypeNameKind {
                        source: self.source,
                        tree_index,
                        name,
                        symbol: *symbol,
                        kind,
                        position,
                    })
                }
                _ => self.unique_symbol_candidate(candidates, name, tree_index, position),
            }
        } else {
            self.unique_symbol_candidate(&type_candidates, name, tree_index, position)
        }
    }

    fn unique_symbol_candidate(
        &self,
        candidates: &[SymbolId],
        name: dotty_core::Name,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<Option<SymbolId>, TyperError> {
        match candidates {
            [] => Ok(None),
            [symbol] => Ok(Some(*symbol)),
            _ => Err(TyperError::AmbiguousTypeName {
                source: self.source,
                tree_index,
                name,
                position,
            }),
        }
    }

    fn scopes_of(&self, symbol: SymbolId) -> Vec<dotty_core::ScopeId> {
        let mut scopes = Vec::new();
        if let Some(scope) = self
            .packages
            .scope_of(symbol)
            .or_else(|| self.index.scope_of(symbol))
        {
            insert_scope(&mut scopes, scope);
        }
        if self.store.symbols.get(symbol).kind == SymbolKind::Package
            && let Some(package_scope) = scopes.first().copied()
        {
            let wrappers: Vec<_> = self
                .store
                .scopes
                .get(package_scope)
                .entered_symbols()
                .filter(|member| {
                    let wrapper = self.store.symbols.get(*member);
                    wrapper.kind == SymbolKind::ModuleClass
                        && wrapper.owner == Some(symbol)
                        && wrapper.origin == SymbolOrigin::Synthetic
                        && self
                            .store
                            .names
                            .resolve(wrapper.name.text())
                            .ends_with("$package$")
                })
                .collect();
            for wrapper in wrappers {
                if let Some(scope) = self.index.scope_of(wrapper) {
                    insert_scope(&mut scopes, scope);
                }
            }
        }
        if let SymbolInfo::Complete(ty) = self.store.symbols.get(symbol).info
            && let Type::ClassInfo(info) = self.store.types.get(ty)
        {
            insert_scope(&mut scopes, info.declarations);
        }
        let semantic = self.store.symbols.get(symbol);
        if semantic.kind != SymbolKind::Object {
            return scopes;
        }
        let Some(owner) = semantic.owner else {
            return scopes;
        };
        let Some(SourceDefinition::Canonical { source, tree }) = self.index.definition_of(symbol)
        else {
            return scopes;
        };
        if source == self.source
            && let Some(module_class) = self.index.derived_symbol_at(owner, source, tree)
            && self.store.symbols.get(module_class).kind == SymbolKind::ModuleClass
            && let Some(scope) = self.index.scope_of(module_class)
        {
            insert_scope(&mut scopes, scope);
        }
        scopes
    }

    /// Exposes the shared semantic store after the driver is no longer needed.
    pub fn store(&self) -> &SemanticStore {
        self.store
    }

    /// Typed expression trees produced by this typer instance.
    pub fn typed_ast(&self) -> &AstArena<Typed> {
        &self.typed_arena
    }

    /// Source-to-typed mappings produced by this typer instance.
    pub fn source_typed_index(&self) -> &SourceTypedIndex {
        &self.typed_index
    }

    /// Exposes the package registry used by this driver.
    pub fn packages(&self) -> &Packages {
        self.packages
    }
}

fn insert_scope(scopes: &mut Vec<dotty_core::ScopeId>, scope: dotty_core::ScopeId) {
    if !scopes.contains(&scope) {
        scopes.push(scope);
    }
}

fn tree_kind_name(kind: &TreeKind<Untyped>) -> &'static str {
    match kind {
        TreeKind::Ident(_) => "identifier",
        TreeKind::TypeTree(_) => "type tree",
        TreeKind::AppliedTypeTree(_) => "applied type tree",
        TreeKind::SingletonTypeTree(_) => "singleton type tree",
        TreeKind::RefinedTypeTree(_) => "refined type tree",
        TreeKind::LambdaTypeTree(_) => "lambda type tree",
        TreeKind::MatchTypeTree(_) => "match type tree",
        TreeKind::ByNameTypeTree(_) => "by-name type tree",
        TreeKind::TypeBoundsTree(_) => "type bounds tree",
        _ => "expression or declaration tree",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::{
        Name, Namespace, SourceText, SymbolFlags, SymbolLinks, SymbolOrigin, TermName, Visibility,
    };
    use dotty_lexer::ContextualScanner;
    use dotty_namer::name_compilation_unit;
    use std::{cell::RefCell, rc::Rc};

    struct ScriptedResolver {
        member: Option<SymbolId>,
        package: Option<SymbolId>,
        member_requests: Rc<RefCell<Vec<dotty_core::Name>>>,
        package_requests: Rc<RefCell<Vec<Vec<String>>>>,
    }

    impl SymbolResolver for ScriptedResolver {
        fn resolve_member(
            &mut self,
            _store: &SemanticStore,
            request: &MemberRequest,
        ) -> Result<Option<SymbolId>, ResolutionError> {
            self.member_requests.borrow_mut().push(request.name);
            Ok(self.member)
        }

        fn resolve_package(
            &mut self,
            _store: &SemanticStore,
            path: &[&str],
        ) -> Result<Option<SymbolId>, ResolutionError> {
            self.package_requests
                .borrow_mut()
                .push(path.iter().map(|segment| (*segment).to_owned()).collect());
            Ok(self.package)
        }
    }

    fn setup() -> (AstArena<Untyped>, SemanticStore, Packages, Definitions) {
        let arena = AstArena::new();
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        (arena, store, Packages::new(), definitions)
    }

    fn symbol(store: &mut SemanticStore, kind: SymbolKind, info: SymbolInfo) -> SymbolId {
        let name = store.names.intern("member");
        store.symbols.alloc(dotty_core::Symbol {
            name: Name::new(name, Namespace::Term),
            owner: None,
            kind,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        })
    }

    fn parse_and_name(
        text: &str,
    ) -> (
        dotty_parser::ParseResult,
        SemanticStore,
        Packages,
        Definitions,
        SourceSemanticIndex,
        SourceId,
    ) {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let source = SourceId::from_index(11);
        let mut packages = Packages::new();
        let (parsed, index) = parse_and_name_unit(text, source, &mut store, &mut packages);
        (parsed, store, packages, definitions, index, source)
    }

    fn parse_and_name_unit(
        text: &str,
        source: SourceId,
        store: &mut SemanticStore,
        packages: &mut Packages,
    ) -> (dotty_parser::ParseResult, SourceSemanticIndex) {
        let scanner = ContextualScanner::new(text).unwrap();
        let parsed = dotty_parser::parse_compilation_unit(
            SourceText::new(text).unwrap(),
            source,
            scanner,
            &mut store.names,
        );
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let index = name_compilation_unit(
            &parsed.ast,
            parsed.root,
            source,
            "Typer.scala",
            store,
            packages,
        )
        .unwrap();
        (parsed, index)
    }

    fn val_symbol(
        parsed: &dotty_parser::ParseResult,
        store: &SemanticStore,
        index: &SourceSemanticIndex,
        source: SourceId,
        target: &str,
    ) -> (SymbolId, TreeId<Untyped>) {
        for (tree, node) in parsed.ast.iter() {
            if let TreeKind::ValDef(definition) = &node.kind
                && store.names.resolve(definition.name.as_name().text()) == target
            {
                return (index.symbol_at(source, tree).unwrap(), definition.tpt);
            }
        }
        panic!("source val `{target}` not found");
    }

    fn val_definition_and_rhs(
        parsed: &dotty_parser::ParseResult,
        store: &SemanticStore,
        index: &SourceSemanticIndex,
        source: SourceId,
        target: &str,
    ) -> (SymbolId, TreeId<Untyped>, TreeId<Untyped>) {
        for (tree, node) in parsed.ast.iter() {
            if let TreeKind::ValDef(definition) = &node.kind
                && store.names.resolve(definition.name.as_name().text()) == target
            {
                return (
                    index.symbol_at(source, tree).unwrap(),
                    tree,
                    definition.rhs.expect("test val should have an rhs"),
                );
            }
        }
        panic!("source val `{target}` not found");
    }

    fn method_definition_and_rhs(
        parsed: &dotty_parser::ParseResult,
        store: &SemanticStore,
        index: &SourceSemanticIndex,
        source: SourceId,
        target: &str,
    ) -> (SymbolId, TreeId<Untyped>) {
        for (tree, node) in parsed.ast.iter() {
            if let TreeKind::DefDef(definition) = &node.kind
                && store.names.resolve(definition.name.as_name().text()) == target
            {
                return (
                    index.symbol_at(source, tree).unwrap(),
                    definition.rhs.expect("test method should have an rhs"),
                );
            }
        }
        panic!("source method `{target}` not found");
    }

    fn type_value_rhs(source_text: &str) -> (dotty_core::Constant, Type, TypeId, Definitions) {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (symbol, _, rhs) = val_definition_and_rhs(&parsed, &store, &index, source, "value");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(symbol).unwrap(),
            owner: store.symbols.get(symbol).owner.unwrap(),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let typed = typer.type_expression(rhs, context).unwrap();
        let TreeKind::Literal(literal) = &typer.typed_ast().get(typed).kind else {
            panic!("expected a typed literal node")
        };
        let value = literal.value.clone();
        let own_type = typer
            .store()
            .types
            .get(typer.typed_ast().get(typed).ty)
            .clone();
        let widened = typer
            .widen_expression_type(typer.typed_ast().get(typed).ty)
            .unwrap();
        (value, own_type, widened, definitions)
    }

    fn type_synthetic_literal(
        value: dotty_core::Constant,
    ) -> (dotty_core::Constant, Type, TypeId, Definitions) {
        let (mut parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value = 0 }");
        let (symbol, _, rhs) = val_definition_and_rhs(&parsed, &store, &index, source, "value");
        parsed.ast.get_mut(rhs).kind = TreeKind::Literal(dotty_core::ast::Literal {
            value: value.clone(),
        });
        let context = ExpressionContext {
            lexical: index.declaration_context_of(symbol).unwrap(),
            owner: store.symbols.get(symbol).owner.unwrap(),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let typed = typer.type_expression(rhs, context).unwrap();
        let TreeKind::Literal(literal) = &typer.typed_ast().get(typed).kind else {
            panic!("expected a typed literal node")
        };
        let literal_value = literal.value.clone();
        let own_type = typer
            .store()
            .types
            .get(typer.typed_ast().get(typed).ty)
            .clone();
        let widened = typer
            .widen_expression_type(typer.typed_ast().get(typed).ty)
            .unwrap();
        (literal_value, own_type, widened, definitions)
    }

    fn type_method_rhs(
        source_text: &str,
        method_name: &str,
    ) -> Result<(TreeKind<Typed>, Type, TypeId, Type, Definitions), TyperError> {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, method_name);
        let parameter_context = parsed.ast.iter().find_map(|(tree, node)| {
            let TreeKind::ValDef(definition) = &node.kind else {
                return None;
            };
            if store.names.resolve(definition.name.as_name().text()) == method_name {
                return None;
            }
            let symbol = index.symbol_at(source, tree)?;
            (store.symbols.get(symbol).kind == SymbolKind::Parameter
                && store.symbols.get(symbol).owner == Some(method))
            .then(|| index.declaration_context_of(symbol))
            .flatten()
        });
        let context = ExpressionContext {
            lexical: parameter_context.unwrap_or_else(|| {
                index
                    .declaration_context_of(method)
                    .expect("test method should have a declaration context")
            }),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let typed = typer.type_expression(rhs, context)?;
        let node = typer.typed_ast().get(typed);
        let kind = node.kind.clone();
        let own_type_id = node.ty;
        let widened_id = typer.widen_expression_type(own_type_id)?;
        Ok((
            kind,
            typer.store().types.get(own_type_id).clone(),
            widened_id,
            typer.store().types.get(widened_id).clone(),
            definitions,
        ))
    }

    fn type_symbol(store: &SemanticStore, ty: TypeId) -> SymbolId {
        match store.types.get(ty) {
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("expected a symbol TypeRef, got {other:?}"),
        }
    }

    fn applied_class_type(
        store: &mut SemanticStore,
        definitions: Definitions,
        class: SymbolId,
        arguments: &[TypeId],
    ) -> TypeId {
        applied_class_type_with_prefix(store, class, definitions.no_prefix, arguments)
    }

    fn applied_class_type_with_prefix(
        store: &mut SemanticStore,
        class: SymbolId,
        prefix: TypeId,
        arguments: &[TypeId],
    ) -> TypeId {
        let tycon = store.types.alloc(Type::type_ref(prefix, class));
        store.types.alloc(Type::Applied {
            tycon,
            args: arguments.to_vec(),
        })
    }

    fn parent_prefix_for(
        store: &SemanticStore,
        class: SymbolId,
        expected_parent: SymbolId,
    ) -> TypeId {
        let SymbolInfo::Complete(info) = *store.symbols.info(class) else {
            panic!("class info for {class:?} is not complete");
        };
        let Type::ClassInfo(info) = store.types.get(info) else {
            panic!("class info for {class:?} does not reference ClassInfo");
        };
        let Some(parent) = info.parents.iter().find(|parent| {
            let tycon = match store.types.get(**parent) {
                Type::Applied { tycon, .. } => *tycon,
                _ => **parent,
            };
            matches!(
                store.types.get(tycon),
                Type::TypeRef {
                    target: TypeRefTarget::Symbol(symbol),
                    ..
                } if *symbol == expected_parent
            )
        }) else {
            panic!("class {class:?} has no parent view");
        };
        let tycon = match store.types.get(*parent) {
            Type::Applied { tycon, .. } => *tycon,
            _ => *parent,
        };
        match store.types.get(tycon) {
            Type::TypeRef { prefix, .. } => *prefix,
            other => panic!("parent view has unexpected type constructor {other:?}"),
        }
    }

    fn nominal_type_ref(
        store: &mut SemanticStore,
        definitions: Definitions,
        class: SymbolId,
    ) -> TypeId {
        nominal_type_ref_with_prefix(store, class, definitions.no_prefix)
    }

    fn nominal_type_ref_with_prefix(
        store: &mut SemanticStore,
        class: SymbolId,
        prefix: TypeId,
    ) -> TypeId {
        store.types.alloc(Type::type_ref(prefix, class))
    }

    fn candidate_for(
        typer: &mut SourceTyper<'_>,
        receiver: TypeId,
        class: SymbolId,
        name: Name,
    ) -> MemberCandidate {
        typer
            .lookup_members(receiver, name)
            .unwrap()
            .into_iter()
            .find(|candidate| candidate.declaring_class == class)
            .unwrap()
    }

    fn type_parameter_symbol(
        parsed: &dotty_parser::ParseResult,
        store: &SemanticStore,
        index: &SourceSemanticIndex,
        source: SourceId,
        name: &str,
    ) -> SymbolId {
        parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == name
                        && index.symbol_at(source, tree).is_some_and(|symbol| {
                            store.symbols.get(symbol).kind == SymbolKind::TypeParameter
                        }) =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("source type parameter `{name}` not found"))
    }

    fn class_symbol(
        parsed: &dotty_parser::ParseResult,
        store: &SemanticStore,
        index: &SourceSemanticIndex,
        source: SourceId,
        name: &str,
    ) -> SymbolId {
        parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == name =>
                {
                    index.symbol_at(source, tree).filter(|symbol| {
                        matches!(
                            store.symbols.get(*symbol).kind,
                            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
                        )
                    })
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("source class-like symbol `{name}` not found"))
    }

    fn type_alias_symbol(
        parsed: &dotty_parser::ParseResult,
        store: &SemanticStore,
        index: &SourceSemanticIndex,
        source: SourceId,
        name: &str,
    ) -> SymbolId {
        parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == name
                        && index.symbol_at(source, tree).is_some_and(|symbol| {
                            store.symbols.get(symbol).kind == SymbolKind::TypeAlias
                        }) =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("source type alias `{name}` not found"))
    }

    fn complete_builtin_annotation(name: &str) -> (TypeId, Definitions) {
        let text = format!("val x: {name} = 1");
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(&text);
        let (symbol, _) = val_symbol(&parsed, &store, &index, source, "x");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let ty = typer.complete_symbol(symbol).unwrap();
        (ty, definitions)
    }

    #[test]
    fn constructor_uses_caller_bootstrapped_definitions() {
        let (arena, mut store, packages, definitions) = setup();
        let index = SourceSemanticIndex::new();
        let source = SourceId::from_index(0);

        let typer = SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages);

        assert_eq!(typer.definitions.int, definitions.int);
    }

    #[test]
    fn subtype_relation_is_reflexive_for_canonical_primitive_types() {
        let (arena, mut store, packages, definitions) = setup();
        let index = SourceSemanticIndex::new();
        let source = SourceId::from_index(0);
        let mut typer =
            SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages);

        assert!(typer.is_subtype(definitions.int, definitions.int).unwrap());
        assert!(typer.conforms(definitions.int, definitions.int).unwrap());
    }

    #[test]
    fn distinct_builtin_primitive_types_do_not_conform() {
        let (arena, mut store, packages, definitions) = setup();
        let index = SourceSemanticIndex::new();
        let source = SourceId::from_index(0);
        let mut typer =
            SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages);

        assert!(
            !typer
                .conforms(definitions.boolean, definitions.int)
                .unwrap()
        );
    }

    #[test]
    fn semantic_type_refs_equivalent_across_distinct_type_ids() {
        let (arena, mut store, packages, definitions) = setup();
        let symbol = type_symbol(&store, definitions.int);
        let prefix = store.types.alloc(Type::NoPrefix);
        let duplicate = store.types.alloc(Type::type_ref(prefix, symbol));
        assert_ne!(definitions.int, duplicate);
        let index = SourceSemanticIndex::new();
        let source = SourceId::from_index(0);
        let mut typer =
            SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages);

        assert!(typer.is_subtype(definitions.int, duplicate).unwrap());
    }

    #[test]
    fn same_class_references_with_distinct_prefixes_are_not_equivalent() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C; class First; class Second");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let first = class_symbol(&parsed, &store, &index, source, "First");
        let second = class_symbol(&parsed, &store, &index, source, "Second");
        let first_prefix = store.types.alloc(Type::ThisType { class: first });
        let second_prefix = store.types.alloc(Type::ThisType { class: second });
        let first_reference = store.types.alloc(Type::type_ref(first_prefix, class));
        let second_reference = store.types.alloc(Type::type_ref(second_prefix, class));
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(!typer.is_subtype(first_reference, second_reference).unwrap());
    }

    #[test]
    fn bottom_and_top_rules_use_canonical_definitions() {
        let (arena, mut store, packages, definitions) = setup();
        let index = SourceSemanticIndex::new();
        let source = SourceId::from_index(0);
        let mut typer =
            SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages);

        assert!(
            typer
                .is_subtype(definitions.nothing_type, definitions.int)
                .unwrap()
        );
        assert!(
            typer
                .is_subtype(definitions.int, definitions.any_type)
                .unwrap()
        );
    }

    #[test]
    fn unrelated_source_classes_do_not_conform_without_inheritance() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class A; class B");
        let a = class_symbol(&parsed, &store, &index, source, "A");
        let b = class_symbol(&parsed, &store, &index, source, "B");
        let a_type = nominal_type_ref(&mut store, definitions, a);
        let b_type = nominal_type_ref(&mut store, definitions, b);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        typer.complete_symbol(a).unwrap();
        typer.complete_symbol(b).unwrap();
        assert!(!typer.is_subtype(a_type, b_type).unwrap());
    }

    #[test]
    fn source_class_conforms_to_direct_parent_after_explicit_completion() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Parent; class Child extends Parent");
        let parent = class_symbol(&parsed, &store, &index, source, "Parent");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let child_type = nominal_type_ref(&mut store, definitions, child);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        typer.complete_symbol(child).unwrap();
        typer.complete_symbol(parent).unwrap();
        let parent_prefix = parent_prefix_for(typer.store, child, parent);
        let parent_type = nominal_type_ref_with_prefix(typer.store, parent, parent_prefix);
        assert!(typer.is_subtype(child_type, parent_type).unwrap());
    }

    #[test]
    fn source_class_conforms_to_trait_parent() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("trait Parent; class Child extends Parent");
        let parent = class_symbol(&parsed, &store, &index, source, "Parent");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let child_type = nominal_type_ref(&mut store, definitions, child);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        typer.complete_symbol(child).unwrap();
        typer.complete_symbol(parent).unwrap();
        let parent_prefix = parent_prefix_for(typer.store, child, parent);
        let parent_type = nominal_type_ref_with_prefix(typer.store, parent, parent_prefix);
        assert!(typer.is_subtype(child_type, parent_type).unwrap());
    }

    #[test]
    fn source_class_conforms_to_multilevel_parent() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Grandparent; class Parent extends Grandparent; class Child extends Parent",
        );
        let grandparent = class_symbol(&parsed, &store, &index, source, "Grandparent");
        let parent = class_symbol(&parsed, &store, &index, source, "Parent");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let child_type = nominal_type_ref(&mut store, definitions, child);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        typer.complete_symbol(child).unwrap();
        typer.complete_symbol(parent).unwrap();
        typer.complete_symbol(grandparent).unwrap();
        let grandparent_prefix = parent_prefix_for(typer.store, parent, grandparent);
        let grandparent_type =
            nominal_type_ref_with_prefix(typer.store, grandparent, grandparent_prefix);
        assert!(typer.is_subtype(child_type, grandparent_type).unwrap());
    }

    #[test]
    fn instantiated_parent_view_preserves_generic_arguments() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Parent[A]; class Child[B] extends Parent[B]");
        let parent = class_symbol(&parsed, &store, &index, source, "Parent");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let child_type = applied_class_type(&mut store, definitions, child, &[definitions.int]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        typer.complete_symbol(child).unwrap();
        typer.complete_symbol(parent).unwrap();
        let parent_prefix = parent_prefix_for(typer.store, child, parent);
        let parent_type =
            applied_class_type_with_prefix(typer.store, parent, parent_prefix, &[definitions.int]);
        let wrong_parent_type = applied_class_type_with_prefix(
            typer.store,
            parent,
            parent_prefix,
            &[definitions.boolean],
        );
        assert!(typer.is_subtype(child_type, parent_type).unwrap());
        assert!(!typer.is_subtype(child_type, wrong_parent_type).unwrap());
    }

    #[test]
    fn missing_class_info_is_reported_without_implicit_completion() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class A; class B");
        let a = class_symbol(&parsed, &store, &index, source, "A");
        let b = class_symbol(&parsed, &store, &index, source, "B");
        let a_type = nominal_type_ref(&mut store, definitions, a);
        let b_type = nominal_type_ref(&mut store, definitions, b);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.is_subtype(a_type, b_type),
            Err(TypeRelationError::ClassInfoUnavailable {
                symbol,
                state: crate::types::SymbolInfoState::Missing,
            }) if symbol == a
        ));
        assert!(matches!(*typer.store.symbols.info(a), SymbolInfo::Missing));
        assert!(matches!(*typer.store.symbols.info(b), SymbolInfo::Missing));
    }

    #[test]
    fn inheritance_cycles_are_reported_for_unrelated_targets() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class A extends B; class B extends A; class C");
        let a = class_symbol(&parsed, &store, &index, source, "A");
        let b = class_symbol(&parsed, &store, &index, source, "B");
        let c = class_symbol(&parsed, &store, &index, source, "C");
        let a_type = nominal_type_ref(&mut store, definitions, a);
        let c_type = nominal_type_ref(&mut store, definitions, c);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        typer.complete_symbol(a).unwrap();
        typer.complete_symbol(b).unwrap();
        typer.complete_symbol(c).unwrap();
        assert!(matches!(
            typer.is_subtype(a_type, c_type),
            Err(TypeRelationError::InheritanceCycle { symbol }) if symbol == a
        ));
    }

    #[test]
    fn matching_parent_does_not_hide_a_cycle_closing_through_that_parent() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class A extends B; class B extends A");
        let a = class_symbol(&parsed, &store, &index, source, "A");
        let b = class_symbol(&parsed, &store, &index, source, "B");
        let b_type = nominal_type_ref(&mut store, definitions, b);
        let a_type = nominal_type_ref(&mut store, definitions, a);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        typer.complete_symbol(a).unwrap();
        typer.complete_symbol(b).unwrap();
        assert!(matches!(
            typer.is_subtype(b_type, a_type),
            Err(TypeRelationError::InheritanceCycle { symbol }) if symbol == b
        ));
    }

    #[test]
    fn cycle_across_deduplicated_parent_branches_is_reported() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class A extends B, C; class B extends C; class C extends B; class Unrelated",
        );
        let a = class_symbol(&parsed, &store, &index, source, "A");
        let b = class_symbol(&parsed, &store, &index, source, "B");
        let c = class_symbol(&parsed, &store, &index, source, "C");
        let unrelated = class_symbol(&parsed, &store, &index, source, "Unrelated");
        let a_type = nominal_type_ref(&mut store, definitions, a);
        let unrelated_type = nominal_type_ref(&mut store, definitions, unrelated);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        typer.complete_symbol(a).unwrap();
        typer.complete_symbol(b).unwrap();
        typer.complete_symbol(c).unwrap();
        typer.complete_symbol(unrelated).unwrap();
        assert!(matches!(
            typer.is_subtype(a_type, unrelated_type),
            Err(TypeRelationError::InheritanceCycle { symbol }) if symbol == b || symbol == c
        ));
    }

    #[test]
    fn this_type_conforms_to_its_own_class_reference() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("class C");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let this = store.types.alloc(Type::ThisType { class });
        let class_type = nominal_type_ref(&mut store, definitions, class);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(typer.is_subtype(this, class_type).unwrap());
    }

    #[test]
    fn this_type_conforms_to_its_class_with_a_canonical_package_prefix() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("package p { class C; val value: C = 1 }");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "value");
        let this = store.types.alloc(Type::ThisType { class });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let class_type = typer.complete_symbol(value).unwrap();
        assert!(typer.is_subtype(this, class_type).unwrap());
    }

    #[test]
    fn nested_this_type_does_not_conform_through_a_different_outer_prefix() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Outer { class Inner }; class Other");
        let outer = class_symbol(&parsed, &store, &index, source, "Outer");
        let other = class_symbol(&parsed, &store, &index, source, "Other");
        let inner = class_symbol(&parsed, &store, &index, source, "Inner");
        let this_inner = store.types.alloc(Type::ThisType { class: inner });
        let other_prefix = store.types.alloc(Type::ThisType { class: other });
        let inner_through_other = store.types.alloc(Type::type_ref(other_prefix, inner));
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(!typer.is_subtype(this_inner, inner_through_other).unwrap());
        assert_ne!(typer.store.symbols.get(inner).owner, Some(other));
        assert_eq!(typer.store.symbols.get(inner).owner, Some(outer));
    }

    #[test]
    fn nested_this_type_conforms_through_its_canonical_owner_prefix() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Outer { class Inner; val value: Inner = null }");
        let outer = class_symbol(&parsed, &store, &index, source, "Outer");
        let inner = class_symbol(&parsed, &store, &index, source, "Inner");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "value");
        let this_inner = store.types.alloc(Type::ThisType { class: inner });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let canonical_inner = typer.complete_symbol(value).unwrap();
        let Type::TypeRef { prefix, .. } = typer.store().types.get(canonical_inner) else {
            panic!("expected a qualified nested class reference")
        };
        assert_eq!(
            typer.store().types.get(*prefix),
            &Type::ThisType { class: outer }
        );
        assert!(typer.is_subtype(this_inner, canonical_inner).unwrap());
    }

    #[test]
    fn class_reference_does_not_conform_to_its_this_type() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("class C");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let this = store.types.alloc(Type::ThisType { class });
        let class_type = nominal_type_ref(&mut store, definitions, class);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(!typer.is_subtype(class_type, this).unwrap());
    }

    #[test]
    fn invariant_applied_types_accept_equivalent_arguments_only() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A]");
        let box_class = class_symbol(&parsed, &store, &index, source, "Box");
        let int_box = applied_class_type(&mut store, definitions, box_class, &[definitions.int]);
        let same_int_box =
            applied_class_type(&mut store, definitions, box_class, &[definitions.int]);
        let any_box =
            applied_class_type(&mut store, definitions, box_class, &[definitions.any_type]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(typer.is_subtype(int_box, same_int_box).unwrap());
        assert!(!typer.is_subtype(int_box, any_box).unwrap());
    }

    #[test]
    fn external_applied_generic_subtyping_is_explicitly_deferred() {
        let (arena, mut store, packages, definitions) = setup();
        let external = symbol(&mut store, SymbolKind::Class, SymbolInfo::Missing);
        let applied = applied_class_type(&mut store, definitions, external, &[definitions.int]);
        let index = SourceSemanticIndex::new();
        let source = SourceId::from_index(0);
        let mut typer =
            SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages);

        assert!(matches!(
            typer.is_subtype(applied, definitions.int),
            Err(TypeRelationError::ExternalGenericInstantiationDeferred { class })
                if class == external
        ));
    }

    #[test]
    fn invariant_applied_types_reject_malformed_arity_explicitly() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A]");
        let box_class = class_symbol(&parsed, &store, &index, source, "Box");
        let int_box = applied_class_type(&mut store, definitions, box_class, &[definitions.int]);
        let raw_box = applied_class_type(&mut store, definitions, box_class, &[]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.is_subtype(int_box, raw_box),
            Err(TypeRelationError::UnsupportedType { .. })
        ));
    }

    #[test]
    fn unsupported_union_relations_return_an_explicit_error() {
        let (arena, mut store, packages, definitions) = setup();
        let union = store.types.alloc(Type::Or {
            left: definitions.int,
            right: definitions.boolean,
        });
        let index = SourceSemanticIndex::new();
        let source = SourceId::from_index(0);
        let mut typer =
            SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages);

        assert!(matches!(
            typer.is_subtype(union, definitions.int),
            Err(TypeRelationError::UnsupportedType { found, expected })
                if found == union && expected == definitions.int
        ));
    }

    #[test]
    fn non_equivalent_by_name_relations_are_explicitly_unsupported() {
        let (arena, mut store, packages, definitions) = setup();
        let int_by_name = store.types.alloc(Type::ByName {
            result: definitions.int,
        });
        let boolean_by_name = store.types.alloc(Type::ByName {
            result: definitions.boolean,
        });
        let index = SourceSemanticIndex::new();
        let source = SourceId::from_index(0);
        let mut typer =
            SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages);

        assert!(matches!(
            typer.is_subtype(int_by_name, boolean_by_name),
            Err(TypeRelationError::UnsupportedType { .. })
        ));
        assert!(matches!(
            typer.is_subtype(int_by_name, definitions.any_type),
            Err(TypeRelationError::UnsupportedType { .. })
        ));
    }

    #[test]
    fn empty_class_publishes_class_info_with_its_existing_scope_and_object_parent() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("class C");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let scope = index.scope_of(class).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info_id = typer.complete_symbol(class).unwrap();

        let Type::ClassInfo(info) = typer.store().types.get(info_id) else {
            panic!("expected ClassInfo");
        };
        assert_eq!(info.class, class);
        assert_eq!(info.prefix, definitions.no_prefix);
        assert_eq!(info.declarations, scope);
        assert_eq!(info.parents, vec![definitions.object_type]);
        assert_eq!(info.self_type, None);
    }

    #[test]
    fn member_lookup_completes_a_missing_class_from_the_current_source() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value: Int = 1 }");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        assert_eq!(*store.symbols.info(class), SymbolInfo::Missing);
        let name = Name::new(store.names.intern("value"), Namespace::Term);
        let scope = index.scope_of(class).unwrap();
        let field = store.scopes.get(scope).lookup(&name).unwrap();
        let receiver = store.types.alloc(Type::TypeRef {
            prefix: definitions.no_prefix,
            target: TypeRefTarget::Symbol(class),
        });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidates = typer.lookup_members(receiver, name).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].symbol, field);
        assert_eq!(candidates[0].declaring_class, class);
        assert!(matches!(
            *typer.store().symbols.info(class),
            SymbolInfo::Complete(_)
        ));
    }

    #[test]
    fn generic_method_result_is_adapted_to_the_receiver_argument() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A] { def get: A = null }; class Text");
        let box_class = class_symbol(&parsed, &store, &index, source, "Box");
        let text_class = class_symbol(&parsed, &store, &index, source, "Text");
        let canonical_parameter = type_parameter_symbol(&parsed, &store, &index, source, "A");
        assert_eq!(
            store.symbols.get(canonical_parameter).owner,
            Some(box_class)
        );
        let (class_tree, template_tree) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Box" =>
                {
                    Some((tree, definition.rhs))
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(index.symbol_at(source, class_tree), Some(box_class));
        let TreeKind::Template(template) = &parsed.ast.get(template_tree).kind else {
            panic!("expected the generic class template")
        };
        let TreeKind::DefDef(constructor) = &parsed.ast.get(template.constructor).kind else {
            panic!("expected the synthetic primary constructor")
        };
        let constructor_symbol = index.symbol_at(source, template.constructor).unwrap();
        let constructor_parameter = index
            .derived_symbol_at(constructor_symbol, source, constructor.type_params[0])
            .unwrap();
        assert_ne!(constructor_parameter, canonical_parameter);
        let name = Name::new(store.names.intern("get"), Namespace::Term);
        let get = store
            .scopes
            .get(index.scope_of(box_class).unwrap())
            .lookup(&name)
            .unwrap();
        let text_ty = nominal_type_ref(&mut store, definitions, text_class);
        let receiver = applied_class_type(&mut store, definitions, box_class, &[text_ty]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidate = candidate_for(&mut typer, receiver, box_class, name);
        let adapted = typer.member_type_on(&candidate).unwrap();

        assert_eq!(type_symbol(typer.store(), adapted), text_class);
        let SymbolInfo::Complete(declaration) = *typer.store().symbols.info(get) else {
            panic!("member signature should be completed")
        };
        assert_eq!(type_symbol(typer.store(), declaration), canonical_parameter);
    }

    #[test]
    fn generic_field_type_is_adapted_without_changing_its_declaration() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A] { val value: A = null }; class Text");
        let box_class = class_symbol(&parsed, &store, &index, source, "Box");
        let text_class = class_symbol(&parsed, &store, &index, source, "Text");
        let parameter = type_parameter_symbol(&parsed, &store, &index, source, "A");
        let name = Name::new(store.names.intern("value"), Namespace::Term);
        let value = store
            .scopes
            .get(index.scope_of(box_class).unwrap())
            .lookup(&name)
            .unwrap();
        let text_ty = nominal_type_ref(&mut store, definitions, text_class);
        let receiver = applied_class_type(&mut store, definitions, box_class, &[text_ty]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidate = candidate_for(&mut typer, receiver, box_class, name);
        let adapted = typer.member_type_on(&candidate).unwrap();

        assert_eq!(type_symbol(typer.store(), adapted), text_class);
        let SymbolInfo::Complete(declaration) = *typer.store().symbols.info(value) else {
            panic!("field signature should be completed")
        };
        assert_eq!(type_symbol(typer.store(), declaration), parameter);
        assert_ne!(adapted, declaration);
    }

    #[test]
    fn generic_member_arguments_follow_primary_constructor_source_order() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Pair[A, B] { val second: B = null }; class Text; class NumberText",
        );
        let pair = class_symbol(&parsed, &store, &index, source, "Pair");
        let text = class_symbol(&parsed, &store, &index, source, "Text");
        let number_text = class_symbol(&parsed, &store, &index, source, "NumberText");
        let b = type_parameter_symbol(&parsed, &store, &index, source, "B");
        assert_eq!(store.symbols.get(b).owner, Some(pair));
        let name = Name::new(store.names.intern("second"), Namespace::Term);
        let text_ty = nominal_type_ref(&mut store, definitions, text);
        let number_ty = nominal_type_ref(&mut store, definitions, number_text);
        let receiver = applied_class_type(&mut store, definitions, pair, &[text_ty, number_ty]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidate = candidate_for(&mut typer, receiver, pair, name);
        let adapted = typer.member_type_on(&candidate).unwrap();

        assert_eq!(type_symbol(typer.store(), adapted), number_text);
    }

    #[test]
    fn nested_applied_member_types_are_substituted_recursively() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Box[A] { val values: List[A] = null }; class List[X]; class Text",
        );
        let box_class = class_symbol(&parsed, &store, &index, source, "Box");
        let list_class = class_symbol(&parsed, &store, &index, source, "List");
        let text_class = class_symbol(&parsed, &store, &index, source, "Text");
        let name = Name::new(store.names.intern("values"), Namespace::Term);
        let text_ty = nominal_type_ref(&mut store, definitions, text_class);
        let receiver = applied_class_type(&mut store, definitions, box_class, &[text_ty]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidate = candidate_for(&mut typer, receiver, box_class, name);
        let adapted = typer.member_type_on(&candidate).unwrap();

        let Type::Applied { tycon, args } = typer.store().types.get(adapted) else {
            panic!("expected the substituted List application")
        };
        assert_eq!(type_symbol(typer.store(), *tycon), list_class);
        assert_eq!(args.len(), 1);
        assert_eq!(type_symbol(typer.store(), args[0]), text_class);
    }

    #[test]
    fn class_substitution_keeps_a_generic_methods_own_binder_intact() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A] { def convert[B](value: B): A = value }; class Text");
        let box_class = class_symbol(&parsed, &store, &index, source, "Box");
        let text_class = class_symbol(&parsed, &store, &index, source, "Text");
        let name = Name::new(store.names.intern("convert"), Namespace::Term);
        let text_ty = nominal_type_ref(&mut store, definitions, text_class);
        let receiver = applied_class_type(&mut store, definitions, box_class, &[text_ty]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidate = candidate_for(&mut typer, receiver, box_class, name);
        let adapted = typer.member_type_on(&candidate).unwrap();

        let Type::Poly(poly) = typer.store().types.get(adapted) else {
            panic!("expected the method's type-parameter binder")
        };
        let Type::Method(method) = typer.store().types.get(poly.result) else {
            panic!("expected the method parameter clause")
        };
        let Type::ParamRef {
            binder: parameter_binder,
            index: 0,
        } = typer.store().types.get(method.params[0].ty)
        else {
            panic!("method-owned B must remain a ParamRef")
        };
        assert_eq!(*parameter_binder, adapted);
        assert_eq!(type_symbol(typer.store(), method.result), text_class);
    }

    #[test]
    fn generic_source_receivers_must_supply_exact_arity() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Pair[A, B] { val second: B = null }; class Text");
        let pair = class_symbol(&parsed, &store, &index, source, "Pair");
        let text = class_symbol(&parsed, &store, &index, source, "Text");
        let name = Name::new(store.names.intern("second"), Namespace::Term);
        let text_ty = nominal_type_ref(&mut store, definitions, text);
        let receiver = applied_class_type(&mut store, definitions, pair, &[text_ty]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidate = candidate_for(&mut typer, receiver, pair, name);
        assert!(matches!(
            typer.member_type_on(&candidate),
            Err(TyperError::ReceiverGenericArityMismatch {
                class,
                expected: 2,
                actual: 1,
            }) if class == pair
        ));
    }

    #[test]
    fn raw_source_generic_receivers_are_rejected_explicitly() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A] { val value: A = null }");
        let box_class = class_symbol(&parsed, &store, &index, source, "Box");
        let name = Name::new(store.names.intern("value"), Namespace::Term);
        let receiver = nominal_type_ref(&mut store, definitions, box_class);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidate = candidate_for(&mut typer, receiver, box_class, name);
        assert!(matches!(
            typer.member_type_on(&candidate),
            Err(TyperError::RawGenericSourceReceiverUnsupported {
                class,
                expected: 1,
            }) if class == box_class
        ));
    }

    #[test]
    fn by_name_member_parameters_are_substituted() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A] { def consume(value: => A): A = value }; class Text");
        let box_class = class_symbol(&parsed, &store, &index, source, "Box");
        let text = class_symbol(&parsed, &store, &index, source, "Text");
        let name = Name::new(store.names.intern("consume"), Namespace::Term);
        let text_ty = nominal_type_ref(&mut store, definitions, text);
        let receiver = applied_class_type(&mut store, definitions, box_class, &[text_ty]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidate = candidate_for(&mut typer, receiver, box_class, name);
        let adapted = typer.member_type_on(&candidate).unwrap();
        let Type::Method(method) = typer.store().types.get(adapted) else {
            panic!("expected a term method")
        };
        let Type::ByName { result } = typer.store().types.get(method.params[0].ty) else {
            panic!("expected a by-name parameter")
        };
        assert_eq!(type_symbol(typer.store(), *result), text);
        assert_eq!(type_symbol(typer.store(), method.result), text);
    }

    #[test]
    fn member_type_bounds_are_substituted() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A] { type T >: A <: A }; class Text");
        let box_class = class_symbol(&parsed, &store, &index, source, "Box");
        let text = class_symbol(&parsed, &store, &index, source, "Text");
        let name = Name::new(store.names.intern("T"), Namespace::Type);
        let text_ty = nominal_type_ref(&mut store, definitions, text);
        let receiver = applied_class_type(&mut store, definitions, box_class, &[text_ty]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidate = candidate_for(&mut typer, receiver, box_class, name);
        let adapted = typer.member_type_on(&candidate).unwrap();
        let Type::Bounds { low, high } = typer.store().types.get(adapted) else {
            panic!("expected member type bounds")
        };
        assert_eq!(type_symbol(typer.store(), *low), text);
        assert_eq!(type_symbol(typer.store(), *high), text);
    }

    #[test]
    fn external_applied_generic_member_adaptation_is_deferred() {
        let (arena, mut store, packages, definitions) = setup();
        let external_class = symbol(&mut store, SymbolKind::Class, SymbolInfo::Missing);
        let external_member = symbol(
            &mut store,
            SymbolKind::Method,
            SymbolInfo::Complete(definitions.int),
        );
        let receiver =
            applied_class_type(&mut store, definitions, external_class, &[definitions.int]);
        let candidate = MemberCandidate {
            symbol: external_member,
            declaring_class: external_class,
            receiver_view: receiver,
            inheritance_depth: 0,
        };
        let index = SourceSemanticIndex::new();
        let source = SourceId::from_index(0);
        let mut typer =
            SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages);

        assert!(matches!(
            typer.member_type_on(&candidate),
            Err(TyperError::ExternalGenericInstantiationDeferred { class })
                if class == external_class
        ));
    }

    #[test]
    fn member_lookup_completes_a_missing_current_source_parent() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Base { val value: Int = 1 }; class Child extends Base");
        let base = class_symbol(&parsed, &store, &index, source, "Base");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let name = Name::new(store.names.intern("value"), Namespace::Term);
        let base_scope = index.scope_of(base).unwrap();
        let field = store.scopes.get(base_scope).lookup(&name).unwrap();
        let receiver = store.types.alloc(Type::TypeRef {
            prefix: definitions.no_prefix,
            target: TypeRefTarget::Symbol(child),
        });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidates = typer.lookup_members(receiver, name).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].symbol, field);
        assert_eq!(candidates[0].declaring_class, base);
        assert!(matches!(
            *typer.store().symbols.info(base),
            SymbolInfo::Complete(_)
        ));
    }

    #[test]
    fn inherited_parent_arguments_are_instantiated_from_the_child_receiver() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Parent[A] { val value: A = null }; class Child[B] extends Parent[B]; class Text",
        );
        let parent = class_symbol(&parsed, &store, &index, source, "Parent");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let text = class_symbol(&parsed, &store, &index, source, "Text");
        let name = Name::new(store.names.intern("value"), Namespace::Term);
        let text_ty = nominal_type_ref(&mut store, definitions, text);
        let receiver = applied_class_type(&mut store, definitions, child, &[text_ty]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidate = candidate_for(&mut typer, receiver, parent, name);
        let Type::Applied { tycon, args } = typer.store().types.get(candidate.receiver_view) else {
            panic!("expected an instantiated parent view")
        };
        assert_eq!(type_symbol(typer.store(), *tycon), parent);
        assert_eq!(type_symbol(typer.store(), args[0]), text);
        let adapted = typer.member_type_on(&candidate).unwrap();
        assert_eq!(type_symbol(typer.store(), adapted), text);
    }

    #[test]
    fn inherited_nested_parent_arguments_are_instantiated_before_member_lookup() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Parent[A] { val value: A = null }; class List[X]; class Child[B] extends Parent[List[B]]; class Text",
        );
        let parent = class_symbol(&parsed, &store, &index, source, "Parent");
        let list = class_symbol(&parsed, &store, &index, source, "List");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let text = class_symbol(&parsed, &store, &index, source, "Text");
        let name = Name::new(store.names.intern("value"), Namespace::Term);
        let text_ty = nominal_type_ref(&mut store, definitions, text);
        let receiver = applied_class_type(&mut store, definitions, child, &[text_ty]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidate = candidate_for(&mut typer, receiver, parent, name);
        let Type::Applied { args, .. } = typer.store().types.get(candidate.receiver_view) else {
            panic!("expected the applied parent view")
        };
        let Type::Applied {
            tycon: nested_tycon,
            args: nested_args,
        } = typer.store().types.get(args[0])
        else {
            panic!("expected a nested List argument")
        };
        assert_eq!(type_symbol(typer.store(), *nested_tycon), list);
        assert_eq!(type_symbol(typer.store(), nested_args[0]), text);
        let adapted = typer.member_type_on(&candidate).unwrap();
        let Type::Applied { args, .. } = typer.store().types.get(adapted) else {
            panic!("expected the inherited member's List result")
        };
        assert_eq!(type_symbol(typer.store(), args[0]), text);
    }

    #[test]
    fn multi_level_parent_views_compose_class_substitutions() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Parent[A] { val value: A = null }; class List[X]; class Middle[C] extends Parent[List[C]]; class Child[B] extends Middle[B]; class Text",
        );
        let parent = class_symbol(&parsed, &store, &index, source, "Parent");
        let list = class_symbol(&parsed, &store, &index, source, "List");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let text = class_symbol(&parsed, &store, &index, source, "Text");
        let name = Name::new(store.names.intern("value"), Namespace::Term);
        let text_ty = nominal_type_ref(&mut store, definitions, text);
        let receiver = applied_class_type(&mut store, definitions, child, &[text_ty]);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidate = candidate_for(&mut typer, receiver, parent, name);
        let adapted = typer.member_type_on(&candidate).unwrap();
        let Type::Applied { tycon, args } = typer.store().types.get(adapted) else {
            panic!("expected the composed List member type")
        };
        assert_eq!(type_symbol(typer.store(), *tycon), list);
        assert_eq!(type_symbol(typer.store(), args[0]), text);
    }

    #[test]
    fn explicit_class_parent_is_preserved() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Base; class Child extends Base");
        let base = class_symbol(&parsed, &store, &index, source, "Base");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let child_info = typer.complete_symbol(child).unwrap();

        let Type::ClassInfo(info) = typer.store().types.get(child_info) else {
            panic!("expected ClassInfo");
        };
        assert_eq!(info.parents.len(), 1);
        assert_eq!(type_symbol(typer.store(), info.parents[0]), base);
    }

    #[test]
    fn class_with_trait_only_parent_gets_object_as_its_real_class_parent() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("trait T; class C extends T");
        let trait_symbol = class_symbol(&parsed, &store, &index, source, "T");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info_id = typer.complete_symbol(class).unwrap();

        let Type::ClassInfo(info) = typer.store().types.get(info_id) else {
            panic!("expected ClassInfo");
        };
        assert_eq!(info.parents.len(), 2);
        assert_eq!(info.parents[0], definitions.object_type);
        assert_eq!(type_symbol(typer.store(), info.parents[1]), trait_symbol);
    }

    #[test]
    fn trait_alias_parent_gets_object_before_the_alias_reference() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("trait T\nobject O { type Alias = T; class C extends Alias }");
        let trait_symbol = class_symbol(&parsed, &store, &index, source, "T");
        let alias = type_alias_symbol(&parsed, &store, &index, source, "Alias");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info_id = typer.complete_symbol(class).unwrap();

        assert!(matches!(
            *typer.store().symbols.info(alias),
            SymbolInfo::Complete(info)
                if matches!(typer.store().types.get(info), Type::AliasingBounds { alias: target }
                    if type_symbol(typer.store(), *target) == trait_symbol)
        ));
        let Type::ClassInfo(info) = typer.store().types.get(info_id) else {
            panic!("expected ClassInfo");
        };
        assert_eq!(info.parents.len(), 2);
        assert_eq!(info.parents[0], definitions.object_type);
        assert_eq!(type_symbol(typer.store(), info.parents[1]), alias);
    }

    #[test]
    fn class_alias_parent_does_not_get_an_unneeded_object_parent() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Base\nobject O { type Alias = Base; class C extends Alias }");
        let alias = type_alias_symbol(&parsed, &store, &index, source, "Alias");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info_id = typer.complete_symbol(class).unwrap();

        let Type::ClassInfo(info) = typer.store().types.get(info_id) else {
            panic!("expected ClassInfo");
        };
        assert_eq!(info.parents.len(), 1);
        assert_eq!(type_symbol(typer.store(), info.parents[0]), alias);
    }

    #[test]
    fn type_parameter_parent_with_unknown_class_kind_is_deferred_explicitly() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C[A] extends A");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let parameter = type_parameter_symbol(&parsed, &store, &index, source, "A");
        let parent_tree_index = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::Ident(identifier)
                    if identifier.name.is_type()
                        && store.names.resolve(identifier.name.text()) == "A" =>
                {
                    Some(tree.index())
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(class),
            Err(TyperError::UnresolvedParentClassKind {
                source: error_source,
                tree_index,
                symbol: Some(error_symbol),
            }) if error_source == source
                && tree_index == parent_tree_index
                && error_symbol == parameter
        ));
        assert_eq!(*typer.store().symbols.info(class), SymbolInfo::Missing);
    }

    #[test]
    fn class_type_parameters_complete_before_generic_parent_projection() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Base[A]; class Child[A] extends Base[A]");
        let base = class_symbol(&parsed, &store, &index, source, "Base");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let child_parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "A"
                        && index.symbol_at(source, tree).is_some_and(|symbol| {
                            store.symbols.get(symbol).kind == SymbolKind::TypeParameter
                                && store.symbols.get(symbol).owner == Some(child)
                        }) =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info_id = typer.complete_symbol(child).unwrap();

        assert!(matches!(
            *typer.store().symbols.info(child_parameter),
            SymbolInfo::Complete(_)
        ));
        let Type::ClassInfo(info) = typer.store().types.get(info_id) else {
            panic!("expected ClassInfo");
        };
        let Type::Applied { tycon, args } = typer.store().types.get(info.parents[0]) else {
            panic!("expected applied generic parent");
        };
        assert_eq!(type_symbol(typer.store(), *tycon), base);
        assert_eq!(args.len(), 1);
        assert_eq!(type_symbol(typer.store(), args[0]), child_parameter);
    }

    #[test]
    fn constructor_arguments_do_not_change_class_parent_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Base(x: Int); class Child extends Base(1)");
        let base = class_symbol(&parsed, &store, &index, source, "Base");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info_id = typer.complete_symbol(child).unwrap();

        let Type::ClassInfo(info) = typer.store().types.get(info_id) else {
            panic!("expected ClassInfo");
        };
        assert_eq!(info.parents.len(), 1);
        assert_eq!(type_symbol(typer.store(), info.parents[0]), base);
    }

    #[test]
    fn outer_parent_type_apply_keeps_its_arguments_after_an_applied_tpt() {
        // Synthetic source AST: the parser represents ordinary nested type
        // applications as AppliedTypeTree, while this explicit TypeApply
        // exercises the distinct constructor-call wrapper shape.
        let (mut parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Base; val seed: Int = 0; class Child extends Base");
        let base = class_symbol(&parsed, &store, &index, source, "Base");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let context = index.declaration_context_of(child).unwrap();
        let base_tpt = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::Ident(identifier)
                    if identifier.name.is_type()
                        && store.names.resolve(identifier.name.text()) == "Base" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let int_tpt = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::Ident(identifier)
                    if identifier.name.is_type()
                        && store.names.resolve(identifier.name.text()) == "Int" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let inner = parsed.ast.alloc(dotty_core::Tree {
            kind: TreeKind::AppliedTypeTree(dotty_core::ast::AppliedTypeTree {
                tpt: base_tpt,
                args: vec![int_tpt],
            }),
            position: None,
            ty: (),
        });
        let outer = parsed.ast.alloc(dotty_core::Tree {
            kind: TreeKind::TypeApply(dotty_core::ast::TypeApply {
                function: inner,
                args: vec![int_tpt],
            }),
            position: None,
            ty: (),
        });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.project_parent_type(outer, context, 0).unwrap();

        let Type::Applied { tycon, args } = typer.store().types.get(projected) else {
            panic!("the outer TypeApply must be preserved");
        };
        assert_eq!(args.len(), 1);
        let Type::Applied {
            tycon: inner_tycon,
            args: inner_args,
        } = typer.store().types.get(*tycon)
        else {
            panic!("the inner applied type tree must be preserved");
        };
        assert_eq!(args[0], inner_args[0]);
        assert_eq!(type_symbol(typer.store(), *inner_tycon), base);
    }

    #[test]
    fn parent_constructor_type_apply_does_not_repeat_applied_tpt_arguments() {
        // Synthetic source AST mirroring Dotty's constructor-call wrapper:
        // `TypeApply(Apply(Select(New(Base[Int]), <init>), ...), [Int])`.
        let (mut parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Base[A]; val seed: Int = 0; class Child extends Base");
        let base = class_symbol(&parsed, &store, &index, source, "Base");
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let context = index.declaration_context_of(child).unwrap();
        let base_tpt = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::Ident(identifier)
                    if identifier.name.is_type()
                        && store.names.resolve(identifier.name.text()) == "Base" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let int_tpt = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::Ident(identifier)
                    if identifier.name.is_type()
                        && store.names.resolve(identifier.name.text()) == "Int" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let applied_tpt = parsed.ast.alloc(dotty_core::Tree {
            kind: TreeKind::AppliedTypeTree(dotty_core::ast::AppliedTypeTree {
                tpt: base_tpt,
                args: vec![int_tpt],
            }),
            position: None,
            ty: (),
        });
        let new_tree = parsed.ast.alloc(dotty_core::Tree {
            kind: TreeKind::New(dotty_core::ast::New { tpt: applied_tpt }),
            position: None,
            ty: (),
        });
        let constructor_name = *TermName::new(store.names.intern("<init>")).as_name();
        let selected_constructor = parsed.ast.alloc(dotty_core::Tree {
            kind: TreeKind::Select(dotty_core::ast::Select {
                qualifier: new_tree,
                name: constructor_name,
                backquoted: false,
            }),
            position: None,
            ty: (),
        });
        let constructor_call = parsed.ast.alloc(dotty_core::Tree {
            kind: TreeKind::Apply(dotty_core::ast::Apply {
                function: selected_constructor,
                args: vec![int_tpt],
                kind: dotty_core::ast::ApplyKind::Regular,
            }),
            position: None,
            ty: (),
        });
        let parent = parsed.ast.alloc(dotty_core::Tree {
            kind: TreeKind::TypeApply(dotty_core::ast::TypeApply {
                function: constructor_call,
                args: vec![int_tpt],
            }),
            position: None,
            ty: (),
        });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.project_parent_type(parent, context, 0).unwrap();

        let Type::Applied { tycon, args } = typer.store().types.get(projected) else {
            panic!("expected one application from the New type tree");
        };
        assert_eq!(type_symbol(typer.store(), *tycon), base);
        assert_eq!(args.len(), 1);
    }

    #[test]
    fn explicit_self_type_is_projected_without_adding_a_self_member() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("trait SelfType\n\nclass C:\n  self: SelfType =>\n");
        let self_type = class_symbol(&parsed, &store, &index, source, "SelfType");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let declaration_scope = index.scope_of(class).unwrap();
        let self_name = Name::new(store.names.intern("self"), Namespace::Term);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info_id = typer.complete_symbol(class).unwrap();

        let Type::ClassInfo(info) = typer.store().types.get(info_id) else {
            panic!("expected ClassInfo");
        };
        assert_eq!(
            type_symbol(typer.store(), info.self_type.unwrap()),
            self_type
        );
        assert!(
            typer
                .store()
                .scopes
                .get(declaration_scope)
                .lookup_all(&self_name)
                .is_empty()
        );
    }

    #[test]
    fn self_alias_without_a_type_has_no_additional_self_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { self => }");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info_id = typer.complete_symbol(class).unwrap();

        let Type::ClassInfo(info) = typer.store().types.get(info_id) else {
            panic!("expected ClassInfo");
        };
        assert_eq!(info.self_type, None);
    }

    #[test]
    fn nested_class_uses_normalized_no_prefix() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Outer { class Inner }");
        let inner = class_symbol(&parsed, &store, &index, source, "Inner");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info_id = typer.complete_symbol(inner).unwrap();

        let Type::ClassInfo(info) = typer.store().types.get(info_id) else {
            panic!("expected ClassInfo");
        };
        assert_eq!(info.prefix, definitions.no_prefix);
    }

    #[test]
    fn trait_without_source_parent_gets_scala_390_object_parent() {
        // The Scala 3.9.0 ClassInfos.scala oracle fixture declares
        // `trait InfoBase[A]` without a parent; its TASTy ClassInfo has one
        // parent entry, java.lang.Object. Source completion preserves it.
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("trait T[A]");
        let trait_symbol = class_symbol(&parsed, &store, &index, source, "T");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info_id = typer.complete_symbol(trait_symbol).unwrap();

        let Type::ClassInfo(info) = typer.store().types.get(info_id) else {
            panic!("expected ClassInfo");
        };
        assert_eq!(info.parents, vec![definitions.object_type]);
    }

    #[test]
    fn class_completion_leaves_ordinary_members_missing() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val field: Int = 1; def method: Int = 1 }");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let field = val_symbol(&parsed, &store, &index, source, "field").0;
        let method = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "method" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        typer.complete_symbol(class).unwrap();

        assert_eq!(*typer.store().symbols.info(field), SymbolInfo::Missing);
        assert_eq!(*typer.store().symbols.info(method), SymbolInfo::Missing);
    }

    #[test]
    fn source_object_module_class_uses_its_template_parents() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("trait T\nobject O extends T");
        let trait_symbol = class_symbol(&parsed, &store, &index, source, "T");
        let (object_tree, object) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module))
                    if store.names.resolve(module.name.as_name().text()) == "O" =>
                {
                    Some((tree, index.symbol_at(source, tree).unwrap()))
                }
                _ => None,
            })
            .unwrap();
        let module_class = index
            .derived_symbol_at(
                store.symbols.get(object).owner.unwrap(),
                source,
                object_tree,
            )
            .unwrap();
        let module_template = match &parsed.ast.get(object_tree).kind {
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module)) => module.template,
            _ => unreachable!(),
        };
        let scope = index.scope_of(module_class).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info_id = typer.complete_symbol(module_class).unwrap();

        let Type::ClassInfo(info) = typer.store().types.get(info_id) else {
            panic!("expected ModuleClass ClassInfo");
        };
        assert_eq!(info.class, module_class);
        assert_eq!(info.declarations, scope);
        assert_eq!(info.parents.len(), 2);
        assert_eq!(info.parents[0], definitions.object_type);
        assert_eq!(type_symbol(typer.store(), info.parents[1]), trait_symbol);
        assert_eq!(*typer.store().symbols.info(object), SymbolInfo::Missing);
        assert!(matches!(
            parsed.ast.get(module_template).kind,
            TreeKind::Template(_)
        ));
    }

    #[test]
    fn failed_class_parent_projection_rolls_back_header_parameters() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C[A] extends MissingParent");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "A"
                        && index.symbol_at(source, tree).is_some_and(|symbol| {
                            store.symbols.get(symbol).kind == SymbolKind::TypeParameter
                        }) =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let parent_name = Name::new(store.names.intern("MissingParent"), Namespace::Type);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let error = typer.complete_symbol(class).unwrap_err();

        assert!(matches!(
            error,
            TyperError::TypeNameNotFound { name, .. } if name == parent_name
        ));
        assert_eq!(*typer.store().symbols.info(class), SymbolInfo::Missing);
        assert_eq!(*typer.store().symbols.info(parameter), SymbolInfo::Missing);
    }

    #[test]
    fn failed_self_type_projection_leaves_class_missing() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C:\n  self: MissingSelf =>\n");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let self_name = Name::new(store.names.intern("MissingSelf"), Namespace::Type);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let error = typer.complete_symbol(class).unwrap_err();

        assert!(matches!(
            error,
            TyperError::TypeNameNotFound { name, .. } if name == self_name
        ));
        assert_eq!(*typer.store().symbols.info(class), SymbolInfo::Missing);
    }

    #[test]
    fn repeated_class_completion_reuses_its_published_info() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("class C");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let first = typer.complete_symbol(class).unwrap();
        let second = typer.complete_symbol(class).unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn class_completion_rejects_non_class_existing_info() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("class C");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let malformed = store.types.alloc(Type::NoType);
        store
            .symbols
            .set_info(class, SymbolInfo::Complete(malformed));
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(class),
            Err(TyperError::MalformedClassInfo { symbol, info })
                if symbol == class && info == malformed
        ));
    }

    #[test]
    fn class_completion_rejects_existing_info_for_another_class() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C; class Other");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let other = class_symbol(&parsed, &store, &index, source, "Other");
        let scope = index.scope_of(class).unwrap();
        let malformed = store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: definitions.no_prefix,
            class: other,
            parents: vec![definitions.object_type],
            declarations: scope,
            self_type: None,
        }));
        store
            .symbols
            .set_info(class, SymbolInfo::Complete(malformed));
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(class),
            Err(TyperError::MalformedClassInfo { symbol, info })
                if symbol == class && info == malformed
        ));
    }

    #[test]
    fn class_completion_rejects_existing_info_with_noncanonical_prefix() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("class C");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let scope = index.scope_of(class).unwrap();
        let malformed = store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: definitions.object_type,
            class,
            parents: vec![definitions.object_type],
            declarations: scope,
            self_type: None,
        }));
        store
            .symbols
            .set_info(class, SymbolInfo::Complete(malformed));
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(class),
            Err(TyperError::MalformedClassInfo { symbol, info })
                if symbol == class && info == malformed
        ));
    }

    #[test]
    fn class_completion_rejects_existing_info_with_another_declaration_scope() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("class C");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let malformed_scope = store.scopes.alloc(dotty_core::Scope::new(Some(class)));
        let malformed = store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: definitions.no_prefix,
            class,
            parents: vec![definitions.object_type],
            declarations: malformed_scope,
            self_type: None,
        }));
        store
            .symbols
            .set_info(class, SymbolInfo::Complete(malformed));
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(class),
            Err(TyperError::MalformedClassInfo { symbol, info })
                if symbol == class && info == malformed
        ));
    }

    #[test]
    fn unbounded_type_parameter_uses_canonical_nothing_and_any() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def id[A](x: A): A = x");
        let parameter = type_parameter_symbol(&parsed, &store, &index, source, "A");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info = typer.complete_symbol(parameter).unwrap();

        assert!(matches!(
            typer.store().types.get(info),
            Type::Bounds { low, high }
                if *low == definitions.nothing_type && *high == definitions.any_type
        ));
    }

    #[test]
    fn type_parameter_completes_explicit_lower_and_upper_bounds() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class L; class H; def id[A >: L <: H](x: A): A = x");
        let parameter = type_parameter_symbol(&parsed, &store, &index, source, "A");
        let class_symbol = |wanted: &str| {
            parsed
                .ast
                .iter()
                .find_map(|(tree, node)| match &node.kind {
                    TreeKind::TypeDef(definition)
                        if store.names.resolve(definition.name.as_name().text()) == wanted =>
                    {
                        index.symbol_at(source, tree)
                    }
                    _ => None,
                })
                .unwrap()
        };
        let expected_low = class_symbol("L");
        let expected_high = class_symbol("H");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info = typer.complete_symbol(parameter).unwrap();

        assert!(matches!(
            typer.store().types.get(info),
            Type::Bounds { low, high }
                if type_symbol(typer.store(), *low) == expected_low
                    && type_symbol(typer.store(), *high) == expected_high
        ));
    }

    #[test]
    fn type_parameter_with_only_upper_bound_uses_nothing_as_lower_bound() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class H; def id[A <: H](x: A): A = x");
        let parameter = type_parameter_symbol(&parsed, &store, &index, source, "A");
        let expected_high = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "H" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info = typer.complete_symbol(parameter).unwrap();

        assert!(matches!(
            typer.store().types.get(info),
            Type::Bounds { low, high }
                if *low == definitions.nothing_type
                    && type_symbol(typer.store(), *high) == expected_high
        ));
    }

    #[test]
    fn type_parameter_with_only_lower_bound_uses_any_as_upper_bound() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class L; def id[A >: L](x: A): A = x");
        let parameter = type_parameter_symbol(&parsed, &store, &index, source, "A");
        let expected_low = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "L" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info = typer.complete_symbol(parameter).unwrap();

        assert!(matches!(
            typer.store().types.get(info),
            Type::Bounds { low, high }
                if type_symbol(typer.store(), *low) == expected_low
                    && *high == definitions.any_type
        ));
    }

    #[test]
    fn context_bound_type_parameter_completes_ordinary_bounds_only() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def id[A: Evidence](x: A): A = x");
        let parameter = type_parameter_symbol(&parsed, &store, &index, source, "A");
        let parameter_tree = index.definition_of(parameter).unwrap();
        let SourceDefinition::Canonical { tree, .. } = parameter_tree else {
            panic!("canonical source type parameter expected");
        };
        let TreeKind::TypeDef(definition) = &parsed.ast.get(tree).kind else {
            panic!("type parameter must be represented by a TypeDef");
        };
        let TreeKind::PhaseSpecific(UntypedNode::ContextBounds(wrapper)) =
            &parsed.ast.get(definition.rhs).kind
        else {
            panic!("parser should preserve the context-bound wrapper");
        };
        assert_eq!(wrapper.context_bounds.len(), 1);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info = typer.complete_symbol(parameter).unwrap();

        assert!(matches!(
            typer.store().types.get(info),
            Type::Bounds { low, high }
                if *low == definitions.nothing_type && *high == definitions.any_type
        ));
    }

    #[test]
    fn higher_kinded_type_parameter_completion_is_deferred_without_mutation() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def id[F[_]](x: F[Int]): Int = 1");
        let parameter = type_parameter_symbol(&parsed, &store, &index, source, "F");
        let SourceDefinition::Canonical { tree, .. } = index.definition_of(parameter).unwrap()
        else {
            panic!("canonical source type parameter expected");
        };
        let TreeKind::TypeDef(definition) = &parsed.ast.get(tree).kind else {
            panic!("type parameter must be represented by a TypeDef");
        };
        let rhs = definition.rhs;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(parameter),
            Err(TyperError::HigherKindedTypeParameterDeferred { symbol, tree_index })
                if symbol == parameter && tree_index == rhs.index()
        ));
        assert_eq!(*typer.store().symbols.info(parameter), SymbolInfo::Missing);
    }

    #[test]
    fn type_parameter_completion_is_idempotent_and_reuses_its_info_id() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def id[A](value: A): A = value");
        let parameter = type_parameter_symbol(&parsed, &store, &index, source, "A");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let first = typer.complete_symbol(parameter).unwrap();
        let after_first = typer.store().checkpoint();
        let second = typer.complete_symbol(parameter).unwrap();

        assert_eq!(first, second);
        assert_eq!(typer.store().checkpoint(), after_first);
    }

    #[test]
    fn failed_type_parameter_bounds_roll_back_allocations_and_leave_info_missing() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def id[A <: Missing](value: A): A = value");
        let parameter = type_parameter_symbol(&parsed, &store, &index, source, "A");
        let SourceDefinition::Canonical { tree, .. } = index.definition_of(parameter).unwrap()
        else {
            panic!("canonical source type parameter expected");
        };
        let TreeKind::TypeDef(definition) = &parsed.ast.get(tree).kind else {
            panic!("type parameter must be represented by a TypeDef");
        };
        let TreeKind::TypeBoundsTree(bounds) = &parsed.ast.get(definition.rhs).kind else {
            panic!("type parameter bounds must be represented by TypeBoundsTree");
        };
        let missing_type = bounds.high.unwrap();
        let missing_position = parsed.ast.get(missing_type).position;
        let before = store.checkpoint();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(parameter),
            Err(TyperError::TypeNameNotFound {
                source: found_source,
                tree_index,
                name,
                position,
            }) if found_source == source
                && tree_index == missing_type.index()
                && typer.store().names.resolve(name.text()) == "Missing"
                && position == missing_position
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(*typer.store().symbols.info(parameter), SymbolInfo::Missing);
    }

    #[test]
    fn source_type_alias_completes_to_aliasing_bounds() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("type Alias = Int");
        let alias = type_alias_symbol(&parsed, &store, &index, source, "Alias");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info = typer.complete_symbol(alias).unwrap();

        assert!(matches!(
            typer.store().types.get(info),
            Type::AliasingBounds { alias } if *alias == definitions.int
        ));
    }

    #[test]
    fn source_abstract_type_completes_to_plain_bounds() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class L; class H; type Abstract >: L <: H");
        let abstract_type = type_alias_symbol(&parsed, &store, &index, source, "Abstract");
        let bound_symbol = |wanted: &str| {
            parsed
                .ast
                .iter()
                .find_map(|(tree, node)| match &node.kind {
                    TreeKind::TypeDef(definition)
                        if store.names.resolve(definition.name.as_name().text()) == wanted =>
                    {
                        index.symbol_at(source, tree)
                    }
                    _ => None,
                })
                .unwrap()
        };
        let expected_low = bound_symbol("L");
        let expected_high = bound_symbol("H");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info = typer.complete_symbol(abstract_type).unwrap();

        assert!(matches!(
            typer.store().types.get(info),
            Type::Bounds { low, high }
                if type_symbol(typer.store(), *low) == expected_low
                    && type_symbol(typer.store(), *high) == expected_high
        ));
    }

    #[test]
    fn opaque_source_type_alias_is_deferred_and_stays_missing() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("opaque type Secret = Int");
        let alias = type_alias_symbol(&parsed, &store, &index, source, "Secret");
        let SourceDefinition::Canonical { tree, .. } = index.definition_of(alias).unwrap() else {
            panic!("canonical source type alias expected");
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(alias),
            Err(TyperError::OpaqueAliasDeferred { symbol, tree_index })
                if symbol == alias && tree_index == tree.index()
        ));
        assert_eq!(*typer.store().symbols.info(alias), SymbolInfo::Missing);
    }

    #[test]
    fn higher_kinded_source_type_alias_is_deferred_and_stays_missing() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("type F[A] = Option[A]");
        let alias = type_alias_symbol(&parsed, &store, &index, source, "F");
        let SourceDefinition::Canonical { tree, .. } = index.definition_of(alias).unwrap() else {
            panic!("canonical source type alias expected");
        };
        let TreeKind::TypeDef(definition) = &parsed.ast.get(tree).kind else {
            panic!("type alias must be represented by a TypeDef");
        };
        let rhs = definition.rhs;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(alias),
            Err(TyperError::HigherKindedTypeAliasDeferred { symbol, tree_index })
                if symbol == alias && tree_index == rhs.index()
        ));
        assert_eq!(*typer.store().symbols.info(alias), SymbolInfo::Missing);
    }

    #[test]
    fn to_bounds_preserves_existing_aliasing_bounds_id() {
        let (arena, mut store, packages, definitions) = setup();
        let index = SourceSemanticIndex::new();
        let existing = store.types.alloc(Type::AliasingBounds {
            alias: definitions.int,
        });
        let mut typer = SourceTyper::new(
            &arena,
            SourceId::from_index(1),
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let actual = typer.alias_bounds_for_type(existing, 14).unwrap();

        assert_eq!(actual, existing);
    }

    #[test]
    fn to_bounds_rejects_by_name_types_with_exact_error() {
        let (arena, mut store, packages, definitions) = setup();
        let index = SourceSemanticIndex::new();
        let by_name = store.types.alloc(Type::ByName {
            result: definitions.int,
        });
        let mut typer = SourceTyper::new(
            &arena,
            SourceId::from_index(1),
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.alias_bounds_for_type(by_name, 14),
            Err(TyperError::InvalidCompletedBounds { tree_index: 14, ty, .. })
                if ty == by_name
        ));
    }

    #[test]
    fn to_bounds_rejects_method_types_with_exact_error() {
        let (arena, mut store, packages, definitions) = setup();
        let index = SourceSemanticIndex::new();
        let method = store
            .types
            .alloc(Type::Method(dotty_core::types::MethodType {
                params: Vec::new(),
                result: definitions.int,
                kind: dotty_core::types::MethodKind::Plain,
            }));
        let mut typer = SourceTyper::new(
            &arena,
            SourceId::from_index(1),
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.alias_bounds_for_type(method, 19),
            Err(TyperError::InvalidCompletedBounds { tree_index: 19, ty, .. })
                if ty == method
        ));
    }

    #[test]
    fn to_bounds_rejects_polymorphic_types_with_exact_error() {
        let (arena, mut store, packages, definitions) = setup();
        let index = SourceSemanticIndex::new();
        let poly = store.types.alloc(Type::Poly(dotty_core::types::PolyType {
            params: Vec::new(),
            result: definitions.int,
        }));
        let mut typer = SourceTyper::new(
            &arena,
            SourceId::from_index(1),
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.alias_bounds_for_type(poly, 23),
            Err(TyperError::InvalidCompletedBounds { tree_index: 23, ty, .. })
                if ty == poly
        ));
    }

    #[test]
    fn complete_returns_an_existing_type_without_requiring_provenance() {
        let (arena, mut store, packages, definitions) = setup();
        let index = SourceSemanticIndex::new();
        let symbol = symbol(
            &mut store,
            SymbolKind::Value,
            SymbolInfo::Complete(definitions.int),
        );
        let mut typer = SourceTyper::new(
            &arena,
            SourceId::from_index(0),
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(symbol),
            Ok(ty) if ty == definitions.int
        ));
    }

    #[test]
    fn complete_class_info_without_source_scope_is_reused_for_external_symbols() {
        let (arena, mut store, packages, definitions) = setup();
        let index = SourceSemanticIndex::new();
        let class = symbol(&mut store, SymbolKind::Class, SymbolInfo::Missing);
        let declarations = store.scopes.alloc(dotty_core::Scope::new(Some(class)));
        let info = store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: definitions.no_prefix,
            class,
            parents: vec![definitions.object_type],
            declarations,
            self_type: None,
        }));
        store.symbols.set_info(class, SymbolInfo::Complete(info));
        let mut typer = SourceTyper::new(
            &arena,
            SourceId::from_index(0),
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(typer.complete_symbol(class), Ok(completed) if completed == info));
    }

    #[test]
    fn missing_provenance_is_reported_without_mutation() {
        let (arena, mut store, packages, definitions) = setup();
        let index = SourceSemanticIndex::new();
        let symbol = symbol(&mut store, SymbolKind::Value, SymbolInfo::Missing);
        let before = store.checkpoint();
        let mut typer = SourceTyper::new(
            &arena,
            SourceId::from_index(0),
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(symbol),
            Err(TyperError::SourceProvenanceMissing { symbol: found }) if found == symbol
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(*typer.store().symbols.info(symbol), SymbolInfo::Missing);
    }

    #[test]
    fn named_value_completion_projects_and_caches_its_type_tree() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C\nval x: C = 1");
        let (symbol, tpt) = val_symbol(&parsed, &store, &index, source, "x");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let completed = typer.complete_symbol(symbol).unwrap();
        let context = index.declaration_context_of(symbol).unwrap();
        let projected_again = typer.type_of_tpt(tpt, context).unwrap();

        assert_eq!(completed, projected_again);
        assert_eq!(
            typer.source_type_index().type_at(source, tpt),
            Some(completed)
        );
        assert_eq!(
            *typer.store().symbols.info(symbol),
            SymbolInfo::Complete(completed)
        );
    }

    #[test]
    fn class_type_parameter_is_resolved_in_a_field_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A] { val value: A = 1 }");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "value");
        let type_parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "A" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), type_parameter);
    }

    #[test]
    fn method_parameter_completion_uses_its_method_type_parameter() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def id[A](value: A): A = value");
        let method = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "id" =>
                {
                    Some(definition)
                }
                _ => None,
            })
            .unwrap();
        let type_parameter = index.symbol_at(source, method.type_params[0]).unwrap();
        let parameter = index
            .symbol_at(source, method.value_param_clauses[0][0])
            .unwrap();
        assert_eq!(store.symbols.get(parameter).kind, SymbolKind::Parameter);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info = typer.complete_symbol(parameter).unwrap();

        assert_eq!(type_symbol(typer.store(), info), type_parameter);
    }

    #[test]
    fn method_signature_binds_type_and_value_parameters_by_symbol() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def id[A](value: A): A = value");
        let (method_tree, method_definition) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "id" =>
                {
                    Some((tree, definition))
                }
                _ => None,
            })
            .unwrap();
        let method = index.symbol_at(source, method_tree).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(method).unwrap();

        let Type::Poly(poly) = typer.store().types.get(signature) else {
            panic!("expected the method type parameter binder");
        };
        assert_eq!(poly.params.len(), 1);
        let poly_binder = signature;
        let Type::Method(method_type) = typer.store().types.get(poly.result) else {
            panic!("expected the value parameter clause");
        };
        assert_eq!(method_type.kind, MethodKind::Plain);
        assert_eq!(method_type.params.len(), 1);
        assert!(matches!(
            typer.store().types.get(method_type.params[0].ty),
            Type::ParamRef { binder, index: 0 } if *binder == poly_binder
        ));
        assert!(matches!(
            typer.store().types.get(method_type.result),
            Type::ParamRef { binder, index: 0 } if *binder == poly_binder
        ));
        let TreeKind::ValDef(parameter) = &parsed
            .ast
            .get(method_definition.value_param_clauses[0][0])
            .kind
        else {
            panic!("expected a value parameter definition");
        };
        assert_eq!(
            typer
                .store()
                .names
                .resolve(method_type.params[0].name.as_name().text()),
            typer.store().names.resolve(parameter.name.as_name().text())
        );
    }

    #[test]
    fn method_signature_preserves_curried_clause_kinds() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def f(a: Int)(using b: Int)(implicit c: Boolean): Unit = ()");
        let method_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "f" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let method = index.symbol_at(source, method_tree).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let mut signature = typer.complete_symbol(method).unwrap();
        let expected_kinds = [
            MethodKind::Plain,
            MethodKind::Contextual,
            MethodKind::Implicit,
        ];
        for (clause_index, expected_kind) in expected_kinds.into_iter().enumerate() {
            let Type::Method(method_type) = typer.store().types.get(signature) else {
                panic!("expected method clause {clause_index}");
            };
            assert_eq!(method_type.kind, expected_kind);
            assert_eq!(method_type.params.len(), 1);
            assert!(!method_type.params[0].erased);
            signature = method_type.result;
        }
        assert_eq!(signature, definitions.unit);
    }

    #[test]
    fn inconsistent_parameter_flags_fail_method_completion_atomically() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def f(using first: Int, second: Int): Int = first");
        let (method_tree, method_definition) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "f" =>
                {
                    Some((tree, definition))
                }
                _ => None,
            })
            .unwrap();
        let method = index.symbol_at(source, method_tree).unwrap();
        let first = index
            .symbol_at(source, method_definition.value_param_clauses[0][0])
            .unwrap();
        let second = index
            .symbol_at(source, method_definition.value_param_clauses[0][1])
            .unwrap();
        let second_flags = store.symbols.get(second).flags;
        store.symbols.get_mut(second).flags =
            second_flags.difference(SymbolFlags::GIVEN) | SymbolFlags::IMPLICIT;
        let before = store.checkpoint();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(method),
            Err(TyperError::MalformedMethodClause {
                method: error_method,
                method_tree_index,
                clause_index: 0
            }) if error_method == method && method_tree_index == method_tree.index()
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(*typer.store().symbols.info(method), SymbolInfo::Missing);
        assert_eq!(*typer.store().symbols.info(first), SymbolInfo::Missing);
    }

    #[test]
    fn explicit_parameterless_method_signature_is_its_result_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def answer: Int = 42");
        let method_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "answer" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let method = index.symbol_at(source, method_tree).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(method).unwrap();
        assert_eq!(typer.complete_symbol(method).unwrap(), signature);

        assert_eq!(signature, definitions.int);
    }

    #[test]
    fn method_signature_preserves_erased_parameter_flags() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def f(value: Int): Int = value");
        let (method_tree, method_definition) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "f" =>
                {
                    Some((tree, definition))
                }
                _ => None,
            })
            .unwrap();
        let method = index.symbol_at(source, method_tree).unwrap();
        let parameter = index
            .symbol_at(source, method_definition.value_param_clauses[0][0])
            .unwrap();
        let parameter_flags = store.symbols.get(parameter).flags;
        store.symbols.get_mut(parameter).flags = parameter_flags | SymbolFlags::ERASED;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(method).unwrap();

        let Type::Method(method_type) = typer.store().types.get(signature) else {
            panic!("expected a method clause");
        };
        assert!(method_type.params[0].erased);
    }

    #[test]
    fn primary_constructor_without_arguments_constructs_its_missing_owner() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("class C");
        let class_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "C" =>
                {
                    Some((tree, definition.rhs))
                }
                _ => None,
            })
            .unwrap();
        let class = index.symbol_at(source, class_tree.0).unwrap();
        let TreeKind::Template(template) = &parsed.ast.get(class_tree.1).kind else {
            panic!("expected a class template");
        };
        let constructor = index.symbol_at(source, template.constructor).unwrap();
        assert_eq!(store.symbols.get(constructor).kind, SymbolKind::Constructor);
        assert_eq!(*store.symbols.info(class), SymbolInfo::Missing);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(constructor).unwrap();

        let Type::Method(method_type) = typer.store().types.get(signature) else {
            panic!("expected normalized empty constructor clause");
        };
        assert_eq!(method_type.kind, MethodKind::Plain);
        assert!(method_type.params.is_empty());
        assert_eq!(type_symbol(typer.store(), method_type.result), class);
        assert_eq!(*typer.store().symbols.info(class), SymbolInfo::Missing);
    }

    #[test]
    fn repeated_primary_constructor_completion_returns_the_same_type_id() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("class C");
        let template = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::Template(template) => Some(template),
                _ => None,
            })
            .unwrap();
        let constructor = index.symbol_at(source, template.constructor).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let first = typer.complete_symbol(constructor).unwrap();
        let second = typer.complete_symbol(constructor).unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn implicit_first_constructor_clause_gets_an_empty_plain_clause_before_it() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C(implicit context: Int)");
        let template = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::Template(template) => Some(template),
                _ => None,
            })
            .unwrap();
        let constructor = index.symbol_at(source, template.constructor).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(constructor).unwrap();

        let Type::Method(plain) = typer.store().types.get(signature) else {
            panic!("expected the inserted leading plain clause");
        };
        assert_eq!(plain.kind, MethodKind::Plain);
        assert!(plain.params.is_empty());
        let Type::Method(implicit) = typer.store().types.get(plain.result) else {
            panic!("expected the source implicit clause after the inserted clause");
        };
        assert_eq!(implicit.kind, MethodKind::Implicit);
        assert_eq!(implicit.params.len(), 1);
        assert_eq!(
            typer
                .store()
                .names
                .resolve(implicit.params[0].name.as_name().text()),
            "context"
        );
    }

    #[test]
    fn all_contextual_constructor_clauses_get_a_trailing_empty_plain_clause() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C(using context: Int)");
        let template = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::Template(template) => Some(template),
                _ => None,
            })
            .unwrap();
        let constructor = index.symbol_at(source, template.constructor).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(constructor).unwrap();

        let Type::Method(contextual) = typer.store().types.get(signature) else {
            panic!("expected the source contextual clause");
        };
        assert_eq!(contextual.kind, MethodKind::Contextual);
        assert_eq!(contextual.params.len(), 1);
        let Type::Method(plain) = typer.store().types.get(contextual.result) else {
            panic!("expected the normalized trailing plain clause");
        };
        assert_eq!(plain.kind, MethodKind::Plain);
        assert!(plain.params.is_empty());
    }

    #[test]
    fn explicit_empty_constructor_clause_prevents_a_second_normalization_clause() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C(using context: Int)()");
        let template = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::Template(template) => Some(template),
                _ => None,
            })
            .unwrap();
        let constructor = index.symbol_at(source, template.constructor).unwrap();
        let class = store.symbols.get(constructor).owner.unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(constructor).unwrap();

        let Type::Method(contextual) = typer.store().types.get(signature) else {
            panic!("expected the source contextual clause");
        };
        assert_eq!(contextual.kind, MethodKind::Contextual);
        let Type::Method(empty) = typer.store().types.get(contextual.result) else {
            panic!("expected the explicit empty clause");
        };
        assert_eq!(empty.kind, MethodKind::Plain);
        assert!(empty.params.is_empty());
        assert_eq!(type_symbol(typer.store(), empty.result), class);
    }

    #[test]
    fn primary_constructor_uses_derived_parameter_separate_from_val_field() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C(val value: Int)");
        let template = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::Template(template) => Some(template),
                _ => None,
            })
            .unwrap();
        let constructor = index.symbol_at(source, template.constructor).unwrap();
        let parameter_tree = match &parsed.ast.get(template.constructor).kind {
            TreeKind::DefDef(definition) => definition.value_param_clauses[0][0],
            _ => panic!("expected a primary constructor definition"),
        };
        let field = index.symbol_at(source, parameter_tree).unwrap();
        let parameter = index
            .derived_symbol_at(constructor, source, parameter_tree)
            .unwrap();
        let class = store.symbols.get(field).owner.unwrap();
        assert_ne!(field, parameter);
        assert_eq!(store.symbols.get(field).kind, SymbolKind::Field);
        assert_eq!(store.symbols.get(parameter).kind, SymbolKind::Parameter);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(constructor).unwrap();

        let Type::Method(method_type) = typer.store().types.get(signature) else {
            panic!("expected a constructor value clause");
        };
        assert_eq!(method_type.params.len(), 1);
        assert_eq!(method_type.params[0].ty, definitions.int);
        assert_eq!(type_symbol(typer.store(), method_type.result), class);
    }

    #[test]
    fn generic_primary_constructor_applies_owner_to_its_derived_type_parameter() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C[A](val value: A)");
        let (class_tree, template_tree) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "C" =>
                {
                    Some((tree, definition.rhs))
                }
                _ => None,
            })
            .unwrap();
        let class = index.symbol_at(source, class_tree).unwrap();
        let TreeKind::Template(template) = &parsed.ast.get(template_tree).kind else {
            panic!("expected a class template");
        };
        let constructor = index.symbol_at(source, template.constructor).unwrap();
        let TreeKind::DefDef(constructor_definition) = &parsed.ast.get(template.constructor).kind
        else {
            panic!("expected a primary constructor definition");
        };
        let class_parameter = index
            .symbol_at(source, constructor_definition.type_params[0])
            .unwrap();
        let constructor_parameter = index
            .derived_symbol_at(constructor, source, constructor_definition.type_params[0])
            .unwrap();
        let value_parameter_tree = constructor_definition.value_param_clauses[0][0];
        let value_field = index.symbol_at(source, value_parameter_tree).unwrap();
        let constructor_value = index
            .derived_symbol_at(constructor, source, value_parameter_tree)
            .unwrap();
        assert_ne!(class_parameter, constructor_parameter);
        assert_ne!(value_field, constructor_value);
        assert_eq!(*store.symbols.info(class), SymbolInfo::Missing);
        assert_eq!(*store.symbols.info(value_field), SymbolInfo::Missing);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(constructor).unwrap();

        let Type::Poly(poly) = typer.store().types.get(signature) else {
            panic!("expected the constructor-owned Poly binder");
        };
        assert_eq!(poly.params.len(), 1);
        let Type::Method(method_type) = typer.store().types.get(poly.result) else {
            panic!("expected the primary constructor value clause");
        };
        assert_eq!(method_type.params.len(), 1);
        assert!(matches!(
            typer.store().types.get(method_type.params[0].ty),
            Type::ParamRef { binder, index: 0 } if *binder == signature
        ));
        let Type::Applied { tycon, args } = typer.store().types.get(method_type.result) else {
            panic!("expected an applied constructed class result");
        };
        assert_eq!(type_symbol(typer.store(), *tycon), class);
        assert_eq!(args.len(), 1);
        assert!(matches!(
            typer.store().types.get(args[0]),
            Type::ParamRef { binder, index: 0 } if *binder == signature
        ));
        let SymbolInfo::Complete(constructor_parameter_info) =
            *typer.store().symbols.info(constructor_parameter)
        else {
            panic!("expected the constructor type parameter to be completed");
        };
        assert!(matches!(
            typer.store().types.get(constructor_parameter_info),
            Type::Bounds { low, high }
                if *low == definitions.nothing_type && *high == definitions.any_type
        ));
        let SymbolInfo::Complete(constructor_value_info) =
            *typer.store().symbols.info(constructor_value)
        else {
            panic!("expected the constructor value parameter to be completed");
        };
        assert_eq!(
            type_symbol(typer.store(), constructor_value_info),
            constructor_parameter
        );
        assert_eq!(
            *typer.store().symbols.info(value_field),
            SymbolInfo::Missing
        );
        assert_eq!(*typer.store().symbols.info(class), SymbolInfo::Missing);
    }

    #[test]
    fn type_clause_is_kept_outer_when_contextual_constructor_clause_is_normalized() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C[A](using context: Int)");
        let template = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::Template(template) => Some(template),
                _ => None,
            })
            .unwrap();
        let constructor = index.symbol_at(source, template.constructor).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(constructor).unwrap();

        let Type::Poly(poly) = typer.store().types.get(signature) else {
            panic!("expected the constructor type parameter binder");
        };
        let Type::Method(contextual) = typer.store().types.get(poly.result) else {
            panic!("expected the contextual clause after the type clause");
        };
        assert_eq!(contextual.kind, MethodKind::Contextual);
        let Type::Method(plain) = typer.store().types.get(contextual.result) else {
            panic!("expected normalization to add an empty plain clause");
        };
        assert_eq!(plain.kind, MethodKind::Plain);
        assert!(plain.params.is_empty());
    }

    #[test]
    fn object_primary_constructor_result_targets_its_module_class() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("object O");
        let (object_tree, template_tree) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::PhaseSpecific(UntypedNode::ModuleDef(definition)) => {
                    Some((tree, definition.template))
                }
                _ => None,
            })
            .unwrap();
        let object = index.symbol_at(source, object_tree).unwrap();
        let object_owner = store.symbols.get(object).owner.unwrap();
        let module_class = index
            .derived_symbol_at(object_owner, source, object_tree)
            .unwrap();
        let TreeKind::Template(template) = &parsed.ast.get(template_tree).kind else {
            panic!("expected an object template");
        };
        let constructor = index.symbol_at(source, template.constructor).unwrap();
        assert_eq!(store.symbols.get(constructor).owner, Some(module_class));
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(constructor).unwrap();

        let Type::Method(method_type) = typer.store().types.get(signature) else {
            panic!("expected the normalized empty object constructor clause");
        };
        assert_eq!(type_symbol(typer.store(), method_type.result), module_class);
    }

    #[test]
    fn constructor_with_non_class_like_owner_fails_without_mutation() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("class C");
        let template = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::Template(template) => Some(template),
                _ => None,
            })
            .unwrap();
        let constructor = index.symbol_at(source, template.constructor).unwrap();
        store.symbols.get_mut(constructor).owner = None;
        let before = store.checkpoint();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(constructor),
            Err(TyperError::ConstructorOwnerNotClassLike {
                constructor: error_constructor,
                owner: None
            }) if error_constructor == constructor
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(
            *typer.store().symbols.info(constructor),
            SymbolInfo::Missing
        );
    }

    #[test]
    fn failed_primary_constructor_parameter_type_rolls_back_everything() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C(value: Missing)");
        let template = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::Template(template) => Some(template),
                _ => None,
            })
            .unwrap();
        let constructor = index.symbol_at(source, template.constructor).unwrap();
        let parameter_tree = match &parsed.ast.get(template.constructor).kind {
            TreeKind::DefDef(definition) => definition.value_param_clauses[0][0],
            _ => panic!("expected a primary constructor definition"),
        };
        let parameter = index
            .derived_symbol_at(constructor, source, parameter_tree)
            .unwrap();
        let type_tree = match &parsed.ast.get(parameter_tree).kind {
            TreeKind::ValDef(definition) => definition.tpt,
            _ => panic!("expected a constructor parameter definition"),
        };
        let before = store.checkpoint();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(constructor),
            Err(TyperError::TypeNameNotFound {
                source: error_source,
                tree_index,
                ..
            }) if error_source == source && tree_index == type_tree.index()
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(
            *typer.store().symbols.info(constructor),
            SymbolInfo::Missing
        );
        assert_eq!(*typer.store().symbols.info(parameter), SymbolInfo::Missing);
    }

    #[test]
    fn constructor_rejects_malformed_precompleted_owner_info() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("class C");
        let (class_tree, template_tree) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "C" =>
                {
                    Some((tree, definition.rhs))
                }
                _ => None,
            })
            .unwrap();
        let class = index.symbol_at(source, class_tree).unwrap();
        let TreeKind::Template(template) = &parsed.ast.get(template_tree).kind else {
            panic!("expected a class template");
        };
        let constructor = index.symbol_at(source, template.constructor).unwrap();
        let malformed_info = store
            .types
            .alloc(Type::ClassInfo(dotty_core::types::ClassInfo {
                prefix: definitions.int,
                class,
                parents: Vec::new(),
                declarations: index.scope_of(class).unwrap(),
                self_type: None,
            }));
        store
            .symbols
            .set_info(class, SymbolInfo::Complete(malformed_info));
        let before = store.checkpoint();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(constructor),
            Err(TyperError::MalformedConstructorOwnerInfo {
                constructor: error_constructor,
                owner: error_owner,
                info
            }) if error_constructor == constructor && error_owner == class && info == malformed_info
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(
            *typer.store().symbols.info(constructor),
            SymbolInfo::Missing
        );
    }

    #[test]
    fn secondary_constructor_completes_its_own_parameter_clause() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C(value: Int) { def this(other: Int) = this(other) }");
        let constructor_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "<init>" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let constructor = index.symbol_at(source, constructor_tree).unwrap();
        let class = store.symbols.get(constructor).owner.unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(constructor).unwrap();

        let Type::Method(method_type) = typer.store().types.get(signature) else {
            panic!("expected the secondary constructor value clause");
        };
        assert_eq!(method_type.params.len(), 1);
        assert_eq!(method_type.params[0].ty, definitions.int);
        assert_eq!(type_symbol(typer.store(), method_type.result), class);
    }

    #[test]
    fn generic_secondary_constructor_is_deferred_without_constructor_type_params() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C[A](value: A) { def this(other: A) = this(other) }");
        let constructor_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "<init>" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let constructor = index.symbol_at(source, constructor_tree).unwrap();
        let owner = store.symbols.get(constructor).owner.unwrap();
        let before = store.checkpoint();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(constructor),
            Err(TyperError::GenericSecondaryConstructorDeferred {
                constructor: error_constructor,
                owner: error_owner
            }) if error_constructor == constructor && error_owner == owner
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(
            *typer.store().symbols.info(constructor),
            SymbolInfo::Missing
        );
    }

    #[test]
    fn secondary_constructor_type_parameters_are_explicitly_deferred() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C(value: Int) { def this[A](other: A) = this(0) }");
        let constructor_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "<init>" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let constructor = index.symbol_at(source, constructor_tree).unwrap();
        let before = store.checkpoint();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(constructor),
            Err(TyperError::SecondaryConstructorTypeParametersDeferred {
                constructor: error_constructor
            }) if error_constructor == constructor
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(
            *typer.store().symbols.info(constructor),
            SymbolInfo::Missing
        );
    }

    #[test]
    fn class_field_completes_its_declared_int_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value: Int = 1 }");
        let (field, _) = val_symbol(&parsed, &store, &index, source, "value");
        assert_eq!(store.symbols.get(field).kind, SymbolKind::Field);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info = typer.complete_symbol(field).unwrap();

        assert_eq!(info, definitions.int);
    }

    #[test]
    fn constructor_field_and_derived_parameter_complete_from_the_same_valdef() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C(val value: Int)");
        let (class_tree, class_definition) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "C" =>
                {
                    Some((tree, definition))
                }
                _ => None,
            })
            .unwrap();
        let class_symbol = index.symbol_at(source, class_tree).unwrap();
        let TreeKind::Template(template) = &parsed.ast.get(class_definition.rhs).kind else {
            panic!("class definition must own a template");
        };
        let TreeKind::DefDef(constructor) = &parsed.ast.get(template.constructor).kind else {
            panic!("class template must own a primary constructor");
        };
        let parameter_tree = constructor.value_param_clauses[0][0];
        let field = index.symbol_at(source, parameter_tree).unwrap();
        let constructor_symbol = index.symbol_at(source, template.constructor).unwrap();
        let derived_parameter = index
            .derived_symbol_at(constructor_symbol, source, parameter_tree)
            .unwrap();
        assert_eq!(store.symbols.get(field).kind, SymbolKind::Field);
        assert_eq!(
            store.symbols.get(derived_parameter).kind,
            SymbolKind::Parameter
        );
        assert_ne!(field, derived_parameter);
        assert_eq!(store.symbols.get(field).owner, Some(class_symbol));
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let field_info = typer.complete_symbol(field).unwrap();
        let parameter_info = typer.complete_symbol(derived_parameter).unwrap();

        assert_eq!(field_info, definitions.int);
        assert_eq!(parameter_info, definitions.int);
        assert_eq!(
            *typer.store().symbols.info(field),
            SymbolInfo::Complete(field_info)
        );
        assert_eq!(
            *typer.store().symbols.info(derived_parameter),
            SymbolInfo::Complete(parameter_info)
        );
    }

    #[test]
    fn extension_receiver_parameter_completes_from_its_source_valdef() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("extension (value: Int) def identity: Int = value");
        let extension = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) => {
                    Some((tree, extension))
                }
                _ => None,
            })
            .unwrap();
        let parameter_tree = extension.1.param_clauses[0][0];
        let method_tree = extension.1.methods[0];
        let method = index.symbol_at(source, method_tree).unwrap();
        let parameter = index
            .derived_symbol_at(method, source, parameter_tree)
            .unwrap();
        assert_eq!(store.symbols.get(parameter).kind, SymbolKind::Parameter);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info = typer.complete_symbol(parameter).unwrap();

        assert_eq!(info, definitions.int);
    }

    #[test]
    fn extension_signature_prepends_contextual_and_receiver_clauses() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "extension (using context: Int) (value: Int) def identity(argument: Int): Int = value",
        );
        let extension = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) => {
                    Some((tree, extension))
                }
                _ => None,
            })
            .unwrap();
        let method = index.symbol_at(source, extension.1.methods[0]).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let mut signature = typer.complete_symbol(method).unwrap();

        for expected_kind in [MethodKind::Contextual, MethodKind::Plain, MethodKind::Plain] {
            let Type::Method(method_type) = typer.store().types.get(signature) else {
                panic!("expected the next extension method clause");
            };
            assert_eq!(method_type.kind, expected_kind);
            assert_eq!(method_type.params.len(), 1);
            assert_eq!(method_type.params[0].ty, definitions.int);
            signature = method_type.result;
        }
        assert_eq!(signature, definitions.int);
    }

    #[test]
    fn extension_signature_binds_prefix_type_parameters_before_value_clauses() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("extension [A](value: A) def identity: A = value");
        let extension = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) => {
                    Some(extension)
                }
                _ => None,
            })
            .unwrap();
        let method = index.symbol_at(source, extension.methods[0]).unwrap();
        let type_parameter = index
            .derived_symbol_at(method, source, extension.param_clauses[0][0])
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let signature = typer.complete_symbol(method).unwrap();

        let Type::Poly(poly) = typer.store().types.get(signature) else {
            panic!("expected an extension type parameter binder");
        };
        let Type::Method(receiver) = typer.store().types.get(poly.result) else {
            panic!("expected an extension receiver clause after the type binder");
        };
        assert_eq!(receiver.params.len(), 1);
        assert!(matches!(
            typer.store().types.get(receiver.params[0].ty),
            Type::ParamRef { binder, index: 0 } if *binder == signature
        ));
        assert!(matches!(
            typer.store().types.get(receiver.result),
            Type::ParamRef { binder, index: 0 } if *binder == signature
        ));
        assert!(matches!(
            typer.store().types.get(poly.params[0].bounds),
            Type::Bounds { low, high }
                if *low == definitions.nothing_type && *high == definitions.any_type
        ));
        assert_eq!(
            typer.store().symbols.get(type_parameter).kind,
            SymbolKind::TypeParameter
        );
    }

    #[test]
    fn extension_methods_get_owner_specific_parameter_binders() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "extension (value: Int) { def first: Int = value; def second: Int = value }",
        );
        let extension = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) => {
                    Some(extension)
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(extension.methods.len(), 2);
        let first = index.symbol_at(source, extension.methods[0]).unwrap();
        let second = index.symbol_at(source, extension.methods[1]).unwrap();
        let receiver_tree = extension.param_clauses[0][0];
        let first_receiver = index
            .derived_symbol_at(first, source, receiver_tree)
            .unwrap();
        let second_receiver = index
            .derived_symbol_at(second, source, receiver_tree)
            .unwrap();
        assert_ne!(first_receiver, second_receiver);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let first_signature = typer.complete_symbol(first).unwrap();
        let second_signature = typer.complete_symbol(second).unwrap();

        for signature in [first_signature, second_signature] {
            let Type::Method(method_type) = typer.store().types.get(signature) else {
                panic!("expected an extension receiver clause");
            };
            assert_eq!(method_type.params.len(), 1);
            assert_eq!(method_type.params[0].ty, definitions.int);
            assert_eq!(method_type.result, definitions.int);
        }
    }

    #[test]
    fn right_associative_extension_signature_is_deferred_without_mutation() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("extension (value: Int) def +::(other: Int): Int = value");
        let method_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(_) => Some(tree),
                _ => None,
            })
            .unwrap();
        let method = index.symbol_at(source, method_tree).unwrap();
        let before = store.checkpoint();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(method),
            Err(TyperError::RightAssociativeExtensionDeferred {
                symbol,
                tree_index
            }) if symbol == method && tree_index == method_tree.index()
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(*typer.store().symbols.info(method), SymbolInfo::Missing);
    }

    #[test]
    fn by_name_parameter_completion_preserves_its_by_name_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def f(value: => Int): Unit = ()");
        let (parameter, _) = val_symbol(&parsed, &store, &index, source, "value");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let info = typer.complete_symbol(parameter).unwrap();

        assert!(matches!(
            typer.store().types.get(info),
            Type::ByName { result } if *result == definitions.int
        ));
    }

    #[test]
    fn method_type_parameter_shadows_a_class_type_parameter() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A] { def f[A](value: A): A = value }");
        let method_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "f" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let TreeKind::DefDef(definition) = &parsed.ast.get(method_tree).kind else {
            unreachable!()
        };
        let method_type_parameter = index.symbol_at(source, definition.type_params[0]).unwrap();
        let value_parameter = index
            .symbol_at(source, definition.value_param_clauses[0][0])
            .unwrap();
        let context = index.declaration_context_of(value_parameter).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.type_of_tpt(definition.tpt, context).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), method_type_parameter);
    }

    #[test]
    fn unresolved_type_name_error_has_position_and_rolls_back() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("val x: MissingType = 1");
        let (value, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let position = parsed.ast.get(type_tree).position;
        let before = store.checkpoint();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(value),
            Err(TyperError::TypeNameNotFound {
                source: found_source,
                tree_index,
                name,
                position: found_position,
            }) if found_source == source
                && tree_index == type_tree.index()
                && found_position == position
                && typer.store().names.resolve(name.text()) == "MissingType"
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(*typer.store().symbols.info(value), SymbolInfo::Missing);
        assert_eq!(typer.source_type_index().type_at(source, type_tree), None);
    }

    #[test]
    fn duplicate_type_candidates_return_a_positioned_ambiguity_error() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class T; class T; val x: T = 1");
        let (value, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let position = parsed.ast.get(type_tree).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(value),
            Err(TyperError::AmbiguousTypeName {
                source: found_source,
                tree_index,
                name,
                position: found_position,
            }) if found_source == source
                && tree_index == type_tree.index()
                && found_position == position
                && typer.store().names.resolve(name.text()) == "T"
        ));
    }

    #[test]
    fn object_name_alone_does_not_resolve_as_a_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("object O\nval x: O = 1");
        let (symbol, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let object = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                matches!(
                    node.kind,
                    TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::ModuleDef(_))
                )
                .then(|| index.symbol_at(source, tree).unwrap())
            })
            .unwrap();
        let position = parsed.ast.get(type_tree).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(symbol),
            Err(TyperError::WrongTypeNameKind {
                source: found_source,
                tree_index,
                name,
                symbol: found_symbol,
                kind: SymbolKind::Object,
                position: found_position,
            }) if found_source == source
                && tree_index == type_tree.index()
                && found_symbol == object
                && found_position == position
                && typer.store().names.resolve(name.text()) == "O"
                && name.is_type()
        ));
    }

    #[test]
    fn imported_object_alias_does_not_resolve_as_a_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("object Lib { object O }\nimport Lib.O as Alias\nval x: Alias = 1");
        let (symbol, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let object = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::ModuleDef(module)) =
                    &node.kind
                else {
                    return None;
                };
                (store.names.resolve(module.name.as_name().text()) == "O")
                    .then(|| index.symbol_at(source, tree).unwrap())
            })
            .unwrap();
        let position = parsed.ast.get(type_tree).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(symbol),
            Err(TyperError::WrongTypeNameKind {
                source: found_source,
                tree_index,
                name,
                symbol: found_symbol,
                kind: SymbolKind::Object,
                position: found_position,
            }) if found_source == source
                && tree_index == type_tree.index()
                && found_symbol == object
                && found_position == position
                && typer.store().names.resolve(name.text()) == "Alias"
                && name.is_type()
        ));
    }

    #[test]
    fn parenthesized_type_projects_the_inner_type_and_caches_both_trees() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("val x: (Int) = 1");
        let (symbol, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::Parens(parens)) =
            &parsed.ast.get(type_tree).kind
        else {
            panic!("expected the parser to preserve parentheses around the type")
        };
        let inner = parens.inner;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(symbol).unwrap();

        assert_eq!(projected, definitions.int);
        assert_eq!(
            typer.source_type_index().type_at(source, type_tree),
            Some(projected)
        );
        assert_eq!(
            typer.source_type_index().type_at(source, inner),
            Some(projected)
        );
    }

    #[test]
    fn nested_parenthesized_type_projects_and_caches_each_layer() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("val x: ((Int)) = 1");
        let (symbol, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let mut parenthesized = vec![type_tree];
        let mut inner = type_tree;
        while let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::Parens(parens)) =
            parsed.ast.get(inner).kind
        {
            inner = parens.inner;
            if matches!(
                parsed.ast.get(inner).kind,
                TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::Parens(_))
            ) {
                parenthesized.push(inner);
            }
        }
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(symbol).unwrap();

        assert_eq!(projected, definitions.int);
        for tree in parenthesized {
            assert_eq!(
                typer.source_type_index().type_at(source, tree),
                Some(projected)
            );
        }
    }

    #[test]
    fn applied_type_projects_constructor_and_arguments_in_source_order() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class F[A, B]; class A; class B; val x: F[A, B] = 1");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let mut expected = Vec::new();
        for (tree, node) in parsed.ast.iter() {
            let TreeKind::TypeDef(definition) = &node.kind else {
                continue;
            };
            let name = store.names.resolve(definition.name.as_name().text());
            if matches!(name, "F" | "A" | "B") {
                let symbol = index.symbol_at(source, tree).unwrap();
                if store.symbols.get(symbol).kind == SymbolKind::Class {
                    expected.push((name.to_owned(), symbol));
                }
            }
        }
        let find_symbol = |name| expected.iter().find(|(found, _)| found == name).unwrap().1;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        let Type::Applied { tycon, args } = typer.store().types.get(projected) else {
            panic!("expected an applied type")
        };
        assert_eq!(type_symbol(typer.store(), *tycon), find_symbol("F"));
        assert_eq!(args.len(), 2);
        assert_eq!(type_symbol(typer.store(), args[0]), find_symbol("A"));
        assert_eq!(type_symbol(typer.store(), args[1]), find_symbol("B"));
    }

    #[test]
    fn by_name_type_projects_its_result_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def f(x: => Int): Unit = ()");
        let (parameter, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let context = index.declaration_context_of(parameter).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.type_of_tpt(type_tree, context).unwrap();

        let Type::ByName { result } = typer.store().types.get(projected) else {
            panic!("expected a by-name type")
        };
        assert_eq!(*result, definitions.int);
    }

    #[test]
    fn failed_applied_type_projection_rolls_back_types_and_cache_entries() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class F[A]; class A; val x: F[A, Missing] = 1");
        let (value, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let TreeKind::AppliedTypeTree(applied) = &parsed.ast.get(type_tree).kind else {
            panic!("expected an applied type tree")
        };
        let constructor = applied.tpt;
        let first_argument = applied.args[0];
        let context = index.declaration_context_of(value).unwrap();
        let before = store.checkpoint();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_of_tpt(type_tree, context),
            Err(TyperError::TypeNameNotFound { name, .. })
                if typer.store().names.resolve(name.text()) == "Missing"
        ));

        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(typer.source_type_index().type_at(source, type_tree), None);
        assert_eq!(typer.source_type_index().type_at(source, constructor), None);
        assert_eq!(
            typer.source_type_index().type_at(source, first_argument),
            None
        );
    }

    #[test]
    fn missing_declared_type_is_not_inferred_from_the_value_rhs() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("val x = 1");
        let (symbol, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let position = parsed.ast.get(type_tree).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(symbol),
            Err(TyperError::MissingDeclaredType {
                source: found_source,
                tree_index,
                position: found_position,
            }) if found_source == source
                && tree_index == type_tree.index()
                && found_position == position
        ));
        assert_eq!(typer.source_type_index().type_at(source, type_tree), None);
    }

    #[test]
    fn nested_type_lookup_walks_to_the_enclosing_declaration_context() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Outer { class Inner { val x: Outer = 1 } }");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let outer = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Outer" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), outer);
    }

    #[test]
    fn member_type_reference_uses_the_enclosing_class_this_type_prefix() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Outer { class Inner; val x: Inner = 1 }");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let inner = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Inner" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let owner = store.symbols.get(inner).owner.unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        let Type::TypeRef { prefix, target } = typer.store().types.get(projected) else {
            panic!("expected a TypeRef for the member type")
        };
        assert_eq!(*target, TypeRefTarget::Symbol(inner));
        assert_eq!(
            typer.store().types.get(*prefix),
            &Type::ThisType { class: owner }
        );
    }

    #[test]
    fn member_type_reference_in_an_object_uses_its_module_class_prefix() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("object O { class Inner; val x: Inner = 1 }");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let inner = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Inner" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let owner = store.symbols.get(inner).owner.unwrap();
        assert_eq!(store.symbols.get(owner).kind, SymbolKind::ModuleClass);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        let Type::TypeRef { prefix, target } = typer.store().types.get(projected) else {
            panic!("expected a TypeRef for the member type")
        };
        assert_eq!(*target, TypeRefTarget::Symbol(inner));
        assert_eq!(
            typer.store().types.get(*prefix),
            &Type::ThisType { class: owner }
        );
    }

    #[test]
    fn qualified_inner_type_resolves_through_source_class_scope() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Outer { class Inner }; val x: Outer.Inner = 1");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let mut outer = None;
        let mut inner = None;
        for (tree, node) in parsed.ast.iter() {
            if let TreeKind::TypeDef(definition) = &node.kind {
                match store.names.resolve(definition.name.as_name().text()) {
                    "Outer" => outer = index.symbol_at(source, tree),
                    "Inner" => inner = index.symbol_at(source, tree),
                    _ => {}
                }
            }
        }
        let outer = outer.unwrap();
        let inner = inner.unwrap();
        assert_eq!(*store.symbols.info(outer), SymbolInfo::Missing);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), inner);
        let Type::TypeRef { prefix, .. } = typer.store().types.get(projected) else {
            panic!("expected a qualified type reference")
        };
        assert_eq!(
            typer.store().types.get(*prefix),
            &Type::ThisType { class: outer }
        );
        assert_eq!(*typer.store().symbols.info(outer), SymbolInfo::Missing);
    }

    #[test]
    fn package_qualified_type_uses_the_canonical_package_prefix() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("package p { class C }; package use { val x: p.C = 1 }");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let class = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "C" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let package = packages.symbol(&["p"]).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), class);
        let Type::TypeRef { prefix, .. } = typer.store().types.get(projected) else {
            panic!("expected a package-qualified type reference")
        };
        let Type::TypeRef {
            prefix: package_prefix,
            target: TypeRefTarget::Symbol(package_target),
        } = typer.store().types.get(*prefix)
        else {
            panic!("expected the canonical package reference prefix")
        };
        assert_eq!(*package_prefix, definitions.no_prefix);
        assert_eq!(*package_target, package);
    }

    #[test]
    fn external_member_resolver_runs_only_after_source_lookup_misses() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Outer { class Local }; val local: Outer.Local = 1; val ext: Outer.External = 1",
        );
        let (local, _) = val_symbol(&parsed, &store, &index, source, "local");
        let (external, _) = val_symbol(&parsed, &store, &index, source, "ext");
        let local_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Local" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let external_type = symbol(&mut store, SymbolKind::Class, SymbolInfo::Missing);
        let requests = Rc::new(RefCell::new(Vec::new()));
        let package_requests = Rc::new(RefCell::new(Vec::new()));
        let resolver = ScriptedResolver {
            member: Some(external_type),
            package: None,
            member_requests: Rc::clone(&requests),
            package_requests,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        )
        .with_resolver(Box::new(resolver));

        let local_projected = typer.complete_symbol(local).unwrap();
        let external_projected = typer.complete_symbol(external).unwrap();

        assert_eq!(type_symbol(typer.store(), local_projected), local_type);
        assert_eq!(
            type_symbol(typer.store(), external_projected),
            external_type
        );
        assert_eq!(requests.borrow().len(), 1);
        assert_eq!(
            typer.store().names.resolve(requests.borrow()[0].text()),
            "External"
        );
    }

    #[test]
    fn external_package_and_member_resolvers_supply_unknown_qualifiers() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("import p.C\nval x: C = 1");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let package_name = store.names.intern("p");
        let package = store.symbols.alloc(dotty_core::Symbol {
            name: Name::new(package_name, Namespace::Term),
            owner: None,
            kind: SymbolKind::Package,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let external_type = symbol(&mut store, SymbolKind::Class, SymbolInfo::Missing);
        store.symbols.get_mut(external_type).owner = Some(package);
        let member_requests = Rc::new(RefCell::new(Vec::new()));
        let package_requests = Rc::new(RefCell::new(Vec::new()));
        let resolver = ScriptedResolver {
            member: Some(external_type),
            package: Some(package),
            member_requests: Rc::clone(&member_requests),
            package_requests: Rc::clone(&package_requests),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        )
        .with_resolver(Box::new(resolver));

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), external_type);
        assert_eq!(*package_requests.borrow(), vec![vec!["p".to_owned()]]);
        assert_eq!(member_requests.borrow().len(), 1);
        let Type::TypeRef {
            target: TypeRefTarget::Symbol(found_package),
            ..
        } = typer
            .store()
            .types
            .get(match typer.store().types.get(projected) {
                Type::TypeRef { prefix, .. } => *prefix,
                _ => panic!("expected a type reference"),
            })
        else {
            panic!("expected the external package prefix")
        };
        assert_eq!(*found_package, package);
    }

    #[test]
    fn innermost_type_declaration_shadows_an_enclosing_name() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class T; class Outer { class T; val x: T = 1 }");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let outer = store.symbols.get(value).owner.unwrap();
        let inner_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "T" =>
                {
                    let symbol = index.symbol_at(source, tree)?;
                    (store.symbols.get(symbol).owner == Some(outer)).then_some(symbol)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), inner_type);
    }

    #[test]
    fn inner_term_name_does_not_shadow_an_enclosing_type_name() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class T; class Outer { object T; val x: T = 1 }");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let enclosing_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "T" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), enclosing_type);
    }

    #[test]
    fn explicit_package_import_resolves_a_type_name() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class Imported }\npackage app { import lib.Imported; val x: Imported = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let imported_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Imported" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), imported_type);
    }

    #[test]
    fn wildcard_package_import_resolves_a_type_name() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class Imported }\npackage app { import lib.*; val x: Imported = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let imported_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Imported" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), imported_type);
    }

    #[test]
    fn renamed_package_import_resolves_the_alias() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class Imported }\npackage app { import lib.{Imported as Alias}; val x: Alias = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let imported_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Imported" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), imported_type);
    }

    #[test]
    fn imported_type_alias_resolves_without_completing_the_alias() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { type Alias = Int }; package app { import lib.Alias; val x: Alias = 1; val y: lib.Alias = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let (qualified_value, _) = val_symbol(&parsed, &store, &index, source, "y");
        let alias = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Alias" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(store.symbols.get(alias).kind, SymbolKind::TypeAlias);
        assert_eq!(*store.symbols.info(alias), SymbolInfo::Missing);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), alias);
        let qualified = typer.complete_symbol(qualified_value).unwrap();
        assert_eq!(type_symbol(typer.store(), qualified), alias);
        assert_eq!(*typer.store().symbols.info(alias), SymbolInfo::Missing);
    }

    #[test]
    fn renamed_import_does_not_expose_the_original_name() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class Imported }; package app { import lib.{Imported as Alias}; val alias: Alias = 1; val original: Imported = 1 }",
        );
        let (alias, _) = val_symbol(&parsed, &store, &index, source, "alias");
        let (original, original_tpt) = val_symbol(&parsed, &store, &index, source, "original");
        let imported_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Imported" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let original_position = parsed.ast.get(original_tpt).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(alias).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), imported_type);
        assert!(matches!(
            typer.complete_symbol(original),
            Err(TyperError::TypeNameNotFound {
                name,
                tree_index,
                position,
                ..
            }) if typer.store().names.resolve(name.text()) == "Imported"
                && tree_index == original_tpt.index()
                && position == original_position
        ));
    }

    #[test]
    fn renamed_selector_hides_its_original_from_the_same_wildcard_import() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class Imported }; package app { import lib.{Imported as Alias, *}; val alias: Alias = 1; val original: Imported = 1 }",
        );
        let (alias, _) = val_symbol(&parsed, &store, &index, source, "alias");
        let (original, original_tpt) = val_symbol(&parsed, &store, &index, source, "original");
        let imported_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Imported" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(alias).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), imported_type);
        assert!(matches!(
            typer.complete_symbol(original),
            Err(TyperError::TypeNameNotFound { tree_index, .. })
                if tree_index == original_tpt.index()
        ));
    }

    #[test]
    fn hide_selector_excludes_a_name_from_the_same_wildcard_import() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class Hidden; class Visible }; package app { import lib.{Hidden as _, *}; val x: Hidden = 1 }",
        );
        let (value, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let position = parsed.ast.get(type_tree).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(value),
            Err(TyperError::TypeNameNotFound {
                name,
                tree_index,
                position: found_position,
                ..
            }) if typer.store().names.resolve(name.text()) == "Hidden"
                && tree_index == type_tree.index()
                && found_position == position
        ));
    }

    #[test]
    fn explicit_import_precedes_a_later_wildcard_import() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package first { class C }; package second { class C }; package app { import first.C; import second.*; val x: C = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let expected = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "C"
                        && store
                            .symbols
                            .get(index.symbol_at(source, tree).unwrap())
                            .owner
                            == packages.symbol(&["first"]) =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), expected);
    }

    #[test]
    fn conflicting_wildcard_imports_in_one_scope_are_ambiguous() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package first { class C }; package second { class C }; package app { import first.*; import second.*; val x: C = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let (_, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let position = parsed.ast.get(type_tree).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(value),
            Err(TyperError::AmbiguousTypeName {
                name,
                tree_index,
                position: found_position,
                ..
            }) if typer.store().names.resolve(name.text()) == "C"
                && tree_index == type_tree.index()
                && found_position == position
        ));
    }

    #[test]
    fn conflicting_explicit_imports_in_one_scope_are_ambiguous() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package first { class C }; package second { class C }; package app { import first.C; import second.C; val x: C = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let (_, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let position = parsed.ast.get(type_tree).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(value),
            Err(TyperError::AmbiguousTypeName {
                name,
                tree_index,
                position: found_position,
                ..
            }) if typer.store().names.resolve(name.text()) == "C"
                && tree_index == type_tree.index()
                && found_position == position
        ));
    }

    #[test]
    fn repeated_import_of_the_same_type_in_one_scope_is_not_ambiguous() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class C }; package app { import lib.C; import lib.C; val x: C = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let expected = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "C" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), expected);
    }

    #[test]
    fn wildcard_imports_of_aliases_to_the_same_type_are_not_ambiguous() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package base { class C }; package first { type T = base.C }; package second { type T = base.C }; package app { import first.*; import second.*; val x: T = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let aliases: Vec<_> = parsed
            .ast
            .iter()
            .filter_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "T" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .collect();
        assert_eq!(aliases.len(), 2);
        let base_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "C" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        for alias in aliases.iter().copied() {
            let underlying = store.types.alloc(Type::TypeRef {
                prefix: definitions.no_prefix,
                target: TypeRefTarget::Symbol(base_type),
            });
            let info = store
                .types
                .alloc(Type::AliasingBounds { alias: underlying });
            store.symbols.set_info(alias, SymbolInfo::Complete(info));
        }
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert!(aliases.contains(&type_symbol(typer.store(), projected)));
    }

    #[test]
    fn explicit_import_precedes_package_type_from_another_source_unit() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let (package_file, package_index) = parse_and_name_unit(
            "package p { class T }",
            SourceId::from_index(21),
            &mut store,
            &mut packages,
        );
        let source = SourceId::from_index(22);
        let (parsed, index) = parse_and_name_unit(
            "package q { class T }; package p { import q.T; val x: T = 1 }",
            source,
            &mut store,
            &mut packages,
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let expected = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "T"
                        && store
                            .symbols
                            .get(index.symbol_at(source, tree).unwrap())
                            .owner
                            == packages.symbol(&["q"]) =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let package_type = package_file
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "T" =>
                {
                    package_index.symbol_at(SourceId::from_index(21), tree)
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(
            store.symbols.get(package_type).origin,
            SymbolOrigin::Source(SourceId::from_index(21))
        );
        assert_eq!(
            store.symbols.get(package_type).owner,
            packages.symbol(&["p"])
        );
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), expected);
    }

    #[test]
    fn current_unit_package_type_precedes_an_explicit_import() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package first { class T }; package second { class T }; package first { import second.T; val x: T = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let expected = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "T"
                        && store
                            .symbols
                            .get(index.symbol_at(source, tree).unwrap())
                            .owner
                            == packages.symbol(&["first"]) =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), expected);
    }

    #[test]
    fn duplicate_alias_candidates_in_one_import_are_ambiguous() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class C; class D }; package app { import lib.{C as Alias, D as Alias}; val x: Alias = 1 }",
        );
        let (value, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let position = parsed.ast.get(type_tree).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(value),
            Err(TyperError::AmbiguousTypeName {
                name,
                tree_index,
                position: found_position,
                ..
            }) if typer.store().names.resolve(name.text()) == "Alias"
                && tree_index == type_tree.index()
                && found_position == position
        ));
    }

    #[test]
    fn lexical_type_declaration_shadows_an_explicit_import() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class C }; package app { import lib.C; class C; val x: C = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let app_class = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "C"
                        && store
                            .symbols
                            .get(index.symbol_at(source, tree).unwrap())
                            .owner
                            == packages.symbol(&["app"]) =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), app_class);
    }

    #[test]
    fn imports_apply_only_to_declarations_after_the_import() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class C }; package app { val before: C = 1; import lib.C; val after: C = 1 }",
        );
        let (before, before_tpt) = val_symbol(&parsed, &store, &index, source, "before");
        let (after, _) = val_symbol(&parsed, &store, &index, source, "after");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(before),
            Err(TyperError::TypeNameNotFound { name, tree_index, .. })
                if typer.store().names.resolve(name.text()) == "C"
                    && tree_index == before_tpt.index()
        ));
        assert!(typer.complete_symbol(after).is_ok());
    }

    #[test]
    fn failed_qualified_type_projection_rolls_back_prefix_and_cache() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Outer; val x: Outer.Missing = 1");
        let (value, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let context = index.declaration_context_of(value).unwrap();
        let before = store.checkpoint();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_of_tpt(type_tree, context),
            Err(TyperError::TypeNameNotFound { name, .. })
                if typer.store().names.resolve(name.text()) == "Missing"
        ));

        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(typer.source_type_index().type_at(source, type_tree), None);
    }

    #[test]
    fn failed_imported_type_projection_rolls_back_prefix_and_cache() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class Existing }; package app { import lib.*; val x: Missing = 1 }",
        );
        let (value, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let context = index.declaration_context_of(value).unwrap();
        let before = store.checkpoint();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_of_tpt(type_tree, context),
            Err(TyperError::TypeNameNotFound { name, .. })
                if typer.store().names.resolve(name.text()) == "Missing"
        ));

        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(typer.source_type_index().type_at(source, type_tree), None);
    }

    #[test]
    fn later_import_qualifier_can_use_an_earlier_type_alias() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class Imported { class Nested } }\npackage app { import lib.Imported as Alias; import Alias.Nested; val x: Nested = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let nested_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Nested" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), nested_type);
    }

    #[test]
    fn later_import_qualifier_can_use_an_earlier_object_alias() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "object Lib { object Nested { class X } }\nimport Lib.Nested as Alias\nimport Alias.X\nval x: X = 1",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let nested_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "X" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), nested_type);
    }

    #[test]
    fn wildcard_import_from_an_object_uses_its_namer_module_class_scope() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("object Lib { class Imported }\nimport Lib.*\nval x: Imported = 1");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let imported_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Imported" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), imported_type);
    }

    #[test]
    fn primitive_int_annotation_uses_the_bootstrapped_type() {
        let (ty, definitions) = complete_builtin_annotation("Int");

        assert_eq!(ty, definitions.int);
    }

    #[test]
    fn primitive_boolean_annotation_uses_the_bootstrapped_type() {
        let (ty, definitions) = complete_builtin_annotation("Boolean");

        assert_eq!(ty, definitions.boolean);
    }

    #[test]
    fn lexical_type_named_int_shadows_the_builtin() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Int; val x: Int = 1");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let local_int = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Int" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), local_int);
    }

    #[test]
    fn unit_annotation_uses_the_bootstrapped_type() {
        let (ty, definitions) = complete_builtin_annotation("Unit");

        assert_eq!(ty, definitions.unit);
    }

    #[test]
    fn any_annotation_uses_the_bootstrapped_type() {
        let (ty, definitions) = complete_builtin_annotation("Any");

        assert_eq!(ty, definitions.any_type);
    }

    #[test]
    fn nothing_annotation_uses_the_bootstrapped_type() {
        let (ty, definitions) = complete_builtin_annotation("Nothing");

        assert_eq!(ty, definitions.nothing_type);
    }

    #[test]
    fn method_without_result_type_is_deferred_without_store_changes() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def method = 1");
        let before = store.checkpoint();
        let method = index
            .symbol_at(
                source,
                parsed
                    .ast
                    .iter()
                    .find_map(|(id, node)| matches!(node.kind, TreeKind::DefDef(_)).then_some(id))
                    .unwrap(),
            )
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let method_tree = index.definition_of(method).unwrap();
        let SourceDefinition::Canonical { tree, .. } = method_tree else {
            panic!("source method should be canonical");
        };
        let TreeKind::DefDef(definition) = &parsed.ast.get(tree).kind else {
            panic!("source method should use a DefDef");
        };
        assert!(matches!(
            typer.complete_symbol(method),
            Err(TyperError::InferredMethodResultDeferred { symbol, tree_index })
                if symbol == method && tree_index == definition.tpt.index()
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(*typer.store().symbols.info(method), SymbolInfo::Missing);
    }

    #[test]
    fn type_projection_rejects_a_context_from_another_naming_index() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C\nval x: C = 1");
        let (symbol, tpt) = val_symbol(&parsed, &store, &index, source, "x");
        let context = index.declaration_context_of(symbol).unwrap();
        let foreign_index = SourceSemanticIndex::new();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &foreign_index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_of_tpt(tpt, context),
            Err(TyperError::SourceContextMissing { tree_index, .. }) if tree_index == tpt.index()
        ));
    }

    #[test]
    fn completion_does_not_find_a_definition_by_scanning_the_ast() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("val x: Int = 1");
        let (symbol, _) = val_symbol(&parsed, &store, &index, source, "x");
        let empty_index = SourceSemanticIndex::new();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &empty_index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(symbol),
            Err(TyperError::SourceProvenanceMissing { symbol: found }) if found == symbol
        ));
    }

    #[test]
    fn a_later_failed_completion_keeps_an_earlier_success() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C\nval x: C = 1\ndef method: Missing = 1");
        let (value, value_tpt) = val_symbol(&parsed, &store, &index, source, "x");
        let method_tree = parsed
            .ast
            .iter()
            .find_map(|(id, node)| {
                matches!(&node.kind, TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "method")
                .then_some(id)
            })
            .unwrap();
        let method = index.symbol_at(source, method_tree).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let value_type = typer.complete_symbol(value).unwrap();
        assert!(matches!(
            typer.complete_symbol(method),
            Err(TyperError::TypeNameNotFound { name, .. })
                if typer.store().names.resolve(name.text()) == "Missing"
        ));

        assert_eq!(
            *typer.store().symbols.info(value),
            SymbolInfo::Complete(value_type)
        );
        assert_eq!(*typer.store().symbols.info(method), SymbolInfo::Missing);
        assert_eq!(
            typer.source_type_index().type_at(source, value_tpt),
            Some(value_type)
        );
    }

    #[test]
    fn object_term_and_derived_module_class_keep_distinct_dispatch() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("object O");
        let (tree, object) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                matches!(
                    node.kind,
                    TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::ModuleDef(_))
                )
                .then(|| (tree, index.symbol_at(source, tree).unwrap()))
            })
            .unwrap();
        let owner = store.symbols.get(object).owner.unwrap();
        let module_class = index.derived_symbol_at(owner, source, tree).unwrap();
        assert_eq!(store.symbols.get(object).kind, SymbolKind::Object);
        assert_eq!(
            store.symbols.get(module_class).kind,
            SymbolKind::ModuleClass
        );
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(object),
            Err(TyperError::UnsupportedSymbolCompletion {
                kind: SymbolKind::Object,
                ..
            })
        ));
        let scope = index.scope_of(module_class).unwrap();
        let info_id = typer.complete_symbol(module_class).unwrap();
        let Type::ClassInfo(info) = typer.store().types.get(info_id) else {
            panic!("expected ModuleClass ClassInfo");
        };
        assert_eq!(info.class, module_class);
        assert_eq!(info.declarations, scope);
        assert_eq!(info.parents, vec![definitions.object_type]);
    }

    #[test]
    fn failed_transaction_restores_store_symbol_info_and_type_cache() {
        let (_unused_arena, mut store, packages, definitions) = setup();
        let source = SourceId::from_index(7);
        let tree = {
            let mut arena = AstArena::<Untyped>::new();
            let tree = arena.alloc(dotty_core::Tree {
                kind: TreeKind::TypeTree(dotty_core::ast::TypeTree),
                position: None,
                ty: (),
            });
            // The transaction only indexes an ID; the caller-owned arena
            // below must outlive the driver, so return the arena with the ID.
            (arena, tree)
        };
        let (arena, tree_id) = tree;
        let index = SourceSemanticIndex::new();
        let symbol = symbol(&mut store, SymbolKind::Value, SymbolInfo::Missing);
        let before = store.checkpoint();
        let attempted_type = store.types.alloc(Type::NoType);
        store.rollback_to(before);
        let mut typer =
            SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages);

        let result: Result<(), TyperError> = typer.run_atomic(|typer, journal| {
            let new_type = typer.store.types.alloc(Type::Error(dotty_core::ErrorType {
                message: typer.store.names.intern("temporary"),
            }));
            journal.push((symbol, *typer.store.symbols.info(symbol)));
            typer
                .store
                .symbols
                .set_info(symbol, SymbolInfo::Complete(new_type));
            typer.type_index.insert(source, tree_id, new_type).unwrap();
            Err(TyperError::UnsupportedSymbolCompletion {
                symbol,
                kind: SymbolKind::Value,
            })
        });

        assert!(matches!(
            result,
            Err(TyperError::UnsupportedSymbolCompletion { .. })
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(*typer.store().symbols.info(symbol), SymbolInfo::Missing);
        assert_eq!(typer.source_type_index().type_at(source, tree_id), None);
        drop(typer);
        assert_eq!(store.types.alloc(Type::NoType), attempted_type);
    }

    #[test]
    fn boolean_literal_becomes_a_typed_literal_with_the_boolean_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value: Boolean = true }");
        let (symbol, _, rhs) = val_definition_and_rhs(&parsed, &store, &index, source, "value");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(symbol).unwrap(),
            owner: store.symbols.get(symbol).owner.unwrap(),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();

        assert_eq!(
            typer.typed_ast().get(typed).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: dotty_core::Constant::Boolean(true)
            })
        );
        let own_type = typer.typed_ast().get(typed).ty;
        assert!(matches!(
            typer.store().types.get(own_type),
            Type::Constant(dotty_core::Constant::Boolean(true))
        ));
        assert_eq!(
            typer.widen_expression_type(own_type).unwrap(),
            definitions.boolean
        );
    }

    #[test]
    fn character_long_float_double_and_unit_literals_use_canonical_types() {
        let cases = [
            (
                "class C { val value = 'x' }",
                dotty_core::Constant::Char('x' as u16),
                0_u8,
            ),
            (
                "class C { val value = 42L }",
                dotty_core::Constant::Long(42),
                1,
            ),
            (
                "class C { val value = 1.25f }",
                dotty_core::Constant::float(1.25),
                2,
            ),
            (
                "class C { val value = 2.5d }",
                dotty_core::Constant::double(2.5),
                3,
            ),
            ("class C { val value = () }", dotty_core::Constant::Unit, 4),
        ];

        for (source_text, expected_value, type_case) in cases {
            let (value, own_type, ty, definitions) = type_value_rhs(source_text);
            let expected_type = match type_case {
                0 => definitions.char,
                1 => definitions.long,
                2 => definitions.float,
                3 => definitions.double,
                _ => definitions.unit,
            };
            assert_eq!(value, expected_value, "{source_text}");
            assert_eq!(own_type, Type::Constant(value.clone()), "{source_text}");
            assert_eq!(ty, expected_type, "{source_text}");
        }
    }

    #[test]
    fn byte_constant_uses_the_byte_type() {
        let (value, own_type, ty, definitions) =
            type_synthetic_literal(dotty_core::Constant::Byte(7));

        assert_eq!(value, dotty_core::Constant::Byte(7));
        assert_eq!(own_type, Type::Constant(value.clone()));
        assert_eq!(ty, definitions.byte);
    }

    #[test]
    fn short_constant_uses_the_short_type() {
        let (value, own_type, ty, definitions) =
            type_synthetic_literal(dotty_core::Constant::Short(7));

        assert_eq!(value, dotty_core::Constant::Short(7));
        assert_eq!(own_type, Type::Constant(value.clone()));
        assert_eq!(ty, definitions.short);
    }

    #[test]
    fn bare_this_gets_the_nearest_enclosing_class_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value = this }");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let (symbol, _, rhs) = val_definition_and_rhs(&parsed, &store, &index, source, "value");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(symbol).unwrap(),
            owner: symbol,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();

        assert!(matches!(
            typer.typed_ast().get(typed).kind,
            TreeKind::This(dotty_core::ast::This { qual: None })
        ));
        assert_eq!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            &Type::ThisType { class }
        );
    }

    #[test]
    fn qualified_this_resolves_an_enclosing_class_owner() {
        let (mut parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Outer { class Inner { val value = this } }");
        let outer = class_symbol(&parsed, &store, &index, source, "Outer");
        let (symbol, _, rhs) = val_definition_and_rhs(&parsed, &store, &index, source, "value");
        let qualifier = store.symbols.get(outer).name;
        parsed.ast.get_mut(rhs).kind = TreeKind::This(dotty_core::ast::This {
            qual: Some(qualifier),
        });
        let context = ExpressionContext {
            lexical: index.declaration_context_of(symbol).unwrap(),
            owner: symbol,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();

        assert_eq!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            &Type::ThisType { class: outer }
        );
    }

    #[test]
    fn qualified_this_rejects_a_non_enclosing_class() {
        let (mut parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Other; class C { val value = this }");
        let other = class_symbol(&parsed, &store, &index, source, "Other");
        let (symbol, _, rhs) = val_definition_and_rhs(&parsed, &store, &index, source, "value");
        let qualifier = store.symbols.get(other).name;
        parsed.ast.get_mut(rhs).kind = TreeKind::This(dotty_core::ast::This {
            qual: Some(qualifier),
        });
        let context = ExpressionContext {
            lexical: index.declaration_context_of(symbol).unwrap(),
            owner: symbol,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ThisOwnerNotEnclosing { .. })
        ));
    }

    #[test]
    fn method_parameter_identifier_gets_its_declared_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def method(param: Int): Int = param }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "method");
        let (parameter, _) = val_symbol(&parsed, &store, &index, source, "param");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();

        assert!(matches!(
            typer.typed_ast().get(typed).kind,
            TreeKind::Ident(_)
        ));
        let Type::TermRef { prefix, target } =
            typer.store().types.get(typer.typed_ast().get(typed).ty)
        else {
            panic!("parameter identifier should retain a term reference")
        };
        assert_eq!(*prefix, definitions.no_prefix);
        assert_eq!(*target, TermRefTarget::Symbol(parameter));
        assert_eq!(
            typer
                .widen_expression_type(typer.typed_ast().get(typed).ty)
                .unwrap(),
            definitions.int
        );
    }

    #[test]
    fn field_identifier_completes_and_uses_its_declared_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val field: Int = 1; def method: Int = field }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "method");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let field = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::ValDef(definition) = &node.kind else {
                    return None;
                };
                (store.names.resolve(definition.name.as_name().text()) == "field")
                    .then(|| index.symbol_at(source, tree).unwrap())
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();

        assert!(matches!(
            typer.typed_ast().get(typed).kind,
            TreeKind::Ident(_)
        ));
        let Type::TermRef { prefix, target } =
            typer.store().types.get(typer.typed_ast().get(typed).ty)
        else {
            panic!("field identifier should retain a term reference")
        };
        assert_eq!(*target, TermRefTarget::Symbol(field));
        assert!(
            matches!(typer.store().types.get(*prefix), Type::ThisType { class: this } if *this == class)
        );
        assert_eq!(
            typer
                .widen_expression_type(typer.typed_ast().get(typed).ty)
                .unwrap(),
            definitions.int
        );
    }

    #[test]
    fn inherited_field_identifier_uses_current_this_prefix() {
        let source_text =
            "class Parent { val value: Int }; class Child extends Parent { def use: Int = value }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let child = class_symbol(&parsed, &store, &index, source, "Child");
        let field = val_symbol(&parsed, &store, &index, source, "value").0;
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let lexical = index.declaration_context_of(method).unwrap();
        let scope = index.source_context(lexical).lexical_scope;
        let field_name = store.symbols.get(field).name;
        store.scopes.get_mut(scope).enter(field_name, field);
        let context = ExpressionContext {
            lexical,
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) }
                if matches!(typer.store().types.get(*prefix), Type::ThisType { class } if *class == child)
                    && *symbol == field
        ));
        let widened = typer
            .widen_expression_type(typer.typed_ast().get(typed).ty)
            .unwrap();
        assert_eq!(widened, definitions.int);
    }

    #[test]
    fn nested_class_identifier_uses_enclosing_generic_this_prefix() {
        let source_text = "class Outer[A] { val value: A; class Inner { def use: A = value } }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let outer = class_symbol(&parsed, &store, &index, source, "Outer");
        let field = val_symbol(&parsed, &store, &index, source, "value").0;
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();
        let term_type = typer.typed_ast().get(typed).ty;
        assert!(
            matches!(
                typer.store().types.get(term_type),
                Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) }
                    if *symbol == field
                        && matches!(typer.store().types.get(*prefix), Type::ThisType { class } if *class == outer)
            ),
            "nested member should retain its enclosing class path"
        );
        let widened = typer.widen_expression_type(term_type).unwrap();
        let SymbolInfo::Complete(raw) = *typer.store().symbols.info(field) else {
            panic!("source field should have complete info");
        };
        assert_ne!(
            widened,
            raw,
            "widening should adapt the outer member through Outer[A]: {:?} vs {:?}",
            typer.store().types.get(widened),
            typer.store().types.get(raw)
        );
    }

    #[test]
    fn top_level_value_identifier_uses_its_declared_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Int = global }");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let lexical = index.declaration_context_of(method).unwrap();
        let name = Name::new(store.names.intern("global"), Namespace::Term);
        let global = store.symbols.alloc(dotty_core::Symbol {
            name,
            owner: Some(class),
            kind: SymbolKind::Value,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Complete(definitions.int),
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let scope = index.source_context(lexical).lexical_scope;
        store.scopes.get_mut(scope).enter(name, global);
        let context = ExpressionContext {
            lexical,
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();

        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } if *symbol == global
        ));
        assert_eq!(
            typer
                .widen_expression_type(typer.typed_ast().get(typed).ty)
                .unwrap(),
            definitions.int
        );
    }

    #[test]
    fn variable_selection_uses_its_declared_type() {
        let source_text = "class C { var value: Int = 1; def use: Int = this.value }";
        let (kind, _, widened, _, definitions) = type_method_rhs(source_text, "use").unwrap();

        assert!(matches!(kind, TreeKind::Select(_)));
        assert_eq!(widened, definitions.int);
    }

    #[test]
    fn explicit_imported_field_identifier_retains_its_symbol_reference() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Lib { val item: Int = 1 }; class C { import Lib.item; def use: Int = item }",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let imported = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::ValDef(definition) = &node.kind else {
                    return None;
                };
                (store.names.resolve(definition.name.as_name().text()) == "item")
                    .then(|| index.symbol_at(source, tree).unwrap())
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();

        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) }
                if *prefix == definitions.no_prefix && *symbol == imported
        ));
        assert_eq!(
            typer
                .widen_expression_type(typer.typed_ast().get(typed).ty)
                .unwrap(),
            definitions.int
        );
    }

    #[test]
    fn wildcard_imported_field_identifier_retains_its_symbol_reference() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Lib { val item: Int = 1 }; class C { import Lib.*; def use: Int = item }",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let imported = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::ValDef(definition) = &node.kind else {
                    return None;
                };
                (store.names.resolve(definition.name.as_name().text()) == "item")
                    .then(|| index.symbol_at(source, tree).unwrap())
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();

        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) }
                if *prefix == definitions.no_prefix && *symbol == imported
        ));
        assert_eq!(
            typer
                .widen_expression_type(typer.typed_ast().get(typed).ty)
                .unwrap(),
            definitions.int
        );
    }

    #[test]
    fn direct_field_selection_produces_a_typed_select() {
        let (kind, _, widened, _, definitions) = type_method_rhs(
            "class C { val value: Int = 1; def use: Int = this.value }",
            "use",
        )
        .unwrap();

        assert!(matches!(kind, TreeKind::Select(_)));
        assert_eq!(widened, definitions.int);
    }

    #[test]
    fn this_selection_keeps_the_exact_field_and_this_type_prefix() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value: Int = 1; def use: Int = this.value }");
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let field = val_symbol(&parsed, &store, &index, source, "value").0;
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let TreeKind::Select(selection) = &parsed.ast.get(rhs).kind else {
            panic!("source RHS should be a selection")
        };
        let source_qualifier = selection.qualifier;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed_select = typer.type_expression(rhs, context).unwrap();
        let typed_qualifier = typer
            .source_typed_index()
            .get(source, source_qualifier)
            .unwrap();
        let qualifier_type = typer.typed_ast().get(typed_qualifier).ty;
        let Type::TermRef { prefix, target } = typer
            .store()
            .types
            .get(typer.typed_ast().get(typed_select).ty)
        else {
            panic!("selection should retain a term reference")
        };

        assert_eq!(*prefix, qualifier_type);
        assert!(matches!(
            typer.store().types.get(*prefix),
            Type::ThisType { class: actual } if *actual == class
        ));
        assert_eq!(*target, TermRefTarget::Symbol(field));
        assert_eq!(
            typer
                .widen_expression_type(typer.typed_ast().get(typed_select).ty)
                .unwrap(),
            definitions.int
        );
    }

    #[test]
    fn generic_this_selection_widens_using_its_class_parameters() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A] { val value: A; def use: A = this.value }");
        let class = class_symbol(&parsed, &store, &index, source, "Box");
        let parameter = type_parameter_symbol(&parsed, &store, &index, source, "A");
        let field = val_symbol(&parsed, &store, &index, source, "value").0;
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) }
                if matches!(typer.store().types.get(*prefix), Type::ThisType { class: receiver } if *receiver == class)
                    && *symbol == field
        ));
        let widened = typer
            .widen_expression_type(typer.typed_ast().get(typed).ty)
            .unwrap();
        assert!(matches!(
            typer.store().types.get(widened),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == parameter
        ));
    }

    #[test]
    fn stable_local_selection_keeps_the_qualifier_reference_path() {
        let source_text = "class Text; class Box[A] { val value: A }; class Use { def use(box: Box[Text]): Text = box.value }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let text = class_symbol(&parsed, &store, &index, source, "Text");
        let parameter = type_parameter_symbol(&parsed, &store, &index, source, "A");
        let field = val_symbol(&parsed, &store, &index, source, "value").0;
        let box_parameter = val_symbol(&parsed, &store, &index, source, "box").0;
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(box_parameter).unwrap(),
            owner: method,
        };
        let TreeKind::Select(selection) = &parsed.ast.get(rhs).kind else {
            panic!("source RHS should be a selection")
        };
        let source_qualifier = selection.qualifier;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed_select = typer.type_expression(rhs, context).unwrap();
        let typed_qualifier = typer
            .source_typed_index()
            .get(source, source_qualifier)
            .unwrap();
        let qualifier_type = typer.typed_ast().get(typed_qualifier).ty;
        let Type::TermRef { prefix, target } = typer
            .store()
            .types
            .get(typer.typed_ast().get(typed_select).ty)
        else {
            panic!("selection should retain a term reference")
        };
        assert_eq!(*prefix, qualifier_type);
        assert!(matches!(
            typer.store().types.get(*prefix),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                if *symbol == box_parameter
        ));
        assert_eq!(*target, TermRefTarget::Symbol(field));
        let widened = typer
            .widen_expression_type(typer.typed_ast().get(typed_select).ty)
            .unwrap();
        assert!(matches!(
            typer.store().types.get(widened),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == text
        ));
        let SymbolInfo::Complete(declaration_type) = *typer.store().symbols.info(field) else {
            panic!("selected source field should be completed")
        };
        assert!(matches!(
            typer.store().types.get(declaration_type),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }
                if *symbol == parameter
        ));
    }

    #[test]
    fn unstable_selection_prefix_fails_atomically() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Box { val value: Int = 1 }; class Use { var box: Box = null; def use: Int = box.value }",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let store_checkpoint = store.checkpoint();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::UnstableSelectionPrefix { .. })
        ));
        assert_eq!(typer.typed_ast().iter().count(), 0);
        assert_eq!(typer.source_typed_index().len(), 0);
        assert_eq!(typer.store().checkpoint(), store_checkpoint);
    }

    #[test]
    fn by_name_parameter_is_not_a_stable_selection_prefix() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Box { val value: Int = 1 }; class Use { def use(box: => Box): Int = box.value }",
        );
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let (parameter, _) = val_symbol(&parsed, &store, &index, source, "box");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::UnstableSelectionPrefix { .. })
        ));
    }

    #[test]
    fn direct_unique_method_selection_keeps_the_methodic_type() {
        let (kind, _, _, widened_type, _) = type_method_rhs(
            "class C { def get(): Int = 1; def use: Int = this.get }",
            "use",
        )
        .unwrap();

        assert!(matches!(kind, TreeKind::Select(_)));
        assert!(
            matches!(widened_type, Type::Method(_) | Type::Poly(_)),
            "{widened_type:?}"
        );
    }

    #[test]
    fn generic_method_selection_widens_to_receiver_adapted_signature() {
        let source_text = "class Box[A] { def get(): A = null }; class Text; class Use { def use(box: Box[Text]): Text = box.get }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let text = class_symbol(&parsed, &store, &index, source, "Text");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let (parameter, _) = val_symbol(&parsed, &store, &index, source, "box");
        let referenced_method = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (store.names.resolve(definition.name.as_name().text()) == "get")
                    .then(|| index.symbol_at(source, tree).unwrap())
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();
        assert!(matches!(
            typer.typed_ast().get(typed).kind,
            TreeKind::Select(_)
        ));
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                if *symbol == referenced_method
        ));
        let widened = typer
            .widen_expression_type(typer.typed_ast().get(typed).ty)
            .unwrap();
        let widened_type = typer.store().types.get(widened);
        let Type::Method(method) = widened_type else {
            panic!("selected method should widen to its method signature")
        };
        assert!(matches!(
            typer.store().types.get(method.result),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == text
        ));
    }

    #[test]
    fn inherited_unique_member_selection_finds_the_parent_field() {
        let (kind, _, widened, _, definitions) = type_method_rhs(
            "class Parent { val value: Int }; class Child extends Parent; class Use { def use(child: Child): Int = child.value }",
            "use",
        )
        .unwrap();

        assert!(matches!(kind, TreeKind::Select(_)));
        assert_eq!(widened, definitions.int);
    }

    #[test]
    fn generic_receiver_selection_substitutes_its_type_argument() {
        let source_text = "class Text; class Box[A] { val value: A }; class Use { def use(box: Box[Text]): Text = box.value }";
        let (kind, _, _, ty, _) = type_method_rhs(source_text, "use").unwrap();
        let (parsed, store, _, _, index, source) = parse_and_name(source_text);
        let text = class_symbol(&parsed, &store, &index, source, "Text");

        assert!(matches!(kind, TreeKind::Select(_)));
        assert!(matches!(
            ty,
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } if symbol == text
        ));
    }

    #[test]
    fn generic_inherited_selection_substitutes_through_the_parent_view() {
        let source_text = "class Text; class Parent[A] { val value: A }; class Child[B] extends Parent[B]; class Use { def use(child: Child[Text]): Text = child.value }";
        let (kind, _, _, ty, _) = type_method_rhs(source_text, "use").unwrap();
        let (parsed, store, _, _, index, source) = parse_and_name(source_text);
        let text = class_symbol(&parsed, &store, &index, source, "Text");

        assert!(matches!(kind, TreeKind::Select(_)));
        assert!(matches!(
            ty,
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } if symbol == text
        ));
    }

    #[test]
    fn missing_member_is_a_typed_expression_error() {
        let source_text = "class C { def use: Int = this.missing }";
        let (parsed, store, packages, definitions, index, source) = parse_and_name(source_text);
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let mut store = store;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let class_info = typer.complete_symbol(class).unwrap();
        let Type::ClassInfo(info) = typer.store.types.get_mut(class_info) else {
            panic!("expected a completed ClassInfo")
        };
        // Give this regression a fully known, parentless lookup graph so the
        // empty result is conclusive rather than blocked by builtin Object.
        info.parents.clear();

        let result = typer.type_expression(rhs, context);
        assert!(
            matches!(result, Err(TyperError::MemberNotFound { .. })),
            "{result:?}"
        );
    }

    #[test]
    fn overloaded_selection_is_deferred_without_allocating_a_typed_result() {
        let source_text = "class C { def item(x: Int): Int = x; def item(x: String): Int = 1; def use: Int = this.item }";
        let (parsed, store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let mut store = store;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::OverloadedSelectionDeferred { .. })
        ));
        assert!(typer.typed_ast().iter().next().is_none());
        assert_eq!(typer.source_typed_index().len(), 0);
    }

    #[test]
    fn failed_selection_rolls_back_child_ast_index_and_source_completion() {
        let source_text = "class C { def item(x: Int): Int = x; def item(x: String): Int = 1; def use: Int = this.item }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let class = class_symbol(&parsed, &store, &index, source, "C");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let store_checkpoint = store.checkpoint();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::OverloadedSelectionDeferred { .. })
        ));
        assert!(typer.typed_ast().iter().next().is_none());
        assert_eq!(typer.source_typed_index().len(), 0);
        assert_eq!(typer.store().checkpoint(), store_checkpoint);
        assert_eq!(*typer.store().symbols.info(class), SymbolInfo::Missing);
    }

    #[test]
    fn unique_method_identifier_keeps_exact_reference_and_widens_to_signature() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def method(param: Int): Int = param; def use: Int = method }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let referenced = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (store.names.resolve(definition.name.as_name().text()) == "method")
                    .then(|| index.symbol_at(source, tree).unwrap())
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_method).unwrap(),
            owner: use_method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();

        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == referenced
        ));
        let widened = typer
            .widen_expression_type(typer.typed_ast().get(typed).ty)
            .unwrap();
        assert!(matches!(typer.store().types.get(widened), Type::Method(_)));
    }

    #[test]
    fn repeated_identifier_typing_reuses_its_term_reference() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use(param: Int): Int = param }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let (parameter, _) = val_symbol(&parsed, &store, &index, source, "param");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let first = typer.type_expression(rhs, context).unwrap();
        let store_checkpoint = typer.store().checkpoint();
        let second = typer.type_expression(rhs, context).unwrap();

        assert_eq!(first, second);
        assert_eq!(typer.store().checkpoint(), store_checkpoint);
        assert_eq!(typer.source_typed_index().len(), 1);
    }

    #[test]
    fn overloaded_method_identifier_is_explicitly_deferred() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def method(x: Int): Int = x; def method(x: String): Int = 1; def use: Int = method }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_method).unwrap(),
            owner: use_method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::OverloadedReferenceDeferred { .. })
        ));
        assert!(typer.typed_ast().iter().next().is_none());
    }

    #[test]
    fn application_lookup_retains_the_full_lexical_overload_bucket() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def method(x: Int): Int = x; def method(x: String): Int = 1; def use: Int = method }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_method).unwrap(),
            owner: use_method,
        };
        let name = Name::new(store.names.get("method").unwrap(), Namespace::Term);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let candidates = typer
            .expression_term_candidates(name, context.lexical, rhs.index(), None)
            .unwrap();

        assert_eq!(candidates.len(), 2);
        assert_ne!(candidates[0], candidates[1]);
        assert!(
            candidates
                .iter()
                .all(|candidate| typer.store().symbols.get(*candidate).kind == SymbolKind::Method)
        );
    }

    #[test]
    fn application_selects_the_overload_that_accepts_its_argument() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class A {}; class B {}; class C { def method(x: A): Int = 1; def method(x: B): Int = 2; def use(b: B): Int = method(b) }",
        );
        let expected_class = class_symbol(&parsed, &store, &index, source, "B");
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let use_parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                if index.symbol_at(source, tree) != Some(use_method) {
                    return None;
                }
                Some(
                    index
                        .symbol_at(source, definition.value_param_clauses[0][0])
                        .unwrap(),
                )
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_parameter).unwrap(),
            owner: use_method,
        };
        let overloads: Vec<_> = parsed
            .ast
            .iter()
            .filter_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (store.names.resolve(definition.name.as_name().text()) == "method")
                    .then(|| index.symbol_at(source, tree).unwrap())
            })
            .collect();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let TreeKind::Apply(source_application) = &parsed.ast.get(rhs).kind else {
            panic!("method RHS should be the source application")
        };
        let source_argument = source_application.args[0];
        let typed = typer.type_expression(rhs, context).unwrap();
        let selected = overloads
            .into_iter()
            .find(|symbol| {
                let Ok(info) = typer.complete_symbol(*symbol) else {
                    return false;
                };
                matches!(
                    typer.store().types.get(info),
                    Type::Method(method)
                        if method.params.len() == 1
                            && matches!(typer.store().types.get(method.params[0].ty),
                                Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }
                                    if *class == expected_class)
                )
            })
            .unwrap();
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            panic!("overloaded invocation should produce a typed Apply")
        };
        assert_eq!(
            typer
                .typed_index
                .get(source, source_argument)
                .expect("source argument should have one typed mapping"),
            application.args[0]
        );
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(application.function).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == selected
        ));
        assert_eq!(typer.typed_ast().get(typed).ty, definitions.int);
    }

    #[test]
    fn overload_application_reports_each_nonconforming_candidate() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def method(x: Int): Int = 1; def method(x: Boolean): Int = 2; def use(x: Byte): Int = method(x) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let use_parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                if index.symbol_at(source, tree) != Some(use_method) {
                    return None;
                }
                Some(
                    index
                        .symbol_at(source, definition.value_param_clauses[0][0])
                        .unwrap(),
                )
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_parameter).unwrap(),
            owner: use_method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed_count_before = typer.typed_arena.iter().count();
        let mapping_count_before = typer.typed_index.len();
        let result = typer.type_expression(rhs, context);
        let Err(TyperError::OverloadApplicationNoApplicable { candidates, .. }) = result else {
            panic!("unexpected overload result: {result:?}");
        };
        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().all(|(_, rejection)| matches!(
            rejection,
            OverloadRejection::ArgumentNonConformance {
                argument_index: 0,
                ..
            }
        )));
        assert_eq!(typer.typed_arena.iter().count(), typed_count_before);
        assert_eq!(typer.typed_index.len(), mapping_count_before);
    }

    #[test]
    fn overload_application_filters_candidates_with_the_wrong_arity() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class A {}; class C { def method(x: A): Int = 1; def method(x: A, y: A): Int = 2; def use(a: A): Int = method(a, a) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let use_parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (index.symbol_at(source, tree) == Some(use_method)).then(|| {
                    index
                        .symbol_at(source, definition.value_param_clauses[0][0])
                        .unwrap()
                })
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_parameter).unwrap(),
            owner: use_method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            panic!("an applicable overload should produce an Apply")
        };
        let TreeKind::Ident(_) = &typer.typed_ast().get(application.function).kind else {
            panic!("the selected method should remain an identifier")
        };
        let selected = match typer
            .store()
            .types
            .get(typer.typed_ast().get(application.function).ty)
        {
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("expected a selected method reference, found {other:?}"),
        };
        let info = typer.complete_symbol(selected).unwrap();
        assert!(matches!(
            typer.store().types.get(info),
            Type::Method(method) if method.params.len() == 2
        ));
    }

    #[test]
    fn overload_application_resolves_imported_method_candidates() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class A {}; class B {}; object Lib { def method(x: A): Int = 1; def method(x: B): Int = 2 }; class Client { import Lib.*; def use(x: B): Int = method(x) }",
        );
        let expected_class = class_symbol(&parsed, &store, &index, source, "B");
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (index.symbol_at(source, tree) == Some(use_method)).then(|| {
                    index
                        .symbol_at(source, definition.value_param_clauses[0][0])
                        .unwrap()
                })
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: use_method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            panic!("imported overload should produce a typed Apply")
        };
        let selected = match typer
            .store()
            .types
            .get(typer.typed_ast().get(application.function).ty)
        {
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("expected a selected imported overload, found {other:?}"),
        };
        let info = typer.complete_symbol(selected).unwrap();
        assert!(matches!(
            typer.store().types.get(info),
            Type::Method(method)
                if matches!(typer.store().types.get(method.params[0].ty),
                    Type::TypeRef { target: TypeRefTarget::Symbol(class), .. } if *class == expected_class)
        ));
    }

    #[test]
    fn overload_application_rejects_mixed_method_and_value_candidates() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { val method: Int = 0; def method(x: Int): Int = x; def use: Int = method(1) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_method).unwrap(),
            owner: use_method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::MixedApplicationCandidateKinds { candidates, .. })
                if candidates.len() == 2
        ));
    }

    #[test]
    fn selected_overload_uses_the_receiver_adapted_candidate() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class A {}; class B {}; class C { def method(x: A): Int = 1; def method(x: B): Int = 2 }; class Client { def use(c: C, b: B): Int = c.method(b) }",
        );
        let expected_class = class_symbol(&parsed, &store, &index, source, "B");
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let use_parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                if index.symbol_at(source, tree) != Some(use_method) {
                    return None;
                }
                Some(
                    index
                        .symbol_at(source, definition.value_param_clauses[0][1])
                        .unwrap(),
                )
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_parameter).unwrap(),
            owner: use_method,
        };
        let overloads: Vec<_> = parsed
            .ast
            .iter()
            .filter_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (store.names.resolve(definition.name.as_name().text()) == "method")
                    .then(|| index.symbol_at(source, tree).unwrap())
            })
            .collect();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();
        let selected = overloads
            .into_iter()
            .find(|symbol| {
                let Ok(info) = typer.complete_symbol(*symbol) else {
                    return false;
                };
                matches!(
                    typer.store().types.get(info),
                    Type::Method(method)
                        if method.params.len() == 1
                            && matches!(typer.store().types.get(method.params[0].ty),
                                Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }
                                    if *class == expected_class)
                )
            })
            .unwrap();
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            panic!("selected overload invocation should produce a typed Apply")
        };
        assert!(matches!(
            typer.typed_ast().get(application.function).kind,
            TreeKind::Select(_)
        ));
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(application.function).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == selected
        ));
        assert_eq!(typer.typed_ast().get(typed).ty, definitions.int);
    }

    #[test]
    fn overload_application_selects_the_unique_most_specific_of_three_candidates() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Parent {}; class Child extends Parent {}; class Grandchild extends Child {}; class C { def method(x: Parent): Int = 1; def method(x: Child): Int = 2; def method(x: Grandchild): Int = 3; def use(x: Grandchild): Int = method(x) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (index.symbol_at(source, tree) == Some(use_method)).then(|| {
                    index
                        .symbol_at(source, definition.value_param_clauses[0][0])
                        .unwrap()
                })
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: use_method,
        };
        let expected = class_symbol(&parsed, &store, &index, source, "Grandchild");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            panic!("the most specific overload should be applied")
        };
        let selected = match typer
            .store()
            .types
            .get(typer.typed_ast().get(application.function).ty)
        {
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("expected selected method reference, found {other:?}"),
        };
        let info = typer.complete_symbol(selected).unwrap();
        assert!(matches!(
            typer.store().types.get(info),
            Type::Method(method)
                if matches!(typer.store().types.get(method.params[0].ty),
                    Type::TypeRef { target: TypeRefTarget::Symbol(class), .. } if *class == expected)
        ));
    }

    #[test]
    fn unrelated_applicable_overloads_remain_ambiguous() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class A {}; class B {}; class Both extends A, B {}; class C { def method(x: A): Int = 1; def method(x: B): Int = 2; def use(x: Both): Int = method(x) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (index.symbol_at(source, tree) == Some(use_method)).then(|| {
                    index
                        .symbol_at(source, definition.value_param_clauses[0][0])
                        .unwrap()
                })
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: use_method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::AmbiguousOverloadApplication { candidates, .. })
                if candidates.len() == 2
        ));
    }

    #[test]
    fn matching_arity_polymorphic_overload_blocks_monomorphic_selection() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class A {}; class C { def method(x: A): Int = 1; def method[T](x: T): Int = 2; def use(x: A): Int = method(x) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (index.symbol_at(source, tree) == Some(use_method)).then(|| {
                    index
                        .symbol_at(source, definition.value_param_clauses[0][0])
                        .unwrap()
                })
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: use_method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                candidate,
                ..
            }) if matches!(typer.store().symbols.get(candidate).info, SymbolInfo::Missing)
                || matches!(typer.store().symbols.get(candidate).info, SymbolInfo::Complete(_))
        ));
    }

    #[test]
    fn potentially_applicable_repeated_parameter_overload_blocks_selection() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def method(x: Int, y: Int): Int = 1; def method(xs: Int*): Int = 2; def use(x: Int, y: Int): Int = method(x, y) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (index.symbol_at(source, tree) == Some(use_method)).then(|| {
                    index
                        .symbol_at(source, definition.value_param_clauses[0][0])
                        .unwrap()
                })
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: use_method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let Err(TyperError::OverloadResolutionRequiresUnsupportedCandidate { candidate, .. }) =
            typer.type_expression(rhs, context)
        else {
            panic!("a potentially applicable repeated-parameter overload must block selection")
        };
        let info = typer.complete_symbol(candidate).unwrap();
        assert!(matches!(
            typer.store().types.get(info),
            Type::Method(method) if method.params.len() == 1 && method.params[0].varargs
        ));
    }

    #[test]
    fn repeated_parameter_overload_with_too_few_arguments_is_filtered() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class C { def method(): Int = 1; def method(prefix: Int, xs: Int*): Int = 2; def use(): Int = method() }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(use_method).unwrap(),
            owner: use_method,
        };
        let expected_method = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (store.names.resolve(definition.name.as_name().text()) == "method"
                    && definition.value_param_clauses[0].is_empty())
                .then(|| index.symbol_at(source, tree).unwrap())
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            panic!("zero-argument overload should produce an Apply")
        };
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(application.function).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == expected_method
        ));
    }

    #[test]
    fn wrong_arity_polymorphic_overload_does_not_block_monomorphic_selection() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class A {}; class C { def method(x: A): Int = 1; def method[T](x: T, y: T): Int = 2; def use(x: A): Int = method(x) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (index.symbol_at(source, tree) == Some(use_method)).then(|| {
                    index
                        .symbol_at(source, definition.value_param_clauses[0][0])
                        .unwrap()
                })
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: use_method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(typer.type_expression(rhs, context).is_ok());
    }

    #[test]
    fn inherited_generic_overload_uses_the_receiver_adapted_signature() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Wider {}; class Text extends Wider {}; class Base[A] { def method(x: A): Int = 1 }; class Child extends Base[Text] { def method(x: Wider): Int = 2 }; class Client { def use(child: Child, x: Text): Int = child.method(x) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (index.symbol_at(source, tree) == Some(use_method)).then(|| {
                    index
                        .symbol_at(source, definition.value_param_clauses[0][1])
                        .unwrap()
                })
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: use_method,
        };
        let base_method = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (store.names.resolve(definition.name.as_name().text()) == "method"
                    && store
                        .symbols
                        .get(index.symbol_at(source, tree).unwrap())
                        .owner
                        == Some(class_symbol(&parsed, &store, &index, source, "Base")))
                .then(|| index.symbol_at(source, tree).unwrap())
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer
            .type_expression(rhs, context)
            .unwrap_or_else(|error| panic!("typing inherited overload failed: {error:?}"));
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            panic!("inherited generic overload should produce an Apply")
        };
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(application.function).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == base_method
        ));
    }

    #[test]
    fn inherited_overload_competes_with_a_multi_method_local_bucket() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class ParentType {}; class ChildType extends ParentType {}; class Other {}; class Base { def method(x: ChildType): Int = 1 }; class C extends Base { def method(x: ParentType): Int = 2; def method(x: Other): Int = 3 }; class Client { def use(c: C, x: ChildType): Int = c.method(x) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (index.symbol_at(source, tree) == Some(use_method)).then(|| {
                    index
                        .symbol_at(source, definition.value_param_clauses[0][1])
                        .unwrap()
                })
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: use_method,
        };
        let base_class = class_symbol(&parsed, &store, &index, source, "Base");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            panic!("local overload bucket should produce an Apply")
        };
        let selected = match typer
            .store()
            .types
            .get(typer.typed_ast().get(application.function).ty)
        {
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("expected selected method reference, found {other:?}"),
        };
        assert_eq!(typer.store().symbols.get(selected).owner, Some(base_class));
    }

    #[test]
    fn exact_inherited_signature_is_suppressed_by_its_override() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class A {}; class Base { def method(x: A): Int = 1 }; class C extends Base { def method(x: A): Int = 2 }; class Client { def use(c: C, x: A): Int = c.method(x) }",
        );
        let (use_method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (index.symbol_at(source, tree) == Some(use_method)).then(|| {
                    index
                        .symbol_at(source, definition.value_param_clauses[0][1])
                        .unwrap()
                })
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: use_method,
        };
        let receiver_class = class_symbol(&parsed, &store, &index, source, "C");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            panic!("overridden method should produce an Apply")
        };
        let selected = match typer
            .store()
            .types
            .get(typer.typed_ast().get(application.function).ty)
        {
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("expected selected method reference, found {other:?}"),
        };
        assert_eq!(
            typer.store().symbols.get(selected).owner,
            Some(receiver_class)
        );
    }

    #[test]
    fn a_type_namespace_name_does_not_resolve_as_a_term_identifier() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { type item = Int; def method: Int = item }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "method");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::TermNameNotFound { .. })
        ));
    }

    #[test]
    fn source_object_identifier_retains_object_symbol_and_widens_to_module_class() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("object O; class C { def use: Any = O }");
        let (object_tree, object) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                matches!(
                    node.kind,
                    TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_))
                )
                .then(|| (tree, index.symbol_at(source, tree).unwrap()))
            })
            .unwrap();
        let owner = store.symbols.get(object).owner.unwrap();
        let module_class = index
            .derived_symbol_at(owner, source, object_tree)
            .expect("source object should have a derived ModuleClass");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == object
        ));
        let widened = typer
            .widen_expression_type(typer.typed_ast().get(typed).ty)
            .unwrap();
        assert!(matches!(
            typer.store().types.get(widened),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }
                if *symbol == module_class
        ));
    }

    #[test]
    fn source_object_selection_looks_up_members_through_its_module_class() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("object O { val value: Int = 1 }; class C { def use: Int = O.value }");
        let (object_tree, object) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                matches!(
                    node.kind,
                    TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_))
                )
                .then(|| (tree, index.symbol_at(source, tree).unwrap()))
            })
            .unwrap();
        let owner = store.symbols.get(object).owner.unwrap();
        let module_class = index
            .derived_symbol_at(owner, source, object_tree)
            .expect("source object should have a derived ModuleClass");
        let field = val_symbol(&parsed, &store, &index, source, "value").0;
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let TreeKind::Select(selection) = &parsed.ast.get(rhs).kind else {
            panic!("source RHS should be a selection")
        };
        let qualifier_tree = selection.qualifier;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();
        let typed_qualifier = typer
            .source_typed_index()
            .get(source, qualifier_tree)
            .unwrap();
        let qualifier_ty = typer.typed_ast().get(typed_qualifier).ty;
        assert!(matches!(
            typer.store().types.get(qualifier_ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == object
        ));
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) }
                if *prefix == qualifier_ty && *symbol == field
        ));
        let widened = typer
            .widen_expression_type(typer.typed_ast().get(typed).ty)
            .unwrap();
        assert_eq!(widened, definitions.int);
        assert_eq!(
            typer.store().symbols.get(module_class).kind,
            SymbolKind::ModuleClass
        );
    }

    #[test]
    fn nested_object_selection_retains_the_concrete_receiver_prefix() {
        let source_text = "class Outer { object Nested { val value: Int = 1 } }; class Use { def use(outer: Outer): Int = outer.Nested.value }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = val_symbol(&parsed, &store, &index, source, "outer").0;
        let lexical = index.declaration_context_of(parameter).unwrap();
        let context = ExpressionContext {
            lexical,
            owner: method,
        };
        let TreeKind::Select(selection) = &parsed.ast.get(rhs).kind else {
            panic!("source RHS should be a selection")
        };
        let object_selection_tree = selection.qualifier;
        let TreeKind::Select(object_selection) = &parsed.ast.get(object_selection_tree).kind else {
            panic!("outer selection qualifier should be a selection")
        };
        let receiver_tree = object_selection.qualifier;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();
        let receiver = typer
            .source_typed_index()
            .get(source, receiver_tree)
            .unwrap();
        let receiver_type = typer.typed_ast().get(receiver).ty;
        assert!(matches!(
            typer.store().types.get(receiver_type),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == parameter
        ));
        let object_selection = typer
            .source_typed_index()
            .get(source, object_selection_tree)
            .unwrap();
        let object_type = typer.typed_ast().get(object_selection).ty;
        let widened_object = typer.widen_expression_type(object_type).unwrap();
        let Type::TypeRef { prefix, target } = typer.store().types.get(widened_object) else {
            panic!("nested object should widen to a module-class reference")
        };
        assert_eq!(*prefix, receiver_type);
        let module_class = match target {
            TypeRefTarget::Symbol(symbol) => *symbol,
            _ => panic!("nested object module class should be a symbol reference"),
        };
        assert_eq!(
            typer.store().symbols.get(module_class).kind,
            SymbolKind::ModuleClass
        );
        assert_eq!(
            typer
                .widen_expression_type(typer.typed_ast().get(typed).ty)
                .unwrap(),
            definitions.int
        );
    }

    #[test]
    fn external_object_identifier_stays_explicitly_deferred() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Any = External }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let lexical = index.declaration_context_of(method).unwrap();
        let name = Name::new(store.names.intern("External"), Namespace::Term);
        let external = store.symbols.alloc(dotty_core::Symbol {
            name,
            owner: None,
            kind: SymbolKind::Object,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let scope = index.source_context(lexical).lexical_scope;
        store.scopes.get_mut(scope).enter(name, external);
        let context = ExpressionContext {
            lexical,
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ObjectTermReferenceDeferred { symbol, .. }) if symbol == external
        ));
        assert!(typer.typed_ast().iter().next().is_none());
    }

    #[test]
    fn nested_member_selections_preserve_the_full_reference_path() {
        let source_text = "class Leaf { val value: Int = 1 }; class Box { val child: Leaf }; class Use { def use(box: Box): Int = box.child.value }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let child = val_symbol(&parsed, &store, &index, source, "child").0;
        let value = val_symbol(&parsed, &store, &index, source, "value").0;
        let box_symbol = val_symbol(&parsed, &store, &index, source, "box").0;
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(box_symbol).unwrap(),
            owner: method,
        };
        let TreeKind::Select(outer_selection) = &parsed.ast.get(rhs).kind else {
            panic!("source RHS should be a selection")
        };
        let middle_tree = outer_selection.qualifier;
        let TreeKind::Select(middle_selection) = &parsed.ast.get(middle_tree).kind else {
            panic!("selection qualifier should also be a selection")
        };
        let box_tree = middle_selection.qualifier;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let outer = typer.type_expression(rhs, context).unwrap();
        let middle = typer.source_typed_index().get(source, middle_tree).unwrap();
        let typed_box = typer.source_typed_index().get(source, box_tree).unwrap();
        let middle_type = typer.typed_ast().get(middle).ty;
        let box_type = typer.typed_ast().get(typed_box).ty;
        assert!(matches!(
            typer.store().types.get(middle_type),
            Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) }
                if *prefix == box_type && *symbol == child
        ));
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(outer).ty),
            Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) }
                if *prefix == middle_type && *symbol == value
        ));
        assert_eq!(
            typer
                .widen_expression_type(typer.typed_ast().get(outer).ty)
                .unwrap(),
            definitions.int
        );
    }

    #[test]
    fn typed_selection_indexes_its_qualifier_and_preserves_source_positions() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value: Int = 1; def use: Int = this.value }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let source_position = parsed.ast.get(rhs).position;
        let TreeKind::Select(source_select) = &parsed.ast.get(rhs).kind else {
            panic!("expected source selection")
        };
        let source_qualifier = source_select.qualifier;
        let qualifier_position = parsed.ast.get(source_qualifier).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed_select = typer.type_expression(rhs, context).unwrap();
        let typed_qualifier = typer
            .source_typed_index()
            .get(source, source_qualifier)
            .expect("typed qualifier should be indexed");

        assert_eq!(
            typer.source_typed_index().get(source, rhs),
            Some(typed_select)
        );
        assert_eq!(
            typer.typed_ast().get(typed_select).position,
            source_position
        );
        assert_eq!(
            typer.typed_ast().get(typed_qualifier).position,
            qualifier_position
        );
        for (_, tree) in typer.typed_ast().iter() {
            assert!(typer.store().types.contains(tree.ty));
        }
    }

    #[test]
    fn application_with_unresolved_callee_reports_name_error() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { def use: Int = method(1) }");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::TermNameNotFound { .. })
        ));
    }

    #[test]
    fn numeric_literal_42_becomes_an_int_constant() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value: Int = 42 }");
        let (symbol, _, rhs) = val_definition_and_rhs(&parsed, &store, &index, source, "value");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(symbol).unwrap(),
            owner: store.symbols.get(symbol).owner.unwrap(),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();

        assert_eq!(
            typer.typed_ast().get(typed).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: dotty_core::Constant::Int(42)
            })
        );
        assert_eq!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            &Type::Constant(dotty_core::Constant::Int(42))
        );
        assert_eq!(
            typer
                .widen_expression_type(typer.typed_ast().get(typed).ty)
                .unwrap(),
            definitions.int
        );
    }

    #[test]
    fn largest_positive_int_literal_and_separators_are_preserved() {
        let (value, own_type, ty, definitions) =
            type_value_rhs("class C { val value = 2147483647 }");
        assert_eq!(value, dotty_core::Constant::Int(i32::MAX));
        assert_eq!(own_type, Type::Constant(value.clone()));
        assert_eq!(ty, definitions.int);

        let (value, own_type, ty, definitions) = type_value_rhs("class C { val value = 4_2 }");
        assert_eq!(value, dotty_core::Constant::Int(42));
        assert_eq!(own_type, Type::Constant(value.clone()));
        assert_eq!(ty, definitions.int);
    }

    #[test]
    fn hexadecimal_integer_literal_preserves_radix_bits() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value = 0xffffffff }");
        let (symbol, _, rhs) = val_definition_and_rhs(&parsed, &store, &index, source, "value");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(symbol).unwrap(),
            owner: store.symbols.get(symbol).owner.unwrap(),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();

        assert_eq!(
            typer.typed_ast().get(typed).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: dotty_core::Constant::Int(-1)
            })
        );
    }

    #[test]
    fn binary_integer_literal_preserves_radix_value() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value = 0b1010 }");
        let (symbol, _, rhs) = val_definition_and_rhs(&parsed, &store, &index, source, "value");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(symbol).unwrap(),
            owner: store.symbols.get(symbol).owner.unwrap(),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();

        assert_eq!(
            typer.typed_ast().get(typed).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: dotty_core::Constant::Int(10)
            })
        );
        assert_eq!(
            typer.store().types.get(typer.typed_ast().get(typed).ty),
            &Type::Constant(dotty_core::Constant::Int(10))
        );
        assert_eq!(
            typer
                .widen_expression_type(typer.typed_ast().get(typed).ty)
                .unwrap(),
            definitions.int
        );
    }

    #[test]
    fn typing_a_literal_twice_reuses_its_typed_tree_and_position() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value = 42 }");
        let (symbol, _, rhs) = val_definition_and_rhs(&parsed, &store, &index, source, "value");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(symbol).unwrap(),
            owner: store.symbols.get(symbol).owner.unwrap(),
        };
        let source_position = parsed.ast.get(rhs).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let first = typer.type_expression(rhs, context).unwrap();
        let second = typer.type_expression(rhs, context).unwrap();

        assert_eq!(first, second);
        assert_eq!(typer.typed_ast().get(first).position, source_position);
        assert_eq!(typer.typed_ast().iter().count(), 1);
        assert_eq!(typer.source_typed_index().len(), 1);
    }

    #[test]
    fn decimal_and_exponent_literals_become_double_constants() {
        for (source_text, expected) in [
            ("class C { val value = 1.25 }", 1.25_f64),
            ("class C { val value = 1e2 }", 100.0_f64),
        ] {
            let (parsed, mut store, packages, definitions, index, source) =
                parse_and_name(source_text);
            let (symbol, _, rhs) = val_definition_and_rhs(&parsed, &store, &index, source, "value");
            let context = ExpressionContext {
                lexical: index.declaration_context_of(symbol).unwrap(),
                owner: store.symbols.get(symbol).owner.unwrap(),
            };
            let mut typer = SourceTyper::new(
                &parsed.ast,
                source,
                &index,
                &mut store,
                definitions,
                &packages,
            );

            let typed = typer.type_expression(rhs, context).unwrap();

            assert_eq!(
                typer.typed_ast().get(typed).kind,
                TreeKind::Literal(dotty_core::ast::Literal {
                    value: dotty_core::Constant::double(expected)
                })
            );
            assert_eq!(
                typer.store().types.get(typer.typed_ast().get(typed).ty),
                &Type::Constant(dotty_core::Constant::double(expected))
            );
            assert_eq!(
                typer
                    .widen_expression_type(typer.typed_ast().get(typed).ty)
                    .unwrap(),
                definitions.double
            );
        }
    }

    #[test]
    fn out_of_range_decimal_integer_returns_a_typed_error_without_allocations() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value = 2147483648 }");
        let (symbol, _, rhs) = val_definition_and_rhs(&parsed, &store, &index, source, "value");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(symbol).unwrap(),
            owner: store.symbols.get(symbol).owner.unwrap(),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let store_before = typer.store().checkpoint();

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::IntegerLiteralOutOfRange { spelling, .. }) if spelling == "2147483648"
        ));
        assert!(typer.typed_ast().iter().next().is_none());
        assert_eq!(typer.source_typed_index().len(), 0);
        assert_eq!(typer.store().checkpoint(), store_before);
    }

    #[test]
    fn overflowing_floating_literal_returns_a_typed_error() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value = 1e400 }");
        let (symbol, _, rhs) = val_definition_and_rhs(&parsed, &store, &index, source, "value");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(symbol).unwrap(),
            owner: store.symbols.get(symbol).owner.unwrap(),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::FloatingLiteralInvalid { spelling, .. }) if spelling == "1e400"
        ));
    }

    #[test]
    fn malformed_integer_radix_returns_an_error_instead_of_panicking() {
        let (mut parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C { val value = 1 }");
        let (symbol, _, rhs) = val_definition_and_rhs(&parsed, &store, &index, source, "value");
        let text = store.names.intern("1");
        parsed.ast.get_mut(rhs).kind =
            TreeKind::PhaseSpecific(UntypedNode::Number(NumberLiteral {
                text,
                kind: NumberKind::Whole(1),
            }));
        let context = ExpressionContext {
            lexical: index.declaration_context_of(symbol).unwrap(),
            owner: store.symbols.get(symbol).owner.unwrap(),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::IntegerLiteralOutOfRange { .. })
        ));
    }

    #[test]
    fn unsupported_string_and_null_literals_have_dedicated_errors() {
        for (source_text, expected_error) in [
            ("class C { val value = \"text\" }", "string"),
            ("class C { val value = null }", "null"),
        ] {
            let (parsed, mut store, packages, definitions, index, source) =
                parse_and_name(source_text);
            let (symbol, _, rhs) = val_definition_and_rhs(&parsed, &store, &index, source, "value");
            let context = ExpressionContext {
                lexical: index.declaration_context_of(symbol).unwrap(),
                owner: store.symbols.get(symbol).owner.unwrap(),
            };
            let mut typer = SourceTyper::new(
                &parsed.ast,
                source,
                &index,
                &mut store,
                definitions,
                &packages,
            );
            let result = typer.type_expression(rhs, context);

            match (expected_error, result) {
                ("string", Err(TyperError::StringLiteralTypingDeferred { .. }))
                | ("null", Err(TyperError::NullLiteralTypingDeferred { .. })) => {}
                (variant, other) => panic!("expected {variant} literal deferral, got {other:?}"),
            }
            assert!(typer.typed_ast().iter().next().is_none());
        }
    }

    #[test]
    fn regular_application_types_a_monomorphic_method() {
        let source_text = "class C { def inc(x: Int): Int = x; def use: Int = inc(1) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let callee = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "inc" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            panic!("regular source call should produce a typed Apply")
        };
        assert_eq!(application.kind, ApplyKind::Regular);
        assert_eq!(typer.typed_ast().get(typed).ty, definitions.int);
        let function_type = typer.typed_ast().get(application.function).ty;
        assert!(matches!(
            typer.store().types.get(function_type),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == callee
        ));
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(application.args[0]).ty),
            Type::Constant(dotty_core::Constant::Int(1))
        ));
        assert_eq!(typer.type_expression(rhs, context).unwrap(), typed);
    }

    #[test]
    fn application_reports_exact_arity_mismatch() {
        let source_text = "class C { def f(x: Int): Int = x; def use: Int = f() }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ApplicationArityMismatch {
                expected: 1,
                actual: 0,
                ..
            })
        ));
    }

    #[test]
    fn application_rejects_non_method_callees() {
        let source_text = "class C { val value: Int = 1; def use: Int = value() }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ApplicationCalleeNotMethod { ty, .. }) if ty == definitions.int
        ));
    }

    #[test]
    fn application_reports_argument_type_mismatch_with_index() {
        let source_text = "class C { def f(x: Int): Int = x; def use: Int = f(false) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let result = typer.type_expression(rhs, context);
        assert!(
            matches!(
                &result,
                Err(TyperError::ApplicationArgumentTypeMismatch {
                    argument_index: 0,
                    actual,
                    expected,
                    ..
                }) if *actual == definitions.boolean && *expected == definitions.int
            ),
            "unexpected application result: {result:?}"
        );
    }

    #[test]
    fn failed_application_rolls_back_typed_children_and_semantic_changes() {
        let source_text = "class C { def f(x: Int, y: Int): Int = x; def use: Int = f(1, false) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let store_before = typer.store().checkpoint();

        let result = typer.type_expression(rhs, context);
        assert!(
            matches!(
                &result,
                Err(TyperError::ApplicationArgumentTypeMismatch {
                    argument_index: 1,
                    ..
                })
            ),
            "unexpected application result: {result:?}"
        );
        assert!(typer.typed_ast().iter().next().is_none());
        assert_eq!(typer.source_typed_index().len(), 0);
        assert_eq!(typer.store().checkpoint(), store_before);
    }

    #[test]
    fn too_many_application_arguments_reports_exact_arity() {
        let source_text = "class C { def f(x: Int): Int = x; def use: Int = f(1, 2) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ApplicationArityMismatch {
                expected: 1,
                actual: 2,
                ..
            })
        ));
    }

    #[test]
    fn polymorphic_method_application_is_deferred() {
        let source_text = "class C { def id[A](x: A): A = x; def use: Int = id(1) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::PolymorphicMethodApplicationDeferred { .. })
        ));
    }

    #[test]
    fn contextual_method_application_is_deferred() {
        let source_text = "class C { def f(using x: Int): Int = x; def use: Int = f(1) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::UnsupportedApplicationMethodKind {
                kind: MethodKind::Contextual,
                ..
            })
        ));
    }

    #[test]
    fn implicit_method_application_is_deferred() {
        let source_text = "class C { def f(implicit x: Int): Int = x; def use: Int = f(1) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::UnsupportedApplicationMethodKind {
                kind: MethodKind::Implicit,
                ..
            })
        ));
    }

    #[test]
    fn using_application_is_deferred() {
        let source_text = "class C { def f(using x: Int): Int = x; def use: Int = f(using 1) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::UsingApplicationDeferred { .. })
        ));
    }

    #[test]
    fn by_name_application_parameter_is_deferred() {
        let source_text = "class C { def f(x: => Int): Int = x; def use: Int = f(1) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ByNameApplicationParameterDeferred {
                parameter_index: 0,
                ..
            })
        ));
    }

    #[test]
    fn erased_application_parameter_is_deferred() {
        let source_text = "class C { def f(x: Int): Int = x; def use: Int = f(1) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let parameter = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "f" =>
                {
                    definition
                        .value_param_clauses
                        .first()
                        .and_then(|clause| clause.first())
                        .and_then(|tree| index.symbol_at(source, *tree))
                }
                _ => None,
            })
            .unwrap();
        let flags = store.symbols.get(parameter).flags;
        store.symbols.get_mut(parameter).flags = flags | SymbolFlags::ERASED;
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::ErasedApplicationParameterDeferred {
                parameter_index: 0,
                ..
            })
        ));
    }

    #[test]
    fn varargs_application_parameter_is_deferred() {
        let source_text = "class C { def f(xs: Int*): Int = 1; def use: Int = f(1) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let result = typer.type_expression(rhs, context);
        assert!(
            matches!(
                &result,
                Err(TyperError::VarargsApplicationParameterDeferred {
                    parameter_index: 0,
                    ..
                })
            ),
            "unexpected varargs result: {result:?}"
        );
    }

    #[test]
    fn varargs_parameter_reference_is_deferred_without_sequence_type() {
        let source_text = "class C { def f(xs: Int*): Int = xs }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "f");
        let method_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, _)| (index.symbol_at(source, tree) == Some(method)).then_some(tree))
            .unwrap();
        let TreeKind::DefDef(definition) = &parsed.ast.get(method_tree).kind else {
            panic!("method symbol should point to a DefDef")
        };
        let parameter_tree = definition.value_param_clauses[0][0];
        let parameter = index.symbol_at(source, parameter_tree).unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        typer.complete_symbol(method).unwrap();
        let method_type = typer.store().symbols.info(method);
        let SymbolInfo::Complete(method_type) = *method_type else {
            panic!("method signature should be complete")
        };
        let Type::Method(method_type) = typer.store().types.get(method_type) else {
            panic!("method symbol should contain a method type")
        };
        assert!(method_type.params[0].varargs);
        assert_eq!(method_type.params[0].ty, definitions.int);

        let parameter_info = typer.store().symbols.info(parameter);
        let SymbolInfo::Complete(parameter_value_type) = *parameter_info else {
            panic!("varargs parameter symbol should retain its repeated type")
        };
        assert!(matches!(
            typer.store().types.get(parameter_value_type),
            Type::Repeated { element } if *element == definitions.int
        ));
        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::VarargsParameterReferenceDeferred {
                source: found_source,
                tree_index,
                symbol: found_symbol,
            }) if found_source == source && tree_index == rhs.index() && found_symbol == parameter
        ));
        assert!(matches!(
            typer.widen_expression_type(parameter_value_type),
            Err(TyperError::ExpressionTypeCannotBeWidened { ty }) if ty == parameter_value_type
        ));
    }

    #[test]
    fn curried_monomorphic_application_types_each_clause() {
        let source_text =
            "class C { def f(x: Int)(y: Boolean): Boolean = y; def use: Boolean = f(1)(true) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();

        assert_eq!(typer.typed_ast().get(typed).ty, definitions.boolean);
        let TreeKind::Apply(outer) = &typer.typed_ast().get(typed).kind else {
            panic!("outer clause should be an Apply")
        };
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(outer.function).ty),
            Type::Method(method) if method.params.len() == 1 && method.params[0].ty == definitions.boolean
        ));
    }

    #[test]
    fn selected_generic_receiver_method_can_be_applied() {
        let source_text = "class Box[A] { def accept(value: A): A = value }; class Use { def use(box: Box[Int]): Int = box.accept(1) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let parameter = val_symbol(&parsed, &store, &index, source, "box").0;
        let context = ExpressionContext {
            lexical: index.declaration_context_of(parameter).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let typed = typer.type_expression(rhs, context).unwrap();

        assert_eq!(typer.typed_ast().get(typed).ty, definitions.int);
    }

    #[test]
    fn dependent_method_result_is_deferred() {
        let source_text = "class C { def f(x: Int): Int = x; def use: Int = f(1) }";
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let callee = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "f" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let reserved = typer.store.types.reserve();
        let dependent = typer.store.types.alloc(Type::ParamRef {
            binder: reserved.id(),
            index: 0,
        });
        let callable = typer.store.types.fill(
            reserved,
            Type::Method(dotty_core::types::MethodType {
                params: vec![dotty_core::types::MethodParam {
                    name: dotty_core::TermName::new(typer.store.names.intern("x")),
                    ty: definitions.int,
                    erased: false,
                    varargs: false,
                }],
                result: dependent,
                kind: MethodKind::Plain,
            }),
        );
        typer
            .store
            .symbols
            .set_info(callee, SymbolInfo::Complete(callable));

        assert!(matches!(
            typer.type_expression(rhs, context),
            Err(TyperError::DependentMethodApplicationDeferred { binder, result, .. })
                if binder == callable && result == dependent
        ));
    }
}
