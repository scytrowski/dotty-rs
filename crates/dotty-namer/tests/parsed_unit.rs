use dotty_core::{
    HardKeyword, Packages, Punctuation, ScannerEvent, SemanticStore, SourceId, SourceText,
    SymbolFlags, TextRange, Token, TokenKind, TokenSource, TokenValue, TreeKind, TypeName,
    Visibility,
};
use dotty_namer::{NamerError, SourceContextId, SourceSemanticIndex, name_compilation_unit};
use dotty_parser::{ParseDiagnosticKind, parse_compilation_unit};

struct NamedSource {
    parsed: dotty_parser::ParseResult,
    source: SourceId,
    store: SemanticStore,
    index: SourceSemanticIndex,
}

fn parsed_source(
    source_text: &str,
    source_index: u32,
) -> (dotty_parser::ParseResult, SourceId, SemanticStore) {
    let source = SourceId::from_index(source_index);
    let mut store = SemanticStore::new();
    let scanner = dotty_lexer::ContextualScanner::new(source_text).expect("source should lex");
    let parsed = parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    (parsed, source, store)
}

fn named_source(source_text: &str, source_index: u32) -> NamedSource {
    let (parsed, source, mut store) = parsed_source(source_text, source_index);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "Parameters.scala",
        &mut store,
        &mut packages,
    )
    .expect("valid source should be named");
    NamedSource {
        parsed,
        source,
        store,
        index,
    }
}

fn named_source_tree_name(
    named: &NamedSource,
    tree: dotty_core::TreeId<dotty_core::Untyped>,
) -> String {
    let symbol = named
        .index
        .symbol_at(named.source, tree)
        .expect("source definition should have a symbol");
    let name = named.store.symbols.get(symbol).name;
    named.store.names.resolve(name.text()).to_owned()
}

fn declaration_context(
    named: &NamedSource,
    tree: dotty_core::TreeId<dotty_core::Untyped>,
) -> (dotty_core::SymbolId, SourceContextId) {
    let symbol = named
        .index
        .symbol_at(named.source, tree)
        .expect("source declaration should have a symbol");
    let context = named
        .index
        .declaration_context_of(symbol)
        .expect("source declaration should have a declaration context");
    (symbol, context)
}

fn package_stat_trees(named: &NamedSource) -> &[dotty_core::TreeId<dotty_core::Untyped>] {
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    &package.stats
}

fn source_pattern_bindings(
    named: &NamedSource,
    patterns: &[dotty_core::TreeId<dotty_core::Untyped>],
) -> Vec<(dotty_core::TreeId<dotty_core::Untyped>, String)> {
    let mut bindings = Vec::new();
    let mut pending = patterns.iter().rev().copied().collect::<Vec<_>>();
    while let Some(tree) = pending.pop() {
        match &named.parsed.ast.get(tree).kind {
            TreeKind::Ident(ident) => bindings.push((
                tree,
                named.store.names.resolve(ident.name.text()).to_owned(),
            )),
            TreeKind::Bind(binding) => {
                bindings.push((
                    tree,
                    named.store.names.resolve(binding.name.text()).to_owned(),
                ));
                pending.push(binding.body);
            }
            TreeKind::Typed(typed) => pending.push(typed.expr),
            TreeKind::Apply(application) => {
                pending.extend(application.args.iter().rev().copied());
            }
            TreeKind::Alternative(alternative) => {
                if let Some(first) = alternative.alternatives.first() {
                    pending.push(*first);
                }
            }
            TreeKind::UnApply(unapply) => {
                pending.extend(unapply.patterns.iter().rev().copied());
            }
            TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::Parens(parens)) => {
                pending.push(parens.inner);
            }
            TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::Tuple(tuple)) => {
                pending.extend(tuple.elements.iter().rev().copied());
            }
            TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::InfixOp(infix)) => {
                pending.push(infix.right);
                pending.push(infix.left);
            }
            _ => {}
        }
    }
    bindings.retain(|(_, name)| name != "_");
    bindings
}

#[test]
fn anonymous_given_alias_gets_a_deterministic_invented_name() {
    let named = named_source("given Config = makeConfig", 105);
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };

    assert_eq!(
        named_source_tree_name(&named, package.stats[0]),
        "given_Config"
    );
    let repeated = named_source("given Config = makeConfig", 110);
    let TreeKind::PackageDef(repeated_package) =
        &repeated.parsed.ast.get(repeated.parsed.root).kind
    else {
        panic!("parser should return a package root");
    };
    assert_eq!(
        named_source_tree_name(&repeated, repeated_package.stats[0]),
        "given_Config"
    );
}

#[test]
fn explicitly_named_given_keeps_its_source_name() {
    let named = named_source("given config: Config = makeConfig", 106);
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };

    assert_eq!(named_source_tree_name(&named, package.stats[0]), "config");
}

#[test]
fn anonymous_given_field_inside_a_class_is_normalized_before_symbol_entry() {
    let named = named_source("class C:\n  given Config = makeConfig", 115);
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::TypeDef(class) = &named.parsed.ast.get(package.stats[0]).kind else {
        panic!("class declaration should be a TypeDef");
    };
    let TreeKind::Template(template) = &named.parsed.ast.get(class.rhs).kind else {
        panic!("class RHS should be a Template");
    };

    assert_eq!(
        named_source_tree_name(&named, template.body[0]),
        "given_Config"
    );
}

#[test]
fn anonymous_method_like_given_uses_its_declared_result_type() {
    let named = named_source(
        "given [A] => (using ctx: Ctx[A]) => Show[A] = makeShow",
        107,
    );
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::DefDef(definition) = &named.parsed.ast.get(package.stats[0]).kind else {
        panic!("parameterized given alias should be a DefDef");
    };

    assert_eq!(
        named_source_tree_name(&named, package.stats[0]),
        "given_Show_A"
    );
    assert_eq!(definition.type_params.len(), 1);
    assert_eq!(definition.value_param_clauses.len(), 1);
}

#[test]
fn anonymous_structural_module_given_derives_object_and_module_class_names() {
    use dotty_core::{SymbolKind, TypeName};

    let named = named_source("given Ordering[Int]:\n  def compare = 0", 108);
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let given = package.stats[0];
    let symbol = named.index.symbol_at(named.source, given).unwrap();
    let package_symbol = named.store.symbols.get(symbol).owner.unwrap();
    let package_scope = named.index.scope_of(package_symbol).unwrap();
    let module_class_name = TypeName::new(
        named
            .store
            .names
            .get("given_Ordering_Int$")
            .expect("normalized module class name should be interned"),
    );
    let module_class = named
        .store
        .scopes
        .get(package_scope)
        .lookup(module_class_name.as_name())
        .expect("module class should use normalized object name");

    assert_eq!(named_source_tree_name(&named, given), "given_Ordering_Int");
    assert_eq!(
        named.store.symbols.get(module_class).kind,
        SymbolKind::ModuleClass
    );
}

#[test]
fn anonymous_structural_given_joins_multiple_parent_names_in_source_order() {
    let named = named_source("given First with Second:\n  def value = result", 111);
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };

    assert_eq!(
        named_source_tree_name(&named, package.stats[0]),
        "given_First_Second"
    );
}

#[test]
fn structurally_identical_anonymous_givens_keep_the_same_name_without_renaming() {
    let named = named_source("given Config = first\ngiven Config = second", 112);
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let first = named
        .index
        .symbol_at(named.source, package.stats[0])
        .unwrap();
    let second = named
        .index
        .symbol_at(named.source, package.stats[1])
        .unwrap();

    assert_ne!(first, second);
    assert_eq!(
        named.store.symbols.get(first).name,
        named.store.symbols.get(second).name
    );
    assert_eq!(
        named_source_tree_name(&named, package.stats[0]),
        "given_Config"
    );
    assert_eq!(
        named_source_tree_name(&named, package.stats[1]),
        "given_Config"
    );
}

#[test]
fn anonymous_parameterized_structural_given_uses_a_type_namespace_name() {
    use dotty_core::Namespace;

    let named = named_source("given [A] => Show[A]:\n  def show = result", 109);
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let given = package.stats[0];
    let symbol = named.index.symbol_at(named.source, given).unwrap();

    assert_eq!(named_source_tree_name(&named, given), "given_Show_A");
    assert_eq!(
        named.store.symbols.get(symbol).name.namespace(),
        Namespace::Type
    );
}

struct PrimaryConstructorParts {
    class_tree: dotty_core::TreeId<dotty_core::Untyped>,
    constructor_tree: dotty_core::TreeId<dotty_core::Untyped>,
    type_params: Vec<dotty_core::TreeId<dotty_core::Untyped>>,
    value_param_clauses: Vec<Vec<dotty_core::TreeId<dotty_core::Untyped>>>,
}

fn primary_constructor_parts(named: &NamedSource) -> PrimaryConstructorParts {
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let class_tree = package.stats[0];
    let TreeKind::TypeDef(class) = &named.parsed.ast.get(class_tree).kind else {
        panic!("source declaration should be a TypeDef");
    };
    let TreeKind::Template(template) = &named.parsed.ast.get(class.rhs).kind else {
        panic!("class RHS should be a Template");
    };
    let constructor_tree = template.constructor;
    let TreeKind::DefDef(constructor) = &named.parsed.ast.get(constructor_tree).kind else {
        panic!("primary constructor should be a DefDef");
    };
    PrimaryConstructorParts {
        class_tree,
        constructor_tree,
        type_params: constructor.type_params.clone(),
        value_param_clauses: constructor.value_param_clauses.clone(),
    }
}

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

    assert_eq!(
        store.symbols.get(class).visibility,
        Visibility::PrivateWithin(root_package.symbol)
    );
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

    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "ImplicitClass.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let root_package = packages.get::<&str>(&[]).unwrap();
    let wrapper_name = TypeName::new(store.names.intern("ImplicitClass$package$"));
    let wrapper = store
        .scopes
        .get(root_package.scope)
        .lookup(wrapper_name.as_name())
        .unwrap();
    let wrapper_scope = index.scope_of(wrapper).unwrap();
    let class_name = TypeName::new(store.names.intern("C"));
    let class = store
        .scopes
        .get(wrapper_scope)
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
    let type_parameter_symbol = index.symbol_at(source, constructor.type_params[0]).unwrap();
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
    let constructor_scope = index.scope_of(constructor_symbol).unwrap();
    let derived_type_parameter = index
        .derived_symbol_at(constructor_symbol, source, constructor.type_params[0])
        .unwrap();
    let derived_x = index
        .derived_symbol_at(constructor_symbol, source, x_tree)
        .unwrap();
    let derived_y = index
        .derived_symbol_at(constructor_symbol, source, y_tree)
        .unwrap();
    let derived_z = index
        .derived_symbol_at(constructor_symbol, source, z_tree)
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
        store
            .scopes
            .get(class_scope)
            .lookup(type_parameter_name.as_name()),
        Some(type_parameter_symbol)
    );
    assert_eq!(
        store.symbols.get(x_symbol).kind,
        dotty_core::SymbolKind::Parameter
    );
    assert_eq!(store.scopes.get(class_scope).lookup(x_name.as_name()), None);
    assert_eq!(store.symbols.get(x_symbol).visibility, Visibility::Private);
    assert_eq!(store.symbols.get(x_symbol).owner, Some(class_symbol));
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
    for (canonical, derived) in [
        (type_parameter_symbol, derived_type_parameter),
        (x_symbol, derived_x),
        (y_symbol, derived_y),
        (z_symbol, derived_z),
    ] {
        assert_ne!(canonical, derived);
        assert_eq!(store.symbols.get(derived).owner, Some(constructor_symbol));
        assert_eq!(
            store.symbols.get(derived).info,
            dotty_core::SymbolInfo::Missing
        );
    }
    assert_eq!(
        store.symbols.get(derived_x).kind,
        dotty_core::SymbolKind::Parameter
    );
    assert_eq!(
        store.symbols.get(derived_y).kind,
        dotty_core::SymbolKind::Parameter
    );
    assert_eq!(
        store.symbols.get(derived_z).kind,
        dotty_core::SymbolKind::Parameter
    );
    assert_eq!(
        store.symbols.get(derived_z).flags,
        dotty_core::SymbolFlags::EMPTY
    );
    assert_eq!(
        store.scopes.get(constructor_scope).owner,
        Some(constructor_symbol)
    );
    assert_eq!(
        store.scopes.get(constructor_scope).lookup(x_name.as_name()),
        Some(derived_x)
    );
    assert_eq!(
        store.scopes.get(constructor_scope).lookup(y_name.as_name()),
        Some(derived_y)
    );
    assert_eq!(
        store.scopes.get(constructor_scope).lookup(z_name.as_name()),
        Some(derived_z)
    );
    assert_eq!(
        store.symbols.get(constructor_symbol).kind,
        dotty_core::SymbolKind::Constructor
    );
    assert_eq!(
        index.symbol_at(source, template.constructor),
        Some(constructor_symbol)
    );
}

