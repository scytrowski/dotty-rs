use dotty_core::ast::*;
use dotty_core::*;
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_tasty_pickler::*;
use dotty_typer::SourceTyper;
use std::path::PathBuf;
use std::process::Command;

const ANSWER: &str = include_str!("../../../tests/fixtures/tasty_pickler/Answer.scala");
const BOX_WITH_METHOD: &str = include_str!("fixtures/Box.scala");
const MULTI: &str = include_str!("../../../tests/fixtures/tasty_pickler/Multi.scala");
const PARAMETERS: &str = include_str!("../../../tests/fixtures/tasty_pickler/Parameters.scala");
const PARAMETER_DECL: &str =
    include_str!("../../../tests/fixtures/tasty_pickler/ParameterDecl.scala");
const REFERENCES: &str = include_str!("../../../tests/fixtures/tasty_pickler/References.scala");
const EXPLICIT_THIS_REFERENCES: &str =
    "package example\nclass References:\n  def first: Int = this.second\n  def second: Int = 42\n";
const OPERATIONS: &str = "package example\nclass Operations:\n  def use: Int = identity(42)\n  def identity(x: Int): Int = x\n  def propagate(x: Int): Int = identity(x)\n  def explicit: Int = this.identity(42)\n  def pick(first: Int, second: Int): Int = second\n  def picks: Int = pick(11, 42)\n  def nested: Int = identity(identity(42))\n";
const MUTABLE: &str = include_str!("fixtures/Mutable.scala");
const LOOP: &str = include_str!("fixtures/Loop.scala");
const ARITHMETIC: &str = "package example\nclass Arithmetic:\n  def add(x: Int, y: Int): Int = x + y\n  def subtract(x: Int, y: Int): Int = x - y\n  def multiply(x: Int, y: Int): Int = x * y\n  def divide(x: Int, y: Int): Int = x / y\n  def remainder(x: Int, y: Int): Int = x % y\n  def nested(x: Int, y: Int, z: Int): Int = x + y * z\n  def throughCall(x: Int, y: Int): Int = identity(x + y)\n  def local(x: Int, y: Int): Int =\n    val result: Int = x + y\n    result\n  def identity(value: Int): Int = value\n  def compute(a: Int, b: Int): Int = (a + b) * 2 - 1\n";
const PARENTHESIZED_CALL_ARGUMENTS: &str = "package example\nclass ParenthesizedCallArguments:\n  def run(a: Int, b: Int): Int = identity((a + b))\n  def nested(a: Int, b: Int): Int = identity(identity((a + b)))\n  def identity(value: Int): Int = value\n";
const COMPARISONS: &str = "package example\nclass Comparisons:\n  def positive(x: Int): Boolean = x > 0\n  def before(a: Int, b: Int): Boolean = a < b\n  def atMost(a: Int, b: Int): Boolean = a <= b\n  def atLeast(a: Int, b: Int): Boolean = a >= b\n  def nested(a: Int, b: Int): Boolean = (a + 1) >= b\n  def choose(x: Int): Int = if x > 0 then 1 else 0\n";
const FLAGS: &str = "package example\nclass Flags:\n  def enabled: Boolean = true\n  def disabled: Boolean = false\n  def passthrough(flag: Boolean): Boolean = flag\n  def keep(flag: Boolean, value: Int): Int = value\n";
const LOGIC: &str = "package example\nclass Logic:\n  def negate(a: Boolean): Boolean = !a\n  def both(a: Boolean, b: Boolean): Boolean = a && b\n  def either(a: Boolean, b: Boolean): Boolean = a || b\n  def safeAnd: Boolean = false && (1 / 0 > 0)\n  def safeOr: Boolean = true || (1 / 0 > 0)\n  def evaluatedAnd(a: Boolean): Boolean = a && (1 / 0 > 0)\n  def evaluatedOr(a: Boolean): Boolean = a || (1 / 0 > 0)\n";
const CONDITIONALS: &str = include_str!("../../../tests/fixtures/tasty_pickler/Conditionals.scala");
const BLOCKS: &str = include_str!("../../../tests/fixtures/tasty_pickler/Blocks.scala");
const LOCALS: &str = include_str!("fixtures/Locals.scala");
const NUMERIC_BLOCKS: &str = "package example\nclass NumericBlocks:\n  def run: Int =\n    1\n    2\n    3\n    4\n    5\n    6\n    7\n    8\n    9\n    10\n    11\n    12\n    13\n    14\n    15\n    16\n    17\n    18\n    19\n    20\n";
const RUN_SCALA_ORACLE: &str = "DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE";

const ANSWER_PRE_REFACTOR: &str = include_str!("fixtures/pre_refactor/Answer.hex");
const OPERATIONS_PRE_REFACTOR: &str = include_str!("fixtures/pre_refactor/Operations.hex");
const PARAMETERS_PRE_REFACTOR: &str = include_str!("fixtures/pre_refactor/Parameters.hex");

fn bytes_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn supported_fixture_outputs_match_the_pre_refactor_bytes_exactly() {
    // Captured from main at 8738a3a0c before issue #1310's validation refactor.
    // Comparing the full encoding pins header UUID, name order, addresses and body bytes.
    for (name, source, expected) in [
        ("Answer", ANSWER, ANSWER_PRE_REFACTOR),
        ("Operations", OPERATIONS, OPERATIONS_PRE_REFACTOR),
        ("Parameters", PARAMETERS, PARAMETERS_PRE_REFACTOR),
    ] {
        let (_, output) = source_outputs_named(source, &format!("{name}.scala"), |_| {}).unwrap();
        assert_eq!(bytes_hex(output.bytes()), expected.trim(), "{name}");
    }
}

#[test]
fn plain_int_primary_constructor_preserves_both_parameter_identities_and_wire_shape() {
    use dotty_tasty::tasty::*;

    let (skeleton, output) = source_outputs_named(BOX_WITH_METHOD, "Box.scala", |plan| {
        let constructor = plan.constructor.symbol;
        let TreeKind::TypeDef(class) = &plan.source_ast.get(plan.class_tree).kind else {
            panic!("class tree")
        };
        let TreeKind::Template(template) = &plan.source_ast.get(class.rhs).kind else {
            panic!("class template")
        };
        let TreeKind::DefDef(definition) = &plan.source_ast.get(template.constructor).kind else {
            panic!("primary constructor")
        };
        let parameter_tree = definition.value_param_clauses[0][0];
        let template_parameter = plan
            .source_index
            .symbol_at(plan.source.id, parameter_tree)
            .unwrap();
        let constructor_parameter = plan.constructor.parameter_clauses[0][0];
        assert_ne!(template_parameter, constructor_parameter);
        assert_eq!(
            plan.store.symbols.get(template_parameter).owner,
            Some(plan.class)
        );
        assert_eq!(
            plan.store.symbols.get(constructor_parameter).owner,
            Some(constructor)
        );
    })
    .unwrap();
    assert_eq!(
        output.bytes(),
        source_outputs_named(BOX_WITH_METHOD, "Box.scala", |_| {})
            .unwrap()
            .1
            .bytes()
    );
    assert_eq!(skeleton.declarations().len(), 5);
    let [
        class,
        constructor,
        answer,
        template_parameter,
        constructor_parameter,
    ] = skeleton.declarations()
    else {
        panic!("class, constructor, method, template parameter and constructor parameter")
    };
    assert_eq!(template_parameter.owner, class.symbol);
    assert_eq!(constructor_parameter.owner, constructor.symbol);
    assert_eq!(constructor.owner, class.symbol);
    assert_eq!(answer.owner, class.symbol);

    let file = TastyFile::parse_and_validate_scala_3_9(output.bytes()).unwrap();
    let addresses = file.ast_address_index().unwrap();
    let template_parameter_node = addresses.get(template_parameter.address).unwrap();
    assert_eq!(template_parameter_node.tag, PARAM_TAG);
    assert_eq!(
        template_parameter_node
            .decode_parameter()
            .unwrap()
            .decode_body()
            .unwrap()
            .tail,
        [
            DefinitionTail::Modifier(PRIVATE_TAG),
            DefinitionTail::Modifier(LOCAL_TAG),
        ]
    );
    assert_eq!(
        file.render_name(template_parameter_node.decode_parameter().unwrap().name())
            .unwrap(),
        "start"
    );
    let constructor_node = addresses.get(constructor.address).unwrap();
    let constructor = constructor_node.decode_defdef_body().unwrap();
    assert_eq!(constructor.parameters.len(), 1);
    assert_eq!(
        file.render_name(constructor.parameters[0].name()).unwrap(),
        "start"
    );
    assert!(constructor.rhs.is_none());
    assert!(
        constructor
            .tail
            .contains(&DefinitionTail::Modifier(STABLE_TAG))
    );
    let constructor_parameter_node = addresses.get(constructor_parameter.address).unwrap();
    assert_eq!(constructor_parameter_node.tag, PARAM_TAG);
    assert_eq!(
        file.render_name(
            constructor_parameter_node
                .decode_parameter()
                .unwrap()
                .name()
        )
        .unwrap(),
        "start"
    );
    let (_, repeated) = source_outputs_named(BOX_WITH_METHOD, "Box.scala", |_| {}).unwrap();
    assert_eq!(output.bytes(), repeated.bytes());
    if std::env::var_os(RUN_SCALA_ORACLE).is_some() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let candidate = root.join("target/issue-1394/generated/example/Box.tasty");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        std::fs::write(&candidate, output.bytes()).unwrap();
        let temp = root.join("target/issue-1394/tmp");
        std::fs::create_dir_all(&temp).unwrap();
        let result = Command::new("python3")
            .arg(root.join("tools/tasty-pickler-oracle/run.py"))
            .arg("verify")
            .arg(&candidate)
            .arg("--runtime")
            .env("TMPDIR", temp)
            .status()
            .expect("start explicitly requested Scala 3.9 constructor interoperability gate");
        assert!(result.success(), "Box Scala interoperability gate failed");
    }
}

#[test]
fn primary_constructor_val_and_var_accessors_are_rejected() {
    for source in [
        "package example\nclass Box(val start: Int):\n  def answer(): Int = 42\n",
        "package example\nclass Box(var start: Int):\n  def answer(): Int = 42\n",
    ] {
        let result = source_outputs_named(source, "Box.scala", |_| {});
        assert!(
            matches!(
                result,
                Err(PickleError::Unsupported {
                    feature: UnsupportedFeature::PrimaryConstructorInput,
                    ..
                })
            ),
            "accessor parameter unexpectedly accepted: {result:?}"
        );
    }
}

#[test]
fn boolean_literals_parameters_and_mixed_clauses_emit_the_expected_wire_shapes() {
    use dotty_tasty::tasty::*;

    let (_, output) = source_outputs_named(FLAGS, "Flags.scala", |_| {}).unwrap();
    let file = TastyFile::parse_and_validate_scala_3_9(output.bytes()).unwrap();
    let addresses = file.ast_address_index().unwrap();
    let body = |method_index: usize| {
        addresses
            .get(output.declarations()[method_index].address)
            .unwrap()
            .decode_defdef_body()
            .unwrap()
            .rhs
            .unwrap()
    };
    assert!(matches!(
        body(2),
        RawTree::Leaf(SimpleTerm {
            tag: TRUECONST_TAG,
            value: TermValue::Boolean(true),
            ..
        })
    ));
    assert!(matches!(
        body(3),
        RawTree::Leaf(SimpleTerm {
            tag: FALSECONST_TAG,
            value: TermValue::Boolean(false),
            ..
        })
    ));
    assert!(matches!(
        body(4),
        RawTree::Leaf(SimpleTerm {
            tag: TERMREFDIRECT_TAG,
            value: TermValue::AstRef(_),
            ..
        })
    ));
    assert!(matches!(
        body(5),
        RawTree::Leaf(SimpleTerm {
            tag: TERMREFDIRECT_TAG,
            value: TermValue::AstRef(_),
            ..
        })
    ));

    if std::env::var_os(RUN_SCALA_ORACLE).is_some() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let candidate = root.join("target/issue-1311/generated/example/Flags.tasty");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        std::fs::write(&candidate, output.bytes()).unwrap();
        let result = Command::new("python3")
            .arg(root.join("tools/tasty-pickler-oracle/run.py"))
            .arg("verify")
            .arg(&candidate)
            .arg("--runtime")
            .env("TMPDIR", root.join("target/issue-1311/tmp"))
            .status()
            .expect("start explicitly requested Scala 3.9 Boolean interoperability gate");
        assert!(
            result.success(),
            "Boolean Scala interoperability gate failed"
        );
    }
}

#[test]
fn source_conditionals_preserve_typed_if_order_nested_nodes_and_boolean_literal_conditions() {
    use dotty_tasty::tasty::*;

    let (_, output) = source_outputs_named(CONDITIONALS, "Conditionals.scala", |_| {}).unwrap();
    let file = TastyFile::parse_and_validate_scala_3_9(output.bytes()).unwrap();
    let addresses = file.ast_address_index().unwrap();
    let parameter_reference = |tree: &RawTree<'_>| {
        let RawTree::Leaf(reference) = tree else {
            panic!("expected a direct parameter reference")
        };
        assert_eq!(reference.tag, TERMREFDIRECT_TAG);
        let target = addresses.get(reference.ast_ref().unwrap().address).unwrap();
        assert_eq!(target.tag, PARAM_TAG);
        file.render_name(target.decode_parameter().unwrap().name())
            .unwrap()
    };
    let body = |declaration_index: usize| {
        addresses
            .get(output.declarations()[declaration_index].address)
            .unwrap()
            .decode_defdef_body()
            .unwrap()
            .rhs
            .unwrap()
    };
    let RawTree::LengthNode(choose_raw) = body(2) else {
        panic!("choose body is not an IF node")
    };
    let StructuredNode::If(choose) = choose_raw.decode_structured().unwrap() else {
        panic!("choose body did not decode as IF")
    };
    assert!(!choose.inline);
    assert!(matches!(
        choose.condition,
        RawTree::Leaf(SimpleTerm {
            tag: TERMREFDIRECT_TAG,
            value: TermValue::AstRef(_),
            ..
        })
    ));
    assert_eq!(parameter_reference(&choose.condition), "flag");
    for branch in [&choose.then_branch, &choose.else_branch] {
        assert!(matches!(
            branch,
            RawTree::Leaf(SimpleTerm {
                tag: TERMREFDIRECT_TAG,
                value: TermValue::AstRef(_),
                ..
            })
        ));
    }
    assert_eq!(parameter_reference(&choose.then_branch), "a");
    assert_eq!(parameter_reference(&choose.else_branch), "b");

    let RawTree::LengthNode(nested_raw) = body(3) else {
        panic!("nested body is not an IF node")
    };
    let StructuredNode::If(nested) = nested_raw.decode_structured().unwrap() else {
        panic!("nested body did not decode as IF")
    };
    assert_eq!(parameter_reference(&nested.condition), "a");
    let RawTree::LengthNode(inner_raw) = nested.then_branch else {
        panic!("then branch did not preserve the nested IF")
    };
    let StructuredNode::If(inner) = inner_raw.decode_structured().unwrap() else {
        panic!("nested then branch did not decode as IF")
    };
    assert_eq!(parameter_reference(&inner.condition), "b");
    let int_value = |tree: &RawTree<'_>| match tree {
        RawTree::Leaf(SimpleTerm {
            tag: INTCONST_TAG,
            value: TermValue::Int(value),
            ..
        }) => Some(*value),
        _ => None,
    };
    assert_eq!(int_value(&inner.then_branch), Some(1));
    assert_eq!(int_value(&inner.else_branch), Some(2));
    assert_eq!(int_value(&nested.else_branch), Some(3));

    let RawTree::LengthNode(constant_raw) = body(4) else {
        panic!("constant body is not an IF node")
    };
    let StructuredNode::If(constant) = constant_raw.decode_structured().unwrap() else {
        panic!("constant body did not decode as IF")
    };
    assert!(matches!(
        constant.condition,
        RawTree::Leaf(SimpleTerm {
            tag: TRUECONST_TAG,
            value: TermValue::Boolean(true),
            ..
        })
    ));
    assert_eq!(int_value(&constant.then_branch), Some(17));
    assert_eq!(int_value(&constant.else_branch), Some(23));

    let (_, repeated) = source_outputs_named(CONDITIONALS, "Conditionals.scala", |_| {}).unwrap();
    assert_eq!(output.bytes(), repeated.bytes());

    if std::env::var_os(RUN_SCALA_ORACLE).is_some() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let candidate = root.join("target/issue-1312/generated/example/Conditionals.tasty");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        std::fs::write(&candidate, output.bytes()).unwrap();
        let result = Command::new("python3")
            .arg(root.join("tools/tasty-pickler-oracle/run.py"))
            .arg("verify")
            .arg(&candidate)
            .arg("--runtime")
            .env("TMPDIR", root.join("target/issue-1312/tmp"))
            .status()
            .expect("start explicitly requested Scala 3.9 If interoperability gate");
        assert!(result.success(), "If Scala interoperability gate failed");
    }
}

