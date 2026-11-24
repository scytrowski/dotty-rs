use super::*;
use crate::{ClassHeaderPhase, MemberSelectionError};

fn headers(
    sources: &NamedClassSources<'_>,
    store: &mut SemanticStore,
    defs: Definitions,
    packages: &Packages,
) {
    let mut arena = AstArena::new();
    let mut bindings = ClassHeaderBindings::default();
    let mut phase =
        ClassHeaderPhase::try_prepare(sources, store, defs, packages, &mut arena, &mut bindings)
            .unwrap();
    for (_, c) in sources.classes() {
        assert!(matches!(
            phase.attempt(c.symbol).unwrap().completion,
            SignatureCompletion::Complete(_)
        ));
    }
}

#[test]
fn roots_preserve_parameter_and_member_order_for_selected_classes() {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let file = named(
        7,
        "class A(val start: Int, var count: Int) { private val hidden = 1; def read() = 7; val answer = 40 }; class B { val other = 2 }",
        &mut store,
        &mut packages,
    );
    let sources = NamedClassSources::new([&file], &store).unwrap();
    let [a, b] = file.classes() else { panic!() };
    assert!(
        matches!(sources.member_signatures([a.symbol],&store),Err(MemberSelectionError::HeaderUnavailable(id)) if id == a.symbol)
    );
    headers(&sources, &mut store, defs, &packages);
    let [start, count] = a.constructor_fields[..] else {
        panic!()
    };
    let mut fields = a.fields();
    let hidden = fields.next().unwrap().symbol;
    let answer = fields.next().unwrap().symbol;
    let read = a.methods().next().unwrap().symbol;
    let other = b.fields().next().unwrap().symbol;
    let selection = sources
        .member_signatures([b.symbol, a.symbol, b.symbol], &store)
        .unwrap();
    assert_eq!(
        selection.members(),
        [other, start, count, hidden, read, answer]
    );
    assert_eq!(selection.field_roots(), [other, start, count, answer]);
    assert!(selection.contains(hidden));
    assert!(!selection.contains(a.constructor));
    assert!(
        !sources
            .member_signatures([a.symbol], &store)
            .unwrap()
            .contains(other)
    );
    assert!(
        matches!(sources.member_signatures([a.constructor],&store),Err(MemberSelectionError::InvalidClass(id)) if id == a.constructor)
    );
}

#[test]
fn excluded_provider_retains_its_error_and_selected_provider_uses_its_symbol() {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let a = named(9, "class A { val seed = 40 }", &mut store, &mut packages);
    let b = named(2, "class B { val seed = 7 }", &mut store, &mut packages);
    let consumer = named(
        4,
        "class Consumer(a: A) { val answer = a.seed }",
        &mut store,
        &mut packages,
    );
    let sources = NamedClassSources::new([&consumer, &b, &a], &store).unwrap();
    headers(&sources, &mut store, defs, &packages);
    let c = &consumer.classes()[0];
    let answer = c.fields().next().unwrap().symbol;
    let seed = a.classes()[0].fields().next().unwrap().symbol;
    let decoy = b.classes()[0].fields().next().unwrap().symbol;
    let partial = sources
        .member_signatures([c.symbol, b.classes()[0].symbol], &store)
        .unwrap();
    assert_eq!(
        partial
            .try_complete(decoy, &mut store, defs, &packages)
            .unwrap(),
        SignatureCompletion::Complete(defs.int)
    );
    let before = store.checkpoint();
    assert!(
        matches!(partial.try_complete(answer,&mut store,defs,&packages),Err(MemberSignatureError::Typing {member,error: TyperError::MemberTypeUnavailable { symbol }, ..}) if member == answer && symbol == seed)
    );
    assert!(
        matches!(partial.try_complete(seed,&mut store,defs,&packages),Err(MemberSignatureError::InvalidMember(id)) if id == seed)
    );
    assert_eq!(store.checkpoint(), before);
    let selection = sources
        .member_signatures([c.symbol, a.classes()[0].symbol], &store)
        .unwrap();
    assert_eq!(
        selection
            .try_complete(answer, &mut store, defs, &packages)
            .unwrap(),
        SignatureCompletion::BlockedOn(seed)
    );
    assert_eq!(store.checkpoint(), before);
    for id in [seed, answer] {
        assert_eq!(
            selection
                .try_complete(id, &mut store, defs, &packages)
                .unwrap(),
            SignatureCompletion::Complete(defs.int)
        );
    }
}

