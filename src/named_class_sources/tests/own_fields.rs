use super::*;
use crate::{CheckedClassBodies, FieldAccessKind};

fn with_checked<R>(
    text: &str,
    f: impl FnOnce(&NamedClassSources<'_>, &CheckedClassBodies, &AstArena<Typed>, &SemanticStore) -> R,
) -> R {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let file = named(1, text, &mut store, &mut packages);
    let sources = NamedClassSources::new([&file], &store).unwrap();
    let mut arena = prepare_bodies(&sources, &mut store, defs, &packages);
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
    f(&sources, &bodies, &arena, &store)
}

#[test]
fn bare_public_access_uses_the_committed_symbol_and_assignment() {
    let text = "class A { var count: Int = 0; def inc(delta: Int): Unit = count = count + delta; def param(count: Int): Int = count; def local(): Int = { val count: Int = 2; count } }";
    with_checked(text, |sources, bodies, arena, store| {
        let (file, class) = sources.classes().next().unwrap();
        let field = class.fields().next().unwrap().symbol;
        let at = |offset: usize| {
            sources
                .resolved_field_at(file.source(), offset as u32, bodies, arena, store)
                .unwrap()
        };
        let sites: Vec<_> = text
            .match_indices("count")
            .map(|(offset, _)| offset)
            .collect();
        let [declaration, write, read, param, param_use, local, local_use] = sites[..] else {
            panic!("fixture names")
        };
        for (offset, kind) in [
            (write, FieldAccessKind::Write),
            (read, FieldAccessKind::Read),
        ] {
            let access = at(offset + 1).unwrap();
            assert_eq!(
                (access.field, access.kind, access.target.name.start()),
                (field, kind, declaration as u32)
            );
            assert!(at(offset + 5).is_none());
        }
        for offset in [
            param,
            param_use,
            local,
            local_use,
            text.rfind("delta").unwrap(),
        ] {
            assert!(at(offset).is_none(), "unexpected field at {offset}");
        }
    });
}
