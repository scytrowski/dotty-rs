use dotty_core::ast::{Block, CaseDef, If, Literal, Match};
use dotty_core::{Constant, NameInterner, SourceId, SourceText, TextRange, TreeKind};
use dotty_lexer::ContextualScanner;
use dotty_parser::parse_expression_fragment;

#[test]
fn empty_case_body_before_outer_else_preserves_layout_and_else_branch() {
    let source = "if cond then\n  value match\n    case A =>\nelse\n  fallback";
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
    let TreeKind::If(If {
        then_branch,
        else_branch,
        ..
    }) = result.ast.get(result.root).kind
    else {
        panic!("expected an if expression");
    };
    let TreeKind::Match(Match { ref cases, .. }) = result.ast.get(then_branch).kind else {
        panic!("expected the then branch to contain a match");
    };
    let [case] = cases.as_slice() else {
        panic!("expected one case clause");
    };
    let TreeKind::CaseDef(CaseDef { body, .. }) = result.ast.get(*case).kind else {
        panic!("expected a case definition");
    };
    let TreeKind::Block(Block { ref stats, expr }) = result.ast.get(body).kind else {
        panic!("expected an empty case body block");
    };
    assert!(stats.is_empty());
    assert!(matches!(
        result.ast.get(expr).kind,
        TreeKind::Literal(Literal {
            value: Constant::Unit
        })
    ));
    let TreeKind::Ident(_) = result.ast.get(else_branch).kind else {
        panic!("expected the outer else branch to remain attached");
    };
    assert_eq!(
        result.ast.get(result.root).position.unwrap().span().range(),
        TextRange::new(0, 56).unwrap()
    );
    assert_eq!(
        result.ast.get(then_branch).position.unwrap().span().range(),
        TextRange::new(15, 40).unwrap()
    );
    assert_eq!(
        result.ast.get(body).position.unwrap().span().range(),
        TextRange::new(38, 40).unwrap()
    );
    assert_eq!(
        result.ast.get(else_branch).position.unwrap().span().range(),
        TextRange::new(48, 56).unwrap()
    );
}

#[test]
fn empty_final_case_body_before_eof_does_not_leave_layout_tokens() {
    let source = "value match\n  case A =>\n";
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
    let TreeKind::Match(Match { ref cases, .. }) = result.ast.get(result.root).kind else {
        panic!("expected a match expression");
    };
    let [case] = cases.as_slice() else {
        panic!("expected one case clause");
    };
    let TreeKind::CaseDef(CaseDef { body, .. }) = result.ast.get(*case).kind else {
        panic!("expected a case definition");
    };
    let TreeKind::Block(Block { ref stats, expr }) = result.ast.get(body).kind else {
        panic!("expected an empty case body block");
    };
    assert!(stats.is_empty());
    assert!(matches!(
        result.ast.get(expr).kind,
        TreeKind::Literal(Literal {
            value: Constant::Unit
        })
    ));
}
