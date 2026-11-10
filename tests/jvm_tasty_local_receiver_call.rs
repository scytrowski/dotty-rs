use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_batch;
use dotty::jvm_tasty_plan::{PairedPlanError, with_paired_jvm_tasty_batch_plan};
use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::ConstantPoolIndex;
use dotty_classfile::reader::Reader as ClassReader;
use dotty_core::ast::TreeKind;
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceId, SourceText, SymbolId, TermRefTarget, Type,
};
use dotty_jvm_tasty_contract::{
    JvmTastyBatchArtifacts, JvmTastyClassFileArtifact, JvmTastyContractError,
    JvmTastySidecarArtifact, validate_jvm_tasty_batch, validate_jvm_tasty_batch_artifacts,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_tasty::ast::{APPLY_TAG, BLOCK_TAG, DEFDEF_TAG, VALDEF_TAG};
use dotty_tasty::file::TastyFile;
use dotty_tasty::term::RawTree;
use dotty_typer::SourceTyper;
use std::process::Command;

#[allow(dead_code)]
#[path = "support/jvm_tasty_batch_publisher.rs"]
mod batch_publisher;

const TARGET: &str = include_str!("fixtures/jvm_tasty_local_receiver_call/LocalReceiverCall.scala");
const TWO_CALL_TARGET: &str =
    include_str!("fixtures/jvm_tasty_ordered_local_unit_calls/OrderedLocalUnitCalls.scala");
const LOCAL_REFERENCE_ALIAS_TARGET: &str =
    include_str!("fixtures/jvm_tasty_local_reference_alias/LocalReferenceAlias.scala");
const ALIAS_CONTROL_RENAMED: &str =
    include_str!("fixtures/jvm_tasty_local_reference_alias/controls/RenamedLocalAlias.scala");
const ALIAS_CONTROL_REVERSED: &str =
    include_str!("fixtures/jvm_tasty_local_reference_alias/controls/ReversedDeclarations.scala");
const ALIAS_CONTROL_SECOND: &str =
    include_str!("fixtures/jvm_tasty_local_reference_alias/controls/AliasOfSecond.scala");
const ALIAS_CONTROL_BEFORE_INIT: &str = include_str!(
    "fixtures/jvm_tasty_local_reference_alias/controls/AliasBeforeInitialization.scala"
);
const ALIAS_CONTROL_WRONG_TYPE: &str =
    include_str!("fixtures/jvm_tasty_local_reference_alias/controls/WrongAliasType.scala");
const ALIAS_CONTROL_MUTABLE: &str =
    include_str!("fixtures/jvm_tasty_local_reference_alias/controls/MutableAlias.scala");
const ALIAS_CONTROL_CHAIN: &str =
    include_str!("fixtures/jvm_tasty_local_reference_alias/controls/AliasChain.scala");
const TWO_LOCAL_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_two_immutable_reference_locals/TwoLocals.scala");
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
const TWO_CALL_FACTORY_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_ordered_local_unit_calls/scala39/example/PairFactory.class");
const TWO_CALL_FACTORY_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_ordered_local_unit_calls/scala39/example/PairFactory.tasty");
const LOCAL_REFERENCE_ALIAS_FACTORY_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_local_reference_alias/scala39/example/PairFactory.class");
const LOCAL_REFERENCE_ALIAS_FACTORY_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_local_reference_alias/scala39/example/PairFactory.tasty");
const LOCAL_REFERENCE_ALIAS_COUNTER_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_local_reference_alias/scala39/example/Counter.class");
const LOCAL_REFERENCE_ALIAS_COUNTER_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_local_reference_alias/scala39/example/Counter.tasty");
const LOCAL_REFERENCE_ALIAS_PAIR_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_local_reference_alias/scala39/example/Pair.class");
const LOCAL_REFERENCE_ALIAS_PAIR_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_local_reference_alias/scala39/example/Pair.tasty");

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

fn require_jdk_21_or_skip() -> bool {
    let available = ["java", "javac"].into_iter().all(|tool| {
        match Command::new(tool).arg("-version").output() {
            Ok(output) if output.status.success() => {
                let version = format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                version.contains("21.")
            }
            _ => false,
        }
    });
    if available {
        true
    } else if std::env::var("DOTTY_CLASSFILE_REQUIRE_JDK").as_deref() == Ok("1") {
        panic!("the Counter.set receiver runtime test requires JDK 21 on PATH");
    } else {
        eprintln!("skipping Counter.set receiver runtime test without JDK 21");
        false
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

#[test]
fn scala39_two_call_oracle_records_ordered_block_and_bytecode() {
    let class = decode_class(TWO_CALL_FACTORY_CLASS);
    let tasty = TastyFile::parse_and_validate_scala_3_9(TWO_CALL_FACTORY_TASTY).unwrap();
    assert_eq!(class.version.major, 65);
    assert_eq!(
        class.constant_pool.class_name(class.this_class).unwrap(),
        "example/PairFactory"
    );
    assert_eq!(
        class.tasty_link_id().unwrap().unwrap().as_bytes(),
        &tasty.header().uuid
    );
    assert_eq!(
        (
            tasty.header().major_version,
            tasty.header().minor_version,
            tasty.header().experimental_version
        ),
        (28, 9, 0)
    );
    let method = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == "create")
        .unwrap();
    assert_eq!(
        class.constant_pool.utf8(method.descriptor_index).unwrap(),
        "(II)Lexample/Pair;"
    );
    let code = method
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::Code(code) => Some(code),
            _ => None,
        })
        .unwrap();
    assert_eq!((code.max_stack, code.max_locals), (4, 5));

    let index = tasty.ast_address_index().unwrap();
    let (_, body) = index
        .iter_nodes_with_tag(DEFDEF_TAG)
        .find_map(|node| {
            let body = index
                .get(node.offset as u32)
                .unwrap()
                .decode_defdef_body()
                .unwrap();
            (tasty.names().render(body.name).unwrap() == "create")
                .then_some((node.offset as u32, body))
        })
        .unwrap();
    let RawTree::LengthNode(block_node) = body.rhs.unwrap() else {
        panic!("Scala encodes create as a BLOCK");
    };
    assert_eq!(block_node.tag, BLOCK_TAG);
    let block = block_node.decode_block().unwrap();
    assert!(
        index
            .iter_nodes_with_tag(BLOCK_TAG)
            .any(|node| node.offset == 58)
    );
    assert_eq!(block.stats.len(), 4);
    assert_eq!(
        block.stats.iter().map(raw_tag).collect::<Vec<_>>(),
        [VALDEF_TAG, VALDEF_TAG, APPLY_TAG, APPLY_TAG]
    );
    assert_eq!(raw_tag(&block.expression), APPLY_TAG);

    let javap =
        include_str!("fixtures/jvm_tasty_ordered_local_unit_calls/oracle/PairFactory.javap.txt");
    let first_call = javap.find("Method example/Counter.set:(I)V").unwrap();
    let second_call = javap[first_call + 1..]
        .find("Method example/Counter.set:(I)V")
        .unwrap()
        + first_call
        + 1;
    let final_pair = javap.find("Method example/Pair.\"<init>\"").unwrap();
    assert!(first_call < second_call && second_call < final_pair);
    assert!(javap.contains("aload_3\n        20: iload_2"));
    assert!(javap.contains("aload         4\n        26: iload_1"));

    let tasty_print =
        include_str!("fixtures/jvm_tasty_ordered_local_unit_calls/oracle/print-tasty.txt");
    for observation in [
        "58:         BLOCK(82)",
        "120:           APPLY(9)",
        "131:           APPLY(9)",
        "72:             TERMREFdirect 76",
        "74:             TERMREFdirect 99",
    ] {
        assert!(
            tasty_print.contains(observation),
            "missing TASTy observation {observation:?}"
        );
    }
}

