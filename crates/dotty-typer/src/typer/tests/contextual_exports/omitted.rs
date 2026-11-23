use super::*;

const LIB: &str =
    include_str!("../../../../tests/fixtures/contextual-arguments/OmittedWitness.scala");

#[test]
fn ordered_search_combines_written_and_omitted_constraints() {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(LIB);
    let mut t = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for (site, types, witnesses) in [
        ("explicit", vec![defs.int], vec![(0, "b"), (1, "f")]),
        ("written", vec![defs.int], vec![(0, "b"), (1, "f")]),
        ("single", vec![defs.int], vec![(0, "b")]),
        ("trailing", vec![defs.boolean], vec![(1, "b")]),
        ("both", vec![defs.int], vec![(0, "b"), (1, "f")]),
        ("reverse", vec![defs.int], vec![(0, "f"), (1, "b")]),
        (
            "twoVars",
            vec![defs.int, defs.boolean],
            vec![(0, "b"), (1, "f")],
        ),
        (
            "partial",
            vec![defs.int, defs.boolean],
            vec![(0, "b"), (1, "f")],
        ),
    ] {
        let tree = client_result(&mut t, &parsed, site).unwrap();
        let Type::Applied { args, .. } = t.store.types.get(t.typed_ast().get(tree).ty) else {
            panic!("{site}")
        };
        assert_eq!(*args, types, "{site}");
        let TreeKind::Apply(app) = &t.typed_ast().get(tree).kind else {
            panic!("{site}")
        };
        assert_eq!(app.kind, ApplyKind::Using);
        for (ordinal, name) in witnesses {
            assert_eq!(
                qualified_name(&t, reference(&t, app.args[ordinal])),
                (site, name)
            );
        }
    }
}
