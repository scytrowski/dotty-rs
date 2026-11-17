//! Borrowed proof for the original Unit-valued conditional assignments of a
//! mutable Counter reference local. The receipt authenticates identities and
//! provenance only; it does not authorize JVM or TASTy emission.

use super::*;
use dotty_core::ast::Modifier;

/// Inputs from one real frontend session for the frozen conditional Counter
/// assignment body. Local bindings must come from that session's SourceTyper.
#[derive(Clone, Copy)]
pub struct ConditionalMutableReferenceAssignmentProofInput<'session> {
    pub source_id: SourceId,
    pub method_tree: TreeId<Untyped>,
    pub store: &'session SemanticStore,
    pub definitions: &'session Definitions,
    pub source_ast: &'session AstArena<Untyped>,
    pub typed_ast: &'session TypedAst,
    pub source_index: &'session SourceSemanticIndex,
    pub source_typed_index: &'session SourceTypedIndex,
    pub original_bindings: [Option<TypedLocalBinding>; 2],
    pub selected_binding: Option<TypedLocalBinding>,
    pub jvm_unit: &'session JvmCompilationUnit<'session>,
}

/// Opaque borrowed identity receipt for the original Unit If and its ordered
/// THEN/ELSE assignments to one initialized mutable Counter local.
pub struct ValidatedConditionalMutableReferenceAssignmentProof<'session> {
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
    source_if: TreeId<Untyped>,
    typed_if: TreeId<Typed>,
    source_condition: TreeId<Untyped>,
    typed_condition: TreeId<Typed>,
    source_assignments: [TreeId<Untyped>; 2],
    typed_assignments: [TreeId<Typed>; 2],
    source_lhss: [TreeId<Untyped>; 2],
    typed_lhss: [TreeId<Typed>; 2],
    source_rhss: [TreeId<Untyped>; 2],
    typed_rhss: [TreeId<Typed>; 2],
    condition_formal: SymbolId,
    selected: SymbolId,
    rhs_locals: [SymbolId; 2],
    local_type: TypeId,
}

impl ValidatedConditionalMutableReferenceAssignmentProof<'_> {
    /// Read-only canonical source/typed binding for the two ordered arms.
    pub fn binding(&self) -> dotty_tasty_pickler::ConditionalMutableReferenceAssignmentBinding {
        dotty_tasty_pickler::ConditionalMutableReferenceAssignmentBinding {
            source_id: self.source_id,
            owner: self.owner,
            method: self.method,
            scope: self.scope,
            source_block: self.source_block,
            typed_block: self.typed_block,
            source_if: self.source_if,
            typed_if: self.typed_if,
            source_condition: self.source_condition,
            typed_condition: self.typed_condition,
            source_assignments: self.source_assignments,
            typed_assignments: self.typed_assignments,
            source_lhss: self.source_lhss,
            typed_lhss: self.typed_lhss,
            source_rhss: self.source_rhss,
            typed_rhss: self.typed_rhss,
            condition_formal: self.condition_formal,
            local: self.selected,
            rhs_locals: self.rhs_locals,
            local_type: self.local_type,
        }
    }

    pub const fn source_id(&self) -> SourceId {
        self.source_id
    }
    pub const fn owner(&self) -> SymbolId {
        self.owner
    }
    pub const fn method(&self) -> SymbolId {
        self.method
    }
    pub const fn scope(&self) -> dotty_core::ScopeId {
        self.scope
    }
    pub const fn source_if(&self) -> TreeId<Untyped> {
        self.source_if
    }
    pub const fn typed_if(&self) -> TreeId<Typed> {
        self.typed_if
    }
    pub const fn source_assignments(&self) -> [TreeId<Untyped>; 2] {
        self.source_assignments
    }
    pub const fn typed_assignments(&self) -> [TreeId<Typed>; 2] {
        self.typed_assignments
    }
    pub const fn selected(&self) -> SymbolId {
        self.selected
    }
    pub const fn rhs_locals(&self) -> [SymbolId; 2] {
        self.rhs_locals
    }
}

