use super::*;
use std::{cell::RefCell, rc::Rc};

fn method_tree_for(
    parsed: &dotty_parser::ParseResult,
    index: &SourceSemanticIndex,
    source: SourceId,
    method: SymbolId,
) -> TreeId<Untyped> {
    parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            (index.symbol_at(source, tree) == Some(method)
                && matches!(node.kind, TreeKind::DefDef(_)))
            .then_some(tree)
        })
        .expect("source method should have a definition tree")
}

fn rollback_projection(typer: &SourceTyper<'_>, parsed: &dotty_parser::ParseResult) -> String {
    let source = typer.source;
    // The typed arena is separate from SemanticStore and SourceTypedIndex.
    // Keep both its allocation boundary and every preexisting node in the
    // projection so rollback checks catch leaked nodes and in-place mutation.
    let typed_arena_checkpoint = typer.typed_ast().checkpoint();
    let typed_arena_nodes = typer
        .typed_ast()
        .iter()
        .map(|(tree, node)| (tree.index(), node.clone()))
        .collect::<Vec<_>>();
    let mappings = parsed
        .ast
        .iter()
        .map(|(tree, _)| {
            (
                tree.index(),
                typer.source_typed_index().get(source, tree),
                typer.function_literal_method_at(source, tree),
                typer.function_literal_definition_at(source, tree),
                typer.function_literal_parameter_symbol_at(source, tree),
            )
        })
        .collect::<Vec<_>>();
    let closure_environments = mappings
        .iter()
        .filter_map(|(source_tree, typed, _, _, _)| {
            let typed = (*typed)?;
            let TreeKind::Closure(closure) = &typer.typed_ast().get(typed).kind else {
                return None;
            };
            Some((
                *source_tree,
                closure
                    .env
                    .iter()
                    .map(|tree| tree.index())
                    .collect::<Vec<_>>(),
            ))
        })
        .collect::<Vec<_>>();
    format!(
        "checkpoint={:?}; typed_arena_checkpoint={typed_arena_checkpoint:?}; typed_arena_nodes={typed_arena_nodes:?}; symbols={:?}; scopes={:?}; types={:?}; mappings={mappings:?}; closures={closure_environments:?}; resolver={:?}",
        typer.store.checkpoint(),
        typer.store.symbols,
        typer.store.scopes,
        typer.store.types,
        typer.resolver.checkpoint(),
    )
}

fn assert_fixture_failure_rolls_back(source_text: &str, method_name: &str) -> String {
    let (parsed, mut store, mut packages, definitions, index, source) = parse_and_name(source_text);
    enter_scala_function_classes(&mut store, &mut packages, [1]);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, method_name);
    let tree = method_tree_for(&parsed, &index, source, method);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let before = rollback_projection(&typer, &parsed);
    let first = typer
        .type_method_definition(tree)
        .expect_err("unsupported or invalid root must fail as a whole method");
    assert_eq!(rollback_projection(&typer, &parsed), before);
    assert_eq!(typer.source_typed_index().get(source, tree), None);
    assert!(typer.function_literal_method_at(source, rhs).is_none());
    let diagnostic = format!("{first:?}");
    let repeated = typer
        .type_method_definition(tree)
        .expect_err("repeating the root should remain an error");
    assert_eq!(format!("{repeated:?}"), diagnostic);
    assert_eq!(rollback_projection(&typer, &parsed), before);
    let (valid_method, _) =
        method_definition_and_rhs(&parsed, typer.store(), &index, source, "valid");
    let valid_tree = method_tree_for(&parsed, &index, source, valid_method);
    let typed_valid = typer
        .type_method_definition(valid_tree)
        .expect("a valid sibling should type after the failed root in the same session");
    assert_eq!(
        typer.source_typed_index().get(source, valid_tree),
        Some(typed_valid)
    );
    diagnostic
        .split('{')
        .next()
        .expect("debug errors have a stable enum name")
        .trim()
        .to_owned()
}

