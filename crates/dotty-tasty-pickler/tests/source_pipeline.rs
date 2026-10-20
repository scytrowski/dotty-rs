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
const MULTI: &str = include_str!("../../../tests/fixtures/tasty_pickler/Multi.scala");
const PARAMETERS: &str = include_str!("../../../tests/fixtures/tasty_pickler/Parameters.scala");
const PARAMETER_DECL: &str =
    include_str!("../../../tests/fixtures/tasty_pickler/ParameterDecl.scala");
const REFERENCES: &str = include_str!("../../../tests/fixtures/tasty_pickler/References.scala");
const EXPLICIT_THIS_REFERENCES: &str =
    "package example\nclass References:\n  def first: Int = this.second\n  def second: Int = 42\n";
const OPERATIONS: &str = "package example\nclass Operations:\n  def use: Int = identity(42)\n  def identity(x: Int): Int = x\n  def propagate(x: Int): Int = identity(x)\n  def explicit: Int = this.identity(42)\n  def pick(first: Int, second: Int): Int = second\n  def picks: Int = pick(11, 42)\n  def nested: Int = identity(identity(42))\n";
const ARITHMETIC: &str = "package example\nclass Arithmetic:\n  def add(x: Int, y: Int): Int = x + y\n  def subtract(x: Int, y: Int): Int = x - y\n  def multiply(x: Int, y: Int): Int = x * y\n  def divide(x: Int, y: Int): Int = x / y\n  def remainder(x: Int, y: Int): Int = x % y\n  def nested(x: Int, y: Int, z: Int): Int = x + y * z\n  def throughCall(x: Int, y: Int): Int = identity(x + y)\n  def local(x: Int, y: Int): Int =\n    val result: Int = x + y\n    result\n  def identity(value: Int): Int = value\n";
const FLAGS: &str = "package example\nclass Flags:\n  def enabled: Boolean = true\n  def disabled: Boolean = false\n  def passthrough(flag: Boolean): Boolean = flag\n  def keep(flag: Boolean, value: Int): Int = value\n";
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
    let forbidden_local =
        source_outputs_named(mutable_local, "LocalBlock.scala", |_| {}).unwrap_err();
    assert!(matches!(
        forbidden_local,
        PickleError::Unsupported {
            feature: UnsupportedFeature::SourceMetadata,
            ..
        }
    ));
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
            parameter_clauses: clauses(constructor_definition),
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
    let output = checked.pickle()?;
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
fn comparisons_remain_outside_the_int_arithmetic_increment() {
    let source =
        "package example\nclass Comparison:\n  def less(x: Int, y: Int): Boolean = x < y\n";
    assert!(matches!(
        source_outputs_named(source, "Comparison.scala", |_| {}),
        Err(PickleError::BodyType { .. })
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
