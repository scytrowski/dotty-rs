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
