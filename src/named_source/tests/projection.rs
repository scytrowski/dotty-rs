use super::*;
use crate::source_declarations::read_source_classes;

fn named(store: &mut SemanticStore, text: &str) -> NamedSource {
    crate::parse_source(SourceId::from_index(7), text, Default::default())
        .unwrap()
        .bind(&mut store.names)
        .name("Model.scala", store, &mut Packages::new())
        .unwrap()
}

#[test]
fn classes_and_unsupported_sites_each_keep_package_order() {
    for traits in ["trait T", "trait T; trait U"] {
        let mut store = SemanticStore::new();
        let text = format!("class A; {traits}; class B");
        let file = named(&mut store, &text);
        let mut errors = Vec::new();
        let classes = read_source_classes(
            &file.syntax.ast,
            file.syntax.root,
            file.source(),
            file.index(),
            &store,
            |error| {
                errors.push(error);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            classes
                .iter()
                .map(|c| store.names.resolve(store.symbols.get(c.symbol).name.text()))
                .collect::<Vec<_>>(),
            ["A", "B"]
        );
        let sites: Vec<_> = errors.iter().map(|error| {
            assert!(matches!(error, DeclarationError::Unsupported {source, ..} if *source == file.source()));
            let range = error.location(&file.syntax.ast).unwrap().span().range();
            &text[range.start() as usize..range.end() as usize]
        }).collect();
        assert_eq!(sites, traits.split("; ").collect::<Vec<_>>());
        assert_eq!(file.into_classes(&store).unwrap_err().error, errors[0]);
    }
}

#[test]
fn an_unsupported_body_statement_omits_its_entire_class() {
    let text = "class Before; class Bad { def f() = 1; f() }; class After";
    let mut store = SemanticStore::new();
    let file = named(&mut store, text);
    let before = store.checkpoint();
    let mut errors = Vec::new();
    let classes = read_source_classes(
        &file.syntax.ast,
        file.syntax.root,
        file.source(),
        file.index(),
        &store,
        |error| {
            errors.push(error);
            Ok(())
        },
    )
    .unwrap();
    let TreeKind::PackageDef(package) = &file.syntax.ast.get(file.syntax.root).kind else {
        panic!()
    };
    assert_eq!(
        classes.iter().map(|c| c.tree).collect::<Vec<_>>(),
        [package.stats[0], package.stats[2]]
    );
    let [error @ DeclarationError::Unsupported { tree, .. }] = errors.as_slice() else {
        panic!("expected one unsupported statement")
    };
    assert!(matches!(
        file.syntax.ast.get(*tree).kind,
        TreeKind::Apply(_)
    ));
    let range = error.location(&file.syntax.ast).unwrap().span().range();
    assert_eq!(&text[range.start() as usize..range.end() as usize], "f()");
    assert_eq!(store.checkpoint(), before);
}

#[test]
fn a_wrong_owner_aborts_even_after_a_valid_class_and_an_unsupported_declaration() {
    let mut store = SemanticStore::new();
    let file = named(
        &mut store,
        "class Before; trait T; class After { def value() = 42 }",
    );
    let TreeKind::PackageDef(pkg) = &file.syntax.ast.get(file.syntax.root).kind else {
        panic!()
    };
    let source = file.source();
    let wrong_owner = file.index().symbol_at(source, pkg.stats[0]).unwrap();
    let tree = file
        .syntax
        .ast
        .iter()
        .find_map(|(tree, node)| {
            matches!(&node.kind, TreeKind::DefDef(d) if d.rhs.is_some()).then_some(tree)
        })
        .unwrap();
    let member = file.index().symbol_at(source, tree).unwrap();
    store.symbols.get_mut(member).owner = Some(wrong_owner);
    let before = store.checkpoint();
    let error = read_source_classes(
        &file.syntax.ast,
        file.syntax.root,
        source,
        file.index(),
        &store,
        |_| Ok(()),
    )
    .unwrap_err();
    assert_eq!(error, DeclarationError::InvalidBinding { source, tree });
    let failure = file.project_classes(&store).unwrap_err();
    assert_eq!(failure.error, error);
    assert_eq!(failure.named.index().symbol_at(source, tree), Some(member));
    assert_eq!(store.checkpoint(), before);
}

#[test]
fn an_empty_view_distinguishes_an_empty_file_from_an_omitted_trait() {
    for text in ["", "trait T"] {
        let mut store = SemanticStore::new();
        let named = named(&mut store, text);
        let original = Arc::clone(&named.syntax);
        let before = store.checkpoint();
        let ClassProjection { file, rejections } = named.project_classes(&store).unwrap();
        assert!(file.classes().is_empty());
        assert!(std::ptr::eq(file.ast(), &original.ast));
        if text.is_empty() {
            assert!(rejections.is_empty());
        } else {
            let [report] = rejections.as_slice() else {
                panic!("expected one rejection")
            };
            let DeclarationError::Unsupported { source, tree } = report.error else {
                panic!()
            };
            assert_eq!(report.error.source_id(), file.source());
            let symbol = file.index().symbol_at(source, tree).unwrap();
            assert_eq!(
                store.symbols.get(symbol).kind,
                crate::core::SymbolKind::Trait
            );
            drop(file);
            drop(original);
            let span = report.location.unwrap();
            assert_eq!(span.source(), source);
            assert_eq!(
                span.span().range(),
                crate::core::TextRange::new(0, 7).unwrap()
            );
        }
        assert_eq!(store.checkpoint(), before);
    }
}

#[test]
fn a_missing_package_binding_returns_the_original_named_file() {
    let mut store = SemanticStore::new();
    let mut file = named(&mut store, "class A; trait T");
    let original = Arc::clone(&file.syntax);
    let source = file.source();
    file.index = Box::default();
    let before = store.checkpoint();
    let failure = file.project_classes(&store).unwrap_err();
    assert_eq!(
        failure.error,
        DeclarationError::InvalidBinding {
            source,
            tree: original.root
        }
    );
    assert!(Arc::ptr_eq(&original, &failure.named.syntax));
    assert_eq!(store.checkpoint(), before);
}
