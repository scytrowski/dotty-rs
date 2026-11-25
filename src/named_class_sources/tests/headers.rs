use super::*;

#[test]
fn constructor_signature_uses_the_selected_class_and_rejects_other_roots() {
    use crate::core::{SymbolInfo, Type};
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let a = named(
        42,
        "class A(start: Int) { def value() = 1 }",
        &mut store,
        &mut packages,
    );
    let b = named(7, "class B(start: Boolean)", &mut store, &mut packages);
    let omitted = named(13, "class Other(start: Int)", &mut store, &mut packages);
    let sources = NamedClassSources::new([&a, &b], &store).unwrap();
    for (file, expected) in [(&b, defs.boolean), (&a, defs.int)] {
        let class = &file.classes()[0];
        let ty = sources
            .complete_constructor_signature(class.symbol, &mut store, defs, &packages)
            .unwrap();
        assert_eq!(
            store.symbols.info(class.constructor),
            &SymbolInfo::Complete(ty)
        );
        let Type::Method(method) = store.types.get(ty) else {
            panic!("expected constructor method")
        };
        let [start] = method.params.as_slice() else {
            panic!("expected one parameter")
        };
        assert_eq!(start.ty, expected);
    }
    let before = store.checkpoint();
    for symbol in [
        a.classes()[0].methods().next().unwrap().symbol,
        omitted.classes()[0].symbol,
    ] {
        assert!(
            matches!(sources.complete_constructor_signature(symbol, &mut store, defs, &packages),
            Err(ClassHeaderError::InvalidClass(id)) if id == symbol)
        );
        assert_eq!(store.checkpoint(), before);
    }
    assert_eq!(
        store.symbols.info(omitted.classes()[0].constructor),
        &SymbolInfo::Missing
    );
}

#[test]
fn constructor_and_header_failures_retain_their_distinct_context_after_analysis() {
    let broken_text = "class Broken(start: Missing)";
    let bad_text = "class Bad(start: Boolean) extends Base(start)";
    let (ctor, header) = {
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let broken = named(7, broken_text, &mut store, &mut packages);
        let bad = named(21, bad_text, &mut store, &mut packages);
        let base = named(42, "class Base(start: Int)", &mut store, &mut packages);
        let sources = NamedClassSources::new([&broken, &bad, &base], &store).unwrap();
        let class = &broken.classes()[0];
        let before = store.checkpoint();
        let Err(ClassHeaderError::Typing(ctor)) =
            sources.complete_constructor_signature(class.symbol, &mut store, defs, &packages)
        else {
            panic!("expected an unknown constructor type")
        };
        assert_eq!(store.checkpoint(), before);
        assert_eq!(
            (ctor.class(), ctor.member()),
            (Some(class.symbol), Some(class.constructor))
        );

        let mut arena = AstArena::new();
        let mut bindings = ClassHeaderBindings::default();
        for file in [&base, &bad] {
            sources
                .complete_constructor_signature(
                    file.classes()[0].symbol,
                    &mut store,
                    defs,
                    &packages,
                )
                .unwrap();
        }
        let before = (store.checkpoint(), arena.checkpoint());
        assert!(
            matches!(sources.try_complete_class_header_reported(class.constructor, &mut store, defs, &packages, &mut arena, &mut bindings), Err(ClassHeaderError::InvalidClass(s)) if s == class.constructor)
        );
        assert_eq!((store.checkpoint(), arena.checkpoint()), before);
        assert!(matches!(
            sources.try_complete_class_header_reported(
                base.classes()[0].symbol,
                &mut store,
                defs,
                &packages,
                &mut arena,
                &mut bindings
            ),
            Ok(SignatureCompletion::Complete(_))
        ));
        let bad_symbol = bad.classes()[0].symbol;
        let Err(ClassHeaderError::Typing(header)) = sources.try_complete_class_header_reported(
            bad_symbol,
            &mut store,
            defs,
            &packages,
            &mut arena,
            &mut bindings,
        ) else {
            panic!("expected a parent argument error")
        };
        assert_eq!((header.class(), header.member()), (Some(bad_symbol), None));
        (ctor, header)
    };
    assert!(matches!(ctor.error(), TyperError::TypeNameNotFound { .. }));
    assert!(matches!(
        header.error(),
        TyperError::ApplicationArgumentTypeMismatch {
            argument_index: 0,
            ..
        }
    ));
    for (report, id, text, token) in [
        (ctor, 7, broken_text, "Missing"),
        (header, 21, bad_text, "start"),
    ] {
        let span = report.location().unwrap();
        assert_eq!(span.source(), SourceId::from_index(id));
        let range = span.span().range();
        assert_eq!(range.start() as usize, text.rfind(token).unwrap());
        assert_eq!(range.len() as usize, token.len());
    }
}

