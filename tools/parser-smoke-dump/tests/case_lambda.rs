use dotty_core::ast::{Match, Template, UntypedNode, ValDef};
use dotty_core::{Constant, NameInterner, SourceId, SourceText, TextRange, TreeKind};
use dotty_lexer::ContextualScanner;
use dotty_parser::parse_compilation_unit;

#[test]
fn parses_an_indented_case_lambda_as_a_match_expression() {
    let source =
        include_str!("../../scala-parser-oracle/fixtures/compilation/indented-case-lambda.scala");
    let scanner = ContextualScanner::new(source).expect("source scans");
    let mut names = NameInterner::new();
    let result = parse_compilation_unit(
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
    let TreeKind::PackageDef(package) = &result.ast.get(result.root).kind else {
        panic!("expected a package root");
    };
    let [object] = package.stats.as_slice() else {
        panic!("expected one object definition");
    };
    let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module)) = &result.ast.get(*object).kind
    else {
        panic!("expected an object definition");
    };
    let TreeKind::Template(Template { body, .. }) = &result.ast.get(module.template).kind else {
        panic!("expected an object template");
    };
    let [value] = body.as_slice() else {
        panic!("expected one value definition");
    };
    let TreeKind::ValDef(ValDef { rhs: Some(rhs), .. }) = result.ast.get(*value).kind else {
        panic!("expected val f to have a right-hand side");
    };
    let TreeKind::Match(Match {
        selector,
        ref cases,
    }) = result.ast.get(rhs).kind
    else {
        panic!("expected an indentation-style case lambda to be a Match tree");
    };
    assert_eq!(cases.len(), 2);
    assert!(matches!(
        result.ast.get(selector).kind,
        TreeKind::Literal(dotty_core::ast::Literal {
            value: Constant::Unit
        })
    ));
    let case_start = source.find("case").expect("case clauses exist") as u32;
    assert_eq!(
        result.ast.get(rhs).position.unwrap().span().range(),
        TextRange::new(case_start, source.trim_end().len() as u32).unwrap()
    );
}

#[test]
fn recovers_from_an_invalid_indented_case_lambda_body() {
    let source = "object BrokenCaseLambda:\n  val f: Int => Int =\n    case 0 => )\n";
    let scanner = ContextualScanner::new(source).expect("source scans");
    let mut names = NameInterner::new();
    let result = parse_compilation_unit(
        SourceText::new(source).expect("source text is valid"),
        SourceId::from_index(0),
        scanner,
        &mut names,
    );

    assert!(!result.diagnostics.is_empty());
    let TreeKind::PackageDef(package) = &result.ast.get(result.root).kind else {
        panic!("expected a recoverable package root");
    };
    assert_eq!(
        package.stats.len(),
        1,
        "the surrounding definition is retained"
    );
}
