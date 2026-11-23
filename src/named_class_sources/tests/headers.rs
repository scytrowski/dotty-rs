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
        "class Good(x: Int) extends Mid(x); class Mid(y: Int) extends Base(y)",
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
        assert_eq!(arena.get(target).position.unwrap().source(), siblings.source());
    }
}