#[test]
fn source_blocks_preserve_ordered_statements_and_final_expression() {
    use dotty_tasty::tasty::*;

    let (_, output) = source_outputs_named(BLOCKS, "Blocks.scala", |_| {}).unwrap();
    let file = TastyFile::parse_and_validate_scala_3_9(output.bytes()).unwrap();
    let addresses = file.ast_address_index().unwrap();
    let body = |declaration_index: usize| {
        addresses
            .get(output.declarations()[declaration_index].address)
            .unwrap()
            .decode_defdef_body()
            .unwrap()
            .rhs
            .unwrap()
    };

    let RawTree::LengthNode(ordered_node) = body(2) else {
        panic!("ordered body is not a BLOCK")
    };
    let ordered = ordered_node.decode_block().unwrap();
    assert_eq!(ordered.stats.len(), 1);
    assert!(matches!(ordered.expression, RawTree::Leaf(_)));
    assert!(matches!(ordered.stats[0], RawTree::LengthNode(ref stat) if stat.tag == IF_TAG));

    let RawTree::LengthNode(single_node) = body(3) else {
        panic!("single body is not a BLOCK")
    };
    let single = single_node.decode_block().unwrap();
    assert!(single.stats.is_empty());
    assert!(matches!(single.expression, RawTree::LengthNode(ref call) if call.tag == APPLY_TAG));

    let RawTree::LengthNode(calls_node) = body(4) else {
        panic!("calls body is not a BLOCK")
    };
    let calls = calls_node.decode_block().unwrap();
    assert_eq!(calls.stats.len(), 2);
    let call_value = |tree: &RawTree<'_>| {
        let RawTree::LengthNode(call) = tree else {
            panic!("expected an APPLY")
        };
        let arguments = call.decode_apply().unwrap().arguments;
        let [
            RawTree::Leaf(SimpleTerm {
                tag: INTCONST_TAG,
                value: TermValue::Int(value),
                ..
            }),
        ] = arguments.as_slice()
        else {
            panic!("expected one integer call argument")
        };
        *value
    };
    for stat in &calls.stats {
        let RawTree::LengthNode(call) = stat else {
            panic!("a call statement was not retained")
        };
        assert_eq!(call.tag, APPLY_TAG);
    }
    assert_eq!(
        calls.stats.iter().map(call_value).collect::<Vec<_>>(),
        [1, 2]
    );
    let RawTree::LengthNode(last_call) = &calls.expression else {
        panic!("the final call expression was not retained")
    };
    assert_eq!(last_call.tag, APPLY_TAG);
    assert_eq!(call_value(&calls.expression), 3);

    let (_, repeated) = source_outputs_named(BLOCKS, "Blocks.scala", |_| {}).unwrap();
    assert_eq!(output.bytes(), repeated.bytes());

    if std::env::var_os(RUN_SCALA_ORACLE).is_some() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let candidate = root.join("target/issue-1313/generated/example/Blocks.tasty");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        std::fs::write(&candidate, output.bytes()).unwrap();
        let result = Command::new("python3")
            .arg(root.join("tools/tasty-pickler-oracle/run.py"))
            .arg("verify")
            .arg(&candidate)
            .arg("--runtime")
            .env("TMPDIR", root.join("target/issue-1313/tmp"))
            .status()
            .expect("start explicitly requested Scala 3.9 Block interoperability gate");
        assert!(result.success(), "Block Scala interoperability gate failed");
    }
}

#[test]
fn local_int_values_emit_ordered_valdefs_and_exact_direct_references() {
    use dotty_tasty::tasty::*;

    let (_, output) = source_outputs_named(LOCALS, "Locals.scala", |_| {}).unwrap();
    let file = TastyFile::parse_and_validate_scala_3_9(output.bytes()).unwrap();
    let addresses = file.ast_address_index().unwrap();
    let body = |declaration_index: usize| {
        addresses
            .get(output.declarations()[declaration_index].address)
            .unwrap()
            .decode_defdef_body()
            .unwrap()
            .rhs
            .unwrap()
    };
    let local_stat = |stat: &RawTree<'_>| {
        let RawTree::LengthNode(node) = stat else {
            panic!("local statement was not a VALDEF")
        };
        assert_eq!(node.tag, VALDEF_TAG);
        let DefinitionBody::ValDef { name, rhs, .. } = node.decode_definition_body().unwrap()
        else {
            panic!("local statement decoded as a non-VALDEF")
        };
        let rhs = rhs.expect("supported local initializer");
        let (rhs_tag, rhs_reference, rhs_int) = match rhs {
            RawTree::Leaf(term) => match term.value {
                TermValue::AstRef(address) => (term.tag, Some(address), None),
                TermValue::Int(value) => (term.tag, None, Some(value)),
                other => panic!("unexpected local initializer leaf: {other:?}"),
            },
            other => panic!("unexpected local initializer tree: {other:?}"),
        };
        (
            file.names().get_utf8(name).unwrap().to_owned(),
            rhs_tag,
            rhs_reference,
            rhs_int,
        )
    };

    let RawTree::LengthNode(copy_body) = body(2) else {
        panic!("copy body is not a BLOCK")
    };
    assert_eq!(copy_body.tag, BLOCK_TAG);
    let copy = copy_body.decode_block().unwrap();
    assert_eq!(copy.stats.len(), 1);
    let (saved_name, saved_tag, saved_ref, _) = local_stat(&copy.stats[0]);
    assert_eq!(saved_name, "saved");
    assert_eq!(saved_tag, TERMREFDIRECT_TAG);
    assert!(saved_ref.is_some());
    let RawTree::Leaf(SimpleTerm {
        tag: TERMREFDIRECT_TAG,
        value: TermValue::AstRef(saved_address),
        ..
    }) = copy.expression
    else {
        panic!("unexpected final local reference: {:?}", copy.expression)
    };
    let saved_declaration = addresses
        .get(saved_address)
        .expect("the direct local reference resolves in the global AST index");
    assert_eq!(saved_declaration.tag, VALDEF_TAG);
    let DefinitionBody::ValDef { name, .. } = saved_declaration.decode_definition_body().unwrap()
    else {
        panic!("local reference target is not a value definition")
    };
    assert_eq!(file.names().get_utf8(name).unwrap(), "saved");

    let RawTree::LengthNode(call_body) = body(4) else {
        panic!("call body is not a BLOCK")
    };
    assert_eq!(call_body.tag, BLOCK_TAG);
    let call = call_body.decode_block().unwrap();
    assert_eq!(call.stats.len(), 1);
    let RawTree::LengthNode(call_local) = &call.stats[0] else {
        panic!("call local statement was not a VALDEF")
    };
    assert_eq!(call_local.tag, VALDEF_TAG);
    let DefinitionBody::ValDef { rhs: Some(rhs), .. } =
        call_local.decode_definition_body().unwrap()
    else {
        panic!("call local has no initializer")
    };
    assert!(matches!(rhs, RawTree::LengthNode(node) if node.tag == APPLY_TAG));

    let RawTree::LengthNode(multiple_body) = body(5) else {
        panic!("multiple body is not a BLOCK")
    };
    assert_eq!(multiple_body.tag, BLOCK_TAG);
    let multiple = multiple_body.decode_block().unwrap();
    assert_eq!(multiple.stats.len(), 2);
    let (first_name, first_tag, first_ref, _) = local_stat(&multiple.stats[0]);
    let (second_name, second_tag, _, second_int) = local_stat(&multiple.stats[1]);
    let first_address = addresses
        .iter_nodes_with_tag(VALDEF_TAG)
        .find_map(|node| {
            let declaration = addresses.get(node.offset as u32)?;
            let DefinitionBody::ValDef { name, .. } = declaration.decode_definition_body().ok()?
            else {
                return None;
            };
            (file.names().get_utf8(name)? == "first").then_some(node.offset as u32)
        })
        .expect("the first local has a global AST address");
    assert_eq!(first_name, "first");
    assert_eq!(second_name, "second");
    assert_eq!(first_tag, TERMREFDIRECT_TAG);
    assert!(first_ref.is_some());
    assert_eq!(second_tag, INTCONST_TAG);
    assert_eq!(second_int, Some(7));
    let RawTree::Leaf(SimpleTerm {
        tag: TERMREFDIRECT_TAG,
        value: TermValue::AstRef(second_address),
        ..
    }) = multiple.expression
    else {
        panic!("unexpected final reference in multiple-local block")
    };
    let first_parent = addresses.parent_of(first_address).unwrap();
    assert_eq!(first_parent.tag, BLOCK_TAG);
    assert_eq!(addresses.parent_of(second_address), Some(first_parent));
    let second_declaration = addresses.get(second_address).unwrap();
    let DefinitionBody::ValDef { name, .. } = second_declaration.decode_definition_body().unwrap()
    else {
        panic!("second local reference target is not a value definition")
    };
    assert_eq!(file.names().get_utf8(name).unwrap(), "second");

    let (_, repeated) = source_outputs_named(LOCALS, "Locals.scala", |_| {}).unwrap();
    assert_eq!(output.bytes(), repeated.bytes());

    if std::env::var_os(RUN_SCALA_ORACLE).is_some() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let candidate = root.join("target/issue-1314/generated/example/Locals.tasty");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        std::fs::write(&candidate, output.bytes()).unwrap();
        let result = Command::new("python3")
            .arg(root.join("tools/tasty-pickler-oracle/run.py"))
            .arg("verify")
            .arg(&candidate)
            .arg("--runtime")
            .env("TMPDIR", root.join("target/issue-1314/tmp"))
            .status()
            .expect("start explicitly requested Scala 3.9 local-value interoperability gate");
        assert!(
            result.success(),
            "Local value Scala interoperability gate failed"
        );
    }
}

#[test]
fn local_value_can_be_an_int_if_branch() {
    let source = "package example\nclass StoredBranch:\n  def stored(flag: Boolean, value: Int): Int = { val saved: Int = value; if flag then saved else 0 }\n";
    source_outputs_named(source, "StoredBranch.scala", |_| {})
        .expect("an in-scope canonical local reference is accepted as an Int branch");
}

#[test]
fn unit_literal_method_roundtrips_as_a_supported_result() {
    use dotty_tasty::tasty::*;

    let source = "package example\nclass UnitResult:\n  def unit: Unit = ()\n";
    let (_, output) = source_outputs_named(source, "UnitResult.scala", |_| {}).unwrap();
    let file = TastyFile::parse_and_validate_scala_3_9(output.bytes()).unwrap();
    let method = output
        .declarations()
        .iter()
        .find(|declaration| {
            file.ast_address_index()
                .unwrap()
                .get(declaration.address)
                .and_then(|node| node.decode_defdef_body().ok())
                .and_then(|body| file.render_name(body.name).ok())
                .as_deref()
                == Some("unit")
        })
        .unwrap();
    let addresses = file.ast_address_index().unwrap();
    let body = addresses
        .get(method.address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    assert!(matches!(
        body.rhs,
        Some(RawTree::Leaf(SimpleTerm {
            tag: UNITCONST_TAG,
            value: TermValue::Unit,
            ..
        }))
    ));
}

#[test]
fn local_values_require_exact_source_typer_bindings() {
    let missing = source_outputs_named(LOCALS, "Locals.scala", |plan| {
        plan.local_values = &[];
    })
    .unwrap_err();
    assert!(matches!(
        missing,
        PickleError::Unsupported {
            feature: UnsupportedFeature::BodyExpression,
            ..
        }
    ));
}

#[test]
fn local_visibility_ends_with_its_lexical_block() {
    let source = "package example\nclass LocalScope:\n  def run: Int =\n    {\n      if true then { val saved: Int = 1; saved; 0 } else 0\n      0\n    }\n";
    let escaped = source_outputs_named_with_tree_edits(
        source,
        "LocalScope.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, store| {
            let (outer_block, outer_typed_id, outer_typed_block) = source_ast
                .iter()
                .find_map(|(source_id, tree)| {
                    let TreeKind::Block(block) = &tree.kind else {
                        return None;
                    };
                    if block.stats.len() != 1 {
                        return None;
                    }
                    let typed_id = bindings.get(SourceId::from_index(7), source_id)?;
                    let TreeKind::Block(typed_block) = &typed_ast.try_get(typed_id)?.kind else {
                        return None;
                    };
                    Some((block.clone(), typed_id, typed_block.clone()))
                })
                .expect("fixture has a one-statement outer block");
            let source_result = outer_block.expr;
            let typed_result = outer_typed_block.expr;
            let (source_ident, typed_ident, typed_ty) = source_ast
                .iter()
                .find_map(|(source_id, tree)| {
                    let TreeKind::Ident(ident) = &tree.kind else {
                        return None;
                    };
                    if store.names.resolve(ident.name.text()) != "saved" {
                        return None;
                    }
                    let typed_id = bindings.get(SourceId::from_index(7), source_id)?;
                    let typed_tree = typed_ast.try_get(typed_id)?;
                    let TreeKind::Ident(typed_ident) = &typed_tree.kind else {
                        return None;
                    };
                    Some((*ident, *typed_ident, typed_tree.ty))
                })
                .expect("fixture has a typed reference to the branch local");
            source_ast.get_mut(source_result).kind = TreeKind::Ident(source_ident);
            let typed_result_binding = bindings
                .get(SourceId::from_index(7), source_result)
                .expect("outer result keeps its typed binding");
            assert_eq!(typed_result_binding, typed_result);
            let typed_node = typed_ast.get_mut(typed_result_binding);
            typed_node.kind = TreeKind::Ident(typed_ident);
            typed_node.ty = typed_ty;
            typed_ast.get_mut(outer_typed_id).ty = typed_ty;
        },
        |_| {},
    )
    .unwrap_err();
    assert!(
        matches!(
            escaped,
            PickleError::Reference {
                reason: ReferenceError::NotRegistered,
                ..
            }
        ),
        "unexpected out-of-scope local result: {escaped:?}"
    );
}

#[test]
fn shallow_local_initializers_respect_zero_expression_depth() {
    let source = "package example\nclass ShallowLocals:\n  def literal: Int = { val saved: Int = 1; saved }\n  def formal(value: Int): Int = { val saved: Int = value; saved }\n";
    source_outputs_named_with_limits(
        source,
        "ShallowLocals.scala",
        ValidationLimits {
            max_expression_depth: 0,
            ..ValidationLimits::default()
        },
        |_| {},
    )
    .expect("literal and formal local initializers fit the emitter's depth-zero limit");
}

#[test]
fn block_validation_rejects_unmapped_reordered_repeated_and_unsupported_statements() {
    let mut expected = None;
    let reordered = source_outputs_named_with_tree_edits(
        BLOCKS,
        "Blocks.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, _| {
            let (source_block, typed_block, _, _) =
                blocks_with_stat_count_in(source_ast, typed_ast, bindings, 2);
            let source_stat = match &source_ast.get(source_block).kind {
                TreeKind::Block(block) => block.stats[0],
                _ => unreachable!(),
            };
            let mapped_typed = bindings.get(SourceId::from_index(7), source_stat).unwrap();
            let TreeKind::Block(block) = &mut typed_ast.get_mut(typed_block).kind else {
                unreachable!()
            };
            expected = Some((source_stat, block.stats[1], mapped_typed));
            block.stats.swap(0, 1);
        },
        |_| {},
    )
    .unwrap_err();
    let (source_stat, expected_typed, actual_typed) = expected.unwrap();
    assert_eq!(
        reordered,
        PickleError::BodyBinding {
            source: SourceId::from_index(7),
            tree: source_stat,
            expected: expected_typed,
            actual: Some(actual_typed),
        }
    );

    let mut shared_argument = None;
    let repeated = source_outputs_named_with_tree_edits(
        BLOCKS,
        "Blocks.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, _| {
            let (source_block, typed_block, _, _) =
                blocks_with_stat_count_in(source_ast, typed_ast, bindings, 2);
            let TreeKind::Block(source) = &source_ast.get(source_block).kind else {
                unreachable!()
            };
            let TreeKind::Block(typed) = &typed_ast.get(typed_block).kind else {
                unreachable!()
            };
            let TreeKind::Apply(source_second) = &source_ast.get(source.stats[1]).kind else {
                unreachable!()
            };
            let TreeKind::Apply(typed_first) = &typed_ast.get(typed.stats[0]).kind else {
                unreachable!()
            };
            let shared = typed_first.args[0];
            let TreeKind::Apply(typed_second) = &mut typed_ast.get_mut(typed.stats[1]).kind else {
                unreachable!()
            };
            shared_argument = Some(shared);
            typed_second.args[0] = shared;
            bindings.remove(SourceId::from_index(7), source_second.args[0]);
            bindings
                .insert(SourceId::from_index(7), source_second.args[0], shared)
                .unwrap();
        },
        |_| {},
    )
    .unwrap_err();
    assert_eq!(
        repeated,
        PickleError::RepeatedExpression {
            tree: shared_argument.unwrap(),
        }
    );

    let mut cycle_id = None;
    let cycle = source_outputs_named_with_tree_edits(
        BLOCKS,
        "Blocks.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, _| {
            let (source_block, typed_block, _, _) =
                blocks_with_stat_count_in(source_ast, typed_ast, bindings, 2);
            cycle_id = Some(typed_block);
            let TreeKind::Block(block) = &mut source_ast.get_mut(source_block).kind else {
                unreachable!()
            };
            block.stats[0] = source_block;
            let TreeKind::Block(block) = &mut typed_ast.get_mut(typed_block).kind else {
                unreachable!()
            };
            block.stats[0] = typed_block;
        },
        |_| {},
    )
    .unwrap_err();
    assert_eq!(
        cycle,
        PickleError::RepeatedExpression {
            tree: cycle_id.unwrap(),
        }
    );

    let missing_typed_stat = source_outputs_named_with_tree_edits(
        BLOCKS,
        "Blocks.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, _| {
            let (_, typed_block, _, _) =
                blocks_with_stat_count_in(source_ast, typed_ast, bindings, 2);
            let TreeKind::Block(block) = &mut typed_ast.get_mut(typed_block).kind else {
                unreachable!()
            };
            block.stats.pop();
        },
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(
        missing_typed_stat,
        PickleError::Unsupported {
            feature: UnsupportedFeature::BodyExpression,
            ..
        }
    ));

    let mutable_local = "package example\nclass LocalBlock:
  def local: Int =
    var saved: Int = 1
    saved