#[test]
fn parsed_private_local_constructor_parameter_is_not_a_class_member() {
    use dotty_core::{SymbolInfo, SymbolKind, TermName};

    let mut named = named_source("class C(x: Int)", 95);
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let class_tree = package.stats[0];
    let TreeKind::TypeDef(class) = &named.parsed.ast.get(class_tree).kind else {
        panic!("source declaration should be a TypeDef");
    };
    let TreeKind::Template(template) = &named.parsed.ast.get(class.rhs).kind else {
        panic!("class RHS should be a Template");
    };
    let TreeKind::DefDef(constructor) = &named.parsed.ast.get(template.constructor).kind else {
        panic!("primary constructor should be a DefDef");
    };
    let parameter_tree = constructor.value_param_clauses[0][0];
    let class_symbol = named.index.symbol_at(named.source, class_tree).unwrap();
    let class_scope = named.index.scope_of(class_symbol).unwrap();
    let constructor_symbol = named
        .index
        .symbol_at(named.source, template.constructor)
        .unwrap();
    let constructor_scope = named.index.scope_of(constructor_symbol).unwrap();
    let canonical = named.index.symbol_at(named.source, parameter_tree).unwrap();
    let derived = named
        .index
        .derived_symbol_at(constructor_symbol, named.source, parameter_tree)
        .unwrap();
    let parameter_name = TermName::new(named.store.names.intern("x"));

    assert_eq!(
        named.store.symbols.get(canonical).kind,
        SymbolKind::Parameter
    );
    assert_eq!(named.store.symbols.get(canonical).owner, Some(class_symbol));
    assert_eq!(
        named.store.symbols.get(canonical).visibility,
        Visibility::Private
    );
    assert_eq!(
        named
            .store
            .scopes
            .get(class_scope)
            .lookup(parameter_name.as_name()),
        None
    );
    assert_eq!(named.store.symbols.get(derived).kind, SymbolKind::Parameter);
    assert_eq!(
        named.store.symbols.get(derived).owner,
        Some(constructor_symbol)
    );
    assert_eq!(named.store.symbols.get(derived).info, SymbolInfo::Missing);
    assert_eq!(
        named
            .store
            .scopes
            .get(constructor_scope)
            .lookup(parameter_name.as_name()),
        Some(derived)
    );
}

#[test]
fn parsed_val_constructor_parameter_is_a_field_and_a_constructor_parameter() {
    use dotty_core::{SymbolInfo, SymbolKind, TermName};

    let mut named = named_source("class C(val x: Int)", 96);
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let class_tree = package.stats[0];
    let TreeKind::TypeDef(class) = &named.parsed.ast.get(class_tree).kind else {
        panic!("source declaration should be a TypeDef");
    };
    let TreeKind::Template(template) = &named.parsed.ast.get(class.rhs).kind else {
        panic!("class RHS should be a Template");
    };
    let TreeKind::DefDef(constructor) = &named.parsed.ast.get(template.constructor).kind else {
        panic!("primary constructor should be a DefDef");
    };
    let parameter_tree = constructor.value_param_clauses[0][0];
    let class_symbol = named.index.symbol_at(named.source, class_tree).unwrap();
    let class_scope = named.index.scope_of(class_symbol).unwrap();
    let constructor_symbol = named
        .index
        .symbol_at(named.source, template.constructor)
        .unwrap();
    let constructor_scope = named.index.scope_of(constructor_symbol).unwrap();
    let canonical = named.index.symbol_at(named.source, parameter_tree).unwrap();
    let derived = named
        .index
        .derived_symbol_at(constructor_symbol, named.source, parameter_tree)
        .unwrap();
    let parameter_name = TermName::new(named.store.names.intern("x"));

    assert_ne!(canonical, derived);
    assert_eq!(named.store.symbols.get(canonical).kind, SymbolKind::Field);
    assert_eq!(named.store.symbols.get(canonical).owner, Some(class_symbol));
    assert_eq!(
        named
            .store
            .scopes
            .get(class_scope)
            .lookup(parameter_name.as_name()),
        Some(canonical)
    );
    assert_eq!(named.store.symbols.get(derived).kind, SymbolKind::Parameter);
    assert_eq!(
        named.store.symbols.get(derived).owner,
        Some(constructor_symbol)
    );
    assert_eq!(named.store.symbols.get(derived).info, SymbolInfo::Missing);
    assert_eq!(
        named
            .store
            .scopes
            .get(constructor_scope)
            .lookup(parameter_name.as_name()),
        Some(derived)
    );
}

#[test]
fn parsed_var_constructor_parameter_keeps_mutability_only_on_its_field() {
    use dotty_core::{SymbolFlags, SymbolKind, TermName};

    let mut named = named_source("class C(var x: Int)", 97);
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let class_tree = package.stats[0];
    let TreeKind::TypeDef(class) = &named.parsed.ast.get(class_tree).kind else {
        panic!("source declaration should be a TypeDef");
    };
    let TreeKind::Template(template) = &named.parsed.ast.get(class.rhs).kind else {
        panic!("class RHS should be a Template");
    };
    let TreeKind::DefDef(constructor) = &named.parsed.ast.get(template.constructor).kind else {
        panic!("primary constructor should be a DefDef");
    };
    let parameter_tree = constructor.value_param_clauses[0][0];
    let class_symbol = named.index.symbol_at(named.source, class_tree).unwrap();
    let constructor_symbol = named
        .index
        .symbol_at(named.source, template.constructor)
        .unwrap();
    let canonical = named.index.symbol_at(named.source, parameter_tree).unwrap();
    let derived = named
        .index
        .derived_symbol_at(constructor_symbol, named.source, parameter_tree)
        .unwrap();
    let parameter_name = TermName::new(named.store.names.intern("x"));

    assert_eq!(named.store.symbols.get(canonical).kind, SymbolKind::Field);
    assert_eq!(named.store.symbols.get(canonical).owner, Some(class_symbol));
    assert!(
        named
            .store
            .symbols
            .get(canonical)
            .flags
            .contains(SymbolFlags::MUTABLE)
    );
    assert_eq!(named.store.symbols.get(derived).kind, SymbolKind::Parameter);
    assert!(
        !named
            .store
            .symbols
            .get(derived)
            .flags
            .contains(SymbolFlags::MUTABLE)
    );
    assert_eq!(
        named
            .store
            .scopes
            .get(named.index.scope_of(constructor_symbol).unwrap())
            .lookup(parameter_name.as_name()),
        Some(derived)
    );
}

#[test]
fn parsed_case_class_first_clause_parameter_is_a_field_and_constructor_copy() {
    use dotty_core::{SymbolInfo, SymbolKind, TermName};

    let mut named = named_source("case class C(x: Int)", 98);
    let parts = primary_constructor_parts(&named);
    let parameter_tree = parts.value_param_clauses[0][0];
    let class_symbol = named
        .index
        .symbol_at(named.source, parts.class_tree)
        .unwrap();
    let class_scope = named.index.scope_of(class_symbol).unwrap();
    let constructor_symbol = named
        .index
        .symbol_at(named.source, parts.constructor_tree)
        .unwrap();
    let constructor_scope = named.index.scope_of(constructor_symbol).unwrap();
    let canonical = named.index.symbol_at(named.source, parameter_tree).unwrap();
    let derived = named
        .index
        .derived_symbol_at(constructor_symbol, named.source, parameter_tree)
        .unwrap();
    let parameter_name = TermName::new(named.store.names.intern("x"));

    assert_eq!(named.store.symbols.get(canonical).kind, SymbolKind::Field);
    assert_eq!(named.store.symbols.get(canonical).owner, Some(class_symbol));
    assert_eq!(named.store.symbols.get(derived).kind, SymbolKind::Parameter);
    assert_eq!(
        named.store.symbols.get(derived).owner,
        Some(constructor_symbol)
    );
    assert_eq!(named.store.symbols.get(derived).info, SymbolInfo::Missing);
    assert_eq!(
        named
            .store
            .scopes
            .get(class_scope)
            .lookup(parameter_name.as_name()),
        Some(canonical)
    );
    assert_eq!(
        named
            .store
            .scopes
            .get(constructor_scope)
            .lookup(parameter_name.as_name()),
        Some(derived)
    );
}

#[test]
fn parsed_using_constructor_parameter_preserves_contextual_flag_on_its_copy() {
    use dotty_core::{SymbolFlags, SymbolKind};

    let named = named_source("class C(using x: Int)", 99);
    let parts = primary_constructor_parts(&named);
    let parameter_tree = parts.value_param_clauses[0][0];
    let constructor_symbol = named
        .index
        .symbol_at(named.source, parts.constructor_tree)
        .unwrap();
    let canonical = named.index.symbol_at(named.source, parameter_tree).unwrap();
    let derived = named
        .index
        .derived_symbol_at(constructor_symbol, named.source, parameter_tree)
        .unwrap();

    assert_eq!(
        named.store.symbols.get(canonical).kind,
        SymbolKind::Parameter
    );
    assert!(
        named
            .store
            .symbols
            .get(canonical)
            .flags
            .contains(SymbolFlags::GIVEN)
    );
    assert_eq!(named.store.symbols.get(derived).kind, SymbolKind::Parameter);
    assert!(
        named
            .store
            .symbols
            .get(derived)
            .flags
            .contains(SymbolFlags::GIVEN)
    );
    assert!(
        !named
            .store
            .symbols
            .get(derived)
            .flags
            .contains(SymbolFlags::MUTABLE)
    );
}

