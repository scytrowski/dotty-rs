use dotty::core::{
    Definitions, Packages, SemanticStore, SourceDefinition, SourceId, SourceSemanticIndex,
    SourceText, SymbolKind, TreeKind,
};
use dotty::lexer::ContextualScanner;
use dotty::namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty::parser::{ParseResult, parse_compilation_unit};
use dotty::source_declarations::{ClassMember, DeclarationError, source_classes};

fn named(text: &str) -> (ParseResult, SourceId, SemanticStore, SourceSemanticIndex) {
    let mut store = SemanticStore::new();
    Definitions::bootstrap(&mut store);
    let source = SourceId::from_index(7);
    let parsed = parse_compilation_unit(
        SourceText::new(text).unwrap(),
        source,
        ContextualScanner::new(text).unwrap(),
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let NamingOutcome::Named { index, diagnostics } = name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source,
        "Classes.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut Packages::new(),
    )
    .unwrap() else {
        panic!("expected named declarations")
    };
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    (parsed, source, store, *index)
}

#[test]
fn declarations_follow_source_order_and_bind_to_their_owners_before_typing() {
    let (parsed, source, store, index) = named(
        "package demo\nclass Z(seed: Int, val id: Int, var count: Int) extends A(seed) { val z = seed; override def value() = z; var a = 1 }\nclass A(x: Int) { def value() = x; val base = 2 }",
    );
    let classes = source_classes(&parsed.ast, parsed.root, source, &index, &store).unwrap();
    let [z, a] = classes.as_slice() else {
        panic!("expected two classes")
    };
    let name = |symbol| store.names.resolve(store.symbols.get(symbol).name.text());
    assert_eq!([name(z.symbol), name(a.symbol)], ["Z", "A"]);
    let package = index.symbol_at(source, parsed.root).unwrap();
    for class in &classes {
        assert_eq!(store.symbols.owner(class.symbol), Some(package));
        assert_eq!(index.symbol_at(source, class.tree), Some(class.symbol));
        let TreeKind::TypeDef(def) = &parsed.ast.get(class.tree).kind else {
            panic!()
        };
        let TreeKind::Template(template) = &parsed.ast.get(def.rhs).kind else {
            panic!()
        };
        assert_eq!(
            index.symbol_at(source, template.constructor),
            Some(class.constructor)
        );
        assert_eq!(store.symbols.owner(class.constructor), Some(class.symbol));
        assert_eq!(class.parents, template.parents);
        assert_eq!(class.members.len(), template.body.len());
        for (&tree, member) in template.body.iter().zip(&class.members) {
            let (decl, kind) = match member {
                ClassMember::Method(d) => (d, SymbolKind::Method),
                ClassMember::Field(d) => (d, SymbolKind::Field),
            };
            assert_eq!(decl.tree, tree);
            assert_eq!(
                index.definition_of(decl.symbol),
                Some(SourceDefinition::Canonical { source, tree })
            );
            assert_eq!(store.symbols.owner(decl.symbol), Some(class.symbol));
            assert_eq!(store.symbols.get(decl.symbol).kind, kind);
        }
    }
    assert_eq!(
        z.constructor_fields
            .iter()
            .copied()
            .map(name)
            .collect::<Vec<_>>(),
        ["id", "count"]
    );
    assert!(
        z.constructor_fields
            .iter()
            .all(|&id| store.symbols.owner(id) == Some(z.symbol))
    );
    assert!(a.constructor_fields.is_empty());
    assert_eq!(
        z.fields().map(|d| name(d.symbol)).collect::<Vec<_>>(),
        ["z", "a"]
    );
    assert_eq!(z.methods().next().map(|d| name(d.symbol)), Some("value"));
    assert_eq!(a.methods().next().map(|d| name(d.symbol)), Some("value"));
    assert_ne!(
        z.methods().next().unwrap().symbol,
        a.methods().next().unwrap().symbol
    );
}

#[test]
fn unsupported_statement_reports_the_exact_source_tree() {
    let (parsed, source, store, index) = named("class Good; class Bad { def f() = 1; f() }");
    let tree = parsed
        .ast
        .iter()
        .find_map(|(id, node)| matches!(node.kind, TreeKind::Apply(_)).then_some(id))
        .unwrap();
    assert_eq!(
        source_classes(&parsed.ast, parsed.root, source, &index, &store).unwrap_err(),
        DeclarationError::Unsupported { source, tree }
    );
}

#[test]
fn a_member_bound_to_another_class_reports_its_declaration_tree() {
    let (parsed, source, mut store, index) = named("class A { def value() = 1 }; class B");
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("expected package")
    };
    let owner = index.symbol_at(source, package.stats[1]).unwrap();
    let (tree, method) = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::DefDef(def) = &node.kind else {
                return None;
            };
            def.rhs
                .and_then(|_| index.symbol_at(source, tree))
                .map(|id| (tree, id))
        })
        .unwrap();
    store.symbols.get_mut(method).owner = Some(owner);
    assert_eq!(
        source_classes(&parsed.ast, parsed.root, source, &index, &store).unwrap_err(),
        DeclarationError::InvalidBinding { source, tree }
    );
}

#[test]
fn declaration_index_covers_class_constructor_fields_and_methods_in_the_batch() {
    use dotty::{SourceDeclaration, SourceDeclarationIndex};
    let (parsed, source, store, index) =
        named("class A(val id: Int) { val seed = 1; def read() = seed }; class Outside");
    let classes = source_classes(&parsed.ast, parsed.root, source, &index, &store).unwrap();
    let class = &classes[0];
    let batch = SourceDeclarationIndex::from_classes(&store, [class]).unwrap();
    for (symbol, kind) in [
        (class.symbol, SymbolKind::Class),
        (class.constructor, SymbolKind::Constructor),
        (class.constructor_fields[0], SymbolKind::Field),
        (class.fields().next().unwrap().symbol, SymbolKind::Field),
        (class.methods().next().unwrap().symbol, SymbolKind::Method),
    ] {
        assert_eq!(
            batch.get(symbol),
            Some(SourceDeclaration {
                source,
                class: class.symbol,
                kind
            })
        );
    }
    assert_eq!(batch.get(classes[1].symbol), None);
}

#[test]
fn declaration_index_rejects_duplicate_classes_and_a_substituted_field() {
    use dotty::{SourceDeclarationIndex, SourceDeclarationIndexError};
    let (parsed, source, store, index) = named("class A(val seed: Int); class B(val seed: Int)");
    let mut classes = source_classes(&parsed.ast, parsed.root, source, &index, &store).unwrap();
    let a = &classes[0];
    assert_eq!(
        SourceDeclarationIndex::from_classes(&store, [a, a]).unwrap_err(),
        SourceDeclarationIndexError::DuplicateDeclaration(a.symbol)
    );
    let foreign = classes[1].constructor_fields[0];
    classes[0].constructor_fields[0] = foreign;
    assert_eq!(
        SourceDeclarationIndex::from_classes(&store, [&classes[0]]).unwrap_err(),
        SourceDeclarationIndexError::InvalidDeclaration(foreign)
    );
}