#[test]
fn shared_field_releases_waiters_once_and_a_rejected_sibling_keeps_its_range() {
    use crate::{MemberSignatureFailure, MemberSignaturePhase};
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let text = "class C(p: P) { val x = p.seed; val y = p.seed; def value(): Int = 7; val bad = value(1); def later() = 2 }";
    let c = named(17, text, &mut store, &mut packages);
    let p = named(
        4,
        "class P { val seed = 40; val alias = seed }",
        &mut store,
        &mut packages,
    );
    let sources = NamedClassSources::new([&c, &p], &store).unwrap();
    headers(&sources, &mut store, defs, &packages);
    let cls = &c.classes()[0];
    let fields: Vec<_> = cls.fields().map(|d| d.symbol).collect();
    let [x, y, bad] = fields[..] else { panic!() };
    let [seed, alias] = p.classes()[0]
        .fields()
        .map(|d| d.symbol)
        .collect::<Vec<_>>()[..]
    else {
        panic!()
    };
    let later = cls.methods().last().unwrap().symbol;
    let selection = sources
        .member_signatures([cls.symbol, p.classes()[0].symbol], &store)
        .unwrap();
    let mut phase = MemberSignaturePhase::new(&selection, &mut store, defs, &packages);
    for id in [y, x, y] {
        assert_eq!(
            phase.attempt(id).unwrap().completion,
            SignatureCompletion::BlockedOn(seed)
        );
    }
    // Completing alias recursively commits seed in the same file.
    assert_eq!(phase.attempt(alias).unwrap().retry, [y, x]);
    assert!(phase.attempt(seed).unwrap().retry.is_empty());
    for id in [x, y] {
        assert_eq!(
            phase.attempt(id).unwrap().completion,
            SignatureCompletion::Complete(defs.int)
        );
    }
    for _ in 0..2 {
        assert!(phase.attempt(bad).is_err());
    }
    let report = phase.finish();
    assert!(report.ready.contains(&x) && report.ready.contains(&y));
    assert!(report.remaining.contains(&later));
    assert!(report.blockages.is_empty());
    let mut report_failures = report.failures;
    let [
        MemberSignatureFailure::Typing {
            declaration,
            report,
        },
    ] = report_failures.as_slice()
    else {
        panic!()
    };
    assert_eq!(
        (declaration.source, declaration.class, report.member()),
        (c.source(), cls.symbol, Some(bad))
    );
    assert!(matches!(
        report.error(),
        TyperError::ApplicationArityMismatch {
            expected: 0,
            actual: 1,
            ..
        }
    ));
    let r = report.location().unwrap().span().range();
    assert_eq!(&text[r.start() as usize..r.end() as usize], "value(1)");
    let direct = selection
        .try_complete(bad, &mut store, defs, &packages)
        .unwrap_err()
        .into_semantic_diagnostic(&sources);
    let captured = report_failures
        .pop()
        .unwrap()
        .into_semantic_diagnostic(&sources);
    for d in [direct, captured] {
        assert_eq!(
            (d.phase, d.owner_source, d.class, d.member()),
            (
                crate::SemanticPhase::MemberSignature,
                Some(c.source()),
                Some(cls.symbol),
                Some(bad)
            )
        );
        assert_eq!(d.location().unwrap().span().range(), r);
        assert!(
            matches!(d.issue,crate::SemanticIssue::Typing(e) if matches!(e.error(),TyperError::ApplicationArityMismatch{expected:0,actual:1,..}))
        );
    }
    let foreign = named(31, "class Other { def x() = 1 }", &mut store, &mut packages).classes()[0]
        .methods()
        .next()
        .unwrap()
        .symbol;
    let rejected = selection
        .try_complete(foreign, &mut store, defs, &packages)
        .unwrap_err()
        .into_semantic_diagnostic(&sources);
    assert_eq!(
        (
            rejected.source(),
            rejected.location(),
            rejected.class,
            rejected.member()
        ),
        (None, None, None, Some(foreign))
    );
    assert!(matches!(rejected.issue, crate::SemanticIssue::InvalidMember(id) if id == foreign));
}