#[test]
fn parsed_constructor_identity_graph_matches_tasty_parameter_ownership() {
    use dotty_core::{SymbolInfo, SymbolKind, TermName, TypeName};

    // Mirrors the class/constructor identity expectations covered by
    // dotty-tasty-unpickler/tests/enter_symbols.rs and scopes_and_identity.rs.
    let mut named = named_source("class C[A](val x: Int)", 100);
    let parts = primary_constructor_parts(&named);
    let class_symbol = named
        .index
        .symbol_at(named.source, parts.class_tree)
        .unwrap();
    let class_scope = named.index.scope_of(class_symbol).unwrap();
    let constructor_symbol = named
        .index
        .symbol_at(named.source, parts.constructor_tree)
        .unwrap();
    let constructor_scope = named.index.scope_of(constructor_symbol).unwrap();
    let class_parameter = named
        .index
        .symbol_at(named.source, parts.type_params[0])
        .unwrap();
    let constructor_type_parameter = named
        .index
        .derived_symbol_at(constructor_symbol, named.source, parts.type_params[0])
        .unwrap();
    let class_field = named
        .index
        .symbol_at(named.source, parts.value_param_clauses[0][0])
        .unwrap();
    let constructor_parameter = named
        .index
        .derived_symbol_at(
            constructor_symbol,
            named.source,
            parts.value_param_clauses[0][0],
        )
        .unwrap();
    let type_name = TypeName::new(named.store.names.intern("A"));
    let term_name = TermName::new(named.store.names.intern("x"));

    assert_eq!(
        named.store.symbols.get(class_parameter).kind,
        SymbolKind::TypeParameter
    );
    assert_eq!(
        named.store.symbols.get(class_parameter).owner,
        Some(class_symbol)
    );
    assert_eq!(named.store.symbols.get(class_field).kind, SymbolKind::Field);
    assert_eq!(
        named.store.symbols.get(class_field).owner,
        Some(class_symbol)
    );
    assert_eq!(
        named.store.symbols.get(constructor_type_parameter).kind,
        SymbolKind::TypeParameter
    );
    assert_eq!(
        named.store.symbols.get(constructor_type_parameter).owner,
        Some(constructor_symbol)
    );
    assert_eq!(
        named.store.symbols.get(constructor_parameter).kind,
        SymbolKind::Parameter
    );
    assert_eq!(
        named.store.symbols.get(constructor_parameter).owner,
        Some(constructor_symbol)
    );
    assert_ne!(class_parameter, constructor_type_parameter);
    assert_ne!(class_field, constructor_parameter);
    assert_eq!(
        named.store.symbols.get(constructor_type_parameter).info,
        SymbolInfo::Missing
    );
    assert_eq!(
        named.store.symbols.get(constructor_parameter).info,
        SymbolInfo::Missing
    );
    assert_eq!(
        named.store.symbols.get(constructor_type_parameter).origin,
        dotty_core::SymbolOrigin::Source(named.source)
    );
    assert_eq!(
        named.store.symbols.get(constructor_parameter).origin,
        dotty_core::SymbolOrigin::Source(named.source)
    );
    assert_eq!(
        named.store.symbols.get(constructor_type_parameter).position,
        named.parsed.ast.get(parts.type_params[0]).position
    );
    assert_eq!(
        named.store.symbols.get(constructor_parameter).position,
        named
            .parsed
            .ast
            .get(parts.value_param_clauses[0][0])
            .position
    );
    assert_eq!(
        named
            .store
            .scopes
            .get(class_scope)
            .lookup(type_name.as_name()),
        Some(class_parameter)
    );
    assert_eq!(
        named
            .store
            .scopes
            .get(class_scope)
            .lookup(term_name.as_name()),
        Some(class_field)
    );
    assert_eq!(
        named
            .store
            .scopes
            .get(constructor_scope)
            .lookup(type_name.as_name()),
        Some(constructor_type_parameter)
    );
    assert_eq!(
        named
            .store
            .scopes
            .get(constructor_scope)
            .lookup(term_name.as_name()),
        Some(constructor_parameter)
    );
}

#[test]
fn parsed_class_type_parameter_has_distinct_class_and_constructor_identities() {
    use dotty_core::{SymbolInfo, SymbolKind, TypeName};
    use dotty_lexer::ContextualScanner;

    let source_text = "class C[A]";
    let source = SourceId::from_index(94);
    let mut store = SemanticStore::new();
    let scanner = ContextualScanner::new(source_text).expect("source should lex");
    let parsed = parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let class_tree = package.stats[0];
    let TreeKind::TypeDef(class) = &parsed.ast.get(class_tree).kind else {
        panic!("source declaration should be a TypeDef");
    };
    let TreeKind::Template(template) = &parsed.ast.get(class.rhs).kind else {
        panic!("class RHS should be a Template");
    };
    let TreeKind::DefDef(constructor) = &parsed.ast.get(template.constructor).kind else {
        panic!("primary constructor should be a DefDef");
    };
    let parameter_tree = constructor.type_params[0];
    let mut packages = Packages::new();

    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "Generic.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let class_symbol = index.symbol_at(source, class_tree).unwrap();
    let class_scope = index.scope_of(class_symbol).unwrap();
    let constructor_symbol = index.symbol_at(source, template.constructor).unwrap();
    let constructor_scope = index.scope_of(constructor_symbol).unwrap();
    let canonical = index.symbol_at(source, parameter_tree).unwrap();
    let derived = index
        .derived_symbol_at(constructor_symbol, source, parameter_tree)
        .unwrap();
    let type_name = TypeName::new(store.names.intern("A"));

    assert_ne!(canonical, derived);
    assert_eq!(store.symbols.get(canonical).kind, SymbolKind::TypeParameter);
    assert_eq!(store.symbols.get(canonical).owner, Some(class_symbol));
    assert_eq!(store.symbols.get(canonical).visibility, Visibility::Private);
    assert_eq!(store.symbols.get(derived).kind, SymbolKind::TypeParameter);
    assert_eq!(store.symbols.get(derived).owner, Some(constructor_symbol));
    assert_eq!(store.symbols.get(derived).info, SymbolInfo::Missing);
    assert_eq!(
        store.scopes.get(class_scope).lookup(type_name.as_name()),
        Some(canonical)
    );
    assert_eq!(
        store
            .scopes
            .get(constructor_scope)
            .lookup(type_name.as_name()),
        Some(derived)
    );
    assert_eq!(index.symbol_at(source, parameter_tree), Some(canonical));
    assert_eq!(
        index.derived_symbol_at(constructor_symbol, source, parameter_tree),
        Some(derived)
    );
}

#[test]
fn parsed_class_members_and_method_parameters_get_their_own_scopes() {
    use dotty_core::TermName;
    use dotty_lexer::ContextualScanner;

    let source_text = "class C { val member: Int; def convert[A](x: Int)(y: Int): Int = x; type Alias = Int }\nval topValue: Int = 0\ndef top = 0\ntype TopAlias = Int";
    let source = SourceId::from_index(39);
    let mut store = SemanticStore::new();
    let scanner = ContextualScanner::new(source_text).expect("source should lex");
    let parsed = parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let class_tree = package.stats[0];
    let TreeKind::TypeDef(class_definition) = &parsed.ast.get(class_tree).kind else {
        panic!("first package statement should be the class");
    };
    let TreeKind::Template(template) = &parsed.ast.get(class_definition.rhs).kind else {
        panic!("class RHS should be a Template");
    };
    let field_tree = template
        .body
        .iter()
        .copied()
        .find(|tree| matches!(parsed.ast.get(*tree).kind, TreeKind::ValDef(_)))
        .unwrap();
    let method_tree = template
        .body
        .iter()
        .copied()
        .find(|tree| matches!(parsed.ast.get(*tree).kind, TreeKind::DefDef(_)))
        .unwrap();
    let TreeKind::DefDef(method) = &parsed.ast.get(method_tree).kind else {
        unreachable!("method tree was found above");
    };
    let alias_tree = template
        .body
        .iter()
        .copied()
        .find(|tree| match &parsed.ast.get(*tree).kind {
            TreeKind::TypeDef(definition) => {
                !matches!(parsed.ast.get(definition.rhs).kind, TreeKind::Template(_))
            }
            _ => false,
        })
        .unwrap();
    let method_body = method.rhs.unwrap();
    let top_level_value = package.stats[1];
    let top_level_method = package.stats[2];
    let top_level_alias = package.stats[3];
    let mut packages = Packages::new();

    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "Members.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let root_package = packages.get::<&str>(&[]).unwrap();
    let class_name = *class_definition.name.as_name();
    let class_symbol = store
        .scopes
        .get(root_package.scope)
        .lookup(&class_name)
        .unwrap();
    let class_scope = index.scope_of(class_symbol).unwrap();
    let wrapper_name = TypeName::new(store.names.intern("Members$package$"));
    let wrapper_class = store
        .scopes
        .get(root_package.scope)
        .lookup(wrapper_name.as_name())
        .unwrap();
    let wrapper_scope = index.scope_of(wrapper_class).unwrap();
    let field_tree_name = match &parsed.ast.get(field_tree).kind {
        TreeKind::ValDef(field) => *field.name.as_name(),
        _ => unreachable!(),
    };
    let field_symbol = store
        .scopes
        .get(class_scope)
        .lookup(&field_tree_name)
        .unwrap();
    let method_symbol = index.symbol_at(source, method_tree).unwrap();
    let method_scope = index.scope_of(method_symbol).unwrap();
    let method_type_name = match &parsed.ast.get(method.type_params[0]).kind {
        TreeKind::TypeDef(parameter) => *parameter.name.as_name(),
        _ => unreachable!(),
    };
    let method_type_symbol = store
        .scopes
        .get(method_scope)
        .lookup(&method_type_name)
        .unwrap();
    let method_value_names = method
        .value_param_clauses
        .iter()
        .map(|clause| match &parsed.ast.get(clause[0]).kind {
            TreeKind::ValDef(parameter) => *parameter.name.as_name(),
            _ => unreachable!(),
        })
        .collect::<Vec<_>>();
    let method_value_symbols = method_value_names
        .iter()
        .map(|name| store.scopes.get(method_scope).lookup(name).unwrap())
        .collect::<Vec<_>>();
    let alias_name = match &parsed.ast.get(alias_tree).kind {
        TreeKind::TypeDef(alias) => *alias.name.as_name(),
        _ => unreachable!(),
    };
    let alias_symbol = store.scopes.get(class_scope).lookup(&alias_name).unwrap();
    let top_value_name = TermName::new(store.names.intern("topValue"));
    let top_name = TermName::new(store.names.intern("top"));

    assert_eq!(
        store.symbols.get(field_symbol).kind,
        dotty_core::SymbolKind::Field
    );
    assert_eq!(store.symbols.get(field_symbol).owner, Some(class_symbol));
    assert_eq!(index.symbol_at(source, field_tree), Some(field_symbol));
    assert_eq!(
        store.symbols.get(method_symbol).kind,
        dotty_core::SymbolKind::Method
    );
    assert_eq!(store.symbols.get(method_symbol).owner, Some(class_symbol));
    assert_eq!(store.scopes.get(method_scope).owner, Some(method_symbol));
    assert_eq!(
        store.symbols.get(method_type_symbol).kind,
        dotty_core::SymbolKind::TypeParameter
    );
    assert_eq!(
        store.symbols.get(method_type_symbol).owner,
        Some(method_symbol)
    );
    assert_eq!(
        index.symbol_at(source, method.type_params[0]),
        Some(method_type_symbol)
    );
    assert_eq!(
        store.symbols.get(method_value_symbols[0]).kind,
        dotty_core::SymbolKind::Parameter
    );
    assert_eq!(
        store.symbols.get(method_value_symbols[1]).kind,
        dotty_core::SymbolKind::Parameter
    );
    assert_eq!(
        store.symbols.get(method_value_symbols[0]).owner,
        Some(method_symbol)
    );
    assert_eq!(
        store.symbols.get(method_value_symbols[1]).owner,
        Some(method_symbol)
    );
    assert_eq!(
        index.symbol_at(source, method.value_param_clauses[0][0]),
        Some(method_value_symbols[0])
    );
    assert_eq!(
        index.symbol_at(source, method.value_param_clauses[1][0]),
        Some(method_value_symbols[1])
    );
    assert_eq!(
        store.scopes.get(class_scope).lookup(&method_value_names[0]),
        None
    );
    assert_eq!(
        store.scopes.get(class_scope).lookup(&method_value_names[1]),
        None
    );
    assert_eq!(
        store.symbols.get(alias_symbol).kind,
        dotty_core::SymbolKind::TypeAlias
    );
    assert_eq!(store.symbols.get(alias_symbol).owner, Some(class_symbol));
    assert_eq!(
        store.symbols.get(alias_symbol).info,
        dotty_core::SymbolInfo::Missing
    );
    assert_eq!(index.symbol_at(source, alias_tree), Some(alias_symbol));
    assert_eq!(index.symbol_at(source, method_body), None);
    let top_value_symbol = index.symbol_at(source, top_level_value).unwrap();
    let top_method_symbol = index.symbol_at(source, top_level_method).unwrap();
    let top_alias_symbol = index.symbol_at(source, top_level_alias).unwrap();
    assert_eq!(
        store.symbols.get(top_value_symbol).owner,
        Some(wrapper_class)
    );
    assert_eq!(
        store.symbols.get(top_method_symbol).owner,
        Some(wrapper_class)
    );
    assert_eq!(
        store.symbols.get(top_value_symbol).kind,
        dotty_core::SymbolKind::Field
    );
    assert_eq!(
        store.symbols.get(top_method_symbol).kind,
        dotty_core::SymbolKind::Method
    );
    assert_eq!(
        store.symbols.get(top_alias_symbol).kind,
        dotty_core::SymbolKind::TypeAlias
    );
    assert_eq!(
        store.symbols.get(top_alias_symbol).owner,
        Some(wrapper_class)
    );
    assert_eq!(
        store
            .scopes
            .get(wrapper_scope)
            .lookup(top_value_name.as_name()),
        Some(top_value_symbol)
    );
    assert_eq!(
        store.scopes.get(wrapper_scope).lookup(top_name.as_name()),
        Some(top_method_symbol)
    );
    assert_eq!(
        store
            .scopes
            .get(root_package.scope)
            .lookup(top_value_name.as_name()),
        None
    );
    assert_eq!(
        store
            .scopes
            .get(root_package.scope)
            .lookup(top_name.as_name()),
        None
    );
}

