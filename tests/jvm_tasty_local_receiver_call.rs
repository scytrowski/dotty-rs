use dotty::jvm_tasty_plan::{PairedPlanError, with_paired_jvm_tasty_batch_plan};
use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::reader::Reader as ClassReader;
use dotty_core::ast::TreeKind;
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceId, SourceText, SymbolId, TermRefTarget, Type,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_tasty::ast::{APPLY_TAG, BLOCK_TAG, DEFDEF_TAG, VALDEF_TAG};
use dotty_tasty::file::TastyFile;
use dotty_tasty::term::RawTree;
use dotty_typer::SourceTyper;

const TARGET: &str = include_str!("fixtures/jvm_tasty_local_receiver_call/LocalReceiverCall.scala");
const COUNTER_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_local_receiver_call/scala39/primary/example/Counter.class");
const COUNTER_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_local_receiver_call/scala39/primary/example/Counter.tasty");
const PAIR_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_local_receiver_call/scala39/primary/example/Pair.class");
const PAIR_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_local_receiver_call/scala39/primary/example/Pair.tasty");
const FACTORY_CLASS: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_local_receiver_call/scala39/primary/example/PairFactory.class"
);
const FACTORY_TASTY: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_local_receiver_call/scala39/primary/example/PairFactory.tasty"
);

fn decode_class(bytes: &[u8]) -> ClassFile<'_> {
    let mut reader = ClassReader::new(bytes);
    let class = ClassFile::decode(&mut reader).unwrap();
    assert!(reader.is_at_end());
    class
}

fn raw_tag(tree: &RawTree<'_>) -> u8 {
    match tree {
        RawTree::Leaf(term) => term.tag,
        RawTree::Ast { tag, .. } | RawTree::NatAst { tag, .. } => *tag,
        RawTree::LengthNode(node) => node.tag,
    }
}

#[test]
fn scala39_oracle_fixture_records_the_block_and_jvm_method() {
    let classes = [
        decode_class(COUNTER_CLASS),
        decode_class(PAIR_CLASS),
        decode_class(FACTORY_CLASS),
    ];
    let tastys = [
        TastyFile::parse_and_validate_scala_3_9(COUNTER_TASTY).unwrap(),
        TastyFile::parse_and_validate_scala_3_9(PAIR_TASTY).unwrap(),
        TastyFile::parse_and_validate_scala_3_9(FACTORY_TASTY).unwrap(),
    ];
    for (class, tasty, expected_name) in classes
        .iter()
        .zip(&tastys)
        .zip(["example/Counter", "example/Pair", "example/PairFactory"])
        .map(|((class, tasty), name)| (class, tasty, name))
    {
        assert_eq!(
            class.constant_pool.class_name(class.this_class).unwrap(),
            expected_name
        );
        assert_eq!(class.version.major, 65);
        assert_eq!(
            (
                tasty.header().major_version,
                tasty.header().minor_version,
                tasty.header().experimental_version
            ),
            (28, 9, 0)
        );
        assert_eq!(
            class.tasty_link_id().unwrap().unwrap().as_bytes(),
            &tasty.header().uuid
        );
    }

    let factory_class = &classes[2];
    let create = factory_class
        .methods
        .iter()
        .find(|method| factory_class.constant_pool.utf8(method.name_index).unwrap() == "create")
        .unwrap();
    assert_eq!(
        factory_class
            .constant_pool
            .utf8(create.descriptor_index)
            .unwrap(),
        "(II)Lexample/Pair;"
    );
    let code = create
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::Code(code) => Some(code),
            _ => None,
        })
        .unwrap();
    assert_eq!((code.max_stack, code.max_locals), (4, 5));

    let factory_tasty = &tastys[2];
    let index = factory_tasty.ast_address_index().unwrap();
    let (_, method) = index
        .iter_nodes_with_tag(DEFDEF_TAG)
        .find_map(|node| {
            let body = index
                .get(node.offset as u32)
                .unwrap()
                .decode_defdef_body()
                .unwrap();
            (factory_tasty.names().render(body.name).unwrap() == "create")
                .then_some((node.offset as u32, body))
        })
        .unwrap();
    let RawTree::LengthNode(block_node) = method.rhs.unwrap() else {
        panic!("Scala encodes create as a BLOCK");
    };
    assert_eq!(block_node.tag, BLOCK_TAG);
    let block = block_node.decode_block().unwrap();
    assert_eq!(block.stats.len(), 3);
    assert_eq!(
        block.stats.iter().map(raw_tag).collect::<Vec<_>>(),
        [VALDEF_TAG, VALDEF_TAG, APPLY_TAG]
    );
    assert_eq!(raw_tag(&block.expression), APPLY_TAG);
}

