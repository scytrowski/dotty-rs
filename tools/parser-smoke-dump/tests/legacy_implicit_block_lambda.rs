use dotty_core::ast::{Block, Function, Modifier, UntypedNode, ValDef};
use dotty_core::{NameInterner, SourceId, SourceText, TreeKind};
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

#[test]
fn accepts_parenthesized_implicit_lambda_as_the_first_block_statement() {
    let source = include_str!(
        "../../scala-parser-oracle/fixtures/legacy-implicit-parenthesized-block-lambda.scala"
    );
    let result = parse_expression(source);

    assert!(
        result.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        result.diagnostics
    );
    let TreeKind::Block(Block { stats, expr }) = &result.ast.get(result.root).kind else {
        panic!("expected a block expression");
    };
    assert!(stats.is_empty());
    let TreeKind::PhaseSpecific(UntypedNode::Function(Function { params, body })) =
        &result.ast.get(*expr).kind
    else {
        panic!("expected the block expression to be the implicit lambda");
    };
    let [parameter] = params.as_slice() else {
        panic!("expected one implicit lambda parameter");
    };
    let TreeKind::ValDef(ValDef { metadata, .. }) = &result.ast.get(*parameter).kind else {
        panic!("expected a lambda parameter definition");
    };
    assert!(metadata.modifiers.contains(&Modifier::Implicit));
    let TreeKind::Block(Block { expr, .. }) = &result.ast.get(*body).kind else {
        panic!("expected the block-local lambda body wrapper");
    };
    assert!(matches!(result.ast.get(*expr).kind, TreeKind::Ident(_)));
}

#[test]
fn implicit_value_definition_is_not_misclassified_as_a_block_lambda() {
    let result = parse_expression("{ implicit val value = 1 }");

    assert!(
        result.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        result.diagnostics
    );
    let TreeKind::Block(Block { stats, .. }) = &result.ast.get(result.root).kind else {
        panic!("expected a block expression");
    };
    let [value] = stats.as_slice() else {
        panic!("expected the implicit value definition in the block stats");
    };
    let TreeKind::ValDef(ValDef { metadata, .. }) = &result.ast.get(*value).kind else {
        panic!("expected a value definition");
    };
    assert!(metadata.modifiers.contains(&Modifier::Implicit));
}