#[test]
fn a_blocked_recursive_attempt_and_a_later_rejection_preserve_header_bindings() {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let derived = named(
        9,
        "class Good(x: Int) extends Mid(x); class Mid(y: Int) extends Base(z = y)",
        &mut store,
        &mut packages,
    );
    let base = named(21, "class Base(z: Int)", &mut store, &mut packages);
    let bad = named(
        7,
        "class Bad(start: Boolean) extends Base(start)",
        &mut store,
        &mut packages,
    );
    let sources = NamedClassSources::new([&derived, &base, &bad], &store).unwrap();
    for (_, class) in sources.classes() {
        sources
            .complete_constructor_signature(class.symbol, &mut store, defs, &packages)
            .unwrap();
    }
    let [good, mid] = derived.classes() else {
        panic!("expected Good and Mid")
    };
    let base_symbol = base.classes()[0].symbol;
    let rejected = &bad.classes()[0];
    let mut bindings = ClassHeaderBindings::default();
    let mut arena = AstArena::new();
    let attempt = |symbol,
                   store: &mut SemanticStore,
                   arena: &mut AstArena<Typed>,
                   bindings: &mut ClassHeaderBindings| {
        sources.try_complete_class_header_reported(symbol, store, defs, &packages, arena, bindings)
    };

    let before = (store.checkpoint(), arena.checkpoint());
    assert_eq!(
        attempt(good.symbol, &mut store, &mut arena, &mut bindings).unwrap(),
        SignatureCompletion::BlockedOn(base_symbol)
    );
    assert_eq!((store.checkpoint(), arena.checkpoint()), before);
    let parent_roots = |bindings: &ClassHeaderBindings| {
        [good, mid].map(|c| bindings.get(derived.source(), c.parents[0]))
    };
    assert_eq!(parent_roots(&bindings), [None, None]);
    for symbol in [base_symbol, good.symbol] {
        assert!(matches!(
            attempt(symbol, &mut store, &mut arena, &mut bindings),
            Ok(SignatureCompletion::Complete(_))
        ));
    }
    let [Some(good_root), Some(mid_root)] = parent_roots(&bindings) else {
        panic!("both headers must retain their parent calls")
    };
    assert_ne!(good_root, mid_root);

    let before = (store.checkpoint(), arena.checkpoint());
    assert!(
        matches!(attempt(rejected.symbol, &mut store, &mut arena, &mut bindings),
        Err(ClassHeaderError::Typing(report)) if matches!(report.error(), TyperError::ApplicationArgumentTypeMismatch { .. }))
    );
    assert_eq!((store.checkpoint(), arena.checkpoint()), before);
    assert_eq!(parent_roots(&bindings), [Some(good_root), Some(mid_root)]);
    assert_eq!(bindings.get(bad.source(), rejected.parents[0]), None);
}