#[test]
fn parsed_secondary_constructor_parameters_belong_to_the_constructor_scope() {
    use dotty_core::TermName;
    use dotty_lexer::ContextualScanner;

    let source_text = "class C { def this(x: Int) = this() }";
    let source = SourceId::from_index(57);
    let mut store = SemanticStore::new();
    let scanner = ContextualScanner::new(source_text).expect("source should lex");
    let parsed = parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let class_tree = package.stats[0];
    let TreeKind::TypeDef(class_definition) = &parsed.ast.get(class_tree).kind else {
        panic!("package statement should be the class");
    };
    let TreeKind::Template(template) = &parsed.ast.get(class_definition.rhs).kind else {
        panic!("class RHS should be a Template");
    };
    let secondary_tree = template
        .body
        .iter()
        .copied()
        .find(|tree| matches!(parsed.ast.get(*tree).kind, TreeKind::DefDef(_)))
        .expect("parser should represent the secondary constructor as a DefDef");
    let TreeKind::DefDef(secondary) = &parsed.ast.get(secondary_tree).kind else {
        unreachable!("secondary constructor tree was found above");
    };
    assert_eq!(secondary.value_param_clauses.len(), 1);
    assert_eq!(secondary.value_param_clauses[0].len(), 1);
    let parameter_tree = secondary.value_param_clauses[0][0];
    let TreeKind::ValDef(_) = &parsed.ast.get(parameter_tree).kind else {
        panic!("secondary constructor parameter should be a ValDef");
    };
    assert_eq!(
        store.names.resolve(secondary.name.as_name().text()),
        "<init>"
    );
    let mut packages = Packages::new();

    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "SecondaryConstructor.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let root_package = packages.get::<&str>(&[]).unwrap();
    let class_name = *class_definition.name.as_name();
    let class_symbol = store
        .scopes
        .get(root_package.scope)
        .lookup(&class_name)
        .unwrap();
    let class_scope = index.scope_of(class_symbol).unwrap();
    let constructor_symbol = index.symbol_at(source, secondary_tree).unwrap();
    let constructor_scope = index.scope_of(constructor_symbol).unwrap();
    let parameter_symbol = index.symbol_at(source, parameter_tree).unwrap();
    let x_name = TermName::new(store.names.intern("x"));

    assert_eq!(
        store.symbols.get(constructor_symbol).kind,
        dotty_core::SymbolKind::Constructor
    );
    assert_eq!(
        store.symbols.get(parameter_symbol).kind,
        dotty_core::SymbolKind::Parameter
    );
    assert_eq!(
        store.symbols.get(parameter_symbol).owner,
        Some(constructor_symbol)
    );
    assert_eq!(
        store.scopes.get(constructor_scope).lookup(x_name.as_name()),
        Some(parameter_symbol)
    );
    assert_eq!(store.scopes.get(class_scope).lookup(x_name.as_name()), None);
}

fn assert_secondary_constructor_is_rejected(source_text: &str, source: SourceId) {
    use dotty_lexer::ContextualScanner;

    let mut store = SemanticStore::new();
    let scanner = ContextualScanner::new(source_text).expect("source should lex");
    let parsed = parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );

    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.kind() == ParseDiagnosticKind::UnexpectedToken),
        "expected an unexpected-token diagnostic, got {:?}",
        parsed.diagnostics
    );
}

#[test]
fn parsed_secondary_constructor_is_rejected_in_a_trait() {
    assert_secondary_constructor_is_rejected(
        "trait T { def this(x: Int) = this() }",
        SourceId::from_index(58),
    );
}

#[test]
fn parsed_secondary_constructor_is_rejected_in_an_object() {
    assert_secondary_constructor_is_rejected(
        "object O { def this(x: Int) = this() }",
        SourceId::from_index(59),
    );
}

#[test]
fn parsed_secondary_constructor_is_rejected_in_a_nested_object_inside_a_class() {
    assert_secondary_constructor_is_rejected(
        "class C { object O { def this(x: Int) = this() } }",
        SourceId::from_index(60),
    );
}

#[test]
fn parsed_secondary_constructor_is_rejected_in_a_direct_block_member() {
    assert_secondary_constructor_is_rejected(
        "class C { { def this() = this() } }",
        SourceId::from_index(61),
    );
}

#[test]
fn parsed_secondary_constructor_is_rejected_in_a_method_parameter_default() {
    assert_secondary_constructor_is_rejected(
        "class C { def f(x: Int = { def this() = this(); 1 }) = x }",
        SourceId::from_index(62),
    );
}

#[test]
fn parsed_nested_object_creates_module_class_and_links_its_companion() {
    use dotty_core::SymbolKind;
    use dotty_lexer::ContextualScanner;

    let source_text = "class Outer { class Foo; object Foo { def run = 1 } }";
    let source = SourceId::from_index(63);
    let mut store = SemanticStore::new();
    let scanner = ContextualScanner::new(source_text).expect("source should lex");
    let parsed = parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty());
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let outer_tree = package.stats[0];
    let TreeKind::TypeDef(outer_def) = &parsed.ast.get(outer_tree).kind else {
        panic!("package member should be a class");
    };
    let TreeKind::Template(outer_template) = &parsed.ast.get(outer_def.rhs).kind else {
        panic!("class RHS should be a Template");
    };
    let class_tree = outer_template.body[0];
    let object_tree = outer_template.body[1];
    let TreeKind::Template(module_template) = (match &parsed.ast.get(object_tree).kind {
        TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::ModuleDef(module)) => {
            &parsed.ast.get(module.template).kind
        }
        _ => panic!("nested object should be a ModuleDef"),
    }) else {
        panic!("object RHS should be a Template");
    };
    let method_tree = module_template.body[0];
    let mut packages = Packages::new();

    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "Companions.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let class_symbol = index.symbol_at(source, class_tree).unwrap();
    let object_symbol = index.symbol_at(source, object_tree).unwrap();
    let method_symbol = index.symbol_at(source, method_tree).unwrap();
    let outer_symbol = store.symbols.get(class_symbol).owner.unwrap();
    let outer_scope = index.scope_of(outer_symbol).unwrap();
    let module_class = store
        .scopes
        .get(outer_scope)
        .lookup(TypeName::new(store.names.intern("Foo$")).as_name())
        .unwrap();

    assert_eq!(store.symbols.get(object_symbol).kind, SymbolKind::Object);
    assert_eq!(
        store.symbols.get(module_class).kind,
        SymbolKind::ModuleClass
    );
    assert_eq!(
        store.symbols.get(class_symbol).links.companion,
        Some(object_symbol)
    );
    assert_eq!(
        store.symbols.get(object_symbol).links.companion,
        Some(class_symbol)
    );
    assert_eq!(store.symbols.get(method_symbol).owner, Some(module_class));
}

#[test]
fn parsed_pattern_val_binders_are_fields_of_the_source_wrapper() {
    use dotty_core::ast::UntypedNode;
    use dotty_core::{SymbolKind, TermName};
    use dotty_lexer::ContextualScanner;

    let source_text = "val (left, right) = pair";
    let source = SourceId::from_index(86);
    let mut store = SemanticStore::new();
    let scanner = ContextualScanner::new(source_text).expect("source should lex");
    let parsed = parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty());
    let TreeKind::PackageDef(package_def) = &parsed.ast.get(parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let patdef_tree = package_def.stats[0];
    let TreeKind::PhaseSpecific(UntypedNode::PatDef(patdef)) = &parsed.ast.get(patdef_tree).kind
    else {
        panic!("tuple value declaration should remain a PatDef");
    };
    let mut names = Vec::new();
    fn collect_idents(
        arena: &dotty_core::AstArena<dotty_core::Untyped>,
        tree: dotty_core::TreeId<dotty_core::Untyped>,
        names: &mut Vec<(dotty_core::TreeId<dotty_core::Untyped>, String)>,
        interner: &dotty_core::NameInterner,
    ) {
        match &arena.get(tree).kind {
            TreeKind::Ident(ident) => {
                let name = interner.resolve(ident.name.text());
                if name != "_" {
                    names.push((tree, name.to_owned()));
                }
            }
            TreeKind::Bind(binding) => {
                names.push((tree, interner.resolve(binding.name.text()).to_owned()));
                collect_idents(arena, binding.body, names, interner);
            }
            TreeKind::PhaseSpecific(UntypedNode::Tuple(tuple)) => {
                for element in &tuple.elements {
                    collect_idents(arena, *element, names, interner);
                }
            }
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => {
                collect_idents(arena, parens.inner, names, interner);
            }
            TreeKind::Typed(typed) => collect_idents(arena, typed.expr, names, interner),
            _ => {}
        }
    }
    for pattern in &patdef.patterns {
        collect_idents(&parsed.ast, *pattern, &mut names, &store.names);
    }

    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "Pattern.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let root_package = packages.get::<&str>(&[]).unwrap();
    let wrapper = store
        .scopes
        .get(root_package.scope)
        .lookup(TypeName::new(store.names.intern("Pattern$package$")).as_name())
        .unwrap();
    let wrapper_scope = index.scope_of(wrapper).unwrap();

    assert_eq!(names.len(), 2);
    for (tree, name) in names {
        let symbol = index.symbol_at(source, tree).unwrap();
        assert_eq!(store.symbols.get(symbol).kind, SymbolKind::Field);
        assert_eq!(store.symbols.get(symbol).owner, Some(wrapper));
        assert_eq!(
            store
                .scopes
                .get(wrapper_scope)
                .lookup(TermName::new(store.names.intern(&name)).as_name()),
            Some(symbol)
        );
    }
}

