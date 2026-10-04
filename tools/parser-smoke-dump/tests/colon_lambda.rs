use dotty_core::ast::{Block, Function, UntypedNode};
use dotty_core::{NameInterner, SourceId, SourceText, TreeKind};
use dotty_lexer::ContextualScanner;
use dotty_parser::parse_expression_fragment;

#[test]
fn lambda_in_an_indented_colon_argument_keeps_its_block_body() {
    let source = include_str!("../../scala-parser-oracle/fixtures/colon-argument-lambda.scala");
    let scanner = ContextualScanner::new(source).expect("source scans");
    let mut names = NameInterner::new();
    let result = parse_expression_fragment(
        SourceText::new(source).expect("source text is valid"),
        SourceId::from_index(0),
        scanner,
        &mut names,
    );

    assert!(
        result.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        result.diagnostics
    );
    let TreeKind::Apply(application) = &result.ast.get(result.root).kind else {
        panic!("expected the colon argument to form an application");
    };
    let [argument] = application.args.as_slice() else {
        panic!("expected one lambda argument");
    };
    let TreeKind::PhaseSpecific(UntypedNode::Function(Function { body, .. })) =
        &result.ast.get(*argument).kind
    else {
        panic!("expected a lambda argument");
    };
    let TreeKind::Block(Block { stats, expr }) = &result.ast.get(*body).kind else {
        panic!("expected Dotty's block wrapper around the lambda body");
    };
    assert!(stats.is_empty());
    assert!(matches!(
        result.ast.get(*expr).kind,
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
    ));
}

#[test]
fn top_level_indented_lambda_body_remains_an_expression() {
    let source = "x =>\n  x + 1";
    let scanner = ContextualScanner::new(source).expect("source scans");
    let mut names = NameInterner::new();
    let result = parse_expression_fragment(
        SourceText::new(source).expect("source text is valid"),
        SourceId::from_index(0),
        scanner,
        &mut names,
    );

    assert!(
        result.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        result.diagnostics
    );
    let TreeKind::PhaseSpecific(UntypedNode::Function(Function { body, .. })) =
        &result.ast.get(result.root).kind
    else {
        panic!("expected a top-level lambda");
    };
    assert!(matches!(
        result.ast.get(*body).kind,
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
    ));
}
