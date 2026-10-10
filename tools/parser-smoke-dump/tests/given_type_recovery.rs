use dotty_core::ast::{DefDef, ModuleDef, Template, UntypedNode};
use dotty_core::{NameInterner, SourceId, SourceText, TextRange, TreeKind};
use dotty_lexer::ContextualScanner;
use dotty_parser::{ParseDiagnosticKind, parse_compilation_unit};

#[test]
fn incomplete_named_given_type_parameters_report_error_and_preserve_next_member() {
    let source = "object O:\n  given g: [T\n  def after = 1";
    let scanner = ContextualScanner::new(source).expect("source scans");
    let mut names = NameInterner::new();
    let result = parse_compilation_unit(
        SourceText::new(source).expect("source text is valid"),
        SourceId::from_index(0),
        scanner,
        &mut names,
    );

    let missing_close = result
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.kind() == ParseDiagnosticKind::ExpectedToken
                && diagnostic
                    .legacy_message()
                    .expect("legacy parser diagnostic")
                    == "expected Punctuation(RightBracket), found Newline"
        })
        .unwrap_or_else(|| {
            panic!(
                "the unterminated type parameter clause should be diagnosed: {:?}",
                result.diagnostics
            )
        });
    let line_break = source.find("\n  def after").unwrap() as u32;
    assert_eq!(
        missing_close.span(),
        TextRange::new(line_break, line_break + 3).unwrap()
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

#[test]
fn missing_named_given_result_type_is_reported_before_next_member() {
    let source = "object O:\n  given g: [T]\n  def after = 1";
    let scanner = ContextualScanner::new(source).expect("source scans");
    let mut names = NameInterner::new();
    let result = parse_compilation_unit(
        SourceText::new(source).expect("source text is valid"),
        SourceId::from_index(0),
        scanner,
        &mut names,
    );

    let missing_type = result
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.kind() == ParseDiagnosticKind::ExpectedType
                && diagnostic
                    .legacy_message()
                    .expect("legacy parser diagnostic")
                    == "expected a given result type after type parameters"
        })
        .expect("a named given without a result type should be diagnosed");
    let line_break = source.find("\n  def after").unwrap() as u32;
    assert_eq!(
        missing_type.span(),
        TextRange::new(line_break, line_break + 3).unwrap()
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
        panic!("expected malformed given and following member, got {body:?}");
    };
    assert!(matches!(result.ast.get(*given).kind, TreeKind::DefDef(_)));
    let TreeKind::DefDef(DefDef { name, .. }) = &result.ast.get(*following).kind else {
        panic!("expected the following definition to remain available");
    };
    assert_eq!(names.resolve(name.as_name().text()), "after");
}