";
    let (_, mutable_output) =
        source_outputs_named(mutable_local, "LocalBlock.scala", |_| {}).unwrap();
    dotty_tasty::tasty::TastyFile::parse_and_validate_scala_3_9(mutable_output.bytes()).unwrap();
}

#[test]
fn block_validation_enforces_result_depth_and_item_limits() {
    let wrong_result = source_outputs_named_with_tree_edits(
        BLOCKS,
        "Blocks.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, _| {
            let (_, typed_block, _, _) =
                blocks_with_stat_count_in(source_ast, typed_ast, bindings, 2);
            let TreeKind::Block(block) = &typed_ast.get(typed_block).kind else {
                unreachable!()
            };
            let TreeKind::Apply(final_call) = &typed_ast.get(block.expr).kind else {
                unreachable!()
            };
            let bad_type = typed_ast.get(final_call.args[0]).ty;
            typed_ast.get_mut(typed_block).ty = bad_type;
        },
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(wrong_result, PickleError::BodyType { .. }));

    let nested_application = "package example\nclass NestedBlock:
  def identity(x: Int): Int = x
  def run: Int =
    identity(identity(1))
    identity(2)
";
    let depth = source_outputs_named_with_limits(
        nested_application,
        "NestedBlock.scala",
        ValidationLimits {
            max_expression_depth: 0,
            ..ValidationLimits::default()
        },
        |_| {},
    )
    .unwrap_err();
    assert_eq!(
        depth,
        PickleError::LimitExceeded {
            kind: LimitKind::ExpressionDepth,
            limit: 0,
        }
    );

    let items = source_outputs_named_with_limits(
        BLOCKS,
        "Blocks.scala",
        ValidationLimits {
            max_items: 0,
            ..ValidationLimits::default()
        },
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(
        items,
        PickleError::LimitExceeded {
            kind: LimitKind::Items,
            limit: 0
        }
    ));
}

#[test]
fn raw_numeric_leaves_consume_items_and_reject_repeated_children() {
    let minimum_budget = |source: &str| {
        (1..=256)
            .find(|&max_items| {
                source_outputs_named_with_limits(
                    source,
                    "NumericBlocks.scala",
                    ValidationLimits {
                        max_items,
                        ..ValidationLimits::default()
                    },
                    |_| {},
                )
                .is_ok()
            })
            .expect("numeric block validates within the test budget range")
    };
    let one_numeric_block = "package example\nclass NumericBlocks:\n  def run: Int =\n    {\n      20\n      20\n    }\n";
    source_outputs_named_with_tree_edits(
        one_numeric_block,
        "NumericBlocks.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, _| {
            blocks_with_stat_count_in(source_ast, typed_ast, bindings, 1);
        },
        |_| {},
    )
    .expect("single numeric statement fixture is a one-stat Block");
    let single_budget = minimum_budget(one_numeric_block);
    let block_budget = minimum_budget(NUMERIC_BLOCKS);
    assert!(
        block_budget > single_budget,
        "additional numeric statements used no validation items: {single_budget} / {block_budget}"
    );
    assert_eq!(
        source_outputs_named_with_limits(
            NUMERIC_BLOCKS,
            "NumericBlocks.scala",
            ValidationLimits {
                max_items: block_budget - 1,
                ..ValidationLimits::default()
            },
            |_| {},
        )
        .unwrap_err(),
        PickleError::LimitExceeded {
            kind: LimitKind::Items,
            limit: block_budget - 1,
        }
    );

    let mut shared_statement = None;
    let repeated_statement = source_outputs_named_with_tree_edits(
        NUMERIC_BLOCKS,
        "NumericBlocks.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, _| {
            let (source_block, typed_block, _, _) =
                blocks_with_stat_count_in(source_ast, typed_ast, bindings, 19);
            let TreeKind::Block(block) = &mut typed_ast.get_mut(typed_block).kind else {
                unreachable!()
            };
            shared_statement = Some(block.stats[0]);
            block.stats[1] = block.stats[0];
            let TreeKind::Block(source) = &source_ast.get(source_block).kind else {
                unreachable!()
            };
            assert_ne!(source.stats[0], source.stats[1]);
        },
        |_| {},
    )
    .unwrap_err();
    assert_eq!(
        repeated_statement,
        PickleError::RepeatedExpression {
            tree: shared_statement.unwrap(),
        }
    );

    const NUMERIC_IF: &str = "package example\nclass NumericIf:\n  def choose(flag: Boolean): Int = if flag then 1 else 2\n";
    let mut shared = None;
    let repeated = source_outputs_named_with_tree_edits(
        NUMERIC_IF,
        "NumericIf.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, _| {
            let (source_id, typed_id, mut source_if, mut typed_if) =
                first_conditional_ids(source_ast, typed_ast, bindings);
            shared = Some(
                bindings
                    .get(SourceId::from_index(7), source_if.then_branch)
                    .unwrap(),
            );
            source_if.else_branch = source_if.then_branch;
            typed_if.else_branch = typed_if.then_branch;
            source_ast.get_mut(source_id).kind = TreeKind::If(source_if);
            typed_ast.get_mut(typed_id).kind = TreeKind::If(typed_if);
        },
        |_| {},
    )
    .unwrap_err();
    assert_eq!(
        repeated,
        PickleError::RepeatedExpression {
            tree: shared.unwrap(),
        }
    );
}

