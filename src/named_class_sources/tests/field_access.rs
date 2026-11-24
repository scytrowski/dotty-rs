use super::*;
use crate::{CheckedClassBodies, FieldAccessKind, selected_field_access};

const PROVIDERS: &str = "class A(var count: Int, plain: Int) { private val hidden: Int = 1 }; class B { val count: Int = 2; val unused: Int = 3 }";
const CLIENT: &str =
    "class C { def copy(a: A, b: B): Unit = a.count = b.count; def read(a: A): Int = a.count }";

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
                Some((b, FieldAccessKind::Read)),
                Some((a, FieldAccessKind::Read))
            ]
        );
    });
}

#[test]
fn field_query_selects_the_canonical_constructor_or_body_name() {
    with_fields(|sources, bodies, arena, store, [a, b]| {
        let id = SourceId::from_index(2);
        let provider = sources.file(SourceId::from_index(9)).unwrap();
        let Some(crate::core::SourceDefinition::Canonical { source, tree }) =
            provider.index().definition_of(a)
        else {
            panic!("canonical field")
        };
        let parameter = provider
            .index()
            .derived_symbol_at(provider.classes()[0].constructor, source, tree)
            .unwrap();
        assert_ne!(parameter, a);
        for (offset, field, kind, declaration) in [
            (
                CLIENT.find("count").unwrap(),
                a,
                FieldAccessKind::Write,
                "var count: Int",
            ),
            (
                CLIENT.find("b.count").unwrap() + 2,
                b,
                FieldAccessKind::Read,
                "val count: Int = 2",
            ),
        ] {
            let lookup = |offset| {
                sources
                    .resolved_field_at(id, offset, bodies, arena, store)
                    .unwrap()
            };
            let usage = lookup(offset as u32 + 1).unwrap();
            assert_eq!(
                (usage.field, usage.kind, usage.target.source),
                (field, kind, provider.source())
            );
            assert_eq!(usage.origin.source(), id);
            assert_eq!(usage.origin.span().range().start(), offset as u32);
            let range = usage.target.declaration;
            assert_eq!(
                &PROVIDERS[range.start() as usize..range.end() as usize],
                declaration
            );
            assert_eq!(
                usage.target.name.start() as usize,
                PROVIDERS.find(declaration).unwrap() + 4
            );
            assert!(lookup(offset as u32 - 1).is_none());
            assert!(lookup(offset as u32 + 5).is_none());
        }
    })
}

#[test]
fn retained_member_index_preserves_order_kinds_and_unused_field_declarations() {
    let (index, [a, b]) = with_fields(|sources, bodies, arena, store, ids| {
        (
            crate::ResolvedMemberIndex::build(sources, bodies, arena, store),
            ids,
        )
    });
    let caller = SourceId::from_index(2);
    assert!(index.diagnostics().is_empty());
    let accesses: Vec<_> = index
        .references_to(a)
        .iter()
        .map(|p| {
            assert_eq!(p.source(), caller);
            let offset = p.span().range().start();
            let access = index.field_at(caller, offset).unwrap();
            assert_eq!(
                index.definition_at(caller, offset),
                Some((*p, access.target))
            );
            assert_eq!(index.member_at(caller, offset), Some((a, access.target)));
            assert!(index.method_at(caller, offset).is_none());
            (offset as usize, access.kind)
        })
        .collect();
    assert_eq!(
        accesses,
        [
            (CLIENT.find("a.count").unwrap() + 2, FieldAccessKind::Write),
            (CLIENT.rfind("a.count").unwrap() + 2, FieldAccessKind::Read)
        ]
    );
    assert_eq!(index.references_to(b).len(), 1);
    let provider = SourceId::from_index(9);
    for name in ["plain", "hidden"] {
        assert!(
            index
                .member_at(provider, PROVIDERS.find(name).unwrap() as u32)
                .is_none()
        );
    }
    let offset = PROVIDERS.find("unused").unwrap() as u32;
    let (unused, target) = index.member_at(provider, offset).unwrap();
    assert_eq!(target.name.start(), offset);
    assert!(index.references_to(unused).is_empty());
    assert!(index.member_at(provider, target.name.end()).is_none());
}