#[test]
fn terminal_member_causes_distinguish_cycle_dependents_failure_and_pending() {
    use crate::{MemberSignaturePhase, SignatureBlockage};
    use std::sync::Arc;
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let texts = [
        "class A(b: B) { val x = b.y }",
        "class B(a: A) { val y = a.x }",
        "class C(a: A) { val z = a.x }",
        "class D { def value(): Int = 1; val bad = value(1); val later = 7 }",
        "class E(d: D) { val bad = d.bad; val pending = d.later }",
    ];
    let files: Vec<_> = texts
        .iter()
        .enumerate()
        .map(|(i, t)| named(i as u32, t, &mut store, &mut packages))
        .collect();
    let sources = NamedClassSources::new(&files, &store).unwrap();
    headers(&sources, &mut store, defs, &packages);
    let field =
        |file: usize, index: usize| files[file].classes()[0].fields().nth(index).unwrap().symbol;
    let (x, y, z, bad, later, dependent, pending) = (
        field(0, 0),
        field(1, 0),
        field(2, 0),
        field(3, 0),
        field(3, 1),
        field(4, 0),
        field(4, 1),
    );
    let selection = sources
        .member_signatures(sources.classes().map(|(_, c)| c.symbol), &store)
        .unwrap();
    let mut phase = MemberSignaturePhase::new(&selection, &mut store, defs, &packages);
    for id in [z, y, x, dependent, pending] {
        assert!(matches!(
            phase.attempt(id).unwrap().completion,
            SignatureCompletion::BlockedOn(_)
        ));
    }
    assert!(phase.attempt(bad).is_err());
    let report = phase.finish();
    assert!(report.remaining.contains(&later));
    let causes = report.blockages;
    assert_eq!(
        causes.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        [x, y, z, dependent, pending]
    );
    let (
        SignatureBlockage::CycleMember(a),
        SignatureBlockage::CycleMember(b),
        SignatureBlockage::DependsOnCycle(c),
    ) = (&causes[0].1, &causes[1].1, &causes[2].1)
    else {
        panic!()
    };
    assert!(Arc::ptr_eq(a, b) && Arc::ptr_eq(b, c));
    assert_eq!(a.code(), "signature.member_cycle");
    assert_eq!(causes[3].1, SignatureBlockage::Failed(bad));
    assert_eq!(causes[4].1, SignatureBlockage::Pending(later));
}

