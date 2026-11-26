#[test]
fn pinned_oracle_records_method_targets_and_unit_returns() {
    let oracle = include_str!("fixtures/jvm_tasty_local_returns/oracle.txt");
    for shape in ["RETURN(", "UNITconst", "[direct]", "[matched]", "[inLoop]"] {
        assert!(oracle.contains(shape), "missing {shape}");
    }
}

use dotty::core::SourceId;
use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_class;

fn emit(methods: &str) -> dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts {
    emit_paired_jvm_tasty_class(
        &format!("package example; class EarlyReturns {{ {methods} }}"),
        "EarlyReturns.scala",
        SourceId::from_index(2472),
    )
    .unwrap_or_else(|error| panic!("{methods}: {error:?}"))
}

#[test]
fn primitive_returns_target_the_exact_enclosing_method_declaration() {
    let pair = emit(
        "def direct(x: Int): Int = return x; def booleanValue(x: Boolean): Boolean = return x; def unit(): Unit = return",
    );
    let tasty =
        dotty_tasty::tasty::TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
    let index = tasty.ast_address_index().unwrap();
    let methods: Vec<_> = index
        .iter()
        .filter(|node| node.tag == dotty_tasty::tasty::DEFDEF_TAG)
        .collect();
    let returns: Vec<_> = index
        .iter()
        .filter(|node| node.tag == dotty_tasty::tasty::RETURN_TAG)
        .collect();
    assert_eq!(returns.len(), 3);
    for returned in returns {
        let target = returned.decode_return().unwrap().target;
        let enclosing = methods
            .iter()
            .filter(|method| method.offset < returned.offset)
            .max_by_key(|method| method.offset)
            .unwrap();
        assert_eq!(target.address as usize, enclosing.offset);
        assert_eq!(target.kind, dotty_tasty::tasty::AstRefKind::ReturnTarget);
        assert!(returned.decode_return().unwrap().expression.is_some());
    }
}

#[test]
fn returning_branches_compose_with_blocks_loops_and_match_binders() {
    for method in [
        "def f(x: Int): Int = { if x < 0 then return 0; x + 1 }",
        "def f(x: Int): Int = if x < 0 then return 0 else return x",
        "def f(x: Boolean): Boolean = if x then return false else true",
        "def f(x: Boolean): Unit = if x then return else ()",
        "def f(x: Int): Int = x match { case n if n < 0 => return n; case n => n + 1 }",
        "def f(x: Int): Int = { var n: Int = x; while n > 0 do { if n == 3 then return n; n = n - 1 }; 0 }",
        "def f(): Int = { return 1; 2 }",
    ] {
        emit(method);
    }
}

#[test]
fn malformed_return_targets_and_types_are_rejected_read_only() {
    use dotty::core::{
        Typed,
        ast::{AstArena, TreeKind},
    };
    use dotty_tasty_pickler::{ClassStatePicklePlan, PickleError, PrimaryConstructorInputPlan};
    let source = "package example; class EarlyReturns { def direct(x: Int): Int = return x }";
    dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(
        source,
        "EarlyReturns.scala",
        SourceId::from_index(2472),
        |plan| {
            let raw = plan.tasty_plan.plan();
            let returned = raw
                .typed_ast
                .iter()
                .find(|(_, node)| matches!(node.kind, TreeKind::Return(_)))
                .unwrap()
                .0;
            let checkpoint = raw.store.checkpoint();
            for mode in 0..3 {
                let mut ast = AstArena::<Typed>::new();
                for (id, node) in raw.typed_ast.iter() {
                    assert_eq!(id, ast.alloc(node.clone()));
                }
                let node = ast.get_mut(returned);
                let TreeKind::Return(value) = &mut node.kind else {
                    panic!()
                };
                match mode {
                    0 => value.from = value.expr,
                    1 => node.ty = raw.definitions.int,
                    2 => value.expr = None,
                    _ => unreachable!(),
                }
                let input = ClassStatePicklePlan {
                    store: raw.store,
                    definitions: raw.definitions,
                    typed_ast: &ast,
                    source_ast: raw.source_ast,
                    source_index: raw.source_index,
                    typed_bindings: raw.typed_bindings,
                    source: raw.source.clone(),
                    class: raw.class,
                    class_tree: raw.class_tree,
                    constructor: PrimaryConstructorInputPlan {
                        symbol: raw.constructor.symbol,
                        source_tree: raw.constructor.source_tree,
                        parameter_clauses: raw.constructor.parameter_clauses.clone(),
                    },
                    members: raw.members,
                };
                let error = input
                    .validate()
                    .and_then(|checked| checked.validate_method_bodies())
                    .unwrap_err();
                assert!(
                    matches!(error, PickleError::Unsupported { .. }),
                    "{error:?}"
                );
                assert_eq!(checkpoint, raw.store.checkpoint());
            }
        },
    )
    .unwrap();
}
