use dotty_core::ast::{
    Block, CaseDef, DefDef, ErrorNodeKind, If, Literal, Match, ParsedTry, Template, UntypedNode,
};
use dotty_core::{Constant, NameInterner, SourceId, SourceText, TextRange, TreeKind};
use dotty_lexer::ContextualScanner;
use dotty_parser::{parse_compilation_unit, parse_expression_fragment};

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

#[test]
fn empty_final_case_body_keeps_following_definition_in_its_template() {
    let source = "object O:\n  def f =\n    value match\n      case A =>\n  def after = 1";
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
        panic!("the following method must not escape to package scope");
    };
    let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module)) = &result.ast.get(*object).kind
    else {
        panic!("expected an object definition");
    };
    let TreeKind::Template(Template { body, .. }) = &result.ast.get(module.template).kind else {
        panic!("expected an object template");
    };
    assert_eq!(body.len(), 2, "both methods belong to the object template");
    let [first, second] = body.as_slice() else {
        unreachable!("length was asserted above");
    };
    let TreeKind::DefDef(DefDef {
        rhs: Some(first_rhs),
        ..
    }) = result.ast.get(*first).kind
    else {
        panic!("expected the first method to have a body");
    };
    let first_expression = match result.ast.get(first_rhs).kind {
        TreeKind::Block(Block { expr, .. }) => expr,
        TreeKind::Match(_) => first_rhs,
        _ => panic!("expected the first method body to contain a match"),
    };
    assert!(matches!(
        result.ast.get(first_expression).kind,
        TreeKind::Match(_)
    ));
    assert!(matches!(result.ast.get(*second).kind, TreeKind::DefDef(_)));
    assert_eq!(
        result.ast.get(*object).position.unwrap().span().range(),
        TextRange::new(0, source.len() as u32).unwrap()
    );
    assert_eq!(
        result.ast.get(*first).position.unwrap().span().range(),
        TextRange::new(12, 51).unwrap()
    );
    assert_eq!(
        result.ast.get(*second).position.unwrap().span().range(),
        TextRange::new(54, 67).unwrap()
    );
}

#[test]
fn empty_catch_case_body_preserves_the_enclosing_local_definition() {
    let source =
        include_str!("../../scala-parser-oracle/fixtures/compilation/catch-empty-case-body.scala");
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
        panic!("expected the empty-catch object");
    };
    let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module)) = &result.ast.get(*object).kind
    else {
        panic!("expected the object definition");
    };
    let TreeKind::Template(Template { body, .. }) = &result.ast.get(module.template).kind else {
        panic!("expected the object template");
    };
    let method = body
        .iter()
        .find(|tree| {
            matches!(
                &result.ast.get(**tree).kind,
                TreeKind::DefDef(definition)
                    if names.resolve(definition.name.as_name().text()) == "f"
            )
        })
        .copied()
        .expect("method f is retained");
    let TreeKind::DefDef(DefDef {
        rhs: Some(method_body),
        ..
    }) = result.ast.get(method).kind
    else {
        panic!("expected method f to have a body");
    };
    let TreeKind::Block(Block { ref stats, .. }) = result.ast.get(method_body).kind else {
        panic!("expected method f's local block");
    };
    let [append, after] = stats.as_slice() else {
        panic!("expected both the local method and following val");
    };
    assert!(matches!(result.ast.get(*after).kind, TreeKind::ValDef(_)));
    assert_eq!(
        result.ast.get(*after).position.unwrap().span().range(),
        TextRange::new(143, 156).unwrap()
    );
    let TreeKind::DefDef(DefDef {
        rhs: Some(append_body),
        ..
    }) = result.ast.get(*append).kind
    else {
        panic!("expected the local append method");
    };
    let TreeKind::PhaseSpecific(UntypedNode::ParsedTry(ParsedTry {
        handler: Some(handler),
        ..
    })) = result.ast.get(append_body).kind
    else {
        panic!("expected a try expression with a catch handler");
    };
    let TreeKind::Match(Match { ref cases, .. }) = result.ast.get(handler).kind else {
        panic!("expected a catch case list");
    };
    let [case] = cases.as_slice() else {
        panic!("expected one catch case");
    };
    let TreeKind::CaseDef(CaseDef { body, .. }) = result.ast.get(*case).kind else {
        panic!("expected the catch case definition");
    };
    let TreeKind::Block(Block { ref stats, expr }) = result.ast.get(body).kind else {
        panic!("expected the empty catch body block");
    };
    assert_eq!(
        result.ast.get(body).position.unwrap().span().range(),
        TextRange::new(133, 135).unwrap()
    );
    assert!(stats.is_empty());
    assert!(matches!(
        result.ast.get(expr).kind,
        TreeKind::Literal(Literal {
            value: Constant::Unit
        })
    ));
}

