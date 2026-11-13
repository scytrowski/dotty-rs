//! Borrowed proof for an immutable local selected from two existing locals by
//! one Boolean `if` expression. This receipt authenticates source/typed
//! identity only; it does not authorize JVM or TASTy emission.

use super::*;

/// Inputs from one real frontend session for the frozen three-local Counter
/// selection body. The first two bindings are the original initialized
/// locals; `selected_binding` is the original third ValDef binding.
#[derive(Clone, Copy)]
pub struct ImmutableLocalConditionalReferenceSelectionProofInput<'session> {
    pub initializers: ImmutableLocalInitializerPairProofInput<'session>,
    pub selected_binding: Option<TypedLocalBinding>,
}

/// Opaque borrowed identity receipt for one immutable Counter local whose
/// initializer selects one of two distinct initialized Counter locals.
pub struct ValidatedImmutableLocalConditionalReferenceSelectionProof<'session> {
    _store: &'session SemanticStore,
    _source_ast: &'session AstArena<Untyped>,
    _typed_ast: &'session TypedAst,
    _source_index: &'session SourceSemanticIndex,
    _source_typed_index: &'session SourceTypedIndex,
    _jvm_unit: &'session JvmCompilationUnit<'session>,
    initializer_pair: ValidatedImmutableLocalInitializerPairProof<'session>,
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
    source_if: TreeId<Untyped>,
    typed_if: TreeId<Typed>,
    source_condition: TreeId<Untyped>,
    typed_condition: TreeId<Typed>,
    source_arms: [TreeId<Untyped>; 2],
    typed_arms: [TreeId<Typed>; 2],
    condition_formal: SymbolId,
    targets: [SymbolId; 2],
    selected: SymbolId,
    local_type: TypeId,
    if_result_type: TypeId,
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
}

impl ValidatedImmutableLocalConditionalReferenceSelectionProof<'_> {
    pub const fn initializer_pair(&self) -> &ValidatedImmutableLocalInitializerPairProof<'_> {
        &self.initializer_pair
    }
    pub fn binding(&self) -> dotty_tasty_pickler::ConditionalLocalReferenceSelectionBinding {
        dotty_tasty_pickler::ConditionalLocalReferenceSelectionBinding {
            source_id: self.source_id,
            owner: self.owner,
            method: self.method,
            scope: self.scope,
            source_block: self.source_block,
            typed_block: self.typed_block,
            source_declaration: self.source_declarations[2],
            typed_declaration: self.typed_declarations[2],
            source_if: self.source_if,
            typed_if: self.typed_if,
            source_condition: self.source_condition,
            typed_condition: self.typed_condition,
            source_arms: self.source_arms,
            typed_arms: self.typed_arms,
            condition_formal: self.condition_formal,
            targets: self.targets,
            selected: self.selected,
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
    pub const fn source_block(&self) -> TreeId<Untyped> {
        self.source_block
    }
    pub const fn typed_block(&self) -> TreeId<Typed> {
        self.typed_block
    }
    pub const fn source_declarations(&self) -> [TreeId<Untyped>; 3] {
        self.source_declarations
    }
    pub const fn typed_declarations(&self) -> [TreeId<Typed>; 3] {
        self.typed_declarations
    }
    pub const fn source_initializers(&self) -> [TreeId<Untyped>; 3] {
        self.source_initializers
    }
    pub const fn typed_initializers(&self) -> [TreeId<Typed>; 3] {
        self.typed_initializers
    }
    pub const fn source_if(&self) -> TreeId<Untyped> {
        self.source_if
    }
    pub const fn typed_if(&self) -> TreeId<Typed> {
        self.typed_if
    }
    pub const fn source_condition(&self) -> TreeId<Untyped> {
        self.source_condition
    }
    pub const fn typed_condition(&self) -> TreeId<Typed> {
        self.typed_condition
    }
    pub const fn source_arms(&self) -> [TreeId<Untyped>; 2] {
        self.source_arms
    }
    pub const fn typed_arms(&self) -> [TreeId<Typed>; 2] {
        self.typed_arms
    }
    pub const fn condition_formal(&self) -> SymbolId {
        self.condition_formal
    }
    pub const fn targets(&self) -> [SymbolId; 2] {
        self.targets
    }
    pub const fn selected(&self) -> SymbolId {
        self.selected
    }
    pub const fn local_type(&self) -> TypeId {
        self.local_type
    }
    pub const fn if_result_type(&self) -> TypeId {
        self.if_result_type
    }
    pub const fn source_setter(&self) -> TreeId<Untyped> {
        self.source_setter
    }
    pub const fn typed_setter(&self) -> TreeId<Typed> {
        self.typed_setter
    }
    pub const fn source_setter_receiver(&self) -> TreeId<Untyped> {
        self.source_setter_receiver
    }
    pub const fn typed_setter_receiver(&self) -> TreeId<Typed> {
        self.typed_setter_receiver
    }
    pub const fn source_setter_argument(&self) -> TreeId<Untyped> {
        self.source_setter_argument
    }
    pub const fn typed_setter_argument(&self) -> TreeId<Typed> {
        self.typed_setter_argument
    }
    pub const fn setter_method(&self) -> SymbolId {
        self.setter_method
    }
    pub const fn source_pair_result(&self) -> TreeId<Untyped> {
        self.source_pair_result
    }
    pub const fn typed_pair_result(&self) -> TreeId<Typed> {
        self.typed_pair_result
    }
    pub const fn source_pair_arguments(&self) -> [TreeId<Untyped>; 2] {
        self.source_pair_arguments
    }
    pub const fn typed_pair_arguments(&self) -> [TreeId<Typed>; 2] {
        self.typed_pair_arguments
    }
}

/// Authenticate the exact immutable Counter-selection body from one source
/// session. The candidate-method four-formal producer rejection is deliberately
/// outside this proof and remains unchanged.
pub fn validate_immutable_local_conditional_reference_selection_proof<'session>(
    input: ImmutableLocalConditionalReferenceSelectionProofInput<'session>,
) -> Result<
    ValidatedImmutableLocalConditionalReferenceSelectionProof<'session>,
    CrossClassConstructorCallProofError,
