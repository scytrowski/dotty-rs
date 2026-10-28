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
    /// The outer constructor receives a same-instance planned method result.
    PlannedCall,
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
    PlannedCall(PlannedCallProof),
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
            ConstructorArgumentProofShape::PlannedCall(_) => {
                ConstructorArgumentProofKind::PlannedCall
            }
        }
    }

    pub const fn planned_call(self) -> Option<PlannedCallProof> {
        match self.shape {
            ConstructorArgumentProofShape::PlannedCall(proof) => Some(proof),
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
}

/// Read-only identities retained by a direct formal argument proof.
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
    pub const fn result_type(self) -> TypeId {
        self.result_type
    }
}

/// Exact original source and typed identities for the supported one-level
/// nested constructor argument. This is present only when the outer
/// constructor argument is itself `Apply(Select(New(...), <init>), formal)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NestedConstructorArgumentProof {
    source_select: TreeId<Untyped>,
    source_new: TreeId<Untyped>,
    source_argument: TreeId<Untyped>,
    typed_select: TreeId<Typed>,
    typed_new: TreeId<Typed>,
    typed_argument: TreeId<Typed>,
    class: SymbolId,
    constructor: SymbolId,
    formal: SymbolId,
    formal_type: TypeId,
    result_type: TypeId,
}

impl NestedConstructorArgumentProof {
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
    pub const fn formal_type(self) -> TypeId {
        self.formal_type
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

/// Scan the complete source-ordered JVM batch for direct, one-formal
/// `Apply(Select(New(target), <init>), formal)` method bodies and validate each
/// against canonical source, typed, semantic, and JVM identities. A body with
/// no such root is ignored; once the structural candidate is present, every
/// failed provenance or shape check is returned as an error rather than
/// silently falling back to unproved planning.
pub fn validate_cross_class_constructor_call_proofs<'session>(
    input: CrossClassConstructorCallProofInput<'session>,
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
            let proof = validate_constructor_call_candidate(
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
            )?;
            proofs.push(proof);
        }
    }
    Ok(proofs)
}

fn is_direct_constructor_call_candidate(ast: &AstArena<Untyped>, rhs: TreeId<Untyped>) -> bool {
    let Some(TreeKind::Apply(application)) = ast.try_get(rhs).map(|node| &node.kind) else {
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

    let TreeKind::Apply(source_apply) = &source_ast
        .try_get(source_rhs)
        .ok_or_else(|| source_error("method RHS source root is stale"))?
        .kind
    else {
        unreachable!()
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
        unreachable!()
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
        unreachable!()
    };
    let source_argument = source_apply.args[0];
    let (argument_formal_tree, nested_source) = match &source_ast
        .try_get(source_argument)
        .ok_or_else(|| source_error("constructor argument source root is stale"))?
        .kind
    {
        TreeKind::Ident(_) => (source_argument, None),
        TreeKind::Apply(nested_apply)
            if nested_apply.kind == ApplyKind::Regular && nested_apply.args.len() == 1 =>
        {
            let nested_select = nested_apply.function;
            let TreeKind::Select(nested_selection) = &source_ast
                .try_get(nested_select)
                .ok_or_else(|| source_error("nested constructor selection source root is stale"))?
                .kind
            else {
                return Err(source_error("nested constructor function must be Select"));
            };
            if store.names.resolve(nested_selection.name.text()) != "<init>" {
                return Err(source_error(
                    "nested constructor call must select the primary constructor",
                ));
            }
            let nested_new = nested_selection.qualifier;
            let TreeKind::New(nested_new_tree) = &source_ast
                .try_get(nested_new)
                .ok_or_else(|| source_error("nested New source root is stale"))?
                .kind
            else {
                return Err(source_error("nested constructor receiver must be New"));
            };
            (
                nested_apply.args[0],
                Some((nested_select, nested_new, nested_new_tree.tpt)),
            )
        }
        TreeKind::Apply(_) => {
            return Err(source_error(
                "nested constructor application must have one regular argument",
            ));
        }
        _ => {
            return Err(source_error(
                "constructor argument must be the exact caller formal identifier or one nested constructor application",
            ));
        }
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
            "constructor argument spelling must resolve to the caller formal",
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
    let nested_typed = if let Some((nested_select, nested_new, nested_new_tpt)) = nested_source {
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
    let typed_root = typed_ast
        .try_get(typed_rhs)
        .ok_or_else(|| typed_error("typed method RHS is stale"))?;
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
    let TreeKind::Apply(typed_apply) = &typed_root.kind else {
        return Err(typed_error("typed candidate RHS is not Apply"));
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
    if store.names.resolve(typed_argument_ident.name.text())
        != store.names.resolve(formal_def.name.as_name().text())
        || !matches!(store.types.try_get(typed_formal_node.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(target), .. }) if *target == method_formal)
    {
        return Err(typed_error(
            "typed constructor argument does not reference the canonical caller formal",
        ));
    }

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
                source_select: source_inner_select,
                source_new: source_inner_new,
                source_argument: argument_formal_tree,
                typed_select: typed_inner_select,
                typed_new: typed_inner_new,
                typed_argument: typed_formal_argument,
                class: inner_class,
                constructor: inner_constructor,
                formal: inner_formal,
                formal_type: inner_formal_type,
                result_type: typed_ast
                    .try_get(typed_argument)
                    .ok_or_else(|| typed_error("typed nested Apply is stale"))?
                    .ty,
            }),
            inner_type,
        )
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
    } else {
        constructor_formal_type == formal_type
            || same_reference_type(constructor_formal_type, formal_type)
    };
    if constructor_signature.kind != dotty_core::MethodKind::Plain
        || constructor_signature.params.len() != 1
        || constructor_signature.params[0].ty != constructor_formal_type
        || !constructor_argument_type_matches
        || (nested_argument_proof.is_some()
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
            None => ConstructorArgumentProofShape::Formal(FormalConstructorArgumentProof {
                source_argument,
                typed_argument,
                formal: method_formal,
                formal_type,
                result_type,
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
    use super::{has_any_flag, is_canonical_class_type_ref};
    use dotty_core::{Definitions, SemanticStore, SymbolFlags, Type, TypeRefTarget};

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