fn source_outputs(
    text: &str,
    edit: impl FnOnce(&mut PicklePlan<'_>),
) -> Result<(DeclarationSkeleton, PickledTasty), PickleError> {
    source_outputs_named(text, "Answer.scala", edit)
}

fn source_outputs_named(
    text: &str,
    source_name: &str,
    edit: impl FnOnce(&mut PicklePlan<'_>),
) -> Result<(DeclarationSkeleton, PickledTasty), PickleError> {
    source_outputs_named_with_limits(text, source_name, ValidationLimits::default(), edit)
}

fn source_outputs_named_with_limits(
    text: &str,
    source_name: &str,
    validation_limits: ValidationLimits,
    edit: impl FnOnce(&mut PicklePlan<'_>),
) -> Result<(DeclarationSkeleton, PickledTasty), PickleError> {
    source_outputs_named_with_tree_edits(
        text,
        source_name,
        validation_limits,
        |_, _, _, _| {},
        edit,
    )
}

fn source_outputs_named_with_tree_edits(
    text: &str,
    source_name: &str,
    validation_limits: ValidationLimits,
    edit_trees: impl FnOnce(
        &mut AstArena<Untyped>,
        &mut AstArena<Typed>,
        &mut dotty_typer::SourceTypedIndex,
        &SemanticStore,
    ),
    edit: impl FnOnce(&mut PicklePlan<'_>),
) -> Result<(DeclarationSkeleton, PickledTasty), PickleError> {
    source_outputs_named_with_tree_edits_and_pickle_limits(
        text,
        source_name,
        validation_limits,
        PickleLimits::default(),
        edit_trees,
        edit,
    )
}

fn source_outputs_named_with_tree_edits_and_pickle_limits(
    text: &str,
    source_name: &str,
    validation_limits: ValidationLimits,
    pickle_limits: PickleLimits,
    edit_trees: impl FnOnce(
        &mut AstArena<Untyped>,
        &mut AstArena<Typed>,
        &mut dotty_typer::SourceTypedIndex,
        &SemanticStore,
    ),
    edit: impl FnOnce(&mut PicklePlan<'_>),
) -> Result<(DeclarationSkeleton, PickledTasty), PickleError> {
    let source = SourceId::from_index(7);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(text).unwrap();
    assert!(scanner.diagnostics().is_empty());
    let parsed = parse_compilation_unit(
        SourceText::new(text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source,
        source_name,
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        other => panic!("source fixture naming failed: {other:?}"),
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!()
    };
    let [class_tree] = package.stats.as_slice() else {
        panic!()
    };
    let class = index.symbol_at(source, *class_tree).unwrap();
    let TreeKind::TypeDef(class_definition) = &parsed.ast.get(*class_tree).kind else {
        panic!()
    };
    let TreeKind::Template(template) = &parsed.ast.get(class_definition.rhs).kind else {
        panic!()
    };
    let constructor = index.symbol_at(source, template.constructor).unwrap();
    let TreeKind::DefDef(constructor_definition) = &parsed.ast.get(template.constructor).kind
    else {
        panic!()
    };
    let clauses = |definition: &DefDef<Untyped>| {
        definition
            .value_param_clauses
            .iter()
            .map(|clause| {
                clause
                    .iter()
                    .map(|tree| index.symbol_at(source, *tree).unwrap())
                    .collect()
            })
            .collect()
    };
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer.complete_symbol(class).unwrap();
    typer.complete_symbol(constructor).unwrap();
    let mut methods = Vec::new();
    for &tree in &template.body {
        let TreeKind::DefDef(definition) = &parsed.ast.get(tree).kind else {
            continue;
        };
        let method = index.symbol_at(source, tree).unwrap();
        typer.complete_symbol(method).unwrap();
        let context = typer.expression_context_for(method).unwrap();
        let method_name = typer
            .store()
            .names
            .resolve(definition.name.as_name().text());
        let is_boolean_result = text.lines().any(|line| {
            line.contains(&format!("def {method_name}"))
                && (line.contains("): Boolean") || line.contains(": Boolean ="))
        });
        let is_unit_result = text.lines().any(|line| {
            line.contains(&format!("def {method_name}"))
                && (line.contains("): Unit") || line.contains(": Unit ="))
        });
        let expected_result = if is_boolean_result {
            definitions.boolean
        } else if is_unit_result {
            definitions.unit
        } else {
            // Keep malformed and not-yet-supported signatures on the old Int
            // expectation path so these tests reach pickler validation.
            definitions.int
        };
        let body = definition.rhs.map(|rhs| {
            typer
                .type_expression_expected(rhs, context, expected_result)
                .unwrap()
        });
        if let (Some(rhs), Some(body)) = (definition.rhs, body) {
            assert_eq!(typer.source_typed_index().get(source, rhs), Some(body));
        }
        methods.push(MethodPicklePlan {
            symbol: method,
            source_tree: tree,
            parameter_clauses: clauses(definition),
            body,
        });
    }
    let local_values = parsed
        .ast
        .iter()
        .filter_map(|(source_tree, tree)| {
            if !matches!(tree.kind, TreeKind::ValDef(_)) {
                return None;
            }
            let binding = typer.typed_local_binding_at(source, source_tree)?;
            Some(LocalValueBinding {
                source_tree,
                symbol: binding.symbol(),
                typed_declaration: binding.declaration(),
                scope: binding.scope(),
            })
        })
        .collect::<Vec<_>>();
    // Give negative tests an isolated candidate to corrupt. The original
    // frontend session below remains available for the read-only input check.
    let mut source_ast = AstArena::new();
    for (_, tree) in parsed.ast.iter() {
        source_ast.alloc(tree.clone());
    }
    let mut typed_ast = AstArena::new();
    for (_, tree) in typer.typed_ast().iter() {
        typed_ast.alloc(tree.clone());
    }
    let mut typed_bindings = typer.source_typed_index().clone();
    edit_trees(
        &mut source_ast,
        &mut typed_ast,
        &mut typed_bindings,
        typer.store(),
    );
    let bindings = |source, tree| typed_bindings.get(source, tree);
    let mut plan = PicklePlan {
        store: typer.store(),
        definitions: &definitions,
        typed_ast: &typed_ast,
        source_ast: &source_ast,
        source_index: &index,
        typed_bindings: &bindings,
        source: SourceMetadata {
            id: source,
            name: source_name.into(),
        },
        source_root: parsed.root,
        class,
        class_tree: *class_tree,
        constructor: DefaultConstructorPlan {
            symbol: constructor,
            source_tree: template.constructor,
            parameter_clauses: constructor_definition
                .value_param_clauses
                .iter()
                .map(|clause| {
                    clause
                        .iter()
                        .map(|tree| {
                            index
                                .derived_symbol_at(constructor, source, *tree)
                                .expect("constructor-owned parameter identity")
                        })
                        .collect()
                })
                .collect(),
        },
        methods: &methods,
        local_values: &local_values,
    };
    edit(&mut plan);
    let before = format!(
        "{:?}{:?}{:?}{:?}",
        typer.store(),
        typer.typed_ast(),
        index,
        parsed.ast
    );
    let mark = typer.store().checkpoint();
    let checked = plan.validate_with_limits(validation_limits);
    assert_eq!(typer.store().checkpoint(), mark);
    assert_eq!(
        format!(
            "{:?}{:?}{:?}{:?}",
            typer.store(),
            typer.typed_ast(),
            index,
            parsed.ast
        ),
        before
    );
    let checked = checked?;
    assert!(std::ptr::eq(checked.plan().store, typer.store()));
    assert!(std::ptr::eq(checked.plan().typed_ast, &typed_ast));
    assert_eq!(
        checked.package(),
        index.symbol_at(source, parsed.root).unwrap()
    );
    assert_eq!(checked.plan().constructor.symbol, constructor);
    assert_eq!(
        checked.plan().methods[0].symbol,
        index.symbol_at(source, template.body[0]).unwrap()
    );
    for method in checked.plan().methods {
        let body = method.body.expect("concrete source method body");
        assert!(matches!(
            typer.typed_ast().get(body).kind,
            TreeKind::Literal(Literal {
                value: Constant::Int(_) | Constant::Boolean(_) | Constant::Unit
            }) | TreeKind::Ident(_)
                | TreeKind::Select(_)
                | TreeKind::Apply(_)
                | TreeKind::If(_)
                | TreeKind::Block(_)
        ));
    }
    let body = checked.plan().methods[0].body.unwrap();
    let expected_literal = match &typer.typed_ast().get(body).kind {
        TreeKind::Literal(Literal { value }) => Some(value.clone()),
        TreeKind::Ident(_)
        | TreeKind::Select(_)
        | TreeKind::Apply(_)
        | TreeKind::If(_)
        | TreeKind::Block(_) => None,
        other => panic!("unexpected typed method body: {other:?}"),
    };
    let output = checked.pickle_with_limits(pickle_limits)?;
    for method in &methods {
        assert_eq!(output.body(method.symbol), method.body);
    }
    assert_eq!(output.source(), &plan.source);
    let complete =
        dotty_tasty::tasty::TastyFile::parse_and_validate_scala_3_9(output.bytes()).unwrap();
    let addresses = complete.ast_address_index().unwrap();
    let wire_method = addresses
        .get(output.declarations()[2].address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    match (expected_literal, wire_method.rhs) {
        (Some(Constant::Int(value)), Some(dotty_tasty::tasty::RawTree::Leaf(rhs))) => {
            assert_eq!(rhs.tag, dotty_tasty::tasty::INTCONST_TAG);
            assert_eq!(rhs.value, dotty_tasty::tasty::TermValue::Int(value));
        }
        (Some(Constant::Boolean(true)), Some(dotty_tasty::tasty::RawTree::Leaf(rhs))) => {
            assert_eq!(rhs.tag, dotty_tasty::tasty::TRUECONST_TAG);
            assert_eq!(rhs.value, dotty_tasty::tasty::TermValue::Boolean(true));
        }
        (Some(Constant::Boolean(false)), Some(dotty_tasty::tasty::RawTree::Leaf(rhs))) => {
            assert_eq!(rhs.tag, dotty_tasty::tasty::FALSECONST_TAG);
            assert_eq!(rhs.value, dotty_tasty::tasty::TermValue::Boolean(false));
        }
        (Some(Constant::Unit), Some(dotty_tasty::tasty::RawTree::Leaf(rhs))) => {
            assert_eq!(rhs.tag, dotty_tasty::tasty::UNITCONST_TAG);
            assert_eq!(rhs.value, dotty_tasty::tasty::TermValue::Unit);
        }
        (None, Some(dotty_tasty::tasty::RawTree::Leaf(rhs))) => {
            assert_eq!(rhs.tag, dotty_tasty::tasty::TERMREFDIRECT_TAG);
            assert!(matches!(
                rhs.value,
                dotty_tasty::tasty::TermValue::AstRef(_)
            ));
        }
        (None, Some(dotty_tasty::tasty::RawTree::NatAst { tag, .. })) => {
            assert_eq!(tag, dotty_tasty::tasty::SELECT_TAG);
        }
        (None, Some(dotty_tasty::tasty::RawTree::LengthNode(node))) => {
            assert!(matches!(
                node.tag,
                dotty_tasty::tasty::APPLY_TAG
                    | dotty_tasty::tasty::IF_TAG
                    | dotty_tasty::tasty::BLOCK_TAG
            ));
        }
        (expected, actual) => panic!("body shape changed: {expected:?} / {actual:?}"),
    }
    assert_eq!(complete.encode_validated().unwrap(), output.bytes());
    let skeleton = checked.declaration_skeleton()?;
    let file =
        dotty_tasty::tasty::TastyFile::parse_and_validate_scala_3_9(skeleton.provisional_bytes())
            .unwrap();
    assert_eq!(file.source_file().unwrap(), Some(source_name));
    assert_eq!(
        skeleton.omitted_bodies(),
        methods
            .iter()
            .map(|method| OmittedMethodBody {
                method: method.symbol,
                typed_root: method.body.unwrap(),
            })
            .collect::<Vec<_>>()
    );
    assert_eq!(typer.store().checkpoint(), mark);
    assert_eq!(
        format!(
            "{:?}{:?}{:?}{:?}",
            typer.store(),
            typer.typed_ast(),
            index,
            parsed.ast
        ),
        before
    );
    Ok((skeleton, output))
}

fn source_plan(
    text: &str,
    edit: impl FnOnce(&mut PicklePlan<'_>),
) -> Result<DeclarationSkeleton, PickleError> {
    source_outputs(text, edit).map(|(skeleton, _)| skeleton)
}

fn nested_application_ids(
    source_ast: &AstArena<Untyped>,
    typed_bindings: &dotty_typer::SourceTypedIndex,
) -> (
    TreeId<Untyped>,
    TreeId<Typed>,
    TreeId<Untyped>,
    TreeId<Typed>,
) {
    source_ast
        .iter()
        .find_map(|(source_outer, source_tree)| {
            let TreeKind::Apply(source_apply) = &source_tree.kind else {
                return None;
            };
            let source_inner = *source_apply.args.first()?;
            if !matches!(source_ast.get(source_inner).kind, TreeKind::Apply(_)) {
                return None;
            }
            let typed_outer = typed_bindings.get(SourceId::from_index(7), source_outer)?;
            let typed_inner = typed_bindings.get(SourceId::from_index(7), source_inner)?;
            Some((source_outer, typed_outer, source_inner, typed_inner))
        })
        .expect("fixture contains a nested application")
}

fn first_int_operator_select(
    source_ast: &AstArena<Untyped>,
    typed_ast: &AstArena<Typed>,
    typed_bindings: &dotty_typer::SourceTypedIndex,
) -> TreeId<Typed> {
    source_ast
        .iter()
        .find_map(|(source_id, source_tree)| {
            if !matches!(
                source_tree.kind,
                TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
            ) {
                return None;
            }
            let typed_id = typed_bindings.get(SourceId::from_index(7), source_id)?;
            let TreeKind::Apply(application) = &typed_ast.try_get(typed_id)?.kind else {
                return None;
            };
            Some(application.function)
        })
        .expect("fixture contains a typed infix operator Select")
}

fn first_conditional_ids(
    source_ast: &AstArena<Untyped>,
    typed_ast: &AstArena<Typed>,
    typed_bindings: &dotty_typer::SourceTypedIndex,
) -> (TreeId<Untyped>, TreeId<Typed>, If<Untyped>, If<Typed>) {
    source_ast
        .iter()
        .find_map(|(source_id, tree)| {
            let TreeKind::If(source_if) = &tree.kind else {
                return None;
            };
            let source_if = *source_if;
            let typed_id = typed_bindings.get(SourceId::from_index(7), source_id)?;
            let typed_if = match &typed_ast.try_get(typed_id)?.kind {
                TreeKind::If(typed_if) => *typed_if,
                _ => return None,
            };
            Some((source_id, typed_id, source_if, typed_if))
        })
        .expect("fixture contains a typed If expression")
}

fn blocks_with_stat_count_in(
    source_ast: &AstArena<Untyped>,
    typed_ast: &AstArena<Typed>,
    bindings: &dotty_typer::SourceTypedIndex,
    stat_count: usize,
) -> (TreeId<Untyped>, TreeId<Typed>, Block<Untyped>, Block<Typed>) {
    source_ast
        .iter()
        .find_map(|(source_id, tree)| {
            let TreeKind::Block(source_block) = &tree.kind else {
                return None;
            };
            if source_block.stats.len() != stat_count {
                return None;
            }
            let typed_id = bindings.get(SourceId::from_index(7), source_id)?;
            let TreeKind::Block(typed_block) = &typed_ast.try_get(typed_id)?.kind else {
                return None;
            };
            Some((
                source_id,
                typed_id,
                source_block.clone(),
                typed_block.clone(),
            ))
        })
        .expect("fixture contains a typed Block with the requested stat count")
}

#[test]
fn actual_answer_source_hands_off_exact_canonical_symbols_and_typed_rhs() {
    let skeleton = source_plan(ANSWER, |_| {}).unwrap();
    assert_eq!(skeleton.intended_tasty_path(), "example/Answer.tasty");
}

#[test]
fn same_class_forward_method_reference_is_a_named_select_and_deterministic() {
    use dotty_tasty::tasty::*;

    let (_, first) = source_outputs_named(REFERENCES, "References.scala", |_| {}).unwrap();
    let (_, second) = source_outputs_named(REFERENCES, "References.scala", |_| {}).unwrap();
    assert_eq!(first.bytes(), second.bytes());
    let class = first.declarations()[0].symbol;
    let first_method = first.declarations()[2];
    let second_method = first.declarations()[3];
    assert_eq!(first_method.owner, class);
    assert_eq!(second_method.owner, class);

    let file = TastyFile::parse_and_validate_scala_3_9(first.bytes()).unwrap();
    let addresses = file.ast_address_index().unwrap();
    let first_body = addresses
        .get(first_method.address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    let select = first_body.rhs.unwrap().decode_select().unwrap();
    assert_eq!(select.tag, SELECT_TAG);
    let receiver = select.qualifier.decode_qual_this().unwrap();
    let class_syntax = receiver.child.decode_ident().unwrap();
    let class_ref = class_syntax.type_tree.decode_reference().unwrap();
    assert_eq!(class_ref.tag, TYPEREFSYMBOL_TAG);
    assert_eq!(class_ref.reference, first.declarations()[0].address);

    if std::env::var_os(RUN_SCALA_ORACLE).is_some() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let candidate = root.join("target/issue-1242/generated/example/References.tasty");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        std::fs::write(&candidate, first.bytes()).unwrap();
        let result = Command::new("python3")
            .arg(root.join("tools/tasty-pickler-oracle/run.py"))
            .arg("verify")
            .arg(&candidate)
            .arg("--runtime")
            .env("TMPDIR", root.join("target/issue-1242/tmp"))
            .status()
            .expect("start explicitly requested Scala 3.9 -from-tasty gate");
        assert!(
            result.success(),
            "requested References Scala interoperability gate failed"
        );
    }
}

#[test]
fn explicit_this_method_reference_preserves_the_verified_this_wire_shape() {
    use dotty_tasty::tasty::*;

    let (_, first) =
        source_outputs_named(EXPLICIT_THIS_REFERENCES, "References.scala", |_| {}).unwrap();
    let (_, second) =
        source_outputs_named(EXPLICIT_THIS_REFERENCES, "References.scala", |_| {}).unwrap();
    assert_eq!(first.bytes(), second.bytes());
    let file = TastyFile::parse_and_validate_scala_3_9(first.bytes()).unwrap();
    let addresses = file.ast_address_index().unwrap();
    let first_body = addresses
        .get(first.declarations()[2].address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    let select = first_body.rhs.unwrap().decode_select().unwrap();
    let receiver = select.qualifier.decode_this().unwrap();
    let class_ref = receiver.child.decode_reference().unwrap();
    assert_eq!(class_ref.tag, TYPEREFSYMBOL_TAG);
    assert_eq!(class_ref.reference, first.declarations()[0].address);

    if std::env::var_os(RUN_SCALA_ORACLE).is_some() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let candidate =
            root.join("target/issue-1242/generated-explicit-this/example/References.tasty");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        std::fs::write(&candidate, first.bytes()).unwrap();
        let result = Command::new("python3")
            .arg(root.join("tools/tasty-pickler-oracle/run.py"))
            .arg("verify")
            .arg(&candidate)
            .arg("--runtime")
            .env("TMPDIR", root.join("target/issue-1242/tmp"))
            .status()
            .expect("start explicitly requested Scala 3.9 -from-tasty gate");
        assert!(
            result.success(),
            "explicit-this Scala interoperability gate failed"
        );
    }
}

#[test]
fn same_class_int_applications_preserve_selected_target_and_argument_order() {
    use dotty_tasty::tasty::*;

    let (_, first) = source_outputs_named(OPERATIONS, "Operations.scala", |_| {}).unwrap();
    let (_, second) = source_outputs_named(OPERATIONS, "Operations.scala", |_| {}).unwrap();
    assert_eq!(first.bytes(), second.bytes());
    let file = TastyFile::parse_and_validate_scala_3_9(first.bytes()).unwrap();
    let addresses = file.ast_address_index().unwrap();
    let use_body = addresses
        .get(first.declarations()[2].address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    let RawTree::LengthNode(call_node) = use_body.rhs.unwrap() else {
        panic!("expected APPLY")
    };
    let call = call_node.decode_apply().unwrap();
    let RawTree::LengthNode(select_node) = call.function else {
        panic!("expected SELECTin")
    };
    let select = select_node.decode_select_in().unwrap();
    assert_eq!(select.tag, SELECTIN_TAG);
    assert!(matches!(
        file.render_signed_name(select.name).unwrap(),
        Some(RenderedSignedName::Signed { original, signature })
            if original == "identity"
                && signature.result == "scala.Int"
                && signature.parameters == [RenderedParamSig::TermParameter("scala.Int".into())]
    ));
    assert_eq!(call.arguments.len(), 1);
    assert!(matches!(
        call.arguments[0],
        RawTree::Leaf(SimpleTerm {
            tag: INTCONST_TAG,
            value: TermValue::Int(42),
            ..
        })
    ));

    let propagate_body = addresses
        .get(first.declarations()[4].address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    let RawTree::LengthNode(propagate_node) = propagate_body.rhs.unwrap() else {
        panic!("expected APPLY")
    };
    let propagate_call = propagate_node.decode_apply().unwrap();
    let RawTree::LengthNode(propagate_select_node) = propagate_call.function else {
        panic!("expected SELECTin")
    };
    let propagate_select = propagate_select_node.decode_select_in().unwrap();
    assert!(
        matches!(file.render_signed_name(propagate_select.name).unwrap(), Some(RenderedSignedName::Signed { original, .. }) if original == "identity")
    );
    assert_eq!(propagate_call.arguments.len(), 1);
    assert!(
        matches!(propagate_call.arguments[0], RawTree::Leaf(ref leaf) if leaf.tag == TERMREFDIRECT_TAG)
    );
    let explicit_body = addresses
        .get(first.declarations()[5].address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    let RawTree::LengthNode(explicit_call_node) = explicit_body.rhs.unwrap() else {
        panic!("expected APPLY")
    };
    let explicit_call = explicit_call_node.decode_apply().unwrap();
    let RawTree::LengthNode(explicit_select_node) = explicit_call.function else {
        panic!("expected SELECTin")
    };
    let explicit_select = explicit_select_node.decode_select_in().unwrap();
    assert!(matches!(
        explicit_select.qualifier,
        RawTree::Ast { tag: THIS_TAG, .. }
    ));

    let picks_body = addresses
        .get(first.declarations()[7].address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    let RawTree::LengthNode(picks_node) = picks_body.rhs.unwrap() else {
        panic!("expected APPLY")
    };
    let picks_call = picks_node.decode_apply().unwrap();
    let RawTree::LengthNode(picks_select_node) = picks_call.function else {
        panic!("expected SELECTin")
    };
    let picks_select = picks_select_node.decode_select_in().unwrap();
    assert!(matches!(
        file.render_signed_name(picks_select.name).unwrap(),
        Some(RenderedSignedName::Signed { original, signature })
            if original == "pick"
                && signature.result == "scala.Int"
                && signature.parameters == [
                    RenderedParamSig::TermParameter("scala.Int".into()),
                    RenderedParamSig::TermParameter("scala.Int".into()),
                ]
    ));
    assert_eq!(picks_call.arguments.len(), 2);
    assert!(
        matches!(picks_call.arguments[0], RawTree::Leaf(ref leaf) if leaf.tag == INTCONST_TAG && leaf.value == TermValue::Int(11))
    );
    assert!(
        matches!(picks_call.arguments[1], RawTree::Leaf(ref leaf) if leaf.tag == INTCONST_TAG && leaf.value == TermValue::Int(42))
    );

    let nested_body = addresses
        .get(first.declarations()[8].address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    let RawTree::LengthNode(outer_node) = nested_body.rhs.unwrap() else {
        panic!("expected outer APPLY")
    };
    let outer_call = outer_node.decode_apply().unwrap();
    let RawTree::LengthNode(inner_node) = &outer_call.arguments[0] else {
        panic!("expected nested APPLY")
    };
    assert_eq!(inner_node.tag, APPLY_TAG);

    assert!(matches!(
        source_outputs_named_with_limits(
            OPERATIONS,
            "Operations.scala",
            ValidationLimits {
                max_expression_depth: 0,
                ..ValidationLimits::default()
            },
            |_| {},
        ),
        Err(PickleError::LimitExceeded {
            kind: LimitKind::ExpressionDepth,
            limit: 0
        })
    ));

    if std::env::var_os(RUN_SCALA_ORACLE).is_some() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let candidate = root.join("target/issue-1243/generated/example/Operations.tasty");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        std::fs::write(&candidate, first.bytes()).unwrap();
        let result = Command::new("python3")
            .arg(root.join("tools/tasty-pickler-oracle/run.py"))
            .arg("verify")
            .arg(&candidate)
            .arg("--runtime")
            .env("TMPDIR", root.join("target/issue-1243/tmp"))
            .status()
            .expect("start explicitly requested Scala 3.9 -from-tasty gate");
        assert!(
            result.success(),
            "Operations Scala interoperability gate failed"
        );
    }
}

#[test]
fn canonical_int_arithmetic_emits_semantic_selectin_and_nested_apply_trees() {
    use dotty_tasty::tasty::*;

    let (_, first) = source_outputs_named(ARITHMETIC, "Arithmetic.scala", |_| {}).unwrap();
    let (_, second) = source_outputs_named(ARITHMETIC, "Arithmetic.scala", |_| {}).unwrap();
    assert_eq!(first.bytes(), second.bytes());
    let file = TastyFile::parse_and_validate_scala_3_9(first.bytes()).unwrap();
    let addresses = file.ast_address_index().unwrap();
    let operator_call = |index: usize, expected_name: &str| {
        let body = addresses
            .get(first.declarations()[index].address)
            .unwrap()
            .decode_defdef_body()
            .unwrap();
        let RawTree::LengthNode(apply_node) = body.rhs.unwrap() else {
            panic!("expected arithmetic APPLY for {expected_name}")
        };
        let apply = apply_node.decode_apply().unwrap();
        let RawTree::LengthNode(select_node) = &apply.function else {
            panic!("expected arithmetic SELECTin for {expected_name}")
        };
        let select = select_node.decode_select_in().unwrap();
        assert_eq!(select.tag, SELECTIN_TAG);
        assert!(matches!(
            file.render_signed_name(select.name).unwrap(),
            Some(RenderedSignedName::Signed { original, signature })
                if original == expected_name
                    && signature.result == "scala.Int"
                    && signature.parameters == [RenderedParamSig::TermParameter("scala.Int".into())]
        ));
        assert_eq!(apply.arguments.len(), 1);
        apply
    };
    for (index, name) in [(2, "+"), (3, "-"), (4, "*"), (5, "/"), (6, "%")] {
        operator_call(index, name);
    }
    let nested = operator_call(7, "+");
    let RawTree::LengthNode(inner_node) = &nested.arguments[0] else {
        panic!("expected nested multiplication APPLY")
    };
    let inner = inner_node.decode_apply().unwrap();
    let RawTree::LengthNode(inner_select_node) = inner.function else {
        panic!("expected nested multiplication SELECTin")
    };
    let inner_select = inner_select_node.decode_select_in().unwrap();
    assert!(matches!(
        file.render_signed_name(inner_select.name).unwrap(),
        Some(RenderedSignedName::Signed { original, .. }) if original == "*"
    ));
    assert_eq!(inner.arguments.len(), 1);

    if std::env::var_os(RUN_SCALA_ORACLE).is_some() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let candidate = root.join("target/issue-1356/generated/example/Arithmetic.tasty");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        std::fs::write(&candidate, first.bytes()).unwrap();
        let result = Command::new("python3")
            .arg(root.join("tools/tasty-pickler-oracle/run.py"))
            .arg("verify")
            .arg(&candidate)
            .arg("--runtime")
            .env("TMPDIR", root.join("target/issue-1356/tmp"))
            .status()
            .expect("start explicitly requested Scala 3.9 -from-tasty gate");
        assert!(
            result.success(),
            "Int arithmetic Scala interoperability gate failed"
        );
    }
}

#[test]
fn parenthesized_int_arithmetic_acceptance_example_is_serialized() {
    let (_, output) = source_outputs_named(ARITHMETIC, "Arithmetic.scala", |_| {}).unwrap();
    assert!(!output.bytes().is_empty());
}

#[test]
fn parenthesized_int_arithmetic_call_arguments_are_serialized() {
    let (_, output) = source_outputs_named(
        PARENTHESIZED_CALL_ARGUMENTS,
        "ParenthesizedCallArguments.scala",
        |_| {},
    )
    .unwrap();
    assert!(!output.bytes().is_empty());
}

#[test]
fn synthetic_int_operator_select_spans_are_validated() {
    for foreign_source in [true, false] {
        let mut select_id = None;
        let result = source_outputs_named_with_tree_edits(
            ARITHMETIC,
            "Arithmetic.scala",
            ValidationLimits::default(),
            |source_ast, typed_ast, bindings, _| {
                let select = first_int_operator_select(source_ast, typed_ast, bindings);
                select_id = Some(select);
                let position = if foreign_source {
                    SourceSpan::new(
                        SourceId::from_index(8),
                        Span::without_point(TextRange::new(0, 1).unwrap()),
                    )
                } else {
                    let source_operator = source_ast
                        .iter()
                        .find(|(_, tree)| {
                            matches!(tree.kind, TreeKind::PhaseSpecific(UntypedNode::InfixOp(_)))
                        })
                        .unwrap()
                        .1
                        .position
                        .unwrap();
                    SourceSpan::new(
                        SourceId::from_index(7),
                        Span::without_point(
                            TextRange::new(
                                source_operator.span().range().start().saturating_add(1),
                                source_operator.span().range().end().saturating_add(1),
                            )
                            .unwrap(),
                        ),
                    )
                };
                typed_ast.get_mut(select).position = Some(position);
            },
            |_| {},
        )
        .unwrap_err();
        let select = select_id.unwrap();
        if foreign_source {
            assert_eq!(
                result,
                PickleError::SourceSpan {
                    expected: SourceId::from_index(7),
                    actual: SourceId::from_index(8),
                }
            );
        } else {
            assert_eq!(result, PickleError::BodySpan { tree: select });
        }
    }
}

#[test]
fn canonical_boolean_operators_preserve_short_circuit_semantics() {
    use dotty_tasty::tasty::*;

    let (_, output) = source_outputs_named(LOGIC, "Logic.scala", |_| {}).unwrap();
    let (_, repeat) = source_outputs_named(LOGIC, "Logic.scala", |_| {}).unwrap();
    assert_eq!(output.bytes(), repeat.bytes());
    let file = TastyFile::parse_and_validate_scala_3_9(output.bytes()).unwrap();
    let addresses = file.ast_address_index().unwrap();
    for (declaration, expected_name) in [(3, "&&"), (4, "||")] {
        let body = addresses
            .get(output.declarations()[declaration].address)
            .unwrap()
            .decode_defdef_body()
            .unwrap();
        let RawTree::LengthNode(application) = body.rhs.unwrap() else {
            panic!("expected Boolean operator APPLY for {expected_name}")
        };
        let application = application.decode_apply().unwrap();
        let RawTree::LengthNode(selection) = application.function else {
            panic!("expected signed Boolean SELECTin for {expected_name}")
        };
        let selection = selection.decode_select_in().unwrap();
        assert!(matches!(
            file.render_signed_name(selection.name).unwrap(),
            Some(RenderedSignedName::Signed { original, signature })
                if original == expected_name
                    && signature.result == "scala.Boolean"
                    && signature.parameters == [RenderedParamSig::TermParameter("scala.Boolean".into())]
        ));
    }

    if std::env::var_os(RUN_SCALA_ORACLE).is_some() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let candidate = root.join("target/issue-1358/generated/example/Logic.tasty");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        std::fs::write(&candidate, output.bytes()).unwrap();
        let result = Command::new("python3")
            .arg(root.join("tools/tasty-pickler-oracle/run.py"))
            .arg("verify")
            .arg(&candidate)
            .arg("--runtime")
            .env("TMPDIR", root.join("target/issue-1358/tmp"))
            .status()
            .expect("start explicitly requested Scala 3.9 Boolean short-circuit gate");
        assert!(
            result.success(),
            "Boolean Scala interoperability gate failed"
        );
    }
}

#[test]
fn boolean_short_circuit_selection_must_be_registered() {
    let result = source_outputs_named_with_tree_edits(
        LOGIC,
        "Logic.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, store| {
            let source_id = source_ast
                .iter()
                .find_map(|(source_id, tree)| {
                    let TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) = tree.kind else {
                        return None;
                    };
                    (store.names.resolve(infix.op.text()) == "&&").then_some(source_id)
                })
                .unwrap();
            let typed_id = bindings.get(SourceId::from_index(7), source_id).unwrap();
            let TreeKind::Apply(application) = &typed_ast.get(typed_id).kind else {
                panic!("expected typed Boolean application")
            };
            let parameter_type = typed_ast
                .iter()
                .find_map(|(_, tree)| {
                    if !matches!(tree.kind, TreeKind::Ident(_)) {
                        return None;
                    }
                    matches!(
                        store.types.get(tree.ty),
                        Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                            if store.symbols.get(*symbol).kind == SymbolKind::Parameter
                    )
                    .then_some(tree.ty)
                })
                .unwrap();
            typed_ast.get_mut(application.function).ty = parameter_type;
        },
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(
        result,
        PickleError::Reference {
            reason: ReferenceError::NotRegistered,
            ..
        }
    ));
}

#[test]
fn boolean_selection_name_must_match_its_canonical_target() {
    let result = source_outputs_named_with_tree_edits(
        LOGIC,
        "Logic.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, store| {
            let source_operator = |expected: &str| {
                source_ast
                    .iter()
                    .find_map(|(source_id, tree)| {
                        let TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) = tree.kind else {
                            return None;
                        };
                        (store.names.resolve(infix.op.text()) == expected).then_some(source_id)
                    })
                    .unwrap()
            };
            let selected_function = |operator: &str| {
                let source_id = source_operator(operator);
                let typed_id = bindings.get(SourceId::from_index(7), source_id).unwrap();
                let TreeKind::Apply(application) = &typed_ast.get(typed_id).kind else {
                    panic!("expected typed Boolean operator application")
                };
                application.function
            };
            let and_function = selected_function("&&");
            let or_function = selected_function("||");
            let or_name = match &typed_ast.get(or_function).kind {
                TreeKind::Select(selection) => selection.name,
                _ => panic!("expected typed || selection"),
            };
            let TreeKind::Select(and_selection) = &mut typed_ast.get_mut(and_function).kind else {
                panic!("expected typed && selection")
            };
            and_selection.name = or_name;
        },
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(result, PickleError::SourceShape { .. }));
}

#[test]
fn nested_boolean_negations_obey_projection_depth_limit() {
    let source = "package example\nclass NestedNegation:\n  def run(a: Boolean): Boolean = !(!a)\n";
    let error = source_outputs_named_with_tree_edits_and_pickle_limits(
        source,
        "NestedNegation.scala",
        ValidationLimits::default(),
        PickleLimits {
            projection: ProjectionLimits {
                max_expression_depth: 0,
                ..ProjectionLimits::default()
            },
            ..PickleLimits::default()
        },
        |_, _, _, _| {},
        |_| {},
    )
    .unwrap_err();
    assert_eq!(
        error,
        PickleError::LimitExceeded {
            kind: LimitKind::ExpressionDepth,
            limit: 0,
        }
    );
}

#[test]
fn same_spelled_class_operator_does_not_replace_boolean_intrinsic() {
    let source = "package example\nclass CustomLogic:\n  def &&(other: Boolean): Boolean = other\n  def both(a: Boolean, b: Boolean): Boolean = a && b\n";
    let (_, output) = source_outputs_named(source, "CustomLogic.scala", |_| {}).unwrap();
    dotty_tasty::tasty::TastyFile::parse_and_validate_scala_3_9(output.bytes()).unwrap();
}

#[test]
fn canonical_int_comparisons_preserve_boolean_signed_names_and_composition() {
    use dotty_tasty::tasty::*;

    let (_, output) = source_outputs_named(COMPARISONS, "Comparisons.scala", |_| {}).unwrap();
    let (_, repeat) = source_outputs_named(COMPARISONS, "Comparisons.scala", |_| {}).unwrap();
    assert_eq!(output.bytes(), repeat.bytes());
    let file = TastyFile::parse_and_validate_scala_3_9(output.bytes()).unwrap();
    let addresses = file.ast_address_index().unwrap();
    for (index, expected_name) in [(2, ">"), (3, "<"), (4, "<="), (5, ">=")] {
        let body = addresses
            .get(output.declarations()[index].address)
            .unwrap()
            .decode_defdef_body()
            .unwrap();
        let RawTree::LengthNode(application) = body.rhs.unwrap() else {
            panic!("expected comparison APPLY for {expected_name}")
        };
        let application = application.decode_apply().unwrap();
        assert_eq!(application.arguments.len(), 1);
        let RawTree::LengthNode(selection) = application.function else {
            panic!("expected comparison SELECTin for {expected_name}")
        };
        let selection = selection.decode_select_in().unwrap();
        assert_eq!(selection.tag, SELECTIN_TAG);
        assert!(matches!(
            file.render_signed_name(selection.name).unwrap(),
            Some(RenderedSignedName::Signed { original, signature })
                if original == expected_name
                    && signature.result == "scala.Boolean"
                    && signature.parameters == [RenderedParamSig::TermParameter("scala.Int".into())]
        ));
    }
    let nested = addresses
        .get(output.declarations()[6].address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    assert!(matches!(nested.rhs, Some(RawTree::LengthNode(node)) if node.tag == APPLY_TAG));
    let choose = addresses
        .get(output.declarations()[7].address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    let RawTree::LengthNode(conditional) = choose.rhs.unwrap() else {
        panic!("expected comparison condition in IF body")
    };
    let StructuredNode::If(conditional) = conditional.decode_structured().unwrap() else {
        panic!("expected IF body")
    };
    let RawTree::LengthNode(condition) = conditional.condition else {
        panic!("expected comparison APPLY condition")
    };
    assert_eq!(condition.tag, APPLY_TAG);

    if std::env::var_os(RUN_SCALA_ORACLE).is_some() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let candidate = root.join("target/issue-1357/generated/example/Comparisons.tasty");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        std::fs::write(&candidate, output.bytes()).unwrap();
        let result = Command::new("python3")
            .arg(root.join("tools/tasty-pickler-oracle/run.py"))
            .arg("verify")
            .arg(&candidate)
            .arg("--runtime")
            .env("TMPDIR", root.join("target/issue-1357/tmp"))
            .status()
            .expect("start explicitly requested Scala 3.9 comparison gate");
        assert!(
            result.success(),
            "Int comparison Scala interoperability gate failed"
        );
    }
}

#[test]
fn comparison_result_type_must_remain_boolean() {
    let result = source_outputs_named_with_tree_edits(
        COMPARISONS,
        "Comparisons.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, store| {
            let (source_id, _) = source_ast
                .iter()
                .find(|(_, tree)| {
                    matches!(tree.kind, TreeKind::PhaseSpecific(UntypedNode::InfixOp(_)))
                })
                .unwrap();
            let typed_id = bindings.get(SourceId::from_index(7), source_id).unwrap();
            let TreeKind::Apply(application) = &typed_ast.get(typed_id).kind else {
                panic!("expected typed comparison application")
            };
            let function = typed_ast.get(application.function);
            let Type::TermRef {
                target: TermRefTarget::Symbol(target),
                ..
            } = store.types.get(function.ty)
            else {
                panic!("expected selected comparison symbol")
            };
            let SymbolInfo::Complete(signature) = store.symbols.get(*target).info else {
                panic!("expected complete comparison signature")
            };
            let Type::Method(signature) = store.types.get(signature) else {
                panic!("expected comparison method signature")
            };
            typed_ast.get_mut(typed_id).ty = signature.params[0].ty;
        },
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(result, PickleError::BodyType { .. }));
}

#[test]
fn comparison_operands_must_remain_int() {
    let result = source_outputs_named_with_tree_edits(
        COMPARISONS,
        "Comparisons.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, _| {
            let source_id = source_ast
                .iter()
                .find_map(|(source_id, tree)| {
                    matches!(tree.kind, TreeKind::PhaseSpecific(UntypedNode::InfixOp(_)))
                        .then_some(source_id)
                })
                .unwrap();
            let typed_id = bindings.get(SourceId::from_index(7), source_id).unwrap();
            let TreeKind::Apply(application) = &typed_ast.get(typed_id).kind else {
                panic!("expected typed comparison application")
            };
            let argument = application.args[0];
            let comparison_type = typed_ast.get(typed_id).ty;
            typed_ast.get_mut(argument).ty = comparison_type;
        },
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(result, PickleError::BodyType { .. }));
}

#[test]
fn comparison_selection_must_use_a_registered_primitive_symbol() {
    let result = source_outputs_named_with_tree_edits(
        COMPARISONS,
        "Comparisons.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, store| {
            let source_id = source_ast
                .iter()
                .find_map(|(source_id, tree)| {
                    matches!(tree.kind, TreeKind::PhaseSpecific(UntypedNode::InfixOp(_)))
                        .then_some(source_id)
                })
                .unwrap();
            let typed_id = bindings.get(SourceId::from_index(7), source_id).unwrap();
            let TreeKind::Apply(application) = &typed_ast.get(typed_id).kind else {
                panic!("expected typed comparison application")
            };
            let non_operator_reference = typed_ast
                .iter()
                .find_map(|(_, tree)| {
                    if !matches!(tree.kind, TreeKind::Ident(_)) {
                        return None;
                    }
                    let Type::TermRef {
                        target: TermRefTarget::Symbol(symbol),
                        ..
                    } = store.types.get(tree.ty)
                    else {
                        return None;
                    };
                    (store.symbols.get(*symbol).kind == SymbolKind::Parameter).then_some(tree.ty)
                })
                .expect("fixture contains a typed parameter reference");
            typed_ast.get_mut(application.function).ty = non_operator_reference;
        },
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(
        result,
        PickleError::Reference {
            reason: ReferenceError::NotRegistered,
            ..
        }
    ));
}

#[test]
fn nested_application_binding_span_and_repeated_id_failures_keep_exact_errors() {
    let mut expected_binding = None;
    let wrong_binding = source_outputs_named_with_tree_edits(
        OPERATIONS,
        "Operations.scala",
        ValidationLimits::default(),
        |source_ast, _, bindings, _| {
            let (_, typed_outer, source_inner, typed_inner) =
                nested_application_ids(source_ast, bindings);
            expected_binding = Some((source_inner, typed_inner, typed_outer));
            bindings.remove(SourceId::from_index(7), source_inner);
            bindings
                .insert(SourceId::from_index(7), source_inner, typed_outer)
                .unwrap();
        },
        |_| {},
    )
    .unwrap_err();
    let (source_inner, typed_inner, typed_outer) = expected_binding.unwrap();
    assert_eq!(
        wrong_binding,
        PickleError::BodyBinding {
            source: SourceId::from_index(7),
            tree: source_inner,
            expected: typed_inner,
            actual: Some(typed_outer),
        }
    );

    let mut expected_span = None;
    let wrong_span = source_outputs_named_with_tree_edits(
        OPERATIONS,
        "Operations.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, _| {
            let (_, _, source_inner, typed_inner) = nested_application_ids(source_ast, bindings);
            expected_span = Some(typed_inner);
            source_ast.get_mut(source_inner).position = Some(SourceSpan::new(
                SourceId::from_index(7),
                Span::without_point(TextRange::new(0, 1).unwrap()),
            ));
            typed_ast.get_mut(typed_inner).position = Some(SourceSpan::new(
                SourceId::from_index(7),
                Span::without_point(TextRange::new(2, 3).unwrap()),
            ));
        },
        |_| {},
    )
    .unwrap_err();
    assert_eq!(
        wrong_span,
        PickleError::BodySpan {
            tree: expected_span.unwrap(),
        }
    );

    for typed_side in [false, true] {
        let mut span_tree = None;
        let error = source_outputs_named_with_tree_edits(
            OPERATIONS,
            "Operations.scala",
            ValidationLimits::default(),
            |source_ast, typed_ast, bindings, _| {
                let (_, _, source_inner, typed_inner) =
                    nested_application_ids(source_ast, bindings);
                span_tree = Some((source_inner, typed_inner));
                let foreign_span = Some(SourceSpan::new(
                    SourceId::from_index(8),
                    Span::without_point(TextRange::new(0, 1).unwrap()),
                ));
                if typed_side {
                    typed_ast.get_mut(typed_inner).position = foreign_span;
                } else {
                    source_ast.get_mut(source_inner).position = foreign_span;
                }
            },
            |_| {},
        )
        .unwrap_err();
        assert_eq!(
            error,
            PickleError::SourceSpan {
                expected: SourceId::from_index(7),
                actual: SourceId::from_index(8),
            },
            "typed side: {typed_side}"
        );
        assert!(span_tree.is_some());
    }

    let mut repeated_id = None;
    let repeated = source_outputs_named_with_tree_edits(
        OPERATIONS,
        "Operations.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, _| {
            let (_, typed_outer, _, _) = nested_application_ids(source_ast, bindings);
            repeated_id = Some(typed_outer);
            let TreeKind::Apply(outer) = &mut typed_ast.get_mut(typed_outer).kind else {
                panic!("expected outer Apply")
            };
            outer.args[0] = typed_outer;
        },
        |_| {},
    )
    .unwrap_err();
    assert_eq!(
        repeated,
        PickleError::RepeatedExpression {
            tree: repeated_id.unwrap(),
        }
    );
}

#[test]
fn if_validation_rejects_mismatched_children_types_invalid_conditions_stale_ids_and_limits() {
    let swapped = source_outputs_named_with_tree_edits(
        CONDITIONALS,
        "Conditionals.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, _| {
            let (_, typed_if_id, _, typed_if) =
                first_conditional_ids(source_ast, typed_ast, bindings);
            let TreeKind::If(if_expr) = &mut typed_ast.get_mut(typed_if_id).kind else {
                panic!()
            };
            std::mem::swap(&mut if_expr.then_branch, &mut if_expr.else_branch);
            assert_eq!(if_expr.then_branch, typed_if.else_branch);
        },
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(swapped, PickleError::BodyBinding { .. }));

    let invalid_condition = source_outputs_named_with_tree_edits(
        CONDITIONALS,
        "Conditionals.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, _| {
            let (_, _, _, typed_if) = first_conditional_ids(source_ast, typed_ast, bindings);
            let bad_type = typed_ast.get(typed_if.then_branch).ty;
            typed_ast.get_mut(typed_if.cond).ty = bad_type;
        },
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(invalid_condition, PickleError::BodyType { .. }));

    let wrong_branch_type = source_outputs_named_with_tree_edits(
        CONDITIONALS,
        "Conditionals.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, _| {
            let (_, _, _, typed_if) = first_conditional_ids(source_ast, typed_ast, bindings);
            let bad_type = typed_ast.get(typed_if.cond).ty;
            typed_ast.get_mut(typed_if.then_branch).ty = bad_type;
        },
        |_| {},
    )
    .unwrap_err();
    assert!(
        matches!(wrong_branch_type, PickleError::NameMismatch { .. }),
        "{wrong_branch_type:?}"
    );

    let wrong_result = source_outputs_named_with_tree_edits(
        CONDITIONALS,
        "Conditionals.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, _| {
            let (_, typed_if_id, _, typed_if) =
                first_conditional_ids(source_ast, typed_ast, bindings);
            let bad_type = typed_ast.get(typed_if.cond).ty;
            typed_ast.get_mut(typed_if_id).ty = bad_type;
        },
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(wrong_result, PickleError::BodyType { .. }));

    let invalid_source_child = source_outputs_named_with_tree_edits(
        CONDITIONALS,
        "Conditionals.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, _| {
            let (source_if_id, _, _, _) = first_conditional_ids(source_ast, typed_ast, bindings);
            let foreign_index = source_ast.iter().count() as u32;
            let mut foreign = AstArena::<Untyped>::new();
            for _ in 0..=foreign_index {
                foreign.alloc(Tree {
                    kind: TreeKind::This(This { qual: None }),
                    position: None,
                    ty: (),
                });
            }
            let stale = foreign.iter().last().unwrap().0;
            let TreeKind::If(if_expr) = &mut source_ast.get_mut(source_if_id).kind else {
                panic!()
            };
            if_expr.else_branch = stale;
        },
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(
        invalid_source_child,
        PickleError::InvalidSourceTree { .. }
    ));

    let invalid_typed_child = source_outputs_named_with_tree_edits(
        CONDITIONALS,
        "Conditionals.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, _| {
            let (_, typed_if_id, _, _) = first_conditional_ids(source_ast, typed_ast, bindings);
            let foreign_index = typed_ast.iter().count() as u32;
            let condition_type = typed_ast.get(typed_if_id).ty;
            let mut foreign = AstArena::<Typed>::new();
            for _ in 0..=foreign_index {
                foreign.alloc(Tree {
                    kind: TreeKind::This(This { qual: None }),
                    position: None,
                    ty: condition_type,
                });
            }
            let stale = foreign.iter().last().unwrap().0;
            let TreeKind::If(if_expr) = &mut typed_ast.get_mut(typed_if_id).kind else {
                panic!()
            };
            if_expr.else_branch = stale;
        },
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(
        invalid_typed_child,
        PickleError::InvalidTypedTree { .. }
    ));

    let unsupported_branch = source_outputs_named_with_tree_edits(
        CONDITIONALS,
        "Conditionals.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, _| {
            let (_, _, source_if, typed_if) =
                first_conditional_ids(source_ast, typed_ast, bindings);
            source_ast.get_mut(source_if.then_branch).kind = TreeKind::This(This { qual: None });
            typed_ast.get_mut(typed_if.then_branch).kind = TreeKind::This(This { qual: None });
        },
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(
        unsupported_branch,
        PickleError::Unsupported {
            feature: UnsupportedFeature::BodyExpression,
            ..
        }
    ));

    let mut cycle_id = None;
    let cycle = source_outputs_named_with_tree_edits(
        CONDITIONALS,
        "Conditionals.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, _| {
            let (source_if_id, typed_if_id, _, _) =
                first_conditional_ids(source_ast, typed_ast, bindings);
            cycle_id = Some(typed_if_id);
            let TreeKind::If(if_expr) = &mut source_ast.get_mut(source_if_id).kind else {
                panic!()
            };
            if_expr.then_branch = source_if_id;
            let TreeKind::If(if_expr) = &mut typed_ast.get_mut(typed_if_id).kind else {
                panic!()
            };
            if_expr.then_branch = typed_if_id;
        },
        |_| {},
    )
    .unwrap_err();
    assert_eq!(
        cycle,
        PickleError::RepeatedExpression {
            tree: cycle_id.unwrap(),
        }
    );

    let depth_exhausted = source_outputs_named_with_limits(
        CONDITIONALS,
        "Conditionals.scala",
        ValidationLimits {
            max_expression_depth: 0,
            ..ValidationLimits::default()
        },
        |_| {},
    )
    .unwrap_err();
    assert_eq!(
        depth_exhausted,
        PickleError::LimitExceeded {
            kind: LimitKind::ExpressionDepth,
            limit: 0,
        }
    );
}

