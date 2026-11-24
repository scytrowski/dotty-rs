use super::*;
use crate::{CheckedClassBodies, FieldAccessKind, selected_field_access};

const PROVIDERS: &str =
    "class A(var count: Int); class B { val count: Int = 2; val unused: Int = 3 }";
const CLIENT: &str = "class C { def copy(a: A, b: B): Unit = a.count = b.count }";

fn with_fields<R>(
    f: impl FnOnce(
        &NamedClassSources<'_>,
        &CheckedClassBodies,
        &AstArena<Typed>,
        &SemanticStore,
        [SymbolId; 2],
    ) -> R,
) -> R {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let provider = named(9, PROVIDERS, &mut store, &mut packages);
    let caller = named(2, CLIENT, &mut store, &mut packages);
    let sources = NamedClassSources::new([&provider, &caller], &store).unwrap();
    let mut arena = prepare_bodies(&sources, &mut store, defs, &packages);
    for &field in &provider.classes()[0].constructor_fields {
        assert!(matches!(
            sources.try_complete_member_signature(field, &mut store, defs, &packages),
            Ok(SignatureCompletion::Complete(_))
        ));
    }
    let mut bodies = CheckedClassBodies::default();
    for (_, class) in sources.classes() {
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
        assert!(report.errors.is_empty(), "{report:?}");
        bodies.record(body).unwrap();
    }
    f(
        &sources,
        &bodies,
        &arena,
        &store,
        [
            provider.classes()[0].constructor_fields[0],
            provider.classes()[1].fields().next().unwrap().symbol,
        ],
    )
}

#[test]
fn committed_assignment_distinguishes_same_named_fields_on_opposite_sides() {
    with_fields(|sources, bodies, arena, store, [a, b]| {
        let caller = sources.file(SourceId::from_index(2)).unwrap();
        let bindings = bodies.bindings(caller.classes()[0].symbol).unwrap();
        let accesses: Vec<_> = caller
            .named()
            .syntax()
            .member_names
            .selections()
            .iter()
            .map(|&s| selected_field_access(s, caller.ast(), bindings, arena, store))
            .collect();
        assert_ne!(a, b);
        assert_eq!(
            accesses,
            [
                Some((a, FieldAccessKind::Write)),
                Some((b, FieldAccessKind::Read))
            ]
        );
    });
}
