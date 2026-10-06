use dotty_core::ast::{Apply, DefDef, ModuleDef, New, Template, UntypedNode};
use dotty_core::{NameInterner, SourceId, SourceText, TextRange, TreeKind};
use dotty_lexer::ContextualScanner;
use dotty_parser::{ParseDiagnosticKind, parse_compilation_unit};

#[test]
fn missing_argument_after_indented_new_template_does_not_swallow_later_argument() {
    let source =
        "object O:\n  def result = consume(\n    new First:\n      def first = 1,\n    next +\n  )";
    let scanner = ContextualScanner::new(source).expect("source scans");
    let mut names = NameInterner::new();
    let result = parse_compilation_unit(
        SourceText::new(source).expect("source text is valid"),
        SourceId::from_index(0),
        scanner,
        &mut names,
    );

    let missing_rhs = result
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.kind() == ParseDiagnosticKind::ExpectedExpression
                && diagnostic.message() == "expected an expression"
        })
        .expect("the incomplete following argument should be diagnosed");
    let closing_paren = source.rfind(')').unwrap() as u32;
    assert_eq!(
        missing_rhs.span(),
        TextRange::new(closing_paren, closing_paren + 1).unwrap()
    );
    let TreeKind::PackageDef(package) = &result.ast.get(result.root).kind else {
        panic!("expected package root");
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
    let [method] = body.as_slice() else {
        panic!("expected the result method");
    };
    let TreeKind::DefDef(DefDef { rhs: Some(rhs), .. }) = &result.ast.get(*method).kind else {
        panic!("expected the method body");
    };
    let TreeKind::Apply(Apply { args, .. }) = &result.ast.get(*rhs).kind else {
        panic!("expected the consume application");
    };
    let [_, next] = args.as_slice() else {
        panic!(
            "template-closing comma and next argument must remain distinct: {args:?}; diagnostics={:?}",
            result.diagnostics
        );
    };
    let TreeKind::New(New { tpt }) = &result.ast.get(args[0]).kind else {
        panic!("expected the first argument to remain the anonymous new expression");
    };
    let TreeKind::Template(template) = &result.ast.get(*tpt).kind else {
        panic!("expected the first argument to retain its anonymous template");
    };
    assert_eq!(template.body.len(), 1);
    let TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) = &result.ast.get(*next).kind else {
        panic!(
            "expected the incomplete next argument to remain an infix expression, got {:?}",
            result.ast.get(*next).kind
        );
    };
    let TreeKind::Ident(identifier) = &result.ast.get(infix.left).kind else {
        panic!("expected the next argument's left operand to remain an identifier");
    };
    assert_eq!(names.resolve(identifier.name.text()), "next");
    assert!(
        result
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.message() != "expected a template member separator")
    );
}