#[test]
fn scala39_local_reference_alias_oracle_records_alias_block_and_bytecode() {
    for (class_bytes, tasty_bytes, expected_name) in [
        (
            LOCAL_REFERENCE_ALIAS_COUNTER_CLASS,
            LOCAL_REFERENCE_ALIAS_COUNTER_TASTY,
            "example/Counter",
        ),
        (
            LOCAL_REFERENCE_ALIAS_PAIR_CLASS,
            LOCAL_REFERENCE_ALIAS_PAIR_TASTY,
            "example/Pair",
        ),
        (
            LOCAL_REFERENCE_ALIAS_FACTORY_CLASS,
            LOCAL_REFERENCE_ALIAS_FACTORY_TASTY,
            "example/PairFactory",
        ),
    ] {
        let class = decode_class(class_bytes);
        let tasty = TastyFile::parse_and_validate_scala_3_9(tasty_bytes).unwrap();
        assert_eq!(class.version.major, 65);
        assert_eq!(
            class.constant_pool.class_name(class.this_class).unwrap(),
            expected_name
        );
        assert_eq!(
            class.tasty_link_id().unwrap().unwrap().as_bytes(),
            &tasty.header().uuid
        );
        assert_eq!(
            (
                tasty.header().major_version,
                tasty.header().minor_version,
                tasty.header().experimental_version
            ),
            (28, 9, 0)
        );
    }
    let class = decode_class(LOCAL_REFERENCE_ALIAS_FACTORY_CLASS);
    let tasty =
        TastyFile::parse_and_validate_scala_3_9(LOCAL_REFERENCE_ALIAS_FACTORY_TASTY).unwrap();
    assert_eq!(class.version.major, 65);
    assert_eq!(
        class.constant_pool.class_name(class.this_class).unwrap(),
        "example/PairFactory"
    );
    assert_eq!(
        class.tasty_link_id().unwrap().unwrap().as_bytes(),
        &tasty.header().uuid
    );
    assert_eq!(
        (
            tasty.header().major_version,
            tasty.header().minor_version,
            tasty.header().experimental_version
        ),
        (28, 9, 0)
    );
    let method = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == "create")
        .unwrap();
    assert_eq!(
        class.constant_pool.utf8(method.descriptor_index).unwrap(),
        "(II)Lexample/Pair;"
    );
    let code = method
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::Code(code) => Some(code),
            _ => None,
        })
        .unwrap();
    assert_eq!((code.max_stack, code.max_locals), (4, 6));

    let index = tasty.ast_address_index().unwrap();
    let (_, body) = index
        .iter_nodes_with_tag(DEFDEF_TAG)
        .find_map(|node| {
            let body = index
                .get(node.offset as u32)
                .unwrap()
                .decode_defdef_body()
                .unwrap();
            (tasty.names().render(body.name).unwrap() == "create")
                .then_some((node.offset as u32, body))
        })
        .unwrap();
    let RawTree::LengthNode(block_node) = body.rhs.unwrap() else {
        panic!("Scala encodes create as a BLOCK");
    };
    assert_eq!(block_node.tag, BLOCK_TAG);
    let block = block_node.decode_block().unwrap();
    assert_eq!(block.stats.len(), 5);
    assert_eq!(
        block.stats.iter().map(raw_tag).collect::<Vec<_>>(),
        [VALDEF_TAG, VALDEF_TAG, VALDEF_TAG, APPLY_TAG, APPLY_TAG]
    );
    assert_eq!(raw_tag(&block.expression), APPLY_TAG);

    let printed =
        include_str!("fixtures/jvm_tasty_local_reference_alias/oracle/PairFactory.print-tasty.txt");
    for observation in [
        "58:         BLOCK(91)",
        "72:             TERMREFdirect 76",
        "74:             TERMREFdirect 99",
        "120:           VALDEF(7) 25 [alias]",
        "127:             SHAREDtype 72",
        "134:               TERMREFdirect 120",
        "145:               SHAREDtype 74",
    ] {
        assert!(
            printed.contains(observation),
            "missing TASTy observation {observation:?}"
        );
    }
    let javap =
        include_str!("fixtures/jvm_tasty_local_reference_alias/oracle/PairFactory.javap.txt");
    assert!(javap.contains("stack=4, locals=6, args_size=3"));
    assert!(javap.contains("19: aload_3\n        20: astore        5"));
    assert!(javap.contains("22: aload         5\n        24: iload_2"));
    let alias_call = javap.find("Method example/Counter.set:(I)V").unwrap();
    let second_call = javap[alias_call + 1..]
        .find("Method example/Counter.set:(I)V")
        .unwrap()
        + alias_call
        + 1;
    let final_pair = javap.find("Method example/Pair.\"<init>\"").unwrap();
    assert!(alias_call < second_call && second_call < final_pair);
    assert!(javap.contains("38: aload_3\n        39: aload         4"));
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
fn local_reference_alias_audit_authenticates_frontend_identities_and_first_paired_failure() {
    let source_id = SourceId::from_index(22320);
    let source_text = SourceText::new(LOCAL_REFERENCE_ALIAS_TARGET).unwrap();
    let scanner = ContextualScanner::new(LOCAL_REFERENCE_ALIAS_TARGET).unwrap();
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
        "LocalReferenceAlias.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        outcome => panic!("alias target names cleanly: {outcome:?}"),
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
    let body_tree = create_def.rhs.expect("create has a body");
    let TreeKind::Block(block) = &parsed.ast.get(body_tree).kind else {
        panic!("create body is a source block");
    };
    assert_eq!(block.stats.len(), 5);
    assert!(
        block.stats[..3]
            .iter()
            .all(|tree| matches!(&parsed.ast.get(*tree).kind, TreeKind::ValDef(_)))
    );
    assert!(
        block.stats[3..]
            .iter()
            .all(|tree| matches!(&parsed.ast.get(*tree).kind, TreeKind::Apply(_)))
    );

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

    let bindings = block.stats[..3]
        .iter()
        .map(|tree| typer.typed_local_binding_at(source_id, *tree).unwrap())
        .collect::<Vec<_>>();
    let [first, second, alias] = bindings.as_slice() else {
        panic!("target has exactly three immutable locals");
    };
    for binding in bindings.iter() {
        assert_eq!(
            typer.store().symbols.get(binding.symbol()).owner,
            Some(create)
        );
        assert_eq!(
            typer.store().scopes.get(binding.scope()).owner,
            Some(create)
        );
    }
    assert_ne!(first.symbol(), second.symbol());
    assert_ne!(alias.symbol(), first.symbol());
    assert_ne!(alias.symbol(), second.symbol());

    let typed_ast = typer.typed_ast();
    let typed_stats = block
        .stats
        .iter()
        .map(|tree| {
            typer
                .source_typed_index()
                .get(source_id, *tree)
                .expect("each source statement has a typed identity")
        })
        .collect::<Vec<_>>();
    let typed_body = typer
        .source_typed_index()
        .get(source_id, body_tree)
        .expect("the source method block has a typed identity");
    let TreeKind::Block(typed_block) = &typed_ast.get(typed_body).kind else {
        panic!("create keeps a typed block");
    };
    assert_eq!(typed_block.stats, typed_stats);

    let alias_declaration = typed_ast.get(alias.declaration());
    let TreeKind::ValDef(alias_val) = &alias_declaration.kind else {
        panic!("alias binding points to its typed ValDef");
    };
    let alias_rhs = alias_val.rhs.expect("alias has an initializer");
    let alias_rhs_type = typer.store().types.get(typed_ast.get(alias_rhs).ty);
    let dotty_core::Type::TermRef {
        target: dotty_core::TermRefTarget::Symbol(alias_source),
        ..
    } = alias_rhs_type
    else {
        panic!("alias initializer retains a canonical local reference: {alias_rhs_type:?}");
    };
    assert_eq!(*alias_source, first.symbol());

    for (ordinal, expected_receiver) in [(3, alias.symbol()), (4, second.symbol())] {
        let call_tree = typed_stats[ordinal];
        let TreeKind::Apply(call) = &typed_ast.get(call_tree).kind else {
            panic!("set call is a typed application");
        };
        let TreeKind::Select(selection) = &typed_ast.get(call.function).kind else {
            panic!("set call has a selected receiver");
        };
        let receiver_type = typer
            .store()
            .types
            .get(typed_ast.get(selection.qualifier).ty);
        let dotty_core::Type::TermRef {
            target: dotty_core::TermRefTarget::Symbol(receiver),
            ..
        } = receiver_type
        else {
            panic!("call receiver retains a canonical local reference: {receiver_type:?}");
        };
        assert_eq!(*receiver, expected_receiver);
    }

    let typed_result = typer
        .source_typed_index()
        .get(source_id, block.expr)
        .expect("the final Pair expression has a typed identity");
    let TreeKind::Apply(pair_call) = &typed_ast.get(typed_result).kind else {
        panic!("create result is a Pair constructor application");
    };
    let pair_argument_symbols = pair_call
        .args
        .iter()
        .map(
            |argument| match typer.store().types.get(typed_ast.get(*argument).ty) {
                dotty_core::Type::TermRef {
                    target: dotty_core::TermRefTarget::Symbol(symbol),
                    ..
                } => *symbol,
                ty => panic!("constructor argument retains its local identity: {ty:?}"),
            },
        )
        .collect::<Vec<_>>();
    assert_eq!(pair_argument_symbols, [first.symbol(), second.symbol()]);
    let source_to_typed = block
        .stats
        .iter()
        .copied()
        .zip(typed_stats.iter().copied())
        .collect::<Vec<_>>();
    let alias_source_rhs = match &parsed.ast.get(block.stats[2]).kind {
        TreeKind::ValDef(alias) => alias.rhs.expect("source alias has an initializer"),
        _ => unreachable!(),
    };
    let alias_rhs_mapping = (
        alias_source_rhs,
        typer
            .source_typed_index()
            .get(source_id, alias_source_rhs)
            .expect("alias initializer maps to a typed tree"),
    );
    assert_eq!(alias_rhs_mapping.1, alias_rhs);
    let result_mapping = (
        block.expr,
        typer
            .source_typed_index()
            .get(source_id, block.expr)
            .expect("final Pair expression maps to a typed tree"),
    );
    assert_eq!(result_mapping.1, typed_result);

    println!(
        "local reference alias source_id={source_id:?}; factory={factory:?}; create={create:?}; formals={formals:?}; locals=[{:?}@{:?}, {:?}@{:?}, {:?}@{:?}]; source_to_typed={source_to_typed:?}; alias_rhs={alias_source:?} via {alias_rhs_mapping:?}; receivers=[{:?}, {:?}]; final_pair={result_mapping:?} args={pair_argument_symbols:?}",
        first.symbol(),
        first.scope(),
        second.symbol(),
        second.scope(),
        alias.symbol(),
        alias.scope(),
        alias.symbol(),
        second.symbol(),
    );

    let mut observed_plan_classes = Vec::new();
    let result = with_paired_jvm_tasty_batch_plan(
        LOCAL_REFERENCE_ALIAS_TARGET,
        "LocalReferenceAlias.scala",
        SourceId::from_index(22321),
        |plan| observed_plan_classes.extend(plan.classes.iter().map(|_| ())),
    );
    println!("current paired producer result: {result:?}");
    assert!(
        result.is_ok(),
        "the authenticated alias and calls reach paired planning"
    );
    assert!(
        observed_plan_classes.len() == 3,
        "the accepted target reaches the callback with all three class plans"
    );
    let artifact_batch = emit_paired_jvm_tasty_batch(
        LOCAL_REFERENCE_ALIAS_TARGET,
        "LocalReferenceAlias.scala",
        SourceId::from_index(22322),
    )
    .expect("the authenticated alias and calls produce all paired artifacts");
    assert_eq!(artifact_batch.classes().len(), 3);

    let baseline =
        include_str!("fixtures/jvm_tasty_two_immutable_reference_locals/TwoLocals.scala");
    assert!(
        paired_result(baseline, 22323).is_ok(),
        "the zero-call baseline remains accepted"
    );
    assert!(
        paired_result(TARGET, 22324).is_ok(),
        "the one-call baseline remains accepted"
    );
    assert!(
        paired_result(TWO_CALL_TARGET, 22325).is_ok(),
        "the two-call baseline remains accepted"
    );

    let source_prefix = LOCAL_REFERENCE_ALIAS_TARGET
        .split_once("class PairFactory:")
        .map(|(prefix, _)| prefix)
        .expect("target has the shared Counter and Pair declarations");
    enum ControlExpectation {
        Accepted,
        RejectedShape,
        InvalidForwardReference,
        InvalidAliasType,
    }
    for (name, control, source_id, expectation) in [
        (
            "renamed-local-alias",
            ALIAS_CONTROL_RENAMED,
            22326,
            ControlExpectation::Accepted,
        ),
        (
            "reversed-declarations",
            ALIAS_CONTROL_REVERSED,
            22327,
            ControlExpectation::Accepted,
        ),
        (
            "alias-of-second",
            ALIAS_CONTROL_SECOND,
            22328,
            ControlExpectation::RejectedShape,
        ),
        (
            "alias-before-initialization",
            ALIAS_CONTROL_BEFORE_INIT,
            22329,
            ControlExpectation::InvalidForwardReference,
        ),
        (
            "wrong-alias-type",
            ALIAS_CONTROL_WRONG_TYPE,
            22330,
            ControlExpectation::InvalidAliasType,
        ),
        (
            "mutable-alias",
            ALIAS_CONTROL_MUTABLE,
            22331,
            ControlExpectation::RejectedShape,
        ),
        (
            "alias-chain",
            ALIAS_CONTROL_CHAIN,
            22332,
            ControlExpectation::RejectedShape,
        ),
    ] {
        let factory_source = control
            .strip_prefix("package example\n\n")
            .expect("independent control carries its package declaration");
        let source = format!("{source_prefix}{factory_source}");
        let result = paired_result(&source, source_id);
        println!("alias control {name}: {result:?}");
        match expectation {
            ControlExpectation::Accepted => assert!(
                result.is_ok(),
                "valid control {name} preserves canonical alias identity"
            ),
            ControlExpectation::RejectedShape => assert!(
                matches!(
                    result,
                    Err(PairedPlanError::BatchUnit {
                        phase: "cross-class constructor-call identity proof",
                        ..
                    })
                ),
                "out-of-contract control {name} remains rejected by the source-shape proof"
            ),
            ControlExpectation::InvalidForwardReference => match result {
                Err(PairedPlanError::BatchClass {
                    class_index: 2,
                    declaration_index: 0,
                    phase: "method body typing",
                    reason,
                }) => {
                    assert!(reason.starts_with("TermNameNotFound"));
                    assert!(reason.contains(&format!("source: SourceId({source_id})")));
                    assert!(reason.contains(&format!("source: SourceId({source_id}), span:")));
                    assert!(reason.contains("start: 445, end: 450"));
                }
                other => panic!("invalid forward-reference control fails during typing: {other:?}"),
            },
            ControlExpectation::InvalidAliasType => match result {
                Err(PairedPlanError::BatchClass {
                    class_index: 2,
                    declaration_index: 0,
                    phase: "method body typing",
                    reason,
                }) => {
                    assert!(reason.starts_with(
                        "LocalValueTypeMismatch { source: SourceId(22330), tree_index: 65, "
                    ));
                    assert!(reason.contains(", actual: TypeId("));
                    assert!(reason.contains(", expected: TypeId("));
                }
                other => panic!("invalid alias-type control fails during typing: {other:?}"),
            },
        }
    }
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

    with_paired_jvm_tasty_batch_plan(TARGET, "LocalReceiverCall.scala", source_id, |plan| {
        assert_eq!(plan.classes.len(), 3);
        let [proof] = plan.constructor_call_proofs else {
            panic!("the source batch retains one canonical constructor proof");
        };
        let call = proof
            .local_receiver_unit_call()
            .expect("the paired proof carries the authenticated Unit call");
        assert_eq!(call.receiver, bindings[0].symbol());
        assert_eq!(call.argument_formal, formals[1]);
        assert_eq!(call.method, *set_symbol);
        assert_eq!(call.source_apply, block.stats[2]);
        assert_ne!(call.typed_apply, call.typed_select);
    })
    .expect("the paired planner admits the authenticated local Unit call");
    let baseline =
        include_str!("fixtures/jvm_tasty_two_immutable_reference_locals/TwoLocals.scala");
    assert!(
        paired_result(baseline, 21641).is_ok(),
        "the existing no-call shape remains accepted"
    );
}

