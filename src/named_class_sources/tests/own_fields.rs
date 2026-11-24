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

#[test]
fn own_private_body_fields_require_the_declaring_receiver_and_canonical_definition() {
    let text = "class A(start: Int) { private val seed: Int = start; private var count: Int = seed; def read(): Int = count; def set(count: Int): Unit = this.count = count; def other(a: A): Int = a.count }";
    with_checked(text, |sources, bodies, arena, store| {
        let (file, class) = sources.classes().next().unwrap();
        let fields: Vec<_> = class.fields().map(|f| f.symbol).collect();
        let at = |offset: usize| {
            sources
                .resolved_field_at(file.source(), offset as u32, bodies, arena, store)
                .unwrap()
        };
        for (offset, field, kind, name) in [
            (
                text.rfind("seed").unwrap(),
                fields[0],
                FieldAccessKind::Read,
                "seed",
            ),
            (
                text.find("= count;").unwrap() + 2,
                fields[1],
                FieldAccessKind::Read,
                "count",
            ),
            (
                text.find("this.count").unwrap() + 5,
                fields[1],
                FieldAccessKind::Write,
                "count",
            ),
        ] {
            let access = at(offset).unwrap();
            assert_eq!(
                (access.field, access.kind, access.target.name.start()),
                (field, kind, text.find(name).unwrap() as u32)
            );
        }
        for offset in [
            text.rfind("start").unwrap(),
            text.find("this.count = count").unwrap() + 13,
            text.rfind("a.count").unwrap() + 2,
        ] {
            assert!(at(offset).is_none(), "unexpected field at {offset}");
        }
    });
}
