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
    assert_eq!(store.checkpoint(), before);
}
