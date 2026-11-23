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

fn invalid_clients() -> NamedTestUnit {
    let invalid =
        include_str!("../../../../tests/fixtures/contextual-arguments/OmittedWitnessErrors.scala");
    parse_and_name(&format!(
        "{LIB}\n{}",
        invalid.strip_prefix("package omitted\n").unwrap()
    ))
}

#[test]
fn missing_and_ambiguous_goals_report_the_first_failed_search_and_roll_back() {
    let (parsed, mut store, packages, defs, index, source) = invalid_clients();
    let box_class = class_symbol(&parsed, &store, &index, source, "Box");
    let fmt_class = class_symbol(&parsed, &store, &index, source, "Fmt");
    let mut t = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for (site, ambiguous, closed) in [
        ("absent", false, false),
        ("boundBad", false, false),
        ("noSecond", false, true),
        ("mismatch", false, true),
        ("ambSame", true, false),
        ("ambDiff", true, false),
        ("lateFilter", true, false),
    ] {
        let (_, rhs) = method_definition_and_rhs(&parsed, t.store(), &index, source, site);
        t.transaction_failure_error_observer = Some(std::rc::Rc::new(move |t, error| {
            let expected = match error {
                TyperError::MissingContextualArgument { expected, .. }
                | TyperError::AmbiguousContextualArgument { expected, .. } => *expected,
                _ => panic!("{site}: {error:?}"),
            };
            let Type::Applied { tycon, args } = t.store.types.get(expected) else {
                panic!("{site}")
            };
            assert_eq!(
                t.store.types.get(*tycon).reference_symbol(),
                Some(if closed { fmt_class } else { box_class })
            );
            if closed {
                assert_eq!(args, &[defs.int]);
            } else {
                assert!(matches!(
                    t.store.types.get(args[0]),
                    Type::ParamRef { index: 0, .. }
                ));
            }
        }));
        let checkpoint = t.store.checkpoint();
        let trees = t.typed_arena.checkpoint();
        for _ in 0..2 {
            let err = client_result(&mut t, &parsed, site).unwrap_err();
            match err {
                TyperError::AmbiguousContextualArgument {
                    tree_index,
                    candidates,
                    ..
                } if ambiguous => {
                    assert_eq!(tree_index, rhs.index());
                    let mut names: Vec<_> =
                        candidates.iter().map(|&s| qualified_name(&t, s)).collect();
                    names.sort();
                    assert_eq!(names, [(site, "a"), (site, "b")]);
                }
                TyperError::MissingContextualArgument { tree_index, .. } if !ambiguous => {
                    assert_eq!(tree_index, rhs.index())
                }
                _ => panic!("{site}: {err:?}"),
            }
            assert_eq!(t.store.checkpoint(), checkpoint);
            assert_eq!(t.typed_arena.checkpoint(), trees);
            assert!(t.typed_index.get(source, rhs).is_none());
        }
    }
    t.transaction_failure_error_observer = None;
    let tree = client_result(&mut t, &parsed, "earlyFilter").unwrap();
    let TreeKind::Apply(a) = &t.typed_ast().get(tree).kind else {
        panic!()
    };
    assert_eq!(
        qualified_name(&t, reference(&t, a.args[0])),
        ("earlyFilter", "f")
    );
    assert_eq!(
        qualified_name(&t, reference(&t, a.args[1])),
        ("earlyFilter", "a")
    );
}

#[test]
fn open_witnesses_satisfy_the_callers_bounds_before_ranking() {
    let (parsed, mut store, packages, defs, index, source) = invalid_clients();
    let dog = class_symbol(&parsed, &store, &index, source, "Dog");
    let mut t = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for site in ["boundOk", "boundMix"] {
        let tree = client_result(&mut t, &parsed, site).unwrap();
        let Type::Applied { args, .. } = t.store.types.get(t.typed_ast().get(tree).ty) else {
            panic!()
        };
        assert_eq!(t.store.types.get(args[0]).reference_symbol(), Some(dog));
        let TreeKind::Apply(a) = &t.typed_ast().get(tree).kind else {
            panic!()
        };
        assert_eq!(qualified_name(&t, reference(&t, a.args[0])), (site, "d"));
    }
}