#[test]
fn signature_rejection_and_body_rejection_have_distinct_owners_and_ranges() {
    use crate::{MemberSignatureFailure, MemberSignaturePhase};
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let text = "class C { def value(): Int = 7; val broken = value(1); def wrong(): Int = false; def good(): Int = value() }";
    let file = named(13, text, &mut store, &mut packages);
    let sources = NamedClassSources::new([&file], &store).unwrap();
    headers(&sources, &mut store, defs, &packages);
    let class = &file.classes()[0];
    let broken = class.fields().next().unwrap();
    let wrong = class.methods().nth(1).unwrap();
    let good = class.methods().nth(2).unwrap();
    let selection = sources.member_signatures([class.symbol], &store).unwrap();
    let mut phase = MemberSignaturePhase::new(&selection, &mut store, defs, &packages);
    for &member in selection.members() {
        let _ = phase.attempt(member);
    }
    let (signatures, work) = phase.finish_with_body_work();
    assert!(signatures.ready.contains(&wrong.symbol));
    let [MemberSignatureFailure::Typing { report, .. }] = signatures.failures.as_slice() else {
        panic!()
    };
    let site = |report: &crate::ReportedTyperError| {
        let span = report.location().unwrap();
        assert_eq!(span.source(), file.source());
        let r = span.span().range();
        &text[r.start() as usize..r.end() as usize]
    };
    assert_eq!(
        (report.member(), site(report)),
        (Some(broken.symbol), "value(1)")
    );
    let mut arena = AstArena::new();
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
    let reports: Vec<_> = report
        .errors
        .into_iter()
        .map(|e| e.into_reported_error(|_| Some(file.ast())).unwrap())
        .collect();
    let [retry, body_error] = reports.as_slice() else {
        panic!()
    };
    assert_eq!(
        (retry.member(), site(retry)),
        (Some(broken.symbol), "value(1)")
    );
    assert_eq!(
        (body_error.member(), site(body_error)),
        (Some(wrong.symbol), "false")
    );
    assert!(matches!(
        body_error.error(),
        TyperError::ExpectedExpressionTypeMismatch { .. }
    ));
    let calls = file.navigation().unwrap();
    let target = calls
        .definition_at_with_bindings(
            text.rfind("value()").unwrap() as u32,
            file.ast(),
            &arena,
            &store,
            |_| body.bindings(),
            |_| Some(calls),
        )
        .unwrap();
    assert_eq!(target.name.start() as usize, text.find("value()").unwrap());
    assert!(
        body.bindings()
            .unwrap()
            .get(file.source(), good.tree)
            .is_some()
    );
    let mut checked_arena = AstArena::new();
    let (checked, report) = work.check(
        class.symbol,
        &mut store,
        defs,
        &packages,
        &mut checked_arena,
        crate::core::NoResolver,
    );
    let [error] = report
        .errors
        .try_into()
        .unwrap_or_else(|_: Vec<_>| panic!("only wrong has a body error"));
    let error = error.into_reported_error(|_| Some(file.ast())).unwrap();
    assert_eq!(
        (error.member(), site(&error)),
        (Some(wrong.symbol), "false")
    );
    let target = calls
        .definition_at_with_bindings(
            text.rfind("value()").unwrap() as u32,
            file.ast(),
            &checked_arena,
            &store,
            |_| checked.bindings(),
            |_| Some(calls),
        )
        .unwrap();
    assert_eq!(target.name.start() as usize, text.find("value()").unwrap());
}

#[test]
fn body_work_preserves_owner_order_and_all_four_signature_states() {
    use crate::{BodySkipReason, MemberSignaturePhase, SignatureBlockage};
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let consumer = named(
        3,
        "class A { val seed = 1; def value(): Int = 7; val bad = value(1); def read(b: B) = b.seed; val later = 2 }; class Empty; class Omitted",
        &mut store,
        &mut packages,
    );
    let provider = named(8, "class B { val seed = 40 }", &mut store, &mut packages);
    let sources = NamedClassSources::new([&provider, &consumer], &store).unwrap();
    headers(&sources, &mut store, defs, &packages);
    let [a, empty, omitted] = consumer.classes() else {
        panic!()
    };
    let b = &provider.classes()[0];
    let other = b.fields().next().unwrap().symbol;
    let ids: Vec<_> = a.members.iter().map(|m| m.symbol()).collect();
    let selection = sources
        .member_signatures([a.symbol, b.symbol, empty.symbol], &store)
        .unwrap();
    let mut phase = MemberSignaturePhase::new(&selection, &mut store, defs, &packages);
    for &id in &ids[..4] {
        let _ = phase.attempt(id);
    }
    let (report, work) = phase.finish_with_body_work();
    drop(report);
    let waiting = BodySkipReason::Blocked(SignatureBlockage::Pending(other));
    assert_eq!(
        work.members(a.symbol).unwrap().collect::<Vec<_>>(),
        [
            (ids[0], None),
            (ids[1], None),
            (ids[2], Some(&BodySkipReason::Rejected)),
            (ids[3], Some(&waiting)),
            (ids[4], Some(&BodySkipReason::Remaining))
        ]
    );
    assert_eq!(
        work.members(b.symbol).unwrap().collect::<Vec<_>>(),
        [(other, Some(&BodySkipReason::Remaining))]
    );
    assert_eq!(work.members(empty.symbol).unwrap().count(), 0);
    assert!(work.members(omitted.symbol).is_none());
}

