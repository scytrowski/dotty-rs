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

#[test]
fn partial_bodies_publish_only_successful_method_and_field_constructions() {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let base = named(9, "class Base(n: Int)", &mut store, &mut packages);
    let text = "class Use { val kept: Base = new Base(1); def bad(): Base = { new Base(2); new Base(true) }; def good(): Base = new Base(3) }";
    let caller = named(4, text, &mut store, &mut packages);
    let sources = NamedClassSources::new([&base, &caller], &store).unwrap();
    let mut arena = prepare_bodies(&sources, &mut store, defs, &packages);
    let class = &caller.classes()[0];
    let (body, report) = sources
        .check_class_body(
            class.symbol,
            &mut store,
            defs,
            &packages,
            &mut arena,
            crate::core::NoResolver,
        )
        .into_parts();
    assert!(matches!(body, crate::CheckedClassBody::Partial { .. }));
    let [
        crate::ClassBodyError::Member {
            member,
            error:
                TyperError::ApplicationArgumentTypeMismatch {
                    source,
                    argument_index: 0,
                    expected,
                    actual_description,
                    ..
                },
            ..
        },
    ] = report.errors.as_slice()
    else {
        panic!("wrong report: {report:?}")
    };
    assert_eq!(member.symbol(), class.methods().next().unwrap().symbol);
    assert_eq!((*source, *expected), (caller.source(), defs.int));
    assert_eq!(actual_description.as_ref(), "Boolean");
    let mut bodies = CheckedClassBodies::default();
    bodies.record(body).unwrap();
    let index = ResolvedMemberIndex::build(&sources, &bodies, &arena, &store);
    let mut expected = Vec::new();
    for (arg, ok) in [("1", true), ("2", false), ("true", false), ("3", true)] {
        let pos = text.find(&format!("Base({arg})")).unwrap() as u32;
        let site = index.construction_at(caller.source(), pos);
        assert_eq!(site.is_some(), ok);
        assert_eq!(index.symbol_at(caller.source(), pos).is_some(), ok);
        if let Some(site) = site {
            assert_eq!(site.constructor, base.classes()[0].constructor);
            expected.push(site.origin);
        }
    }
    for (pos, _) in text.match_indices("(): Base") {
        expected.push(
            index
                .method_type_at(caller.source(), pos as u32 + 4)
                .unwrap()
                .origin,
        );
    }
    expected.sort_by_key(|s| s.span().range().start());
    assert_eq!(index.references_to(base.classes()[0].symbol), expected);
}

#[test]
fn unfinished_constructor_prefix_uses_written_types_and_formal_ordinals() {
    for (args, slot) in [
        ("count = ", Some(3)),
        ("count = 4, enabled = ", Some(2)),
        ("1, 9, true, ", Some(3)),
        ("count = 4, ", Some(0)),
        ("count = 4, count = ", None),
        ("missing = ", None),
        ("count = false, ", None),
        ("0, 1, true, 4, ", None),
        ("count = 4, 1, ", None),
    ] {
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let base = named(
            3,
            "class Base(seed: Int, val id: Int, enabled: Boolean, var count: Int)",
            &mut store,
            &mut packages,
        );
        let sources = NamedClassSources::new([&base], &store).unwrap();
        prepare_bodies(&sources, &mut store, defs, &packages);
        let source = SourceId::from_index(7);
        let text = format!("class Use {{ def make(): Base = new Base({args}");
        let parsed = crate::parse_source(source, &text, Default::default())
            .unwrap()
            .bind(&mut store.names);
        let index = crate::IncompleteCallIndex::build(
            &sources,
            [("Use.scala", parsed)],
            store,
            defs,
            packages,
        );
        assert_eq!(
            index
                .primary_constructor_at(source, text.len() as u32)
                .map(|s| s.active_parameter),
            slot,
            "{args}"
        );
        if args.starts_with("count = false") {
            let before = text.find("false").unwrap() as u32;
            assert_eq!(
                index
                    .primary_constructor_at(source, before)
                    .unwrap()
                    .active_parameter,
                3
            );
        }
    }
}

#[test]
fn unfinished_parent_uses_the_child_formal_and_rejects_its_uninitialized_receiver() {
    for reversed in [false, true] {
        for (arg, valid) in [("initial", true), ("this.initial", false), ("false", false)] {
            let mut store = SemanticStore::new();
            let defs = Definitions::bootstrap(&mut store);
            let packages = Packages::new();
            let child_id = SourceId::from_index(8);
            let child = format!("class Child(val initial: Int) extends Base(count = {arg}, id = ");
            let mut files = [
                (
                    SourceId::from_index(1),
                    "Base.scala",
                    "class Base(val id: Int, var count: Int); class Other { def bad = missing(",
                ),
                (child_id, "Child.scala", child.as_str()),
            ];
            if reversed {
                files.reverse();
            }
            let parsed = files.map(|(id, name, text)| {
                (
                    name,
                    crate::parse_source(id, text, Default::default())
                        .unwrap()
                        .bind(&mut store.names),
                )
            });
            let sources = NamedClassSources::new([], &store).unwrap();
            let index = crate::IncompleteCallIndex::build(&sources, parsed, store, defs, packages);
            let sig = index.primary_constructor_at(child_id, child.len() as u32);
            assert_eq!(sig.is_some(), valid, "{child}");
            if let Some(sig) = sig {
                assert_eq!(sig.declarations[0].target.source, SourceId::from_index(1));
                assert_eq!((sig.active_parameter, &*sig.parameters[0].name), (0, "id"));
                assert!(
                    matches!(sig.usage, crate::SourceConstructorUse::DirectParent { child } if child != sig.instance.symbol)
                );
            }
        }
    }
}
