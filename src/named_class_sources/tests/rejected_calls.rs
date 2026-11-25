use super::*;

fn reports(a: &str, b: &str, reverse: bool) -> Vec<crate::ReportedTyperError> {
    reports_with_provider(a, b, reverse, true)
}

fn reports_with_provider(
    a: &str,
    b: &str,
    reverse: bool,
    include_provider: bool,
) -> Vec<crate::ReportedTyperError> {
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
    let caller_only = NamedClassSources::new([&caller], &store).unwrap();
    report
        .into_semantic_diagnostics(if include_provider {
            &sources
        } else {
            &caller_only
        })
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

#[test]
fn one_overload_report_pairs_each_cause_with_its_own_source_evidence() {
    let a = "class Ops { def pick(right: Int): Int = right; def pick(flag: Boolean): Int = 1; def both(x: Int, y: Any): Int = 1; def both(x: Any, y: Int): Int = 2 }";
    let b = "class Use { def bad(o: Ops): Int = o.pick(right = true); def tied(o: Ops): Int = o.both(1, 1); def ok(o: Ops): Int = o.pick(flag = true) }";
    for reverse in [false, true] {
        let result = reports(a, b, reverse);
        assert_eq!(result.len(), 2);
        let candidates = result[0].overload_candidates().collect::<Vec<_>>();
        assert_eq!(candidates.len(), 2);
        for (c, token) in candidates.iter().zip(["true", "right"]) {
            let arg = c.argument.unwrap();
            assert_eq!((arg.written, arg.label.as_deref()), (0, Some("right")));
            assert_eq!(arg.location.source(), SourceId::from_index(3));
            assert_eq!(
                arg.location.span().range().start() as usize,
                b.find(token).unwrap()
            );
            assert!(c.reason.is_some());
        }
        assert!(candidates[0].target.unwrap().parameter.is_some());
        assert!(candidates[1].target.unwrap().parameter.is_none());
        let tied = result[1].overload_candidates().collect::<Vec<_>>();
        assert_eq!(tied.len(), 2);
        assert!(
            tied.iter()
                .all(|c| c.reason.is_none() && c.argument.is_none() && c.target.is_some())
        );
    }
}

#[test]
fn an_unindexed_provider_preserves_order_causes_and_caller_spans() {
    let a = "class Ops { def pick(right: Int): Int = right; def pick(flag: Boolean): Int = 1 }";
    let b = "class Use { def bad(o: Ops): Int = o.pick(right = true) }";
    let result = reports_with_provider(a, b, false, false);
    let candidates: Vec<_> = result[0].overload_candidates().collect();
    assert_eq!(candidates.len(), 2);
    assert!(
        candidates
            .iter()
            .all(|c| c.reason.is_some() && c.target.is_none() && c.argument.is_some())
    );
}

#[test]
fn an_omitted_required_parameter_has_a_target_but_no_written_operand() {
    let a = "class Ops { def pick(left: Int, right: Int = 9): Int = left; def pick(flag: Boolean): Int = 1 }";
    let b = "class Use { def bad(o: Ops): Int = o.pick(right = 4) }";
    let result = reports(a, b, false);
    let first = result[0].overload_candidates().next().unwrap();
    assert!(matches!(
        first.reason,
        Some(crate::typer::OverloadRejection::MissingRequiredArgument { parameter_index: 0 })
    ));
    assert!(first.argument.is_none());
    assert_eq!(
        first.target.unwrap().parameter.unwrap().target.name.start() as usize,
        a.find("left:").unwrap()
    );
}

#[test]
fn failed_named_constructions_report_the_written_site_and_preserve_sibling_bindings() {
    use crate::typer::TyperError;
    let a = "class Pair(left: Int, right: Int)";
    let b = "class Use {
      def unknown(): Pair = new Pair(`other` = 2, left = 9)
      def duplicate(): Pair = new Pair(right = 1, right = 2)
      def wrong(): Pair = new Pair(right = false, left = 9)
      def missing(): Pair = new Pair(right = 4)
      def good(): Pair = new Pair(right = 7, left = 9)
    }";
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let pair = named(8, a, &mut store, &mut packages);
    let caller = named(3, b, &mut store, &mut packages);
    let sources = NamedClassSources::new([&caller, &pair], &store).unwrap();
    let mut arena = prepare_bodies(&sources, &mut store, defs, &packages);
    let (body, report) = sources
        .check_class_body(
            caller.classes()[0].symbol,
            &mut store,
            defs,
            &packages,
            &mut arena,
            crate::core::NoResolver,
        )
        .into_parts();
    let errors: Vec<_> = report
        .into_semantic_diagnostics(&sources)
        .map(|d| match d.issue {
            crate::SemanticIssue::Typing(e) => e,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(errors.len(), 4);
    assert!(
        matches!(errors[0].error(), TyperError::UnknownNamedArgument { name, .. } if store.names.resolve(name.text()) == "other")
    );
    assert!(
        matches!(errors[1].error(), TyperError::DuplicateNamedArgument { name, .. } if store.names.resolve(name.text()) == "right")
    );
    assert!(
        matches!(errors[2].error(), TyperError::ApplicationArgumentTypeMismatch { argument_index: 0, expected, .. } if *expected == defs.int)
    );
    assert!(
        matches!(errors[3].error(), TyperError::MissingRequiredArgument { method, parameter_index: 0, parameter_name, .. } if *method == pair.classes()[0].constructor && &**parameter_name == "left")
    );
    let mut bodies = crate::CheckedClassBodies::default();
    bodies.record(body).unwrap();
    let index = crate::ResolvedMemberIndex::build(&sources, &bodies, &arena, &store);
    for (error, (token, width)) in errors.iter().zip([
        ("`other`", 7),
        ("right = 2", 5),
        ("false", 5),
        ("new Pair(right = 4)", 19),
    ]) {
        let span = error.location().unwrap();
        let offset = b.find(token).unwrap() as u32;
        assert_eq!(span.source(), caller.source());
        assert_eq!(
            (span.span().range().start(), span.span().range().end()),
            (offset, offset + width)
        );
    }
    for (i, (offset, _)) in b.match_indices("new Pair(").enumerate() {
        assert_eq!(
            index
                .construction_arguments_at(caller.source(), offset as u32 + 9)
                .is_some(),
            i == 4
        );
    }
}
