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
