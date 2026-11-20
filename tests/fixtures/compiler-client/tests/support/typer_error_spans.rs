//! Source ranges observed by a compiler client reporting class body errors.
use super::{SignatureSources, class_planning::named, complete_signatures};
use dotty::core::{Definitions, TextRange};
use dotty::source_declarations::SourceClass;
use dotty::typer::TyperError;

#[test]
fn cross_file_diagnostic_tracks_an_unsaved_edit_in_utf16() {
    use super::{name_units, named_classes, parsing};
    use dotty::classloader::{ClasspathSymbolResolver, JmodClassPath, LoadingSession};
    use dotty::core::{Packages, SemanticStore, SourceId, SourceText, SymbolResolver};
    use std::path::PathBuf;

    let a = SourceId::from_index(7);
    let b = SourceId::from_index(2);
    let mut buffer = String::from("class B {\n  def read(a: A): Int = /* 😀 */ a.take(\"bad\")\n}");
    let analyze = |buffer: &str| {
        let files = [
            (b, "B.scala", buffer),
            (
                a,
                "A.scala",
                r#"class A { def take(n: Int): Int = n; def label(): Any = "A" }"#,
            ),
        ];
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let units = name_units(
            &files,
            parsing::parse_files(&files, 2),
            &mut store,
            &mut packages,
        );
        let mut headers = complete_signatures(&units, &mut store, defs, &packages).unwrap();
        let sources = SignatureSources::new(&units, &store);
        // Keep the loader's generic class metadata available during conformance checking.
        let jdk =
            PathBuf::from(std::env::var_os("JAVA_HOME").expect("JAVA_HOME for the client tests"));
        let mut resolver = ClasspathSymbolResolver::new(
            JmodClassPath::new(jdk.join("jmods/java.base.jmod")).unwrap(),
            defs,
            LoadingSession::with_packages(packages),
        );
        let local_packages = Packages::new();
        let mut diagnostics = Vec::new();
        for (unit, class) in named_classes(&units) {
            let before = store.checkpoint();
            let resolver_before = resolver.checkpoint();
            let error = match dotty::type_class_body_with_resolver(
                class,
                &unit.ast,
                &unit.index,
                &mut store,
                defs,
                &local_packages,
                &mut headers.arena,
                &mut resolver,
            ) {
                Ok(_) => continue,
                Err(dotty::ClassBodyError::Member { error, .. }) => error,
                Err(other) => panic!("unexpected class error: {other:?}"),
            };
            assert_eq!(store.checkpoint(), before);
            assert_eq!(resolver.checkpoint(), resolver_before);
            let TyperError::ApplicationArgumentTypeMismatch {
                source,
                argument_index,
                expected,
                ref actual_description,
                ref expected_description,
                ..
            } = error
            else {
                panic!("expected a mismatched argument, got {error:?}");
            };
            assert_eq!((source, argument_index, expected), (b, 0, defs.int));
            let span = sources.error_span(&error).expect("argument position");
            let &(_, name, text) = files.iter().find(|file| file.0 == span.source()).unwrap();
            let source = SourceText::new(text).unwrap();
            let range = span.span().range();
            assert_eq!(source.slice(range).unwrap(), "\"bad\"");
            let lines = source.line_index().unwrap();
            let position = |offset| {
                (
                    lines.line_of_offset(offset).unwrap(),
                    lines.utf16_column(text, offset).unwrap(),
                )
            };
            let message = format!("expected {expected_description}, found {actual_description}");
            diagnostics.push((
                name,
                position(range.start()),
                position(range.end()),
                message,
            ));
        }
        diagnostics
    };

    assert_eq!(
        analyze(&buffer),
        [(
            "B.scala",
            (1, 40),
            (1, 45),
            String::from("expected Int, found String")
        )]
    );
    let start = buffer.find("\"bad\"").unwrap();
    buffer.replace_range(start..start + "\"bad\"".len(), "42");
    assert!(analyze(&buffer).is_empty());
}

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
