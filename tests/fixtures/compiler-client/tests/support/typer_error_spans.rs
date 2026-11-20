//! Source ranges observed by a compiler client reporting class body errors.
use super::{SignatureSources, class_planning::named, complete_signatures};
use dotty::core::{Definitions, TextRange};
use dotty::source_declarations::SourceClass;
use dotty::typer::TyperError;

/// The delimiters mark the expected byte range, even when its spelling is repeated.
fn check_body_error(marked: &str, check: impl FnOnce(TyperError, Definitions, &SourceClass)) {
    let (before, rest) = marked.split_once('«').unwrap();
    let (selected, after) = rest.split_once('»').unwrap();
    let text = format!("{before}{selected}{after}");
    let (mut store, defs, packages, unit) = named(&text);
    let units = std::slice::from_ref(&unit);
    complete_signatures(units, &mut store, defs, &packages).unwrap();
    let class = unit.classes.last().unwrap();
    let result = dotty::type_class_body(
        class,
        &unit.ast,
        &unit.index,
        &mut store,
        defs,
        &packages,
        &mut Default::default(),
    );
    let Err(dotty::ClassBodyError::Member { error, .. }) = result else {
        panic!("expected a member typing error, got {result:?}");
    };
    let span = SignatureSources::new(units, &store)
        .error_span(&error)
        .unwrap_or_else(|| panic!("missing span for {error:?}"));
    assert_eq!(span.source(), unit.source);
    assert_eq!(
        span.span().range(),
        TextRange::new(before.len() as u32, (before.len() + selected.len()) as u32).unwrap(),
        "{error:?}"
    );
    check(error, defs, class);
}

#[test]
fn control_flow_errors_locate_conditions_and_return_values() {
    check_body_error(
        "class C { def f(): Int = if «1» then 1 else 0 }",
        |error, defs, _| {
            assert!(
                matches!(error, TyperError::IfConditionTypeMismatch { actual, expected, .. } if actual == defs.int && expected == defs.boolean),
                "{error:?}"
            );
        },
    );
    check_body_error(
        "class C { def f(): Unit = while «1» do () }",
        |error, defs, _| {
            assert!(
                matches!(error, TyperError::WhileConditionTypeMismatch { actual, expected, .. } if actual == defs.int && expected == defs.boolean),
                "{error:?}"
            );
        },
    );
    for (marked, bare) in [
        ("class C { def f(): Int = return «false» }", false),
        ("class C { def f(): Int = «return» }", true),
    ] {
        check_body_error(marked, |error, defs, _| {
            let value_type = if bare { defs.unit } else { defs.boolean };
            assert!(
                matches!(error, TyperError::ReturnExpressionTypeMismatch { actual, expected, .. } if actual == value_type && expected == defs.int),
                "{error:?}"
            );
        });
    }
}

#[test]
fn assignment_errors_distinguish_the_target_from_the_value() {
    check_body_error(
        "class C { def f(): Unit = { val n = 1; «n» = 2 } }",
        |error, _, _| {
            assert!(
                matches!(error, TyperError::AssignmentTargetImmutable { .. }),
                "{error:?}"
            );
        },
    );
    check_body_error(
        "class C { def f(): Unit = { var n: Int = 1; n = «false» } }",
        |error, defs, _| {
            assert!(
                matches!(error, TyperError::ExpectedExpressionTypeMismatch { actual, expected, .. } if actual == defs.boolean && expected == defs.int),
                "{error:?}"
            );
        },
    );
    check_body_error(
        "class C { def n(): Int = 0; def f(): Unit = «n()» = 2 }",
        |error, _, _| {
            assert!(
                matches!(error, TyperError::AssignmentLhsNotAssignable { .. }),
                "{error:?}"
            );
        },
    );
}

#[test]
fn explicit_type_application_errors_locate_the_application_or_the_bound_argument() {
    use dotty::typer::TypeArgumentBoundSide;
    check_body_error(
        "class C { def f[A](x: A): A = x; def bad(): Int = «f[Int, Boolean]»(1) }",
        |error, _, _| {
            assert!(
                matches!(
                    error,
                    TyperError::ExplicitTypeApplicationArityMismatch {
                        expected: 1,
                        actual: 2,
                        ..
                    }
                ),
                "{error:?}"
            );
        },
    );
    check_body_error(
        "class C { def f[A, B <: Int](x: Int): Int = x; def bad(): Int = f[Boolean, «Boolean»](1) }",
        |error, defs, _| {
            assert!(
                matches!(error, TyperError::ExplicitTypeArgumentBoundViolation { parameter_index: 1, argument, bound, side: TypeArgumentBoundSide::Upper, .. } if argument == defs.boolean && bound == defs.int),
                "{error:?}"
            );
        },
    );
}

#[test]
fn overload_errors_locate_the_whole_call_and_keep_its_candidates() {
    use dotty::typer::OverloadRejection;

    check_body_error(
        "class C { def f(x: Int): Int = 1; def f(x: Boolean, y: Int): Int = 2; def bad(x: Boolean): Int = «f(x)» }",
        |error, defs, class| {
            let TyperError::OverloadApplicationNoApplicable { candidates, .. } = error else {
                panic!("{error:?}")
            };
            assert_eq!(candidates.len(), 2);
            let mut reasons = class.methods().take(2).map(|method| {
                candidates
                    .iter()
                    .find(|(id, _)| *id == method.symbol)
                    .expect("rejection for the declared overload")
                    .1
            });
            assert!(
                matches!(reasons.next(), Some(OverloadRejection::ArgumentNonConformance {
                argument_index: 0, actual, expected,
            }) if actual == defs.boolean && expected == defs.int)
            );
            assert!(matches!(
                reasons.next(),
                Some(OverloadRejection::WrongArity {
                    expected: 2,
                    actual: 1,
                })
            ));
        },
    );
    check_body_error(
        "class C { def f(x: Int, y: Any): Int = 1; def f(x: Any, y: Int): Int = 2; def bad(): Int = «f(1, 1)» }",
        |error, _, class| {
            let TyperError::AmbiguousOverloadApplication { candidates, .. } = error else {
                panic!("{error:?}")
            };
            let declared: Vec<_> = class.methods().take(2).map(|m| m.symbol).collect();
            assert_eq!(candidates.len(), declared.len());
            for symbol in declared {
                assert!(candidates.contains(&symbol));
            }
        },
    );
}