#[test]
fn skipped_only_body_is_partial_and_preserves_committed_warning_order() {
    use crate::core::SymbolInfo;
    use crate::{BodySkipReason, CheckedClassBody, MemberSignaturePhase};
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let text = "class A { def f(implicit i: Int): Int = i; def first(): Int = f(1); private val later = f(2); def last(): Int = f(3) }; class Empty";
    let file = named(4, text, &mut store, &mut packages);
    let sources = NamedClassSources::new([&file], &store).unwrap();
    headers(&sources, &mut store, defs, &packages);
    let [class, empty] = file.classes() else {
        panic!()
    };
    let later = class.fields().next().unwrap();
    let selection = sources
        .member_signatures([class.symbol, empty.symbol], &store)
        .unwrap();
    let mut phase = MemberSignaturePhase::new(&selection, &mut store, defs, &packages);
    for method in class.methods() {
        let _ = phase.attempt(method.symbol).unwrap();
    }
    let (signatures, work) = phase.finish_with_body_work();
    assert_eq!(signatures.remaining, [later.symbol]);
    let mut arena = AstArena::new();
    let (body, report) = work.check(
        class.symbol,
        &mut store,
        defs,
        &packages,
        &mut arena,
        crate::core::NoResolver,
    );
    let CheckedClassBody::Partial(partial) = body else {
        panic!("skipped field forbids planning")
    };
    let bindings = partial.bindings();
    assert!(report.errors.is_empty());
    assert_eq!(store.symbols.info(later.symbol), &SymbolInfo::Missing);
    assert!(bindings.get(file.source(), later.tree).is_none());
    for method in class.methods() {
        assert!(bindings.get(file.source(), method.tree).is_some());
    }
    let warnings: Vec<_> = report
        .diagnostics
        .iter()
        .map(|(source, d)| {
            assert_eq!(*source, file.source());
            assert_eq!(
                d.issue(),
                &crate::typer::TyperWarning::ImplicitClauseWithoutUsing
            );
            let r = d.span();
            &text[r.start() as usize..r.end() as usize]
        })
        .collect();
    assert_eq!(warnings, ["1", "3"]);
    assert_eq!(
        work.members(class.symbol)
            .unwrap()
            .find(|(id, _)| *id == later.symbol)
            .unwrap()
            .1,
        Some(&BodySkipReason::Remaining)
    );
    let (body, report) = work.check(
        empty.symbol,
        &mut store,
        defs,
        &packages,
        &mut arena,
        crate::core::NoResolver,
    );
    assert!(matches!(body, CheckedClassBody::Complete(_)));
    assert!(report.errors.is_empty() && report.diagnostics.is_empty());
}