#[test]
fn empty_nested_match_case_preserves_the_enclosing_match_case() {
    let source = "outer match\n  case A =>\n    inner match\n      case B =>\n  case C => 1\n";
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
        panic!("expected an outer match expression");
    };
    let [first, second] = cases.as_slice() else {
        panic!("expected the outer match to retain both cases");
    };
    let TreeKind::CaseDef(CaseDef { body, .. }) = result.ast.get(*first).kind else {
        panic!("expected the first outer case");
    };
    let TreeKind::Block(Block { expr, .. }) = result.ast.get(body).kind else {
        panic!("expected the first case body block");
    };
    let TreeKind::Match(Match { ref cases, .. }) = result.ast.get(expr).kind else {
        panic!("expected a nested match in the first case body");
    };
    let [nested_case] = cases.as_slice() else {
        panic!("expected one nested case");
    };
    let TreeKind::CaseDef(CaseDef { body, .. }) = result.ast.get(*nested_case).kind else {
        panic!("expected a nested case definition");
    };
    let TreeKind::Block(Block { ref stats, expr }) = result.ast.get(body).kind else {
        panic!("expected the nested empty body block");
    };
    assert!(stats.is_empty());
    assert!(matches!(
        result.ast.get(expr).kind,
        TreeKind::Literal(Literal {
            value: Constant::Unit
        })
    ));
    assert!(matches!(result.ast.get(*second).kind, TreeKind::CaseDef(_)));
    assert_eq!(
        result.ast.get(*second).position.unwrap().span().range(),
        TextRange::new(58, 69).unwrap()
    );
}

#[test]
fn missing_nested_case_arrow_reports_and_preserves_the_enclosing_match_case() {
    let source = "outer match\n  case A =>\n    inner match\n      case B\n  case C => 1\n";
    let scanner = ContextualScanner::new(source).expect("source scans");
    let mut names = NameInterner::new();
    let result = parse_expression_fragment(
        SourceText::new(source).expect("source text is valid"),
        SourceId::from_index(0),
        scanner,
        &mut names,
    );

    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(
        result.diagnostics[0].kind(),
        dotty_parser::ParseDiagnosticKind::ExpectedToken
    );
    assert_eq!(
        result.diagnostics[0].span(),
        TextRange::new(55, 55).unwrap()
    );

    let TreeKind::Match(Match { ref cases, .. }) = result.ast.get(result.root).kind else {
        panic!("expected an outer match expression");
    };
    let [first, second] = cases.as_slice() else {
        panic!("recovery must preserve both outer cases");
    };
    let TreeKind::CaseDef(CaseDef { body, .. }) = result.ast.get(*first).kind else {
        panic!("expected the first outer case");
    };
    let TreeKind::Block(Block { expr, .. }) = result.ast.get(body).kind else {
        panic!("expected the first outer case body block");
    };
    let TreeKind::Match(Match { ref cases, .. }) = result.ast.get(expr).kind else {
        panic!("expected the nested match in the first outer case");
    };
    let [nested_case] = cases.as_slice() else {
        panic!("expected the malformed nested case to remain in its match");
    };
    let TreeKind::CaseDef(CaseDef { body, .. }) = result.ast.get(*nested_case).kind else {
        panic!("expected a nested case definition");
    };
    assert!(matches!(
        result.ast.get(body).kind,
        TreeKind::PhaseSpecific(UntypedNode::Error(error))
            if error.kind == ErrorNodeKind::MissingExpression
    ));
    assert_eq!(
        result.ast.get(*second).position.unwrap().span().range(),
        TextRange::new(55, 66).unwrap()
    );
}