#[test]
fn parsed_class_pattern_val_binders_are_class_fields() {
    use dotty_core::SymbolKind;
    use dotty_core::ast::UntypedNode;

    let named = named_source("class C:\n  val (left, right) = pair", 212);
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let class_tree = package.stats[0];
    let TreeKind::TypeDef(class) = &named.parsed.ast.get(class_tree).kind else {
        panic!("class declaration should be a TypeDef");
    };
    let TreeKind::Template(template) = &named.parsed.ast.get(class.rhs).kind else {
        panic!("class RHS should be a Template");
    };
    let patdef_tree = template.body[0];
    let TreeKind::PhaseSpecific(UntypedNode::PatDef(patdef)) =
        &named.parsed.ast.get(patdef_tree).kind
    else {
        panic!("tuple member should remain a PatDef");
    };
    let class_symbol = named.index.symbol_at(named.source, class_tree).unwrap();
    let class_scope = named.index.scope_of(class_symbol).unwrap();
    let bindings = source_pattern_bindings(&named, &patdef.patterns);

    assert_eq!(bindings.len(), 2);
    for (tree, name) in bindings {
        let field = named.index.symbol_at(named.source, tree).unwrap();
        let field_symbol = named.store.symbols.get(field);
        assert_eq!(field_symbol.kind, SymbolKind::Field);
        assert_eq!(field_symbol.owner, Some(class_symbol));
        assert_eq!(named.store.names.resolve(field_symbol.name.text()), name);
        assert_eq!(
            field_symbol.origin,
            dotty_core::SymbolOrigin::Source(named.source)
        );
        assert_eq!(field_symbol.position, named.parsed.ast.get(tree).position);
        assert_eq!(
            named
                .store
                .scopes
                .get(class_scope)
                .lookup(&field_symbol.name),
            Some(field)
        );
    }
    assert_eq!(named.index.symbol_at(named.source, patdef_tree), None);
    assert_eq!(
        named.index.symbol_at(named.source, patdef.rhs.unwrap()),
        None,
        "naming must not traverse the PatDef RHS"
    );
}

#[test]
fn parsed_object_pattern_val_binders_are_module_class_fields() {
    use dotty_core::SymbolKind;
    use dotty_core::ast::UntypedNode;

    let named = named_source("object O:\n  val (left, right) = pair", 213);
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module)) =
        &named.parsed.ast.get(package.stats[0]).kind
    else {
        panic!("object should be a ModuleDef");
    };
    let TreeKind::Template(template) = &named.parsed.ast.get(module.template).kind else {
        panic!("object body should be a Template");
    };
    let TreeKind::PhaseSpecific(UntypedNode::PatDef(patdef)) =
        &named.parsed.ast.get(template.body[0]).kind
    else {
        panic!("tuple member should remain a PatDef");
    };
    let package_symbol = named
        .index
        .symbol_at(named.source, named.parsed.root)
        .expect("package should have a semantic symbol");
    let package_scope = named.index.scope_of(package_symbol).unwrap();
    let module_class_name = TypeName::new(named.store.names.get("O$").unwrap());
    let module_class = named
        .store
        .scopes
        .get(package_scope)
        .lookup(module_class_name.as_name())
        .expect("object should have a module class");
    let module_class_scope = named.index.scope_of(module_class).unwrap();
    let bindings = source_pattern_bindings(&named, &patdef.patterns);

    assert_eq!(bindings.len(), 2);
    for (tree, name) in bindings {
        let field = named.index.symbol_at(named.source, tree).unwrap();
        let owner = named.store.symbols.get(field).owner.unwrap();
        assert_eq!(named.store.symbols.get(field).kind, SymbolKind::Field);
        assert_eq!(
            named
                .store
                .names
                .resolve(named.store.symbols.get(field).name.text()),
            name
        );
        assert_eq!(named.store.symbols.get(owner).kind, SymbolKind::ModuleClass);
        assert_eq!(
            named
                .store
                .scopes
                .get(module_class_scope)
                .lookup(&named.store.symbols.get(field).name),
            Some(field)
        );
    }
}

#[test]
fn class_pattern_bindings_follow_nested_bind_typed_and_unapply_patterns() {
    let named = named_source("class C:\n  val Some(x @ Some(y: Int)) = value", 214);
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::TypeDef(class) = &named.parsed.ast.get(package.stats[0]).kind else {
        panic!("class declaration should be a TypeDef");
    };
    let TreeKind::Template(template) = &named.parsed.ast.get(class.rhs).kind else {
        panic!("class RHS should be a Template");
    };
    let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::PatDef(patdef)) =
        &named.parsed.ast.get(template.body[0]).kind
    else {
        panic!("nested pattern should remain a PatDef");
    };
    let bindings = source_pattern_bindings(&named, &patdef.patterns);
    let names = bindings
        .iter()
        .map(|(_, name)| name.as_str())
        .collect::<Vec<_>>();

    assert_eq!(names, ["x", "y"]);
    for (tree, _) in bindings {
        assert!(named.index.symbol_at(named.source, tree).is_some());
    }
}

#[test]
fn class_alternative_pattern_uses_only_its_first_alternative() {
    let named = named_source("class C:\n  val (Some(left) | Some(right)) = value", 215);
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::TypeDef(class) = &named.parsed.ast.get(package.stats[0]).kind else {
        panic!("class declaration should be a TypeDef");
    };
    let TreeKind::Template(template) = &named.parsed.ast.get(class.rhs).kind else {
        panic!("class RHS should be a Template");
    };
    let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::PatDef(patdef)) =
        &named.parsed.ast.get(template.body[0]).kind
    else {
        panic!("alternative member should remain a PatDef");
    };
    let bindings = source_pattern_bindings(&named, &patdef.patterns);

    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].1, "left");
    assert!(named.index.symbol_at(named.source, bindings[0].0).is_some());
}

#[test]
fn duplicate_textual_bindings_in_one_class_pattern_create_one_field() {
    let named = named_source("class C:\n  val (same, same) = value", 216);
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::TypeDef(class) = &named.parsed.ast.get(package.stats[0]).kind else {
        panic!("class declaration should be a TypeDef");
    };
    let TreeKind::Template(template) = &named.parsed.ast.get(class.rhs).kind else {
        panic!("class RHS should be a Template");
    };
    let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::PatDef(patdef)) =
        &named.parsed.ast.get(template.body[0]).kind
    else {
        panic!("tuple member should remain a PatDef");
    };
    let bindings = source_pattern_bindings(&named, &patdef.patterns);

    assert_eq!(bindings.len(), 2);
    let fields = bindings
        .iter()
        .filter_map(|(tree, _)| named.index.symbol_at(named.source, *tree))
        .collect::<Vec<_>>();
    assert_eq!(fields.len(), 1);
}

#[test]
fn wildcard_only_class_pattern_introduces_no_field() {
    let named = named_source("class C:\n  val Some(_) = value", 217);
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::TypeDef(class) = &named.parsed.ast.get(package.stats[0]).kind else {
        panic!("class declaration should be a TypeDef");
    };
    let TreeKind::Template(template) = &named.parsed.ast.get(class.rhs).kind else {
        panic!("class RHS should be a Template");
    };
    let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::PatDef(patdef)) =
        &named.parsed.ast.get(template.body[0]).kind
    else {
        panic!("wildcard member should remain a PatDef");
    };

    assert!(source_pattern_bindings(&named, &patdef.patterns).is_empty());
    let class_symbol = named
        .index
        .symbol_at(named.source, package.stats[0])
        .unwrap();
    let class_scope = named.index.scope_of(class_symbol).unwrap();
    assert!(
        named
            .store
            .scopes
            .get(class_scope)
            .lookup_all(dotty_core::TermName::new(named.store.names.get("_").unwrap()).as_name())
            .is_empty()
    );
}

#[test]
fn wildcard_only_pattern_does_not_resolve_unused_visibility_qualifier() {
    let named = named_source("class C:\n  private[Missing] val Some(_) = value", 223);
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::TypeDef(class) = &named.parsed.ast.get(package.stats[0]).kind else {
        panic!("class declaration should be a TypeDef");
    };
    let TreeKind::Template(template) = &named.parsed.ast.get(class.rhs).kind else {
        panic!("class RHS should be a Template");
    };
    let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::PatDef(patdef)) =
        &named.parsed.ast.get(template.body[0]).kind
    else {
        panic!("wildcard member should remain a PatDef");
    };

    assert!(source_pattern_bindings(&named, &patdef.patterns).is_empty());
}

#[test]
fn package_imports_form_ordered_contexts_for_following_wrapped_declarations() {
    let named = named_source(
        "val first = 1\nval same = 2\nimport alpha.*\nval after_alpha = 3\nimport beta.*\nval after_beta = 4",
        224,
    );
    let stats = package_stat_trees(&named);
    let (first_symbol, first_context) = declaration_context(&named, stats[0]);
    let (_, same_context) = declaration_context(&named, stats[1]);
    let (after_alpha_symbol, after_alpha_context) = declaration_context(&named, stats[3]);
    let (after_beta_symbol, after_beta_context) = declaration_context(&named, stats[5]);
    let alpha = named.index.source_context(after_alpha_context);
    let beta = named.index.source_context(after_beta_context);

    assert_eq!(first_context, same_context);
    assert_eq!(
        alpha.owner,
        named
            .index
            .symbol_at(named.source, named.parsed.root)
            .unwrap()
    );
    assert_eq!(alpha.parent, Some(first_context));
    assert_eq!(alpha.import, Some(stats[2]));
    assert_eq!(beta.parent, Some(after_alpha_context));
    assert_eq!(beta.import, Some(stats[4]));
    assert_ne!(
        named.store.symbols.get(first_symbol).owner,
        Some(alpha.owner),
        "wrapper symbols retain their semantic owner while contexts keep package ownership"
    );
    assert_eq!(
        named.store.symbols.get(after_alpha_symbol).owner,
        named.store.symbols.get(after_beta_symbol).owner
    );

    let package_symbol = named
        .index
        .symbol_at(named.source, named.parsed.root)
        .unwrap();
    let package_scope = named.index.scope_of(package_symbol).unwrap();
    for import_name in ["alpha", "beta"] {
        assert!(
            named
                .store
                .scopes
                .get(package_scope)
                .lookup_all(
                    dotty_core::TypeName::new(named.store.names.get(import_name).unwrap())
                        .as_name()
                )
                .is_empty()
        );
    }
}

#[test]
fn package_import_context_is_captured_by_following_direct_class_and_object() {
    use dotty_core::ast::UntypedNode;

    let named = named_source("import alpha.*\nclass C\nobject O", 225);
    let stats = package_stat_trees(&named);
    let (_, class_context) = declaration_context(&named, stats[1]);
    let (_, object_context) = declaration_context(&named, stats[2]);
    let class_env = named.index.source_context(class_context);
    let object_env = named.index.source_context(object_context);
    let TreeKind::Import(_) = &named.parsed.ast.get(stats[0]).kind else {
        panic!("first package statement should be an import");
    };
    let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_)) = &named.parsed.ast.get(stats[2]).kind
    else {
        panic!("object declaration should be a ModuleDef");
    };

    assert_eq!(class_context, object_context);
    assert_eq!(class_env.import, Some(stats[0]));
    assert_eq!(object_env.import, Some(stats[0]));
}