#[test]
fn ordered_two_call_audit_authenticates_source_and_emits_complete_pair_batch() {
    let source_id = SourceId::from_index(21870);
    let source_text = SourceText::new(TWO_CALL_TARGET).unwrap();
    let scanner = ContextualScanner::new(TWO_CALL_TARGET).unwrap();
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
        "OrderedLocalUnitCalls.scala",
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
    let factory_tree = *package.stats.last().unwrap();
    let factory = source_index.symbol_at(source_id, factory_tree).unwrap();
    let TreeKind::TypeDef(factory_def) = &parsed.ast.get(factory_tree).kind else {
        panic!("PairFactory is a class");
    };
    let TreeKind::Template(template) = &parsed.ast.get(factory_def.rhs).kind else {
        panic!("PairFactory has a template");
    };
    let create_tree = *template
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
        .collect::<Vec<_>>();
    let body_tree = create_def.rhs.unwrap();
    let TreeKind::Block(block) = &parsed.ast.get(body_tree).kind else {
        panic!("create body is a source block");
    };
    assert_eq!(block.stats.len(), 4);
    assert!(
        block.stats[..2]
            .iter()
            .all(|tree| matches!(&parsed.ast.get(*tree).kind, TreeKind::ValDef(_)))
    );
    assert!(
        block.stats[2..]
            .iter()
            .all(|tree| matches!(&parsed.ast.get(*tree).kind, TreeKind::Apply(_)))
    );
    let local_trees = block.stats[..2].to_vec();

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
    assert_eq!(typer.store().symbols.get(create).owner, Some(factory));
    let typed_ast = typer.typed_ast();
    let typed_block_id = typer
        .source_typed_index()
        .get(source_id, body_tree)
        .unwrap();
    let TreeKind::Block(typed_block) = &typed_ast.get(typed_block_id).kind else {
        panic!("create retains the typed block");
    };
    assert_eq!(typed_block.stats.len(), 4);
    let typed_local_bindings = local_trees
        .iter()
        .map(|tree| typer.typed_local_binding_at(source_id, *tree).unwrap())
        .collect::<Vec<_>>();
    for (tree, binding) in local_trees.iter().zip(&typed_local_bindings) {
        assert_eq!(
            typer.source_typed_index().get(source_id, *tree),
            Some(binding.declaration())
        );
        assert!(matches!(
            &typed_ast.get(binding.declaration()).kind,
            TreeKind::ValDef(_)
        ));
    }
    assert_ne!(
        typed_local_bindings[0].symbol(),
        typed_local_bindings[1].symbol()
    );
    assert!(
        typed_local_bindings
            .iter()
            .all(|binding| { typer.store().symbols.get(binding.symbol()).owner == Some(create) })
    );
    let typed_local_symbols = typed_local_bindings
        .iter()
        .map(|binding| binding.symbol())
        .collect::<Vec<_>>();
    let source_result = block.expr;
    let typed_result = typer
        .source_typed_index()
        .get(source_id, source_result)
        .expect("the original final Pair construction has a typed identity");
    assert_eq!(typed_block.expr, typed_result);
    let TreeKind::Apply(pair_call) = &typed_ast.get(typed_result).kind else {
        panic!("the final result remains the two-local Pair construction");
    };
    let TreeKind::Select(_) = &typed_ast.get(pair_call.function).kind else {
        panic!("the final result retains its canonical Pair constructor selection");
    };
    let constructor_ty = typer
        .store()
        .types
        .get(typed_ast.get(pair_call.function).ty);
    let Type::TermRef {
        target: TermRefTarget::Symbol(pair_constructor_symbol),
        ..
    } = constructor_ty
    else {
        panic!("Pair constructor selection carries canonical identity: {constructor_ty:?}");
    };
    assert_eq!(
        typer.store().symbols.get(*pair_constructor_symbol).owner,
        Some(source_index.symbol_at(source_id, package.stats[1]).unwrap())
    );
    assert_eq!(pair_call.args.len(), 2);
    for (argument, local_symbol) in pair_call.args.iter().zip(&typed_local_symbols) {
        let argument_ty = typer.store().types.get(typed_ast.get(*argument).ty);
        assert!(matches!(
            argument_ty,
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } if symbol == local_symbol
        ));
    }
    for (ordinal, (receiver_ordinal, formal_ordinal)) in [(0, 1), (1, 0)].into_iter().enumerate() {
        let typed_stat = typer
            .source_typed_index()
            .get(source_id, block.stats[ordinal + 2])
            .unwrap();
        assert_eq!(typed_block.stats[ordinal + 2], typed_stat);
        let TreeKind::Apply(call) = &typed_ast.get(typed_stat).kind else {
            panic!("each ordered Unit call stays an Apply");
        };
        let TreeKind::Select(selection) = &typed_ast.get(call.function).kind else {
            panic!("each Unit call retains a selected method");
        };
        let receiver_ty = typer
            .store()
            .types
            .get(typed_ast.get(selection.qualifier).ty);
        let Type::TermRef {
            target: TermRefTarget::Symbol(receiver),
            ..
        } = receiver_ty
        else {
            panic!("receiver has canonical local identity: {receiver_ty:?}");
        };
        assert_eq!(*receiver, typed_local_symbols[receiver_ordinal]);
        let method_ty = typer.store().types.get(typed_ast.get(call.function).ty);
        let Type::TermRef {
            target: TermRefTarget::Symbol(method),
            ..
        } = method_ty
        else {
            panic!("selection has canonical method identity: {method_ty:?}");
        };
        assert_eq!(
            typer.store().symbols.get(*method).owner,
            Some(source_index.symbol_at(source_id, package.stats[0]).unwrap())
        );
        let argument_ty = typer.store().types.get(typed_ast.get(call.args[0]).ty);
        let Type::TermRef {
            target: TermRefTarget::Symbol(argument),
            ..
        } = argument_ty
        else {
            panic!("argument retains canonical formal identity: {argument_ty:?}");
        };
        assert_eq!(*argument, formals[formal_ordinal]);
        println!(
            "ordered_call={ordinal}; source_tree={:?}; typed_tree={typed_stat:?}; receiver={receiver:?}; method={method:?}; argument={argument:?}",
            block.stats[ordinal + 2]
        );
    }

    let batch =
        emit_paired_jvm_tasty_batch(TWO_CALL_TARGET, "OrderedLocalUnitCalls.scala", source_id);
    let batch = batch.expect("the ordered proof emits the complete paired artifact batch");
    let repeated =
        emit_paired_jvm_tasty_batch(TWO_CALL_TARGET, "OrderedLocalUnitCalls.scala", source_id)
            .expect("a fresh source session emits the same six artifacts");
    assert_eq!(batch.classes(), repeated.classes());
    assert_eq!(batch.classes().len(), 3);
    let expected = [
        ("example/Counter.class", "example/Counter.tasty"),
        ("example/Pair.class", "example/Pair.tasty"),
        ("example/PairFactory.class", "example/PairFactory.tasty"),
    ];
    for (artifacts, (class_path, tasty_path)) in batch.classes().iter().zip(expected) {
        assert_eq!(artifacts.classfile_path(), class_path);
        assert_eq!(artifacts.tasty_path(), tasty_path);
        let class = decode_class(artifacts.classfile_bytes());
        let tasty = TastyFile::parse_and_validate_scala_3_9(artifacts.tasty_bytes()).unwrap();
        assert_eq!(class.version.major, 65);
        assert_eq!(
            (tasty.header().major_version, tasty.header().minor_version),
            (28, 9)
        );
        assert_eq!(
            class.tasty_link_id().unwrap().unwrap().as_bytes(),
            &tasty.header().uuid
        );
        if class_path.ends_with("PairFactory.class") {
            let index = tasty.ast_address_index().unwrap();
            let (_, method) = index
                .iter_nodes_with_tag(DEFDEF_TAG)
                .find_map(|node| {
                    let body = index
                        .get(node.offset as u32)
                        .unwrap()
                        .decode_defdef_body()
                        .unwrap();
                    (tasty.names().render(body.name).unwrap() == "create")
                        .then_some((node.offset as u32, body))
                })
                .unwrap();
            let RawTree::LengthNode(block) = method.rhs.unwrap() else {
                panic!("Rust TASTy records create's source BLOCK");
            };
            let block = block.decode_block().unwrap();
            assert_eq!(block.stats.len(), 4);
            assert_eq!(
                block.stats.iter().map(raw_tag).collect::<Vec<_>>(),
                [VALDEF_TAG, VALDEF_TAG, APPLY_TAG, APPLY_TAG]
            );
            assert_eq!(raw_tag(&block.expression), APPLY_TAG);

            let create = class
                .methods
                .iter()
                .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == "create")
                .unwrap();
            let [Attribute::Code(code)] = create.attributes.as_slice() else {
                panic!("PairFactory.create has one Code attribute");
            };
            assert_eq!((code.max_stack, code.max_locals), (4, 5));
            let invocations = code
                .code
                .windows(3)
                .enumerate()
                .filter(|(_, instruction)| instruction[0] == 0xb6)
                .map(|(pc, instruction)| {
                    let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Methodref {
                        class_index,
                        name_and_type_index,
                    }) = class
                        .constant_pool
                        .get(ConstantPoolIndex(u16::from_be_bytes([
                            instruction[1],
                            instruction[2],
                        ])))
                    else {
                        panic!("invokevirtual references CONSTANT_Methodref");
                    };
                    let owner = class.constant_pool.class_name(*class_index).unwrap();
                    let (name, descriptor) = class
                        .constant_pool
                        .name_and_type(*name_and_type_index)
                        .unwrap();
                    (pc, owner.to_owned(), name.to_owned(), descriptor.to_owned())
                })
                .collect::<Vec<_>>();
            assert_eq!(invocations.len(), 2);
            assert!(invocations.iter().all(|(_, owner, name, descriptor)| {
                owner == "example/Counter" && name == "set" && descriptor == "(I)V"
            }));
            let pair_constructor = code
                .code
                .windows(3)
                .enumerate()
                .find_map(|(pc, instruction)| {
                    if instruction[0] != 0xb7 {
                        return None;
                    }
                    let index =
                        ConstantPoolIndex(u16::from_be_bytes([instruction[1], instruction[2]]));
                    let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Methodref {
                        class_index,
                        name_and_type_index,
                    }) = class.constant_pool.get(index)
                    else {
                        return None;
                    };
                    let owner = class.constant_pool.class_name(*class_index).ok()?;
                    let (name, descriptor) = class
                        .constant_pool
                        .name_and_type(*name_and_type_index)
                        .ok()?;
                    (owner == "example/Pair"
                        && name == "<init>"
                        && descriptor == "(Lexample/Counter;Lexample/Counter;)V")
                        .then_some(pc)
                })
                .expect("the final call constructs Pair(Counter, Counter)");
            assert!(invocations[0].0 < invocations[1].0);
            assert!(invocations[1].0 < pair_constructor);
        }
    }
}

