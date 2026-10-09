use dotty_core::ast::{Apply, Block, Function, Tuple, UntypedNode};
use dotty_core::{NameInterner, SourceId, SourceText, TreeKind, Untyped};
use dotty_lexer::ContextualScanner;
use dotty_parser::parse_expression_fragment;

fn parse_expression(source: &str) -> dotty_parser::ParseResult {
    let scanner = ContextualScanner::new(source).expect("source should scan");
    let mut names = NameInterner::new();
    parse_expression_fragment(
        SourceText::new(source).expect("source text should be valid"),
        SourceId::from_index(0),
        scanner,
        &mut names,
    )
}

fn assert_two_element_tuple(
    ast: &dotty_core::ast::AstArena<Untyped>,
    tree: dotty_core::TreeId<Untyped>,
) {
    let TreeKind::PhaseSpecific(UntypedNode::Tuple(Tuple { elements })) = &ast.get(tree).kind
    else {
        panic!("expected tuple expression");
    };
    let [lambda, second] = elements.as_slice() else {
        panic!("expected exactly two tuple elements");
    };
    assert!(matches!(
        ast.get(*lambda).kind,
        TreeKind::PhaseSpecific(UntypedNode::Function(Function { .. }))
    ));
    assert!(matches!(ast.get(*second).kind, TreeKind::Ident(_)));
}

#[test]
fn multiline_lambda_body_does_not_swallow_the_tuple_comma() {
    let source =
        include_str!("../../scala-parser-oracle/fixtures/lambda-indented-body-tuple-comma.scala");
    let result = parse_expression(source);

    assert!(
        result.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        result.diagnostics
    );
    assert_two_element_tuple(&result.ast, result.root);
}

#[test]
fn tuple_and_argument_commas_remain_distinct_inside_a_nested_block() {
    let source = "{\n  consume((\n    e =>\n      e,\n    tupleTail\n  ),\n  argumentTail)\n}";
    let result = parse_expression(source);

    assert!(
        result.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        result.diagnostics
    );
    let TreeKind::Block(Block { expr, .. }) = &result.ast.get(result.root).kind else {
        panic!("expected an enclosing block");
    };
    let TreeKind::Apply(Apply { args, .. }) = &result.ast.get(*expr).kind else {
        panic!("expected the block's final expression to be an application");
    };
    let [tuple, argument_tail] = args.as_slice() else {
        panic!("expected two application arguments");
    };
    assert_two_element_tuple(&result.ast, *tuple);
    assert!(matches!(
        result.ast.get(*argument_tail).kind,
        TreeKind::Ident(_)
    ));
}
