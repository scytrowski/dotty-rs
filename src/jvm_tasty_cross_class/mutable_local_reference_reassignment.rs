//! Borrowed proof for one mutable Counter reference local and one direct
//! reference reassignment. This receipt authenticates source/typed identity
//! only; it does not authorize JVM or TASTy emission.

use super::*;

/// Inputs from one real frontend session for the frozen mutable Counter
/// rebinding body. Local bindings must come from that session's SourceTyper.
#[derive(Clone, Copy)]
pub struct MutableLocalReferenceReassignmentProofInput<'session> {
    /// Source identity shared by parsing, naming, and typing.
    pub source_id: SourceId,
    /// Canonical untyped definition of the method being validated.
    pub method_tree: TreeId<Untyped>,
    /// Canonical symbol, name, and type stores borrowed from the same session.
    pub store: &'session SemanticStore,
    /// Primitive definitions bootstrapped in the supplied semantic store.
    pub definitions: &'session Definitions,
    /// Original untyped arena containing the source roots in this proof.
    pub source_ast: &'session AstArena<Untyped>,
    /// Exact typed arena containing the mapped roots in this proof.
    pub typed_ast: &'session TypedAst,
    /// Canonical declaration and scope bindings from naming this source.
    pub source_index: &'session SourceSemanticIndex,
    /// Exact source-to-typed mappings produced in this session.
    pub source_typed_index: &'session SourceTypedIndex,
    /// Typer bindings for the two original locals, in source order; missing entries are rejected.
    pub original_bindings: [Option<TypedLocalBinding>; 2],
    /// Typer-produced selected-local binding; absence is rejected by validation.
    pub selected_binding: Option<TypedLocalBinding>,
    /// Complete JVM planning unit borrowing the same semantic and typed stores.
    pub jvm_unit: &'session JvmCompilationUnit<'session>,
}

/// Opaque borrowed identity receipt for the exact two immutable Counter
/// initializers, mutable selected local, one reference assignment, setter and
/// original Pair construction.
pub struct ValidatedMutableLocalReferenceReassignmentProof<'session> {
    _store: &'session SemanticStore,
    _source_ast: &'session AstArena<Untyped>,
    _typed_ast: &'session TypedAst,
    _source_index: &'session SourceSemanticIndex,
    _source_typed_index: &'session SourceTypedIndex,
    _jvm_unit: &'session JvmCompilationUnit<'session>,
    source_id: SourceId,
    owner: SymbolId,
    method: SymbolId,
    scope: dotty_core::ScopeId,
    source_block: TreeId<Untyped>,
    typed_block: TreeId<Typed>,
    source_declarations: [TreeId<Untyped>; 3],
    typed_declarations: [TreeId<Typed>; 3],
    source_initializers: [TreeId<Untyped>; 3],
    typed_initializers: [TreeId<Typed>; 3],
    source_assignment: TreeId<Untyped>,
    typed_assignment: TreeId<Typed>,
    source_assignment_lhs: TreeId<Untyped>,
    typed_assignment_lhs: TreeId<Typed>,
    source_assignment_rhs: TreeId<Untyped>,
    typed_assignment_rhs: TreeId<Typed>,
    selected: SymbolId,
    initializer_target: SymbolId,
    assigned_target: SymbolId,
    counter: SymbolId,
    local_type: TypeId,
    source_setter: TreeId<Untyped>,
    typed_setter: TreeId<Typed>,
    source_setter_receiver: TreeId<Untyped>,
    typed_setter_receiver: TreeId<Typed>,
    source_setter_argument: TreeId<Untyped>,
    typed_setter_argument: TreeId<Typed>,
    setter_method: SymbolId,
    source_pair_result: TreeId<Untyped>,
    typed_pair_result: TreeId<Typed>,
    source_pair_arguments: [TreeId<Untyped>; 2],
    typed_pair_arguments: [TreeId<Typed>; 2],
    original_locals: [SymbolId; 2],
    semantic_originals: [SymbolId; 2],
    prefix_locals: [SymbolId; 2],
}