#[test]
fn ordered_two_call_controls_authenticate_supported_source_before_artifact_rejection() {
    let reversed_declarations = TWO_CALL_TARGET.replace(
        "    val first: Counter = new Counter(firstSeed)\n    val second: Counter = new Counter(secondSeed)",
        "    val second: Counter = new Counter(secondSeed)\n    val first: Counter = new Counter(firstSeed)",
    );
    let swapped_calls = TWO_CALL_TARGET.replace(
        "    first.set(secondSeed)\n    second.set(firstSeed)",
        "    second.set(firstSeed)\n    first.set(secondSeed)",
    );
    let same_receiver =
        TWO_CALL_TARGET.replace("    second.set(firstSeed)", "    first.set(firstSeed)");
    let duplicate_call = TWO_CALL_TARGET.replace(
        "    first.set(secondSeed)\n    second.set(firstSeed)",
        "    first.set(secondSeed)\n    first.set(secondSeed)",
    );
    let modified_argument =
        TWO_CALL_TARGET.replace("    second.set(firstSeed)", "    second.set(secondSeed)");
    let wrong_arity = TWO_CALL_TARGET.replace("first.set(secondSeed)", "first.set()");
    let wrong_type = TWO_CALL_TARGET.replace("first.set(secondSeed)", "first.set(true)");
    let third_call = TWO_CALL_TARGET.replace(
        "    second.set(firstSeed)",
        "    second.set(firstSeed)\n    first.set(firstSeed)",
    );
    assert_ne!(same_receiver, duplicate_call);

    for (name, source, source_id) in [
        ("original", TWO_CALL_TARGET, 21879),
        (
            "reversed-declarations",
            reversed_declarations.as_str(),
            21871,
        ),
        ("swapped-call-order", swapped_calls.as_str(), 21872),
        ("same-receiver", same_receiver.as_str(), 21873),
        ("duplicate-call", duplicate_call.as_str(), 21874),
        ("modified-argument", modified_argument.as_str(), 21875),
        ("third-call-deferred", third_call.as_str(), 21878),
    ] {
        let result = paired_result(source, source_id);
        println!("ordered two-call control {name}: {result:?}");
        if matches!(
            name,
            "original" | "reversed-declarations" | "swapped-call-order" | "same-receiver"
        ) {
            assert!(
                result.is_ok(),
                "the ordered source proof admits this bounded variant before artifact validation: {result:?}"
            );
            let batch = emit_paired_jvm_tasty_batch(
                source,
                "OrderedLocalUnitCalls.scala",
                SourceId::from_index(source_id),
            )
            .unwrap_or_else(|error| panic!("supported source variant {name} emits: {error:?}"));
            assert_eq!(batch.classes().len(), 3, "variant {name} emits all classes");
        } else {
            assert!(
                matches!(
                    result,
                    Err(PairedPlanError::BatchUnit {
                        phase: "cross-class constructor-call identity proof",
                        ..
                    })
                ),
                "malformed ordered call controls fail their source/typed proof: {result:?}"
            );
        }
    }
    for (name, source, source_id) in [
        ("wrong-arity", wrong_arity.as_str(), 21876),
        ("wrong-type", wrong_type.as_str(), 21877),
    ] {
        let result = paired_result(source, source_id);
        println!("ordered two-call invalid-type control {name}: {result:?}");
        assert!(matches!(
            result,
            Err(PairedPlanError::BatchClass {
                phase: "method body typing",
                ref reason,
                ..
            }) if (name == "wrong-arity" && reason.contains("ApplicationArityMismatch"))
                || (name == "wrong-type" && reason.contains("ApplicationArgumentTypeMismatch"))
        ));
    }
}

