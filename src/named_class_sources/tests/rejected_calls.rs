use super::*;

fn reports(a: &str, b: &str, reverse: bool) -> Vec<crate::ReportedTyperError> {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let provider = named(8, a, &mut store, &mut packages);
    let caller = named(3, b, &mut store, &mut packages);
    let files = if reverse {
        [&caller, &provider]
    } else {
        [&provider, &caller]
    };
    let sources = NamedClassSources::new(files, &store).unwrap();
    let mut arena = prepare_bodies(&sources, &mut store, defs, &packages);
    let (_, report) = sources
        .check_class_body(
            caller.classes()[0].symbol,
            &mut store,
            defs,
            &packages,
            &mut arena,
            crate::core::NoResolver,
        )
        .into_parts();
    report
        .into_semantic_diagnostics(&sources)
        .map(|d| match d.issue {
            crate::SemanticIssue::Typing(e) => *e,
            other => panic!("{other:?}"),
        })
        .collect()
}

#[test]
fn named_source_reports_select_the_exact_label_or_operand() {
    let a = "class Ops { def pick(left: Int, right: Int): Int = left }";
    let b = "class Use { def u(o: Ops): Int = o.pick(1, o.pick(`bad` = 9, right = 4)); def d(o: Ops): Int = o.pick(left = 1, left = 2); def t(o: Ops): Int = o.pick(right = false, left = 9) }";
    let result = reports(a, b, false);
    assert_eq!(result.len(), 3);
    for (d, (token, len)) in result
        .iter()
        .zip([("`bad`", 5), ("left = 2", 4), ("false", 5)])
    {
        let range = d.location().unwrap().span().range();
        let start = b.find(token).unwrap() as u32;
        assert_eq!((range.start(), range.end()), (start, start + len));
    }
}