#[test]
fn class_imports_affect_following_members_and_nested_contexts_inherit_them() {
    let named = named_source(
        "class C:\n  val before = 1\n  import alpha.*\n  val after_alpha = 2\n  import beta.*\n  val after_beta = 3\n  class Nested:\n    val inside = 4",
        226,
    );
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let class_tree = package.stats[0];
    let TreeKind::TypeDef(class) = &named.parsed.ast.get(class_tree).kind else {
        panic!("class declaration should be a TypeDef");
    };
    let TreeKind::Template(template) = &named.parsed.ast.get(class.rhs).kind else {
        panic!("class RHS should be a Template");
    };
    let (_, before_context) = declaration_context(&named, template.body[0]);
    let (_, after_alpha_context) = declaration_context(&named, template.body[2]);
    let (_, after_beta_context) = declaration_context(&named, template.body[4]);
    let (_, nested_context) = declaration_context(&named, template.body[5]);
    let class_symbol = named.index.symbol_at(named.source, class_tree).unwrap();
    let before = named.index.source_context(before_context);
    let after_alpha = named.index.source_context(after_alpha_context);
    let after_beta = named.index.source_context(after_beta_context);
    let nested = named.index.source_context(nested_context);
    let TreeKind::Import(_) = &named.parsed.ast.get(template.body[1]).kind else {
        panic!("template statement should be an import");
    };
    let TreeKind::Import(_) = &named.parsed.ast.get(template.body[3]).kind else {
        panic!("template statement should be an import");
    };
    let TreeKind::TypeDef(nested_def) = &named.parsed.ast.get(template.body[5]).kind else {
        panic!("nested declaration should be a TypeDef");
    };
    let TreeKind::Template(nested_template) = &named.parsed.ast.get(nested_def.rhs).kind else {
        panic!("nested class RHS should be a Template");
    };
    let (_, inside_context) = declaration_context(&named, nested_template.body[0]);
    let inside = named.index.source_context(inside_context);

    assert_ne!(before_context, after_alpha_context);
    assert_eq!(before.owner, class_symbol);
    assert_eq!(after_alpha.parent, Some(before_context));
    assert_eq!(after_alpha.import, Some(template.body[1]));
    assert_eq!(after_beta.parent, Some(after_alpha_context));
    assert_eq!(after_beta.import, Some(template.body[3]));
    assert_eq!(nested_context, after_beta_context);
    assert_eq!(nested.owner, class_symbol);
    assert_eq!(
        inside.owner,
        named
            .index
            .symbol_at(named.source, template.body[5])
            .unwrap()
    );
    assert_eq!(inside.parent, Some(nested_context));
    assert_eq!(inside.import, None);

    let class_scope = named.index.scope_of(class_symbol).unwrap();
    assert!(
        named
            .store
            .scopes
            .get(class_scope)
            .lookup_all(
                dotty_core::TermName::new(named.store.names.get("alpha").unwrap()).as_name()
            )
            .is_empty()
    );
    assert!(
        named
            .store
            .scopes
            .get(class_scope)
            .lookup_all(dotty_core::TermName::new(named.store.names.get("beta").unwrap()).as_name())
            .is_empty()
    );
}

#[test]
fn module_class_imports_follow_source_order_without_resolving_imports() {
    use dotty_core::ast::UntypedNode;

    let named = named_source(
        "object O:\n  import alpha.*\n  val before = 1\n  import beta.*\n  val after = 2",
        231,
    );
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module)) =
        &named.parsed.ast.get(package.stats[0]).kind
    else {
        panic!("object should be a ModuleDef");
    };
    let TreeKind::Template(template) = &named.parsed.ast.get(module.template).kind else {
        panic!("object body should be a Template");
    };
    let (_, before_context) = declaration_context(&named, template.body[1]);
    let (_, after_context) = declaration_context(&named, template.body[3]);
    let before = named.index.source_context(before_context);
    let after = named.index.source_context(after_context);
    let package_symbol = named
        .index
        .symbol_at(named.source, named.parsed.root)
        .unwrap();
    let package_scope = named.index.scope_of(package_symbol).unwrap();
    let module_class = named
        .store
        .scopes
        .get(package_scope)
        .lookup(dotty_core::TypeName::new(named.store.names.get("O$").unwrap()).as_name())
        .unwrap();
    let module_scope = named.index.scope_of(module_class).unwrap();

    assert_eq!(before.owner, module_class);
    assert_eq!(before.lexical_scope, module_scope);
    assert_eq!(after.parent, Some(before_context));
    assert_eq!(after.import, Some(template.body[2]));
    for import_name in ["alpha", "beta"] {
        assert!(
            named
                .store
                .scopes
                .get(module_scope)
                .lookup_all(
                    dotty_core::TermName::new(named.store.names.get(import_name).unwrap())
                        .as_name()
                )
                .is_empty()
        );
    }
}

#[test]
fn method_parameters_use_a_method_owned_child_context() {
    let named = named_source("class C:\n  def method[A](value: Int): Int = value", 227);
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::TypeDef(class) = &named.parsed.ast.get(package.stats[0]).kind else {
        panic!("class declaration should be a TypeDef");
    };
    let TreeKind::Template(template) = &named.parsed.ast.get(class.rhs).kind else {
        panic!("class RHS should be a Template");
    };
    let method_tree = template.body[0];
    let TreeKind::DefDef(method) = &named.parsed.ast.get(method_tree).kind else {
        panic!("method declaration should be a DefDef");
    };
    let (method_symbol, method_context) = declaration_context(&named, method_tree);
    let (type_parameter_symbol, type_parameter_context) =
        declaration_context(&named, method.type_params[0]);
    let (value_parameter_symbol, value_parameter_context) =
        declaration_context(&named, method.value_param_clauses[0][0]);
    let lexical_scope = named.index.scope_of(method_symbol).unwrap();
    let parameter_env = named.index.source_context(value_parameter_context);

    assert_eq!(type_parameter_context, value_parameter_context);
    assert_eq!(parameter_env.owner, method_symbol);
    assert_eq!(parameter_env.lexical_scope, lexical_scope);
    assert_eq!(parameter_env.parent, Some(method_context));
    assert_eq!(
        named.index.declaration_context_of(type_parameter_symbol),
        Some(parameter_env_id(&named, value_parameter_symbol))
    );
}

fn parameter_env_id(named: &NamedSource, symbol: dotty_core::SymbolId) -> SourceContextId {
    named.index.declaration_context_of(symbol).unwrap()
}

#[test]
fn constructor_parameters_use_the_constructor_lexical_child_context() {
    let named = named_source("class C[A](val field: Int, local: String)", 228);
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let class_tree = package.stats[0];
    let TreeKind::TypeDef(class) = &named.parsed.ast.get(class_tree).kind else {
        panic!("class declaration should be a TypeDef");
    };
    let TreeKind::Template(template) = &named.parsed.ast.get(class.rhs).kind else {
        panic!("class RHS should be a Template");
    };
    let TreeKind::DefDef(constructor) = &named.parsed.ast.get(template.constructor).kind else {
        panic!("primary constructor should be a DefDef");
    };
    let constructor_symbol = named
        .index
        .symbol_at(named.source, template.constructor)
        .unwrap();
    let (_, constructor_context) = declaration_context(&named, template.constructor);
    let class_type_parameter = constructor.type_params[0];
    let class_type_parameter_symbol = named
        .index
        .symbol_at(named.source, class_type_parameter)
        .unwrap();
    let derived_type_parameter = named
        .index
        .derived_symbol_at(constructor_symbol, named.source, class_type_parameter)
        .unwrap();
    let field_parameter = constructor.value_param_clauses[0][0];
    let canonical_field_parameter = named
        .index
        .symbol_at(named.source, field_parameter)
        .unwrap();
    let derived_field_parameter = named
        .index
        .derived_symbol_at(constructor_symbol, named.source, field_parameter)
        .unwrap();
    let local_parameter = constructor.value_param_clauses[0][1];
    let constructor_env_id = named
        .index
        .declaration_context_of(derived_type_parameter)
        .unwrap();
    let constructor_env = named.index.source_context(constructor_env_id);

    assert_eq!(
        named
            .index
            .declaration_context_of(class_type_parameter_symbol),
        Some(constructor_context)
    );
    assert_eq!(constructor_env.owner, constructor_symbol);
    assert_eq!(constructor_env.parent, Some(constructor_context));
    assert_eq!(
        named.index.declaration_context_of(derived_field_parameter),
        Some(constructor_env_id)
    );
    assert_eq!(
        named
            .index
            .declaration_context_of(canonical_field_parameter),
        Some(constructor_context)
    );
    assert_eq!(
        named.index.declaration_context_of(
            named
                .index
                .symbol_at(named.source, local_parameter)
                .unwrap()
        ),
        Some(constructor_env_id)
    );
}

#[test]
fn extension_prefix_parameters_use_the_target_methods_lexical_context() {
    use dotty_core::ast::UntypedNode;

    let named = named_source("extension (receiver: Int)\n  def doubled = receiver", 229);
    let stats = package_stat_trees(&named);
    let TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) =
        &named.parsed.ast.get(stats[0]).kind
    else {
        panic!("source stat should be ExtensionMethods");
    };
    let method_tree = extension.methods[0];
    let (method_symbol, method_context) = declaration_context(&named, method_tree);
    let receiver_tree = extension.param_clauses[0][0];
    let receiver_symbol = named
        .index
        .derived_symbol_at(method_symbol, named.source, receiver_tree)
        .unwrap();
    let receiver_context_id = parameter_env_id(&named, receiver_symbol);
    let receiver_context = named.index.source_context(receiver_context_id);

    assert_eq!(receiver_context.owner, method_symbol);
    assert_eq!(receiver_context.parent, Some(method_context));
    assert_eq!(
        receiver_context.lexical_scope,
        named.index.scope_of(method_symbol).unwrap()
    );
}

#[test]
fn pattern_binding_symbols_share_the_patdef_declaration_context() {
    let named = named_source(
        "class C:\n  import alpha.*\n  val (left, right) = pair",
        230,
    );
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::TypeDef(class) = &named.parsed.ast.get(package.stats[0]).kind else {
        panic!("class declaration should be a TypeDef");
    };
    let TreeKind::Template(template) = &named.parsed.ast.get(class.rhs).kind else {
        panic!("class RHS should be a Template");
    };
    let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::PatDef(patdef)) =
        &named.parsed.ast.get(template.body[1]).kind
    else {
        panic!("tuple member should remain a PatDef");
    };
    let context_ids = source_pattern_bindings(&named, &patdef.patterns)
        .into_iter()
        .map(|(tree, _)| declaration_context(&named, tree).1)
        .collect::<Vec<_>>();

    assert_eq!(context_ids.len(), 2);
    assert_eq!(context_ids[0], context_ids[1]);
    assert_eq!(
        named.index.source_context(context_ids[0]).import,
        Some(template.body[0])
    );
}

#[test]
fn var_pattern_bindings_inherit_mutability() {
    let named = named_source("class C:\n  var (left, right) = pair", 218);
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::TypeDef(class) = &named.parsed.ast.get(package.stats[0]).kind else {
        panic!("class declaration should be a TypeDef");
    };
    let TreeKind::Template(template) = &named.parsed.ast.get(class.rhs).kind else {
        panic!("class RHS should be a Template");
    };
    let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::PatDef(patdef)) =
        &named.parsed.ast.get(template.body[0]).kind
    else {
        panic!("tuple member should remain a PatDef");
    };

    for (tree, _) in source_pattern_bindings(&named, &patdef.patterns) {
        let field = named.index.symbol_at(named.source, tree).unwrap();
        assert!(
            named
                .store
                .symbols
                .get(field)
                .flags
                .contains(SymbolFlags::MUTABLE)
        );
    }
}

#[test]
fn lazy_pattern_bindings_inherit_lazy_flag() {
    let named = named_source("class C:\n  lazy val (left, right) = compute()", 219);
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::TypeDef(class) = &named.parsed.ast.get(package.stats[0]).kind else {
        panic!("class declaration should be a TypeDef");
    };
    let TreeKind::Template(template) = &named.parsed.ast.get(class.rhs).kind else {
        panic!("class RHS should be a Template");
    };
    let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::PatDef(patdef)) =
        &named.parsed.ast.get(template.body[0]).kind
    else {
        panic!("tuple member should remain a PatDef");
    };

    for (tree, _) in source_pattern_bindings(&named, &patdef.patterns) {
        let field = named.index.symbol_at(named.source, tree).unwrap();
        assert!(
            named
                .store
                .symbols
                .get(field)
                .flags
                .contains(SymbolFlags::LAZY)
        );
    }
}