> {
    let src = |s| CrossClassConstructorCallProofError::SourceShape(s);
    let sid = |s| CrossClassConstructorCallProofError::SourceIdentity(s);
    let sem = |s| CrossClassConstructorCallProofError::SemanticIdentity(s);
    let typed = |s| CrossClassConstructorCallProofError::TypedBinding(s);
    let base = input.initializers;
    let ImmutableLocalInitializerPairProofInput {
        source_id,
        method_tree,
        store,
        definitions,
        source_ast,
        typed_ast,
        source_index,
        source_typed_index,
        local_bindings,
        jvm_unit,
    } = base;
    if !std::ptr::eq(jvm_unit.store, store) || !std::ptr::eq(jvm_unit.typed_ast, typed_ast) {
        return Err(sem(
            "JVM unit and proof must borrow the same semantic and typed stores",
        ));
    }
    let method_node = source_ast
        .try_get(method_tree)
        .ok_or_else(|| src("factory method tree is stale"))?;
    let TreeKind::DefDef(method_def) = &method_node.kind else {
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
        || !has_source_origin(store, owner, source_id)
        || source_index.definition_of(owner).is_none_or(
            |definition| !matches!(definition, SourceDefinition::Canonical { source, .. } if source == source_id),
        )
    {
        return Err(sid("factory owner must be the canonical source class"));
    }
    let package = store
        .symbols
        .get(owner)
        .owner
        .ok_or_else(|| sid("factory class has no package owner"))?;
    let signature_ty = match store.symbols.get(method).info {
        SymbolInfo::Complete(ty) => ty,
        _ => return Err(sem("factory method signature is incomplete")),
    };
    let Some(Type::Method(signature)) = store.types.try_get(signature_ty) else {
        return Err(sem("factory method must retain a canonical signature"));
    };
    if !method_def.type_params.is_empty()
        || method_def.value_param_clauses.len() != 1
        || method_def.value_param_clauses[0].len() != 4
        || signature.kind != dotty_core::MethodKind::Plain
        || signature.params.len() != 4
    {
        return Err(src("factory must have one plain four-formal clause"));
    }
    let formal_types = [
        definitions.int,
        definitions.int,
        definitions.boolean,
        definitions.int,
    ];
    let mut formal_symbols = Vec::with_capacity(4);
    let method_scope = source_index
        .scope_of(method)
        .ok_or_else(|| sid("factory method has no canonical formal scope"))?;
    if !store.scopes.contains(method_scope) || store.scopes.get(method_scope).owner != Some(method)
    {
        return Err(sid("factory method formal scope must be method-owned"));
    }
    for (index, (tree, expected_type)) in method_def.value_param_clauses[0]
        .iter()
        .zip(formal_types)
        .enumerate()
    {
        let TreeKind::ValDef(formal_def) = &source_ast
            .try_get(*tree)
            .ok_or_else(|| src("factory formal tree is stale"))?
            .kind
        else {
            return Err(src("factory parameters must be value formals"));
        };
        if formal_def.rhs.is_some()
            || matches!(
                source_ast.try_get(formal_def.tpt).map(|n| &n.kind),
                Some(TreeKind::TypeTree(_))
            )
        {
            return Err(src(
                "factory formals must be explicit values without defaults",
            ));
        }
        let formal = source_index
            .symbol_at(source_id, *tree)
            .ok_or_else(|| sid("factory formal has no canonical source symbol"))?;
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
            || !matches!(store.symbols.get(formal).info, SymbolInfo::Complete(ty) if ty == expected_type)
            || signature.params[index].ty != expected_type
        {
            return Err(sem(
                "factory formals must preserve the ordered Int, Int, Boolean, Int types",
            ));
        }
        require_source_position(
            source_ast,
            source_id,
            *tree,
            "factory formal has a foreign source span",
        )?;
        formal_symbols.push(formal);
    }
    let formals: [SymbolId; 4] = formal_symbols
        .try_into()
        .map_err(|_| src("factory must retain four formal symbols"))?;
    let rhs = method_def
        .rhs
        .ok_or_else(|| src("factory method must have a body"))?;
    let TreeKind::Block(block) = &source_ast
        .try_get(rhs)
        .ok_or_else(|| src("factory body is stale"))?
        .kind
    else {
        return Err(src("factory body must be a direct Block"));
    };
    if block.stats.len() != 4 {
        return Err(src(
            "factory Block must contain exactly three local ValDefs and one setter call",
        ));
    }
    let declarations: [TreeId<Untyped>; 3] = block.stats[..3]
        .try_into()
        .map_err(|_| src("factory must retain three local declarations"))?;
    let TreeKind::Apply(setter_apply) = &source_ast
        .try_get(block.stats[3])
        .ok_or_else(|| src("setter statement is stale"))?
        .kind
    else {
        return Err(src("fourth Block statistic must be the setter Apply"));
    };
    if setter_apply.kind != ApplyKind::Regular || setter_apply.args.len() != 1 {
        return Err(src("setter must be one regular single-argument Apply"));
    }
    let setter_position = require_source_position(
        source_ast,
        source_id,
        block.stats[3],
        "setter statement has a foreign source span",
    )?;
    let typed_block = source_typed_index
        .get(source_id, rhs)
        .ok_or_else(|| typed("source Block has no typed mapping"))?;
    let typed_declarations = [
        source_typed_index
            .get(source_id, declarations[0])
            .ok_or_else(|| typed("first local has no typed mapping"))?,
        source_typed_index
            .get(source_id, declarations[1])
            .ok_or_else(|| typed("second local has no typed mapping"))?,
        source_typed_index
            .get(source_id, declarations[2])
            .ok_or_else(|| typed("selected local has no typed mapping"))?,
    ];
    let typed_setter = source_typed_index
        .get(source_id, block.stats[3])
        .ok_or_else(|| typed("setter call has no typed mapping"))?;
    let TreeKind::Block(typed_block_node) = &typed_ast
        .try_get(typed_block)
        .ok_or_else(|| typed("typed Block is stale"))?
        .kind
    else {
        return Err(typed("source Block must remain typed Block"));
    };
    if typed_block_node.stats
        != [
            typed_declarations[0],
            typed_declarations[1],
            typed_declarations[2],
            typed_setter,
        ]
    {
        return Err(typed(
            "typed Block must retain exact ordered source-stat mappings",
        ));
    }
    let counter = {
        let binding =
            local_bindings[0].ok_or_else(|| sid("first local has no original Typer binding"))?;
        if !store.symbols.contains(binding.symbol()) {
            return Err(sid("first local Typer binding has a stale symbol"));
        }
        let ty = match store.symbols.get(binding.symbol()).info {
            SymbolInfo::Complete(ty) => ty,
            _ => return Err(sem("first local type is incomplete")),
        };
        match store.types.try_get(ty) {
            Some(Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            }) => *symbol,
            _ => return Err(sem("first local must have a canonical Counter class type")),
        }
    };
    let mut seed_formals = [formals[0]; 2];
    for index in 0..2 {
        let TreeKind::ValDef(local_def) = &source_ast
            .try_get(declarations[index])
            .ok_or_else(|| src("Counter local declaration is stale"))?
            .kind
        else {
            return Err(src("first two Block statistics must be Counter ValDefs"));
        };
        let initializer = local_def
            .rhs
            .ok_or_else(|| src("Counter local initializer is absent"))?;
        let TreeKind::Apply(application) = &source_ast
            .try_get(initializer)
            .ok_or_else(|| src("Counter local initializer is stale"))?
            .kind
        else {
            return Err(src(
                "Counter local initializer must be direct constructor application",
            ));
        };
        if application.args.len() != 1 {
            return Err(src("Counter initializer must consume one Int formal"));
        }
        let typed_argument = source_typed_index
            .get(source_id, application.args[0])
            .ok_or_else(|| typed("Counter initializer seed has no typed mapping"))?;
        let seed_formal = match store.types.try_get(
            typed_ast
                .try_get(typed_argument)
                .ok_or_else(|| typed("typed Counter seed is stale"))?
                .ty,
        ) {
            Some(Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            }) => *symbol,
            _ => {
                return Err(typed(
                    "Counter initializer seed must retain its formal TermRef",
                ));
            }
        };
        if !formals[..2].contains(&seed_formal) {
            return Err(typed(
                "Counter initializer seed must use one of the two original Int formals",
            ));
        }
        seed_formals[index] = seed_formal;
    }
    if seed_formals[0] == seed_formals[1] {
        return Err(typed(
            "the two initialized Counter locals must use distinct original seed formals",
        ));
    }
    let initializer_receipts = [
        validate_immutable_local_declaration_core(
            source_id,
            method,
            declarations[0],
            store,
            definitions,
            source_ast,
            typed_ast,
            source_typed_index,
            local_bindings[0],
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
            local_bindings[1],
            counter,
            package,
            seed_formals[1],
        )?,
    ];
    if initializer_receipts[0].symbol == initializer_receipts[1].symbol
        || initializer_receipts[0].scope != initializer_receipts[1].scope
    {
        return Err(sid(
            "source Counter locals must be distinct and share the method Block scope",
        ));
    }
    let selected_binding = input
        .selected_binding
        .ok_or_else(|| sid("selected local has no original Typer binding"))?;
    let selected = selected_binding.symbol();
    if !store.symbols.contains(selected) {
        return Err(sid("selected Typer binding has a stale symbol"));
    }
    let TreeKind::ValDef(selected_def) = &source_ast
        .try_get(declarations[2])
        .ok_or_else(|| src("selected declaration is stale"))?
        .kind
    else {
        return Err(src("third Block statistic must be selected ValDef"));
    };
    let source_if = selected_def
        .rhs
        .ok_or_else(|| src("selected local must have an initializer"))?;
    let TreeKind::If(source_if_node) = &source_ast
        .try_get(source_if)
        .ok_or_else(|| src("selected initializer is stale"))?
        .kind
    else {
        return Err(src("selected initializer must be one direct If"));
    };
    let selected_type = match store.symbols.get(selected).info {
        SymbolInfo::Complete(ty) => ty,
        _ => return Err(sem("selected local type is incomplete")),
    };
    if selected != selected_binding.symbol()
        || selected_binding.declaration() != typed_declarations[2]
        || !store.symbols.contains(selected)
        || store.symbols.get(selected).kind != SymbolKind::Local
        || store.symbols.get(selected).owner != Some(method)
        || store
            .symbols
            .get(selected)
            .flags
            .contains(SymbolFlags::MUTABLE)
        || !has_source_origin(store, selected, source_id)
        || selected_binding.scope() != initializer_receipts[0].scope
        || !store.scopes.contains(selected_binding.scope())
        || store.scopes.get(selected_binding.scope()).owner != Some(method)
        || store
            .scopes
            .get(selected_binding.scope())
            .lookup(&store.symbols.get(selected).name)
            != Some(selected)
        || !is_canonical_class_type_ref(
            store,
            selected_type,
            counter,
            package,
            definitions.no_prefix,
        )
    {
        return Err(sid(
            "selected must be an immutable original Counter local in the same method-owned scope",
        ));
    }
    if initializer_receipts.iter().any(|receipt| {
        !is_canonical_class_type_ref(
            store,
            receipt.local_type,
            counter,
            package,
            definitions.no_prefix,
        )
    }) {
        return Err(sem(
            "selected and target locals must have the exact declared Counter type",
        ));
    }
    if matches!(
        source_ast.try_get(selected_def.tpt).map(|node| &node.kind),
        Some(TreeKind::TypeTree(_))
    ) {
        return Err(src("selected local must retain its explicit Counter type"));
    }
    let source_if_condition = source_if_node.cond;
    let source_arms = [source_if_node.then_branch, source_if_node.else_branch];
    let TreeKind::Ident(source_condition_ident) = &source_ast
        .try_get(source_if_condition)
        .ok_or_else(|| src("IF condition is stale"))?
        .kind
    else {
        return Err(src(
            "IF condition must reference the original Boolean formal",
        ));
    };
    let source_if_ty = source_typed_index
        .get(source_id, source_if)
        .ok_or_else(|| typed("IF root has no typed mapping"))?;
    let typed_condition = source_typed_index
        .get(source_id, source_if_condition)
        .ok_or_else(|| typed("IF condition has no typed mapping"))?;
    let typed_arms = [
        source_typed_index
            .get(source_id, source_arms[0])
            .ok_or_else(|| typed("THEN arm has no typed mapping"))?,
        source_typed_index
            .get(source_id, source_arms[1])
            .ok_or_else(|| typed("ELSE arm has no typed mapping"))?,
    ];
    let TreeKind::If(typed_if_node) = &typed_ast
        .try_get(source_if_ty)
        .ok_or_else(|| typed("typed IF is stale"))?
        .kind
    else {
        return Err(typed("source IF must remain typed IF"));
    };
    if typed_if_node.cond != typed_condition
        || typed_if_node.then_branch != typed_arms[0]
        || typed_if_node.else_branch != typed_arms[1]
    {
        return Err(typed(
            "typed IF must preserve exact condition and ordered branch mappings",
        ));
    }
    let typed_condition_node = typed_ast
        .try_get(typed_condition)
        .ok_or_else(|| typed("typed condition is stale"))?;
    let TreeKind::Ident(typed_condition_ident) = &typed_condition_node.kind else {
        return Err(typed("typed IF condition must remain Ident"));
    };
    if source_condition_ident.name != typed_condition_ident.name
        || source_condition_ident.name != store.symbols.get(formals[2]).name
        || !matches!(store.types.try_get(typed_condition_node.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(s), .. }) if *s == formals[2])
    {
        return Err(typed(
            "typed IF condition must resolve to the original Boolean formal",
        ));
    }
    let initialized_targets = [
        initializer_receipts[0].symbol,
        initializer_receipts[1].symbol,
    ];
    let mut targets = [initialized_targets[0]; 2];
    for (index, (source_arm, typed_arm)) in source_arms.into_iter().zip(typed_arms).enumerate() {
        let TreeKind::Ident(source_arm_ident) = &source_ast
            .try_get(source_arm)
            .ok_or_else(|| src("IF branch tree is stale"))?
            .kind
        else {
            return Err(src(
                "IF branches must be direct references to existing locals",
            ));
        };
        let typed_arm_node = typed_ast
            .try_get(typed_arm)
            .ok_or_else(|| typed("typed branch is stale"))?;
        let TreeKind::Ident(typed_arm_ident) = &typed_arm_node.kind else {
            return Err(typed("typed IF branch must remain Ident"));
        };
        let branch_symbol = match store.types.try_get(typed_arm_node.ty) {
            Some(Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            }) => *symbol,
            _ => return Err(typed("typed IF branch must retain a local TermRef")),
        };
        if !store.symbols.contains(branch_symbol)
            || !initialized_targets.contains(&branch_symbol)
            || source_arm_ident.name != typed_arm_ident.name
            || source_arm_ident.name != store.symbols.get(branch_symbol).name
        {
            return Err(typed(
                "typed IF branch must resolve to one of the exact initialized locals",
            ));
        }
        targets[index] = branch_symbol;
    }
    if targets[0] == targets[1] {
        return Err(typed(
            "IF branches must resolve to distinct existing locals",
        ));
    }
    if !is_canonical_class_type_ref(
        store,
        typed_ast
            .try_get(source_if_ty)
            .ok_or_else(|| typed("typed IF is stale"))?
            .ty,
        counter,
        package,
        definitions.no_prefix,
    ) {
        return Err(typed(
            "IF result must retain the selected local's exact Counter type",
        ));
    }
    let counter_tree = match source_index.definition_of(counter) {
        Some(SourceDefinition::Canonical { source, tree }) if source == source_id => tree,
        _ => return Err(sid("Counter class has no canonical source definition")),
    };
    let TreeKind::TypeDef(counter_def) = &source_ast
        .try_get(counter_tree)
        .ok_or_else(|| sid("Counter source tree is stale"))?
        .kind
    else {
        return Err(sid("Counter source definition must remain TypeDef"));
    };
    let TreeKind::Template(counter_template) = &source_ast
        .try_get(counter_def.rhs)
        .ok_or_else(|| sid("Counter template is stale"))?
        .kind
    else {
        return Err(sid("Counter source definition must retain its Template"));
    };
    let counter_constructor = source_index
        .symbol_at(source_id, counter_template.constructor)
        .ok_or_else(|| sid("Counter constructor has no canonical source symbol"))?;
    if counter_constructor != initializer_receipts[0].constructor
        || counter_constructor != initializer_receipts[1].constructor
    {
        return Err(sid(
            "both locals must use the same original Counter constructor",
        ));
    }
    let TreeKind::DefDef(counter_constructor_def) = &source_ast
        .try_get(counter_template.constructor)
        .ok_or_else(|| sid("Counter constructor source tree is stale"))?
        .kind
    else {
        return Err(sid("Counter primary constructor must remain DefDef"));
    };
    let counter_constructor_formal_tree = *counter_constructor_def
        .value_param_clauses
        .first()
        .and_then(|clause| clause.first())
        .ok_or_else(|| src("Counter constructor must retain its original formal"))?;
    let counter_constructor_formal = source_index
        .derived_symbol_at(
            counter_constructor,
            source_id,
            counter_constructor_formal_tree,
        )
        .ok_or_else(|| sid("Counter constructor formal is not canonically derived"))?;
    let initializer_result_types = [
        typed_ast
            .try_get(initializer_receipts[0].typed_initializer)
            .ok_or_else(|| typed("first typed initializer became stale"))?
            .ty,
        typed_ast
            .try_get(initializer_receipts[1].typed_initializer)
            .ok_or_else(|| typed("second typed initializer became stale"))?
            .ty,
    ];
    let initializer_pair_receipts = [
        ImmutableLocalInitializerReceipt {
            declaration: initializer_receipts[0],
            seed_formal: seed_formals[0],
            seed_formal_type: definitions.int,
            initializer_result_type: initializer_result_types[0],
            constructor_formal: counter_constructor_formal,
            constructor_formal_type: definitions.int,
        },
        ImmutableLocalInitializerReceipt {
            declaration: initializer_receipts[1],
            seed_formal: seed_formals[1],
            seed_formal_type: definitions.int,
            initializer_result_type: initializer_result_types[1],
            constructor_formal: counter_constructor_formal,
            constructor_formal_type: definitions.int,
        },
    ];
    let typed_initializers = [
        initializer_receipts[0].typed_initializer,
        initializer_receipts[1].typed_initializer,
        source_if_ty,
    ];
    let source_initializers = [
        initializer_receipts[0].initializer,
        initializer_receipts[1].initializer,
        source_if,
    ];
    let selected_typed_declaration = typed_ast
        .try_get(typed_declarations[2])
        .ok_or_else(|| typed("selected ValDef is stale"))?;
    let selected_typed_def = match &selected_typed_declaration.kind {
        TreeKind::ValDef(def) => def,
        _ => return Err(typed("selected declaration must remain typed ValDef")),
    };
    if !matches!(
        typed_ast
            .try_get(selected_typed_def.tpt)
            .map(|tree| &tree.kind),
        Some(TreeKind::TypeTree(_))
    ) {
        return Err(typed(
            "selected typed annotation must retain its original TypeTree kind",
        ));
    }
    if selected_typed_declaration.ty != selected_type
        || selected_typed_def.rhs != Some(source_if_ty)
        || selected_typed_def.tpt
            != source_typed_index
                .get(source_id, selected_def.tpt)
                .ok_or_else(|| typed("selected declared type has no mapping"))?
        || !is_canonical_class_type_ref(
            store,
            typed_ast
                .try_get(selected_typed_def.tpt)
                .ok_or_else(|| typed("selected typed type tree is stale"))?
                .ty,
            counter,
            package,
            definitions.no_prefix,
        )
    {
        return Err(typed(
            "selected typed ValDef must preserve exact IF and declared-type roots",
        ));
    }
    let (source_setter_receiver, source_setter_argument) = {
        let TreeKind::Select(select) = &source_ast
            .try_get(setter_apply.function)
            .ok_or_else(|| src("setter function is stale"))?
            .kind
        else {
            return Err(src("setter function must be a Select"));
        };
        if store.names.resolve(select.name.text()) != "set" || setter_apply.args.len() != 1 {
            return Err(src(
                "setter must be the original one-argument Counter.set call",
            ));
        }
        (select.qualifier, setter_apply.args[0])
    };
    let TreeKind::Ident(source_setter_receiver_ident) = &source_ast
        .try_get(source_setter_receiver)
        .ok_or_else(|| src("setter receiver is stale"))?
        .kind
    else {
        return Err(src("setter receiver must reference selected"));
    };
    let TreeKind::Ident(source_setter_argument_ident) = &source_ast
        .try_get(source_setter_argument)
        .ok_or_else(|| src("setter argument is stale"))?
        .kind
    else {
        return Err(src("setter argument must be the original Int formal"));
    };
    let typed_setter_receiver = source_typed_index
        .get(source_id, source_setter_receiver)
        .ok_or_else(|| typed("setter receiver has no typed mapping"))?;
    let typed_setter_argument = source_typed_index
        .get(source_id, source_setter_argument)
        .ok_or_else(|| typed("setter argument has no typed mapping"))?;
    let typed_setter_receiver_node = typed_ast
        .try_get(typed_setter_receiver)
        .ok_or_else(|| typed("typed receiver is stale"))?;
    let typed_setter_argument_node = typed_ast
        .try_get(typed_setter_argument)
        .ok_or_else(|| typed("typed argument is stale"))?;
    let TreeKind::Ident(typed_setter_receiver_ident) = &typed_setter_receiver_node.kind else {
        return Err(typed("typed setter receiver must remain Ident"));
    };
    let TreeKind::Ident(typed_setter_argument_ident) = &typed_setter_argument_node.kind else {
        return Err(typed("typed setter argument must remain Ident"));
    };
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
    if typed_setter_node.function
        != source_typed_index
            .get(source_id, setter_apply.function)
            .ok_or_else(|| typed("setter Select has no mapping"))?
        || typed_setter_node.args != [typed_setter_argument]
        || typed_setter_select.qualifier != typed_setter_receiver
        || source_setter_receiver_ident.name != typed_setter_receiver_ident.name
        || source_setter_receiver_ident.name != store.symbols.get(selected).name
        || source_setter_argument_ident.name != typed_setter_argument_ident.name
        || source_setter_argument_ident.name != store.symbols.get(formals[3]).name
        || !matches!(store.types.try_get(typed_setter_receiver_node.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(s), .. }) if *s == selected)
        || !matches!(store.types.try_get(typed_setter_argument_node.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(s), .. }) if *s == formals[3])
        || typed_ast
            .try_get(typed_setter)
            .ok_or_else(|| typed("typed setter is stale"))?
            .ty
            != definitions.unit
    {
        return Err(typed(
            "typed setter must preserve selected receiver and nextValue formal identities",
        ));
    }
    let setter_method = match store.types.try_get(
        typed_ast
            .try_get(typed_setter_node.function)
            .ok_or_else(|| typed("typed setter Select is stale"))?
            .ty,
    ) {
        Some(Type::TermRef {
            target: TermRefTarget::Symbol(s),
            ..
        }) => *s,
        _ => return Err(typed("typed setter must resolve to a canonical method")),
    };
    if !store.symbols.contains(setter_method)
        || store.symbols.get(setter_method).owner != Some(counter)
        || store.symbols.get(setter_method).kind != SymbolKind::Method
        || store.symbols.get(setter_method).visibility != dotty_core::Visibility::Public
        || !has_source_origin(store, setter_method, source_id)
        || store
            .names
            .resolve(store.symbols.get(setter_method).name.text())
            != "set"
    {
        return Err(sem("setter must be the canonical Counter.set method"));
    }
    let source_pair_result = block.expr;
    let typed_pair_result = source_typed_index
        .get(source_id, source_pair_result)
        .ok_or_else(|| typed("final Pair result has no typed mapping"))?;
    let pair_position = require_source_position(
        source_ast,
        source_id,
        source_pair_result,
        "final Pair result has a foreign source span",
    )?;
    if setter_position.span().range().end() > pair_position.span().range().start() {
        return Err(src(
            "setter statement must precede the final Pair construction",
        ));
    }
    if typed_block_node.expr != typed_pair_result {
        return Err(typed(
            "typed Block must retain the exact final Pair expression mapping",
        ));
    }
    let TreeKind::Apply(source_pair_apply) = &source_ast
        .try_get(source_pair_result)
        .ok_or_else(|| src("final Pair result is stale"))?
        .kind
    else {
        return Err(src(
            "final expression must be the original Pair constructor Apply",
        ));
    };
    if source_pair_apply.args.len() != 2 {
        return Err(src(
            "final Pair construction must retain exactly two original arguments",
        ));
    }
    let source_pair_arguments: [TreeId<Untyped>; 2] = source_pair_apply
        .args
        .clone()
        .try_into()
        .map_err(|_| src("Pair constructor argument count changed"))?;
    let typed_pair_arguments = [
        source_typed_index
            .get(source_id, source_pair_arguments[0])
            .ok_or_else(|| typed("first Pair argument has no typed mapping"))?,
        source_typed_index
            .get(source_id, source_pair_arguments[1])
            .ok_or_else(|| typed("second Pair argument has no typed mapping"))?,
    ];
    let TreeKind::Apply(typed_pair_apply) = &typed_ast
        .try_get(typed_pair_result)
        .ok_or_else(|| typed("typed Pair result is stale"))?
        .kind
    else {
        return Err(typed("final Pair result must remain typed Apply"));
    };
    if typed_pair_apply.kind != ApplyKind::Regular
        || typed_pair_apply.function
            != source_typed_index
                .get(source_id, source_pair_apply.function)
                .ok_or_else(|| typed("Pair constructor Select has no typed mapping"))?
        || typed_pair_apply.args != typed_pair_arguments
    {
        return Err(typed(
            "Pair constructor Apply must retain its exact Select and source-to-typed argument mappings",
        ));
    }
    let TreeKind::Select(typed_pair_select) = &typed_ast
        .try_get(typed_pair_apply.function)
        .ok_or_else(|| typed("typed Pair Select is stale"))?
        .kind
    else {
        return Err(typed(
            "typed Pair constructor Apply function must remain Select",
        ));
    };
    let source_pair_select = match &source_ast
        .try_get(source_pair_apply.function)
        .ok_or_else(|| src("Pair constructor Select is stale"))?
        .kind
    {
        TreeKind::Select(select) => select,
        _ => return Err(src("Pair constructor Apply function must be Select")),
    };
    let TreeKind::New(source_pair_new) = &source_ast
        .try_get(source_pair_select.qualifier)
        .ok_or_else(|| src("Pair constructor New is stale"))?
        .kind
    else {
        return Err(src("Pair constructor Select qualifier must be New"));
    };
    let typed_pair_new = source_typed_index
        .get(source_id, source_pair_select.qualifier)
        .ok_or_else(|| typed("Pair constructor New has no typed mapping"))?;
    let TreeKind::New(typed_pair_new_node) = &typed_ast
        .try_get(typed_pair_new)
        .ok_or_else(|| typed("typed Pair New is stale"))?
        .kind
    else {
        return Err(typed(
            "typed Pair constructor Select qualifier must remain New",
        ));
    };
    if typed_pair_select.qualifier != typed_pair_new
        || typed_pair_new_node.tpt
            != source_typed_index
                .get(source_id, source_pair_new.tpt)
                .ok_or_else(|| typed("Pair constructor type tree has no typed mapping"))?
    {
        return Err(typed(
            "Pair constructor New must retain its exact type mapping",
        ));
    }
    if !matches!(
        typed_ast
            .try_get(typed_pair_new_node.tpt)
            .map(|tree| &tree.kind),
        Some(TreeKind::TypeTree(_))
    ) {
        return Err(typed(
            "typed Pair New type must retain its original TypeTree kind",
        ));
    }
    match &source_ast
        .try_get(source_pair_apply.function)
        .ok_or_else(|| src("Pair constructor Select is stale"))?
        .kind
    {
        TreeKind::Select(select) if store.names.resolve(select.name.text()) == "<init>" => {}
        TreeKind::Select(_) => {
            return Err(src(
                "final expression must select the Pair primary constructor",
            ));
        }
        _ => return Err(src("Pair constructor Apply function must be Select")),
    }
    let pair_constructor = match store.types.try_get(
        typed_ast
            .try_get(typed_pair_apply.function)
            .ok_or_else(|| typed("typed Pair Select is stale"))?
            .ty,
    ) {
        Some(Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        }) => *symbol,
        _ => {
            return Err(typed(
                "Pair Select must retain its canonical constructor TermRef",
            ));
        }
    };
    if !store.symbols.contains(pair_constructor) {
        return Err(sem("Pair constructor symbol is stale"));
    }
    let pair_class = store
        .symbols
        .get(pair_constructor)
        .owner
        .ok_or_else(|| sem("Pair constructor has no class owner"))?;
    if !store.symbols.contains(pair_constructor) || store.symbols.get(pair_constructor).kind != SymbolKind::Constructor || !store.symbols.contains(pair_class) || store.symbols.get(pair_class).kind != SymbolKind::Class || store.types.try_get(typed_ast.try_get(typed_pair_result).ok_or_else(|| typed("typed Pair result is stale"))?.ty).is_none_or(|ty| !matches!(ty, Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == pair_class)) { return Err(typed("final result must remain an application of the canonical Pair constructor")); }
    if typed_ast
        .try_get(typed_block)
        .ok_or_else(|| typed("typed Block is stale"))?
        .ty
        != typed_ast
            .try_get(typed_pair_result)
            .ok_or_else(|| typed("typed Pair result is stale"))?
            .ty
    {
        return Err(typed(
            "typed Block must retain the canonical final Pair expression TypeId",
        ));
    }
    if store.symbols.get(pair_constructor).visibility != dotty_core::Visibility::Public
        || !has_source_origin(store, pair_constructor, source_id)
        || store.symbols.get(pair_class).visibility != dotty_core::Visibility::Public
        || !has_source_origin(store, pair_class, source_id)
    {
        return Err(sem(
            "Pair constructor and class must be public canonical source symbols",
        ));
    }
    let mut pair_targets = [initialized_targets[0]; 2];
    for (index, argument) in source_pair_arguments.iter().enumerate() {
        let TreeKind::Ident(source_pair_ident) = &source_ast
            .try_get(*argument)
            .ok_or_else(|| src("Pair argument is stale"))?
            .kind
        else {
            return Err(src("Pair arguments must be the original local references"));
        };
        let typed_pair_argument = typed_ast
            .try_get(typed_pair_arguments[index])
            .ok_or_else(|| typed("typed Pair argument is stale"))?;
        let TreeKind::Ident(typed_pair_ident) = &typed_pair_argument.kind else {
            return Err(typed("typed Pair argument must remain Ident"));
        };
        let pair_symbol = match store.types.try_get(typed_pair_argument.ty) {
            Some(Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            }) => *symbol,
            _ => return Err(typed("typed Pair argument must retain a local TermRef")),
        };
        if !store.symbols.contains(pair_symbol)
            || !initialized_targets.contains(&pair_symbol)
            || source_pair_ident.name != typed_pair_ident.name
            || source_pair_ident.name != store.symbols.get(pair_symbol).name
        {
            return Err(typed(
                "typed Pair arguments must reference the two initialized locals",
            ));
        }
        pair_targets[index] = pair_symbol;
    }
    if pair_targets[0] == pair_targets[1] {
        return Err(typed(
            "Pair arguments must retain both distinct initialized local identities",
        ));
    }
    let pair_package = store
        .symbols
        .get(pair_class)
        .owner
        .ok_or_else(|| sem("Pair class has no package owner"))?;
    let typed_pair_new_ty = typed_ast
        .try_get(typed_pair_new)
        .ok_or_else(|| typed("typed Pair New is stale"))?
        .ty;
    let typed_pair_new_tpt_ty = typed_ast
        .try_get(typed_pair_new_node.tpt)
        .ok_or_else(|| typed("typed Pair New type tree is stale"))?
        .ty;
    if !is_canonical_class_type_ref(
        store,
        typed_pair_new_ty,
        pair_class,
        pair_package,
        definitions.no_prefix,
    ) {
        return Err(typed("Pair New must retain the canonical Pair type"));
    }
    if !is_canonical_class_type_ref(
        store,
        typed_pair_new_tpt_ty,
        pair_class,
        pair_package,
        definitions.no_prefix,
    ) {
        return Err(typed(
            "Pair New type tree must retain the canonical Pair type",
        ));
    }
    Ok(ValidatedImmutableLocalConditionalReferenceSelectionProof {
        _store: store,
        _source_ast: source_ast,
        _typed_ast: typed_ast,
        _source_index: source_index,
        _source_typed_index: source_typed_index,
        _jvm_unit: jvm_unit,
        initializer_pair: ValidatedImmutableLocalInitializerPairProof {
            _store: store,
            _definitions: definitions,
            _source_ast: source_ast,
            _typed_ast: typed_ast,
            _source_index: source_index,
            _source_typed_index: source_typed_index,
            _jvm_unit: jvm_unit,
            source_id,
            method,
            counter,
            source_block: rhs,
            typed_block,
            source_final_expression: source_pair_result,
            typed_final_expression: typed_pair_result,
            initializers: initializer_pair_receipts,
        },
        source_id,
        owner,
        method,
        scope: selected_binding.scope(),
        source_block: rhs,
        typed_block,
        source_declarations: declarations,
        typed_declarations,
        source_initializers,
        typed_initializers,
        source_if,
        typed_if: source_if_ty,
        source_condition: source_if_condition,
        typed_condition,
        source_arms,
        typed_arms,
        condition_formal: formals[2],
        targets,
        selected,
        local_type: selected_type,
        if_result_type: typed_ast
            .try_get(source_if_ty)
            .ok_or_else(|| typed("typed IF is stale"))?
            .ty,
        source_setter: block.stats[3],
        typed_setter,
        source_setter_receiver,
        typed_setter_receiver,
        source_setter_argument,
        typed_setter_argument,
        setter_method,
        source_pair_result,
        typed_pair_result,
        source_pair_arguments,
        typed_pair_arguments,
    })
}