#[test]
fn nested_if_nodes_consume_validation_item_budget() {
    let minimum_budget = |source: &str| {
        (1..=256)
            .find(|&max_items| {
                source_outputs_named_with_limits(
                    source,
                    "Conditionals.scala",
                    ValidationLimits {
                        max_items,
                        ..ValidationLimits::default()
                    },
                    |_| {},
                )
                .is_ok()
            })
            .expect("conditionals fixture validates within the test budget range")
    };
    let nested = minimum_budget(CONDITIONALS);
    let shallow_source = CONDITIONALS.replace(
        "if a then\n      if b then 1 else 2\n    else 3",
        "if a then 1 else 3",
    );
    let shallow = minimum_budget(&shallow_source);
    assert!(
        nested > shallow,
        "nested If uses no additional items: {nested} / {shallow}"
    );

    assert_eq!(
        source_outputs_named_with_limits(
            CONDITIONALS,
            "Conditionals.scala",
            ValidationLimits {
                max_items: nested - 1,
                ..ValidationLimits::default()
            },
            |_| {},
        )
        .unwrap_err(),
        PickleError::LimitExceeded {
            kind: LimitKind::Items,
            limit: nested - 1,
        }
    );
}

#[test]
fn nested_application_rejects_a_foreign_method_parameter_identity() {
    const FOREIGN_PARAMETER: &str = "package example\nclass Operations:\n  def identity(x: Int): Int = x\n  def foreign(y: Int): Int = identity(identity(y))\n  def current(z: Int): Int = identity(identity(z))\n";

    let mut expected_foreign_parameter = None;
    let error = source_outputs_named_with_tree_edits(
        FOREIGN_PARAMETER,
        "Operations.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, store| {
            let (_, _, source_inner, _) = nested_application_ids(source_ast, bindings);
            let TreeKind::Apply(inner) = &source_ast.get(source_inner).kind else {
                panic!("expected nested source Apply")
            };
            let source_argument = inner.args[0];
            let typed_argument = bindings
                .get(SourceId::from_index(7), source_argument)
                .unwrap();
            let Type::TermRef {
                target: TermRefTarget::Symbol(current_parameter),
                ..
            } = store.types.get(typed_ast.get(typed_argument).ty)
            else {
                panic!("expected a typed parameter reference")
            };
            let foreign_parameter = source_ast
                .iter()
                .filter_map(|(_, tree)| {
                    let TreeKind::Apply(apply) = &tree.kind else {
                        return None;
                    };
                    let source_argument = *apply.args.first()?;
                    if !matches!(source_ast.get(source_argument).kind, TreeKind::Ident(_)) {
                        return None;
                    }
                    let typed_argument = bindings.get(SourceId::from_index(7), source_argument)?;
                    let Type::TermRef {
                        target: TermRefTarget::Symbol(symbol),
                        ..
                    } = store.types.get(typed_ast.get(typed_argument).ty)
                    else {
                        return None;
                    };
                    (*symbol != *current_parameter).then_some((typed_argument, *symbol))
                })
                .next()
                .expect("fixture has a second method parameter");
            expected_foreign_parameter = Some(foreign_parameter.1);
            let foreign_type = typed_ast.get(foreign_parameter.0).ty;
            typed_ast.get_mut(typed_argument).ty = foreign_type;
        },
        |_| {},
    )
    .unwrap_err();
    let foreign_parameter = expected_foreign_parameter.unwrap();
    let PickleError::Owner {
        symbol,
        expected: Some(expected_owner),
        actual: Some(actual_owner),
    } = error
    else {
        panic!("expected a misowned parameter failure, got {error:?}")
    };
    assert_eq!(symbol, foreign_parameter);
    assert_ne!(expected_owner, actual_owner);

    let mut expected_foreign_target = None;
    let error = source_outputs_named_with_tree_edits(
        FOREIGN_PARAMETER,
        "Operations.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, store| {
            let (_, _, source_inner, typed_inner) = nested_application_ids(source_ast, bindings);
            let TreeKind::Apply(source_apply) = &source_ast.get(source_inner).kind else {
                panic!("expected nested source Apply")
            };
            let typed_argument = bindings
                .get(SourceId::from_index(7), source_apply.args[0])
                .unwrap();
            let current_parameter = match store.types.get(typed_ast.get(typed_argument).ty) {
                Type::TermRef {
                    target: TermRefTarget::Symbol(symbol),
                    ..
                } => *symbol,
                other => panic!("expected parameter term reference, got {other:?}"),
            };
            let (foreign_type, foreign_target) = source_ast
                .iter()
                .filter_map(|(_, tree)| {
                    let TreeKind::Apply(apply) = &tree.kind else {
                        return None;
                    };
                    let source_argument = *apply.args.first()?;
                    if !matches!(source_ast.get(source_argument).kind, TreeKind::Ident(_)) {
                        return None;
                    }
                    let typed_argument = bindings.get(SourceId::from_index(7), source_argument)?;
                    let ty = typed_ast.get(typed_argument).ty;
                    let Type::TermRef {
                        target: TermRefTarget::Symbol(symbol),
                        ..
                    } = store.types.get(ty)
                    else {
                        return None;
                    };
                    (*symbol != current_parameter).then_some((ty, *symbol))
                })
                .next()
                .expect("fixture has a second method parameter");
            let TreeKind::Apply(inner) = &typed_ast.get(typed_inner).kind else {
                panic!("expected nested typed Apply")
            };
            let function = inner.function;
            typed_ast.get_mut(function).ty = foreign_type;
            expected_foreign_target = Some(foreign_target);
        },
        |_| {},
    )
    .unwrap_err();
    assert_eq!(
        error,
        PickleError::Reference {
            symbol: expected_foreign_target.unwrap(),
            reason: ReferenceError::NotRegistered,
        }
    );
}