#[test]
fn selected_constructors_can_stop_or_continue_after_a_located_failure() {
    use crate::core::SymbolInfo;
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let text = "class Bad(start: Missing); class Good(x: Int)";
    let file = named(19, text, &mut store, &mut packages);
    let sources = NamedClassSources::new([&file], &store).unwrap();
    let [bad, good] = file.classes() else {
        panic!()
    };
    let roots = [bad.symbol, good.symbol];
    let failure = sources
        .prepare_class_constructors(roots, &mut store, defs, &packages)
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();
    assert_eq!(
        (failure.source, failure.class),
        (Some(file.source()), bad.symbol)
    );
    let ClassHeaderError::Typing(error) = failure.error else {
        panic!()
    };
    assert!(matches!(error.error(), TyperError::TypeNameNotFound { .. }));
    let range = error.location().unwrap().span().range();
    assert_eq!(
        &text[range.start() as usize..range.end() as usize],
        "Missing"
    );
    assert_eq!(store.symbols.info(good.constructor), &SymbolInfo::Missing);
    let results: Vec<_> = sources
        .prepare_class_constructors(
            [good.constructor, bad.symbol, good.symbol],
            &mut store,
            defs,
            &packages,
        )
        .collect();
    assert!(
        matches!(&results[0], Err(e) if e.source.is_none() && matches!(e.error,ClassHeaderError::InvalidClass(s) if s == good.constructor))
    );
    assert!(matches!(&results[1], Err(e) if e.class == bad.symbol));
    assert!(matches!(results[2], Ok(s) if s == good.symbol));
    assert!(matches!(
        store.symbols.info(good.constructor),
        SymbolInfo::Complete(_)
    ));
}

#[test]
fn completing_a_shared_base_wakes_both_siblings_once_in_observed_order() {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let siblings = named(
        4,
        "class A(x: Int) extends Base(x); class B(y: Int) extends Base(y)",
        &mut store,
        &mut packages,
    );
    let base = named(7, "class Base(x: Int)", &mut store, &mut packages);
    let sources = NamedClassSources::new([&siblings, &base], &store).unwrap();
    let [a, b] = siblings.classes() else { panic!() };
    let base = base.classes()[0].symbol;
    let mut arena = AstArena::new();
    let mut bindings = ClassHeaderBindings::default();
    let mut phase = crate::ClassHeaderPhase::prepare(
        &sources,
        &mut store,
        defs,
        &packages,
        &mut arena,
        &mut bindings,
    );
    for class in [b, a, b] {
        let attempt = phase.attempt(class.symbol).unwrap();
        assert_eq!(attempt.completion, SignatureCompletion::BlockedOn(base));
        assert!(attempt.retry.is_empty());
    }
    assert_eq!(phase.attempt(base).unwrap().retry, [b.symbol, a.symbol]);
    assert!(phase.attempt(base).unwrap().retry.is_empty());
    for class in [a, b] {
        assert!(matches!(
            phase.attempt(class.symbol).unwrap().completion,
            SignatureCompletion::Complete(_)
        ));
        assert!(phase.is_ready(class.symbol));
    }
    drop(phase);
    for class in [a, b] {
        let target = bindings.get(siblings.source(), class.parents[0]).unwrap();
        assert_eq!(
            arena.get(target).position.unwrap().source(),
            siblings.source()
        );
    }
}

#[test]
fn a_recursive_same_file_completion_releases_an_external_waiter() {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let external = named(
        4,
        "class External(x: Int) extends Mid(x)",
        &mut store,
        &mut packages,
    );
    let pair = named(
        9,
        "class Leaf(x: Int) extends Mid(x); class Mid(x: Int)",
        &mut store,
        &mut packages,
    );
    let sources = NamedClassSources::new([&external, &pair], &store).unwrap();
    let mut arena = AstArena::new();
    let mut bindings = ClassHeaderBindings::default();
    let mut phase = crate::ClassHeaderPhase::prepare(
        &sources,
        &mut store,
        defs,
        &packages,
        &mut arena,
        &mut bindings,
    );
    let external = external.classes()[0].symbol;
    let [leaf, mid] = pair.classes() else {
        panic!()
    };
    assert_eq!(
        phase.attempt(external).unwrap().completion,
        SignatureCompletion::BlockedOn(mid.symbol)
    );
    assert_eq!(phase.attempt(leaf.symbol).unwrap().retry, [external]);
    assert!(phase.is_ready(mid.symbol));
    assert!(matches!(
        phase.attempt(external).unwrap().completion,
        SignatureCompletion::Complete(_)
    ));
    assert!(phase.finish().blockages.is_empty());
}