fn observe_captures_before_method_rollback(source_text: &str, method_name: &str) -> Vec<String> {
    let (parsed, mut store, mut packages, definitions, index, source) = parse_and_name(source_text);
    enter_scala_function_classes(&mut store, &mut packages, [1]);
    let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, method_name);
    let method_tree = method_tree_for(&parsed, &index, source, method);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let rhs = method_definition_and_rhs(&parsed, typer.store(), &index, source, method_name).1;
    let delta = match &parsed.ast.get(method_tree).kind {
        TreeKind::DefDef(definition) => index
            .symbol_at(source, definition.value_param_clauses[0][0])
            .unwrap(),
        _ => unreachable!(),
    };
    let observation = Rc::new(RefCell::new(None));
    let observed = Rc::clone(&observation);
    typer.transaction_failure_observer = Some(Rc::new(move |typer| {
        let Some(lambda_method) = typer.function_literal_method_at(source, rhs) else {
            return;
        };
        let captures = typer.function_literals.captures(lambda_method);
        let roles = captures
            .iter()
            .map(|capture| {
                let tree = typer.typed_ast().get(*capture);
                if matches!(tree.kind, TreeKind::This(_)) {
                    "this".to_owned()
                } else if matches!(
                    typer.store.types.get(tree.ty),
                    Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == delta
                ) {
                    "delta".to_owned()
                } else {
                    "other".to_owned()
                }
            })
            .collect::<Vec<_>>();
        *observed.borrow_mut() = Some(roles);
    }));
    let result = typer.type_method_definition(method_tree);
    match (method_name, result) {
        ("broken", Err(TyperError::ApplicationArgumentTypeMismatch { .. }))
        | (
            "wrongResult",
            Err(
                TyperError::ExpectedExpressionTypeMismatch { .. }
                | TyperError::ExpectedExpressionConformanceUnsupported { .. },
            ),
        ) => {}
        (_, other) => panic!("unexpected late failure result for {method_name}: {other:?}"),
    }
    let captures = observation
        .borrow_mut()
        .take()
        .expect("transaction observer must see captures before rollback");
    assert_eq!(
        captures.len(),
        2,
        "both enclosing-this and delta captures must be recorded before the late error"
    );
    assert_eq!(captures, ["this", "delta"]);
    captures
}

