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