impl ValidatedMutableLocalReferenceReassignmentProof<'_> {
    /// Source identity shared by parsing, naming, and typing.
    pub const fn source_id(&self) -> SourceId {
        self.source_id
    }
    /// Canonical owning class of the validated method.
    pub const fn owner(&self) -> SymbolId {
        self.owner
    }
    /// Canonical method symbol retained by this proof.
    pub const fn method(&self) -> SymbolId {
        self.method
    }
    /// Canonical lexical scope containing the validated local declaration.
    pub const fn scope(&self) -> dotty_core::ScopeId {
        self.scope
    }
    /// Original source root for the enclosing method-body block.
    pub const fn source_block(&self) -> TreeId<Untyped> {
        self.source_block
    }
    /// Exact mapped typed root for the enclosing method-body block.
    pub const fn typed_block(&self) -> TreeId<Typed> {
        self.typed_block
    }
    /// Original source roots for the retained local declarations in source order.
    pub const fn source_declarations(&self) -> [TreeId<Untyped>; 3] {
        self.source_declarations
    }
    /// Exact mapped typed roots for the retained local declarations in source order.
    pub const fn typed_declarations(&self) -> [TreeId<Typed>; 3] {
        self.typed_declarations
    }
    /// Original source roots for the retained local initializer roots in source order.
    pub const fn source_initializers(&self) -> [TreeId<Untyped>; 3] {
        self.source_initializers
    }
    /// Exact mapped typed roots for the retained local initializer roots in source order.
    pub const fn typed_initializers(&self) -> [TreeId<Typed>; 3] {
        self.typed_initializers
    }
    /// Original source root for the reference reassignment expression.
    pub const fn source_assignment(&self) -> TreeId<Untyped> {
        self.source_assignment
    }
    /// Exact mapped typed root for the reference reassignment expression.
    pub const fn typed_assignment(&self) -> TreeId<Typed> {
        self.typed_assignment
    }
    /// Original source root for the local reference on the left of the assignment.
    pub const fn source_assignment_lhs(&self) -> TreeId<Untyped> {
        self.source_assignment_lhs
    }
    /// Exact mapped typed root for the local reference on the left of the assignment.
    pub const fn typed_assignment_lhs(&self) -> TreeId<Typed> {
        self.typed_assignment_lhs
    }
    /// Original source root for the original local reference on the right of the assignment.
    pub const fn source_assignment_rhs(&self) -> TreeId<Untyped> {
        self.source_assignment_rhs
    }
    /// Exact mapped typed root for the original local reference on the right of the assignment.
    pub const fn typed_assignment_rhs(&self) -> TreeId<Typed> {
        self.typed_assignment_rhs
    }
    /// Canonical selected local whose uses are authenticated by this proof.
    pub const fn selected(&self) -> SymbolId {
        self.selected
    }
    /// Canonical local targeted by the original selected declaration RHS.
    /// This is independent of the later assignment's RHS target.
    pub const fn initializer_target(&self) -> SymbolId {
        self.initializer_target
    }
    /// Original local symbol assigned to the selected mutable reference.
    pub const fn assigned_target(&self) -> SymbolId {
        self.assigned_target
    }
    /// Canonical Counter class shared by the validated reference types.
    pub const fn counter(&self) -> SymbolId {
        self.counter
    }
    /// Semantic type of the validated local reference.
    pub const fn local_type(&self) -> TypeId {
        self.local_type
    }
    /// Original source root for the validated Counter.set application.
    pub const fn source_setter(&self) -> TreeId<Untyped> {
        self.source_setter
    }
    /// Exact mapped typed root for the validated Counter.set application.
    pub const fn typed_setter(&self) -> TreeId<Typed> {
        self.typed_setter
    }
    /// Original source root for the selected local reference receiving the Counter.set call.
    pub const fn source_setter_receiver(&self) -> TreeId<Untyped> {
        self.source_setter_receiver
    }
    /// Exact mapped typed root for the selected local reference receiving the Counter.set call.
    pub const fn typed_setter_receiver(&self) -> TreeId<Typed> {
        self.typed_setter_receiver
    }
    /// Original source root for the argument of the validated Counter.set call.
    pub const fn source_setter_argument(&self) -> TreeId<Untyped> {
        self.source_setter_argument
    }
    /// Exact mapped typed root for the argument of the validated Counter.set call.
    pub const fn typed_setter_argument(&self) -> TreeId<Typed> {
        self.typed_setter_argument
    }
    /// Canonical Counter.set method invoked through the selected reference.
    pub const fn setter_method(&self) -> SymbolId {
        self.setter_method
    }
    /// Original source root for the final Pair constructor application.
    pub const fn source_pair_result(&self) -> TreeId<Untyped> {
        self.source_pair_result
    }
    /// Exact mapped typed root for the final Pair constructor application.
    pub const fn typed_pair_result(&self) -> TreeId<Typed> {
        self.typed_pair_result
    }
    /// Original source roots for the arguments of the final Pair construction in source order.
    pub const fn source_pair_arguments(&self) -> [TreeId<Untyped>; 2] {
        self.source_pair_arguments
    }
    /// Exact mapped typed roots for the arguments of the final Pair construction in source order.
    pub const fn typed_pair_arguments(&self) -> [TreeId<Typed>; 2] {
        self.typed_pair_arguments
    }
    /// The two original initialized local symbols in source order.
    pub const fn original_locals(&self) -> [SymbolId; 2] {
        self.original_locals
    }
    /// The two immutable locals in seed/Pair semantic order, independent of
    /// their source declaration order.
    pub const fn semantic_originals(&self) -> [SymbolId; 2] {
        self.semantic_originals
    }
    /// Original locals in source declaration order; this is distinct from the
    /// semantic `first` target under reversed declarations.
    pub const fn prefix_locals(&self) -> [SymbolId; 2] {
        self.prefix_locals
    }
}

/// Authenticate the frozen mutable Counter-reference assignment body from a
/// single source/typed session. The method has exactly three Int formals and
/// the ordered Block shape `first`, `second`, mutable `selected`, one direct
/// `selected = first|second`, one `selected.set(nextValue)`, then Pair(first,
/// second). The paired producer's candidate rejection is unchanged.
pub fn validate_mutable_local_reference_reassignment_proof<'session>(
    input: MutableLocalReferenceReassignmentProofInput<'session>,
) -> Result<
    ValidatedMutableLocalReferenceReassignmentProof<'session>,
    CrossClassConstructorCallProofError,
