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
fn parses_nested_indented_case_lambdas_and_keeps_following_members() {
    let source = include_str!(
        "../../scala-parser-oracle/fixtures/compilation/indented-nested-case-lambda.scala"
    );
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
    assert_eq!(body.len(), 2, "both vals should survive parsing");

    let TreeKind::ValDef(ValDef {
        rhs: Some(outer), ..
    }) = result.ast.get(body[0]).kind
    else {
        panic!("expected val f to have a right-hand side");
    };
    let TreeKind::Match(Match { cases, .. }) = &result.ast.get(outer).kind else {
        panic!("expected the outer case lambda to be a Match tree");
    };
    assert_eq!(cases.len(), 2);
    for (case, expected_nested_case_count) in cases.iter().zip([2, 1]) {
        let TreeKind::CaseDef(dotty_core::ast::CaseDef { body, .. }) = result.ast.get(*case).kind
        else {
            panic!("expected a case clause");
        };
        let TreeKind::Block(dotty_core::ast::Block { stats, expr }) = &result.ast.get(body).kind
        else {
            panic!("expected a case body block");
        };
        assert!(stats.is_empty());
        let TreeKind::Match(Match { cases, .. }) = &result.ast.get(*expr).kind else {
            panic!("expected each case body to contain a nested case lambda");
        };
        assert_eq!(cases.len(), expected_nested_case_count);
    }
}

#[test]
fn recovers_from_an_invalid_indented_case_lambda_body() {
    let source = "object BrokenCaseLambda:\n  val f: Int => Int =\n    case 0 => x +\n    case x => x\n  val g = 2\n";
    let scanner = ContextualScanner::new(source).expect("source scans");
    let mut names = NameInterner::new();
    let result = parse_compilation_unit(
        SourceText::new(source).expect("source text is valid"),
        SourceId::from_index(0),
        scanner,
        &mut names,
    );

    assert_eq!(
        result
            .diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.kind(), diagnostic.span()))
            .collect::<Vec<_>>(),
        [(
            dotty_parser::ParseDiagnosticKind::ExpectedExpression,
            TextRange::new(64, 69).unwrap(),
        )],
        "diagnostics: {:?}",
        result.diagnostics
    );
    let TreeKind::PackageDef(package) = &result.ast.get(result.root).kind else {
        panic!("expected a recoverable package root");
    };
    let [object] = package.stats.as_slice() else {
        panic!("the surrounding object definition should be retained");
    };
    let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module)) = &result.ast.get(*object).kind
    else {
        panic!("expected an object definition");
    };
    let TreeKind::Template(Template { body, .. }) = &result.ast.get(module.template).kind else {
        panic!("expected an object template");
    };
    let retained_values = body
        .iter()
        .filter_map(|tree| match &result.ast.get(*tree).kind {
            TreeKind::ValDef(definition) => Some(names.resolve(definition.name.as_name().text())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(retained_values, ["f", "g"], "template members: {body:?}");

    let f = body
        .iter()
        .find(|tree| {
            matches!(
                &result.ast.get(**tree).kind,
                TreeKind::ValDef(definition)
                    if names.resolve(definition.name.as_name().text()) == "f"
            )
        })
        .copied()
        .expect("val f survives recovery");
    let TreeKind::ValDef(ValDef { rhs: Some(rhs), .. }) = result.ast.get(f).kind else {
        panic!("expected val f to retain its case-lambda rhs");
    };
    let TreeKind::Match(Match { cases, .. }) = &result.ast.get(rhs).kind else {
        panic!("expected the case-lambda tree to survive recovery");
    };
    assert_eq!(cases.len(), 2, "the valid later case clause should survive");
    let TreeKind::CaseDef(dotty_core::ast::CaseDef { pattern, .. }) = result.ast.get(cases[1]).kind
    else {
        panic!("expected the recovered second case clause");
    };
    let TreeKind::Ident(identifier) = &result.ast.get(pattern).kind else {
        panic!("expected the recovered case pattern to be an identifier");
    };
    assert_eq!(names.resolve(identifier.name.text()), "x");
}
