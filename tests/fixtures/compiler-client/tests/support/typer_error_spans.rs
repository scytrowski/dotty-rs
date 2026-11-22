//! Source ranges observed by a compiler client reporting class body errors.
use super::{SignatureSources, class_planning::named, complete_signatures};
use dotty::core::{Definitions, DiagnosticSeverity, SymbolKind, TextRange};
use dotty::source_declarations::SourceClass;
use dotty::typer::TyperError;

#[test]
fn cross_file_diagnostics_track_two_unsaved_edits_in_utf16() {
    use super::{name_units, named_classes, parsing};
    use dotty::classloader::{ClasspathSymbolResolver, JmodClassPath, LoadingSession};
    use dotty::core::{Packages, SemanticStore, SourceId, SourceText, SymbolResolver};
    use dotty::jvm_class_plan::{ClassPlanError, JvmClassPlanner, TypedClassBodies};
    use std::path::PathBuf;

    let a = SourceId::from_index(7);
    let b = SourceId::from_index(2);
    let mut buffer = String::from(
        "class B {\n  def read(a: A): Int = /* 😀 */ a.take(\"bad\")\n  def next(a: A): Int = a.take(false)\n}",
    );
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
        let mut bodies = TypedClassBodies::default();
        for (unit, class) in named_classes(&units) {
            let before = store.checkpoint();
            let resolver_before = resolver.checkpoint();
            let report = dotty::check_class_body(
                class,
                unit.ast(),
                unit.index(),
                &mut store,
                defs,
                &local_packages,
                &mut headers.arena,
                &mut resolver,
            );
            assert!(report.diagnostics.is_empty());
            let errors = match report.body {
                Ok(body) => {
                    bodies.record(body).unwrap();
                    continue;
                }
                Err(errors) => errors,
            };
            assert!(!errors.is_empty());
            if errors.len() == class.members.len() {
                assert_eq!(store.checkpoint(), before);
                assert_eq!(resolver.checkpoint(), resolver_before);
            }
            for failure in errors {
                let call = failure
                    .into_call_diagnostic(|source| {
                        sources.by_source.get(&source).map(|unit| unit.ast())
                    })
                    .unwrap();
                let member = call.member().unwrap();
                let declaration = sources.declarations.member(member).unwrap();
                assert_eq!(declaration.class, class.symbol);
                assert_eq!(declaration.kind, SymbolKind::Method);
                let diagnostic = call.render();
                let TyperError::ApplicationArgumentTypeMismatch {
                    source,
                    argument_index,
                    expected,
                    ..
                } = call.error()
                else {
                    panic!("expected a mismatched argument, got {call:?}");
                };
                assert_eq!((*source, *argument_index, *expected), (b, 0, defs.int));
                assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
                let span = diagnostic.location.expect("argument position");
                let &(_, name, text) = files.iter().find(|file| file.0 == span.source()).unwrap();
                let source = SourceText::new(text).unwrap();
                let range = span.span().range();
                let spelling = source.slice(range).unwrap().to_owned();
                let member_name = store
                    .names
                    .resolve(store.symbols.get(member).name.text())
                    .to_owned();
                let lines = source.line_index().unwrap();
                let position = |offset| {
                    (
                        lines.line_of_offset(offset).unwrap(),
                        lines.utf16_column(text, offset).unwrap(),
                    )
                };
                diagnostics.push((
                    name,
                    position(range.start()),
                    position(range.end()),
                    diagnostic.message,
                    spelling,
                    member_name,
                ));
            }
        }
        let b_class = units[0].classes()[0].symbol;
        assert!(bodies.get(units[1].classes()[0].symbol).is_ok());
        if diagnostics.is_empty() {
            let b = &units[0];
            let class = &b.classes()[0];
            let plan = JvmClassPlanner::new(&store, b.ast(), b.index())
                .with_header_bindings(&headers.arena, &headers.bindings)
                .plan_typed_class(class, &headers.arena, bodies.get(b_class).unwrap())
                .unwrap();
            assert!(
                plan.methods
                    .iter()
                    .map(|m| m.method)
                    .eq(class.methods().map(|m| m.symbol))
            );
        } else {
            assert_eq!(
                bodies.get(b_class).unwrap_err(),
                ClassPlanError::MissingClassBody(b_class)
            );
        }
        diagnostics
    };

    let string_error = (
        "B.scala",
        (1, 40),
        (1, 45),
        "Expected Int, found String".into(),
        "\"bad\"".into(),
        "read".into(),
    );
    let boolean_error = (
        "B.scala",
        (2, 31),
        (2, 36),
        "Expected Int, found Boolean".into(),
        "false".into(),
        "next".into(),
    );
    assert_eq!(analyze(&buffer), [string_error, boolean_error.clone()]);
    buffer = buffer.replace("\"bad\"", "42");
    assert_eq!(analyze(&buffer), [boolean_error]);
    buffer = buffer.replace("false", "7");
    assert!(analyze(&buffer).is_empty());
}