#[test]
fn nested_expression_item_budget_has_a_typed_failure() {
    let minimum_budget = |source: &str| {
        (1..=256)
            .find(|&max_items| {
                source_outputs_named_with_limits(
                    source,
                    "Operations.scala",
                    ValidationLimits {
                        max_items,
                        ..ValidationLimits::default()
                    },
                    |_| {},
                )
                .is_ok()
            })
            .expect("fixture validates within the test budget range")
    };
    let nested_budget = minimum_budget(OPERATIONS);
    let shallow = OPERATIONS.replace("identity(identity(42))", "identity(42)");
    let shallow_budget = minimum_budget(&shallow);
    assert_eq!(nested_budget, shallow_budget + 1);
    assert!(shallow_budget > 0);

    let error = source_outputs_named_with_limits(
        OPERATIONS,
        "Operations.scala",
        ValidationLimits {
            max_items: nested_budget - 1,
            ..ValidationLimits::default()
        },
        |_| {},
    )
    .unwrap_err();
    assert_eq!(
        error,
        PickleError::LimitExceeded {
            kind: LimitKind::Items,
            limit: nested_budget - 1,
        }
    );
    assert!(
        source_outputs_named_with_limits(
            OPERATIONS,
            "Operations.scala",
            ValidationLimits {
                max_items: nested_budget,
                ..ValidationLimits::default()
            },
            |_| {},
        )
        .is_ok()
    );
}

#[test]
fn nested_expression_depth_budget_has_a_typed_failure() {
    let error = source_outputs_named_with_limits(
        OPERATIONS,
        "Operations.scala",
        ValidationLimits {
            max_expression_depth: 0,
            ..ValidationLimits::default()
        },
        |_| {},
    )
    .unwrap_err();
    assert_eq!(
        error,
        PickleError::LimitExceeded {
            kind: LimitKind::ExpressionDepth,
            limit: 0,
        }
    );
}

#[test]
fn if_application_branches_share_depth_and_repeated_node_validation_state() {
    let shallow_if_with_application = "package example\nclass Shallow:
  def choose(flag: Boolean): Int = if flag then identity(1) else 2
  def identity(x: Int): Int = x
";
    assert!(
        source_outputs_named_with_limits(
            shallow_if_with_application,
            "Shallow.scala",
            ValidationLimits {
                max_expression_depth: 0,
                ..ValidationLimits::default()
            },
            |_| {},
        )
        .is_ok()
    );

    let nested_if_with_applications = "package example\nclass Mixed:
  def choose(flag: Boolean): Int = if flag then if flag then identity(identity(1)) else 2 else 3
  def identity(x: Int): Int = x
";
    let depth_error = source_outputs_named_with_limits(
        nested_if_with_applications,
        "Mixed.scala",
        ValidationLimits {
            max_expression_depth: 1,
            ..ValidationLimits::default()
        },
        |_| {},
    )
    .unwrap_err();
    assert_eq!(
        depth_error,
        PickleError::LimitExceeded {
            kind: LimitKind::ExpressionDepth,
            limit: 1,
        }
    );

    let two_application_branches = "package example\nclass Shared:
  def choose(flag: Boolean): Int = if flag then identity(1) else identity(1)
  def identity(x: Int): Int = x
";
    let mut shared_argument = None;
    let repeated = source_outputs_named_with_tree_edits(
        two_application_branches,
        "Shared.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, _| {
            let (_, _, source_if, typed_if) =
                first_conditional_ids(source_ast, typed_ast, bindings);
            let TreeKind::Apply(source_then) = &source_ast.get(source_if.then_branch).kind else {
                panic!("expected an application in the then branch")
            };
            let TreeKind::Apply(source_else) = &source_ast.get(source_if.else_branch).kind else {
                panic!("expected an application in the else branch")
            };
            let TreeKind::Apply(typed_then) = &typed_ast.get(typed_if.then_branch).kind else {
                panic!("expected a typed application in the then branch")
            };
            let shared = typed_then.args[0];
            let TreeKind::Apply(typed_else) = &mut typed_ast.get_mut(typed_if.else_branch).kind
            else {
                panic!("expected a typed application in the else branch")
            };
            shared_argument = Some(shared);
            typed_else.args[0] = shared;
            bindings.remove(SourceId::from_index(7), source_else.args[0]);
            bindings
                .insert(SourceId::from_index(7), source_else.args[0], shared)
                .unwrap();
            assert_ne!(source_then.args[0], source_else.args[0]);
        },
        |_| {},
    )
    .unwrap_err();
    assert_eq!(
        repeated,
        PickleError::RepeatedExpression {
            tree: shared_argument.unwrap(),
        }
    );
}

#[test]
fn local_call_initializer_obeys_expression_depth_limit() {
    let source = "package example\nclass LocalCall:\n  def copy(value: Int): Int = { val saved: Int = identity(value); saved }\n  def identity(value: Int): Int = value\n";

    let depth_zero_error = source_outputs_named_with_limits(
        source,
        "LocalCall.scala",
        ValidationLimits {
            max_expression_depth: 0,
            ..ValidationLimits::default()
        },
        |_| {},
    )
    .unwrap_err();
    assert_eq!(
        depth_zero_error,
        PickleError::LimitExceeded {
            kind: LimitKind::ExpressionDepth,
            limit: 0,
        }
    );

    assert!(
        source_outputs_named_with_limits(
            source,
            "LocalCall.scala",
            ValidationLimits {
                max_expression_depth: 1,
                ..ValidationLimits::default()
            },
            |_| {},
        )
        .is_ok()
    );
}

