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
    let [
        MemberSignatureFailure::Typing {
            declaration,
            report,
        },
    ] = report.failures.as_slice()
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