#[test]
fn blocked_body_creates_no_trees_and_unselected_class_is_invalid() {
    use crate::{CheckedClassBody, MemberSignaturePhase};
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let file = named(
        7,
        "class C { def read(p: P) = p.seed }; class Other",
        &mut store,
        &mut packages,
    );
    let provider = named(6, "class P { val seed = 40 }", &mut store, &mut packages);
    let sources = NamedClassSources::new([&file, &provider], &store).unwrap();
    headers(&sources, &mut store, defs, &packages);
    let [c, other] = file.classes() else { panic!() };
    let p = &provider.classes()[0];
    let selection = sources
        .member_signatures([c.symbol, p.symbol], &store)
        .unwrap();
    let mut phase = MemberSignaturePhase::new(&selection, &mut store, defs, &packages);
    let method = c.methods().next().unwrap();
    assert_eq!(
        phase.attempt(method.symbol).unwrap().completion,
        SignatureCompletion::BlockedOn(p.fields().next().unwrap().symbol)
    );
    let (_, work) = phase.finish_with_body_work();
    let mut arena = AstArena::new();
    let before = (arena.checkpoint(), store.checkpoint());
    let (body, report) = work.check(
        c.symbol,
        &mut store,
        defs,
        &packages,
        &mut arena,
        crate::core::NoResolver,
    );
    assert!(matches!(body,CheckedClassBody::Partial(body) if body.bindings().is_empty()));
    assert!(report.errors.is_empty() && report.diagnostics.is_empty());
    let (invalid, report) = work.check(
        other.symbol,
        &mut store,
        defs,
        &packages,
        &mut arena,
        crate::core::NoResolver,
    );
    assert!(matches!(invalid, CheckedClassBody::Invalid(_)));
    assert!(
        matches!(report.errors.as_slice(),[crate::ClassBodyError::InvalidClass(id)] if *id==other.symbol)
    );
    assert_eq!((arena.checkpoint(), store.checkpoint()), before);
}

#[test]
fn checked_bodies_keep_their_original_same_file_owners_after_reporting() {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let file = named(
        8,
        "class A { def value(): Int = 40 }; class B { def value(): Int = false }",
        &mut store,
        &mut packages,
    );
    let sources = NamedClassSources::new([&file], &store).unwrap();
    headers(&sources, &mut store, defs, &packages);
    let [a, b] = file.classes() else { panic!() };
    let selection = sources
        .member_signatures([a.symbol, b.symbol], &store)
        .unwrap();
    let mut phase = crate::MemberSignaturePhase::new(&selection, &mut store, defs, &packages);
    for &member in selection.members() {
        let _ = phase.attempt(member).unwrap();
    }
    let (_, work) = phase.finish_with_body_work();
    let mut arena = AstArena::new();
    let mut bodies = crate::CheckedClassBodies::default();
    assert!(bodies.get(b.symbol).is_none());
    for class in [b, a] {
        let (body, report) = work.check(
            class.symbol,
            &mut store,
            defs,
            &packages,
            &mut arena,
            crate::core::NoResolver,
        );
        assert_eq!(report.errors.len(), usize::from(class.symbol == b.symbol));
        drop(report);
        assert_eq!(body.class(), class.symbol);
        assert_eq!(
            matches!(body, crate::CheckedClassBody::Complete(_)),
            class.symbol == a.symbol
        );
        bodies.record(body).unwrap();
    }
    assert_eq!(bodies.complete(a.symbol).unwrap().class(), a.symbol);
    assert!(
        bodies
            .bindings(a.symbol)
            .unwrap()
            .get(file.source(), a.methods().next().unwrap().tree)
            .is_some()
    );
    assert!(matches!(
        bodies.get(a.symbol),
        Some(crate::CheckedClassBody::Complete(_))
    ));
    assert!(matches!(
        bodies.get(b.symbol),
        Some(crate::CheckedClassBody::Partial(_))
    ));
    assert_eq!(
        bodies.record(crate::CheckedClassBody::Invalid(b.symbol)),
        Err(crate::CheckedBodyError::Duplicate(b.symbol))
    );
    assert!(matches!(
        bodies.get(b.symbol),
        Some(crate::CheckedClassBody::Partial(_))
    ));
}