#[test]
fn header_report_distinguishes_cycles_unvisited_and_failed_providers() {
    use crate::SignatureBlockage;
    use std::sync::Arc;
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let texts = [
        "class A(x: Int) extends B(x)",
        "class B(x: Int) extends A(x)",
        "class C(x: Int) extends A(x)",
        "class Good",
        "class Bad(x: Missing)",
        "class Child(x: Int) extends Bad(x)",
        "class New(x: Int)",
        "class Waiting(x: Int) extends New(x)",
    ];
    let files: Vec<_> = texts
        .iter()
        .enumerate()
        .map(|(i, t)| named(i as u32, t, &mut store, &mut packages))
        .collect();
    let ids: Vec<_> = files.iter().map(|f| f.classes()[0].symbol).collect();
    let sources = NamedClassSources::new(&files, &store).unwrap();
    let mut arena = AstArena::new();
    let mut bindings = ClassHeaderBindings::default();
    let mut phase = crate::ClassHeaderPhase::prepare(
        &sources,
        &mut store,
        defs,
        &packages,
        &mut arena,
        &mut bindings,
    );
    for i in [2, 0, 1, 3, 5, 7] {
        let _ = phase.attempt(ids[i]).unwrap();
    }
    assert!(phase.attempt(ids[4]).is_err());
    let result = phase.finish();
    assert_eq!(result.ready, [ids[3]]);
    assert_eq!(result.remaining, [ids[6]]);
    let [failure] = result.failures.as_slice() else {
        panic!()
    };
    assert_eq!(failure.class, ids[4]);
    assert!(
        matches!(&failure.error, ClassHeaderError::Typing(e) if matches!(e.error(),TyperError::TypeNameNotFound { .. }))
    );
    let causes = result.blockages;
    assert_eq!(
        causes.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        [ids[0], ids[1], ids[2], ids[5], ids[7]]
    );
    let (
        SignatureBlockage::CycleMember(a),
        SignatureBlockage::CycleMember(b),
        SignatureBlockage::DependsOnCycle(c),
    ) = (&causes[0].1, &causes[1].1, &causes[2].1)
    else {
        panic!()
    };
    assert!(Arc::ptr_eq(a, b) && Arc::ptr_eq(a, c));
    assert_eq!(a.path(), [ids[0], ids[1], ids[0]]);
    assert_eq!(causes[3].1, SignatureBlockage::Failed(ids[4]));
    assert_eq!(causes[4].1, SignatureBlockage::Pending(ids[6]));
}

#[test]
fn fail_fast_phase_stops_before_the_next_constructor() {
    use crate::core::SymbolInfo;
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let file = named(
        5,
        "class First; class Bad(x: Missing); class Last",
        &mut store,
        &mut packages,
    );
    let sources = NamedClassSources::new([&file], &store).unwrap();
    let [first, bad, last] = file.classes() else {
        panic!()
    };
    let mut arena = AstArena::new();
    let mut bindings = ClassHeaderBindings::default();
    let error = crate::ClassHeaderPhase::try_prepare(
        &sources,
        &mut store,
        defs,
        &packages,
        &mut arena,
        &mut bindings,
    )
    .unwrap_err();
    assert_eq!(error.class, bad.symbol);
    assert!(matches!(
        store.symbols.info(first.constructor),
        SymbolInfo::Complete(_)
    ));
    assert_eq!(store.symbols.info(last.constructor), &SymbolInfo::Missing);
}

