use dotty_core::ast::{Block, Function, Template, UntypedNode, ValDef};
use dotty_core::{NameInterner, SourceId, SourceText, TreeKind};
use dotty_lexer::ContextualScanner;
use dotty_parser::{parse_compilation_unit, parse_expression_fragment};

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

#[test]
fn lambda_block_stops_before_the_next_template_member() {
    let source = include_str!(
        "../../scala-parser-oracle/fixtures/compilation/colon-lambda-block-body.scala"
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
    let TreeKind::Template(Template { body: members, .. }) = &result.ast.get(module.template).kind
    else {
        panic!("expected an object template");
    };
    assert_eq!(
        members.len(),
        3,
        "val g must remain outside both lambda blocks"
    );
    let TreeKind::ValDef(ValDef { rhs: Some(rhs), .. }) = result.ast.get(members[0]).kind else {
        panic!("expected val f to have a right-hand side");
    };
    let TreeKind::Apply(application) = &result.ast.get(rhs).kind else {
        panic!("expected val f's right-hand side to be an application");
    };
    let [argument] = application.args.as_slice() else {
        panic!("expected one colon argument");
    };
    let TreeKind::PhaseSpecific(UntypedNode::Function(Function { body, .. })) =
        &result.ast.get(*argument).kind
    else {
        panic!("expected the colon argument to remain a lambda");
    };
    let TreeKind::Block(Block { stats, expr }) = &result.ast.get(*body).kind else {
        panic!("expected the lambda body to own a block");
    };
    let [first, value] = stats.as_slice() else {
        panic!("expected expression and val y inside the lambda body: {stats:?}");
    };
    assert!(matches!(
        result.ast.get(*first).kind,
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
    ));
    let TreeKind::ValDef(ValDef { name, .. }) = &result.ast.get(*value).kind else {
        panic!("expected val y inside the lambda body");
    };
    assert_eq!(names.resolve(name.as_name().text()), "y");
    let TreeKind::Ident(identifier) = &result.ast.get(*expr).kind else {
        panic!("expected y as the lambda body's final expression");
    };
    assert_eq!(names.resolve(identifier.name.text()), "y");

    let TreeKind::ValDef(ValDef { rhs: Some(rhs), .. }) = result.ast.get(members[1]).kind else {
        panic!("expected val h to have a right-hand side");
    };
    let TreeKind::Apply(application) = &result.ast.get(rhs).kind else {
        panic!("expected val h's right-hand side to be an application");
    };
    let [argument] = application.args.as_slice() else {
        panic!("expected one colon argument for val h");
    };
    let TreeKind::PhaseSpecific(UntypedNode::Function(Function { body, .. })) =
        &result.ast.get(*argument).kind
    else {
        panic!("expected val h's colon argument to be a lambda");
    };
    let TreeKind::Block(Block { stats, expr }) = &result.ast.get(*body).kind else {
        panic!("expected val h's lambda body to retain a block");
    };
    let [first] = stats.as_slice() else {
        panic!("expected first lambda-body expression as a statement: {stats:?}");
    };
    assert!(matches!(
        result.ast.get(*first).kind,
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
    ));
    assert!(matches!(
        result.ast.get(*expr).kind,
        TreeKind::PhaseSpecific(UntypedNode::Number(_))
    ));

    let TreeKind::ValDef(ValDef { name, .. }) = &result.ast.get(members[2]).kind else {
        panic!("expected val g after the colon argument");
    };
    assert_eq!(names.resolve(name.as_name().text()), "g");
}

#[test]
fn indented_lambda_body_keeps_dottys_single_expression_block() {
    let source = include_str!(
        "../../scala-parser-oracle/fixtures/colon-argument-lambda-indented-body.scala"
    );
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
        panic!("expected one colon argument");
    };
    let TreeKind::PhaseSpecific(UntypedNode::Function(Function { body, .. })) =
        &result.ast.get(*argument).kind
    else {
        panic!("expected the colon argument to be a lambda");
    };
    let TreeKind::Block(Block { stats, expr }) = &result.ast.get(*body).kind else {
        panic!("expected Dotty's one-expression block around the lambda body");
    };
    assert!(stats.is_empty());
    assert!(matches!(
        result.ast.get(*expr).kind,
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
    ));
}

#[test]
fn indented_lambda_body_includes_following_statements_before_its_outdent() {
    let source = include_str!(
        "../../scala-parser-oracle/fixtures/colon-argument-lambda-indented-body-following.scala"
    );
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
        panic!("expected one colon argument");
    };
    let TreeKind::PhaseSpecific(UntypedNode::Function(Function { body, .. })) =
        &result.ast.get(*argument).kind
    else {
        panic!("expected the colon argument to be a lambda");
    };
    let TreeKind::Block(Block { stats, expr }) = &result.ast.get(*body).kind else {
        panic!("expected the lambda body to be a block");
    };
    let [first] = stats.as_slice() else {
        panic!("expected the first expression as a lambda-body statement: {stats:?}");
    };
    assert!(matches!(
        result.ast.get(*first).kind,
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
    ));
    assert!(matches!(
        result.ast.get(*expr).kind,
        TreeKind::PhaseSpecific(UntypedNode::Number(_))
    ));
}
