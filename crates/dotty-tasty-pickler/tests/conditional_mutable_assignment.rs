use std::collections::HashMap;
use std::{env, fs};

use dotty_core::ast::*;
use dotty_core::*;
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_tasty::ast::{APPLY_TAG, ASSIGN_TAG, BLOCK_TAG, DEFDEF_TAG, IF_TAG, VALDEF_TAG};
use dotty_tasty::file::TastyFile;
use dotty_tasty::term::RawTree;
use dotty_tasty_pickler::*;
use dotty_typer::SourceTyper;

const SOURCE: &str = include_str!(
    "../../../tests/fixtures/jvm_tasty_conditional_mutable_reference_assignment/ConditionalMutableReferenceAssignment.scala"
);

#[derive(Clone, Default)]
struct Bindings {
    typed: HashMap<(SourceId, TreeId<Untyped>), TreeId<Typed>>,
    locals: HashMap<(SourceId, SymbolId), Vec<LocalValueBinding>>,
    classes: Vec<SymbolId>,
    methods: Vec<SymbolId>,
    conditional: Option<ConditionalMutableReferenceAssignmentBinding>,
    initializers: Vec<LocalCounterInitializerBinding>,
    setter: Option<LocalReceiverUnitCallBinding>,
    setter_pair: Option<[LocalReceiverUnitCallBinding; 2]>,
    pair: Option<FormalLocalConstructorCallPairBinding>,
}

fn clone_ast<P: AstPhase + Clone>(source: &AstArena<P>) -> AstArena<P> {
    let mut cloned = AstArena::new();
    for (_, tree) in source.iter() {
        cloned.alloc(tree.clone());
    }
    cloned
}

impl SourceTypedBindings for Bindings {
    fn typed_at(&self, source: SourceId, tree: TreeId<Untyped>) -> Option<TreeId<Typed>> {
        self.typed.get(&(source, tree)).copied()
    }

    fn planned_class_symbols(&self) -> &[SymbolId] {
        &self.classes
    }

    fn planned_method_symbols(&self) -> &[SymbolId] {
        &self.methods
    }

    fn local_value_bindings(&self, source: SourceId, method: SymbolId) -> Vec<LocalValueBinding> {
        self.locals
            .get(&(source, method))
            .cloned()
            .unwrap_or_default()
    }

    fn conditional_mutable_reference_assignment(
        &self,
        _source: SourceId,
        _method: SymbolId,
    ) -> Option<ConditionalMutableReferenceAssignmentBinding> {
        self.conditional
    }

    fn local_counter_initializers(
        &self,
        _source: SourceId,
        _method: SymbolId,
    ) -> Vec<LocalCounterInitializerBinding> {
        self.initializers.clone()
    }

    fn local_receiver_unit_call(
        &self,
        _source: SourceId,
        _method: SymbolId,
    ) -> Option<LocalReceiverUnitCallBinding> {
        self.setter
    }

    fn local_receiver_unit_call_pair(
        &self,
        _source: SourceId,
        _method: SymbolId,
    ) -> Option<[LocalReceiverUnitCallBinding; 2]> {
        self.setter_pair
    }

    fn formal_local_constructor_call_pair(
        &self,
        _source: SourceId,
        _method: SymbolId,
    ) -> Option<FormalLocalConstructorCallPairBinding> {
        self.pair
    }
}

fn typed_at(typer: &SourceTyper<'_>, source: SourceId, tree: TreeId<Untyped>) -> TreeId<Typed> {
    typer
        .source_typed_index()
        .get(source, tree)
        .expect("frontend records each source tree's exact typed edge")
}

fn symbol_ref(store: &SemanticStore, ast: &AstArena<Typed>, tree: TreeId<Typed>) -> SymbolId {
    match store.types.get(ast.get(tree).ty) {
        Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        } => *symbol,
        other => panic!("reference has a canonical symbol TermRef: {other:?}"),
    }
}