fn exercise_failed_captured_methods_restore_state_and_allow_stable_retry()
-> (Vec<String>, Vec<&'static str>) {
    // Both `broken` and `wrongResult` visit `inc(x)` and `delta` before the
    // error: one fails at the later Boolean argument, the other at the
    // declared Function1 result conformance.
    let source_text = concat!(
        "class Calculator { ",
        "def inc(x: Int): Int = x + 1; ",
        "def prior(delta: Int): Int => Int = (delta: Int) => delta; ",
        "def priorCaptured(delta: Int): Int => Int = x => x + delta; ",
        "def broken(delta: Int): Int => Int = x => inc(x) + delta + inc(true); ",
        "def wrongResult(delta: Int): Int => Boolean = x => inc(x) + delta; ",
        "def unknown(delta: Int): Int => Int = x => inc(x) + delta + missing; ",
        "def mutable(delta: Int): Int => Int = { var changed: Int = delta; x => x + changed }; ",
        "def byName(value: => Int): Int => Int = x => x + value; ",
        "def valid(delta: Int): Int => Int = x => inc(x) + delta",
        " }",
    );
    let (parsed, mut store, mut packages, definitions, index, source) = parse_and_name(source_text);
    enter_scala_function_classes(&mut store, &mut packages, [1]);
    let method_trees = [
        "prior",
        "priorCaptured",
        "broken",
        "wrongResult",
        "unknown",
        "mutable",
        "byName",
        "valid",
    ]
    .map(|name| {
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, name);
        (
            name,
            method,
            rhs,
            method_tree_for(&parsed, &index, source, method),
        )
    });
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );

    let (_, prior_method, _, prior_tree) = method_trees
        .iter()
        .find(|(name, _, _, _)| *name == "prior")
        .copied()
        .unwrap();
    let typed_prior = typer
        .type_method_definition(prior_tree)
        .expect("an existing successful method establishes prior mappings");
    assert_eq!(
        typer.source_typed_index().get(source, prior_tree),
        Some(typed_prior)
    );
    assert_eq!(
        typer.store.symbols.get(prior_method).kind,
        SymbolKind::Method
    );
    let (_, prior_capture_method, prior_capture_rhs, prior_capture_tree) = method_trees
        .iter()
        .find(|(name, _, _, _)| *name == "priorCaptured")
        .copied()
        .unwrap();
    let typed_prior_capture = typer
        .type_method_definition(prior_capture_tree)
        .expect("a parameter-capture closure establishes a preexisting environment");
    assert_eq!(
        typer.source_typed_index().get(source, prior_capture_tree),
        Some(typed_prior_capture)
    );
    assert_eq!(
        typer.store.symbols.get(prior_capture_method).kind,
        SymbolKind::Method
    );
    let typed_prior_capture_rhs = typer
        .source_typed_index()
        .get(source, prior_capture_rhs)
        .unwrap();
    assert!(matches!(
        typer.typed_ast().get(typed_prior_capture_rhs).kind,
        TreeKind::Closure(ref closure) if closure.env.len() == 1
    ));

    let mut diagnostic_categories = Vec::new();
    for (name, method, rhs, tree) in method_trees
        .iter()
        .filter(|(name, _, _, _)| *name != "prior" && *name != "priorCaptured" && *name != "valid")
    {
        let before = rollback_projection(&typer, &parsed);
        let first = typer
            .type_method_definition(*tree)
            .expect_err("negative method should fail as a whole");
        let first_diagnostic = format!("{first:?}");
        let category = match (*name, &first) {
            ("broken", TyperError::ApplicationArgumentTypeMismatch { .. }) => "argument-type",
            ("wrongResult", TyperError::ExpectedExpressionTypeMismatch { .. })
            | ("wrongResult", TyperError::ExpectedExpressionConformanceUnsupported { .. }) => {
                "result-conformance"
            }
            ("unknown", TyperError::TermNameNotFound { .. }) => "unknown-name",
            ("mutable", TyperError::FunctionLiteralCaptureUnsupported { .. }) => "mutable-capture",
            ("byName", TyperError::FunctionLiteralCaptureUnsupported { .. }) => "by-name-capture",
            other => panic!("unexpected error for {name}: {other:?}"),
        };
        if *name == "broken" {
            let failing_tree = match &first {
                TyperError::ApplicationArgumentTypeMismatch { tree_index, .. }
                | TyperError::ExpectedExpressionTypeMismatch { tree_index, .. }
                | TyperError::ExpectedExpressionConformanceUnsupported { tree_index, .. } => {
                    *tree_index
                }
                _ => unreachable!("the cases above checked the late-error category"),
            };
            let failure_start = parsed
                .ast
                .iter()
                .find_map(|(tree, node)| {
                    (tree.index() == failing_tree)
                        .then(|| {
                            node.position
                                .map(|position| position.span().range().start())
                        })
                        .flatten()
                })
                .expect("late failure should retain its source span");
            let method_start = source_text
                .find(&format!("def {name}"))
                .expect("late-error method should be present in the source root");
            let delta_start = method_start
                + source_text[method_start..]
                    .find("+ delta")
                    .expect("late-error method contains the second capture");
            assert!(
                failure_start as usize > delta_start,
                "{name} must fail after the source use that discovers the delta capture"
            );
        }
        diagnostic_categories.push(format!("{name}:{category}"));
        assert_eq!(
            rollback_projection(&typer, &parsed),
            before,
            "{name} failure must restore every observable semantic projection"
        );
        assert_eq!(
            typer.source_typed_index().get(source, *tree),
            None,
            "failed source method must not be published"
        );
        assert!(
            typer.function_literal_method_at(source, *rhs).is_none(),
            "failed source method must not leave a lambda identity"
        );
        assert!(typer.store.symbols.contains(*method));
        assert_eq!(
            typer.source_typed_index().get(source, prior_tree),
            Some(typed_prior)
        );
        let second = typer
            .type_method_definition(*tree)
            .expect_err("repeating the same negative method should fail");
        assert_eq!(format!("{second:?}"), first_diagnostic);
        assert_eq!(rollback_projection(&typer, &parsed), before);
    }

    let (_, valid_method, valid_rhs, valid_tree) = method_trees
        .iter()
        .find(|(name, _, _, _)| *name == "valid")
        .copied()
        .unwrap();
    let typed_valid = typer
        .type_method_definition(valid_tree)
        .expect("valid sibling should type in the same session after failures");
    assert_eq!(
        typer.source_typed_index().get(source, valid_tree),
        Some(typed_valid)
    );
    assert_eq!(
        typer.store.symbols.get(valid_method).kind,
        SymbolKind::Method
    );
    let typed_rhs = typer.source_typed_index().get(source, valid_rhs).unwrap();
    let TreeKind::Closure(closure) = &typer.typed_ast().get(typed_rhs).kind else {
        panic!("valid sibling RHS should be a typed closure");
    };
    assert_eq!(closure.env.len(), 2);
    assert!(matches!(
        typer.typed_ast().get(closure.env[0]).kind,
        TreeKind::This(_)
    ));
    let delta = match &parsed.ast.get(valid_tree).kind {
        TreeKind::DefDef(definition) => index
            .symbol_at(source, definition.value_param_clauses[0][0])
            .unwrap(),
        _ => unreachable!(),
    };
    assert!(matches!(
        typer.store().types.get(typer.typed_ast().get(closure.env[1]).ty),
        Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == delta
    ));
    (diagnostic_categories, vec!["this", "delta"])
}

