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