#[test]
fn private_pattern_bindings_inherit_private_visibility() {
    let named = named_source("class C:\n  private val (left, right) = pair", 220);
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::TypeDef(class) = &named.parsed.ast.get(package.stats[0]).kind else {
        panic!("class declaration should be a TypeDef");
    };
    let TreeKind::Template(template) = &named.parsed.ast.get(class.rhs).kind else {
        panic!("class RHS should be a Template");
    };
    let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::PatDef(patdef)) =
        &named.parsed.ast.get(template.body[0]).kind
    else {
        panic!("tuple member should remain a PatDef");
    };

    for (tree, _) in source_pattern_bindings(&named, &patdef.patterns) {
        let field = named.index.symbol_at(named.source, tree).unwrap();
        assert_eq!(
            named.store.symbols.get(field).visibility,
            Visibility::Private
        );
    }
}

#[test]
fn pattern_bindings_and_sibling_headers_precede_nested_class_body() {
    let named = named_source(
        "class C:\n  val (left, right) = pair\n  class Nested:\n    val child = 1\n  def sibling = 1",
        221,
    );
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::TypeDef(class) = &named.parsed.ast.get(package.stats[0]).kind else {
        panic!("class declaration should be a TypeDef");
    };
    let TreeKind::Template(template) = &named.parsed.ast.get(class.rhs).kind else {
        panic!("class RHS should be a Template");
    };
    let pattern = match &named.parsed.ast.get(template.body[0]).kind {
        TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::PatDef(patdef)) => {
            source_pattern_bindings(&named, &patdef.patterns)
                .into_iter()
                .map(|(tree, _)| named.index.symbol_at(named.source, tree).unwrap())
                .collect::<Vec<_>>()
        }
        _ => panic!("tuple member should remain a PatDef"),
    };
    let nested = named
        .index
        .symbol_at(named.source, template.body[1])
        .unwrap();
    let sibling = named
        .index
        .symbol_at(named.source, template.body[2])
        .unwrap();
    let TreeKind::TypeDef(nested_def) = &named.parsed.ast.get(template.body[1]).kind else {
        panic!("nested declaration should be a TypeDef");
    };
    let TreeKind::Template(nested_template) = &named.parsed.ast.get(nested_def.rhs).kind else {
        panic!("nested class RHS should be a Template");
    };
    let child = named
        .index
        .symbol_at(named.source, nested_template.body[0])
        .unwrap();

    assert_eq!(pattern.len(), 2);
    assert!(pattern.iter().all(|field| field.index() < nested.index()));
    assert!(nested.index() < sibling.index());
    assert!(sibling.index() < child.index());
}

#[test]
fn enum_case_pattern_definition_is_deferred_instead_of_named_as_fields() {
    use dotty_core::ast::{Modifier, UntypedNode};

    let (mut parsed, source, mut store) =
        parsed_source("class C:\n  val (left, right) = pair", 222);
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::TypeDef(class) = &parsed.ast.get(package.stats[0]).kind else {
        panic!("class declaration should be a TypeDef");
    };
    let TreeKind::Template(template) = &parsed.ast.get(class.rhs).kind else {
        panic!("class RHS should be a Template");
    };
    let patdef_tree = template.body[0];
    let TreeKind::PhaseSpecific(UntypedNode::PatDef(patdef)) =
        &mut parsed.ast.get_mut(patdef_tree).kind
    else {
        panic!("tuple member should remain a PatDef");
    };
    patdef.modifiers.modifiers.push(Modifier::EnumCase);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "EnumCase.scala",
        &mut store,
        &mut packages,
    )
    .expect("enum-case PatDef should be deferred without an error");
    let named = NamedSource {
        parsed,
        source,
        store,
        index,
    };
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let class_symbol = named.index.symbol_at(source, package.stats[0]).unwrap();
    let class_scope = named.index.scope_of(class_symbol).unwrap();
    let TreeKind::PhaseSpecific(UntypedNode::PatDef(patdef)) =
        &named.parsed.ast.get(patdef_tree).kind
    else {
        panic!("tuple member should remain a PatDef");
    };
    let bindings = source_pattern_bindings(&named, &patdef.patterns);

    assert!(
        bindings
            .iter()
            .all(|(tree, _)| named.index.symbol_at(source, *tree).is_none())
    );
    for name in ["left", "right"] {
        assert!(
            named
                .store
                .scopes
                .get(class_scope)
                .lookup_all(
                    dotty_core::TermName::new(named.store.names.get(name).unwrap()).as_name()
                )
                .is_empty()
        );
    }
}

#[test]
fn source_wrappers_keep_distinct_file_stems_in_a_shared_package() {
    use dotty_core::TermName;
    use dotty_lexer::ContextualScanner;

    let mut store = SemanticStore::new();
    let mut packages = Packages::new();
    let mut named = Vec::new();
    for (source_id, file, method_name) in
        [(87, "Foo.scala", "fromFoo"), (88, "Bar.scala", "fromBar")]
    {
        let source_text = format!("def {method_name} = 1");
        let source = SourceId::from_index(source_id);
        let scanner = ContextualScanner::new(&source_text).expect("source should lex");
        let parsed = parse_compilation_unit(
            SourceText::new(&source_text).unwrap(),
            source,
            scanner,
            &mut store.names,
        );
        assert!(parsed.diagnostics.is_empty());
        let TreeKind::PackageDef(package_def) = &parsed.ast.get(parsed.root).kind else {
            panic!("parser should return a package root");
        };
        let method = package_def.stats[0];
        let index = name_compilation_unit(
            &parsed.ast,
            parsed.root,
            source,
            file,
            &mut store,
            &mut packages,
        )
        .unwrap();
        named.push((source, method, method_name, index));
    }

    let root_package = packages.get::<&str>(&[]).unwrap();
    let mut owner_symbols = Vec::new();
    for (source, method, method_name, index) in named {
        let wrapper = store
            .scopes
            .get(root_package.scope)
            .lookup(
                TypeName::new(store.names.intern(&format!(
                    "{}$package$",
                    if source.index() == 87 { "Foo" } else { "Bar" }
                )))
                .as_name(),
            )
            .unwrap();
        let symbol = index.symbol_at(source, method).unwrap();
        assert_eq!(store.symbols.get(symbol).owner, Some(wrapper));
        assert_eq!(
            store
                .scopes
                .get(index.scope_of(wrapper).unwrap())
                .lookup(TermName::new(store.names.intern(method_name)).as_name()),
            Some(symbol)
        );
        owner_symbols.push(wrapper);
    }
    assert_ne!(owner_symbols[0], owner_symbols[1]);
}

#[test]
fn parsed_private_top_level_value_uses_package_visibility() {
    use dotty_core::TermName;
    use dotty_lexer::ContextualScanner;

    let source_text = "private val hidden = 1";
    let source = SourceId::from_index(89);
    let mut store = SemanticStore::new();
    let scanner = ContextualScanner::new(source_text).expect("source should lex");
    let parsed = parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty());
    let mut packages = Packages::new();

    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "PrivateValue.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let package = packages.get::<&str>(&[]).unwrap();
    let wrapper = store
        .scopes
        .get(package.scope)
        .lookup(TypeName::new(store.names.intern("PrivateValue$package$")).as_name())
        .unwrap();
    let wrapper_scope = index.scope_of(wrapper).unwrap();
    let value = store
        .scopes
        .get(wrapper_scope)
        .lookup(TermName::new(store.names.intern("hidden")).as_name())
        .unwrap();

    assert_eq!(
        store.symbols.get(value).visibility,
        dotty_core::Visibility::PrivateWithin(package.symbol)
    );
}

#[test]
fn parsed_private_package_qualified_top_level_method_uses_package_boundary() {
    use dotty_core::TermName;
    use dotty_lexer::ContextualScanner;

    let source_text = "package p\nprivate[p] def hidden = 1";
    let source = SourceId::from_index(104);
    let mut store = SemanticStore::new();
    let scanner = ContextualScanner::new(source_text).expect("source should lex");
    let parsed = parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();

    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "QualifiedPrivate.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let package = packages.get(&["p"]).unwrap();
    let wrapper = store
        .scopes
        .get(package.scope)
        .lookup(TypeName::new(store.names.intern("QualifiedPrivate$package$")).as_name())
        .unwrap();
    let wrapper_scope = index.scope_of(wrapper).unwrap();
    let method = store
        .scopes
        .get(wrapper_scope)
        .lookup(TermName::new(store.names.intern("hidden")).as_name())
        .unwrap();

    assert_eq!(
        store.symbols.get(method).visibility,
        Visibility::PrivateWithin(package.symbol)
    );
}

#[test]
fn parsed_top_level_extension_methods_are_entered_in_the_source_wrapper() {
    use dotty_core::SymbolKind;
    use dotty_lexer::ContextualScanner;

    let source_text = "extension (value: Int) { def twice = value * 2 }";
    let source = SourceId::from_index(90);
    let mut store = SemanticStore::new();
    let scanner = ContextualScanner::new(source_text).expect("source should lex");
    let parsed = parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty());
    let TreeKind::PackageDef(package_def) = &parsed.ast.get(parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::ExtensionMethods(extension)) =
        &parsed.ast.get(package_def.stats[0]).kind
    else {
        panic!("extension should remain an ExtensionMethods node");
    };
    let method = extension.methods[0];
    let mut packages = Packages::new();

    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "Extension.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let package = packages.get::<&str>(&[]).unwrap();
    let wrapper = store
        .scopes
        .get(package.scope)
        .lookup(TypeName::new(store.names.intern("Extension$package$")).as_name())
        .unwrap();
    let method_symbol = index.symbol_at(source, method).unwrap();

    assert_eq!(store.symbols.get(method_symbol).kind, SymbolKind::Method);
    assert_eq!(store.symbols.get(method_symbol).owner, Some(wrapper));
}

