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

#[test]
fn rejected_argument_keeps_its_written_and_formal_positions_and_type_descriptions() {
    use crate::typer::{OverloadRejection, TyperError};
    let a = "class Ops { def pick(n: Int, right: Int): Int = right; def pick(flag: Boolean, n: Int): Int = n }";
    let b = "class Use { def bad(o: Ops): Int = o.pick(right = true, n = 1) }";
    let result = reports(a, b, false);
    let TyperError::OverloadApplicationNoApplicable { candidates, .. } = result[0].error() else {
        panic!()
    };
    assert!(
        matches!(&candidates[0].1, OverloadRejection::ArgumentNonConformance {argument_index:0, parameter_index:1, actual_description, expected_description, ..}
        if &**actual_description == "Boolean" && &**expected_description == "Int")
    );
    assert!(matches!(
        &candidates[1].1,
        OverloadRejection::NamedArgument {
            reason: crate::typer::NamedArgumentRejection::Unknown(_),
            ..
        }
    ));
}

#[test]
fn candidate_declarations_use_the_rejected_method_and_its_own_parameter() {
    use crate::typer::OverloadRejection;
    let a = "class Ops { def pick(right: Int): Int = right; def pick(flag: Boolean): Int = 1 }";
    let mut store = SemanticStore::new();
    let mut packages = Packages::new();
    let file = named(8, a, &mut store, &mut packages);
    let sources = NamedClassSources::new([&file], &store).unwrap();
    let absent = NamedClassSources::new([], &store).unwrap();
    for (method, token) in file.classes()[0].methods().zip(["right:", "flag:"]) {
        let reason = OverloadRejection::MissingRequiredArgument { parameter_index: 0 };
        let target = sources
            .overload_candidate_target(method.symbol, Some(&reason))
            .unwrap();
        let p = target.parameter.unwrap();
        assert_eq!(p.method, method.symbol);
        assert_eq!(p.target.name.start() as usize, a.find(token).unwrap());
        assert_eq!(store.symbols.get(p.parameter).owner, Some(method.symbol));
        assert!(target.method.declaration.start() < p.target.name.start());
        assert!(
            sources
                .overload_candidate_target(method.symbol, None)
                .unwrap()
                .parameter
                .is_none()
        );
        assert!(
            absent
                .overload_candidate_target(method.symbol, Some(&reason))
                .is_none()
        );
    }
}
