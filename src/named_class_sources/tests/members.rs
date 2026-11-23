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