#[test]
fn direct_phase_reports_capture_constructor_and_parent_argument_sites() {
    use crate::{ClassHeaderPhase, SemanticIssue, SemanticPhase};
    for reverse in [false, true] {
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let ctor_text = "class Bad(x: Missing)";
        let child_text = "class Child(start: Boolean) extends Base(start)";
        let bad = named(8, ctor_text, &mut store, &mut packages);
        let child = named(3, child_text, &mut store, &mut packages);
        let base = named(21, "class Base(start: Int)", &mut store, &mut packages);
        let sources = NamedClassSources::new(
            if reverse {
                [&base, &child, &bad]
            } else {
                [&bad, &child, &base]
            },
            &store,
        )
        .unwrap();
        let mut arena = AstArena::new();
        let mut bindings = ClassHeaderBindings::default();
        let failure = ClassHeaderPhase::try_prepare(
            &sources,
            &mut store,
            defs,
            &packages,
            &mut arena,
            &mut bindings,
        )
        .unwrap_err();
        let d = failure.into_semantic_diagnostic();
        assert_eq!(
            (d.phase, d.class, d.member(), d.owner_source),
            (
                SemanticPhase::Constructor,
                Some(bad.classes()[0].symbol),
                Some(bad.classes()[0].constructor),
                Some(bad.source())
            )
        );
        let r = d.location().unwrap().span().range();
        assert_eq!(&ctor_text[r.start() as usize..r.end() as usize], "Missing");
        let mut phase = ClassHeaderPhase::prepare(
            &sources,
            &mut store,
            defs,
            &packages,
            &mut arena,
            &mut bindings,
        );
        let mut queue: std::collections::VecDeque<_> =
            sources.classes().map(|(_, c)| c.symbol).collect();
        while let Some(id) = queue.pop_front() {
            if let Ok(attempt) = phase.attempt(id) {
                queue.extend(attempt.retry);
            }
        }
        let reports: Vec<_> = phase
            .finish()
            .failures
            .into_iter()
            .map(|f| f.into_semantic_diagnostic())
            .collect();
        assert_eq!(reports.len(), 2);
        let d = &reports[1];
        assert_eq!(
            (d.phase, d.class, d.member(), d.source()),
            (
                SemanticPhase::ClassHeader,
                Some(child.classes()[0].symbol),
                None,
                Some(child.source())
            )
        );
        assert!(
            matches!(&d.issue, SemanticIssue::Typing(e) if matches!(e.error(), TyperError::ApplicationArgumentTypeMismatch{..}))
        );
        let r = d.location().unwrap().span().range();
        assert_eq!(r.start() as usize, child_text.rfind("start").unwrap());
        assert_eq!(r.len(), 5);
    }
}

#[test]
fn cycle_report_groups_disjoint_cycles_even_when_the_first_root_is_a_dependent() {
    let mut store = SemanticStore::new();
    let mut packages = Packages::new();
    let unit = named(
        3,
        "class Leaf; class A; class B; class Solo",
        &mut store,
        &mut packages,
    );
    let sources = NamedClassSources::new([&unit], &store).unwrap();
    let [leaf, a, b, solo] = unit.classes() else {
        panic!()
    };
    for phase in [
        crate::SignaturePhase::Headers,
        crate::SignaturePhase::Members,
    ] {
        let mut graph = crate::SignatureDependencies::new(phase);
        for (root, provider) in [(leaf, a), (a, b), (b, a), (solo, solo)] {
            graph.record(root.symbol, SignatureCompletion::BlockedOn(provider.symbol));
        }
        let blockages = graph.blockages([leaf.symbol, solo.symbol, b.symbol, a.symbol]);
        let report = match phase {
            crate::SignaturePhase::Headers => crate::ClassHeaderReport {
                ready: vec![],
                failures: vec![],
                remaining: vec![],
                blockages,
            }
            .blockage_report(sources.declarations(), &store),
            crate::SignaturePhase::Members => crate::MemberSignatureReport {
                ready: vec![],
                failures: vec![],
                remaining: vec![],
                blockages,
            }
            .blockage_report(sources.declarations(), &store),
        };
        let mut cycles = report.cycles();
        assert_eq!(cycles.len(), 2);
        let (cycle, sites) = cycles.next().unwrap();
        assert_eq!(cycle.phase(), phase);
        assert_eq!(cycle.path(), [a.symbol, b.symbol, a.symbol]);
        assert_eq!(
            sites
                .map(|s| s.name.as_deref().unwrap())
                .collect::<Vec<_>>(),
            ["A", "B"]
        );
        let (cycle, sites) = cycles.next().unwrap();
        assert_eq!(cycle.path(), [solo.symbol, solo.symbol]);
        assert_eq!(sites.map(|s| s.symbol).collect::<Vec<_>>(), [solo.symbol]);
        assert_eq!(
            cycle.code(),
            match phase {
                crate::SignaturePhase::Headers => "signature.header_cycle",
                crate::SignaturePhase::Members => "signature.member_cycle",
            }
        );
    }
}

