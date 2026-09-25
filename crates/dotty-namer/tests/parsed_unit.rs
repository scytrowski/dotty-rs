use dotty_core::{
    Packages, ScannerEvent, SemanticStore, SourceId, SourceText, TextRange, Token, TokenKind,
    TokenSource, TokenValue, TreeKind,
};
use dotty_namer::name_compilation_unit;
use dotty_parser::parse_compilation_unit;

struct EofTokenSource {
    eof: Token,
}

impl TokenSource for EofTokenSource {
    fn current(&self) -> &Token {
        &self.eof
    }

    fn position(&self) -> usize {
        0
    }

    fn advance(&mut self) {}

    fn lookahead(&mut self, _n: usize) -> &Token {
        &self.eof
    }

    fn observe(&mut self, _event: ScannerEvent) {}
}

#[test]
fn an_empty_parsed_compilation_unit_passes_through_naming_without_allocations() {
    let source = SourceId::from_index(7);
    let mut store = SemanticStore::new();
    let parsed = parse_compilation_unit(
        SourceText::new("").expect("empty source is valid"),
        source,
        EofTokenSource {
            eof: Token {
                kind: TokenKind::Eof,
                span: TextRange::new(0, 0).expect("empty span is valid"),
                value: TokenValue::None,
            },
        },
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty());
    assert!(matches!(
        parsed.ast.get(parsed.root).kind,
        TreeKind::PackageDef(_)
    ));
    let before = store.checkpoint();
    let mut packages = Packages::new();

    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "Empty.scala",
        &mut store,
        &mut packages,
    )
    .expect("an empty parsed compilation unit should be accepted");

    assert!(index.symbol_at(source, parsed.root).is_none());
    assert_eq!(store.checkpoint(), before);
    assert!(packages.is_empty());
}
