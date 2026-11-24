use super::*;
use crate::{CheckedClassBodies, ResolvedMemberIndex};

#[test]
fn same_named_class_declarations_keep_their_package_identity_before_typing() {
    let mut store = SemanticStore::new();
    let mut packages = Packages::new();
    let texts = ["package a\nclass Base", "package b\nclass Base"];
    let a = named(6, texts[0], &mut store, &mut packages);
    let b = named(2, texts[1], &mut store, &mut packages);
    let sources = NamedClassSources::new([&b, &a], &store).unwrap();
    let index = ResolvedMemberIndex::build(
        &sources,
        &CheckedClassBodies::default(),
        &AstArena::new(),
        &store,
    );
    assert!(index.diagnostics().is_empty());
    assert_ne!(a.classes()[0].symbol, b.classes()[0].symbol);
    for (file, text) in [(&a, texts[0]), (&b, texts[1])] {
        let at = text.find("Base").unwrap() as u32;
        let site = index.symbol_at(file.source(), at).unwrap();
        assert_eq!(
            (site.symbol, site.kind, site.usage),
            (file.classes()[0].symbol, SymbolKind::Class, None)
        );
        assert_eq!(site.target.source, file.source());
        assert_eq!(site.target.name.start(), at);
        assert_eq!(site.target.name.end(), at + 4);
        assert_eq!(
            site.target.declaration.start(),
            text.find("class").unwrap() as u32
        );
        assert_eq!(site.target.declaration.end(), at + 4);
        assert!(index.symbol_at(file.source(), at + 4).is_none());
        assert!(index.references_to(site.symbol).is_empty());
    }
}

#[test]
fn bare_and_applied_parent_names_select_the_completed_class_in_the_right_package() {
    for (base, child) in [
        ("class Base", "class Child extends Base"),
        (
            "class Base(n: Int)",
            "class Child(start: Int) extends Base(start)",
        ),
    ] {
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let decoy = named(4, "package other\nclass Base", &mut store, &mut packages);
        let base = named(
            3,
            &format!("package demo\n{base}"),
            &mut store,
            &mut packages,
        );
        let text = format!("package demo\n{child}");
        let leaf = named(7, &text, &mut store, &mut packages);
        let sources = NamedClassSources::new([&decoy, &leaf, &base], &store).unwrap();
        prepare_constructors(&sources, &mut store, defs, &packages);
        let mut arena = AstArena::new();
        let mut headers = ClassHeaderBindings::default();
        for file in [&base, &leaf] {
            assert!(matches!(
                sources.try_complete_class_header(
                    file.classes()[0].symbol,
                    &mut store,
                    defs,
                    &packages,
                    &mut arena,
                    &mut headers
                ),
                Ok(SignatureCompletion::Complete(_))
            ));
        }
        let index = ResolvedMemberIndex::build_with_headers(
            &sources,
            &CheckedClassBodies::default(),
            &headers,
            &arena,
            &store,
        );
        let pos = text.find("extends Base").unwrap() as u32 + 8;
        let relation = index.parent_at(leaf.source(), pos).unwrap();
        assert_eq!(
            (relation.child, relation.parent),
            (leaf.classes()[0].symbol, base.classes()[0].symbol)
        );
        assert_eq!(
            relation.target,
            index
                .symbol_at(base.source(), "package demo\nclass ".len() as u32)
                .unwrap()
                .target
        );
        assert_eq!(relation.origin.source(), leaf.source());
        assert_eq!(relation.origin.span().range().start(), pos);
        assert_eq!(relation.origin.span().range().end(), pos + 4);
        assert!(index.parent_at(leaf.source(), pos + 4).is_none());
        if child.ends_with("(start)") {
            let arg = text.rfind("start").unwrap() as u32;
            assert!(index.parent_at(leaf.source(), arg).is_none());
            assert_eq!(
                index.symbol_at(leaf.source(), arg).unwrap().kind,
                SymbolKind::Parameter
            );
        }
    }
}

#[test]
fn only_completed_headers_contribute_references_in_file_and_token_order() {
    let texts = [
        "class First(x: Int) extends Base(x); class Second(x: Int) extends Base(x)",
        "class Bad(x: Boolean) extends Base(x)",
        "class A(x: Int) extends B(x)",
        "class B(x: Int) extends A(x)",
        "class Base(x: Int)",
        "class Third(x: Int) extends Base(x)",
    ];
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let files: Vec<_> = texts
        .iter()
        .enumerate()
        .map(|(i, text)| named(i as u32, text, &mut store, &mut packages))
        .collect();
    let sources = NamedClassSources::new(
        [
            &files[5], &files[0], &files[1], &files[2], &files[3], &files[4],
        ],
        &store,
    )
    .unwrap();
    prepare_constructors(&sources, &mut store, defs, &packages);
    let mut arena = AstArena::new();
    let mut headers = ClassHeaderBindings::default();
    let id = |i: usize| files[i].classes()[0].symbol;
    let attempt = |class,
                   store: &mut SemanticStore,
                   arena: &mut AstArena<Typed>,
                   headers: &mut ClassHeaderBindings| {
        sources.try_complete_class_header(class, store, defs, &packages, arena, headers)
    };
    for (child, parent) in [(0, 4), (2, 3), (3, 2)] {
        assert!(
            matches!(attempt(id(child),&mut store,&mut arena,&mut headers),Ok(SignatureCompletion::BlockedOn(p)) if p==id(parent))
        );
    }
    let build = |store: &SemanticStore, arena: &AstArena<Typed>, headers: &ClassHeaderBindings| {
        ResolvedMemberIndex::build_with_headers(
            &sources,
            &CheckedClassBodies::default(),
            headers,
            arena,
            store,
        )
    };
    let pending = build(&store, &arena, &headers);
    assert!(pending.references_to(id(4)).is_empty());
    for class in [id(4), id(0), id(5), files[0].classes()[1].symbol, id(0)] {
        assert!(matches!(
            attempt(class, &mut store, &mut arena, &mut headers),
            Ok(SignatureCompletion::Complete(_))
        ));
    }
    assert!(matches!(
        attempt(id(1), &mut store, &mut arena, &mut headers),
        Err(TyperError::ApplicationArgumentTypeMismatch { .. })
    ));
    let index = build(&store, &arena, &headers);
    let mut expected = Vec::new();
    for i in [5, 0] {
        for (at, _) in texts[i].match_indices("Base") {
            let site = index.symbol_at(files[i].source(), at as u32).unwrap();
            assert_eq!((site.symbol, site.kind), (id(4), SymbolKind::Class));
            assert_eq!(site.target.source, files[4].source());
            expected.push(site.usage.unwrap());
        }
    }
    assert_eq!(index.references_to(id(4)), expected);
    for i in [1, 2, 3] {
        let pos = texts[i].find("extends ").unwrap() as u32 + 8;
        assert!(index.symbol_at(files[i].source(), pos).is_none());
        assert!(index.references_to(id(i)).is_empty());
    }
}
