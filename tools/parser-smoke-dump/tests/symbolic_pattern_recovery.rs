use dotty_core::{NameInterner, SourceId, SourceText};
use dotty_lexer::ContextualScanner;
use dotty_parser::parse_pattern_fragment;

fn parse_pattern(source: &str) -> dotty_parser::ParseResult {
    let scanner = ContextualScanner::new(source).expect("source should scan");
    let mut names = NameInterner::new();
    parse_pattern_fragment(
        SourceText::new(source).expect("source text should be valid"),
        SourceId::from_index(0),
        scanner,
        &mut names,
    )
}

#[test]
fn reserved_arrow_symbol_is_not_accepted_as_a_symbolic_extractor() {
    let result = parse_pattern("=>(x)");

    assert!(
        !result.diagnostics.is_empty(),
        "reserved arrow syntax must be rejected as a pattern"
    );
    assert!(result.ast.get(result.root).position.is_some());
}

#[test]
fn unterminated_symbolic_extractor_pattern_recovers() {
    let result = parse_pattern("::(x");

    assert!(
        !result.diagnostics.is_empty(),
        "unterminated extractor arguments must produce a diagnostic"
    );
    assert!(result.ast.get(result.root).position.is_some());
}