/// Authenticate the exact original `create` method body from one real source
/// and typed session. The ordered body is two initialized immutable Counter
/// locals, an initialized mutable `selected` local, one Boolean Unit If with
/// two Assign arms, one setter call, and a Pair construction preserving the
/// original locals. The function returns a receipt only after all checks pass.
pub fn validate_conditional_mutable_reference_assignment_proof<'session>(
    input: ConditionalMutableReferenceAssignmentProofInput<'session>,
) -> Result<
    ValidatedConditionalMutableReferenceAssignmentProof<'session>,
    CrossClassConstructorCallProofError,
> {
    let src = |s| CrossClassConstructorCallProofError::SourceShape(s);
    let sid = |s| CrossClassConstructorCallProofError::SourceIdentity(s);
    let sem = |s| CrossClassConstructorCallProofError::SemanticIdentity(s);
    let typed = |s| CrossClassConstructorCallProofError::TypedBinding(s);
    let ConditionalMutableReferenceAssignmentProofInput {
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
        || !has_source_origin(store, method, source_id)
        || store.names.resolve(store.symbols.get(method).name.text()) != "create"
    {
        return Err(sid(
            "factory method is not the canonical source create method",
        ));
    }
    let owner = store
        .symbols
        .get(method)
        .owner
        .ok_or_else(|| sid("factory method has no class owner"))?;
    if !store.symbols.contains(owner)
        || store.symbols.get(owner).kind != SymbolKind::Class
        || store.names.resolve(store.symbols.get(owner).name.text()) != "PairFactory"
        || !has_source_origin(store, owner, source_id)
        || source_index.definition_of(owner).is_none_or(
            |d| !matches!(d, SourceDefinition::Canonical { source, .. } if source == source_id),
        )
    {
        return Err(sid(
            "factory owner must be the canonical source PairFactory class",
        ));
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
        || method_def.value_param_clauses[0].len() != 4
        || signature.params.len() != 4
        || signature.kind != dotty_core::MethodKind::Plain
        || signature.params[0].ty != definitions.int
        || signature.params[1].ty != definitions.int
        || signature.params[2].ty != definitions.boolean
        || signature.params[3].ty != definitions.int
    {
        return Err(src(
            "factory method must have ordered Int, Int, Boolean, Int formals",
        ));
    }
    let method_scope = source_index
        .scope_of(method)
        .ok_or_else(|| sid("factory method has no formal scope"))?;
    if !store.scopes.contains(method_scope) || store.scopes.get(method_scope).owner != Some(method)
    {
        return Err(sid("factory formal scope must be method-owned"));
    }
    let mut formals = Vec::with_capacity(4);
    for (index, tree) in method_def.value_param_clauses[0].iter().enumerate() {
        let TreeKind::ValDef(def) = &source_ast
            .try_get(*tree)
            .ok_or_else(|| src("formal tree is stale"))?
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
            .ok_or_else(|| sid("formal has no canonical symbol"))?;
        let expected = signature.params[index].ty;
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
            || !matches!(store.symbols.get(formal).info, SymbolInfo::Complete(ty) if ty == expected)
        {
            return Err(sem(
                "factory formals must preserve canonical ordered parameter identities",
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
    let formals: [SymbolId; 4] = formals
        .try_into()
        .map_err(|_| src("factory must retain four formals"))?;
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
            "factory Block must contain three locals, one If and one setter",
        ));
    }
    let declarations: [TreeId<Untyped>; 3] = block.stats[..3]
        .try_into()
        .map_err(|_| src("factory must retain three local declarations"))?;
    let source_if = block.stats[3];
    let source_setter = block.stats[4];
    let TreeKind::If(source_if_node) = &source_ast
        .try_get(source_if)
        .ok_or_else(|| src("If statement is stale"))?
        .kind
    else {
        return Err(src("fourth Block statistic must be the original If"));
    };
    let TreeKind::Apply(setter_apply) = &source_ast
        .try_get(source_setter)
        .ok_or_else(|| src("setter is stale"))?
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
            .ok_or_else(|| typed("selected ValDef has no typed mapping"))?,
    ];
    let typed_if = source_typed_index
        .get(source_id, source_if)
        .ok_or_else(|| typed("If has no typed mapping"))?;
    let typed_setter = source_typed_index
        .get(source_id, source_setter)
        .ok_or_else(|| typed("setter has no typed mapping"))?;
    let typed_pair_result = source_typed_index
        .get(source_id, block.expr)
        .ok_or_else(|| typed("Pair result has no typed mapping"))?;
    let TreeKind::Block(typed_block_node) = &typed_ast
        .try_get(typed_block)
        .ok_or_else(|| typed("typed Block is stale"))?
        .kind
    else {
        return Err(typed("original source Block must remain a typed Block"));
    };
    if typed_block_node.stats.as_slice()
        != [
            typed_declarations[0],
            typed_declarations[1],
            typed_declarations[2],
            typed_if,
            typed_setter,
        ]
        || typed_block_node.expr != typed_pair_result
    {
        return Err(typed(
            "typed Block must preserve exact ordered source-stat and Pair mappings",
        ));
    }
    let TreeKind::If(typed_if_node) = &typed_ast
        .try_get(typed_if)
        .ok_or_else(|| typed("typed If is stale"))?
        .kind
    else {
        return Err(typed("source If must remain the exact typed If"));
    };
    if typed_ast.try_get(typed_if).map(|node| node.ty) != Some(definitions.unit) {
        return Err(typed(
            "conditional assignments must have canonical Unit type",
        ));
    }

    let mut seed_formals = [formals[0]; 2];
    for index in 0..2 {
        let TreeKind::ValDef(def) = &source_ast
            .try_get(declarations[index])
            .ok_or_else(|| src("Counter local is stale"))?
            .kind
        else {
            return Err(src("first two Block statistics must be Counter ValDefs"));
        };
        let initializer = def
            .rhs
            .ok_or_else(|| src("Counter local must have an initializer"))?;
        let TreeKind::Apply(apply) = &source_ast
            .try_get(initializer)
            .ok_or_else(|| src("Counter initializer is stale"))?
            .kind
        else {
            return Err(src(
                "Counter initializer must be a direct constructor Apply",
            ));
        };
        if apply.args.len() != 1 {
            return Err(src("Counter initializer must have one seed argument"));
        }
        let typed_seed = source_typed_index
            .get(source_id, apply.args[0])
            .ok_or_else(|| typed("Counter seed has no typed mapping"))?;
        let seed = match store.types.try_get(
            typed_ast
                .try_get(typed_seed)
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
        seed_formals[index] = seed;
    }
    if seed_formals[0] == seed_formals[1] {
        return Err(typed("two Counter locals must use distinct seed formals"));
    }
    let counter = {
        let binding =
            original_bindings[0].ok_or_else(|| sid("first local has no original Typer binding"))?;
        let SymbolInfo::Complete(ty) = store.symbols.get(binding.symbol()).info else {
            return Err(sem("first Counter local type is incomplete"));
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
        return Err(sid(
            "Counter type must be canonical in the same source session",
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
            "original Counter locals must be distinct initialized locals in one Block scope",
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
        .ok_or_else(|| src("selected declaration is stale"))?
        .kind
    else {
        return Err(src(
            "third Block statistic must be the mutable selected ValDef",
        ));
    };
    let local_type = match store
        .symbols
        .contains(selected)
        .then(|| store.symbols.get(selected).info)
    {
        Some(SymbolInfo::Complete(ty)) => ty,
        _ => return Err(sem("selected local type is incomplete")),
    };
    if !store.symbols.contains(selected)
        || store.symbols.get(selected).kind != SymbolKind::Local
        || store.symbols.get(selected).owner != Some(method)
        || !store
            .symbols
            .get(selected)
            .flags
            .contains(SymbolFlags::MUTABLE)
        || !has_source_origin(store, selected, source_id)
        || selected_def.name.as_name() != &store.symbols.get(selected).name
        || !selected_def.metadata.modifiers.contains(&Modifier::Var)
        || source_typed_index.get(source_id, declarations[2])
            != Some(selected_binding.declaration())
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
            "selected must be the original mutable Counter local in the method Block scope",
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
        .ok_or_else(|| src("selected initializer is absent"))?;
    let typed_selected_initializer = source_typed_index
        .get(source_id, selected_initializer)
        .ok_or_else(|| typed("selected initializer has no typed mapping"))?;
    let TreeKind::Ident(selected_initializer_ident) = &source_ast
        .try_get(selected_initializer)
        .ok_or_else(|| src("selected initializer is stale"))?
        .kind
    else {
        return Err(src(
            "selected initializer must be a direct original-local reference",
        ));
    };
    if source_typed_index.get(source_id, selected_initializer) != Some(typed_selected_initializer)
        || !matches!(store.types.try_get(typed_ast.try_get(typed_selected_initializer).ok_or_else(|| typed("selected initializer is stale"))?.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }) if *symbol == semantic_first)
        || selected_initializer_ident.name != store.symbols.get(semantic_first).name
        || !matches!(typed_ast.try_get(typed_selected_initializer).map(|node| &node.kind), Some(TreeKind::Ident(ident)) if ident.name == selected_initializer_ident.name)
        || source_typed_index.get(source_id, selected_initializer)
            != Some(typed_selected_initializer)
    {
        return Err(typed(
            "selected initializer must reference the original first Counter local",
        ));
    }
    let typed_selected_declaration = source_typed_index
        .get(source_id, declarations[2])
        .ok_or_else(|| typed("selected declaration has no typed mapping"))?;
    let TreeKind::ValDef(typed_selected_def) = &typed_ast
        .try_get(typed_selected_declaration)
        .ok_or_else(|| typed("typed selected declaration is stale"))?
        .kind
    else {
        return Err(typed("selected declaration must remain a typed ValDef"));
    };
    let typed_selected_type_tree = source_typed_index
        .get(source_id, selected_def.tpt)
        .ok_or_else(|| typed("selected Counter annotation has no typed mapping"))?;
    if typed_selected_def.rhs != Some(typed_selected_initializer)
        || typed_selected_def.name.as_name() != &store.symbols.get(selected).name
        || typed_selected_def.tpt != typed_selected_type_tree
        || !is_canonical_class_type_ref(
            store,
            typed_ast
                .try_get(typed_selected_type_tree)
                .ok_or_else(|| typed("typed selected annotation is stale"))?
                .ty,
            counter,
            package,
            definitions.no_prefix,
        )
        || typed_ast
            .try_get(typed_selected_declaration)
            .ok_or_else(|| typed("typed selected declaration is stale"))?
            .ty
            != local_type
    {
        return Err(typed(
            "typed selected ValDef must preserve its exact Counter annotation and initializer",
        ));
    }

    let condition_formal = formals[2];
    let source_condition = source_if_node.cond;
    if !matches!(source_ast.try_get(source_condition).map(|node| &node.kind), Some(TreeKind::Ident(ident)) if ident.name == store.symbols.get(condition_formal).name)
    {
        return Err(src(
            "If condition must be the original Boolean chooseSecond formal",
        ));
    }
    let typed_condition = source_typed_index
        .get(source_id, source_condition)
        .ok_or_else(|| typed("If condition has no typed mapping"))?;
    if typed_if_node.cond != typed_condition
        || !matches!(store.types.try_get(typed_ast.try_get(typed_condition).ok_or_else(|| typed("typed condition is stale"))?.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }) if *symbol == condition_formal)
    {
        return Err(typed(
            "typed If must preserve its exact Boolean formal condition",
        ));
    }
    let source_assignments = [source_if_node.then_branch, source_if_node.else_branch];
    let typed_assignments = [typed_if_node.then_branch, typed_if_node.else_branch];
    let expected_rhs = [semantic_second, semantic_first];
    let mut source_lhss = [source_assignments[0]; 2];
    let mut typed_lhss = [typed_assignments[0]; 2];
    let mut source_rhss = [source_assignments[0]; 2];
    let mut typed_rhss = [typed_assignments[0]; 2];
    for index in 0..2 {
        let source_assignment = source_assignments[index];
        let typed_assignment = typed_assignments[index];
        let TreeKind::Assign(source_assign) = &source_ast
            .try_get(source_assignment)
            .ok_or_else(|| src("If arm is stale"))?
            .kind
        else {
            return Err(src(
                "both original If arms must be direct Assign statements",
            ));
        };
        if source_typed_index.get(source_id, source_assignment) != Some(typed_assignment) {
            return Err(typed(
                "each original Assign arm must have its exact SourceTypedIndex edge",
            ));
        }
        let TreeKind::Assign(typed_assign) = &typed_ast
            .try_get(typed_assignment)
            .ok_or_else(|| typed("typed If arm is stale"))?
            .kind
        else {
            return Err(typed(
                "both typed If arms must remain direct Assign statements",
            ));
        };
        source_lhss[index] = source_assign.lhs;
        source_rhss[index] = source_assign.rhs;
        typed_lhss[index] = source_typed_index
            .get(source_id, source_assign.lhs)
            .ok_or_else(|| typed("Assign LHS has no typed mapping"))?;
        typed_rhss[index] = source_typed_index
            .get(source_id, source_assign.rhs)
            .ok_or_else(|| typed("Assign RHS has no typed mapping"))?;
        if typed_assign.lhs != typed_lhss[index] || typed_assign.rhs != typed_rhss[index] {
            return Err(typed(
                "typed Assign must retain the exact mapped LHS and RHS children",
            ));
        }
        if !matches!(store.types.try_get(typed_ast.try_get(typed_lhss[index]).ok_or_else(|| typed("typed LHS is stale"))?.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }) if *symbol == selected)
            || !matches!(store.types.try_get(typed_ast.try_get(typed_rhss[index]).ok_or_else(|| typed("typed RHS is stale"))?.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }) if *symbol == expected_rhs[index])
            || !matches!(source_ast.try_get(source_lhss[index]).map(|node| &node.kind), Some(TreeKind::Ident(ident)) if ident.name == store.symbols.get(selected).name)
            || !matches!(source_ast.try_get(source_rhss[index]).map(|node| &node.kind), Some(TreeKind::Ident(ident)) if ident.name == store.symbols.get(expected_rhs[index]).name)
            || !matches!(typed_ast.try_get(typed_lhss[index]).map(|node| &node.kind), Some(TreeKind::Ident(ident)) if ident.name == store.symbols.get(selected).name)
            || !matches!(typed_ast.try_get(typed_rhss[index]).map(|node| &node.kind), Some(TreeKind::Ident(ident)) if ident.name == store.symbols.get(expected_rhs[index]).name)
        {
            return Err(typed(
                "ordered Assign arms must target the same selected local with second then first RHS",
            ));
        }
        if typed_ast
            .try_get(typed_assignment)
            .ok_or_else(|| typed("typed Assign is stale"))?
            .ty
            != definitions.unit
        {
            return Err(typed("each branch Assign must have canonical Unit type"));
        }
    }

    // The setter and Pair result are authenticated as exact source/typed child
    // graphs, retaining selected and original-local identities.
    let TreeKind::Apply(setter_apply) = &source_ast.try_get(source_setter).unwrap().kind else {
        unreachable!()
    };
    let TreeKind::Select(setter_select) = &source_ast
        .try_get(setter_apply.function)
        .ok_or_else(|| src("setter Select is stale"))?
        .kind
    else {
        return Err(src("setter must select set on the selected local"));
    };
    let typed_setter = source_typed_index
        .get(source_id, source_setter)
        .ok_or_else(|| typed("setter has no typed mapping"))?;
    let typed_setter_function = source_typed_index
        .get(source_id, setter_apply.function)
        .ok_or_else(|| typed("setter Select has no typed mapping"))?;
    let typed_setter_receiver = source_typed_index
        .get(source_id, setter_select.qualifier)
        .ok_or_else(|| typed("setter receiver has no typed mapping"))?;
    let typed_setter_argument = source_typed_index
        .get(source_id, setter_apply.args[0])
        .ok_or_else(|| typed("setter argument has no typed mapping"))?;
    let TreeKind::Apply(typed_setter_node) = &typed_ast
        .try_get(typed_setter)
        .ok_or_else(|| typed("typed setter is stale"))?
        .kind
    else {
        return Err(typed("setter must remain Apply"));
    };
    let TreeKind::Select(typed_setter_select) = &typed_ast
        .try_get(typed_setter_function)
        .ok_or_else(|| typed("typed setter selection is stale"))?
        .kind
    else {
        return Err(typed("typed setter must remain Select"));
    };
    if typed_setter_node.function != typed_setter_function
        || typed_setter_node.args.as_slice() != [typed_setter_argument]
        || typed_setter_select.qualifier != typed_setter_receiver
        || store.names.resolve(typed_setter_select.name.text()) != "set"
        || !matches!(store.types.try_get(typed_ast.try_get(typed_setter_receiver).ok_or_else(|| typed("typed setter receiver is stale"))?.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }) if *symbol == selected)
        || !matches!(typed_ast.try_get(typed_setter_argument).map(|node| &node.kind), Some(TreeKind::Ident(ident)) if ident.name == store.symbols.get(formals[3]).name)
    {
        return Err(typed(
            "setter must use the selected local and the original fourth formal",
        ));
    }
    let setter_method = match store.types.try_get(
        typed_ast
            .try_get(typed_setter_function)
            .ok_or_else(|| typed("typed setter selection is stale"))?
            .ty,
    ) {
        Some(Type::TermRef {
            target: TermRefTarget::Symbol(method),
            ..
        }) => *method,
        _ => {
            return Err(typed(
                "setter selection must resolve to a canonical method TermRef",
            ));
        }
    };
    if !store.symbols.contains(setter_method)
        || store.symbols.get(setter_method).owner != Some(counter)
        || store
            .names
            .resolve(store.symbols.get(setter_method).name.text())
            != "set"
        || !has_source_origin(store, setter_method, source_id)
        || source_index.definition_of(setter_method).is_none_or(
            |definition| !matches!(definition, SourceDefinition::Canonical { source, .. } if source == source_id),
        )
    {
        return Err(typed("setter must be the original Counter.set method"));
    }
    let SymbolInfo::Complete(setter_type) = store.symbols.get(setter_method).info else {
        return Err(sem("Counter.set signature is incomplete"));
    };
    let Some(Type::Method(setter_signature)) = store.types.try_get(setter_type) else {
        return Err(sem("Counter.set must have a canonical method signature"));
    };
    if setter_signature.params.len() != 1
        || setter_signature.params[0].ty != definitions.int
        || setter_signature.result != definitions.unit
    {
        return Err(sem(
            "Counter.set must preserve its original Int-to-Unit signature",
        ));
    }
    if !matches!(store.types.try_get(typed_ast.try_get(typed_setter_argument).ok_or_else(|| typed("typed setter argument is stale"))?.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }) if *symbol == formals[3])
        || !matches!(source_ast.try_get(setter_apply.args[0]).map(|node| &node.kind), Some(TreeKind::Ident(ident)) if ident.name == store.symbols.get(formals[3]).name)
    {
        return Err(typed(
            "setter argument must be the original fourth Int formal",
        ));
    }
    let TreeKind::Apply(pair_apply) = &source_ast
        .try_get(block.expr)
        .ok_or_else(|| src("Pair result is stale"))?
        .kind
    else {
        return Err(src("Block result must construct the original Pair"));
    };
    if pair_apply.args.len() != 2 {
        return Err(src(
            "Pair construction must retain two ordered original locals",
        ));
    }
    let TreeKind::Select(source_pair_select) = &source_ast
        .try_get(pair_apply.function)
        .ok_or_else(|| src("Pair constructor selection is stale"))?
        .kind
    else {
        return Err(src("Pair constructor must retain its source Select"));
    };
    let TreeKind::New(source_pair_new) = &source_ast
        .try_get(source_pair_select.qualifier)
        .ok_or_else(|| src("Pair allocation is stale"))?
        .kind
    else {
        return Err(src("Pair constructor selection must target the source New"));
    };
    if store.names.resolve(source_pair_select.name.text()) != "<init>"
        || !matches!(
            source_ast
                .try_get(source_pair_new.tpt)
                .map(|node| &node.kind),
            Some(TreeKind::Ident(ident)) if store.names.resolve(ident.name.text()) == "Pair"
        )
    {
        return Err(src(
            "Pair construction must retain its constructor name and class type tree",
        ));
    }
    let mut pair_typed_args = [typed_pair_result; 2];
    for index in 0..2 {
        pair_typed_args[index] = source_typed_index
            .get(source_id, pair_apply.args[index])
            .ok_or_else(|| typed("Pair argument has no typed mapping"))?;
        if !matches!(store.types.try_get(typed_ast.try_get(pair_typed_args[index]).ok_or_else(|| typed("typed Pair argument is stale"))?.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }) if *symbol == [semantic_first, semantic_second][index])
        {
            return Err(typed(
                "Pair arguments must preserve first then second original local identity",
            ));
        }
    }
    let typed_pair = source_typed_index
        .get(source_id, block.expr)
        .ok_or_else(|| typed("Pair result has no typed mapping"))?;
    let TreeKind::Apply(typed_pair_apply) = &typed_ast
        .try_get(typed_pair)
        .ok_or_else(|| typed("typed Pair result is stale"))?
        .kind
    else {
        return Err(typed("Pair result must remain the original typed Apply"));
    };
    if typed_pair_apply.args.as_slice() != pair_typed_args {
        return Err(typed(
            "typed Pair must preserve exact ordered argument edges",
        ));
    }
    let typed_pair_function = source_typed_index
        .get(source_id, pair_apply.function)
        .ok_or_else(|| typed("Pair constructor selection has no typed mapping"))?;
    let TreeKind::Select(typed_pair_select) = &typed_ast
        .try_get(typed_pair_function)
        .ok_or_else(|| typed("typed Pair constructor selection is stale"))?
        .kind
    else {
        return Err(typed("Pair constructor must retain its typed Select"));
    };
    let typed_pair_new = source_typed_index
        .get(source_id, source_pair_select.qualifier)
        .ok_or_else(|| typed("Pair allocation has no typed mapping"))?;
    let typed_pair_type_tree = source_typed_index
        .get(source_id, source_pair_new.tpt)
        .ok_or_else(|| typed("Pair type tree has no typed mapping"))?;
    let TreeKind::New(typed_new) = &typed_ast
        .try_get(typed_pair_new)
        .ok_or_else(|| typed("typed Pair allocation is stale"))?
        .kind
    else {
        return Err(typed(
            "typed Pair constructor selection must target the typed New",
        ));
    };
    if typed_pair_select.qualifier != typed_pair_new
        || typed_new.tpt != typed_pair_type_tree
        || !matches!(
            typed_ast
                .try_get(typed_pair_type_tree)
                .map(|node| &node.kind),
            Some(TreeKind::TypeTree(_))
        )
        || typed_pair_apply.function != typed_pair_function
    {
        return Err(typed(
            "typed Pair must preserve the exact Select-to-New-to-type-tree edges",
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
                "Pair selection must resolve to a constructor TermRef",
            ));
        }
    };
    let pair_class = match store.types.try_get(
        typed_ast
            .try_get(typed_pair)
            .ok_or_else(|| typed("typed Pair result is stale"))?
            .ty,
    ) {
        Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        }) => *symbol,
        _ => return Err(typed("Pair result must retain its canonical class TypeRef")),
    };
    if !store.symbols.contains(pair_constructor)
        || store.symbols.get(pair_constructor).owner != Some(pair_class)
        || store.names.resolve(store.symbols.get(pair_constructor).name.text()) != "<init>"
        || !has_source_origin(store, pair_constructor, source_id)
        || source_index.definition_of(pair_constructor).is_none_or(
            |definition| !matches!(definition, SourceDefinition::Canonical { source, .. } if source == source_id),
        )
        || store.names.resolve(typed_pair_select.name.text()) != "<init>"
        || !store.symbols.contains(pair_class)
        || store.symbols.get(pair_class).kind != SymbolKind::Class
        || store.names.resolve(store.symbols.get(pair_class).name.text()) != "Pair"
        || source_index.definition_of(pair_class).is_none_or(
            |definition| !matches!(definition, SourceDefinition::Canonical { source, .. } if source == source_id),
        )
        || !matches!(store.types.try_get(typed_ast.try_get(typed_pair_type_tree).ok_or_else(|| typed("typed Pair type tree is stale"))?.ty), Some(Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }) if *class == pair_class)
        || !is_canonical_class_type_ref(
            store,
            typed_ast
                .try_get(typed_pair_new)
                .ok_or_else(|| typed("typed Pair allocation is stale"))?
                .ty,
            pair_class,
            package,
            definitions.no_prefix,
        )
    {
        return Err(typed("Pair result must use the original Pair constructor and class"));
    }
    let SymbolInfo::Complete(pair_constructor_type) = store.symbols.get(pair_constructor).info
    else {
        return Err(sem("Pair constructor signature is incomplete"));
    };
    let Some(Type::Method(pair_constructor_signature)) = store.types.try_get(pair_constructor_type)
    else {
        return Err(sem(
            "Pair constructor must retain its canonical method signature",
        ));
    };
    if pair_constructor_signature.params.len() != 2
        || pair_constructor_signature.params.iter().any(|parameter| {
            !matches!(store.types.try_get(parameter.ty), Some(Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }) if *class == counter)
        })
        || !matches!(store.types.try_get(signature.result), Some(Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }) if *class == pair_class)
    {
        return Err(sem("Pair constructor and factory result must preserve the exact two-Counter Pair signature"));
    }
    // Every decisive tree identity is from the same source and keeps its exact
    // original span through the source-to-typed edge.
    for (source_tree, typed_tree, description) in [
        (source_block, typed_block, "Block"),
        (
            declarations[2],
            typed_declarations[2],
            "selected declaration",
        ),
        (
            selected_initializer,
            typed_selected_initializer,
            "selected initializer",
        ),
        (source_if, typed_if, "If"),
        (source_condition, typed_condition, "condition"),
        (source_assignments[0], typed_assignments[0], "THEN Assign"),
        (source_assignments[1], typed_assignments[1], "ELSE Assign"),
        (source_lhss[0], typed_lhss[0], "THEN LHS"),
        (source_lhss[1], typed_lhss[1], "ELSE LHS"),
        (source_rhss[0], typed_rhss[0], "THEN RHS"),
        (source_rhss[1], typed_rhss[1], "ELSE RHS"),
        (source_setter, typed_setter, "setter"),
        (block.expr, typed_pair, "Pair result"),
        (
            pair_apply.function,
            typed_pair_function,
            "Pair constructor Select",
        ),
        (
            source_pair_select.qualifier,
            typed_pair_new,
            "Pair constructor New",
        ),
        (
            source_pair_new.tpt,
            typed_pair_type_tree,
            "Pair constructor type tree",
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

    Ok(ValidatedConditionalMutableReferenceAssignmentProof {
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
        source_if,
        typed_if,
        source_condition,
        typed_condition,
        source_assignments,
        typed_assignments,
        source_lhss,
        typed_lhss,
        source_rhss,
        typed_rhss,
        condition_formal,
        selected,
        rhs_locals: expected_rhs,
        local_type,
    })
}
