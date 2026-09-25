use dotty_core::{
    HardKeyword, Packages, Punctuation, ScannerEvent, SemanticStore, SourceId, SourceText,
    TextRange, Token, TokenKind, TokenSource, TokenValue, TreeKind, TypeName, Visibility,
};
use dotty_namer::name_compilation_unit;
use dotty_parser::parse_compilation_unit;

struct VecTokenSource {
    tokens: Vec<Token>,
    index: usize,
}

impl TokenSource for VecTokenSource {
    fn current(&self) -> &Token {
        &self.tokens[self.index]
    }

    fn position(&self) -> usize {
        self.index
    }

    fn advance(&mut self) {
        if self.index + 1 < self.tokens.len() {
            self.index += 1;
        }
    }

    fn lookahead(&mut self, n: usize) -> &Token {
        let index = self.index.saturating_add(n).min(self.tokens.len() - 1);
        &self.tokens[index]
    }

    fn observe(&mut self, _event: ScannerEvent) {}
}

#[test]
fn an_empty_parsed_compilation_unit_maps_to_the_shared_root_package() {
    let source = SourceId::from_index(7);
    let mut store = SemanticStore::new();
    let parsed = parse_compilation_unit(
        SourceText::new("").expect("empty source is valid"),
        source,
        VecTokenSource {
            tokens: vec![token(TokenKind::Eof, 0, 0)],
            index: 0,
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

    let root_package = packages.get::<&str>(&[]).unwrap();
    assert_eq!(
        index.symbol_at(source, parsed.root),
        Some(root_package.symbol)
    );
    assert_eq!(
        index.scope_of(root_package.symbol),
        Some(root_package.scope)
    );
    assert_ne!(store.checkpoint(), before);
    assert!(packages.get(&["<empty>"]).is_none());
    assert!(packages.is_empty());
}

#[test]
fn a_backquoted_empty_package_identifier_is_not_the_synthetic_default_package() {
    let source_text = "package `<empty>`";
    let source = SourceId::from_index(10);
    let mut store = SemanticStore::new();
    let parsed = parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        VecTokenSource {
            tokens: vec![
                token(TokenKind::Keyword(HardKeyword::Package), 0, 7),
                token(TokenKind::BackquotedIdentifier, 8, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            index: 0,
        },
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty());
    let mut packages = Packages::new();

    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "QuotedPackage.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let named_package = packages.get(&["<empty>"]).unwrap();
    let root_package = packages.get::<&str>(&[]).unwrap();

    assert_ne!(named_package.symbol, root_package.symbol);
    assert_eq!(
        index.symbol_at(source, parsed.root),
        Some(named_package.symbol)
    );
}

#[test]
fn a_parsed_private_class_keeps_its_source_visibility() {
    let source_text = "private class C";
    let source = SourceId::from_index(11);
    let mut store = SemanticStore::new();
    let parsed = parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        VecTokenSource {
            tokens: vec![
                token(TokenKind::Keyword(HardKeyword::Private), 0, 7),
                token(TokenKind::Keyword(HardKeyword::Class), 8, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Eof, 15, 15),
            ],
            index: 0,
        },
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty());
    let mut packages = Packages::new();

    name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "PrivateClass.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let root_package = packages.get::<&str>(&[]).unwrap();
    let class_name = TypeName::new(store.names.intern("C"));
    let class = store
        .scopes
        .get(root_package.scope)
        .lookup(class_name.as_name())
        .unwrap();

    assert_eq!(store.symbols.get(class).visibility, Visibility::Private);
}

fn token(kind: TokenKind, start: u32, end: u32) -> Token {
    Token {
        kind,
        span: TextRange::new(start, end).expect("test token range is valid"),
        value: TokenValue::None,
    }
}

#[test]
fn parsed_qualified_package_name_enters_all_segments() {
    let source_text = "package foo.bar.baz";
    let source = SourceId::from_index(8);
    let mut store = SemanticStore::new();
    let parsed = parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        VecTokenSource {
            tokens: vec![
                token(TokenKind::Keyword(HardKeyword::Package), 0, 7),
                token(TokenKind::Identifier, 8, 11),
                token(TokenKind::Punctuation(Punctuation::Dot), 11, 12),
                token(TokenKind::Identifier, 12, 15),
                token(TokenKind::Punctuation(Punctuation::Dot), 15, 16),
                token(TokenKind::Identifier, 16, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            index: 0,
        },
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty());
    let mut packages = Packages::new();

    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "Qualified.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let leaf = packages.get(&["foo", "bar", "baz"]).unwrap();

    assert_eq!(index.symbol_at(source, parsed.root), Some(leaf.symbol));
    assert!(packages.get(&["foo", "bar"]).is_some());
}

#[test]
fn parsed_nested_package_clauses_extend_the_enclosing_package() {
    let source_text = "package foo { package bar { } }";
    let source = SourceId::from_index(9);
    let mut store = SemanticStore::new();
    let parsed = parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        VecTokenSource {
            tokens: vec![
                token(TokenKind::Keyword(HardKeyword::Package), 0, 7),
                token(TokenKind::Identifier, 8, 11),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 12, 13),
                token(TokenKind::Keyword(HardKeyword::Package), 14, 21),
                token(TokenKind::Identifier, 22, 25),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 26, 27),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 28, 29),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 30, 31),
                token(TokenKind::Eof, 31, 31),
            ],
            index: 0,
        },
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty());
    let dotty_core::TreeKind::PackageDef(root_package) = &parsed.ast.get(parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let nested_tree = root_package.stats[0];
    let mut packages = Packages::new();

    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "Nested.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let nested = packages.get(&["foo", "bar"]).unwrap();

    assert_eq!(index.symbol_at(source, nested_tree), Some(nested.symbol));
    assert_eq!(
        store
            .names
            .resolve(store.symbols.get(nested.symbol).name.text()),
        "bar"
    );
}
