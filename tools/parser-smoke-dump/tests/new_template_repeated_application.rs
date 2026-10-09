use dotty_core::ast::UntypedNode;
use dotty_core::{NameInterner, SourceId, SourceText, TreeKind};
use dotty_lexer::ContextualScanner;
use dotty_parser::parse_compilation_unit;

#[test]
fn reports_repeated_constructor_application_and_preserves_the_next_tuple() {
    let source = include_str!(
        "../../scala-parser-oracle/fixtures/oracle-only/new-template-repeated-application.scala"
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
        result.diagnostics.iter().any(|diagnostic| {
            diagnostic.message() == "a constructor application cannot be applied again"
        }),
        "expected the repeated constructor application diagnostic, got {:?}",
        result.diagnostics
    );
    let (_, tuple_tree) = result
        .ast
        .iter()
        .find(|(_, tree)| matches!(tree.kind, TreeKind::PhaseSpecific(UntypedNode::Tuple(_))))
        .expect("the dedented tuple remains a separate expression");
    let tuple_span = tuple_tree
        .position
        .expect("tuple has a source position")
        .span()
        .range();
    assert_eq!(
        source.get(tuple_span.start() as usize..tuple_span.end() as usize),
        Some("(2, 3)")
    );
}