fn paired_result(source: &str, source_id: u32) -> Result<(), PairedPlanError> {
    with_paired_jvm_tasty_batch_plan(
        source,
        "LocalReceiverCall.scala",
        SourceId::from_index(source_id),
        |_| (),
    )
}

#[test]
fn local_receiver_call_audit_keeps_the_real_frontend_identities_and_first_failure() {
    let source_id = SourceId::from_index(21640);
    let source_text = SourceText::new(TARGET).unwrap();
    let scanner = ContextualScanner::new(TARGET).unwrap();
    assert!(scanner.diagnostics().is_empty());
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = parse_compilation_unit(source_text, source_id, scanner, &mut store.names);
    assert!(parsed.diagnostics.is_empty());
    let mut packages = Packages::new();
    let source_index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        "LocalReceiverCall.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        outcome => panic!("target names cleanly: {outcome:?}"),
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("target package root");
    };
    assert_eq!(package.stats.len(), 3);
    let factory_tree = *package.stats.last().unwrap();
    let factory = source_index.symbol_at(source_id, factory_tree).unwrap();
    let TreeKind::TypeDef(factory_def) = &parsed.ast.get(factory_tree).kind else {
        panic!("PairFactory is a class");
    };
    let TreeKind::Template(factory_template) = &parsed.ast.get(factory_def.rhs).kind else {
        panic!("PairFactory has a template");
    };
    let create_tree = *factory_template
        .body
        .iter()
        .find(|tree| matches!(&parsed.ast.get(**tree).kind, TreeKind::DefDef(def) if def.rhs.is_some()))
        .unwrap();
    let create = source_index.symbol_at(source_id, create_tree).unwrap();
    let TreeKind::DefDef(create_def) = &parsed.ast.get(create_tree).kind else {
        panic!("create is a method");
    };
    let formals = create_def.value_param_clauses[0]
        .iter()
        .map(|tree| source_index.symbol_at(source_id, *tree).unwrap())
        .collect::<Vec<SymbolId>>();
    let Some(body_tree) = create_def.rhs else {
        panic!("create has a body");
    };
    let TreeKind::Block(block) = &parsed.ast.get(body_tree).kind else {
        panic!("create body is a source block");
    };
    assert_eq!(block.stats.len(), 3);
    assert!(matches!(
        &parsed.ast.get(block.stats[0]).kind,
        TreeKind::ValDef(_)
    ));
    assert!(matches!(
        &parsed.ast.get(block.stats[1]).kind,
        TreeKind::ValDef(_)
    ));
    assert!(matches!(
        &parsed.ast.get(block.stats[2]).kind,
        TreeKind::Apply(_)
    ));
    let locals = block.stats[..2]
        .iter()
        .map(|tree| {
            let TreeKind::ValDef(def) = &parsed.ast.get(*tree).kind else {
                unreachable!();
            };
            (def.name, *tree)
        })
        .collect::<Vec<_>>();

    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &source_index,
        &mut store,
        definitions,
        &packages,
    );
    for class_tree in package.stats.iter().copied() {
        let class = source_index.symbol_at(source_id, class_tree).unwrap();
        typer.complete_symbol(class).unwrap();
        let TreeKind::TypeDef(def) = &parsed.ast.get(class_tree).kind else {
            unreachable!();
        };
        let TreeKind::Template(template) = &parsed.ast.get(def.rhs).kind else {
            unreachable!();
        };
        typer
            .complete_symbol(
                source_index
                    .symbol_at(source_id, template.constructor)
                    .unwrap(),
            )
            .unwrap();
        for member in template.body.iter().copied() {
            if matches!(&parsed.ast.get(member).kind, TreeKind::DefDef(_)) {
                typer.type_method_definition(member).unwrap();
            }
        }
    }
    let bindings = locals
        .iter()
        .map(|(_, tree)| typer.typed_local_binding_at(source_id, *tree).unwrap())
        .collect::<Vec<_>>();
    assert_ne!(bindings[0].symbol(), bindings[1].symbol());
    assert_eq!(
        typer.store().symbols.get(bindings[0].symbol()).owner,
        Some(create)
    );
    assert_eq!(
        typer.store().symbols.get(bindings[1].symbol()).owner,
        Some(create)
    );
    let typed_stats = block
        .stats
        .iter()
        .map(|tree| {
            typer
                .source_typed_index()
                .get(source_id, *tree)
                .expect("each original block stat has a typed identity")
        })
        .collect::<Vec<_>>();
    let typed_body = typer
        .source_typed_index()
        .get(source_id, body_tree)
        .expect("the original method block has a typed identity");
    let TreeKind::Block(typed_block) = &typer.typed_ast().get(typed_body).kind else {
        panic!("create keeps the typed block");
    };
    assert_eq!(typed_block.stats, typed_stats);
    for (ordinal, formal) in formals.iter().copied().enumerate() {
        let TreeKind::ValDef(local) = &typer.typed_ast().get(bindings[ordinal].declaration()).kind
        else {
            panic!("each local binding points to its typed ValDef");
        };
        let initializer = local.rhs.expect("both locals have initializers");
        let TreeKind::Apply(constructor_call) = &typer.typed_ast().get(initializer).kind else {
            panic!("each local initializer is a typed constructor application");
        };
        let seed_argument = constructor_call.args[0];
        let seed_type = typer
            .store()
            .types
            .get(typer.typed_ast().get(seed_argument).ty);
        let Type::TermRef {
            target: TermRefTarget::Symbol(seed_symbol),
            ..
        } = seed_type
        else {
            panic!("constructor seed remains a canonical formal reference: {seed_type:?}");
        };
        assert_eq!(*seed_symbol, formal);
    }
    let call_tree = typer
        .source_typed_index()
        .get(source_id, block.stats[2])
        .expect("the source Unit call has a canonical typed stat");
    let typed_ast = typer.typed_ast();
    let TreeKind::Apply(call) = &typed_ast.get(call_tree).kind else {
        panic!("set call is typed as an application");
    };
    let TreeKind::Select(set_select) = &typed_ast.get(call.function).kind else {
        panic!("set call keeps a selected method");
    };
    let TreeKind::Ident(first_receiver) = &typed_ast.get(set_select.qualifier).kind else {
        panic!("set receiver remains the original local identifier");
    };
    let receiver_ty = typer
        .store()
        .types
        .get(typed_ast.get(set_select.qualifier).ty);
    let dotty_core::Type::TermRef {
        target: dotty_core::TermRefTarget::Symbol(receiver_symbol),
        ..
    } = receiver_ty
    else {
        panic!("receiver carries a canonical local term reference: {receiver_ty:?}");
    };
    assert_eq!(*receiver_symbol, bindings[0].symbol());
    let method_ty = typer.store().types.get(typed_ast.get(call.function).ty);
    let dotty_core::Type::TermRef {
        target: dotty_core::TermRefTarget::Symbol(set_symbol),
        ..
    } = method_ty
    else {
        panic!("selection carries a canonical set method reference: {method_ty:?}");
    };
    let counter = source_index.symbol_at(source_id, package.stats[0]).unwrap();
    assert_eq!(typer.store().symbols.get(*set_symbol).owner, Some(counter));
    let TreeKind::TypeDef(counter_def) = &parsed.ast.get(package.stats[0]).kind else {
        panic!("first source class is Counter");
    };
    let TreeKind::Template(counter_template) = &parsed.ast.get(counter_def.rhs).kind else {
        panic!("Counter has a template");
    };
    let set_declaration = counter_template
        .body
        .iter()
        .copied()
        .find(|tree| {
            matches!(&parsed.ast.get(*tree).kind, TreeKind::DefDef(definition) if typer.store().names.try_resolve(definition.name.as_name().text()) == Some("set"))
        })
        .unwrap();
    assert_eq!(
        *set_symbol,
        source_index.symbol_at(source_id, set_declaration).unwrap()
    );
    println!(
        "source_id={source_id:?}; factory={factory:?}; create={create:?}; formals={formals:?}; local_symbols={:?}; source_stats={:?}; typed_call={call_tree:?}@{:?}; receiver_term={receiver_symbol:?}; selected_method={set_symbol:?}; receiver_name={:?}",
        bindings
            .iter()
            .map(|binding| binding.symbol())
            .collect::<Vec<_>>(),
        block.stats,
        typed_ast.get(call_tree).position,
        first_receiver.name,
    );

    let error = paired_result(TARGET, source_id.index())
        .expect_err("the paired source path reports the measured first failure");
    assert!(matches!(
        &error,
        PairedPlanError::BatchUnit { phase, reason }
            if *phase == "cross-class constructor-call identity proof"
                && reason == "SourceShape(\"factory body must be a direct Apply or a one- or two-local Block\")"
    ));
    println!("paired first failure: {error:?}");
    let baseline =
        include_str!("fixtures/jvm_tasty_two_immutable_reference_locals/TwoLocals.scala");
    assert!(
        paired_result(baseline, 21641).is_ok(),
        "the existing no-call shape remains accepted"
    );
}