#[test]
fn ordered_two_call_artifact_contract_rejects_forged_slots_targets_and_links() {
    let batch = emit_paired_jvm_tasty_batch(
        TWO_CALL_TARGET,
        "OrderedLocalUnitCalls.scala",
        SourceId::from_index(21920),
    )
    .expect("the ordered source proof emits the six-artifact batch");
    let encoded = JvmTastyBatchArtifacts {
        classfiles: batch
            .classes()
            .iter()
            .map(|artifact| JvmTastyClassFileArtifact {
                classfile_path: artifact.classfile_path().to_owned(),
                classfile_bytes: artifact.classfile_bytes().to_vec(),
            })
            .collect(),
        sidecars: batch
            .classes()
            .iter()
            .map(|artifact| JvmTastySidecarArtifact {
                tasty_path: artifact.tasty_path().to_owned(),
                tasty_bytes: artifact.tasty_bytes().to_vec(),
            })
            .collect(),
    };
    let factory_index = encoded
        .classfiles
        .iter()
        .position(|artifact| artifact.classfile_path == "example/PairFactory.class")
        .unwrap();
    let class = decode_class(&encoded.classfiles[factory_index].classfile_bytes);
    let create = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == "create")
        .unwrap();
    let [Attribute::Code(code)] = create.attributes.as_slice() else {
        panic!("PairFactory.create has one Code attribute");
    };
    let invocations = code
        .code
        .windows(3)
        .enumerate()
        .filter_map(|(pc, instruction)| {
            if instruction[0] != 0xb6 {
                return None;
            }
            let index = ConstantPoolIndex(u16::from_be_bytes([instruction[1], instruction[2]]));
            let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Methodref {
                class_index,
                name_and_type_index,
            }) = class.constant_pool.get(index)
            else {
                return None;
            };
            let owner = class.constant_pool.class_name(*class_index).ok()?;
            let (name, descriptor) = class
                .constant_pool
                .name_and_type(*name_and_type_index)
                .ok()?;
            (owner == "example/Counter" && name == "set" && descriptor == "(I)V").then_some(pc)
        })
        .collect::<Vec<_>>();
    assert_eq!(invocations.len(), 2);
    let code_offset = code.code.as_ptr() as usize
        - encoded.classfiles[factory_index].classfile_bytes.as_ptr() as usize;

    fn preceding_load(code: &[u8], end: usize, reference: bool) -> (usize, usize, u8) {
        let (compact_first, generic) = if reference {
            (0x2a, 0x19)
        } else {
            (0x1a, 0x15)
        };
        let last = code[end - 1];
        if (compact_first..=compact_first + 3).contains(&last) {
            return (end - 1, 1, last - compact_first);
        }
        assert!(
            end >= 2 && code[end - 2] == generic,
            "generic local load before {end}: {:02x?}",
            &code[end.saturating_sub(4)..end]
        );
        (end - 2, 2, code[end - 1])
    }
    fn replace_load(code: &mut [u8], start: usize, len: usize, slot: u8) {
        if len == 1 {
            assert!(slot <= 3, "compact local slot remains encodable");
            let compact_first = if (0x2a..=0x2d).contains(&code[start]) {
                0x2a
            } else {
                0x1a
            };
            code[start] = compact_first + slot;
        } else {
            code[start + 1] = slot;
        }
    }

    with_paired_jvm_tasty_batch_plan(
        TWO_CALL_TARGET,
        "OrderedLocalUnitCalls.scala",
        SourceId::from_index(21920),
        |plan| {
            let tasty_plans = plan
                .classes
                .iter()
                .map(|class| class.tasty_plan)
                .collect::<Vec<_>>();
            let validated = validate_jvm_tasty_batch(plan.jvm_unit, &tasty_plans).unwrap();
            validate_jvm_tasty_batch_artifacts(&validated, encoded.clone()).unwrap();

            let check_invocation_rejection =
                |mut corrupted: JvmTastyBatchArtifacts, label: &str| {
                    let error = validate_jvm_tasty_batch_artifacts(&validated, corrupted.clone())
                        .expect_err("corrupted pair invocation is rejected before acceptance");
                    assert!(
                        matches!(
                            error,
                            JvmTastyContractError::BatchArtifact {
                                class_index: 2,
                                phase: "cross-class invocation",
                                ..
                            }
                        ),
                        "{label} reaches the pair invocation contract: {error:?}"
                    );
                    corrupted.classfiles.clear();
                };

            let mut wrong_receiver = encoded.clone();
            let second_pc = invocations[1];
            let (second_argument_start, _, _) = preceding_load(code.code, second_pc, false);
            let (first_argument_start, _, _) = preceding_load(code.code, invocations[0], false);
            let (_, _, first_receiver_slot) = preceding_load(code.code, first_argument_start, true);
            let (second_receiver_start, second_receiver_len, _) =
                preceding_load(code.code, second_argument_start, true);
            replace_load(
                &mut wrong_receiver.classfiles[factory_index].classfile_bytes
                    [code_offset..code_offset + code.code.len()],
                second_receiver_start,
                second_receiver_len,
                first_receiver_slot,
            );
            check_invocation_rejection(wrong_receiver, "forged receiver slot");

            let mut wrong_seed = encoded.clone();
            let (first_seed_start, first_seed_len, first_seed_slot) =
                preceding_load(code.code, invocations[0], false);
            let (second_seed_start, second_seed_len, _) =
                preceding_load(code.code, invocations[1], false);
            replace_load(
                &mut wrong_seed.classfiles[factory_index].classfile_bytes
                    [code_offset..code_offset + code.code.len()],
                first_seed_start,
                first_seed_len,
                1, // firstSeed's canonical parameter slot in (II)
            );
            assert_ne!(first_seed_slot, 1, "the first call reads secondSeed");
            let _ = second_seed_start;
            let _ = second_seed_len;
            check_invocation_rejection(wrong_seed, "forged seed slot");

            let mut wrong_opcode = encoded.clone();
            wrong_opcode.classfiles[factory_index].classfile_bytes[code_offset + invocations[0]] =
                0xb8;
            check_invocation_rejection(wrong_opcode, "wrong invocation opcode");

            let counter_init_ref = (1..256)
                .map(ConstantPoolIndex)
                .find(|index| {
                    matches!(
                        class.constant_pool.get(*index),
                        Some(dotty_classfile::constant_pool::ConstantPoolEntry::Methodref {
                            class_index,
                            name_and_type_index,
                        }) if class.constant_pool.class_name(*class_index) == Ok("example/Counter")
                            && class.constant_pool.name_and_type(*name_and_type_index)
                                == Ok(("<init>", "(I)V"))
                    )
                })
                .unwrap();
            let mut wrong_methodref = encoded.clone();
            wrong_methodref.classfiles[factory_index].classfile_bytes
                [code_offset + invocations[1] + 1..code_offset + invocations[1] + 3]
                .copy_from_slice(&counter_init_ref.0.to_be_bytes());
            check_invocation_rejection(wrong_methodref, "wrong Methodref");

            let mut missing_call = encoded.clone();
            missing_call.classfiles[factory_index].classfile_bytes[code_offset + invocations[1]] =
                0x00;
            check_invocation_rejection(missing_call, "missing second call");

            let swapped_calls = TWO_CALL_TARGET.replace(
                "    first.set(secondSeed)\n    second.set(firstSeed)",
                "    second.set(firstSeed)\n    first.set(secondSeed)",
            );
            let swapped = emit_paired_jvm_tasty_batch(
                &swapped_calls,
                "OrderedLocalUnitCalls.scala",
                SourceId::from_index(21921),
            )
            .unwrap();
            let swapped_tasty = swapped
                .classes()
                .iter()
                .find(|artifact| artifact.tasty_path() == "example/PairFactory.tasty")
                .unwrap()
                .tasty_bytes();
            let mut wrong_tasty = encoded.clone();
            let sidecar = wrong_tasty
                .sidecars
                .iter_mut()
                .find(|artifact| artifact.tasty_path == "example/PairFactory.tasty")
                .unwrap();
            let swapped_file = TastyFile::parse_and_validate_scala_3_9(swapped_tasty).unwrap();
            let expected_uuid = TastyFile::parse_and_validate_scala_3_9(&sidecar.tasty_bytes)
                .unwrap()
                .header()
                .uuid;
            let swapped_uuid = swapped_file.header().uuid;
            let uuid_at = swapped_tasty
                .windows(swapped_uuid.len())
                .position(|window| window == swapped_uuid)
                .unwrap();
            sidecar.tasty_bytes = swapped_tasty.to_vec();
            sidecar.tasty_bytes[uuid_at..uuid_at + expected_uuid.len()]
                .copy_from_slice(&expected_uuid);
            assert_eq!(
                TastyFile::parse_and_validate_scala_3_9(&sidecar.tasty_bytes)
                    .unwrap()
                    .header()
                    .uuid,
                expected_uuid
            );
            assert!(validate_jvm_tasty_batch_artifacts(&validated, wrong_tasty.clone()).is_err());

            let mut wrong_uuid = encoded.clone();
            let factory_bytes = &mut wrong_uuid.classfiles[factory_index].classfile_bytes;
            let tasty_uuid = TastyFile::parse_and_validate_scala_3_9(
                &wrong_tasty.sidecars[factory_index].tasty_bytes,
            )
            .unwrap()
            .header()
            .uuid;
            let link = factory_bytes
                .windows(tasty_uuid.len())
                .position(|window| window == tasty_uuid)
                .unwrap();
            factory_bytes[link] ^= 0x01;
            assert!(validate_jvm_tasty_batch_artifacts(&validated, wrong_uuid).is_err());
        },
    )
    .expect("the paired plan validates source/typed identities before mutations");
}