#[test]
fn multiple_literal_methods_and_empty_clause_match_the_scala_390_oracle() {
    use dotty_tasty::tasty::*;

    let (skeleton, first) = source_outputs_named(MULTI, "Multi.scala", |_| {}).unwrap();
    let (_, second) = source_outputs_named(MULTI, "Multi.scala", |_| {}).unwrap();
    assert_eq!(first.bytes(), second.bytes());
    assert_eq!(first.intended_tasty_path(), "example/Multi.tasty");
    let class = first.declarations()[0].symbol;
    let declaration_ids = first
        .declarations()
        .iter()
        .map(|declaration| declaration.symbol)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(declaration_ids.len(), 5);
    assert_eq!(
        first.declarations()[1..]
            .iter()
            .map(|declaration| declaration.owner)
            .collect::<Vec<_>>(),
        [class; 4]
    );
    assert_eq!(first.declarations().len(), 5);
    assert_eq!(skeleton.declarations().len(), 5);
    assert_eq!(skeleton.omitted_bodies().len(), 3);
    let body_roots = first.declarations()[2..]
        .iter()
        .map(|declaration| first.body(declaration.symbol).unwrap())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(body_roots.len(), 3);
    assert!(
        first.declarations()[2..]
            .windows(2)
            .all(|pair| pair[0].address < pair[1].address)
    );

    let file = TastyFile::parse_and_validate_scala_3_9(first.bytes()).unwrap();
    let addresses = file.ast_address_index().unwrap();
    let observed = first.declarations()[2..]
        .iter()
        .map(|declaration| {
            let method = addresses
                .get(declaration.address)
                .unwrap()
                .decode_defdef_body()
                .unwrap();
            let name = file.render_name(method.name).unwrap();
            let rhs = match method.rhs.unwrap() {
                RawTree::Leaf(leaf) if leaf.tag == INTCONST_TAG => match leaf.value {
                    TermValue::Int(value) => value,
                    other => panic!("unexpected Int constant payload: {other:?}"),
                },
                other => panic!("unexpected literal body: {other:?}"),
            };
            (name.to_owned(), method.clauses, rhs)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        observed,
        [
            ("one".to_owned(), vec![], 1),
            ("two".to_owned(), vec![], 2),
            ("zero".to_owned(), vec![EMPTYCLAUSE_TAG], 0),
        ]
    );
    assert_eq!(
        first.declarations()[2..]
            .iter()
            .map(|declaration| first.body(declaration.symbol).unwrap())
            .collect::<Vec<_>>(),
        skeleton
            .omitted_bodies()
            .iter()
            .map(|body| body.typed_root)
            .collect::<Vec<_>>()
    );

    if std::env::var_os("DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE").is_some() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let candidate = root.join("target/issue-1239/generated/example/Multi.tasty");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        std::fs::write(&candidate, first.bytes()).unwrap();
        let temporary = root.join("target/issue-1239/tmp");
        std::fs::create_dir_all(&temporary).unwrap();
        let result = Command::new("python3")
            .arg(root.join("tools/tasty-pickler-oracle/run.py"))
            .arg("verify")
            .arg(&candidate)
            .env("TMPDIR", temporary)
            .status()
            .expect("start explicitly requested Scala 3.9 -from-tasty gate");
        assert!(
            result.success(),
            "requested Multi Scala interoperability gate failed"
        );
    }
}

