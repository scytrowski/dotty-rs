use dotty_core::ast::{DefDef, If, ModuleDef, Template, UntypedNode};
use dotty_core::{NameInterner, SourceId, SourceText, TreeKind, Untyped};
use dotty_lexer::ContextualScanner;
use dotty_parser::parse_compilation_unit;

#[test]
fn continues_alphabetic_infix_after_parenthesized_condition_operand() {
    let source = include_str!(
        "../../scala-parser-oracle/fixtures/compilation/parenthesized-alphabetic-infix-condition.scala"
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
    let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(ModuleDef { template, .. })) =
        &result.ast.get(*object).kind
    else {
        panic!("expected an object definition");
    };
    let TreeKind::Template(Template { body, .. }) = &result.ast.get(*template).kind else {
        panic!("expected an object template");
    };
    let [alpha, parenthesized] = body.as_slice() else {
        panic!("expected both condition methods");
    };
    let condition = method_if_condition(&result.ast, *alpha);
    let TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) = &result.ast.get(condition).kind
    else {
        panic!("expected an infix condition after the parenthesized left operand");
    };
    assert_eq!(names.resolve(infix.op.text()), "eq");
    assert!(matches!(
        result.ast.get(infix.left).kind,
        TreeKind::PhaseSpecific(UntypedNode::Parens(_))
    ));
    assert!(matches!(
        result.ast.get(infix.right).kind,
        TreeKind::PhaseSpecific(UntypedNode::Parens(_))
    ));
    assert!(matches!(
        result
            .ast
            .get(method_if_condition(&result.ast, *parenthesized))
            .kind,
        TreeKind::PhaseSpecific(UntypedNode::Parens(_))
    ));
}

fn method_if_condition(
    ast: &dotty_core::ast::AstArena<Untyped>,
    method: dotty_core::TreeId<Untyped>,
) -> dotty_core::TreeId<Untyped> {
    let TreeKind::DefDef(DefDef { rhs: Some(rhs), .. }) = &ast.get(method).kind else {
        panic!("expected a method body");
    };
    let TreeKind::If(If { cond, .. }) = &ast.get(*rhs).kind else {
        panic!("expected an if expression");
    };
    *cond
}