#[test]
fn header_root_order_controls_attempts_and_skips_repeated_ready_roots() {
    for base_first in [false, true] {
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let leaf = named(
            8,
            "class Leaf(x: Int) extends Mid(x)",
            &mut store,
            &mut packages,
        );
        let mid = named(
            2,
            "class Mid(x: Int) extends Base(x)",
            &mut store,
            &mut packages,
        );
        let base = named(6, "class Base(x: Int)", &mut store, &mut packages);
        let sources = NamedClassSources::new([&leaf, &mid, &base], &store).unwrap();
        let [leaf, mid, base] = [&leaf, &mid, &base].map(|f| f.classes()[0].symbol);
        let roots = if base_first {
            [base, mid, leaf, leaf]
        } else {
            [leaf, mid, base, leaf]
        };
        let mut arena = Default::default();
        let mut bindings = Default::default();
        let phase = crate::ClassHeaderPhase::try_prepare(
            &sources,
            &mut store,
            defs,
            &packages,
            &mut arena,
            &mut bindings,
        )
        .unwrap();
        let mut seen = Vec::new();
        let report = phase
            .try_complete_roots(roots, |symbol, completion| {
                seen.push((
                    symbol,
                    match completion {
                        SignatureCompletion::Complete(_) => None,
                        SignatureCompletion::BlockedOn(provider) => Some(provider),
                    },
                ))
            })
            .unwrap()
            .finish();
        if base_first {
            assert_eq!(seen, [(base, None), (mid, None), (leaf, None)]);
        } else {
            assert_eq!(
                seen,
                [
                    (leaf, Some(mid)),
                    (mid, Some(base)),
                    (base, None),
                    (mid, None),
                    (leaf, None)
                ]
            );
        }
        assert_eq!(report.ready, [leaf, mid, base]);
        assert!(
            report.failures.is_empty()
                && report.remaining.is_empty()
                && report.blockages.is_empty()
        );
    }
}

#[test]
fn named_parent_failures_point_to_the_written_label_or_value_and_preserve_other_headers() {
    for (args, token) in [
        ("unknown = id, count = 1", "unknown"),
        ("count = 1, count = 2", "count"),
        ("count = initial, id = id", "initial"),
    ] {
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let base = named(
            3,
            "class Base(val id: Int, var count: Int)",
            &mut store,
            &mut packages,
        );
        let text = format!(
            "class Bad(id: Int, initial: Boolean) extends Base({args}); class Good(x: Int) extends Base(count = x, id = x)"
        );
        let children = named(7, &text, &mut store, &mut packages);
        let sources = NamedClassSources::new([&base, &children], &store).unwrap();
        let (mut arena, mut bindings) = (AstArena::new(), ClassHeaderBindings::default());
        let mut phase = crate::ClassHeaderPhase::prepare(
            &sources,
            &mut store,
            defs,
            &packages,
            &mut arena,
            &mut bindings,
        );
        for (_, class) in sources.classes() {
            assert_eq!(
                phase.attempt(class.symbol).is_err(),
                class.symbol == children.classes()[0].symbol
            );
        }
        let report = phase.finish();
        let [bad, good] = children.classes() else {
            panic!()
        };
        assert!(report.ready.contains(&good.symbol));
        assert!(bindings.get(children.source(), good.parents[0]).is_some());
        assert!(bindings.get(children.source(), bad.parents[0]).is_none());
        let [failure] = report.failures.as_slice() else {
            panic!("{report:?}")
        };
        let ClassHeaderError::Typing(error) = &failure.error else {
            panic!()
        };
        assert_eq!(failure.class, bad.symbol);
        match token {
            "unknown" => assert!(matches!(
                error.error(),
                TyperError::UnknownNamedArgument { .. }
            )),
            "count" => assert!(matches!(
                error.error(),
                TyperError::DuplicateNamedArgument { .. }
            )),
            _ => assert!(
                matches!(error.error(), TyperError::ApplicationArgumentTypeMismatch { argument_index: 0, expected, .. } if *expected == defs.int)
            ),
        }
        let span = error.location().unwrap();
        let start = text.find(args).unwrap() + args.rfind(token).unwrap();
        assert_eq!(span.source(), children.source());
        assert_eq!(span.span().range().start() as usize, start);
        assert_eq!(span.span().range().len() as usize, token.len());
    }
}