#[test]
fn source_int_parameters_emit_ordered_canonical_param_declarations() {
    use dotty_tasty::tasty::*;

    let (skeleton, first) =
        source_outputs_named(PARAMETER_DECL, "ParameterDecl.scala", |_| {}).unwrap();
    let (_, second) = source_outputs_named(PARAMETER_DECL, "ParameterDecl.scala", |_| {}).unwrap();
    assert_eq!(first.bytes(), second.bytes());
    assert_eq!(first.intended_tasty_path(), "example/ParameterDecl.tasty");
    assert_eq!(first.declarations().len(), 5);
    assert_eq!(skeleton.declarations().len(), 5);

    let file = TastyFile::parse_and_validate_scala_3_9(first.bytes()).unwrap();
    let address_index = file.ast_address_index().unwrap();
    let method_declaration = &first.declarations()[2];
    let method = address_index
        .get(method_declaration.address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    assert!(method.clauses.is_empty());
    assert_eq!(
        method
            .parameters
            .iter()
            .map(|parameter| file.render_name(parameter.name()).unwrap())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert!(matches!(
        method.rhs,
        Some(RawTree::Leaf(SimpleTerm {
            tag: INTCONST_TAG,
            value: TermValue::Int(42),
            ..
        }))
    ));

    let parameter_declarations = &first.declarations()[3..];
    assert_ne!(
        parameter_declarations[0].symbol,
        parameter_declarations[1].symbol
    );
    assert!(
        parameter_declarations
            .iter()
            .all(|parameter| parameter.owner == method_declaration.symbol)
    );
    for (declaration, expected_name) in parameter_declarations.iter().zip(["first", "second"]) {
        let raw = address_index.get_node(declaration.address).unwrap();
        assert_eq!(raw.tag, PARAM_TAG);
        let parameter = address_index
            .get(declaration.address)
            .unwrap()
            .decode_parameter()
            .unwrap();
        assert_eq!(file.render_name(parameter.name()).unwrap(), expected_name);
        assert!(matches!(
            parameter.decode_body().unwrap().type_tree,
            RawTree::NatAst {
                tag: IDENTTPT_TAG,
                ..
            }
        ));
    }

    if std::env::var_os("DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE").is_some() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let candidate = root.join("target/issue-1240/generated/example/ParameterDecl.tasty");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        std::fs::write(&candidate, first.bytes()).unwrap();
        let result = Command::new("python3")
            .arg(root.join("tools/tasty-pickler-oracle/run.py"))
            .arg("verify")
            .arg(&candidate)
            .env("TMPDIR", root.join("target/issue-1240/tmp"))
            .status()
            .expect("start explicitly requested Scala 3.9 -from-tasty gate");
        assert!(
            result.success(),
            "requested ParameterDecl Scala interoperability gate failed"
        );
    }
}

#[test]
fn source_parameter_ident_emits_a_direct_reference_to_its_exact_param_declaration() {
    use dotty_tasty::tasty::*;

    let (_, first) = source_outputs_named(PARAMETERS, "Parameters.scala", |_| {}).unwrap();
    let (_, second) = source_outputs_named(PARAMETERS, "Parameters.scala", |_| {}).unwrap();
    assert_eq!(first.bytes(), second.bytes());
    assert_eq!(first.intended_tasty_path(), "example/Parameters.tasty");

    let file = TastyFile::parse_and_validate_scala_3_9(first.bytes()).unwrap();
    let address_index = file.ast_address_index().unwrap();
    let pick = first
        .declarations()
        .iter()
        .find(|declaration| {
            address_index
                .get(declaration.address)
                .and_then(|node| node.decode_defdef_body().ok())
                .is_some_and(|body| file.render_name(body.name).ok().as_deref() == Some("pick"))
        })
        .unwrap();
    let pick_body = address_index
        .get(pick.address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    let parameters = first
        .declarations()
        .iter()
        .filter(|declaration| declaration.owner == pick.symbol)
        .collect::<Vec<_>>();
    assert_eq!(parameters.len(), 2);
    assert!(parameters[0].address < parameters[1].address);
    let names = parameters
        .iter()
        .map(|declaration| {
            let parameter = address_index
                .get(declaration.address)
                .unwrap()
                .decode_parameter()
                .unwrap();
            file.render_name(parameter.name()).unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(names, ["first", "second"]);
    let Some(RawTree::Leaf(reference)) = pick_body.rhs else {
        panic!("formal parameter RHS must be one direct reference leaf")
    };
    assert_eq!(reference.tag, TERMREFDIRECT_TAG);
    assert_eq!(
        reference.ast_ref().map(|reference| reference.address),
        Some(parameters[1].address),
        "the body must reference the second canonical PARAM declaration"
    );

    if std::env::var_os("DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE").is_some() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let candidate = root.join("target/issue-1241/generated/example/Parameters.tasty");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        std::fs::write(&candidate, first.bytes()).unwrap();
        let result = Command::new("python3")
            .arg(root.join("tools/tasty-pickler-oracle/run.py"))
            .arg("verify")
            .arg(&candidate)
            .arg("--runtime")
            .env("TMPDIR", root.join("target/issue-1241/tmp"))
            .status()
            .expect("start explicitly requested Scala 3.9 -from-tasty gate");
        assert!(
            result.success(),
            "requested Parameters Scala interoperability gate failed"
        );
    }
}

#[test]
fn backquoted_class_and_package_components_use_encoded_artifact_paths_only() {
    use dotty_tasty::tasty::*;

    for (package, class, expected) in [
        ("example", "A-B", "example/A$minusB.tasty"),
        ("p-q", "Answer", "p$minusq/Answer.tasty"),
        ("p+q", "A B", "p$plusq/A$u0020B.tasty"),
        ("example", "😀", "example/$uD83D$uDE00.tasty"),
    ] {
        let text = format!("package `{package}`; class `{class}` {{ def value: Int = 42 }}");
        let skeleton = source_plan(&text, |_| {}).unwrap();
        assert_eq!(skeleton.intended_tasty_path(), expected);
        let file = TastyFile::parse_and_validate_scala_3_9(skeleton.provisional_bytes()).unwrap();
        let index = file.ast_address_index().unwrap();
        let definition = index
            .get(skeleton.declarations()[0].address)
            .unwrap()
            .decode_definition()
            .unwrap();
        assert_eq!(file.render_name(definition.name()).unwrap(), class);
        let roots = file.asts().unwrap();
        let wire_package = roots.get(0).unwrap().decode_package().unwrap();
        let RawTree::Leaf(path) = wire_package.path else {
            panic!()
        };
        assert_eq!(file.render_name(path.name_ref().unwrap()).unwrap(), package);
    }
}

#[test]
fn a_second_independent_source_session_validates_without_cross_session_identity_assumptions() {
    source_plan(ANSWER, |_| {}).unwrap();
    source_plan(ANSWER, |_| {}).unwrap();
}

#[test]
fn real_source_default_constructor_clauses_normalize_without_fabricating_method_parens() {
    let implicit = source_plan(ANSWER, |_| {}).unwrap();
    let explicit =
        source_plan(&ANSWER.replace("class Answer:", "class Answer():"), |_| {}).unwrap();
    assert_eq!(implicit.provisional_bytes(), explicit.provisional_bytes());
}

#[test]
fn contextual_and_type_parameter_methods_remain_explicitly_unsupported() {
    for (text, feature) in [
        (
            "package example; class Answer { def value(using x: Int): Int = x }",
            UnsupportedFeature::SourceMetadata,
        ),
        (
            "package example; class Answer { def value[A]: Int = 42 }",
            UnsupportedFeature::TypeParameters,
        ),
    ] {
        let result = source_plan(text, |_| {});
        assert!(
            matches!(
                &result,
                Err(PickleError::Unsupported { feature: actual, .. }) if *actual == feature
            ),
            "{text}: {result:?}"
        );
    }
}

#[test]
fn unsupported_parameter_kinds_and_defaults_fail_before_emission() {
    for (text, feature) in [
        (
            "package example; class Answer { def value(x: => Int): Int = 42 }",
            UnsupportedFeature::ParameterClauses,
        ),
        (
            "package example; class Answer { def value(x: Int = 1): Int = 42 }",
            UnsupportedFeature::ParameterClauses,
        ),
        (
            "package example; class Answer { def value(x: Int)(y: Int): Int = 42 }",
            UnsupportedFeature::ParameterClauses,
        ),
    ] {
        let result = source_plan(text, |_| {});
        assert!(
            matches!(
                &result,
                Err(PickleError::Unsupported { feature: actual, .. }) if *actual == feature
            ),
            "{text}: {result:?}"
        );
    }
}

#[test]
fn malformed_clause_shapes_and_empty_class_fail_explicitly() {
    for text in [
        "package example; class Answer { def value()(): Int = 42 }",
        "package example; class Answer { def value()(x: Int): Int = x }",
    ] {
        assert!(matches!(
            source_plan(text, |_| {}),
            Err(PickleError::Unsupported {
                feature: UnsupportedFeature::ParameterClauses,
                ..
            })
        ));
    }
    assert!(matches!(
        source_plan("package example; class Answer {}", |_| {}),
        Err(PickleError::Unsupported {
            feature: UnsupportedFeature::MethodCount,
            ..
        })
    ));
}

#[test]
fn missing_real_frontend_binding_is_an_error_without_mutating_the_pipeline() {
    fn missing(_: SourceId, _: TreeId<Untyped>) -> Option<TreeId<Typed>> {
        None
    }
    assert!(matches!(
        source_plan(ANSWER, |plan| plan.typed_bindings = &missing),
        Err(PickleError::BodyBinding { actual: None, .. })
    ));
}

#[test]
fn valid_source_fields_are_unsupported_rather_than_malformed_method_order() {
    assert!(matches!(
        source_plan(
            "package example; class Answer { val field: Int = 0; def value: Int = 42 }",
            |_| {}
        ),
        Err(PickleError::Unsupported {
            feature: UnsupportedFeature::ClassMembers,
            ..
        })
    ));
}

#[test]
fn valid_source_traits_are_unsupported_rather_than_invalid_class_identities() {
    assert!(matches!(
        source_plan(
            "package example; trait Answer { def value: Int = 42 }",
            |_| {}
        ),
        Err(PickleError::Unsupported {
            feature: UnsupportedFeature::SourceMetadata,
            ..
        })
    ));
}

#[test]
fn real_typed_rhs_values_are_serialized_without_a_synthetic_method_tree() {
    for spelling in [
        "0",
        "7",
        "2147483647",
        "0x80000000",
        "0xffffffff",
        "0b101010",
    ] {
        source_outputs(&ANSWER.replace("42", spelling), |_| {}).unwrap();
    }
}

#[test]
fn complete_source_output_is_deterministic_across_actual_frontend_sessions() {
    let (_, first) = source_outputs(ANSWER, |_| {}).unwrap();
    let (_, second) = source_outputs(ANSWER, |_| {}).unwrap();
    assert_eq!(first.bytes(), second.bytes());
}

#[test]
fn mutable_int_locals_serialize_ordered_assignments_and_exact_local_targets() {
    use dotty_tasty::tasty::*;

    let (_, output) = source_outputs_named(MUTABLE, "Mutable.scala", |_| {}).unwrap();
    let file = TastyFile::parse_and_validate_scala_3_9(output.bytes()).unwrap();
    let addresses = file.ast_address_index().unwrap();
    for (method_index, expected_assignments) in [(2, 1), (3, 1), (4, 2), (5, 1)] {
        let method = addresses
            .get(output.declarations()[method_index].address)
            .unwrap()
            .decode_defdef_body()
            .unwrap();
        let RawTree::LengthNode(block_node) = method.rhs.unwrap() else {
            panic!("mutable method body must retain its source block")
        };
        let StructuredNode::Block(block) = block_node.decode_structured().unwrap() else {
            panic!("mutable method body must be a BLOCK")
        };
        assert_eq!(block.stats.len(), expected_assignments + 1);
        let RawTree::LengthNode(value_node) = &block.stats[0] else {
            panic!("first source statement must be the local VALDEF")
        };
        assert_eq!(value_node.tag, VALDEF_TAG);
        let DefinitionBody::ValDef { name, tail, .. } =
            value_node.decode_definition_body().unwrap()
        else {
            panic!("local declaration did not decode as VALDEF")
        };
        assert_eq!(tail, [DefinitionTail::Modifier(MUTABLE_TAG)]);
        assert_eq!(file.render_name(name).unwrap(), "count");

        for statement in &block.stats[1..] {
            let RawTree::LengthNode(assign_node) = statement else {
                panic!("assignment statement was dropped or reordered")
            };
            assert_eq!(assign_node.tag, ASSIGN_TAG);
            let assign = assign_node.decode_assign().unwrap();
            let RawTree::Leaf(lhs) = assign.left else {
                panic!("assignment LHS must be a direct local reference")
            };
            assert_eq!(lhs.tag, TERMREFDIRECT_TAG);
            let target = addresses.get(lhs.ast_ref().unwrap().address).unwrap();
            let DefinitionBody::ValDef {
                name: target_name, ..
            } = target.decode_definition_body().unwrap()
            else {
                panic!("assignment target must point to its local VALDEF")
            };
            assert_eq!(target_name, name);
        }
        let RawTree::Leaf(result) = block.expression else {
            panic!("block result must read the mutated local")
        };
        assert_eq!(result.tag, TERMREFDIRECT_TAG);
        let target = addresses.get(result.ast_ref().unwrap().address).unwrap();
        let DefinitionBody::ValDef {
            name: result_name, ..
        } = target.decode_definition_body().unwrap()
        else {
            panic!("block result target must be its local VALDEF")
        };
        assert_eq!(result_name, name);
    }

    let (_, repeated) = source_outputs_named(MUTABLE, "Mutable.scala", |_| {}).unwrap();
    assert_eq!(output.bytes(), repeated.bytes());
    assert_eq!(file.encode_validated().unwrap(), output.bytes());

    if std::env::var_os(RUN_SCALA_ORACLE).is_some() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let candidate = root.join("target/issue-1359/generated/example/Mutable.tasty");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        std::fs::write(&candidate, output.bytes()).unwrap();
        let result = Command::new("python3")
            .arg(root.join("tools/tasty-pickler-oracle/run.py"))
            .arg("verify")
            .arg(&candidate)
            .arg("--runtime")
            .env("TMPDIR", root.join("target/issue-1359/tmp"))
            .status()
            .expect("start explicitly requested Scala 3.9 mutable-local interoperability gate");
        assert!(
            result.success(),
            "Mutable Scala interoperability gate failed"
        );
    }
}

#[test]
fn mutable_assignment_rhs_obeys_projection_expression_depth_after_validation() {
    let limits = PickleLimits {
        projection: ProjectionLimits {
            max_expression_depth: 0,
            ..ProjectionLimits::default()
        },
        ..PickleLimits::default()
    };
    let error = source_outputs_named_with_tree_edits_and_pickle_limits(
        MUTABLE,
        "Mutable.scala",
        ValidationLimits::default(),
        limits,
        |_, _, _, _| {},
        |_| {},
    )
    .unwrap_err();
    assert_eq!(
        error,
        PickleError::LimitExceeded {
            kind: LimitKind::ExpressionDepth,
            limit: 0,
        }
    );

    let nested_block = "package example\nclass NestedAssignment:\n  def run: Int =\n    var count: Int = 0\n    count = { count = 1; count + 1 }\n    count\n";
    let error = source_outputs_named_with_tree_edits_and_pickle_limits(
        nested_block,
        "NestedAssignment.scala",
        ValidationLimits::default(),
        limits,
        |_, _, _, _| {},
        |_| {},
    )
    .unwrap_err();
    assert_eq!(
        error,
        PickleError::LimitExceeded {
            kind: LimitKind::ExpressionDepth,
            limit: 0,
        }
    );
}

#[test]
fn typed_while_loops_serialize_condition_and_unit_body_in_oracle_order() {
    use dotty_tasty::tasty::*;

    let (_, output) = source_outputs_named(LOOP, "Loop.scala", |_| {}).unwrap();
    let file = TastyFile::parse_and_validate_scala_3_9(output.bytes()).unwrap();
    let addresses = file.ast_address_index().unwrap();
    let method = addresses
        .get(output.declarations()[2].address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    let RawTree::LengthNode(outer_block) = method.rhs.unwrap() else {
        panic!("sumTo body must be a BLOCK")
    };
    let StructuredNode::Block(outer_block) = outer_block.decode_structured().unwrap() else {
        panic!("sumTo body must decode as BLOCK")
    };
    assert_eq!(outer_block.stats.len(), 3);
    let local_names = outer_block.stats[..2]
        .iter()
        .map(|stat| {
            let RawTree::LengthNode(value) = stat else {
                panic!("loop locals must be VALDEF nodes")
            };
            assert_eq!(value.tag, VALDEF_TAG);
            let DefinitionBody::ValDef { name, .. } = value.decode_definition_body().unwrap()
            else {
                panic!("loop local must decode as VALDEF")
            };
            file.render_name(name).unwrap().to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(local_names, ["i", "total"]);
    let RawTree::LengthNode(while_node) = &outer_block.stats[2] else {
        panic!("while statement must be retained as a length node")
    };
    assert_eq!(while_node.tag, WHILE_TAG);
    let while_node = while_node.decode_while().unwrap();
    let RawTree::LengthNode(condition) = while_node.condition else {
        panic!("while condition must retain the typed comparison")
    };
    assert_eq!(condition.tag, APPLY_TAG);
    let RawTree::LengthNode(body) = while_node.body else {
        panic!("while body must retain its source block")
    };
    assert_eq!(body.tag, BLOCK_TAG);
    let StructuredNode::Block(body) = body.decode_structured().unwrap() else {
        panic!("while body must decode as BLOCK")
    };
    assert_eq!(body.stats.len(), 1);
    let RawTree::LengthNode(first_assignment) = &body.stats[0] else {
        panic!("first loop assignment must remain a statement")
    };
    let RawTree::LengthNode(last_assignment) = &body.expression else {
        panic!("last loop assignment must remain the block result")
    };
    assert_eq!(first_assignment.tag, ASSIGN_TAG);
    assert_eq!(last_assignment.tag, ASSIGN_TAG);
    let assignment_targets = [first_assignment, last_assignment]
        .into_iter()
        .map(|assignment| {
            let assignment = assignment.decode_assign().unwrap();
            let RawTree::Leaf(target) = assignment.left else {
                panic!("loop assignment target must be a direct local reference")
            };
            assert_eq!(target.tag, TERMREFDIRECT_TAG);
            let definition = addresses.get(target.ast_ref().unwrap().address).unwrap();
            let DefinitionBody::ValDef { name, .. } = definition.decode_definition_body().unwrap()
            else {
                panic!("loop assignment target must point to its VALDEF")
            };
            file.render_name(name).unwrap().to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(assignment_targets, ["total", "i"]);

    if std::env::var_os(RUN_SCALA_ORACLE).is_some() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let candidate = root.join("target/issue-1360/generated/example/Loop.tasty");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        std::fs::write(&candidate, output.bytes()).unwrap();
        let result = Command::new("python3")
            .arg(root.join("tools/tasty-pickler-oracle/run.py"))
            .arg("verify")
            .arg(&candidate)
            .arg("--runtime")
            .env("TMPDIR", root.join("target/issue-1360/tmp"))
            .status()
            .expect("start pinned Scala 3.9 while-loop interoperability gate");
        assert!(result.success(), "Loop Scala interoperability gate failed");
    }
}

#[test]
fn typed_while_accepts_boolean_formals_and_constant_unit_bodies() {
    use dotty_tasty::tasty::*;

    let (_, output) = source_outputs_named(LOOP, "Loop.scala", |_| {}).unwrap();
    let file = TastyFile::parse_and_validate_scala_3_9(output.bytes()).unwrap();
    let addresses = file.ast_address_index().unwrap();

    let run = addresses
        .get(output.declarations()[3].address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    let RawTree::LengthNode(run_block) = run.rhs.unwrap() else {
        panic!("run body must retain its local block")
    };
    let StructuredNode::Block(run_block) = run_block.decode_structured().unwrap() else {
        panic!("run body must decode as BLOCK")
    };
    let RawTree::LengthNode(run_while) = &run_block.stats[1] else {
        panic!("run block must retain its while statement")
    };
    let run_while = run_while.decode_while().unwrap();
    let RawTree::Leaf(flag) = run_while.condition else {
        panic!("Boolean formal condition must be a direct parameter reference")
    };
    assert_eq!(flag.tag, TERMREFDIRECT_TAG);

    let empty = addresses
        .get(output.declarations()[4].address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    let RawTree::LengthNode(empty_block) = empty.rhs.unwrap() else {
        panic!("empty Unit loop method must retain its result block")
    };
    let StructuredNode::Block(empty_block) = empty_block.decode_structured().unwrap() else {
        panic!("empty Unit loop method must decode as BLOCK")
    };
    let RawTree::LengthNode(empty_while) = &empty_block.stats[0] else {
        panic!("empty Unit loop body must remain a WHILE node")
    };
    let empty_while = empty_while.decode_while().unwrap();
    let RawTree::Leaf(unit) = empty_while.body else {
        panic!("empty Unit loop body must retain its Unit literal")
    };
    assert_eq!(unit.tag, UNITCONST_TAG);

    let assigned = addresses
        .get(output.declarations()[5].address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    let RawTree::LengthNode(assigned_block) = assigned.rhs.unwrap() else {
        panic!("assignment loop method must retain its result block")
    };
    let StructuredNode::Block(assigned_block) = assigned_block.decode_structured().unwrap() else {
        panic!("assignment loop method must decode as BLOCK")
    };
    let RawTree::LengthNode(assigned_while) = &assigned_block.stats[1] else {
        panic!("assignment loop body must remain a WHILE node")
    };
    let assigned_while = assigned_while.decode_while().unwrap();
    let RawTree::LengthNode(body) = assigned_while.body else {
        panic!("assignment followed by Unit must retain its body BLOCK")
    };
    let StructuredNode::Block(body) = body.decode_structured().unwrap() else {
        panic!("assignment followed by Unit must decode as BLOCK")
    };
    assert_eq!(body.stats.len(), 1);
    let RawTree::Leaf(unit) = body.expression else {
        panic!("Unit block result must remain a Unit literal")
    };
    assert_eq!(unit.tag, UNITCONST_TAG);
}

#[test]
fn while_validation_rejects_forged_types_swapped_body_bindings_and_depth_exhaustion() {
    let non_boolean = source_outputs_named_with_tree_edits(
        LOOP,
        "Loop.scala",
        ValidationLimits::default(),
        |_, typed_ast, _, _| {
            let int_ty = typed_ast
                .iter()
                .find_map(|(_, tree)| {
                    matches!(
                        tree.kind,
                        TreeKind::Literal(Literal {
                            value: Constant::Int(_)
                        })
                    )
                    .then_some(tree.ty)
                })
                .unwrap();
            let (_, while_tree) = typed_ast
                .iter()
                .find(|(_, tree)| matches!(tree.kind, TreeKind::While(_)))
                .unwrap();
            let TreeKind::While(while_expr) = while_tree.kind else {
                unreachable!()
            };
            typed_ast.get_mut(while_expr.cond).ty = int_ty;
        },
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(non_boolean, PickleError::BodyType { .. }));

    let non_unit = source_outputs_named_with_tree_edits(
        LOOP,
        "Loop.scala",
        ValidationLimits::default(),
        |_, typed_ast, _, _| {
            let int_ty = typed_ast
                .iter()
                .find_map(|(_, tree)| {
                    matches!(
                        tree.kind,
                        TreeKind::Literal(Literal {
                            value: Constant::Int(_)
                        })
                    )
                    .then_some(tree.ty)
                })
                .unwrap();
            let (while_id, _) = typed_ast
                .iter()
                .find(|(_, tree)| matches!(tree.kind, TreeKind::While(_)))
                .unwrap();
            typed_ast.get_mut(while_id).ty = int_ty;
        },
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(non_unit, PickleError::BodyType { .. }));

    let swapped_body = source_outputs_named_with_tree_edits(
        LOOP,
        "Loop.scala",
        ValidationLimits::default(),
        |_, typed_ast, _, _| {
            let (_, while_tree) = typed_ast
                .iter()
                .find(|(_, tree)| matches!(tree.kind, TreeKind::While(_)))
                .unwrap();
            let TreeKind::While(while_expr) = while_tree.kind else {
                unreachable!()
            };
            let TreeKind::Block(body) = &mut typed_ast.get_mut(while_expr.body).kind else {
                panic!("expected a source block as the loop body")
            };
            body.stats[0] = body.expr;
        },
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(swapped_body, PickleError::BodyBinding { .. }));

    let depth = source_outputs_named_with_limits(
        LOOP,
        "Loop.scala",
        ValidationLimits {
            max_expression_depth: 0,
            ..ValidationLimits::default()
        },
        |_| {},
    )
    .unwrap_err();
    assert_eq!(
        depth,
        PickleError::LimitExceeded {
            kind: LimitKind::ExpressionDepth,
            limit: 0,
        }
    );

    let projection_depth = source_outputs_named_with_tree_edits_and_pickle_limits(
        LOOP,
        "Loop.scala",
        ValidationLimits::default(),
        PickleLimits {
            projection: ProjectionLimits {
                max_expression_depth: 0,
                ..ProjectionLimits::default()
            },
            ..PickleLimits::default()
        },
        |_, _, _, _| {},
        |_| {},
    )
    .unwrap_err();
    assert_eq!(
        projection_depth,
        PickleError::LimitExceeded {
            kind: LimitKind::ExpressionDepth,
            limit: 0,
        }
    );

    let unsupported_source = LOOP.replace(
        "      total = total + i\n",
        "      i + 1\n      total = total + i\n",
    );
    let unsupported = source_outputs_named(&unsupported_source, "Loop.scala", |_| {}).unwrap_err();
    assert!(matches!(
        unsupported,
        PickleError::Unsupported {
            feature: UnsupportedFeature::BodyExpression,
            ..
        }
    ));

    let missing_result = source_outputs_named_with_tree_edits(
        LOOP,
        "Loop.scala",
        ValidationLimits::default(),
        |_, typed_ast, _, _| {
            let (while_id, _) = typed_ast
                .iter()
                .find(|(_, tree)| matches!(tree.kind, TreeKind::While(_)))
                .unwrap();
            let (outer_block_id, _) = typed_ast
                .iter()
                .find(|(_, tree)| matches!(&tree.kind, TreeKind::Block(block) if block.stats.len() == 3))
                .unwrap();
            let TreeKind::Block(block) = &mut typed_ast.get_mut(outer_block_id).kind else {
                unreachable!()
            };
            block.expr = while_id;
        },
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(missing_result, PickleError::BodyType { .. }));
}

#[test]
fn mutable_local_assignments_reject_forged_flags_swapped_children_and_non_unit_results() {
    let forged_source_modifier = source_outputs_named_with_tree_edits(
        MUTABLE,
        "Mutable.scala",
        ValidationLimits::default(),
        |source_ast, _, _, _| {
            let (tree, mut value) = source_ast
                .iter()
                .find_map(|(id, tree)| match &tree.kind {
                    TreeKind::ValDef(value)
                        if value.metadata.modifiers == [dotty_core::ast::Modifier::Var] =>
                    {
                        Some((id, value.clone()))
                    }
                    _ => None,
                })
                .unwrap();
            value.metadata.modifiers.clear();
            source_ast.get_mut(tree).kind = TreeKind::ValDef(value);
        },
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(
        forged_source_modifier,
        PickleError::Unsupported {
            feature: UnsupportedFeature::SymbolMetadata,
            ..
        }
    ));

    let swapped_children = source_outputs_named_with_tree_edits(
        MUTABLE,
        "Mutable.scala",
        ValidationLimits::default(),
        |_, typed_ast, _, _| {
            let typed_assign = typed_ast
                .iter()
                .find_map(|(id, tree)| matches!(tree.kind, TreeKind::Assign(_)).then_some(id))
                .unwrap();
            let TreeKind::Assign(assign) = &mut typed_ast.get_mut(typed_assign).kind else {
                unreachable!()
            };
            std::mem::swap(&mut assign.lhs, &mut assign.rhs);
        },
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(swapped_children, PickleError::BodyBinding { .. }));

    let non_unit_assignment = source_outputs_named_with_tree_edits(
        MUTABLE,
        "Mutable.scala",
        ValidationLimits::default(),
        |_, typed_ast, _, _| {
            let assign = typed_ast
                .iter()
                .find_map(|(id, tree)| matches!(tree.kind, TreeKind::Assign(_)).then_some(id))
                .unwrap();
            let TreeKind::Assign(value) = &typed_ast.get(assign).kind else {
                unreachable!()
            };
            let rhs_type = typed_ast.get(value.rhs).ty;
            typed_ast.get_mut(assign).ty = rhs_type;
        },
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(non_unit_assignment, PickleError::BodyType { .. }));

    let assignment_to_parameter = source_outputs_named_with_tree_edits(
        MUTABLE,
        "Mutable.scala",
        ValidationLimits::default(),
        |_, typed_ast, _, store| {
            let parameter_ty = typed_ast
                .iter()
                .find_map(|(_, tree)| match store.types.try_get(tree.ty) {
                    Some(Type::TermRef {
                        target: TermRefTarget::Symbol(symbol),
                        ..
                    }) if store.symbols.get(*symbol).kind == SymbolKind::Parameter => Some(tree.ty),
                    _ => None,
                })
                .unwrap();
            let assign = typed_ast
                .iter()
                .find_map(|(id, tree)| matches!(tree.kind, TreeKind::Assign(_)).then_some(id))
                .unwrap();
            let TreeKind::Assign(value) = &typed_ast.get(assign).kind else {
                unreachable!()
            };
            typed_ast.get_mut(value.lhs).ty = parameter_ty;
        },
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(
        assignment_to_parameter,
        PickleError::Reference { .. }
    ));

    let assignment_to_immutable_local = source_outputs_named_with_tree_edits(
        MUTABLE,
        "Mutable.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, store| {
            let source_method = source_ast
                .iter()
                .find_map(|(_, tree)| match &tree.kind {
                    TreeKind::DefDef(definition)
                        if store.names.try_resolve(definition.name.as_name().text())
                            == Some("immutableTarget") =>
                    {
                        Some(definition.rhs.unwrap())
                    }
                    _ => None,
                })
                .unwrap();
            let TreeKind::Block(block) = &source_ast.get(source_method).kind else {
                unreachable!()
            };
            let source_assignment = block
                .stats
                .iter()
                .copied()
                .find(|stat| matches!(source_ast.get(*stat).kind, TreeKind::Assign(_)))
                .unwrap();
            let typed_assignment = bindings
                .get(SourceId::from_index(7), source_assignment)
                .unwrap();
            let TreeKind::Assign(assignment) = &typed_ast.get(typed_assignment).kind else {
                unreachable!()
            };
            let immutable_type = typed_ast
                .iter()
                .find_map(
                    |(_, tree)| match (&tree.kind, store.types.try_get(tree.ty)) {
                        (
                            TreeKind::Ident(_),
                            Some(Type::TermRef {
                                target: TermRefTarget::Symbol(symbol),
                                ..
                            }),
                        ) if store.symbols.get(*symbol).kind == SymbolKind::Local
                            && store.symbols.get(*symbol).flags.is_empty() =>
                        {
                            Some(tree.ty)
                        }
                        _ => None,
                    },
                )
                .unwrap();
            typed_ast.get_mut(assignment.lhs).ty = immutable_type;
        },
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(
        assignment_to_immutable_local,
        PickleError::Reference { .. }
    ));

    let assignment_before_declaration = source_outputs_named_with_tree_edits(
        MUTABLE,
        "Mutable.scala",
        ValidationLimits::default(),
        |source_ast, typed_ast, bindings, _| {
            let source_block_id = source_ast
                .iter()
                .find_map(|(id, tree)| {
                    let TreeKind::Block(block) = &tree.kind else {
                        return None;
                    };
                    (block.stats.len() >= 2
                        && block
                            .stats
                            .iter()
                            .any(|stat| matches!(source_ast.get(*stat).kind, TreeKind::Assign(_))))
                    .then_some(id)
                })
                .unwrap();
            let typed_block_id = bindings
                .get(SourceId::from_index(7), source_block_id)
                .unwrap();
            let TreeKind::Block(block) = &mut source_ast.get_mut(source_block_id).kind else {
                unreachable!()
            };
            block.stats.swap(0, 1);
            let TreeKind::Block(block) = &mut typed_ast.get_mut(typed_block_id).kind else {
                unreachable!()
            };
            block.stats.swap(0, 1);
        },
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(
        assignment_before_declaration,
        PickleError::Reference { .. }
    ));
}
