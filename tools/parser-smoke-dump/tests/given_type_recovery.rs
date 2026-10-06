use dotty_core::ast::{DefDef, ModuleDef, Template, UntypedNode};
use dotty_core::{NameInterner, SourceId, SourceText, TextRange, TreeKind};
use dotty_lexer::ContextualScanner;
use dotty_parser::{ParseDiagnosticKind, parse_compilation_unit};

#[test]
fn incomplete_named_given_type_parameters_report_error_and_preserve_next_member() {
    let source = "object O:\n  given g: [T <: ] = value\n  def after = 1";
    let scanner = ContextualScanner::new(source).expect("source scans");
    let mut names = NameInterner::new();
    let result = parse_compilation_unit(
        SourceText::new(source).expect("source text is valid"),
        SourceId::from_index(0),
        scanner,
        &mut names,
    );

    let malformed_bound = result
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.kind() == ParseDiagnosticKind::ExpectedType
                && diagnostic.message() == "expected a type operand"
        })
        .expect("the missing type bound should be diagnosed");
    let closing_bracket = source.find(']').unwrap() as u32;
    assert_eq!(
        malformed_bound.span(),
        TextRange::new(closing_bracket, closing_bracket + 1).unwrap()
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
    let [given, following] = body.as_slice() else {
        panic!(
            "expected malformed given and following member, got {body:?}; diagnostics={:?}",
            result.diagnostics
        );
    };
    assert!(matches!(result.ast.get(*given).kind, TreeKind::DefDef(_)));
    let TreeKind::DefDef(DefDef { name, .. }) = &result.ast.get(*following).kind else {
        panic!("expected the following definition to remain available");
    };
    assert_eq!(names.resolve(name.as_name().text()), "after");
}