#[test]
fn ordered_two_call_batch_publication_is_atomic_across_six_writes() {
    let batch = emit_paired_jvm_tasty_batch(
        TWO_CALL_TARGET,
        "OrderedLocalUnitCalls.scala",
        SourceId::from_index(21913),
    )
    .expect("the authenticated ordered-call source emits six artifacts");
    let stage = std::env::temp_dir().join(format!(
        "dotty-rs-ordered-two-call-publisher-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&stage).unwrap();
    let destination = stage.join("published");
    let published = batch_publisher::publish_paired_batch(&destination, &batch, None)
        .expect("the complete six-write batch publishes");
    assert_eq!(published.len(), 6);
    let expected = [
        "example/Counter.class",
        "example/Counter.tasty",
        "example/Pair.class",
        "example/Pair.tasty",
        "example/PairFactory.class",
        "example/PairFactory.tasty",
    ];
    let mut actual = published
        .iter()
        .map(|path| {
            path.strip_prefix(&destination)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect::<Vec<_>>();
    actual.sort();
    assert_eq!(actual, expected);

    for (index, fault) in (1..=6)
        .map(batch_publisher::BatchPublishFault::AfterStagedFiles)
        .chain([
            batch_publisher::BatchPublishFault::BeforePromotion,
            batch_publisher::BatchPublishFault::AfterPromotion,
        ])
        .enumerate()
    {
        let failed_destination = stage.join(format!("failed-{index}"));
        assert!(
            batch_publisher::publish_paired_batch(&failed_destination, &batch, Some(fault))
                .is_err()
        );
        assert!(
            !failed_destination.exists(),
            "failure {index} leaves no published destination"
        );
    }

    let sentinel_destination = stage.join("sentinel");
    std::fs::create_dir(&sentinel_destination).unwrap();
    let sentinel = sentinel_destination.join("sentinel.txt");
    std::fs::write(&sentinel, b"preserve existing destination").unwrap();
    assert!(batch_publisher::publish_paired_batch(&sentinel_destination, &batch, None).is_err());
    assert_eq!(
        std::fs::read(&sentinel).unwrap(),
        b"preserve existing destination"
    );
    std::fs::remove_dir_all(stage).unwrap();
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
    let wrong_formal = TARGET.replace("first.set(secondSeed)", "first.set(firstSeed)");
    let moved_call = TARGET.replace(
        "    first.set(secondSeed)\n    new Pair(first, second)",
        "    val result: Pair = new Pair(first, second)\n    first.set(secondSeed)\n    result",
    );

    for (name, source, id) in [
        ("target", TARGET, 21642),
        ("wrong-arity", wrong_arity.as_str(), 21643),
        ("wrong-type", wrong_type.as_str(), 21644),
        ("second-call", second_call.as_str(), 21647),
        ("wrong-formal", wrong_formal.as_str(), 21648),
        ("moved-call", moved_call.as_str(), 21651),
    ] {
        let result = paired_result(source, id);
        println!("control {name}: {result:?}");
        match name {
            "target" => assert!(result.is_ok(), "accepted bounded case: {result:?}"),
            "second-call" => assert!(result.is_ok(), "accepted ordered pair: {result:?}"),
            "wrong-arity" => assert!(
                matches!(result, Err(PairedPlanError::BatchClass { phase: "method body typing", reason, .. }) if reason.contains("ApplicationArityMismatch"))
            ),
            "wrong-type" => assert!(
                matches!(result, Err(PairedPlanError::BatchClass { phase: "method body typing", reason, .. }) if reason.contains("ApplicationArgumentTypeMismatch"))
            ),
            "wrong-formal" => assert!(matches!(
                result,
                Err(PairedPlanError::BatchUnit {
                    phase: "cross-class constructor-call identity proof",
                    reason
                }) if reason.contains("SourceShape")
            )),
            "moved-call" => assert!(matches!(
                result,
                Err(PairedPlanError::BatchClass {
                    phase: "JVM/TASTy cross-contract validation",
                    ..
                })
            )),
            _ => assert!(matches!(
                result,
                Err(PairedPlanError::BatchUnit {
                    phase: "cross-class constructor-call identity proof",
                    ..
                })
            )),
        }
    }

    assert!(matches!(
        emit_paired_jvm_tasty_batch(
            receiver_second.as_str(),
            "LocalReceiverCall.scala",
            SourceId::from_index(21645)
        ),
        Err(dotty::jvm_tasty_artifacts::PairedBatchArtifactError::Contract(_))
    ));
    assert!(
        emit_paired_jvm_tasty_batch(
            second_call.as_str(),
            "LocalReceiverCall.scala",
            SourceId::from_index(21647)
        )
        .is_ok()
    );
    assert!(paired_result(source_order.as_str(), 21646).is_ok());
}

#[test]
fn local_receiver_slot_tracks_the_authenticated_symbol_after_declaration_reversal() {
    let reversed = TARGET.replace(
        "    val first: Counter = new Counter(firstSeed)\n    val second: Counter = new Counter(secondSeed)",
        "    val second: Counter = new Counter(secondSeed)\n    val first: Counter = new Counter(firstSeed)",
    );
    let source_id = SourceId::from_index(21652);
    let batch = emit_paired_jvm_tasty_batch(&reversed, "LocalReceiverCall.scala", source_id)
        .expect("reversed local declarations preserve the authenticated Counter.set call");
    let repeated = emit_paired_jvm_tasty_batch(&reversed, "LocalReceiverCall.scala", source_id)
        .expect("an independent session emits the reversed batch deterministically");
    assert_eq!(batch.classes(), repeated.classes());
    assert_eq!(batch.classes().len(), 3);

    let classfiles = batch
        .classes()
        .iter()
        .map(|artifact| JvmTastyClassFileArtifact {
            classfile_path: artifact.classfile_path().to_owned(),
            classfile_bytes: artifact.classfile_bytes().to_vec(),
        })
        .collect::<Vec<_>>();
    let sidecars = batch
        .classes()
        .iter()
        .map(|artifact| JvmTastySidecarArtifact {
            tasty_path: artifact.tasty_path().to_owned(),
            tasty_bytes: artifact.tasty_bytes().to_vec(),
        })
        .collect::<Vec<_>>();
    assert_eq!(classfiles.len() + sidecars.len(), 6);
    for (classfile, sidecar) in classfiles.iter().zip(&sidecars) {
        let class = decode_class(&classfile.classfile_bytes);
        let tasty = TastyFile::parse_and_validate_scala_3_9(&sidecar.tasty_bytes).unwrap();
        assert_eq!(
            class.tasty_link_id().unwrap().unwrap().as_bytes(),
            &tasty.header().uuid,
            "each reversed classfile remains linked to its TASTy sidecar"
        );
    }

    with_paired_jvm_tasty_batch_plan(
        &reversed,
        "LocalReceiverCall.scala",
        source_id,
        |plan| {
            let [proof] = plan.constructor_call_proofs else {
                panic!("the reversed source keeps one canonical constructor proof");
            };
            let call = proof
                .local_receiver_unit_call()
                .expect("the proof authenticates the original local receiver");
            assert_eq!(
                plan.jvm_unit
                    .store
                    .names
                    .resolve(plan.jvm_unit.store.symbols.get(call.receiver).name.text()),
                "first",
                "the reversed declaration source still authenticates first as receiver"
            );

            let tasty_plans = plan
                .classes
                .iter()
                .map(|class| class.tasty_plan)
                .collect::<Vec<_>>();
            let validated = validate_jvm_tasty_batch(plan.jvm_unit, &tasty_plans).unwrap();
            let artifacts = JvmTastyBatchArtifacts {
                classfiles: classfiles.clone(),
                sidecars: sidecars.clone(),
            };
            validate_jvm_tasty_batch_artifacts(&validated, artifacts.clone())
                .expect("the six linked reversed artifacts satisfy the canonical contract");

            let factory = classfiles
                .iter()
                .find(|artifact| artifact.classfile_path == "example/PairFactory.class")
                .unwrap();
            let class = decode_class(&factory.classfile_bytes);
            let create = class
                .methods
                .iter()
                .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == "create")
                .unwrap();
            let [Attribute::Code(code)] = create.attributes.as_slice() else {
                panic!("PairFactory.create has one Code attribute");
            };
            let invocation_pc = code
                .code
                .windows(3)
                .position(|window| window[0] == 0xb6)
                .expect("the call uses invokevirtual Counter.set");
            assert_eq!(
                &code.code[invocation_pc - 3..invocation_pc],
                &[0x19, 0x04, 0x1c],
                "the authenticated first local occupies slot 4 after declaration reversal"
            );

            // Slot 3 is the other initialized Counter and produces JVM-valid bytecode,
            // but it no longer implements the source receiver identity in the receipt.
            let mut wrong_receiver = artifacts;
            let factory = wrong_receiver
                .classfiles
                .iter_mut()
                .find(|artifact| artifact.classfile_path == "example/PairFactory.class")
                .unwrap();
            let mut reader = ClassReader::new(&factory.classfile_bytes);
            let decoded = ClassFile::decode(&mut reader).unwrap();
            let create = decoded
                .methods
                .iter()
                .find(|method| decoded.constant_pool.utf8(method.name_index).unwrap() == "create")
                .unwrap();
            let [Attribute::Code(code)] = create.attributes.as_slice() else {
                panic!("PairFactory.create has one Code attribute");
            };
            let invocation_pc = code
                .code
                .windows(3)
                .position(|window| window[0] == 0xb6)
                .unwrap();
            let code_start = code.code.as_ptr() as usize - factory.classfile_bytes.as_ptr() as usize;
            factory.classfile_bytes[code_start + invocation_pc - 3..code_start + invocation_pc]
                .copy_from_slice(&[0x2d, 0x00, 0x1c]);
            let wrong_factory_bytes = factory.classfile_bytes.clone();
            let error = validate_jvm_tasty_batch_artifacts(&validated, wrong_receiver)
                .expect_err("the other local cannot replace the authenticated receiver slot");
            assert!(
                matches!(
                    error,
                    JvmTastyContractError::BatchArtifact {
                        class_index: 2,
                        phase: "cross-class invocation",
                        ..
                    }
                ),
                "a valid load of the wrong local must fail semantic slot authentication, not verification: {error:?}"
            );

            if require_jdk_21_or_skip() {
                let root = std::env::temp_dir().join(format!(
                    "dotty-counter-set-reversed-{}",
                    std::process::id()
                ));
                let _ = std::fs::remove_dir_all(&root);
                let original_classes = root.join("original");
                let wrong_classes = root.join("wrong-receiver");
                let harness_classes = root.join("harness");
                for directory in [&original_classes, &wrong_classes, &harness_classes] {
                    std::fs::create_dir_all(directory).unwrap();
                }
                for artifact in &classfiles {
                    let original_path = original_classes.join(&artifact.classfile_path);
                    let wrong_path = wrong_classes.join(&artifact.classfile_path);
                    std::fs::create_dir_all(original_path.parent().unwrap()).unwrap();
                    std::fs::create_dir_all(wrong_path.parent().unwrap()).unwrap();
                    std::fs::write(&original_path, &artifact.classfile_bytes).unwrap();
                    std::fs::write(
                        &wrong_path,
                        if artifact.classfile_path == "example/PairFactory.class" {
                            &wrong_factory_bytes
                        } else {
                            &artifact.classfile_bytes
                        },
                    )
                    .unwrap();
                }
                let harness = root.join("CounterSetReceiverHarness.java");
                std::fs::write(
                    &harness,
                    r#"import example.Pair;
import example.PairFactory;
public final class CounterSetReceiverHarness {
  public static void main(String[] args) {
    Pair pair = new PairFactory().create(7, 3);
    if (pair.leftValue() != 3 || pair.rightValue() != 3)
      throw new AssertionError("first receiver mutation did not reach Pair.first");
    System.out.println("counter-set=3/3");
  }
}"#,
                )
                .unwrap();
                let javac = Command::new("javac")
                    .args(["--release", "21", "-classpath"])
                    .arg(&original_classes)
                    .arg("-d")
                    .arg(&harness_classes)
                    .arg(&harness)
                    .output()
                    .expect("start JDK 21 javac for the reversed receiver harness");
                assert!(
                    javac.status.success(),
                    "javac rejected the reversed receiver harness: {}",
                    String::from_utf8_lossy(&javac.stderr)
                );
                let run = |classes: &std::path::Path| {
                    let classpath = std::env::join_paths([&harness_classes, classes]).unwrap();
                    Command::new("java")
                        .args(["-Xverify:all", "-ea", "-classpath"])
                        .arg(classpath)
                        .arg("CounterSetReceiverHarness")
                        .output()
                        .expect("start JDK 21 with strict bytecode verification")
                };
                let valid = run(&original_classes);
                assert!(
                    valid.status.success(),
                    "JDK rejected the reversed six-artifact class files:\n{}\n{}",
                    String::from_utf8_lossy(&valid.stdout),
                    String::from_utf8_lossy(&valid.stderr)
                );
                assert_eq!(String::from_utf8_lossy(&valid.stdout).trim(), "counter-set=3/3");
                let wrong_target = run(&wrong_classes);
                let wrong_output = format!(
                    "{}{}",
                    String::from_utf8_lossy(&wrong_target.stdout),
                    String::from_utf8_lossy(&wrong_target.stderr)
                );
                assert!(
                    !wrong_target.status.success()
                        && wrong_output.contains("AssertionError: first receiver mutation"),
                    "the wrong local remains verifier-valid and fails the semantic receiver assertion, not verification:\n{wrong_output}"
                );
                std::fs::remove_dir_all(&root).unwrap();
            }
        },
    )
    .expect("reversed declaration slot mapping remains inside the paired contract");
}

#[test]
fn local_receiver_unit_call_emits_one_linked_six_artifact_batch() {
    let batch = emit_paired_jvm_tasty_batch(
        TARGET,
        "LocalReceiverCall.scala",
        SourceId::from_index(21649),
    )
    .expect("the source session emits the complete paired batch");
    let repeated = emit_paired_jvm_tasty_batch(
        TARGET,
        "LocalReceiverCall.scala",
        SourceId::from_index(21649),
    )
    .expect("an independent source session emits the complete paired batch");
    assert_eq!(batch.classes(), repeated.classes());
    assert_eq!(batch.classes().len(), 3);
    let expected = [
        ("example/Counter.class", "example/Counter.tasty"),
        ("example/Pair.class", "example/Pair.tasty"),
        ("example/PairFactory.class", "example/PairFactory.tasty"),
    ];
    for (artifacts, (class_path, tasty_path)) in batch.classes().iter().zip(expected) {
        assert_eq!(artifacts.classfile_path(), class_path);
        assert_eq!(artifacts.tasty_path(), tasty_path);
        let class = decode_class(artifacts.classfile_bytes());
        let tasty = TastyFile::parse_and_validate_scala_3_9(artifacts.tasty_bytes()).unwrap();
        assert_eq!(class.version.major, 65);
        assert_eq!(
            (tasty.header().major_version, tasty.header().minor_version),
            (28, 9)
        );
        assert_eq!(
            class.tasty_link_id().unwrap().unwrap().as_bytes(),
            &tasty.header().uuid
        );
        if class_path.ends_with("PairFactory.class") {
            let index = tasty.ast_address_index().unwrap();
            let (_, method) = index
                .iter_nodes_with_tag(DEFDEF_TAG)
                .find_map(|node| {
                    let body = index
                        .get(node.offset as u32)
                        .unwrap()
                        .decode_defdef_body()
                        .unwrap();
                    (tasty.names().render(body.name).unwrap() == "create")
                        .then_some((node.offset as u32, body))
                })
                .unwrap();
            let RawTree::LengthNode(block) = method.rhs.unwrap() else {
                panic!("Rust TASTy records create's source BLOCK");
            };
            let block = block.decode_block().unwrap();
            assert_eq!(block.stats.len(), 3);
            assert_eq!(
                block.stats.iter().map(raw_tag).collect::<Vec<_>>(),
                [VALDEF_TAG, VALDEF_TAG, APPLY_TAG]
            );
            assert_eq!(raw_tag(&block.expression), APPLY_TAG);
        }
    }
    let encoded_batch = JvmTastyBatchArtifacts {
        classfiles: batch
            .classes()
            .iter()
            .map(|artifact| JvmTastyClassFileArtifact {
                classfile_path: artifact.classfile_path().to_owned(),
                classfile_bytes: artifact.classfile_bytes().to_vec(),
            })
            .collect(),
        sidecars: batch
            .classes()
            .iter()
            .map(|artifact| JvmTastySidecarArtifact {
                tasty_path: artifact.tasty_path().to_owned(),
                tasty_bytes: artifact.tasty_bytes().to_vec(),
            })
            .collect(),
    };
    with_paired_jvm_tasty_batch_plan(
        TARGET,
        "LocalReceiverCall.scala",
        SourceId::from_index(21649),
        |plan| {
            let tasty_plans = plan
                .classes
                .iter()
                .map(|class| class.tasty_plan)
                .collect::<Vec<_>>();
            let validated = validate_jvm_tasty_batch(plan.jvm_unit, &tasty_plans).unwrap();
            let factory_index = encoded_batch
                .classfiles
                .iter()
                .position(|artifact| artifact.classfile_path == "example/PairFactory.class")
                .unwrap();
            let factory = &encoded_batch.classfiles[factory_index];
            let class = decode_class(&factory.classfile_bytes);
            let create = class
                .methods
                .iter()
                .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == "create")
                .unwrap();
            let [Attribute::Code(code)] = create.attributes.as_slice() else {
                panic!("PairFactory.create has one Code attribute");
            };
            let invocation_pc = code
                .code
                .windows(3)
                .position(|window| window[0] == 0xb6)
                .expect("the ordinary emitter writes one invokevirtual");
            let code_offset =
                code.code.as_ptr() as usize - factory.classfile_bytes.as_ptr() as usize;
            let mut wrong_methodref = None;
            for raw_index in 1..256 {
                let index = ConstantPoolIndex(raw_index);
                let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Methodref {
                    class_index,
                    name_and_type_index,
                }) = class.constant_pool.get(index)
                else {
                    continue;
                };
                let owner = class.constant_pool.class_name(*class_index).unwrap();
                let (name, descriptor) = class
                    .constant_pool
                    .name_and_type(*name_and_type_index)
                    .unwrap();
                if owner == "example/Counter" && name == "<init>" && descriptor == "(I)V" {
                    wrong_methodref = Some(raw_index);
                    break;
                }
            }
            let wrong_methodref = wrong_methodref.expect("Counter constructor Methodref control");
            let mut corruptions = Vec::new();
            for (label, edit) in [
                ("receiver slot", 0),
                ("formal slot", 1),
                ("static invocation", 2),
                ("wrong selected method", 3),
                ("missing invocation", 4),
            ] {
                let mut corrupted = encoded_batch.clone();
                let bytes = &mut corrupted.classfiles[factory_index].classfile_bytes;
                match edit {
                    0 => bytes[code_offset + invocation_pc - 2] = 0x2c,
                    1 => bytes[code_offset + invocation_pc - 1] = 0x1b,
                    2 => bytes[code_offset + invocation_pc] = 0xb8,
                    3 => bytes[code_offset + invocation_pc + 1..code_offset + invocation_pc + 3]
                        .copy_from_slice(&wrong_methodref.to_be_bytes()),
                    4 => bytes[code_offset + invocation_pc - 2] = 0x00,
                    _ => unreachable!(),
                }
                let error = validate_jvm_tasty_batch_artifacts(&validated, corrupted).unwrap_err();
                assert!(
                    matches!(
                        error,
                        JvmTastyContractError::BatchArtifact {
                            class_index: 2,
                            phase: "cross-class invocation",
                            ..
                        }
                    ),
                    "{label} corruption must fail at the bounded invocation contract: {error:?}"
                );
                corruptions.push(label);
            }
            assert_eq!(corruptions.len(), 5);
        },
    )
    .expect("fresh proof session validates the same six-artifact contract");
    let first = batch.classes().last().unwrap();
    let class = decode_class(first.classfile_bytes());
    let create = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == "create")
        .unwrap();
    let [Attribute::Code(code)] = create.attributes.as_slice() else {
        panic!("PairFactory.create has one Code attribute");
    };
    assert_eq!((code.max_stack, code.max_locals), (4, 5));
    let code = code.code;
    let mut pc = 0;
    fn opcode(code: &[u8], pc: &mut usize, expected: u8) {
        assert_eq!(code[*pc], expected, "opcode at actual offset {}", *pc);
        *pc += 1;
    }
    fn operand(code: &[u8], pc: &mut usize, expected: u8) -> u16 {
        opcode(code, pc, expected);
        let value = u16::from_be_bytes([code[*pc], code[*pc + 1]]);
        *pc += 2;
        value
    }
    let method_ref = |index| {
        let Some(dotty_classfile::constant_pool::ConstantPoolEntry::Methodref {
            class_index,
            name_and_type_index,
        }) = class.constant_pool.get(index)
        else {
            panic!("invocation uses CONSTANT_Methodref");
        };
        let owner = class.constant_pool.class_name(*class_index).unwrap();
        let (name, descriptor) = class
            .constant_pool
            .name_and_type(*name_and_type_index)
            .unwrap();
        (owner.to_owned(), name.to_owned(), descriptor.to_owned())
    };
    operand(code, &mut pc, 0xbb);
    opcode(code, &mut pc, 0x59);
    opcode(code, &mut pc, 0x1b);
    assert_eq!(
        method_ref(ConstantPoolIndex(operand(code, &mut pc, 0xb7))),
        ("example/Counter".into(), "<init>".into(), "(I)V".into())
    );
    opcode(code, &mut pc, 0x4e);
    operand(code, &mut pc, 0xbb);
    opcode(code, &mut pc, 0x59);
    opcode(code, &mut pc, 0x1c);
    assert_eq!(
        method_ref(ConstantPoolIndex(operand(code, &mut pc, 0xb7))),
        ("example/Counter".into(), "<init>".into(), "(I)V".into())
    );
    opcode(code, &mut pc, 0x3a);
    assert_eq!(code[pc], 4, "second local store uses slot 4");
    pc += 1;
    opcode(code, &mut pc, 0x2d);
    opcode(code, &mut pc, 0x1c);
    assert_eq!(
        method_ref(ConstantPoolIndex(operand(code, &mut pc, 0xb6))),
        ("example/Counter".into(), "set".into(), "(I)V".into())
    );
    operand(code, &mut pc, 0xbb);
    opcode(code, &mut pc, 0x59);
    opcode(code, &mut pc, 0x2d);
    opcode(code, &mut pc, 0x19);
    assert_eq!(code[pc], 4, "second Pair argument uses slot 4");
    pc += 1;
    assert_eq!(
        method_ref(ConstantPoolIndex(operand(code, &mut pc, 0xb7))),
        (
            "example/Pair".into(),
            "<init>".into(),
            "(Lexample/Counter;Lexample/Counter;)V".into()
        )
    );
    opcode(code, &mut pc, 0xb0);
    assert_eq!(
        pc,
        code.len(),
        "the final constructor call precedes areturn"
    );

    let renamed = TARGET
        .replace(
            "def create(firstSeed: Int, secondSeed: Int): Pair =",
            "def create(leftSeed: Int, rightSeed: Int): Pair =",
        )
        .replace(
            "val first: Counter = new Counter(firstSeed)",
            "val leftCounter: Counter = new Counter(leftSeed)",
        )
        .replace(
            "val second: Counter = new Counter(secondSeed)",
            "val rightCounter: Counter = new Counter(rightSeed)",
        )
        .replace("first.set(secondSeed)", "leftCounter.set(rightSeed)")
        .replace(
            "new Pair(first, second)",
            "new Pair(leftCounter, rightCounter)",
        );
    let renamed_a = emit_paired_jvm_tasty_batch(
        &renamed,
        "RenamedLocalReceiverCall.scala",
        SourceId::from_index(21650),
    )
    .expect("renaming declarations preserves the bounded source contract");
    let renamed_b = emit_paired_jvm_tasty_batch(
        &renamed,
        "RenamedLocalReceiverCall.scala",
        SourceId::from_index(21650),
    )
    .expect("a fresh renamed source session is deterministic");
    assert_eq!(renamed_a.classes(), renamed_b.classes());

    let original = emit_paired_jvm_tasty_batch(
        TWO_LOCAL_SOURCE,
        "LocalReceiverCall.scala",
        SourceId::from_index(21280),
    )
    .expect("the existing no-call two-local batch remains supported");
    for name in ["Counter", "Pair"] {
        let with_call = batch
            .classes()
            .iter()
            .find(|class| class.classfile_path().ends_with(&format!("{name}.class")))
            .unwrap();
        let without_call = original
            .classes()
            .iter()
            .find(|class| class.classfile_path().ends_with(&format!("{name}.class")))
            .unwrap();
        assert_eq!(with_call.classfile_bytes(), without_call.classfile_bytes());
        assert_eq!(with_call.tasty_bytes(), without_call.tasty_bytes());
    }
}
