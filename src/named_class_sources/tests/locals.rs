use super::*;
use crate::{CheckedClassBody, ClassBodyReport};

fn with_checked<R>(
    text: &str,
    f: impl FnOnce(
        &NamedClassSources<'_>,
        CheckedClassBody,
        ClassBodyReport,
        &AstArena<Typed>,
        &SemanticStore,
    ) -> R,
) -> R {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let file = named(7, text, &mut store, &mut packages);
    let sources = NamedClassSources::new([&file], &store).unwrap();
    let mut arena = prepare_bodies(&sources, &mut store, defs, &packages);
    let (body, report) = sources
        .check_class_body(
            file.classes()[0].symbol,
            &mut store,
            defs,
            &packages,
            &mut arena,
            crate::core::NoResolver,
        )
        .into_parts();
    f(&sources, body, report, &arena, &store)
}

#[test]
fn complete_and_partial_results_retain_only_committed_local_bindings() {
    for bad in ["", "; def bad(): Int = { val lost = 1; lost + false }"] {
        let text = format!("class A {{ def good(): Int = {{ val kept = 1; kept }}{bad} }}");
        with_checked(&text, |sources, body, report, _, store| {
            assert_eq!(report.errors.len(), usize::from(!bad.is_empty()));
            assert_eq!(
                matches!(body, CheckedClassBody::Complete(_)),
                bad.is_empty()
            );
            let [local] = body.local_bindings() else {
                panic!("only the good method committed a local")
            };
            let (source, tree) = local.source_declaration();
            let file = sources.file(source).unwrap();
            let name = file.named().syntax().member_names.value_name(tree).unwrap();
            assert_eq!(&text[name.start() as usize..name.end() as usize], "kept");
            assert_eq!(
                store.symbols.get(local.symbol()).owner,
                Some(file.classes()[0].methods().next().unwrap().symbol)
            );
            assert_eq!(
                body.bindings().unwrap().get(source, tree),
                Some(local.declaration())
            );
        });
    }
}

fn retained(text: &str, errors: usize) -> crate::ResolvedMemberIndex {
    with_checked(text, |sources, body, report, arena, store| {
        assert_eq!(report.errors.len(), errors, "{report:?}");
        let mut bodies = crate::CheckedClassBodies::default();
        bodies.record(body).unwrap();
        let index = crate::ResolvedMemberIndex::build(sources, &bodies, arena, store);
        assert!(index.diagnostics().is_empty());
        index
    })
}

#[test]
fn shadowed_local_reads_keep_their_exact_declarations_after_arena_release() {
    let text = "class A { def good(): Int = { val `x` = 1; val unused = `x`; { val `x` = 2; `x` }; `x` }; def bad(): Int = { val lost = 1; lost + false } }";
    let index = retained(text, 1);
    let id = SourceId::from_index(7);
    let names: Vec<_> = text.match_indices("`x`").map(|(i, _)| i as u32).collect();
    let [outer, before, inner, within, after] = names[..] else {
        panic!("local names")
    };
    let at = |pos| index.symbol_at(id, pos).unwrap();
    for (decl, reads) in [(outer, vec![before, after]), (inner, vec![within])] {
        let target = at(decl);
        assert!(target.usage.is_none());
        assert_eq!(target.kind, crate::core::SymbolKind::Local);
        assert_eq!(
            (target.target.name.start(), target.target.name.end()),
            (decl, decl + 3)
        );
        let refs = index.references_to(target.symbol);
        assert_eq!(refs.len(), reads.len());
        for (&span, pos) in refs.iter().zip(reads) {
            let site = at(pos + 1);
            assert_eq!(site.symbol, target.symbol);
            assert_eq!(site.target, target.target);
            assert_eq!(site.usage, Some(span));
            assert!(index.symbol_at(id, pos + 3).is_none());
        }
    }
    assert_ne!(at(outer).symbol, at(inner).symbol);
    assert!(
        index
            .references_to(at(text.find("unused").unwrap() as u32).symbol)
            .is_empty()
    );
    for (i, _) in text.match_indices("lost") {
        assert!(index.symbol_at(id, i as u32).is_none());
    }
}

#[test]
fn local_assignment_uses_its_committed_lhs_and_rejects_an_immutable_target() {
    use crate::ValueAccessKind::{Read, Write};
    let text = "class A { def add(step: Int): Int = { var n = 0; n = n + step; n }; def bad(): Unit = { val frozen = 1; frozen = 2 } }";
    let index = with_checked(text, |sources, body, report, arena, store| {
        assert!(matches!(
            report.errors.as_slice(),
            [crate::ClassBodyError::Member {
                error: crate::typer::TyperError::AssignmentTargetImmutable { .. },
                ..
            }]
        ));
        let mut bodies = crate::CheckedClassBodies::default();
        bodies.record(body).unwrap();
        crate::ResolvedMemberIndex::build(sources, &bodies, arena, store)
    });
    let id = SourceId::from_index(7);
    let lhs = text.find("n = n").unwrap() as u32;
    let step = text.rfind("step").unwrap() as u32;
    let n = index.symbol_at(id, lhs).unwrap().symbol;
    for (pos, kind) in [
        (lhs, Write),
        (lhs + 4, Read),
        (text.find("; n }").unwrap() as u32 + 2, Read),
    ] {
        assert_eq!(index.value_access_at(id, pos), Some(kind));
        assert_eq!(index.symbol_at(id, pos).unwrap().symbol, n);
    }
    assert_eq!(index.value_access_at(id, step), Some(Read));
    assert_eq!(
        index.symbol_at(id, step).unwrap().kind,
        crate::core::SymbolKind::Parameter
    );
    assert!(
        index
            .value_access_at(id, text.find("var n").unwrap() as u32 + 4)
            .is_none()
    );
    for (pos, _) in text.match_indices("frozen") {
        assert!(index.value_access_at(id, pos as u32).is_none());
        assert!(index.symbol_at(id, pos as u32).is_none());
    }
}
