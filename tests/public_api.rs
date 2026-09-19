use dotty::core::{
    Name, Namespace, SemanticStore, Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks,
    SymbolOrigin, Visibility,
};
use dotty::tasty::{NodeCategory, SimpleTerm, TastyFile, TermValue, Writer};

use dotty::core::{ScannerEvent, TextRange, Token, TokenKind, TokenSource, TokenValue};
use dotty::parser::parse_compilation_unit;

struct SingleTokenSource {
    tokens: [Token; 2],
    index: usize,
}

impl TokenSource for SingleTokenSource {
    fn current(&self) -> &Token {
        &self.tokens[self.index]
    }

    fn position(&self) -> usize {
        self.index
    }

    fn advance(&mut self) {
        self.index = (self.index + 1).min(self.tokens.len() - 1);
    }

    fn lookahead(&mut self, n: usize) -> &Token {
        &self.tokens[(self.index + n).min(self.tokens.len() - 1)]
    }

    fn observe(&mut self, _event: ScannerEvent) {}
}

#[test]
fn exposes_the_tasty_api_under_the_dotty_namespace() {
    assert!(NodeCategory::is_known_tag(255));

    let term = SimpleTerm::new(70, TermValue::Int(42)).unwrap();
    let mut writer = Writer::new();
    term.encode(&mut writer).unwrap();

    assert_eq!(writer.as_slice(), &[70, 0xaa]);
}

#[test]
fn resolves_source_files_through_the_public_tasty_facade() {
    let bytes = include_bytes!("../crates/dotty-tasty/tests/fixtures/simple_def/SimpleDef.tasty");
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();

    assert_eq!(
        file.source_file(),
        Ok(Some(
            "IdeaProjects/TastyFixtures/src/main/scala/me/cytrowski/tastyfixtures/SimpleDef.scala",
        ))
    );
}

#[test]
fn validates_and_reencodes_a_file_through_the_public_facade() {
    let bytes = include_bytes!("../crates/dotty-tasty/tests/fixtures/simple_def/SimpleDef.tasty");
    let file = TastyFile::parse_and_validate_scala_3_9(bytes).unwrap();

    assert!(!file.asts().unwrap().is_empty());
    assert_eq!(file.encode().unwrap(), bytes);
}

#[test]
fn exposes_the_core_semantic_api_under_the_dotty_namespace() {
    let mut store = SemanticStore::new();
    let text = store.names.intern("x");
    let name = Name::new(text, Namespace::Term);
    let symbol = store.symbols.alloc(Symbol {
        name,
        owner: None,
        kind: SymbolKind::Value,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Public,
        info: SymbolInfo::Missing,
        origin: SymbolOrigin::Synthetic,
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    });

    assert_eq!(store.symbols.get(symbol).name, name);
}

#[test]
fn exposes_the_parser_api_under_the_dotty_namespace() {
    let source = "x";
    let tokens = SingleTokenSource {
        tokens: [
            Token {
                kind: TokenKind::Identifier,
                span: TextRange::new(0, 1).unwrap(),
                value: TokenValue::None,
            },
            Token {
                kind: TokenKind::Eof,
                span: TextRange::new(1, 1).unwrap(),
                value: TokenValue::None,
            },
        ],
        index: 0,
    };
    let mut names = dotty::core::NameInterner::new();
    let result = parse_compilation_unit(
        dotty::core::SourceText::new(source).unwrap(),
        dotty::core::SourceId::from_index(0),
        tokens,
        &mut names,
    );

    assert!(result.diagnostics.is_empty());
    assert!(matches!(
        result.ast.get(result.root).kind,
        dotty::core::TreeKind::Block(_)
    ));
}

#[test]
fn reencodes_structured_asts_through_the_public_facade() {
    let bytes = include_bytes!("../crates/dotty-tasty/tests/fixtures/simple_def/SimpleDef.tasty");
    let file = TastyFile::parse_and_validate_scala_3_9(bytes).unwrap();
    let original_ast_count = file.asts().unwrap().len();
    let structured = file.structured_asts().unwrap();

    let encoded = file.encode_structured_relocated(&structured).unwrap();
    let reparsed = TastyFile::parse_and_validate_scala_3_9(&encoded).unwrap();

    assert_eq!(reparsed.asts().unwrap().len(), original_ast_count);
    reparsed.validate_ast_reference_targets().unwrap();
    assert_eq!(reparsed.source_file(), file.source_file());
}

#[test]
fn exposes_the_tasty_unpickler_error_under_the_dotty_namespace() {
    use dotty::tasty::AstError;
    use dotty::tasty_unpickler::UnpickleError;

    let error = UnpickleError::from(AstError::InvalidTag { tag: 1, offset: 7 });

    assert_eq!(
        error,
        UnpickleError::Ast(AstError::InvalidTag { tag: 1, offset: 7 })
    );
}

#[test]
fn exposes_an_empty_tasty_semantic_index_through_the_dotty_namespace() {
    use dotty::tasty_unpickler::TastySemanticIndex;

    let index = TastySemanticIndex::new();

    assert_eq!(index.symbol_at(0), None);
    assert_eq!(index.symbol_count(), 0);
}