> {
    let src = |s| CrossClassConstructorCallProofError::SourceShape(s);
    let sid = |s| CrossClassConstructorCallProofError::SourceIdentity(s);
    let sem = |s| CrossClassConstructorCallProofError::SemanticIdentity(s);
    let typed = |s| CrossClassConstructorCallProofError::TypedBinding(s);
    let MutableLocalReferenceReassignmentProofInput {
        source_id,
        method_tree,
        store,
        definitions,
        source_ast,
        typed_ast,
        source_index,
        source_typed_index,
        original_bindings,
        selected_binding,
        jvm_unit,
    } = input;
    if !std::ptr::eq(jvm_unit.store, store) || !std::ptr::eq(jvm_unit.typed_ast, typed_ast) {
        return Err(sem(
            "JVM unit and proof must borrow the same semantic and typed stores",
        ));
    }
    let TreeKind::DefDef(method_def) = &source_ast
        .try_get(method_tree)
        .ok_or_else(|| src("factory method tree is stale"))?
        .kind
    else {
        return Err(src("factory source root must be a method"));
    };
    require_source_position(
        source_ast,
        source_id,
        method_tree,
        "factory method has a foreign source position",
    )?;
    let method = source_index
        .symbol_at(source_id, method_tree)
        .ok_or_else(|| sid("factory method has no canonical source symbol"))?;
    if source_index.definition_of(method)
        != Some(SourceDefinition::Canonical {
            source: source_id,
            tree: method_tree,
        })
        || !store.symbols.contains(method)
        || store.symbols.get(method).kind != SymbolKind::Method
        || store.symbols.get(method).visibility != dotty_core::Visibility::Public
        || !has_source_origin(store, method, source_id)
    {
        return Err(sid(
            "factory method is not canonical in this source session",
        ));
    }
    let owner = store
        .symbols
        .get(method)
        .owner
        .ok_or_else(|| sid("factory method has no class owner"))?;
    if !store.symbols.contains(owner)
        || store.symbols.get(owner).kind != SymbolKind::Class
        || store.symbols.get(owner).visibility != dotty_core::Visibility::Public
        || store.names.resolve(store.symbols.get(owner).name.text()) != "PairFactory"
        || store.names.resolve(store.symbols.get(method).name.text()) != "create"
        || !has_source_origin(store, owner, source_id)
        || source_index.definition_of(owner).is_none_or(
            |d| !matches!(d, SourceDefinition::Canonical { source, .. } if source == source_id),
        )
    {
        return Err(sid("factory owner must be the canonical source class"));
    }
    let package = store
        .symbols
        .get(owner)
        .owner
        .ok_or_else(|| sid("factory class has no package owner"))?;
    let SymbolInfo::Complete(method_type) = store.symbols.get(method).info else {
        return Err(sem("factory method signature is incomplete"));
    };
    let Some(Type::Method(signature)) = store.types.try_get(method_type) else {
        return Err(sem("factory method must have one canonical signature"));
    };
    if !method_def.type_params.is_empty()
        || method_def.value_param_clauses.len() != 1
        || method_def.value_param_clauses[0].len() != 3
        || signature.kind != dotty_core::MethodKind::Plain
        || signature.params.len() != 3
        || signature.params.iter().any(|p| p.ty != definitions.int)
    {
        return Err(src(
            "factory method must have one plain three-Int-formal clause",
        ));
    }
    let method_scope = source_index
        .scope_of(method)
        .ok_or_else(|| sid("factory method has no formal scope"))?;
    if !store.scopes.contains(method_scope) || store.scopes.get(method_scope).owner != Some(method)
    {
        return Err(sid("factory formal scope must be method-owned"));
    }
    let mut formals = Vec::with_capacity(3);
    for (index, tree) in method_def.value_param_clauses[0].iter().enumerate() {
        let TreeKind::ValDef(def) = &source_ast
            .try_get(*tree)
            .ok_or_else(|| src("factory formal tree is stale"))?
            .kind
        else {
            return Err(src("factory parameters must be value formals"));
        };
        if def.rhs.is_some()
            || matches!(
                source_ast.try_get(def.tpt).map(|n| &n.kind),
                Some(TreeKind::TypeTree(_))
            )
        {
            return Err(src(
                "factory formals must retain explicit source types without defaults",
            ));
        }
        let formal = source_index
            .symbol_at(source_id, *tree)
            .ok_or_else(|| sid("factory formal has no canonical symbol"))?;
        if !store.symbols.contains(formal)
            || store.symbols.get(formal).kind != SymbolKind::Parameter
            || store.symbols.get(formal).owner != Some(method)
            || !has_source_origin(store, formal, source_id)
            || source_index.definition_of(formal)
                != Some(SourceDefinition::Canonical {
                    source: source_id,
                    tree: *tree,
                })
            || store
                .scopes
                .get(method_scope)
                .lookup_all(&store.symbols.get(formal).name)
                != [formal]
            || !matches!(store.symbols.get(formal).info, SymbolInfo::Complete(ty) if ty == definitions.int)
            || signature.params[index].ty != definitions.int
        {
            return Err(sem(
                "factory formals must preserve canonical ordered Int identities",
            ));
        }
        require_source_position(
            source_ast,
            source_id,
            *tree,
            "factory formal has a foreign source span",
        )?;
        formals.push(formal);
    }
    let formals: [SymbolId; 3] = formals
        .try_into()
        .map_err(|_| src("factory must retain three formals"))?;
    let rhs = method_def
        .rhs
        .ok_or_else(|| src("factory method must have a body"))?;
    let TreeKind::Block(block) = &source_ast
        .try_get(rhs)
        .ok_or_else(|| src("factory body is stale"))?
        .kind
    else {
        return Err(src("factory body must be the original Block"));
    };
    if block.stats.len() != 5 {
        return Err(src(
            "factory Block must contain two immutable locals, mutable selected, one assignment and one setter",
        ));
    }
    let declarations: [TreeId<Untyped>; 3] = block.stats[..3]
        .try_into()
        .map_err(|_| src("factory must retain three local declarations"))?;
    let [source_assignment, source_setter] = [block.stats[3], block.stats[4]];
    let TreeKind::Assign(assign) = &source_ast
        .try_get(source_assignment)
        .ok_or_else(|| src("assignment statement is stale"))?
        .kind
    else {
        return Err(src(
            "fourth Block statistic must be the direct reference Assign",
        ));
    };
    let TreeKind::Apply(setter_apply) = &source_ast
        .try_get(source_setter)
        .ok_or_else(|| src("setter statement is stale"))?
        .kind
    else {
        return Err(src("fifth Block statistic must be the setter Apply"));
    };
    if setter_apply.kind != ApplyKind::Regular || setter_apply.args.len() != 1 {
        return Err(src("setter must be one regular single-argument Apply"));
    }
    let source_block = rhs;
    let typed_block = source_typed_index
        .get(source_id, source_block)
        .ok_or_else(|| typed("original Block has no typed mapping"))?;
    let typed_declarations = [
        source_typed_index
            .get(source_id, declarations[0])
            .ok_or_else(|| typed("first ValDef has no typed mapping"))?,
        source_typed_index
            .get(source_id, declarations[1])
            .ok_or_else(|| typed("second ValDef has no typed mapping"))?,
        source_typed_index
            .get(source_id, declarations[2])
            .ok_or_else(|| typed("mutable selected ValDef has no typed mapping"))?,
    ];
    let typed_assignment = source_typed_index
        .get(source_id, source_assignment)
        .ok_or_else(|| typed("Assign has no typed mapping"))?;
    let typed_setter = source_typed_index
        .get(source_id, source_setter)
        .ok_or_else(|| typed("setter has no typed mapping"))?;
    let typed_pair_result = source_typed_index
        .get(source_id, block.expr)
        .ok_or_else(|| typed("final Pair expression has no typed mapping"))?;
    let TreeKind::Block(typed_block_node) = &typed_ast
        .try_get(typed_block)
        .ok_or_else(|| typed("typed Block is stale"))?
        .kind
    else {
        return Err(typed("original source Block must remain typed Block"));
    };
    if typed_block_node.stats.as_slice()
        != [
            typed_declarations[0],
            typed_declarations[1],
            typed_declarations[2],
            typed_assignment,
            typed_setter,
        ]
        || typed_block_node.expr != typed_pair_result
    {
        return Err(typed(
            "typed Block must preserve exact ordered source-stat and Pair mappings",
        ));
    }
    let source_initializers = [
        match &source_ast
            .try_get(declarations[0])
            .ok_or_else(|| src("first local is stale"))?
            .kind
        {
            TreeKind::ValDef(d) => d
                .rhs
                .ok_or_else(|| src("first Counter initializer is absent"))?,
            _ => return Err(src("first statistic must be a Counter ValDef")),
        },
        match &source_ast
            .try_get(declarations[1])
            .ok_or_else(|| src("second local is stale"))?
            .kind
        {
            TreeKind::ValDef(d) => d
                .rhs
                .ok_or_else(|| src("second Counter initializer is absent"))?,
            _ => return Err(src("second statistic must be a Counter ValDef")),
        },
        match &source_ast
            .try_get(declarations[2])
            .ok_or_else(|| src("selected local is stale"))?
            .kind
        {
            TreeKind::ValDef(d) => d
                .rhs
                .ok_or_else(|| src("mutable selected initializer is absent"))?,
            _ => return Err(src("third statistic must be a mutable selected ValDef")),
        },
    ];
    let counter = {
        let binding =
            original_bindings[0].ok_or_else(|| sid("first local has no original Typer binding"))?;
        if !store.symbols.contains(binding.symbol()) {
            return Err(sid("first local binding has a stale symbol"));
        }
        let SymbolInfo::Complete(ty) = store.symbols.get(binding.symbol()).info else {
            return Err(sem("first local type is incomplete"));
        };
        match store.types.try_get(ty) {
            Some(Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            }) => *symbol,
            _ => return Err(sem("first local must have canonical Counter class type")),
        }
    };
    if !store.symbols.contains(counter)
        || store.symbols.get(counter).kind != SymbolKind::Class
        || source_index.definition_of(counter).is_none_or(
            |d| !matches!(d, SourceDefinition::Canonical { source, .. } if source == source_id),
        )
    {
        return Err(sid("Counter type must be the canonical source class"));
    }
    let mut seed_formals = [formals[0]; 2];
    for i in 0..2 {
        let TreeKind::Apply(init) = &source_ast
            .try_get(source_initializers[i])
            .ok_or_else(|| src("Counter initializer is stale"))?
            .kind
        else {
            return Err(src(
                "Counter initializer must be direct constructor application",
            ));
        };
        if init.args.len() != 1 {
            return Err(src("Counter initializer must consume one Int formal"));
        }
        let typed_arg = source_typed_index
            .get(source_id, init.args[0])
            .ok_or_else(|| typed("Counter seed has no typed mapping"))?;
        let seed = match store.types.try_get(
            typed_ast
                .try_get(typed_arg)
                .ok_or_else(|| typed("typed Counter seed is stale"))?
                .ty,
        ) {
            Some(Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            }) => *symbol,
            _ => return Err(typed("Counter seed must retain its formal TermRef")),
        };
        if !formals[..2].contains(&seed) {
            return Err(typed(
                "Counter seeds must use the first two original Int formals",
            ));
        }
        seed_formals[i] = seed;
    }
    if seed_formals[0] == seed_formals[1] {
        return Err(typed(
            "the two Counter locals must use distinct seed formals",
        ));
    }
    let originals = [
        validate_immutable_local_declaration_core(
            source_id,
            method,
            declarations[0],
            store,
            definitions,
            source_ast,
            typed_ast,
            source_typed_index,
            original_bindings[0],
            counter,
            package,
            seed_formals[0],
        )?,
        validate_immutable_local_declaration_core(
            source_id,
            method,
            declarations[1],
            store,
            definitions,
            source_ast,
            typed_ast,
            source_typed_index,
            original_bindings[1],
            counter,
            package,
            seed_formals[1],
        )?,
    ];
    if originals[0].symbol == originals[1].symbol || originals[0].scope != originals[1].scope {
        return Err(sid(
            "original Counter locals must be distinct immutable locals in one method scope",
        ));
    }
    let semantic_first = if seed_formals[0] == formals[0] {
        originals[0].symbol
    } else {
        originals[1].symbol
    };
    let semantic_second = if semantic_first == originals[0].symbol {
        originals[1].symbol
    } else {
        originals[0].symbol
    };
    let selected_binding = selected_binding
        .ok_or_else(|| sid("mutable selected local has no original Typer binding"))?;
    let selected = selected_binding.symbol();
    let TreeKind::ValDef(selected_def) = &source_ast
        .try_get(declarations[2])
        .ok_or_else(|| src("mutable selected declaration is stale"))?
        .kind
    else {
        return Err(src(
            "third Block statistic must be the mutable selected ValDef",
        ));
    };
    if !store.symbols.contains(selected) {
        return Err(sid("mutable selected binding has a stale symbol"));
    }
    let local_type = match store.symbols.get(selected).info {
        SymbolInfo::Complete(ty) => ty,
        _ => return Err(sem("mutable selected type is incomplete")),
    };
    let selected_typed = selected_binding.declaration();
    if source_typed_index.get(source_id, declarations[2]) != Some(selected_typed)
        || !store.symbols.contains(selected)
        || store.symbols.get(selected).kind != SymbolKind::Local
        || store.symbols.get(selected).owner != Some(method)
        || !store
            .symbols
            .get(selected)
            .flags
            .contains(SymbolFlags::MUTABLE)
        || !has_source_origin(store, selected, source_id)
        || selected_binding.scope() != originals[0].scope
        || selected_binding.scope() != originals[1].scope
        || !store.scopes.contains(selected_binding.scope())
        || store.scopes.get(selected_binding.scope()).owner != Some(method)
        || store
            .scopes
            .get(selected_binding.scope())
            .lookup(&store.symbols.get(selected).name)
            != Some(selected)
        || !is_canonical_class_type_ref(store, local_type, counter, package, definitions.no_prefix)
    {
        return Err(sid(
            "selected must be the canonical mutable Counter local in the original method Block scope",
        ));
    }
    if matches!(
        source_ast.try_get(selected_def.tpt).map(|n| &n.kind),
        Some(TreeKind::TypeTree(_))
    ) {
        return Err(src(
            "mutable selected local must retain its explicit Counter type",
        ));
    }
    let selected_initializer = selected_def
        .rhs
        .ok_or_else(|| src("mutable selected initializer is absent"))?;
    let source_init_ident = match &source_ast
        .try_get(selected_initializer)
        .ok_or_else(|| src("selected initializer is stale"))?
        .kind
    {
        TreeKind::Ident(i) => i,
        _ => {
            return Err(src(
                "mutable selected initializer must directly reference the first original local",
            ));
        }
    };
    let typed_selected = typed_ast
        .try_get(selected_typed)
        .ok_or_else(|| typed("mutable selected typed ValDef is stale"))?;
    let TreeKind::ValDef(typed_selected_def) = &typed_selected.kind else {
        return Err(typed("mutable declaration must remain typed ValDef"));
    };
    let typed_type_tree = source_typed_index
        .get(source_id, selected_def.tpt)
        .ok_or_else(|| typed("mutable explicit annotation has no typed mapping"))?;
    if typed_selected_def.tpt != typed_type_tree
        || !matches!(
            typed_ast.try_get(typed_type_tree).map(|node| &node.kind),
            Some(TreeKind::TypeTree(_))
        )
        || !matches!(
            typed_ast.try_get(typed_type_tree).map(|node| node.ty),
            Some(ty) if is_canonical_class_type_ref(
                store,
                ty,
                counter,
                package,
                definitions.no_prefix,
            )
        )
    {
        return Err(typed(
            "mutable typed ValDef must preserve its exact canonical Counter annotation root",
        ));
    }
    let typed_initializer = source_typed_index
        .get(source_id, selected_initializer)
        .ok_or_else(|| typed("mutable initializer has no typed mapping"))?;
    if typed_selected_def.rhs != Some(typed_initializer) || typed_selected.ty != local_type {
        return Err(typed(
            "typed mutable declaration must retain the exact initializer and Counter type",
        ));
    }
    let selected_initializer_target = match store.types.try_get(
        typed_ast
            .try_get(typed_initializer)
            .ok_or_else(|| typed("typed mutable initializer is stale"))?
            .ty,
    ) {
        Some(Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        }) => *symbol,
        _ => {
            return Err(typed(
                "mutable initializer must resolve to an original local TermRef",
            ));
        }
    };
    if selected_initializer_target != semantic_first
        || !store.symbols.contains(selected_initializer_target)
        || !originals
            .iter()
            .any(|local| local.symbol == selected_initializer_target)
        || source_init_ident.name != store.symbols.get(selected_initializer_target).name
        || !matches!(
            typed_ast.try_get(typed_initializer).map(|node| &node.kind),
            Some(TreeKind::Ident(ident)) if ident.name == store.symbols.get(selected_initializer_target).name
        )
    {
        return Err(typed(
            "mutable initializer must reference the exact first original Counter local",
        ));
    }
    let source_assignment_lhs = assign.lhs;
    let source_assignment_rhs = assign.rhs;
    let (TreeKind::Ident(lhs_ident), TreeKind::Ident(rhs_ident)) = (
        &source_ast
            .try_get(source_assignment_lhs)
            .ok_or_else(|| src("assignment LHS is stale"))?
            .kind,
        &source_ast
            .try_get(source_assignment_rhs)
            .ok_or_else(|| src("assignment RHS is stale"))?
            .kind,
    ) else {
        return Err(src(
            "assignment must directly rebind selected to an original local",
        ));
    };
    let typed_assignment_lhs = source_typed_index
        .get(source_id, source_assignment_lhs)
        .ok_or_else(|| typed("assignment LHS has no typed mapping"))?;
    let typed_assignment_rhs = source_typed_index
        .get(source_id, source_assignment_rhs)
        .ok_or_else(|| typed("assignment RHS has no typed mapping"))?;
    let TreeKind::Assign(typed_assign) = &typed_ast
        .try_get(typed_assignment)
        .ok_or_else(|| typed("typed Assign is stale"))?
        .kind
    else {
        return Err(typed("source assignment must remain typed Assign"));
    };
    if typed_assign.lhs != typed_assignment_lhs || typed_assign.rhs != typed_assignment_rhs {
        return Err(typed(
            "typed Assign must preserve exact LHS and RHS source mappings",
        ));
    }
    let lhs_node = typed_ast
        .try_get(typed_assignment_lhs)
        .ok_or_else(|| typed("typed assignment LHS is stale"))?;
    let rhs_node = typed_ast
        .try_get(typed_assignment_rhs)
        .ok_or_else(|| typed("typed assignment RHS is stale"))?;
    let assigned_target = match store.types.try_get(rhs_node.ty) {
        Some(Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        }) => *symbol,
        _ => {
            return Err(typed(
                "assignment RHS must retain an original local TermRef",
            ));
        }
    };
    if !store.symbols.contains(assigned_target)
        || !originals
            .iter()
            .any(|local| local.symbol == assigned_target)
    {
        return Err(typed(
            "assignment RHS must designate one of the canonical original local symbols",
        ));
    }
    if !matches!(&lhs_node.kind, TreeKind::Ident(i) if i.name == store.symbols.get(selected).name)
        || !matches!(store.types.try_get(lhs_node.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(s), .. }) if *s == selected)
        || lhs_ident.name != store.symbols.get(selected).name
    {
        return Err(typed(
            "assignment LHS must resolve to the exact mutable selected local",
        ));
    }
    if !matches!(&rhs_node.kind, TreeKind::Ident(i) if i.name == store.symbols.get(assigned_target).name)
        || rhs_ident.name != store.symbols.get(assigned_target).name
    {
        return Err(typed(
            "assignment RHS source and typed names must resolve to the same canonical local",
        ));
    }
    if !matches!(store.types.try_get(rhs_node.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(s), .. }) if *s == assigned_target)
    {
        return Err(typed(
            "assignment RHS must retain its exact canonical local TermRef",
        ));
    }
    let assigned_type = match store.symbols.get(assigned_target).info {
        SymbolInfo::Complete(ty) => ty,
        _ => return Err(sem("assignment RHS local type is incomplete")),
    };
    if !is_canonical_class_type_ref(
        store,
        assigned_type,
        counter,
        package,
        definitions.no_prefix,
    ) {
        return Err(typed(
            "assignment RHS local must have the selected Counter type",
        ));
    }
    if !originals
        .iter()
        .any(|local| local.symbol == assigned_target)
    {
        return Err(typed(
            "assignment RHS must be one of the exact initialized Counter locals",
        ));
    }
    let typed_assign_node = typed_ast
        .try_get(typed_assignment)
        .ok_or_else(|| typed("typed Assign is stale"))?;
    if typed_assign_node.ty != definitions.unit {
        return Err(typed("typed reference assignment must have Unit type"));
    }
    let (source_setter_function, source_setter_receiver, source_setter_argument) = {
        let TreeKind::Apply(apply) = &source_ast
            .try_get(source_setter)
            .ok_or_else(|| src("setter is stale"))?
            .kind
        else {
            return Err(src("setter statement must be Apply"));
        };
        let TreeKind::Select(selection) = &source_ast
            .try_get(apply.function)
            .ok_or_else(|| src("setter function is stale"))?
            .kind
        else {
            return Err(src("setter function must be Select"));
        };
        if apply.kind != ApplyKind::Regular
            || apply.args.len() != 1
            || store.names.resolve(selection.name.text()) != "set"
        {
            return Err(src("setter must be one direct Counter.set call"));
        }
        (apply.function, selection.qualifier, apply.args[0])
    };
    let (TreeKind::Ident(setter_receiver_ident), TreeKind::Ident(setter_argument_ident)) = (
        &source_ast
            .try_get(source_setter_receiver)
            .ok_or_else(|| src("setter receiver is stale"))?
            .kind,
        &source_ast
            .try_get(source_setter_argument)
            .ok_or_else(|| src("setter argument is stale"))?
            .kind,
    ) else {
        return Err(src(
            "setter receiver and argument must be direct identifiers",
        ));
    };
    let typed_setter_receiver = source_typed_index
        .get(source_id, source_setter_receiver)
        .ok_or_else(|| typed("setter receiver has no typed mapping"))?;
    let typed_setter_argument = source_typed_index
        .get(source_id, source_setter_argument)
        .ok_or_else(|| typed("setter argument has no typed mapping"))?;
    let TreeKind::Select(source_setter_select) = &source_ast
        .try_get(source_setter_function)
        .ok_or_else(|| src("source setter selection is stale"))?
        .kind
    else {
        return Err(src("source setter function must remain Select"));
    };
    let typed_setter_function = source_typed_index
        .get(source_id, source_setter_function)
        .ok_or_else(|| typed("setter selection has no typed mapping"))?;
    let TreeKind::Apply(typed_setter_node) = &typed_ast
        .try_get(typed_setter)
        .ok_or_else(|| typed("typed setter is stale"))?
        .kind
    else {
        return Err(typed("setter must remain typed Apply"));
    };
    let TreeKind::Select(typed_setter_select) = &typed_ast
        .try_get(typed_setter_node.function)
        .ok_or_else(|| typed("typed setter function is stale"))?
        .kind
    else {
        return Err(typed("typed setter function must remain Select"));
    };
    let typed_receiver_node = typed_ast
        .try_get(typed_setter_receiver)
        .ok_or_else(|| typed("typed setter receiver is stale"))?;
    let typed_argument_node = typed_ast
        .try_get(typed_setter_argument)
        .ok_or_else(|| typed("typed setter argument is stale"))?;
    let setter_method = match store.types.try_get(
        typed_ast
            .try_get(typed_setter_node.function)
            .ok_or_else(|| typed("typed setter Select is stale"))?
            .ty,
    ) {
        Some(Type::TermRef {
            target: TermRefTarget::Symbol(method),
            ..
        }) => *method,
        _ => {
            return Err(typed(
                "typed setter selection must retain canonical method identity",
            ));
        }
    };
    if typed_setter_node.function != typed_setter_function
        || typed_setter_node.kind != ApplyKind::Regular
        || typed_setter_node.args.as_slice() != [typed_setter_argument]
        || typed_setter_select.qualifier != typed_setter_receiver
        || !matches!(&typed_receiver_node.kind, TreeKind::Ident(i) if i.name == store.symbols.get(selected).name)
        || !matches!(store.types.try_get(typed_receiver_node.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(s), .. }) if *s == selected)
        || !matches!(&typed_argument_node.kind, TreeKind::Ident(i) if i.name == store.symbols.get(formals[2]).name)
        || !matches!(store.types.try_get(typed_argument_node.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(s), .. }) if *s == formals[2])
        || setter_receiver_ident.name != store.symbols.get(selected).name
        || setter_argument_ident.name != store.symbols.get(formals[2]).name
        || !store.symbols.contains(setter_method)
        || store.symbols.get(setter_method).kind != SymbolKind::Method
        || store.symbols.get(setter_method).owner != Some(counter)
        || source_setter_select.name != store.symbols.get(setter_method).name
        || typed_setter_select.name != store.symbols.get(setter_method).name
        || store
            .names
            .resolve(store.symbols.get(setter_method).name.text())
            != "set"
        || typed_ast
            .try_get(typed_setter)
            .ok_or_else(|| typed("typed setter is stale"))?
            .ty
            != definitions.unit
    {
        return Err(typed(
            "setter must use the selected local and final Int formal exactly once",
        ));
    }
    let (source_pair_function, source_pair_new, source_pair_arguments) = {
        let TreeKind::Apply(pair) = &source_ast
            .try_get(block.expr)
            .ok_or_else(|| src("Pair result is stale"))?
            .kind
        else {
            return Err(src("Block result must construct Pair"));
        };
        let TreeKind::Select(selection) = &source_ast
            .try_get(pair.function)
            .ok_or_else(|| src("Pair constructor function is stale"))?
            .kind
        else {
            return Err(src("Pair result must select a constructor"));
        };
        if pair.kind != ApplyKind::Regular || pair.args.len() != 2 {
            return Err(src(
                "final Pair construction must have two ordered arguments",
            ));
        }
        let TreeKind::New(_) = &source_ast
            .try_get(selection.qualifier)
            .ok_or_else(|| src("Pair new is stale"))?
            .kind
        else {
            return Err(src("final expression must construct a new Pair"));
        };
        (
            pair.function,
            selection.qualifier,
            [pair.args[0], pair.args[1]],
        )
    };
    let typed_pair_arguments = {
        let TreeKind::Apply(pair) = &typed_ast
            .try_get(typed_pair_result)
            .ok_or_else(|| typed("typed Pair result is stale"))?
            .kind
        else {
            return Err(typed("typed Block result must remain Pair Apply"));
        };
        if pair.args.len() != 2 {
            return Err(typed("typed Pair construction must retain two arguments"));
        }
        [pair.args[0], pair.args[1]]
    };
    let typed_pair_function = source_typed_index
        .get(source_id, source_pair_function)
        .ok_or_else(|| typed("Pair constructor function has no typed mapping"))?;
    let typed_pair_new = source_typed_index
        .get(source_id, source_pair_new)
        .ok_or_else(|| typed("Pair New has no typed mapping"))?;
    let source_pair_new_type = match &source_ast
        .try_get(source_pair_new)
        .ok_or_else(|| src("Pair New is stale"))?
        .kind
    {
        TreeKind::New(new) => new.tpt,
        _ => return Err(src("Pair constructor qualifier must remain New")),
    };
    let typed_pair_new_type = source_typed_index
        .get(source_id, source_pair_new_type)
        .ok_or_else(|| typed("Pair New type tree has no typed mapping"))?;
    let TreeKind::Apply(typed_pair_node) = &typed_ast
        .try_get(typed_pair_result)
        .ok_or_else(|| typed("typed Pair result is stale"))?
        .kind
    else {
        return Err(typed("typed Block result must remain Pair Apply"));
    };
    let TreeKind::Select(typed_pair_selection) = &typed_ast
        .try_get(typed_pair_function)
        .ok_or_else(|| typed("typed Pair constructor function is stale"))?
        .kind
    else {
        return Err(typed("typed Pair constructor function must remain Select"));
    };
    let TreeKind::Select(source_pair_selection) = &source_ast
        .try_get(source_pair_function)
        .ok_or_else(|| src("source Pair constructor selection is stale"))?
        .kind
    else {
        return Err(src("source Pair constructor function must remain Select"));
    };
    let TreeKind::New(typed_pair_new_node) = &typed_ast
        .try_get(typed_pair_new)
        .ok_or_else(|| typed("typed Pair New is stale"))?
        .kind
    else {
        return Err(typed("typed Pair constructor qualifier must remain New"));
    };
    if typed_pair_node.function != typed_pair_function
        || typed_pair_node.kind != ApplyKind::Regular
        || typed_pair_selection.qualifier != typed_pair_new
        || typed_pair_new_node.tpt != typed_pair_new_type
        || !matches!(
            typed_ast.try_get(typed_pair_new).map(|node| &node.kind),
            Some(TreeKind::New(_))
        )
    {
        return Err(typed(
            "typed Pair constructor must preserve its exact source function and New mappings",
        ));
    }
    let pair_constructor = match store.types.try_get(
        typed_ast
            .try_get(typed_pair_function)
            .ok_or_else(|| typed("typed Pair constructor selection is stale"))?
            .ty,
    ) {
        Some(Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        }) => *symbol,
        _ => {
            return Err(typed(
                "typed Pair selection must retain canonical constructor identity",
            ));
        }
    };
    if !store.symbols.contains(pair_constructor) {
        return Err(typed(
            "typed Pair selection has a stale constructor symbol identity",
        ));
    }
    if source_pair_selection.name != store.symbols.get(pair_constructor).name
        || typed_pair_selection.name != store.symbols.get(pair_constructor).name
        || store
            .names
            .resolve(store.symbols.get(pair_constructor).name.text())
            != "<init>"
    {
        return Err(typed(
            "source and typed Pair selections must retain the canonical constructor name",
        ));
    }
    let expected_pair_args = [semantic_first, semantic_second];
    for i in 0..2 {
        let source_arg = match &source_ast
            .try_get(source_pair_arguments[i])
            .ok_or_else(|| src("Pair argument is stale"))?
            .kind
        {
            TreeKind::Ident(ident) => ident,
            _ => {
                return Err(src(
                    "Pair arguments must directly reference the original Counter locals",
                ));
            }
        };
        let mapped = source_typed_index
            .get(source_id, source_pair_arguments[i])
            .ok_or_else(|| typed("Pair argument has no typed mapping"))?;
        if mapped != typed_pair_arguments[i] {
            return Err(typed(
                "typed Pair arguments must preserve their exact source mappings",
            ));
        }
        let node = typed_ast
            .try_get(mapped)
            .ok_or_else(|| typed("typed Pair argument is stale"))?;
        let symbol = match store.types.try_get(node.ty) {
            Some(Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            }) => *symbol,
            _ => {
                return Err(typed(
                    "typed Pair argument must retain local TermRef identity",
                ));
            }
        };
        if !store.symbols.contains(symbol) || !expected_pair_args.contains(&symbol) {
            return Err(typed(
                "Pair argument must designate one of the canonical original local symbols",
            ));
        }
        if symbol != expected_pair_args[i]
            || !matches!(&node.kind, TreeKind::Ident(ident) if ident.name == store.symbols.get(symbol).name)
            || source_arg.name != store.symbols.get(symbol).name
        {
            return Err(typed(
                "final Pair arguments must remain first and second original Counter identities in order",
            ));
        }
    }
    let pair_result_type = typed_ast
        .try_get(typed_pair_result)
        .ok_or_else(|| typed("typed Pair result is stale"))?
        .ty;
    let pair_class = match store.types.try_get(pair_result_type) {
        Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(pair),
            ..
        }) => *pair,
        _ => {
            return Err(typed(
                "final result must retain a canonical source Pair class type",
            ));
        }
    };
    if !store.symbols.contains(pair_class) {
        return Err(typed("typed Pair result has a stale class symbol identity"));
    }
    let pair_new_type = typed_ast
        .try_get(typed_pair_new)
        .ok_or_else(|| typed("typed Pair New is stale"))?
        .ty;
    let result_type = signature.result;
    let typed_pair_type_tree = typed_ast
        .try_get(typed_pair_new_type)
        .ok_or_else(|| typed("typed Pair New type tree is stale"))?;
    if !matches!(&typed_pair_type_tree.kind, TreeKind::TypeTree(_))
        || !is_canonical_class_type_ref(
            store,
            typed_pair_type_tree.ty,
            pair_class,
            package,
            definitions.no_prefix,
        )
        || typed_ast
            .try_get(typed_block)
            .is_none_or(|typed_block_tree| typed_block_tree.ty != pair_result_type)
    {
        return Err(typed(
            "typed Pair New annotation and Block result must retain the exact canonical Pair type",
        ));
    }
    if source_index.definition_of(pair_class).is_none_or(
        |d| !matches!(d, SourceDefinition::Canonical { source, .. } if source == source_id),
    ) || store.symbols.get(pair_class).owner != Some(package)
        || store
            .names
            .resolve(store.symbols.get(pair_class).name.text())
            != "Pair"
        || !matches!(
            store.symbols.get(pair_constructor).kind,
            SymbolKind::Constructor
        )
        || store.symbols.get(pair_constructor).owner != Some(pair_class)
        || !matches!(store.types.try_get(pair_new_type), Some(Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }) if *class == pair_class)
        || !matches!(store.types.try_get(result_type), Some(Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }) if *class == pair_class)
    {
        return Err(typed(
            "factory result, Pair New, and canonical Pair constructor identities must agree",
        ));
    }
    for (source_tree, typed_tree, description) in [
        (source_block, typed_block, "factory Block"),
        (declarations[0], typed_declarations[0], "first local"),
        (declarations[1], typed_declarations[1], "second local"),
        (declarations[2], typed_declarations[2], "selected local"),
        (
            originals[0].source_type_tree,
            originals[0].typed_type_tree,
            "first local annotation",
        ),
        (
            originals[1].source_type_tree,
            originals[1].typed_type_tree,
            "second local annotation",
        ),
        (
            selected_def.tpt,
            typed_type_tree,
            "selected local annotation",
        ),
        (
            originals[0].initializer,
            originals[0].typed_initializer,
            "first initializer",
        ),
        (
            originals[0].source_select,
            originals[0].typed_select,
            "first constructor selection",
        ),
        (
            originals[0].source_new,
            originals[0].typed_new,
            "first constructor New",
        ),
        (
            originals[0].source_new_type_tree,
            originals[0].typed_new_type_tree,
            "first constructor type tree",
        ),
        (
            originals[0].source_argument,
            originals[0].typed_argument,
            "first seed argument",
        ),
        (
            originals[1].initializer,
            originals[1].typed_initializer,
            "second initializer",
        ),
        (
            originals[1].source_select,
            originals[1].typed_select,
            "second constructor selection",
        ),
        (
            originals[1].source_new,
            originals[1].typed_new,
            "second constructor New",
        ),
        (
            originals[1].source_new_type_tree,
            originals[1].typed_new_type_tree,
            "second constructor type tree",
        ),
        (
            originals[1].source_argument,
            originals[1].typed_argument,
            "second seed argument",
        ),
        (
            selected_initializer,
            typed_initializer,
            "selected initializer",
        ),
        (source_assignment, typed_assignment, "assignment"),
        (
            source_assignment_lhs,
            typed_assignment_lhs,
            "assignment LHS",
        ),
        (
            source_assignment_rhs,
            typed_assignment_rhs,
            "assignment RHS",
        ),
        (source_setter, typed_setter, "setter call"),
        (
            source_setter_function,
            typed_setter_function,
            "setter selection",
        ),
        (
            source_setter_receiver,
            typed_setter_receiver,
            "setter receiver",
        ),
        (
            source_setter_argument,
            typed_setter_argument,
            "setter argument",
        ),
        (block.expr, typed_pair_result, "Pair result"),
        (
            source_pair_function,
            typed_pair_function,
            "Pair constructor selection",
        ),
        (source_pair_new, typed_pair_new, "Pair New"),
        (
            source_pair_new_type,
            typed_pair_new_type,
            "Pair constructor type tree",
        ),
        (
            source_pair_arguments[0],
            typed_pair_arguments[0],
            "first Pair argument",
        ),
        (
            source_pair_arguments[1],
            typed_pair_arguments[1],
            "second Pair argument",
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
    let typed_initializers = [
        originals[0].typed_initializer,
        originals[1].typed_initializer,
        typed_initializer,
    ];
    Ok(ValidatedMutableLocalReferenceReassignmentProof {
        _store: store,
        _source_ast: source_ast,
        _typed_ast: typed_ast,
        _source_index: source_index,
        _source_typed_index: source_typed_index,
        _jvm_unit: jvm_unit,
        source_id,
        owner,
        method,
        scope: selected_binding.scope(),
        source_block,
        typed_block,
        source_declarations: declarations,
        typed_declarations,
        source_initializers,
        typed_initializers,
        source_assignment,
        typed_assignment,
        source_assignment_lhs,
        typed_assignment_lhs,
        source_assignment_rhs,
        typed_assignment_rhs,
        selected,
        initializer_target: selected_initializer_target,
        assigned_target,
        counter,
        local_type,
        source_setter,
        typed_setter,
        source_setter_receiver,
        typed_setter_receiver,
        source_setter_argument,
        typed_setter_argument,
        setter_method,
        source_pair_result: block.expr,
        typed_pair_result,
        source_pair_arguments,
        typed_pair_arguments,
        original_locals: [originals[0].symbol, originals[1].symbol],
        semantic_originals: [semantic_first, semantic_second],
        prefix_locals: [
            original_bindings[0]
                .ok_or_else(|| sid("first local has no original Typer binding"))?
                .symbol(),
            original_bindings[1]
                .ok_or_else(|| sid("second local has no original Typer binding"))?
                .symbol(),
        ],
    })
}