#[test]
fn migration_warning_survives_a_sibling_error_and_tracks_source_edits() {
    use dotty::core::NoResolver;
    use dotty::typer::TyperWarning;

    let check = |text: &str, warning: bool, bad_argument: bool| {
        let (mut store, defs, packages, unit) = named(text);
        let mut headers =
            complete_signatures(std::slice::from_ref(&unit), &mut store, defs, &packages).unwrap();
        let report = dotty::check_class_body(
            &unit.classes()[0],
            unit.ast(),
            unit.index(),
            &mut store,
            defs,
            &packages,
            &mut headers.arena,
            NoResolver,
        );

        let implicit_clause_warning_count = report
            .diagnostics
            .iter()
            .filter(|(_, d)| matches!(d.issue(), TyperWarning::ImplicitClauseWithoutUsing))
            .count();
        assert_eq!(implicit_clause_warning_count, usize::from(warning));

        if warning {
            let [(source, diagnostic)] = report.diagnostics.as_slice() else {
                panic!("expected one warning: {:?}", report.diagnostics)
            };
            assert_eq!(*source, unit.source());
            assert_eq!(diagnostic.severity(), DiagnosticSeverity::Warning);
            assert_eq!(
                diagnostic.issue().code(),
                "typer.implicit_clause_without_using"
            );
            let start = (text.find("f(1)").unwrap() + 2) as u32;
            assert_eq!(diagnostic.span(), TextRange::new(start, start + 1).unwrap());
        } else {
            assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
        }

        if bad_argument {
            let errors = report.body.unwrap_err();
            let [failure @ dotty::ClassBodyError::Member { error, .. }] = errors.as_slice() else {
                panic!("expected one member error: {errors:?}")
            };
            assert!(
                matches!(error, TyperError::ApplicationArgumentTypeMismatch { actual, expected, .. } if *actual == defs.boolean && *expected == defs.int)
            );
            let diagnostic = failure.diagnostic(|id| (id == unit.source()).then_some(unit.ast()));
            assert_eq!(diagnostic.message, "Expected Int, found Boolean");
            let span = diagnostic.location.expect("argument position");
            assert_eq!(span.source(), unit.source());
            let start = text.find("false").unwrap() as u32;
            assert_eq!(
                span.span().range(),
                TextRange::new(start, start + 5).unwrap()
            );
        } else {
            report.body.expect("complete class body");
        }
    };

    let text = "class C { def f(implicit x: Int): Int = x; def only(n: Int): Int = n; def use(): Int = f(1); def bad(): Int = only(false) }";
    check(text, true, true);
    check(&text.replace("f(1)", "f(using 1)"), false, true);
    let fixed = text.replace("false", "42");
    check(&fixed, true, false);
    let explicit = fixed.replace("f(1)", "f(using 1)");
    check(&explicit, false, false);

    // A warning produced earlier in the same failed method must roll back with it.
    check(
        &explicit.replace("only(42)", "{ val x = f(2); only(false) }"),
        false,
        true,
    );
}

/// The delimiters mark the expected byte range, even when its spelling is repeated.
fn check_body_error(marked: &str, check: impl FnOnce(TyperError, Definitions, &SourceClass)) {
    let (before, rest) = marked.split_once('«').unwrap();
    let (selected, after) = rest.split_once('»').unwrap();
    let text = format!("{before}{selected}{after}");
    let (mut store, defs, packages, unit) = named(&text);
    let units = std::slice::from_ref(&unit);
    complete_signatures(units, &mut store, defs, &packages).unwrap();
    let class = unit.classes().last().unwrap();
    let result = dotty::type_class_body(
        class,
        unit.ast(),
        unit.index(),
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
    assert_eq!(span.source(), unit.source());
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