fn raw_tag(tree: &RawTree<'_>) -> u8 {
    match tree {
        RawTree::Leaf(term) => term.tag,
        RawTree::Ast { tag, .. } | RawTree::NatAst { tag, .. } => *tag,
        RawTree::LengthNode(node) => node.tag,
    }
}

#[test]
fn exact_conditional_receipt_emits_the_bounded_result_first_tasty_body() {
    check_conditional_source(SOURCE, true);
}

#[test]
fn reversed_conditional_pair_arguments_are_rejected_from_fresh_source() {
    let reversed = SOURCE.replace("new Pair(first, second)", "new Pair(second, first)");
    assert_ne!(
        reversed, SOURCE,
        "the regression source actually reverses Pair"
    );
    check_conditional_source(&reversed, false);
}

fn check_conditional_source(source: &str, should_emit: bool) {
    let source_id = SourceId::from_index(235_001);
    let scanner = ContextualScanner::new(source).unwrap();
    assert!(scanner.diagnostics().is_empty());
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = parse_compilation_unit(
        SourceText::new(source).unwrap(),
        source_id,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let source_index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        "ConditionalMutableReferenceAssignment.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        outcome => panic!("source names without diagnostics: {outcome:?}"),
    };

    let mut method_tree = None;
    let mut factory_tree = None;
    let mut classes = Vec::new();
    let mut methods = Vec::new();
    if let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind {
        for &class_tree in &package.stats {
            let class = source_index.symbol_at(source_id, class_tree).unwrap();
            classes.push(class);
            if let TreeKind::TypeDef(class_def) = &parsed.ast.get(class_tree).kind {
                if store.names.resolve(class_def.name.as_name().text()) == "PairFactory" {
                    factory_tree = Some(class_tree);
                }
                if let TreeKind::Template(template) = &parsed.ast.get(class_def.rhs).kind {
                    methods.push(
                        source_index
                            .symbol_at(source_id, template.constructor)
                            .unwrap(),
                    );
                    for &member in &template.body {
                        if matches!(parsed.ast.get(member).kind, TreeKind::DefDef(_)) {
                            let symbol = source_index.symbol_at(source_id, member).unwrap();
                            methods.push(symbol);
                            if let TreeKind::DefDef(definition) = &parsed.ast.get(member).kind
                                && store.names.resolve(definition.name.as_name().text()) == "create"
                            {
                                method_tree = Some((member, definition.clone()));
                            }
                        }
                    }
                }
            }
        }
    }
    let factory_tree = factory_tree.unwrap();
    let factory = source_index.symbol_at(source_id, factory_tree).unwrap();
    let (method_tree, method_definition) = method_tree.expect("PairFactory.create");
    let method = source_index.symbol_at(source_id, method_tree).unwrap();

    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &source_index,
        &mut store,
        definitions,
        &packages,
    );
    for &symbol in &classes {
        typer.complete_symbol(symbol).unwrap();
    }
    for &symbol in &methods {
        typer.complete_symbol(symbol).unwrap();
    }
    let formals = method_definition.value_param_clauses[0]
        .iter()
        .map(|tree| source_index.symbol_at(source_id, *tree).unwrap())
        .collect::<Vec<_>>();
    for &formal in &formals {
        typer.complete_symbol(formal).unwrap();
    }
    let method_body = method_definition.rhs.unwrap();
    let context = typer.expression_context_for(method).unwrap();
    typer.type_expression(method_body, context).unwrap();

    let TreeKind::Block(block) = &parsed.ast.get(method_body).kind else {
        panic!("create body is the bounded block")
    };
    assert_eq!(block.stats.len(), 5);
    let local_bindings = block.stats[..3]
        .iter()
        .map(|tree| typer.typed_local_binding_at(source_id, *tree).unwrap())
        .collect::<Vec<_>>();
    let [first, second, selected] = local_bindings.as_slice() else {
        unreachable!()
    };
    let conditional = match &parsed.ast.get(block.stats[3]).kind {
        TreeKind::If(value) => value,
        _ => panic!("fourth statement is the original IF"),
    };
    let typed_if = typed_at(&typer, source_id, block.stats[3]);
    let typed_if_node = match &typer.typed_ast().get(typed_if).kind {
        TreeKind::If(value) => value,
        _ => panic!("source IF has an exact typed IF edge"),
    };
    let source_assignments = [conditional.then_branch, conditional.else_branch];
    let typed_assignments = [typed_if_node.then_branch, typed_if_node.else_branch];
    let source_lhss = source_assignments.map(|tree| match &parsed.ast.get(tree).kind {
        TreeKind::Assign(value) => value.lhs,
        _ => panic!("both branches are source assignments"),
    });
    let source_rhss = source_assignments.map(|tree| match &parsed.ast.get(tree).kind {
        TreeKind::Assign(value) => value.rhs,
        _ => panic!("both branches are source assignments"),
    });
    let typed_lhss = typed_assignments.map(|tree| match &typer.typed_ast().get(tree).kind {
        TreeKind::Assign(value) => value.lhs,
        _ => panic!("both branches remain typed assignments"),
    });
    let typed_rhss = typed_assignments.map(|tree| match &typer.typed_ast().get(tree).kind {
        TreeKind::Assign(value) => value.rhs,
        _ => panic!("both branches remain typed assignments"),
    });
    let selected_type = match typer.store().symbols.get(selected.symbol()).info {
        SymbolInfo::Complete(ty) => ty,
        _ => panic!("selected local type is complete"),
    };
    let receipt = ConditionalMutableReferenceAssignmentBinding {
        source_id,
        owner: factory,
        method,
        scope: selected.scope(),
        source_block: method_body,
        typed_block: typed_at(&typer, source_id, method_body),
        source_if: block.stats[3],
        typed_if,
        source_condition: conditional.cond,
        typed_condition: typed_if_node.cond,
        source_assignments,
        typed_assignments,
        source_lhss,
        typed_lhss,
        source_rhss,
        typed_rhss,
        condition_formal: formals[2],
        local: selected.symbol(),
        rhs_locals: [second.symbol(), first.symbol()],
        local_type: selected_type,
    };

    let mut bindings = Bindings {
        classes,
        methods: methods.clone(),
        conditional: Some(receipt),
        ..Bindings::default()
    };
    for (tree, _) in parsed.ast.iter() {
        if let Some(typed) = typer.source_typed_index().get(source_id, tree) {
            bindings.typed.insert((source_id, tree), typed);
        }
    }
    bindings.locals.insert(
        (source_id, method),
        block.stats[..3]
            .iter()
            .zip(&local_bindings)
            .map(|(&source_tree, binding)| LocalValueBinding {
                source_tree,
                symbol: binding.symbol(),
                typed_declaration: binding.declaration(),
                scope: binding.scope(),
            })
            .collect(),
    );

    // The remaining receipts reuse source and typed identities from this same
    // frontend session; no node or symbol is recovered by spelling.
    for (local_index, local_binding) in [first, second].into_iter().enumerate() {
        let source_declaration = block.stats[local_index];
        let TreeKind::ValDef(local_def) = &parsed.ast.get(source_declaration).kind else {
            unreachable!()
        };
        let source_initializer = local_def.rhs.unwrap();
        let TreeKind::Apply(source_apply) = &parsed.ast.get(source_initializer).kind else {
            panic!("Counter local is initialized by its constructor call")
        };
        let source_select = source_apply.function;
        let TreeKind::Select(source_selection) = &parsed.ast.get(source_select).kind else {
            unreachable!()
        };
        let source_new = source_selection.qualifier;
        let source_argument = source_apply.args[0];
        let typed_initializer = typed_at(&typer, source_id, source_initializer);
        let TreeKind::Apply(typed_apply) = &typer.typed_ast().get(typed_initializer).kind else {
            unreachable!()
        };
        let typed_select = typed_apply.function;
        let typed_new = typed_at(&typer, source_id, source_new);
        let typed_argument = typed_apply.args[0];
        let local_type = match typer.store().symbols.get(local_binding.symbol()).info {
            SymbolInfo::Complete(ty) => ty,
            _ => unreachable!(),
        };
        let class = match typer.store().types.get(local_type) {
            Type::TypeRef {
                target: TypeRefTarget::Symbol(class),
                ..
            } => *class,
            _ => unreachable!(),
        };
        let ctor_tree = match source_index.definition_of(class).unwrap() {
            SourceDefinition::Canonical { tree, .. } => tree,
            _ => unreachable!(),
        };
        let TreeKind::TypeDef(counter_def) = &parsed.ast.get(ctor_tree).kind else {
            unreachable!()
        };
        let TreeKind::Template(counter_template) = &parsed.ast.get(counter_def.rhs).kind else {
            unreachable!()
        };
        let constructor = source_index
            .symbol_at(source_id, counter_template.constructor)
            .unwrap();
        let TreeKind::DefDef(constructor_def) = &parsed.ast.get(counter_template.constructor).kind
        else {
            unreachable!()
        };
        let formal_tree = constructor_def.value_param_clauses[0][0];
        let formal = source_index
            .derived_symbol_at(constructor, source_id, formal_tree)
            .unwrap();
        let formal_type = match typer.store().symbols.get(formal).info {
            SymbolInfo::Complete(ty) => ty,
            _ => unreachable!(),
        };
        let argument_formal = symbol_ref(typer.store(), typer.typed_ast(), typed_argument);
        bindings.initializers.push(LocalCounterInitializerBinding {
            source_block: method_body,
            typed_block: typed_at(&typer, source_id, method_body),
            source_declaration,
            typed_declaration: local_binding.declaration(),
            source_initializer,
            typed_initializer,
            source_type_tree: local_def.tpt,
            typed_type_tree: typed_at(&typer, source_id, local_def.tpt),
            local: local_binding.symbol(),
            scope: local_binding.scope(),
            local_type,
            argument_formal,
            call: NestedConstructorCallBinding {
                source_apply: source_initializer,
                typed_apply: typed_initializer,
                source_select,
                source_new,
                source_argument,
                typed_select,
                typed_new,
                typed_argument,
                class,
                constructor,
                formal,
                formal_type,
                result_type: typer.typed_ast().get(typed_initializer).ty,
            },
        });
    }

    let setter_source = block.stats[4];
    let TreeKind::Apply(setter_apply) = &parsed.ast.get(setter_source).kind else {
        unreachable!()
    };
    let setter_source_select = setter_apply.function;
    let TreeKind::Select(setter_select) = &parsed.ast.get(setter_source_select).kind else {
        unreachable!()
    };
    let setter_typed = typed_at(&typer, source_id, setter_source);
    let TreeKind::Apply(setter_typed_apply) = &typer.typed_ast().get(setter_typed).kind else {
        unreachable!()
    };
    let setter_typed_select = setter_typed_apply.function;
    let TreeKind::Select(setter_typed_selection) = &typer.typed_ast().get(setter_typed_select).kind
    else {
        unreachable!()
    };
    let set_method = methods
        .iter()
        .copied()
        .find(|symbol| {
            typer
                .store()
                .names
                .resolve(typer.store().symbols.get(*symbol).name.text())
                == "set"
        })
        .unwrap();
    bindings.setter = Some(LocalReceiverUnitCallBinding {
        source_apply: setter_source,
        typed_apply: setter_typed,
        source_select: setter_source_select,
        typed_select: setter_typed_select,
        source_receiver: setter_select.qualifier,
        typed_receiver: setter_typed_selection.qualifier,
        source_argument: setter_apply.args[0],
        typed_argument: setter_typed_apply.args[0],
        receiver: selected.symbol(),
        receiver_scope: selected.scope(),
        method: set_method,
        argument_formal: formals[3],
    });

    let source_formals: [TreeId<Untyped>; 2] = method_definition.value_param_clauses[0][..2]
        .try_into()
        .unwrap();
    let TreeKind::Apply(pair_apply) = &parsed.ast.get(block.expr).kind else {
        unreachable!()
    };
    let typed_pair = typed_at(&typer, source_id, block.expr);
    let TreeKind::Apply(typed_pair_apply) = &typer.typed_ast().get(typed_pair).kind else {
        unreachable!()
    };
    let arguments = std::array::from_fn(|index| {
        let source_argument = pair_apply.args[index];
        let typed_argument = typed_pair_apply.args[index];
        let result_type = typer.typed_ast().get(typed_argument).ty;
        FormalLocalConstructorCallArgumentBinding {
            source_argument,
            typed_argument,
            origin: FormalLocalConstructorCallArgumentOrigin::LocalValue {
                symbol: symbol_ref(typer.store(), typer.typed_ast(), typed_argument),
                result_type,
            },
        }
    });
    bindings.pair = Some(FormalLocalConstructorCallPairBinding {
        source_method_formals: source_formals,
        method_formals: [formals[0], formals[1]],
        arguments,
    });

    let class = factory;
    let TreeKind::TypeDef(factory_definition) = &parsed.ast.get(factory_tree).kind else {
        unreachable!()
    };
    let TreeKind::Template(factory_template) = &parsed.ast.get(factory_definition.rhs).kind else {
        unreachable!()
    };
    let constructor_tree = factory_template.constructor;
    let constructor = source_index.symbol_at(source_id, constructor_tree).unwrap();
    let members = [ClassMemberPicklePlan::Method {
        symbol: method,
        source_tree: method_tree,
    }];
    let bytes = {
        let plan = ClassStatePicklePlan {
            store: typer.store(),
            definitions: &definitions,
            source_ast: &parsed.ast,
            typed_ast: typer.typed_ast(),
            source_index: &source_index,
            typed_bindings: &bindings,
            source: SourceMetadata {
                id: source_id,
                name: "ConditionalMutableReferenceAssignment.scala".into(),
            },
            class,
            class_tree: factory_tree,
            constructor: PrimaryConstructorInputPlan {
                symbol: constructor,
                source_tree: constructor_tree,
                parameter_clauses: vec![],
            },
            members: &members,
        };
        let validated = plan.validate().unwrap();
        let body_validation = validated.validate_method_bodies();
        if !should_emit {
            assert!(
                body_validation.is_err(),
                "fresh, fully typed reversed Pair arguments fail body validation"
            );
            return;
        }
        body_validation.unwrap();
        validated.pickle().unwrap().bytes().to_vec()
    };
    if let Ok(output_path) = env::var("DOTTY_CONDITIONAL_MUTABLE_TASTY_OUTPUT") {
        fs::write(output_path, &bytes).unwrap();
    }
    let tasty = TastyFile::parse_and_validate_scala_3_9(&bytes).unwrap();
    let index = tasty.ast_address_index().unwrap();
    let create = index
        .iter_nodes_with_tag(DEFDEF_TAG)
        .find_map(|node| {
            let body = index.get(node.offset as u32)?.decode_defdef_body().ok()?;
            (tasty.names().render(body.name).ok()? == "create").then_some(body)
        })
        .expect("PairFactory.create is emitted");
    let RawTree::LengthNode(body) = create.rhs.unwrap() else {
        panic!("method body is a BLOCK")
    };
    assert_eq!(body.tag, BLOCK_TAG);
    let block = body.decode_block().unwrap();
    assert_eq!(
        block.stats.iter().map(raw_tag).collect::<Vec<_>>(),
        [VALDEF_TAG, VALDEF_TAG, VALDEF_TAG, IF_TAG, APPLY_TAG]
    );
    let RawTree::LengthNode(if_node) = &block.stats[3] else {
        panic!("the fourth statement is a bare IF")
    };
    assert_eq!(if_node.tag, IF_TAG);
    let if_node = if_node.decode_if().unwrap();
    assert_eq!(raw_tag(&if_node.then_branch), ASSIGN_TAG);
    assert_eq!(raw_tag(&if_node.else_branch), ASSIGN_TAG);
    assert_eq!(raw_tag(&block.expression), APPLY_TAG);

    let scala_bytes = include_bytes!(
        "../../../tests/fixtures/jvm_tasty_conditional_mutable_reference_assignment/scala39/example/PairFactory.tasty"
    );
    let scala_tasty = TastyFile::parse_and_validate_scala_3_9(scala_bytes).unwrap();
    let scala_index = scala_tasty.ast_address_index().unwrap();
    let scala_create = scala_index
        .iter_nodes_with_tag(DEFDEF_TAG)
        .find_map(|node| {
            let body = scala_index
                .get(node.offset as u32)?
                .decode_defdef_body()
                .ok()?;
            (scala_tasty.names().render(body.name).ok()? == "create").then_some(body)
        })
        .expect("pinned Scala 3.9 oracle contains PairFactory.create");
    let RawTree::LengthNode(scala_body) = scala_create.rhs.unwrap() else {
        panic!("pinned Scala emits the method body as a BLOCK")
    };
    let scala_block = scala_body.decode_block().unwrap();
    assert_eq!(
        block.stats.iter().map(raw_tag).collect::<Vec<_>>(),
        scala_block.stats.iter().map(raw_tag).collect::<Vec<_>>()
    );
    let RawTree::LengthNode(scala_if) = &scala_block.stats[3] else {
        panic!("pinned Scala emits a bare IF in statement position")
    };
    let scala_if = scala_if.decode_if().unwrap();
    assert_eq!(raw_tag(&if_node.condition), raw_tag(&scala_if.condition));
    assert_eq!(
        raw_tag(&if_node.then_branch),
        raw_tag(&scala_if.then_branch)
    );
    assert_eq!(
        raw_tag(&if_node.else_branch),
        raw_tag(&scala_if.else_branch)
    );
    assert_eq!(raw_tag(&block.expression), raw_tag(&scala_block.expression));

    let setter = bindings.setter.unwrap();
    for corrupt in [
        LocalReceiverUnitCallBinding {
            argument_formal: formals[2],
            ..setter
        },
        LocalReceiverUnitCallBinding {
            source_apply: receipt.source_if,
            typed_apply: typed_if,
            ..setter
        },
    ] {
        bindings.setter = Some(corrupt);
        let plan = ClassStatePicklePlan {
            store: typer.store(),
            definitions: &definitions,
            source_ast: &parsed.ast,
            typed_ast: typer.typed_ast(),
            source_index: &source_index,
            typed_bindings: &bindings,
            source: SourceMetadata {
                id: source_id,
                name: "ConditionalMutableReferenceAssignment.scala".into(),
            },
            class,
            class_tree: factory_tree,
            constructor: PrimaryConstructorInputPlan {
                symbol: constructor,
                source_tree: constructor_tree,
                parameter_clauses: vec![],
            },
            members: &members,
        };
        assert!(
            plan.validate()
                .and_then(|validated| validated.validate_method_bodies())
                .is_err()
        );
    }
    bindings.setter = None;
    bindings.setter_pair = Some([setter, setter]);
    let plan = ClassStatePicklePlan {
        store: typer.store(),
        definitions: &definitions,
        source_ast: &parsed.ast,
        typed_ast: typer.typed_ast(),
        source_index: &source_index,
        typed_bindings: &bindings,
        source: SourceMetadata {
            id: source_id,
            name: "ConditionalMutableReferenceAssignment.scala".into(),
        },
        class,
        class_tree: factory_tree,
        constructor: PrimaryConstructorInputPlan {
            symbol: constructor,
            source_tree: constructor_tree,
            parameter_clauses: vec![],
        },
        members: &members,
    };
    assert!(
        plan.validate()
            .and_then(|validated| validated.validate_method_bodies())
            .is_err()
    );

    bindings.setter = Some(setter);
    bindings.setter_pair = None;
    let validate_body =
        |source_ast: &AstArena<Untyped>, typed_ast: &AstArena<Typed>, typed_bindings: &Bindings| {
            ClassStatePicklePlan {
                store: typer.store(),
                definitions: &definitions,
                source_ast,
                typed_ast,
                source_index: &source_index,
                typed_bindings,
                source: SourceMetadata {
                    id: source_id,
                    name: "ConditionalMutableReferenceAssignment.scala".into(),
                },
                class,
                class_tree: factory_tree,
                constructor: PrimaryConstructorInputPlan {
                    symbol: constructor,
                    source_tree: constructor_tree,
                    parameter_clauses: vec![],
                },
                members: &members,
            }
            .validate()
            .and_then(|validated| validated.validate_method_bodies())
        };

    let mut truncated_source = clone_ast(&parsed.ast);
    let mut truncated_typed = clone_ast(typer.typed_ast());
    let TreeKind::Block(truncated_source_block) = &mut truncated_source.get_mut(method_body).kind
    else {
        unreachable!()
    };
    let TreeKind::Block(truncated_typed_block) =
        &mut truncated_typed.get_mut(receipt.typed_block).kind
    else {
        unreachable!()
    };
    truncated_source_block.stats.truncate(1);
    truncated_typed_block.stats.truncate(1);
    assert!(
        validate_body(&truncated_source, &truncated_typed, &bindings).is_err(),
        "a short source/typed block is rejected without panicking"
    );

    let first_initializer = bindings
        .initializers
        .iter()
        .find(|initializer| initializer.local == first.symbol())
        .unwrap();
    let source_new_type = match &parsed.ast.get(first_initializer.call.source_new).kind {
        TreeKind::New(new_tree) => new_tree.tpt,
        _ => unreachable!(),
    };
    for source_child in [first_initializer.call.source_new, source_new_type] {
        let mut corrupted = bindings.clone();
        corrupted
            .typed
            .insert((source_id, source_child), receipt.typed_if);
        assert!(
            validate_body(&parsed.ast, typer.typed_ast(), &corrupted).is_err(),
            "forged constructor NEW/type-tree child mappings are rejected"
        );
    }

    for target in [first, second] {
        let rhs_index = receipt
            .rhs_locals
            .iter()
            .position(|&local| local == target.symbol())
            .unwrap();
        let mut substituted_source = clone_ast(&parsed.ast);
        substituted_source.get_mut(setter.source_receiver).kind =
            parsed.ast.get(receipt.source_rhss[rhs_index]).kind.clone();
        let mut substituted_typed = clone_ast(typer.typed_ast());
        let replacement = typer.typed_ast().get(receipt.typed_rhss[rhs_index]).clone();
        let receiver = substituted_typed.get_mut(setter.typed_receiver);
        receiver.kind = replacement.kind;
        receiver.ty = replacement.ty;
        let substituted_setter = LocalReceiverUnitCallBinding {
            receiver: target.symbol(),
            receiver_scope: target.scope(),
            ..setter
        };
        let mut substituted_bindings = bindings.clone();
        substituted_bindings.setter = Some(substituted_setter);
        assert!(
            validate_body(
                &substituted_source,
                &substituted_typed,
                &substituted_bindings,
            )
            .is_err(),
            "post-IF setter receiver substitutions to original locals are rejected"
        );
    }
}