#[test]
fn extension_prefix_parameters_are_derived_for_each_top_level_method() {
    use dotty_core::{SymbolKind, ast::UntypedNode};

    let named = named_source(
        "extension [A](x: A)\n  def id[B](value: B) = value\n  def discard = x",
        201,
    );
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) =
        &named.parsed.ast.get(package.stats[0]).kind
    else {
        panic!("extension should remain an ExtensionMethods node");
    };
    let prefix = extension
        .param_clauses
        .iter()
        .flat_map(|clause| clause.iter().copied())
        .collect::<Vec<_>>();

    assert_eq!(extension.methods.len(), 2);
    for method_tree in &extension.methods {
        let method = named
            .index
            .symbol_at(named.source, *method_tree)
            .expect("extension method keeps its canonical source identity");
        let method_symbol = named.store.symbols.get(method);
        assert!(method_symbol.flags.contains(SymbolFlags::EXTENSION));

        for (parameter_tree, expected_kind) in [
            (prefix[0], SymbolKind::TypeParameter),
            (prefix[1], SymbolKind::Parameter),
        ] {
            assert_eq!(named.index.symbol_at(named.source, parameter_tree), None);
            let derived = named
                .index
                .derived_symbol_at(method, named.source, parameter_tree)
                .expect("each method gets a derived prefix identity");
            let derived_symbol = named.store.symbols.get(derived);
            assert_eq!(derived_symbol.kind, expected_kind);
            assert_eq!(derived_symbol.owner, Some(method));
            assert_eq!(
                derived_symbol.position,
                named.parsed.ast.get(parameter_tree).position
            );
            assert_eq!(
                derived_symbol.origin,
                dotty_core::SymbolOrigin::Source(named.source)
            );
            let method_scope = named.index.scope_of(method).unwrap();
            assert_eq!(
                named
                    .store
                    .scopes
                    .get(method_scope)
                    .lookup(&derived_symbol.name),
                Some(derived)
            );
        }
    }

    let first = named
        .index
        .symbol_at(named.source, extension.methods[0])
        .unwrap();
    let second = named
        .index
        .symbol_at(named.source, extension.methods[1])
        .unwrap();
    assert_ne!(
        named
            .index
            .derived_symbol_at(first, named.source, prefix[0]),
        named
            .index
            .derived_symbol_at(second, named.source, prefix[0])
    );
    assert_eq!(
        named.index.extension_prefix_clauses(first),
        Some(extension.param_clauses.as_slice())
    );
    assert_eq!(
        named.index.extension_prefix_clauses(second),
        Some(extension.param_clauses.as_slice())
    );

    let TreeKind::DefDef(definition) = &named.parsed.ast.get(extension.methods[0]).kind else {
        panic!("extension child should be a DefDef");
    };
    let own_type_parameter = named
        .index
        .symbol_at(named.source, definition.type_params[0])
        .expect("method type parameter keeps its canonical identity");
    let own_value_parameter = named
        .index
        .symbol_at(named.source, definition.value_param_clauses[0][0])
        .expect("method value parameter keeps its canonical identity");
    assert_eq!(
        named.store.symbols.get(own_type_parameter).owner,
        Some(first)
    );
    assert_eq!(
        named.store.symbols.get(own_value_parameter).owner,
        Some(first)
    );
    assert!(
        !definition
            .metadata
            .modifiers
            .contains(&dotty_core::ast::Modifier::Extension)
    );
    let first_prefix = named
        .index
        .derived_symbol_at(first, named.source, prefix[0])
        .unwrap();
    let second_prefix = named
        .index
        .derived_symbol_at(first, named.source, prefix[1])
        .unwrap();
    assert!(first_prefix.index() < second_prefix.index());
    assert!(second_prefix.index() < own_type_parameter.index());
    assert!(own_type_parameter.index() < own_value_parameter.index());
}

#[test]
fn extension_methods_inside_class_templates_are_named_with_prefixes() {
    use dotty_core::ast::UntypedNode;

    let named = named_source(
        "class C:\n  extension (value: Int)\n    def twice = value",
        202,
    );
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::TypeDef(class) = &named.parsed.ast.get(package.stats[0]).kind else {
        panic!("class declaration should be a TypeDef");
    };
    let TreeKind::Template(template) = &named.parsed.ast.get(class.rhs).kind else {
        panic!("class RHS should be a Template");
    };
    let TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) =
        &named.parsed.ast.get(template.body[0]).kind
    else {
        panic!("class extension should remain an ExtensionMethods node");
    };
    let method_tree = extension.methods[0];
    let method = named
        .index
        .symbol_at(named.source, method_tree)
        .expect("class extension method has a canonical symbol");
    let prefix_tree = extension.param_clauses[0][0];
    let parameter = named
        .index
        .derived_symbol_at(method, named.source, prefix_tree)
        .expect("class extension receiver has a method-owned identity");

    assert!(
        named
            .store
            .symbols
            .get(method)
            .flags
            .contains(SymbolFlags::EXTENSION)
    );
    assert_eq!(named.store.symbols.get(parameter).owner, Some(method));
}

#[test]
fn extension_methods_inside_module_class_templates_are_named_with_prefixes() {
    use dotty_core::SymbolKind;
    use dotty_core::ast::UntypedNode;

    let named = named_source(
        "object O:\n  extension (value: Int)\n    def twice = value",
        210,
    );
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module)) =
        &named.parsed.ast.get(package.stats[0]).kind
    else {
        panic!("object declaration should be a ModuleDef");
    };
    let TreeKind::Template(template) = &named.parsed.ast.get(module.template).kind else {
        panic!("object body should be a Template");
    };
    let TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) =
        &named.parsed.ast.get(template.body[0]).kind
    else {
        panic!("object extension should remain an ExtensionMethods node");
    };
    let method = named
        .index
        .symbol_at(named.source, extension.methods[0])
        .unwrap();
    let receiver = named
        .index
        .derived_symbol_at(method, named.source, extension.param_clauses[0][0])
        .unwrap();
    let module_class = named.store.symbols.get(method).owner.unwrap();

    assert_eq!(
        named.store.symbols.get(module_class).kind,
        SymbolKind::ModuleClass
    );
    assert_eq!(named.store.symbols.get(receiver).owner, Some(method));
}

#[test]
fn extension_method_source_modifiers_and_visibility_are_preserved() {
    use dotty_core::ast::UntypedNode;

    let named = named_source("extension (value: Int)\n  private def hidden = value", 211);
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) =
        &named.parsed.ast.get(package.stats[0]).kind
    else {
        panic!("extension should remain an ExtensionMethods node");
    };
    let method = named
        .index
        .symbol_at(named.source, extension.methods[0])
        .unwrap();
    let method_symbol = named.store.symbols.get(method);

    assert!(method_symbol.flags.contains(SymbolFlags::EXTENSION));
    assert!(matches!(
        method_symbol.visibility,
        Visibility::PrivateWithin(_)
    ));
}

#[test]
fn extension_using_prefix_clauses_keep_given_flags_and_source_structure() {
    use dotty_core::ast::UntypedNode;

    let named = named_source(
        "extension [A](using before: Ctx[A])(value: A)(using after: End)\n  def use = value",
        203,
    );
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) =
        &named.parsed.ast.get(package.stats[0]).kind
    else {
        panic!("extension should remain an ExtensionMethods node");
    };
    let method = named
        .index
        .symbol_at(named.source, extension.methods[0])
        .unwrap();

    assert_eq!(extension.param_clauses.len(), 4);
    assert_eq!(
        named.index.extension_prefix_clauses(method),
        Some(extension.param_clauses.as_slice())
    );
    for clause_index in [1, 3] {
        let parameter_tree = extension.param_clauses[clause_index][0];
        let parameter = named
            .index
            .derived_symbol_at(method, named.source, parameter_tree)
            .unwrap();
        assert!(
            named
                .store
                .symbols
                .get(parameter)
                .flags
                .contains(SymbolFlags::GIVEN)
        );
    }
}

#[test]
fn erased_extension_prefix_parameter_keeps_the_erased_flag() {
    use dotty_core::ast::UntypedNode;

    let source_text = "extension (value: Int)\n  def use = value";
    let source = SourceId::from_index(204);
    let mut store = SemanticStore::new();
    let scanner = dotty_lexer::ContextualScanner::new(source_text).expect("source should lex");
    let mut parsed = parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) =
        &parsed.ast.get(package.stats[0]).kind
    else {
        panic!("extension should remain an ExtensionMethods node");
    };
    let prefix_tree = extension.param_clauses[0][0];
    let method_tree = extension.methods[0];
    let TreeKind::ValDef(parameter) = &mut parsed.ast.get_mut(prefix_tree).kind else {
        panic!("extension prefix parameter should be a ValDef");
    };
    parameter
        .metadata
        .modifiers
        .push(dotty_core::ast::Modifier::Erased);

    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "ErasedExtension.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let method = index.symbol_at(source, method_tree).unwrap();
    let parameter = index
        .derived_symbol_at(method, source, prefix_tree)
        .unwrap();

    assert!(
        store
            .symbols
            .get(parameter)
            .flags
            .contains(SymbolFlags::ERASED)
    );
}

#[test]
fn extension_export_children_are_deferred_without_method_symbols() {
    use dotty_core::ast::UntypedNode;

    let source_text = "extension (value: Box) { export value.* }";
    let source = SourceId::from_index(205);
    let mut store = SemanticStore::new();
    let scanner = dotty_lexer::ContextualScanner::new(source_text).expect("source should lex");
    let parsed = parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) =
        &parsed.ast.get(package.stats[0]).kind
    else {
        panic!("extension should remain an ExtensionMethods node");
    };
    let [export] = extension.methods.as_slice() else {
        panic!("extension should contain one export child");
    };
    assert!(matches!(parsed.ast.get(*export).kind, TreeKind::Export(_)));

    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "ExtensionExport.scala",
        &mut store,
        &mut packages,
    )
    .expect("extension exports are deferred by naming");

    assert_eq!(index.symbol_at(source, *export), None);
}

#[test]
fn ordinary_methods_have_no_extension_prefix_metadata() {
    let named = named_source("def ordinary = 1", 206);
    let TreeKind::PackageDef(package) = &named.parsed.ast.get(named.parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let method = named
        .index
        .symbol_at(named.source, package.stats[0])
        .expect("top-level method should be named");

    assert_eq!(named.index.extension_prefix_clauses(method), None);
}

#[test]
fn implicit_extension_prefix_modifier_is_preserved() {
    use dotty_core::ast::UntypedNode;

    let (mut parsed, source, mut store) =
        parsed_source("extension (value: Int)\n  def use = value", 207);
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) =
        &parsed.ast.get(package.stats[0]).kind
    else {
        panic!("extension should remain an ExtensionMethods node");
    };
    let prefix_tree = extension.param_clauses[0][0];
    let method_tree = extension.methods[0];
    let TreeKind::ValDef(parameter) = &mut parsed.ast.get_mut(prefix_tree).kind else {
        panic!("extension prefix parameter should be a ValDef");
    };
    parameter
        .metadata
        .modifiers
        .push(dotty_core::ast::Modifier::Implicit);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "ImplicitExtension.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let method = index.symbol_at(source, method_tree).unwrap();
    let parameter = index
        .derived_symbol_at(method, source, prefix_tree)
        .unwrap();

    assert!(
        store
            .symbols
            .get(parameter)
            .flags
            .contains(SymbolFlags::IMPLICIT)
    );
}

#[test]
fn malformed_extension_child_is_a_structural_namer_error() {
    use dotty_core::ast::UntypedNode;

    let (mut parsed, source, mut store) =
        parsed_source("extension (value: Int)\n  def use = value", 208);
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let extension_tree = package.stats[0];
    let TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) =
        &mut parsed.ast.get_mut(extension_tree).kind
    else {
        panic!("extension should remain an ExtensionMethods node");
    };
    let unexpected_tree = extension.param_clauses[0][0];
    extension.methods.push(unexpected_tree);
    let mut packages = Packages::new();

    assert_eq!(
        name_compilation_unit(
            &parsed.ast,
            parsed.root,
            source,
            "MalformedExtension.scala",
            &mut store,
            &mut packages,
        )
        .unwrap_err(),
        NamerError::MalformedAstShape {
            tree_index: unexpected_tree.index(),
            expected: "DefDef or Export extension method",
        }
    );
    assert!(packages.get::<&str>(&[]).is_none());
}

#[test]
fn malformed_extension_prefix_parameter_is_a_structural_namer_error() {
    use dotty_core::ast::{This, UntypedNode};

    let (mut parsed, source, mut store) =
        parsed_source("extension (value: Int)\n  def use = value", 209);
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) =
        &parsed.ast.get(package.stats[0]).kind
    else {
        panic!("extension should remain an ExtensionMethods node");
    };
    let prefix_tree = extension.param_clauses[0][0];
    parsed.ast.get_mut(prefix_tree).kind = TreeKind::This(This { qual: None });
    let mut packages = Packages::new();

    assert_eq!(
        name_compilation_unit(
            &parsed.ast,
            parsed.root,
            source,
            "MalformedExtensionPrefix.scala",
            &mut store,
            &mut packages,
        )
        .unwrap_err(),
        NamerError::MalformedAstShape {
            tree_index: prefix_tree.index(),
            expected: "TypeDef or ValDef extension prefix parameter",
        }
    );
    assert!(packages.get::<&str>(&[]).is_none());
}
