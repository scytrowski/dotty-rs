use super::*;
use crate::{CheckedClassBodies, ResolvedMemberIndex};

#[test]
fn explicit_new_retains_the_selected_constructor_of_the_correct_source_class() {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let other = named(5, "package other\nclass Base", &mut store, &mut packages);
    let base = named(2, "package demo\nclass Base", &mut store, &mut packages);
    let text = "package demo\nclass Caller { def bare(): Base = new Base; def empty(): Base = new Base() }";
    let caller = named(7, text, &mut store, &mut packages);
    let sources = NamedClassSources::new([&other, &caller, &base], &store).unwrap();
    let mut arena = prepare_bodies(&sources, &mut store, defs, &packages);
    let (body, report) = sources
        .check_class_body(
            caller.classes()[0].symbol,
            &mut store,
            defs,
            &packages,
            &mut arena,
            crate::core::NoResolver,
        )
        .into_parts();
    assert!(report.errors.is_empty(), "{report:?}");
    let mut bodies = CheckedClassBodies::default();
    bodies.record(body).unwrap();
    let index = ResolvedMemberIndex::build(&sources, &bodies, &arena, &store);
    for (start, _) in text.match_indices("new Base") {
        let pos = start as u32 + 4;
        let hit = index.construction_at(caller.source(), pos).unwrap();
        assert_eq!(
            (hit.class, hit.constructor),
            (base.classes()[0].symbol, base.classes()[0].constructor)
        );
        assert_eq!(store.symbols.get(hit.constructor).owner, Some(hit.class));
        assert_eq!(
            hit.target,
            index
                .symbol_at(base.source(), "package demo\nclass ".len() as u32)
                .unwrap()
                .target
        );
        assert_eq!(hit.origin.span().range().start(), pos);
        assert!(index.construction_at(caller.source(), pos + 4).is_none());
    }
}

#[test]
fn references_interleave_constructions_and_parents_and_preserve_neighboring_terms() {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let first =
        "class Base(seed: Int) { def read(): Int = seed }; class First(x: Int) extends Base(x)";
    let second = "class Factory { def make(start: Int): Int = new Base(start).read() }; class Last(x: Int) extends Base(x)";
    let a = named(9, first, &mut store, &mut packages);
    let b = named(1, second, &mut store, &mut packages);
    let sources = NamedClassSources::new([&a, &b], &store).unwrap();
    let mut arena = prepare_bodies(&sources, &mut store, defs, &packages);
    let (body, report) = sources
        .check_class_body(
            b.classes()[0].symbol,
            &mut store,
            defs,
            &packages,
            &mut arena,
            crate::core::NoResolver,
        )
        .into_parts();
    assert!(report.errors.is_empty(), "{report:?}");
    let mut bodies = CheckedClassBodies::default();
    bodies.record(body).unwrap();
    let index = ResolvedMemberIndex::build(&sources, &bodies, &arena, &store);
    let class = &a.classes()[0];
    let mut expected = Vec::new();
    for (file, text, pattern) in [
        (&a, first, "Base(x)"),
        (&b, second, "Base(start)"),
        (&b, second, "Base(x)"),
    ] {
        let at = text.find(pattern).unwrap() as u32;
        let site = index.symbol_at(file.source(), at).unwrap();
        assert_eq!((site.symbol, site.kind), (class.symbol, SymbolKind::Class));
        expected.push(site.usage.unwrap());
    }
    assert_eq!(index.references_to(class.symbol), expected);
    assert!(index.references_to(class.constructor).is_empty());
    let arg = index
        .symbol_at(b.source(), second.rfind("start").unwrap() as u32)
        .unwrap();
    assert_eq!(arg.kind, SymbolKind::Parameter);
    assert_eq!(
        arg.target.name.start(),
        second.find("start:").unwrap() as u32
    );
    let read = index
        .symbol_at(b.source(), second.find("read()").unwrap() as u32)
        .unwrap();
    assert_eq!(
        (read.kind, read.symbol),
        (SymbolKind::Method, class.methods().next().unwrap().symbol)
    );
}