#[test]
fn local_receiver_call_controls_record_each_real_paired_path_result() {
    let wrong_arity = TARGET.replace("first.set(secondSeed)", "first.set()");
    let wrong_type = TARGET.replace("first.set(secondSeed)", "first.set(true)");
    let receiver_second = TARGET.replace("first.set(secondSeed)", "second.set(secondSeed)");
    let source_order = TARGET.replace(
        "    val first: Counter = new Counter(firstSeed)\n    val second: Counter = new Counter(secondSeed)",
        "    val second: Counter = new Counter(secondSeed)\n    val first: Counter = new Counter(firstSeed)",
    );
    let second_call = TARGET.replace(
        "    first.set(secondSeed)",
        "    first.set(secondSeed)\n    second.set(firstSeed)",
    );

    for (name, source, id) in [
        ("target", TARGET, 21642),
        ("wrong-arity", wrong_arity.as_str(), 21643),
        ("wrong-type", wrong_type.as_str(), 21644),
        ("receiver-second", receiver_second.as_str(), 21645),
        ("reversed-local-order", source_order.as_str(), 21646),
        ("second-call", second_call.as_str(), 21647),
    ] {
        let result = paired_result(source, id);
        println!("control {name}: {result:?}");
        match name {
            "wrong-arity" => assert!(
                matches!(result, Err(PairedPlanError::BatchClass { phase: "method body typing", reason, .. }) if reason.contains("ApplicationArityMismatch"))
            ),
            "wrong-type" => assert!(
                matches!(result, Err(PairedPlanError::BatchClass { phase: "method body typing", reason, .. }) if reason.contains("ApplicationArgumentTypeMismatch"))
            ),
            _ => assert!(
                matches!(result, Err(PairedPlanError::BatchUnit { phase: "cross-class constructor-call identity proof", reason }) if reason.contains("SourceShape"))
            ),
        }
    }
}
