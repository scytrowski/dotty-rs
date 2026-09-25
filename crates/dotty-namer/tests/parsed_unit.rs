use dotty_core::{
    HardKeyword, Packages, Punctuation, ScannerEvent, SemanticStore, SourceId, SourceText,
    SymbolFlags, TextRange, Token, TokenKind, TokenSource, TokenValue, TreeKind, TypeName,
    Visibility,
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

#[test]
fn a_parsed_final_class_sets_the_final_symbol_flag() {
    let source_text = "final class C";
    let source = SourceId::from_index(12);
    let mut store = SemanticStore::new();
    let parsed = parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        VecTokenSource {
            tokens: vec![
                token(TokenKind::Keyword(HardKeyword::Final), 0, 5),
                token(TokenKind::Keyword(HardKeyword::Class), 6, 11),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::Eof, 13, 13),
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
        "FinalClass.scala",
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

    assert!(store.symbols.get(class).flags.contains(SymbolFlags::FINAL));
}

#[test]
fn a_parsed_implicit_class_sets_the_implicit_symbol_flag() {
    let source_text = "implicit class C(x: Int)";
    let source = SourceId::from_index(13);
    let mut store = SemanticStore::new();
    let parsed = parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        VecTokenSource {
            tokens: vec![
                token(TokenKind::Keyword(HardKeyword::Implicit), 0, 8),
                token(TokenKind::Keyword(HardKeyword::Class), 9, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 16, 17),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Punctuation(Punctuation::Colon), 18, 19),
                token(TokenKind::Identifier, 20, 23),
                token(TokenKind::Punctuation(Punctuation::RightParen), 23, 24),
                token(TokenKind::Eof, 24, 24),
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
        "ImplicitClass.scala",
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

    assert!(
        store
            .symbols
            .get(class)
            .flags
            .contains(SymbolFlags::IMPLICIT)
    );
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

#[test]
fn parsed_class_header_parameters_and_constructor_are_named_from_parser_metadata() {
    use dotty_core::{TermName, TypeName};
    use dotty_lexer::ContextualScanner;

    let source_text = "class C[A](x: Int, val y: Int)(var z: Int)";
    let source = SourceId::from_index(27);
    let mut store = SemanticStore::new();
    let scanner = ContextualScanner::new(source_text).expect("source should lex");
    let parsed = parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty());
    let dotty_core::TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let class_tree = package.stats[0];
    let dotty_core::TreeKind::TypeDef(class_def) = &parsed.ast.get(class_tree).kind else {
        panic!("parser should preserve the class TypeDef");
    };
    let dotty_core::TreeKind::Template(template) = &parsed.ast.get(class_def.rhs).kind else {
        panic!("class RHS should be a Template");
    };
    let dotty_core::TreeKind::DefDef(constructor) = &parsed.ast.get(template.constructor).kind
    else {
        panic!("template constructor should be a DefDef");
    };
    assert_eq!(constructor.type_params.len(), 1);
    assert_eq!(constructor.value_param_clauses.len(), 2);
    let mut packages = Packages::new();

    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "Header.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let root_package = packages.get::<&str>(&[]).unwrap();
    let class_name = *class_def.name.as_name();
    let class_symbol = store
        .scopes
        .get(root_package.scope)
        .lookup(&class_name)
        .unwrap();
    let class_scope = index.scope_of(class_symbol).unwrap();
    let type_parameter_name = TypeName::new(store.names.intern("A"));
    let type_parameter_symbol = store
        .scopes
        .get(class_scope)
        .lookup(type_parameter_name.as_name())
        .unwrap();
    let x_name = TermName::new(store.names.intern("x"));
    let y_name = TermName::new(store.names.intern("y"));
    let z_name = TermName::new(store.names.intern("z"));
    let constructor_name = TermName::new(store.names.intern("<init>"));
    let x_tree = constructor.value_param_clauses[0][0];
    let y_tree = constructor.value_param_clauses[0][1];
    let z_tree = constructor.value_param_clauses[1][0];
    let x_symbol = index.symbol_at(source, x_tree).unwrap();
    let y_symbol = store
        .scopes
        .get(class_scope)
        .lookup(y_name.as_name())
        .unwrap();
    let z_symbol = store
        .scopes
        .get(class_scope)
        .lookup(z_name.as_name())
        .unwrap();
    let constructor_symbol = store
        .scopes
        .get(class_scope)
        .lookup(constructor_name.as_name())
        .unwrap();

    assert_eq!(
        store.symbols.get(type_parameter_symbol).kind,
        dotty_core::SymbolKind::TypeParameter
    );
    assert_eq!(
        index.symbol_at(source, constructor.type_params[0]),
        Some(type_parameter_symbol)
    );
    assert_eq!(
        store.symbols.get(x_symbol).kind,
        dotty_core::SymbolKind::Parameter
    );
    assert_eq!(store.scopes.get(class_scope).lookup(x_name.as_name()), None);
    assert_eq!(
        store.symbols.get(y_symbol).kind,
        dotty_core::SymbolKind::Field
    );
    assert_eq!(
        store.symbols.get(z_symbol).kind,
        dotty_core::SymbolKind::Field
    );
    assert_eq!(index.symbol_at(source, y_tree), Some(y_symbol));
    assert_eq!(index.symbol_at(source, z_tree), Some(z_symbol));
    assert_eq!(
        store.symbols.get(constructor_symbol).kind,
        dotty_core::SymbolKind::Constructor
    );
    assert_eq!(
        index.symbol_at(source, template.constructor),
        Some(constructor_symbol)
    );
}
