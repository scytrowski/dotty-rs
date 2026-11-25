use super::*;

#[test]
fn exact_arity_beats_defaults_only_among_applicable_source_overloads() {
    let methods = "class Ops { def chosen(x: Int): Int = 1; def chosen(x: Int, y: Int = 0): Int = 2; def pick(x: Boolean): Int = 1; def pick(x: Int, y: Int = 4): Int = 2 }";
    let calls = "class Use { def a(o: Ops): Int = o.chosen(x = 1); def b(o: Ops): Int = o.chosen(y = 4, x = 1); def c(o: Ops): Int = o.pick(x = 1); def d(o: Ops): Int = o.pick(1); def bad(o: Ops): Int = o.chosen(y = 4) }";
    for reverse in [false, true] {
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let ops = named(2, methods, &mut store, &mut packages);
        let caller = named(3, calls, &mut store, &mut packages);
        let sources = NamedClassSources::new(
            if reverse {
                [&caller, &ops]
            } else {
                [&ops, &caller]
            },
            &store,
        )
        .unwrap();
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
        let [
            crate::ClassBodyError::Member {
                error: crate::typer::TyperError::OverloadApplicationNoApplicable { candidates, .. },
                ..
            },
        ] = report.errors.as_slice()
        else {
            panic!("{report:?}")
        };
        assert!(matches!(
            candidates[1].1,
            crate::typer::OverloadRejection::MissingRequiredArgument { parameter_index: 0 }
        ));
        let mut bodies = crate::CheckedClassBodies::default();
        bodies.record(body).unwrap();
        let index = crate::ResolvedMemberIndex::build(&sources, &bodies, &arena, &store);
        for (token, i) in [
            ("chosen(x", 0),
            ("chosen(y = 4,", 1),
            ("pick(x", 3),
            ("pick(1", 3),
        ] {
            let method = ops.classes()[0].methods().nth(i).unwrap().symbol;
            assert_eq!(
                index
                    .call_at(caller.source(), calls.find(token).unwrap() as u32)
                    .unwrap()
                    .method,
                method
            );
        }
    }
}

#[test]
fn unfinished_and_checked_signatures_keep_the_same_defaulted_declaration() {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let provider = named(
        8,
        "class Ops { def diff(left: Int = 9, right: Int = 4): Int = left - right }",
        &mut store,
        &mut packages,
    );
    let calls = "class Use { def a(o: Ops): Int = o.diff(right = 4); def b(o: Ops): Int = o.diff(); def c(o: Ops): Int = o.diff(9) }";
    let caller = named(3, calls, &mut store, &mut packages);
    let sources = NamedClassSources::new([&caller, &provider], &store).unwrap();
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
    let mut bodies = crate::CheckedClassBodies::default();
    bodies.record(body).unwrap();
    let index = crate::ResolvedMemberIndex::build(&sources, &bodies, &arena, &store);
    let method = provider.classes()[0].methods().next().unwrap().symbol;
    for (token, delta, active, defaults) in [
        ("right = 4", 8, Some(1), &[0][..]),
        ("diff()", 5, None, &[0, 1]),
        ("diff(9)", 5, Some(0), &[1]),
    ] {
        let query = index
            .call_signature_at(caller.source(), (calls.find(token).unwrap() + delta) as u32)
            .unwrap();
        assert_eq!(
            (query.call.method, query.active_parameter),
            (method, active)
        );
        assert_eq!(
            query
                .defaulted
                .iter()
                .map(|d| (d.formal, d.getter))
                .collect::<Vec<_>>(),
            defaults
                .iter()
                .map(|&i| (i, store.default_getter(method, i).unwrap()))
                .collect::<Vec<_>>()
        );
    }
    let id = SourceId::from_index(4);
    let text = "class Edit { def run(o: Ops): Int = o.diff(right = ";
    let parsed = crate::parse_source(id, text, Default::default())
        .unwrap()
        .bind(&mut store.names);
    let idx = crate::IncompleteCallIndex::build(
        &sources,
        [("Edit.scala", parsed)],
        store,
        defs,
        packages,
    );
    let help = idx.at(id, text.len() as u32).unwrap();
    let [candidate] = help.candidates.as_slice() else {
        panic!()
    };
    assert_eq!((candidate.method, candidate.active_parameter), (method, 1));
    assert!(
        candidate
            .parameters
            .iter()
            .all(|p| p.default_getter.is_some())
    );
}