#[test]
fn failed_captured_methods_restore_state_and_allow_stable_retry() {
    // Each call parses and names a new root and constructs an independent
    // store and Typer; semantic receipts are checked canonically in-session.
    let first = exercise_failed_captured_methods_restore_state_and_allow_stable_retry();
    let second = exercise_failed_captured_methods_restore_state_and_allow_stable_retry();
    assert_eq!(
        first, second,
        "fresh sessions should have the same canonical receipt"
    );
}

#[test]
fn late_argument_and_result_errors_are_observed_after_both_capture_records() {
    let source = concat!(
        "class Calculator { ",
        "def inc(x: Int): Int = x + 1; ",
        "def broken(delta: Int): Int => Int = x => inc(x) + delta + inc(true); ",
        "def wrongResult(delta: Int): Int => Boolean = x => inc(x) + delta",
        " }",
    );
    assert_eq!(
        observe_captures_before_method_rollback(source, "broken"),
        observe_captures_before_method_rollback(source, "broken")
    );
    assert_eq!(
        observe_captures_before_method_rollback(source, "wrongResult"),
        observe_captures_before_method_rollback(source, "wrongResult")
    );
}

#[test]
fn scala_invalid_qualification_and_deferred_member_forms_rollback() {
    let wrong_qualified = include_str!(
        "../../../tests/fixtures/lexical-closure-contract/WrongQualifiedOtherThis.scala"
    );
    let wrong_qualified_diagnostic = assert_fixture_failure_rolls_back(wrong_qualified, "wrong");
    assert_eq!(
        wrong_qualified_diagnostic,
        assert_fixture_failure_rolls_back(wrong_qualified, "wrong")
    );
    assert!(
        wrong_qualified_diagnostic.contains("ThisOwnerNotEnclosing"),
        "unexpected qualification failure: {wrong_qualified_diagnostic}"
    );

    // The oracle runner characterizes both valid forms before these Rust-side
    // errors are asserted as deferred support rather than invalid Scala.
    let inherited =
        include_str!("../../../tests/fixtures/lexical-closure-contract/InheritedMember.scala");
    let inherited_diagnostic = assert_fixture_failure_rolls_back(inherited, "inherited");
    assert_eq!(
        inherited_diagnostic,
        assert_fixture_failure_rolls_back(inherited, "inherited")
    );
    assert!(
        inherited_diagnostic.contains("TermNameNotFound"),
        "Scala-valid inherited member must remain an observable unresolved-member error: {inherited_diagnostic}"
    );

    let super_capture =
        include_str!("../../../tests/fixtures/lexical-closure-contract/SuperCapture.scala");
    let super_diagnostic = assert_fixture_failure_rolls_back(super_capture, "throughSuper");
    assert_eq!(
        super_diagnostic,
        assert_fixture_failure_rolls_back(super_capture, "throughSuper")
    );
    assert!(
        super_diagnostic.contains("Unsupported"),
        "super source was not rejected as an explicit deferred feature: {super_diagnostic}"
    );

    let by_name =
        include_str!("../../../tests/fixtures/lexical-closure-contract/ByNameCapture.scala");
    let by_name_diagnostic = assert_fixture_failure_rolls_back(by_name, "capture");
    assert_eq!(
        by_name_diagnostic,
        assert_fixture_failure_rolls_back(by_name, "capture")
    );
    assert!(
        by_name_diagnostic.contains("FunctionLiteralCaptureUnsupported"),
        "by-name capture must be an explicit deferred feature: {by_name_diagnostic}"
    );
}
