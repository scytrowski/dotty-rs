//! Class plans built through the single-dependency client's public API.
use super::{
    NamedClassSources, NamedUnit, class_plans, complete_signatures, name_units, parsing,
    type_bodies,
};
use crate::FixtureMethods;
use dotty::core::{Definitions, Packages, SemanticStore, SourceId, TreeKind};
use dotty::jvm_class_plan::{ClassPlanError, JvmClassPlanner};
use dotty::jvm_method_plan::JvmMethodPlanner;

pub(super) fn named(text: &str) -> (SemanticStore, Definitions, Packages, NamedUnit) {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let files = [(SourceId::from_index(2), "Classes.scala", text)];
    let mut units = name_units(
        &files,
        parsing::parse_files(&files, 1),
        &mut store,
        &mut packages,
    );
    (store, defs, packages, units.pop().unwrap())
}

fn body_files(b_source: &str) -> [(SourceId, &str, &str); 2] {
    [
        (
            SourceId::from_index(0),
            "A.scala",
            "package demo\nclass A { private val seed: Int = 40; def value(): Int = { val n = seed; n } }",
        ),
        (SourceId::from_index(1), "B.scala", b_source),
    ]
}

fn body_units(b_source: &str) -> (SemanticStore, Definitions, Packages, Vec<NamedUnit>) {
    let files = body_files(b_source);
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let units = name_units(
        &files,
        parsing::parse_files(&files, 2),
        &mut store,
        &mut packages,
    );
    (store, defs, packages, units)
}

const VALID_B: &str =
    "package demo\nclass B { private val seed: Int = 7; def value(): Int = { val n = seed; n } }";

#[test]
fn declaration_index_routes_identically_named_fields_to_their_own_class_and_file() {
    use dotty::core::SymbolKind;
    use dotty::{SourceDeclaration, SourceDeclarationIndex};

    let (store, _, _, units) = body_units(VALID_B);
    let index = SourceDeclarationIndex::from_classes(
        &store,
        units.iter().rev().flat_map(|unit| unit.classes()),
    )
    .unwrap();
    let seeds: Vec<_> = units
        .iter()
        .map(|unit| {
            let class = &unit.classes()[0];
            let seed = class.fields().next().unwrap().symbol;
            assert_eq!(
                store.names.resolve(store.symbols.get(seed).name.text()),
                "seed"
            );
            assert_eq!(
                index.get(seed),
                Some(SourceDeclaration {
                    source: unit.source(),
                    class: class.symbol,
                    kind: SymbolKind::Field,
                })
            );
            seed
        })
        .collect();
    assert_ne!(seeds[0], seeds[1]);
    let mut dependencies = dotty::SignatureDependencies::new(dotty::SignaturePhase::Members);
    let blocked = dotty::typer::SignatureCompletion::BlockedOn;
    dependencies.record(seeds[0], blocked(seeds[1]));
    dependencies.record(seeds[1], blocked(seeds[0]));
    assert_eq!(dependencies.waiting_on(seeds[0]), Some(seeds[1]));
    dependencies.record(seeds[0], blocked(seeds[0]));
    assert_eq!(dependencies.waiting_on(seeds[0]), Some(seeds[0]));
    assert_eq!(dependencies.waiting_on(seeds[1]), Some(seeds[0]));
}

#[test]
fn member_signature_rejects_the_other_seed_before_and_after_completion() {
    use dotty::core::SymbolInfo;
    use dotty::typer::SignatureCompletion;
    use dotty::{MemberSignatureError, try_complete_member_signature};

    let (mut store, defs, packages, units) = body_units(VALID_B);
    let sources = NamedClassSources::new(&units, &store).unwrap();
    super::complete_headers(&sources, &mut store, defs, &packages, &mut |_, _| {}).unwrap();
    let (a, b) = (&units[0], &units[1]);
    let seed = a.classes()[0].fields().next().unwrap().symbol;
    let other = b.classes()[0].fields().next().unwrap().symbol;
    for expected in [SymbolInfo::Missing, SymbolInfo::Complete(defs.int)] {
        let checkpoint = store.checkpoint();
        for (ast, index) in [
            (b.ast(), b.index()),
            (b.ast(), a.index()),
            (a.ast(), b.index()),
        ] {
            let error = try_complete_member_signature(
                seed,
                sources.declarations(),
                ast,
                index,
                &mut store,
                defs,
                &packages,
            )
            .unwrap_err();
            assert!(matches!(error, MemberSignatureError::InvalidMember(id) if id == seed));
            let report = error.diagnostic(|_| panic!("invalid member has no source site"));
            assert_eq!(report.location, None);
            assert_eq!(report.message, error.to_string());
            assert_eq!(store.checkpoint(), checkpoint);
            assert_eq!(store.symbols.info(seed), &expected);
            assert_eq!(store.symbols.info(other), &SymbolInfo::Missing);
        }
        assert_eq!(
            try_complete_member_signature(
                seed,
                sources.declarations(),
                a.ast(),
                a.index(),
                &mut store,
                defs,
                &packages
            )
            .unwrap(),
            SignatureCompletion::Complete(defs.int)
        );
    }
}

#[test]
fn bodies_recorded_b_then_a_plan_and_execute_a_then_b() {
    let (mut store, defs, packages, mut units) = body_units(VALID_B);
    let headers = complete_signatures(&units, &mut store, defs, &packages).unwrap();
    units.reverse();
    let (output, bodies) = type_bodies(
        &dotty::NamedClassSources::new(&units, &store).unwrap(),
        &mut store,
        defs,
        &packages,
        headers,
    );
    units.reverse();
    let classes = class_plans(&units, &bodies, &output, &store).unwrap();
    let plan = dotty::jvm::JvmCompilationUnit {
        store: &store,
        typed_ast: &output.arena,
        classes,
    };
    let prepared = plan.prepare_emission(&defs).unwrap();
    let (artifacts, _) = super::emit_batch(&prepared, &plan.classes, 2);
    assert!(
        artifacts
            .iter()
            .map(|c| c.internal_name())
            .eq(["demo/A", "demo/B"])
    );
    assert_eq!(
        super::run_java_client("typed-bodies", "TypedBodiesClient", &artifacts)
            .lines()
            .collect::<Vec<_>>(),
        ["40", "7"]
    );
}

fn assert_error_text(
    sources: &NamedClassSources<'_>,
    error: &dotty::typer::TyperError,
    unit: &NamedUnit,
    text: &str,
    expected: &str,
) {
    let span = dotty::typer_error_span_with(error, |id| sources.file(id).map(|u| u.ast()))
        .expect("source position for typer error");
    assert_eq!(span.source(), unit.source());
    assert_eq!(
        dotty::core::SourceText::new(text)
            .unwrap()
            .slice(span.span().range())
            .unwrap(),
        expected
    );
}

/// All examples put the offending occurrence last, including repeated names.
fn name_error_at_last_use(text: &str, spelling: &str) -> dotty::typer::TyperError {
    let (mut store, defs, packages, unit) = named(text);
    let units = std::slice::from_ref(&unit);
    let error = complete_signatures(units, &mut store, defs, &packages).unwrap_err();
    let super::SignatureError::Member(dotty::MemberSignatureError::Typing { error, .. }) = error
    else {
        panic!("expected a member typing error: {error:?}")
    };
    let sources = NamedClassSources::new(units, &store).unwrap();
    let span = dotty::typer_error_span_with(&error, |id| sources.file(id).map(|u| u.ast()))
        .unwrap_or_else(|| panic!("missing position for {error:?}"));
    let start = text.rfind(spelling).unwrap() as u32;
    assert_eq!(span.source(), unit.source());
    assert_eq!(
        span.span().range(),
        dotty::core::TextRange::new(start, start + spelling.len() as u32).unwrap()
    );
    error
}

#[test]
fn unresolved_names_locate_the_identifier_or_whole_selection() {
    use dotty::typer::TyperError;
    assert!(matches!(
        name_error_at_last_use("class C { def f() = missing + 1 }", "missing"),
        TyperError::TermNameNotFound { .. }
    ));
    assert!(matches!(
        name_error_at_last_use("class C { def f(x: Missing) = 1 }", "Missing"),
        TyperError::TypeNameNotFound { .. }
    ));
    assert!(matches!(
        name_error_at_last_use(
            "class P; class C { def f(obj: P) = obj.missing }",
            "obj.missing"
        ),
        TyperError::MemberNotFound { .. }
    ));
}

#[test]
fn ambiguous_and_wrong_kind_names_locate_the_use() {
    use dotty::typer::TyperError;
    assert!(matches!(
        name_error_at_last_use("class C(xs: Int, xs: Int) { def f() = xs }", "xs"),
        TyperError::AmbiguousTermReference { .. }
    ));
    assert!(matches!(
        name_error_at_last_use("class T; class T; class C { def f(x: T) = 1 }", "T"),
        TyperError::AmbiguousTypeName { .. }
    ));
    assert!(matches!(
        name_error_at_last_use(
            "class C { val marker: Int = 1; def f(x: marker) = 1 }",
            "marker"
        ),
        TyperError::WrongTypeNameKind { .. }
    ));
}

#[test]
fn field_and_local_value_errors_locate_the_initializer() {
    use dotty::typer::TyperError;
    for (text, local) in [
        ("class B { val count: Int = false }", false),
        (
            "class B { def value(): Int = { val count: Int = false; count } }",
            true,
        ),
    ] {
        let (mut store, defs, packages, unit) = named(text);
        let units = std::slice::from_ref(&unit);
        complete_signatures(units, &mut store, defs, &packages).unwrap();
        let error = dotty::type_class_body(
            &unit.classes()[0],
            unit.ast(),
            unit.index(),
            &mut store,
            defs,
            &packages,
            &mut Default::default(),
        )
        .unwrap_err();
        let dotty::ClassBodyError::Member { error, .. } = error else {
            panic!("{error:?}")
        };
        let (actual, expected) = match error {
            TyperError::ExpectedExpressionTypeMismatch {
                actual, expected, ..
            } if !local => (actual, expected),
            TyperError::LocalValueTypeMismatch {
                actual, expected, ..
            } if local => (actual, expected),
            _ => panic!("{error:?}"),
        };
        assert_eq!((actual, expected), (defs.boolean, defs.int));
        assert_error_text(
            &NamedClassSources::new(units, &store).unwrap(),
            &error,
            &unit,
            text,
            "false",
        );
    }
}

#[test]
fn rejected_signature_leaves_only_the_independent_class_emittable() {
    for (initializer, repaired) in [("value(1)", false), ("7", true)] {
        let text =
            format!("package demo\nclass B {{ def value(): Int = 7; val seed = {initializer} }}");
        let [a_file, b_file] = body_files(&text);
        for files in [[a_file, b_file], [b_file, a_file]] {
            let mut store = SemanticStore::new();
            let defs = Definitions::bootstrap(&mut store);
            let mut packages = Packages::new();
            let units = name_units(
                &files,
                parsing::parse_files(&files, 2),
                &mut store,
                &mut packages,
            );
            let sources = NamedClassSources::new(&units, &store).unwrap();
            let a = sources.file(a_file.0).unwrap();
            let b = &sources.file(b_file.0).unwrap().classes()[0];
            let mut output =
                super::complete_headers(&sources, &mut store, defs, &packages, &mut |_, _| {})
                    .unwrap();
            let selection = sources
                .member_signatures(output.ready.iter().copied(), &store)
                .unwrap();
            let mut phase =
                dotty::MemberSignaturePhase::new(&selection, &mut store, defs, &packages);
            for (_, c) in sources.classes() {
                for method in c.methods() {
                    assert!(matches!(
                        phase.attempt(method.symbol).unwrap().completion,
                        dotty::typer::SignatureCompletion::Complete(_)
                    ));
                }
            }
            let broken = b.fields().next().unwrap().symbol;
            assert_eq!(phase.attempt(broken).is_ok(), repaired);
            let seed = a.classes()[0].fields().next().unwrap().symbol;
            assert!(!phase.is_ready(seed));
            assert_eq!(
                phase.attempt(seed).unwrap().completion,
                dotty::typer::SignatureCompletion::Complete(defs.int)
            );
            let (report, work) = phase.finish_with_body_work();
            if repaired {
                assert!(report.failures.is_empty());
            } else {
                assert!(
                    matches!(report.failures.as_slice(), [dotty::MemberSignatureFailure::Typing { report, .. }] if report.member() == Some(broken))
                );
            }
            let mut bodies = dotty::CheckedClassBodies::default();
            for (_, c) in sources.classes() {
                let (body, report) = work.check(
                    c.symbol,
                    &mut store,
                    defs,
                    &packages,
                    &mut output.arena,
                    dotty::core::NoResolver,
                );
                assert!(report.errors.is_empty());
                bodies.record(body).unwrap();
            }
            let selected = if repaired {
                units.as_slice()
            } else {
                assert_eq!(
                    bodies.complete(b.symbol).unwrap_err(),
                    dotty::CheckedBodyError::Partial(b.symbol)
                );
                std::slice::from_ref(a)
            };
            let plan = dotty::jvm::JvmCompilationUnit {
                store: &store,
                typed_ast: &output.arena,
                classes: class_plans(selected, &bodies, &output, &store).unwrap(),
            };
            let prepared = plan.prepare_emission(&defs).unwrap();
            let artifacts: Vec<_> = plan
                .classes
                .iter()
                .map(|class| prepared.emit_class(class.class).unwrap())
                .collect();
            let (client, expected) = if repaired {
                ("TypedBodiesClient", "40\n7")
            } else {
                ("SurvivingClassClient", "40")
            };
            assert_eq!(
                super::run_java_client("surviving-signature", client, &artifacts).trim(),
                expected
            );
        }
    }
}

#[test]
fn method_error_keeps_a_executable_and_b_unpublished() {
    let text = "package demo\nclass B { private val seed: Int = 7; def value(): Int = true }";
    let [a_file, b_file] = body_files(text);
    for files in [[a_file, b_file], [b_file, a_file]] {
        let (report, owner, member, boolean, int) = {
            let mut store = SemanticStore::new();
            let defs = Definitions::bootstrap(&mut store);
            let mut packages = Packages::new();
            let units = name_units(
                &files,
                parsing::parse_files(&files, 2),
                &mut store,
                &mut packages,
            );
            let sources = NamedClassSources::new(&units, &store).unwrap();
            let a = sources.file(a_file.0).unwrap();
            let b = &sources.file(b_file.0).unwrap().classes()[0];
            let method = b.methods().next().unwrap();
            let headers = complete_signatures(&units, &mut store, defs, &packages).unwrap();
            let (mut output, mut bodies) = type_bodies(
                &NamedClassSources::new([a], &store).unwrap(),
                &mut store,
                defs,
                &packages,
                headers,
            );
            let failure = sources
                .type_class_body(
                    b.symbol,
                    &mut store,
                    defs,
                    &packages,
                    &mut output.arena,
                    dotty::core::NoResolver,
                )
                .map(|body| {
                    bodies
                        .record(dotty::CheckedClassBody::Complete(body))
                        .unwrap()
                })
                .unwrap_err();
            let report = failure
                .into_reported_error(|id| sources.file(id).map(NamedUnit::ast))
                .unwrap();
            assert_eq!(dotty::typer_error_span(report.error(), a.ast()), None);
            assert_eq!(
                bodies.complete(b.symbol).unwrap_err(),
                dotty::CheckedBodyError::Missing(b.symbol)
            );
            let plan = dotty::jvm::JvmCompilationUnit {
                store: &store,
                typed_ast: &output.arena,
                classes: class_plans(std::slice::from_ref(a), &bodies, &output, &store).unwrap(),
            };
            let artifact = plan
                .prepare_emission(&defs)
                .unwrap()
                .emit_class(a.classes()[0].symbol)
                .unwrap();
            assert_eq!(
                super::run_java_client("surviving-body", "SurvivingClassClient", &[artifact])
                    .trim(),
                "40"
            );
            (report, b.symbol, method.symbol, defs.boolean, defs.int)
        };
        assert_eq!(
            (report.class(), report.member()),
            (Some(owner), Some(member))
        );
        assert!(
            matches!(report.error(),dotty::typer::TyperError::ExpectedExpressionTypeMismatch{actual,expected,..} if *actual == boolean && *expected == int)
        );
        assert_eq!(
            report.code(),
            Some("typer.expected_expression_type_mismatch")
        );
        let span = report.location().unwrap();
        assert_eq!(span.source(), b_file.0);
        let range = span.span().range();
        assert_eq!(&text[range.start() as usize..range.end() as usize], "true");
    }
}

#[test]
fn class_body_checks_interleaved_members_and_keeps_successes_after_a_failure() {
    use dotty::{ClassBodyError, core::NoResolver, typer::TyperError};

    let (mut store, defs, packages, unit) = named(
        "class C { val bad: Int = false; def good(): Int = 7; def wrong(): Int = true; val last: Boolean = 3 }",
    );
    let mut headers =
        complete_signatures(std::slice::from_ref(&unit), &mut store, defs, &packages).unwrap();
    let class = &unit.classes()[0];
    let before = headers.arena.checkpoint();
    let first = dotty::type_class_body(
        class,
        unit.ast(),
        unit.index(),
        &mut store,
        defs,
        &packages,
        &mut headers.arena,
    )
    .unwrap_err();
    assert!(matches!(first, ClassBodyError::Member { member, .. } if member == class.members[0]));
    assert_eq!(headers.arena.checkpoint(), before);

    let errors = dotty::check_class_body(
        class,
        unit.ast(),
        unit.index(),
        &mut store,
        defs,
        &packages,
        &mut headers.arena,
        NoResolver,
    )
    .body
    .unwrap_err();
    let expected = [
        (0, defs.boolean, defs.int),
        (2, defs.boolean, defs.int),
        (3, defs.int, defs.boolean),
    ];
    assert_eq!(errors.len(), expected.len());
    for (error, (index, found, required)) in errors.into_iter().zip(expected) {
        let ClassBodyError::Member {
            class: owner,
            member,
            error:
                TyperError::ExpectedExpressionTypeMismatch {
                    actual, expected, ..
                },
        } = error
        else {
            panic!("unexpected member error: {error:?}");
        };
        assert_eq!(
            (owner, member, actual, expected),
            (class.symbol, class.members[index], found, required)
        );
    }
    let typed_methods: Vec<_> = headers
        .arena
        .iter()
        .filter_map(|(_, tree)| match &tree.kind {
            TreeKind::DefDef(method) => Some(store.names.resolve(method.name.as_name().text())),
            _ => None,
        })
        .collect();
    assert_eq!(typed_methods, ["good"]);
}

// These two regressions deliberately corrupt a caller-owned declaration view.
fn editable_classes(
    unit: &NamedUnit,
    store: &SemanticStore,
) -> Vec<dotty::source_declarations::SourceClass> {
    dotty::source_declarations::source_classes(
        unit.ast(),
        unit.named().syntax().root,
        unit.source(),
        unit.index(),
        store,
    )
    .unwrap()
}

#[test]
fn type_class_body_rejects_substituted_member_before_typing() {
    let (mut store, defs, packages, unit) =
        named("class A { def value(): Int = 40 }; class B { def value(): Int = 7 }");
    let mut classes = editable_classes(&unit, &store);
    classes[0].members[0] = classes[1].members[0];
    let a = &classes[0];
    let mut arena = dotty::core::AstArena::new();
    let before = arena.checkpoint();
    let error = dotty::type_class_body(
        a,
        unit.ast(),
        unit.index(),
        &mut store,
        defs,
        &packages,
        &mut arena,
    )
    .unwrap_err();
    assert!(matches!(error, dotty::ClassBodyError::InvalidClass(class) if class == a.symbol));
    assert_eq!(arena.checkpoint(), before);
    let report = dotty::check_class_body(
        a,
        unit.ast(),
        unit.index(),
        &mut store,
        defs,
        &packages,
        &mut arena,
        dotty::core::NoResolver,
    );
    assert!(report.diagnostics.is_empty());
    let errors = report.body.unwrap_err();
    assert!(
        matches!(errors.as_slice(), [dotty::ClassBodyError::InvalidClass(id)] if *id == a.symbol)
    );
    assert_eq!(arena.checkpoint(), before);
}

#[test]
fn planning_requires_b_and_recording_a_twice_preserves_the_first_body() {
    let (mut store, defs, packages, units) = body_units(VALID_B);
    let headers = complete_signatures(&units, &mut store, defs, &packages).unwrap();
    let (mut output, mut bodies) = type_bodies(
        &dotty::NamedClassSources::new(&units[..1], &store).unwrap(),
        &mut store,
        defs,
        &packages,
        headers,
    );
    let unit = &units[0];
    let a = &unit.classes()[0];
    let b = &units[1].classes()[0];
    assert_eq!(
        class_plans(&units, &bodies, &output, &store),
        Err(super::PlanningError::Body(
            dotty::CheckedBodyError::Missing(b.symbol)
        ))
    );

    // A second output for A has no method mapping. Replacing the first would break planning.
    let typer = dotty::typer::SourceTyper::new_with_typed_ast(
        unit.ast(),
        unit.source(),
        unit.index(),
        &mut store,
        defs,
        &packages,
        output.arena,
    );
    let (arena, duplicate) =
        dotty::jvm_class_plan::TypedClassBody::from_output(a, typer.into_typed_output());
    output.arena = arena;
    assert_eq!(
        bodies.record(dotty::CheckedClassBody::Complete(duplicate)),
        Err(dotty::CheckedBodyError::Duplicate(a.symbol))
    );
    let plans = class_plans(&units[..1], &bodies, &output, &store).unwrap();
    assert_eq!(plans[0].methods[0].method, unit.method_symbol());
}

#[test]
fn deferred_plans_keep_each_class_body_and_locals_after_extending_the_arena() {
    use dotty::core::AstArena;
    use dotty::jvm_class_plan::TypedClassBody;
    use dotty::typer::SourceTyper;

    let (mut store, defs, packages, unit) = named(
        "class A { private val seed: Int = 40; def value(): Int = { val n = seed; n }; def other(): Int = { val n = 9; n } }
         class B { private val seed: Int = 7; def value(): Int = { var n = seed; n } }",
    );
    let mut arena = AstArena::new();
    let mut bodies = Vec::new();
    for class in unit.classes() {
        let mut typer = SourceTyper::new_with_typed_ast(
            unit.ast(),
            unit.source(),
            unit.index(),
            &mut store,
            defs,
            &packages,
            arena,
        );
        let field = class.fields().next().unwrap().symbol;
        let initializer = typer.type_field_initializer(field).unwrap();
        let mut methods = Vec::new();
        for method in class.methods() {
            let root = typer.type_method_definition(method.tree).unwrap();
            let TreeKind::DefDef(def) = &typer.typed_ast().get(root).kind else {
                panic!("expected a method")
            };
            methods.push((method.symbol, def.rhs.unwrap()));
        }
        let (extended, body) = TypedClassBody::from_output(class, typer.into_typed_output());
        arena = extended;
        bodies.push((body, field, initializer, methods));
    }

    let planner = JvmClassPlanner::new(&store, unit.ast(), unit.index());
    for (i, (body, field, initializer, methods)) in bodies.iter().enumerate().rev() {
        let plan = planner
            .plan_typed_class(&unit.classes()[i], &arena, body)
            .unwrap();
        assert_eq!(
            (plan.fields[0].field, plan.fields[0].initializer),
            (*field, Some(*initializer))
        );
        assert_eq!(plan.methods.len(), methods.len());
        for (planned, &(method, rhs)) in plan.methods.iter().zip(methods) {
            assert_eq!((planned.method, planned.body), (method, Some(rhs)));
            let [local] = &planned.locals[..] else {
                panic!("expected one local")
            };
            assert_eq!(store.symbols.owner(local.symbol), Some(method));
            let TreeKind::Block(block) = &arena.get(rhs).kind else {
                panic!("expected a block")
            };
            assert_eq!(block.stats, [local.declaration]);
            assert_eq!(local.mutable, i == 1);
        }
    }
    let [a, b] = unit.classes() else {
        panic!("expected two classes")
    };
    assert_eq!(
        planner.plan_typed_class(a, &arena, &bodies[1].0),
        Err(ClassPlanError::ForeignClassBody {
            expected: a.symbol,
            actual: b.symbol
        })
    );
}

#[test]
fn typed_class_requires_canonical_method_mappings_even_with_matching_class_label() {
    use dotty::jvm_class_plan::TypedClassBody;
    use dotty::jvm_method_plan::MethodPlanError;
    let (mut store, defs, packages, unit) =
        named("class A { def value(): Int = 40 }; class B { def value(): Int = 7 }");
    let [a, b] = unit.classes() else {
        panic!("expected two classes")
    };
    let mut typer = unit.typer(&mut store, defs, &packages);
    typer
        .type_method_definition(b.methods().next().unwrap().tree)
        .unwrap();
    let (arena, mislabeled) = TypedClassBody::from_output(a, typer.into_typed_output());
    assert_eq!(
        JvmClassPlanner::new(&store, unit.ast(), unit.index()).plan_typed_class(
            a,
            &arena,
            &mislabeled
        ),
        Err(ClassPlanError::MethodPlan(MethodPlanError::TypedMethod(
            a.methods().next().unwrap().symbol
        )))
    );
}

#[test]
fn class_plans_require_exact_methods_and_restore_source_order() {
    let (mut store, defs, packages, unit) = named(
        "class A { def value(): Int = 17; def other(): Int = 5 }\nclass B { def value(): Int = 42 }",
    );
    let class = unit.classes()[0].symbol;
    let mut typer = unit.typer(&mut store, defs, &packages);
    for method in unit.methods() {
        typer.type_method_definition(method.tree).unwrap();
    }
    let output = typer.into_typed_output();
    let method_planner = JvmMethodPlanner::new(&store, unit.ast(), unit.index(), &output);
    let plans: Vec<_> = unit
        .method_symbols()
        .map(|method| method_planner.plan(method).unwrap())
        .collect();
    let [first, second, foreign] = &plans[..] else {
        panic!("expected three methods")
    };
    let planner = JvmClassPlanner::new(&store, unit.ast(), unit.index());

    assert_eq!(
        planner.plan(class, [foreign.clone(), second.clone()]),
        Err(ClassPlanError::UnexpectedMethod {
            class,
            method: foreign.method
        })
    );
    assert_eq!(
        planner.plan(class, [first.clone()]),
        Err(ClassPlanError::MissingMethod {
            class,
            method: second.method
        })
    );
    assert_eq!(
        planner.plan(class, [first.clone(), first.clone()]),
        Err(ClassPlanError::DuplicateMethod {
            class,
            method: first.method
        })
    );
    let plan = planner
        .plan(class, [second.clone(), first.clone()])
        .unwrap();
    assert_eq!(plan.methods, [first.clone(), second.clone()]);
}

#[test]
fn ordinary_constructor_parameters_initialize_instance_state_in_source_order_on_jdk21() {
    use dotty::classfile::{ACC_PUBLIC, ClassFile, MethodAccessFlags, Reader};
    let inputs = [
        (
            SourceId::from_index(0),
            "Counter.scala",
            r#"package demo
class Counter(start: Int):
  private var current: Int = start
  def value(): Int = current
  def add(delta: Int): Unit = current = current + delta
"#,
        ),
        (
            SourceId::from_index(1),
            "Pair.scala",
            r#"package demo
class Pair(z: Int, a: Int):
  private val second: Int = a
  private val first: Int = z
  def difference(): Int = first - second
"#,
        ),
    ];
    let compile = |files: [_; 2]| {
        super::with_plan(&files, |plan, defs, sources, bodies, _| {
            let (id, _, text) = inputs[0];
            let field = sources.file(id).unwrap().classes()[0]
                .fields()
                .next()
                .unwrap()
                .symbol;
            let index =
                dotty::ResolvedMemberIndex::build(sources, bodies, plan.typed_ast, plan.store);
            assert!(index.diagnostics().is_empty());
            let offsets: Vec<_> = text
                .match_indices("current")
                .map(|(i, _)| i as u32)
                .collect();
            let [declaration, read, write, rhs] = offsets[..] else {
                panic!("Counter field sites")
            };
            let sites = index.references_to(field);
            assert_eq!(sites.len(), 3);
            for (site, (offset, kind)) in sites.iter().zip([
                (read, dotty::FieldAccessKind::Read),
                (write, dotty::FieldAccessKind::Write),
                (rhs, dotty::FieldAccessKind::Read),
            ]) {
                let access = sources
                    .resolved_field_at(id, offset, bodies, plan.typed_ast, plan.store)
                    .unwrap()
                    .unwrap();
                assert_eq!(index.field_at(id, offset), Some(&access));
                assert_eq!(
                    (
                        site.source(),
                        site.span().range().start(),
                        access.field,
                        access.kind,
                        access.target.name.start()
                    ),
                    (id, offset, field, kind, declaration)
                );
            }
            assert!(
                index
                    .member_at(id, text.rfind("start").unwrap() as u32)
                    .is_none()
            );
            super::checked_emission(plan, defs)
        })
    };
    let artifacts = compile(inputs);
    let reversed = compile([inputs[1], inputs[0]]);
    assert!(artifacts.iter().eq(reversed.iter().rev()));
    let counter = artifacts
        .iter()
        .find(|a| a.internal_name() == "demo/Counter")
        .unwrap();
    let decoded = ClassFile::decode(&mut Reader::new(counter.bytes())).unwrap();
    let pool = &decoded.constant_pool;
    let constructors: Vec<_> = decoded
        .methods
        .iter()
        .filter(|m| pool.utf8(m.name_index).unwrap() == "<init>")
        .map(|m| (pool.utf8(m.descriptor_index).unwrap(), m.access_flags))
        .collect();
    assert_eq!(constructors, [("(I)V", MethodAccessFlags(ACC_PUBLIC))]);
    let fields: Vec<_> = decoded
        .fields
        .iter()
        .map(|f| pool.utf8(f.name_index).unwrap())
        .collect();
    assert_eq!(fields, ["current"]);
    assert_eq!(
        super::run_java_client("primary-constructor", "ConstructorClient", &artifacts)
            .lines()
            .collect::<Vec<_>>(),
        ["40", "7", "42", "7", "5"]
    );
}

#[test]
fn constructor_accessors_have_the_scala_jvm_abi_and_work_across_sources() {
    let counter = (
        SourceId::from_index(0),
        "Counter.scala",
        "package demo\nclass Counter(val id: Int, var count: Int)",
    );
    let use_counter = (
        SourceId::from_index(1),
        "UseCounter.scala",
        r#"package demo
class UseCounter:
  def create(id: Int, count: Int): Counter = new Counter(id, count)
  def read(c: Counter): Int = c.count
  def update(c: Counter, next: Int): Unit = c.count = next
"#,
    );
    let artifacts = compile_counter([counter, use_counter]);
    let reversed = compile_counter([use_counter, counter]);
    assert!(artifacts.iter().eq(reversed.iter().rev()));
    assert_counter_abi(&artifacts, "id", "(II)V");
    assert_eq!(
        super::run_java_client("constructor-accessors", "AccessorClient", &artifacts)
            .lines()
            .collect::<Vec<_>>(),
        ["9", "4", "7", "2"]
    );
}

fn compile_counter(files: [(SourceId, &str, &str); 2]) -> Vec<dotty::jvm::GeneratedClass> {
    let &(counter, _, declaration) = files.iter().find(|f| f.1 == "Counter.scala").unwrap();
    let &(client, _, text) = files.iter().find(|f| f.1 == "UseCounter.scala").unwrap();
    super::with_plan(&files, |plan, definitions, sources, bodies, _| {
        let class = &sources.file(counter).unwrap().classes()[0];
        let field = class
            .constructor_fields
            .iter()
            .copied()
            .chain(class.fields().map(|f| f.symbol))
            .find(|&id| {
                plan.store
                    .names
                    .resolve(plan.store.symbols.get(id).name.text())
                    == "count"
            })
            .unwrap();
        let start = declaration.find("var count").unwrap() as u32 + 4;
        let sites: Vec<_> = text.match_indices("c.count").collect();
        assert_eq!(sites.len(), 2);
        for ((offset, _), kind) in sites
            .into_iter()
            .zip([dotty::FieldAccessKind::Read, dotty::FieldAccessKind::Write])
        {
            let access = sources
                .resolved_field_at(
                    client,
                    offset as u32 + 2,
                    bodies,
                    plan.typed_ast,
                    plan.store,
                )
                .unwrap()
                .unwrap();
            assert_eq!((access.field, access.kind), (field, kind));
            assert_eq!(access.origin.source(), client);
            assert_eq!(
                (
                    access.target.source,
                    access.target.name.start(),
                    access.target.name.end()
                ),
                (counter, start, start + 5)
            );
        }
        super::checked_emission(plan, definitions)
    })
}

fn assert_counter_abi(
    artifacts: &[dotty::jvm::GeneratedClass],
    val_name: &str,
    constructor_descriptor: &str,
) {
    use dotty::classfile::{ACC_FINAL, ACC_PRIVATE, ACC_PUBLIC, ClassFile, Reader};

    let artifact = artifacts
        .iter()
        .find(|class| class.internal_name() == "demo/Counter")
        .unwrap();
    let decoded = ClassFile::decode(&mut Reader::new(artifact.bytes())).unwrap();
    let pool = &decoded.constant_pool;
    let fields: Vec<_> = decoded
        .fields
        .iter()
        .map(|field| {
            (
                pool.utf8(field.name_index).unwrap(),
                pool.utf8(field.descriptor_index).unwrap(),
                field.access_flags.0,
            )
        })
        .collect();
    assert_eq!(
        fields,
        [
            (val_name, "I", ACC_PRIVATE | ACC_FINAL),
            ("count", "I", ACC_PRIVATE)
        ]
    );
    let mut methods: Vec<_> = decoded
        .methods
        .iter()
        .map(|method| {
            (
                pool.utf8(method.name_index).unwrap(),
                pool.utf8(method.descriptor_index).unwrap(),
                method.access_flags.0,
            )
        })
        .collect();
    let mut expected_methods = [
        ("<init>", constructor_descriptor, ACC_PUBLIC),
        ("count", "()I", ACC_PUBLIC),
        ("count_$eq", "(I)V", ACC_PUBLIC),
        (val_name, "()I", ACC_PUBLIC),
    ];
    methods.sort_unstable();
    expected_methods.sort_unstable();
    assert_eq!(methods, expected_methods);
}

const BODY_COUNTER: &str = "package demo
class Counter(start: Int):
  val initial: Int = start
  var count: Int = start
";

fn field_signature_units(
    consumer: &str,
    provider: &str,
    provider_first: bool,
    store: &mut SemanticStore,
    packages: &mut Packages,
) -> Vec<NamedUnit> {
    let mut files = [
        (SourceId::from_index(3), "Consumer.scala", consumer),
        (SourceId::from_index(8), "Provider.scala", provider),
    ];
    if provider_first {
        files.reverse();
    }
    name_units(&files, parsing::parse_files(&files, 2), store, packages)
}

#[test]
fn field_signature_retries_foreign_method_and_infers_int_in_both_orders() {
    use dotty::ClassFieldSignatureCompletion::{BlockedOn, Complete};
    use dotty::core::SymbolInfo;
    let consumer = "class Consumer(val provider: Provider) { private val hidden = 1; val answer = provider.value() }";
    for provider_first in [false, true] {
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let units = field_signature_units(
            consumer,
            "class Provider { def value() = 40 }",
            provider_first,
            &mut store,
            &mut packages,
        );
        let consumer = &units[usize::from(provider_first)];
        let provider = &units[usize::from(!provider_first)];
        let class = &consumer.classes()[0];
        let fields: Vec<_> = class.fields().collect();
        let answer = fields[1].symbol;
        let value = provider.method_symbol();
        super::complete_headers(
            &NamedClassSources::new(&units, &store).unwrap(),
            &mut store,
            defs,
            &packages,
            &mut |_, _| {},
        )
        .unwrap();
        let attempt = |store: &mut SemanticStore, schedule_value: bool| {
            dotty::try_complete_class_field_signatures(
                class.symbol,
                consumer.ast(),
                consumer.index(),
                store,
                defs,
                &packages,
                |dep| schedule_value && dep == value,
            )
        };
        if !provider_first {
            assert_eq!(
                attempt(&mut store, true).unwrap(),
                BlockedOn {
                    field: answer,
                    dependency: value
                }
            );
            let error = attempt(&mut store, false).unwrap_err();
            assert!(matches!(error, dotty::ClassFieldSignatureError::Field {
                field, error: dotty::typer::TyperError::MemberTypeUnavailable { symbol }, ..
            } if field == answer && symbol == value));
            assert_eq!(store.symbols.info(answer), &SymbolInfo::Missing);
            assert!(matches!(
                store.symbols.info(class.constructor_fields[0]),
                SymbolInfo::Complete(_)
            ));
            // The field itself must make the client schedule value and retry answer.
            super::complete_methods_in_order(
                &NamedClassSources::new(&units, &store).unwrap(),
                [],
                &mut store,
                defs,
                &packages,
                |_, _| {},
            )
            .unwrap();
            assert_eq!(store.symbols.info(answer), &SymbolInfo::Complete(defs.int));
        } else {
            provider
                .typer(&mut store, defs, &packages)
                .complete_symbol(value)
                .unwrap();
        }
        assert_eq!(attempt(&mut store, true).unwrap(), Complete);
        assert_eq!(store.symbols.info(answer), &SymbolInfo::Complete(defs.int));
        assert_eq!(store.symbols.info(fields[0].symbol), &SymbolInfo::Missing);
    }
}

const CONSUMER_OF_SEED: &str = "class Consumer(provider: Provider) { val answer = provider.seed }";

#[test]
fn field_method_field_chain_infers_int_and_executes_in_both_file_orders() {
    use dotty::core::{SymbolInfo, Type};
    use dotty::typer::SignatureCompletion;

    let consumer = (
        SourceId::from_index(2),
        "Consumer.scala",
        "class Consumer(provider: Provider) { val answer = provider.value() }",
    );
    let provider = (
        SourceId::from_index(4),
        "Provider.scala",
        "class Provider(val seeds: Seeds) { def value() = seeds.seed }",
    );
    let seeds = (
        SourceId::from_index(6),
        "Seeds.scala",
        "class Seeds { val seed = 40 }; class OtherSeeds { val seed = 7 }",
    );
    for files in [[consumer, provider, seeds], [seeds, provider, consumer]] {
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let units = name_units(
            &files,
            parsing::parse_files(&files, 3),
            &mut store,
            &mut packages,
        );
        let consumer_first = files[0].0 == consumer.0;
        let (caller, leaves) = if consumer_first {
            (&units[0], &units[2])
        } else {
            (&units[2], &units[0])
        };
        let answer = caller.classes()[0].fields().next().unwrap().symbol;
        let value = units[1].method_symbol();
        let seed = leaves.classes()[0].fields().next().unwrap().symbol;
        assert_ne!(seed, leaves.classes()[1].fields().next().unwrap().symbol);
        let mut edges = Vec::new();
        let headers = super::complete_signatures_in_order(
            &units,
            &units,
            &mut store,
            defs,
            &packages,
            |symbol, attempt| {
                if let SignatureCompletion::BlockedOn(dep) = attempt {
                    edges.push((symbol, dep));
                }
            },
        )
        .unwrap();
        if consumer_first {
            assert_eq!(edges, [(answer, value), (value, seed)]);
        } else {
            assert_eq!(edges, [(answer, value)]);
        }
        for field in [answer, seed] {
            assert_eq!(store.symbols.info(field), &SymbolInfo::Complete(defs.int));
        }
        let SymbolInfo::Complete(ty) = *store.symbols.info(value) else {
            panic!("missing value signature")
        };
        assert!(matches!(store.types.get(ty), Type::Method(method) if method.result == defs.int));
        let (output, bodies) = type_bodies(
            &dotty::NamedClassSources::new(&units, &store).unwrap(),
            &mut store,
            defs,
            &packages,
            headers,
        );
        let classes = class_plans(&units, &bodies, &output, &store).unwrap();
        let plan = dotty::jvm::JvmCompilationUnit {
            store: &store,
            typed_ast: &output.arena,
            classes,
        };
        let prepared = plan.prepare_emission(&defs).unwrap();
        let (artifacts, _) = super::emit_batch(&prepared, &plan.classes, 2);
        let consumer = artifacts
            .iter()
            .find(|artifact| artifact.internal_name() == "Consumer")
            .unwrap();
        assert_answer_constructor(consumer.bytes());
        assert_eq!(
            super::run_java_client("initializer-call", "InitializerCallClient", &artifacts).trim(),
            "40"
        );
    }
}

#[test]
fn initializer_parameter_call_checks_arity_and_result_type() {
    for (method, initializer, reason) in [
        (
            "def value(n: Int): Int = n",
            "provider.value(1)",
            "non-approved constructor application",
        ),
        (
            "def value(): Boolean = true",
            "provider.value()",
            "expected a public parameterless Int method on the parameter class",
        ),
    ] {
        let provider = format!("class Provider {{ {method} }}");
        let consumer =
            format!("class Consumer(provider: Provider) {{ val answer = {initializer} }}");
        let files = [
            (SourceId::from_index(0), "Provider.scala", provider.as_str()),
            (SourceId::from_index(1), "Consumer.scala", consumer.as_str()),
        ];
        super::with_plan(&files, |unit, defs, _, _, _| {
            let consumer = &unit.classes[1];
            let answer = &consumer.fields[0];
            let error = dotty::jvm::emit_class(unit, consumer.class, defs).unwrap_err();
            assert!(
                matches!(error,
                    dotty::jvm::JvmCodegenError::UnsupportedConstructorFieldInitializer { field, tree, node }
                        if field == answer.field && Some(tree) == answer.initializer && node == reason
                ),
                "{error:?}"
            );
        });
    }
}

fn assert_answer_constructor(bytes: &[u8]) {
    use dotty::classfile::{Attribute, ClassFile, ConstantPoolEntry, ConstantPoolIndex, Reader};

    let class = ClassFile::decode(&mut Reader::new(bytes)).unwrap();
    let pool = &class.constant_pool;
    let ctor = class
        .methods
        .iter()
        .find(|m| pool.utf8(m.name_index) == Ok("<init>"))
        .unwrap();
    assert_eq!(pool.utf8(ctor.descriptor_index), Ok("(LProvider;)V"));
    let [Attribute::Code(code)] = ctor.attributes.as_slice() else {
        panic!("expected constructor code")
    };
    // aload_0, invokespecial, aload_0, aload_1, invokevirtual, putfield, return
    let [
        0x2a,
        0xb7,
        p1,
        p2,
        0x2a,
        0x2b,
        0xb6,
        m1,
        m2,
        0xb5,
        f1,
        f2,
        0xb1,
    ] = *code.code
    else {
        panic!(
            "unexpected Consumer constructor instructions: {:x?}",
            code.code
        )
    };
    let reference = |hi, lo| pool.get(ConstantPoolIndex(u16::from_be_bytes([hi, lo])));
    for (hi, lo, owner, name, descriptor) in [
        (p1, p2, "java/lang/Object", "<init>", "()V"),
        (m1, m2, "Provider", "value", "()I"),
    ] {
        let Some(ConstantPoolEntry::Methodref {
            class_index,
            name_and_type_index,
        }) = reference(hi, lo)
        else {
            panic!("expected method reference")
        };
        assert_eq!(pool.class_name(*class_index), Ok(owner));
        assert_eq!(
            pool.name_and_type(*name_and_type_index),
            Ok((name, descriptor))
        );
    }
    let Some(ConstantPoolEntry::Fieldref {
        class_index,
        name_and_type_index,
    }) = reference(f1, f2)
    else {
        panic!("expected field reference")
    };
    assert_eq!(pool.class_name(*class_index), Ok("Consumer"));
    assert_eq!(
        pool.name_and_type(*name_and_type_index),
        Ok(("answer", "I"))
    );
}

#[test]
fn public_field_dependency_infers_both_signatures_in_either_file_order() {
    use dotty::core::SymbolInfo;
    use dotty::typer::SignatureCompletion;

    for provider_first in [false, true] {
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let units = field_signature_units(
            CONSUMER_OF_SEED,
            "class Provider { val seed = 40 }",
            provider_first,
            &mut store,
            &mut packages,
        );
        let answer = units[usize::from(provider_first)].classes()[0]
            .fields()
            .next()
            .unwrap()
            .symbol;
        let seed = units[usize::from(!provider_first)].classes()[0]
            .fields()
            .next()
            .unwrap()
            .symbol;
        let mut edges = Vec::new();
        super::complete_signatures_in_order(
            &units,
            &units,
            &mut store,
            defs,
            &packages,
            |symbol, attempt| {
                if let SignatureCompletion::BlockedOn(dependency) = attempt {
                    edges.push((symbol, dependency));
                }
            },
        )
        .unwrap();
        if provider_first {
            assert!(edges.is_empty());
        } else {
            assert_eq!(edges, [(answer, seed)]);
        }
        for field in [seed, answer] {
            assert_eq!(store.symbols.info(field), &SymbolInfo::Complete(defs.int));
        }
    }
}

#[test]
fn public_field_dependency_outside_the_batch_keeps_the_original_error() {
    use dotty::core::SymbolInfo;
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let units = field_signature_units(
        CONSUMER_OF_SEED,
        "class Provider { val seed = 40 }",
        false,
        &mut store,
        &mut packages,
    );
    super::complete_headers(
        &NamedClassSources::new(&units, &store).unwrap(),
        &mut store,
        defs,
        &packages,
        &mut |_, _| {},
    )
    .unwrap();
    let consumer = &units[0];
    let class = &consumer.classes()[0];
    let answer = class.fields().next().unwrap().symbol;
    let seed = units[1].classes()[0].fields().next().unwrap().symbol;
    let error = dotty::try_complete_class_field_signatures(
        class.symbol,
        consumer.ast(),
        consumer.index(),
        &mut store,
        defs,
        &packages,
        |dep| dep != seed,
    )
    .unwrap_err();
    assert!(
        matches!(error, dotty::ClassFieldSignatureError::Field { class: owner, field,
        error: dotty::typer::TyperError::MemberTypeUnavailable { symbol }
    } if owner == class.symbol && field == answer && symbol == seed)
    );
    let batch = dotty::SourceDeclarationIndex::from_classes(&store, [class]).unwrap();
    let checkpoint = store.checkpoint();
    let error = dotty::try_complete_member_signature(
        answer,
        &batch,
        consumer.ast(),
        consumer.index(),
        &mut store,
        defs,
        &packages,
    )
    .unwrap_err();
    assert!(
        matches!(error, dotty::MemberSignatureError::Typing { member, declaration,
        error: dotty::typer::TyperError::MemberTypeUnavailable { symbol }
    } if member == answer && declaration.class == class.symbol && symbol == seed)
    );
    assert_eq!(store.checkpoint(), checkpoint);
    for field in [answer, seed] {
        assert_eq!(store.symbols.info(field), &SymbolInfo::Missing);
    }
}

#[test]
fn public_field_dependency_completes_only_the_requested_provider_field() {
    use dotty::core::SymbolInfo;
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let units = field_signature_units(
        CONSUMER_OF_SEED,
        "class Provider(consumer: Consumer) { val echo = consumer.answer; val seed = 40 }",
        false,
        &mut store,
        &mut packages,
    );
    // answer needs seed. Starting the entire Provider class here would revisit answer via echo.
    complete_signatures(&units, &mut store, defs, &packages).unwrap();
    for unit in &units {
        for field in unit.classes()[0].fields() {
            assert_eq!(
                store.symbols.info(field.symbol),
                &SymbolInfo::Complete(defs.int)
            );
        }
    }
}

#[test]
fn public_field_cycle_reports_the_closed_symbol_path_in_either_order() {
    use dotty::core::SymbolInfo;
    for provider_first in [false, true] {
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let units = field_signature_units(
            CONSUMER_OF_SEED,
            "class Provider(consumer: Consumer) { val seed = consumer.answer }",
            provider_first,
            &mut store,
            &mut packages,
        );
        let first = units[0].classes()[0].fields().next().unwrap().symbol;
        let second = units[1].classes()[0].fields().next().unwrap().symbol;
        let error = complete_signatures(&units, &mut store, defs, &packages).unwrap_err();
        let super::SignatureError::Cycle(cycle) = error else {
            panic!("expected a field cycle, got {error:?}")
        };
        assert_eq!(cycle.code(), "signature.member_cycle");
        assert_eq!(cycle.path(), [first, second, first]);
        let locations: Vec<_> = cycle.declarations(&store).collect();
        assert_eq!(
            locations,
            [
                (first, store.symbols.get(first).position),
                (second, store.symbols.get(second).position)
            ]
        );
        assert!(locations.iter().all(|(_, position)| position.is_some()));
        for field in [first, second] {
            assert_eq!(store.symbols.info(field), &SymbolInfo::Missing);
        }
    }
}

#[test]
fn wrong_second_method_argument_points_to_true_in_both_file_orders() {
    let text = "class Consumer { def run(provider: Provider) = provider.diff(9, true) }";
    for provider_first in [false, true] {
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let units = field_signature_units(
            text,
            "class Provider { def diff(x: Int, y: Int): Int = x - y }",
            provider_first,
            &mut store,
            &mut packages,
        );
        let consumer = &units[usize::from(provider_first)];
        let provider = &units[usize::from(!provider_first)];
        let error = complete_signatures(&units, &mut store, defs, &packages).unwrap_err();
        let super::SignatureError::Member(failure) = error else {
            panic!("expected a member signature error, got {error:?}");
        };
        let sources = NamedClassSources::new(&units, &store).unwrap();
        let diagnostic = failure.diagnostic(|id| sources.file(id).map(|u| u.ast()));
        assert_eq!(diagnostic.message, "Expected Int, found Boolean");
        assert_eq!(failure.diagnostic(|_| Some(provider.ast())).location, None);
        let dotty::MemberSignatureError::Typing { member, error, .. } = failure else {
            panic!("expected a member typing error");
        };
        assert_eq!(member, consumer.method_symbol());
        assert!(
            matches!(error, dotty::typer::TyperError::ApplicationArgumentTypeMismatch {
            argument_index: 1, actual, expected, ..
        } if actual == defs.boolean && expected == defs.int),
            "{error:?}"
        );
        let span = diagnostic.location.unwrap();
        assert_eq!(span.source(), consumer.source());
        assert_eq!(
            dotty::core::SourceText::new(text)
                .unwrap()
                .slice(span.span().range())
                .unwrap(),
            "true"
        );
        assert_eq!(dotty::typer_error_span(&error, provider.ast()), None);
    }
}

#[test]
fn field_scheduler_reports_the_bad_call_in_consumer_in_both_orders() {
    use dotty::core::{SourceText, SymbolInfo};
    use dotty::typer::{SignatureCompletion, TyperError};

    let consumer_file = (
        SourceId::from_index(3),
        "Consumer.scala",
        "// źródło\nclass Consumer(provider: Provider) { val answer = provider.value(1) }",
    );
    let provider_file = (
        SourceId::from_index(8),
        "Provider.scala",
        "class Provider { def value() = { val seed = 40; val n = seed; n } }",
    );
    for files in [
        [consumer_file, provider_file],
        [provider_file, consumer_file],
    ] {
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let units = name_units(
            &files,
            parsing::parse_files(&files, 2),
            &mut store,
            &mut packages,
        );
        let sources = NamedClassSources::new(&units, &store).unwrap();
        let consumer = sources.file(consumer_file.0).unwrap();
        let provider = sources.file(provider_file.0).unwrap();
        let class = &consumer.classes()[0];
        let answer = class.fields().next().unwrap();
        let value = provider.method_symbol();
        let TreeKind::ValDef(def) = &consumer.ast().get(answer.tree).kind else {
            panic!("expected answer field")
        };
        let rhs = def.rhs.unwrap();
        // Both files have a positioned node in the slot reported for the failed call.
        assert_eq!(
            provider.ast().get(rhs).position.unwrap().source(),
            provider.source()
        );

        let mut edges = Vec::new();
        let error = super::complete_methods_in_order(
            &sources,
            [],
            &mut store,
            defs,
            &packages,
            |symbol, result| {
                if let SignatureCompletion::BlockedOn(dep) = result {
                    edges.push((symbol, dep));
                }
            },
        )
        .unwrap_err();
        assert_eq!(edges, [(answer.symbol, value)]);
        let super::SignatureError::Member(failure) = error else {
            panic!("unexpected signature error for answer: {error:?}");
        };
        let ast_for = |id| sources.file(id).map(|u| u.ast());
        let diagnostic = failure.diagnostic(ast_for);
        assert_eq!(diagnostic.severity, dotty::core::DiagnosticSeverity::Error);
        let span = diagnostic.location.expect("source of the failed call");
        let (_, filename, text) = files
            .iter()
            .find(|(id, _, _)| *id == span.source())
            .unwrap();
        let call = SourceText::new(text)
            .unwrap()
            .slice(span.span().range())
            .unwrap();
        assert_eq!(
            (*filename, call, diagnostic.message.as_str()),
            (
                "Consumer.scala",
                "provider.value(1)",
                "Expected 0 arguments, got 1"
            )
        );
        for ast in [Some(provider.ast()), None] {
            let unlocated = failure.diagnostic(|_| ast);
            assert_eq!(unlocated.location, None);
            assert_eq!(unlocated.message, diagnostic.message);
        }
        let dotty::MemberSignatureError::Typing {
            declaration,
            member,
            error: member_error,
        } = failure
        else {
            panic!("expected a member typing error");
        };
        assert_eq!(
            declaration,
            dotty::SourceDeclaration {
                class: class.symbol,
                source: consumer.source(),
                kind: dotty::core::SymbolKind::Field,
            }
        );
        assert_eq!(member, answer.symbol);
        assert!(matches!(store.symbols.info(value), SymbolInfo::Complete(_)));
        assert_eq!(store.symbols.info(answer.symbol), &SymbolInfo::Missing);

        // With the dependency complete, the class field pass encounters the same call.
        let failure = dotty::try_complete_class_field_signatures(
            class.symbol,
            consumer.ast(),
            consumer.index(),
            &mut store,
            defs,
            &packages,
            |dep| sources.declarations().member(dep).is_some(),
        )
        .unwrap_err();
        assert_eq!(failure.diagnostic(ast_for), diagnostic);
        let dotty::ClassFieldSignatureError::Field {
            class: owner,
            field,
            error: field_error,
        } = failure
        else {
            panic!("expected a field typing failure");
        };
        assert_eq!((owner, field), (class.symbol, answer.symbol));
        for error in [member_error, field_error] {
            let TyperError::ApplicationArityMismatch {
                source,
                tree_index,
                expected,
                actual,
            } = &error
            else {
                panic!("expected a call arity error: {error:?}")
            };
            assert_eq!((*source, *tree_index), (consumer.source(), rhs.index()));
            assert_eq!(dotty::typer_error_span(&error, provider.ast()), None);
            assert_eq!((*expected, *actual), (0, 1));
            assert_eq!(
                dotty::typer_error_span_with(&error, |id| sources.file(id).map(|u| u.ast())),
                Some(span)
            );
        }
    }
}

#[test]
fn public_body_fields_emit_accessors_and_work_in_both_file_orders_on_jdk21() {
    let counter = (SourceId::from_index(0), "Counter.scala", BODY_COUNTER);
    let use_counter = (
        SourceId::from_index(1),
        "UseCounter.scala",
        "package demo
class UseCounter:
  def create(start: Int): Counter = new Counter(start)
  def initial(c: Counter): Int = c.initial
  def read(c: Counter): Int = c.count
  def update(c: Counter, next: Int): Unit = c.count = next
",
    );
    let first = compile_counter([counter, use_counter]);
    let second = compile_counter([use_counter, counter]);
    assert!(first.iter().eq(second.iter().rev()));
    for artifacts in [first, second] {
        assert_counter_abi(&artifacts, "initial", "(I)V");
        assert_eq!(
            super::run_java_client("public-body-fields", "BodyFieldClient", &artifacts)
                .lines()
                .collect::<Vec<_>>(),
            ["4", "4", "7", "2"]
        );
    }
}

#[test]
fn public_body_fields_reject_swapped_initializers_with_identical_source_text() {
    assert_substituted_field_root_is_rejected(BODY_COUNTER, 0);
}

fn assert_substituted_field_root_is_rejected(source: &str, target: usize) {
    use dotty::jvm_class_plan::TypedFieldInitializer;

    let (mut store, defs, packages, unit) = named(source);
    complete_signatures(std::slice::from_ref(&unit), &mut store, defs, &packages).unwrap();
    let class = &unit.classes()[0];
    let mut typer = unit.typer(&mut store, defs, &packages);
    let fields: Vec<_> = class
        .fields()
        .map(|field| {
            (
                field.symbol,
                typer.type_field_initializer(field.symbol).unwrap(),
            )
        })
        .collect();
    for method in class.methods() {
        typer.type_method_definition(method.tree).unwrap();
    }
    let output = typer.into_typed_output();
    let planner = JvmMethodPlanner::new(&store, unit.ast(), unit.index(), &output);
    let methods = class
        .methods()
        .map(|method| planner.plan(method.symbol).unwrap());
    let mut inputs: Vec<_> = fields
        .into_iter()
        .map(|(field, root)| TypedFieldInitializer {
            field,
            root,
            index: &output.source_typed_index,
        })
        .collect();
    assert_eq!(inputs.len(), 2);
    let expected = inputs[target].root;
    let actual = inputs[1 - target].root;
    inputs[target].root = actual;
    let field = inputs[target].field;
    assert_eq!(
        JvmClassPlanner::new(&store, unit.ast(), unit.index()).plan_with_fields(
            class.symbol,
            methods,
            &output.typed_ast,
            inputs
        ),
        Err(ClassPlanError::FieldInitializerMismatch {
            field,
            expected,
            actual
        })
    );
}

#[test]
fn class_planner_rejects_nonordinary_parameters_and_multiple_lists() {
    for source in [
        "class C(private val x: Int)",
        "class C(protected var x: Int)",
        "class C(x: Int)(y: Int)",
        "class C(x: => Int)",
        "class C(xs: Int*)",
        "class C(using x: Int)",
    ] {
        let (store, _, _, unit) = named(source);
        let class = unit.classes()[0].symbol;
        assert_eq!(
            JvmClassPlanner::new(&store, unit.ast(), unit.index()).plan(class, []),
            Err(ClassPlanError::ConstructorParameters(class)),
            "{source}"
        );
    }
}

#[test]
fn method_only_class_plan_preserves_constructor_formal_identity_and_order() {
    let (store, _, _, unit) = named("class Pair(z: Int, val a: Int, var b: Int)");
    let class = &unit.classes()[0];
    let plan = JvmClassPlanner::new(&store, unit.ast(), unit.index())
        .plan(class.symbol, [])
        .unwrap();
    let dotty::jvm::JvmClassInitializationPlan::PrimaryConstructor(constructor) =
        plan.initialization
    else {
        panic!("expected an explicit primary constructor");
    };
    assert_eq!(constructor.symbol(), class.constructor);
    let names: Vec<_> = constructor
        .parameters()
        .iter()
        .map(|&id| {
            let parameter = store.symbols.get(id);
            assert_eq!(parameter.kind, dotty::core::SymbolKind::Parameter);
            assert_eq!(parameter.owner, Some(class.constructor));
            store.names.resolve(parameter.name.text())
        })
        .collect();
    assert_eq!(names, ["z", "a", "b"]);
    let bindings: Vec<_> = constructor
        .accessor_bindings()
        .iter()
        .map(|binding| {
            let field = store.symbols.get(binding.field());
            assert_eq!(field.owner, Some(class.symbol));
            assert_eq!(field.kind, dotty::core::SymbolKind::Field);
            assert_eq!(
                binding.parameter(),
                constructor.parameters()[binding.source_ordinal()]
            );
            (
                binding.source_ordinal(),
                store.names.resolve(field.name.text()),
                binding.is_mutable(),
            )
        })
        .collect();
    assert_eq!(bindings, [(1, "a", false), (2, "b", true)]);
}

#[test]
fn class_planner_identifies_the_parameter_with_a_default() {
    for prefix in ["", "val ", "var "] {
        let (store, _, _, unit) = named(&format!("class C(x: Int, {prefix}y: Int = 3)"));
        let class = &unit.classes()[0];
        let Some(dotty::core::SourceDefinition::Canonical { tree, .. }) =
            unit.index().definition_of(class.constructor)
        else {
            panic!("expected a canonical constructor");
        };
        let TreeKind::DefDef(constructor) = &unit.ast().get(tree).kind else {
            panic!("expected a constructor")
        };
        let parameter = constructor.value_param_clauses[0][1];
        assert_eq!(
            JvmClassPlanner::new(&store, unit.ast(), unit.index()).plan(class.symbol, []),
            Err(ClassPlanError::DefaultConstructorParameter {
                class: class.symbol,
                tree: parameter
            })
        );
    }
}

#[test]
fn class_planner_rejects_template_initializer() {
    let source = SourceId::from_index(2);
    let files = [(
        source,
        "Classes.scala",
        "class C { def run(): Int = 1; run() }",
    )];
    let mut store = SemanticStore::new();
    Definitions::bootstrap(&mut store);
    let parsed = parsing::parse_files(&files, 1)
        .pop()
        .unwrap()
        .bind(&mut store.names);
    let named = parsed
        .name(files[0].1, &mut store, &mut Packages::new())
        .unwrap();
    let (parsed, index) = (named.syntax(), named.index());
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("expected package")
    };
    let class = index.symbol_at(source, package.stats[0]).unwrap();
    let tree = parsed
        .ast
        .iter()
        .find_map(|(id, node)| matches!(node.kind, TreeKind::Apply(_)).then_some(id))
        .unwrap();
    assert_eq!(
        JvmClassPlanner::new(&store, &parsed.ast, index).plan(class, []),
        Err(ClassPlanError::UnsupportedStatement { class, tree })
    );
}

#[test]
fn class_planner_requires_a_public_constructor() {
    let (store, _, _, unit) = named("class A()\nclass B private ()");
    let planner = JvmClassPlanner::new(&store, unit.ast(), unit.index());
    let [plain, private] = unit.classes() else {
        panic!("expected two classes")
    };
    let plan = planner.plan(plain.symbol, []).unwrap();
    assert_eq!(
        plan.initialization,
        dotty::jvm::JvmClassInitializationPlan::ImplicitNoArg
    );
    assert!(plan.fields.is_empty() && plan.methods.is_empty());
    assert_eq!(
        planner.plan(private.symbol, []),
        Err(ClassPlanError::UnsupportedConstructor(private.symbol))
    );
}

#[test]
fn class_planner_rejects_parent_arguments_even_after_type_resolution() {
    let (mut store, defs, packages, unit) =
        named("class Base(x: Int)\nclass Derived(start: Int) extends Base(1)");
    let headers =
        complete_signatures(std::slice::from_ref(&unit), &mut store, defs, &packages).unwrap();
    let child = unit.classes()[1].symbol;
    let planner = JvmClassPlanner::new(&store, unit.ast(), unit.index());
    assert_eq!(
        planner.plan(child, []),
        Err(ClassPlanError::UnsupportedTemplate(child))
    );
    assert_eq!(
        planner
            .with_header_bindings(&headers.arena, &headers.bindings)
            .plan(child, []),
        Err(ClassPlanError::UnsupportedTemplate(child))
    );
}

#[test]
fn bare_parent_requires_an_available_no_arg_constructor_before_emission() {
    use dotty::jvm::{JvmCodegenError, JvmSuperclassIssue};
    let files = [(
        SourceId::from_index(0),
        "Classes.scala",
        "class Base(x: Int)\nclass Derived extends Base",
    )];
    super::with_plan(&files, |unit, definitions, _, _, _| {
        let [base, derived] = unit.classes.as_slice() else {
            panic!("expected two classes")
        };
        assert_eq!(
            unit.prepare_emission(definitions).err(),
            Some(JvmCodegenError::InvalidJvmSuperclass {
                class: derived.class,
                parent: Some(base.class),
                issue: JvmSuperclassIssue::ParentConstructorUnavailable
            })
        );
    });
}

#[test]
fn bare_parent_constructor_and_virtual_dispatch_work_in_both_file_orders_on_jdk21() {
    let base = (
        SourceId::from_index(0),
        "Base.scala",
        "package demo
class Base:
  private val seed: Int = 1
  def initialized(): Int = seed
  def value(): Int = seed
  def read(): Int = value()
",
    );
    let derived = (
        SourceId::from_index(1),
        "Derived.scala",
        "package demo
class Derived extends Base:
  override def value(): Int = 7
",
    );
    let first = super::compile([base, derived]);
    let second = super::compile([derived, base]);
    assert!(first.iter().eq(second.iter().rev()));
    for artifacts in [first, second] {
        assert_super_call(&artifacts, "()V", &[0x2a]);
        assert_eq!(
            super::run_java_client("inheritance", "InheritanceClient", &artifacts)
                .lines()
                .collect::<Vec<_>>(),
            ["1", "1", "7"]
        );
    }
}

fn assert_super_call(artifacts: &[dotty::jvm::GeneratedClass], descriptor: &str, loads: &[u8]) {
    use dotty::classfile::{Attribute, ClassFile, ConstantPoolEntry, ConstantPoolIndex, Reader};
    let artifact = artifacts
        .iter()
        .find(|a| a.internal_name() == "demo/Derived")
        .unwrap();
    let class = ClassFile::decode(&mut Reader::new(artifact.bytes())).unwrap();
    let pool = &class.constant_pool;
    assert_eq!(pool.class_name(class.super_class.unwrap()), Ok("demo/Base"));
    let constructor = class
        .methods
        .iter()
        .find(|m| pool.utf8(m.name_index) == Ok("<init>"))
        .unwrap();
    assert_eq!(pool.utf8(constructor.descriptor_index), Ok(descriptor));
    let [Attribute::Code(code)] = constructor.attributes.as_slice() else {
        panic!("expected constructor code")
    };
    let Some([0xb7, hi, lo, 0xb1]) = code.code.strip_prefix(loads) else {
        panic!("expected receiver and argument loads, invokespecial, return")
    };
    let reference = ConstantPoolIndex(u16::from_be_bytes([*hi, *lo]));
    let Some(ConstantPoolEntry::Methodref {
        class_index,
        name_and_type_index,
    }) = pool.get(reference)
    else {
        panic!("invokespecial must reference a class method")
    };
    assert_eq!(pool.class_name(*class_index), Ok("demo/Base"));
    assert_eq!(
        pool.name_and_type(*name_and_type_index),
        Ok(("<init>", descriptor))
    );
}

#[test]
fn forwarded_parent_argument_reaches_base_state_in_both_file_orders() {
    let base = (
        SourceId::from_index(0),
        "Base.scala",
        "package demo
class Base(seed: Int):
  private val initial: Int = seed
  def read(): Int = initial
",
    );
    let derived = (
        SourceId::from_index(1),
        "Derived.scala",
        "package demo
class Derived(start: Int) extends Base(start)
",
    );
    let compile = |files: [_; 2]| {
        super::with_plan(&files, |plan, defs, sources, bodies, headers| {
            let index = dotty::ResolvedMemberIndex::build_with_headers(
                sources,
                bodies,
                headers,
                plan.typed_ast,
                plan.store,
            );
            assert!(index.diagnostics().is_empty());
            for ((source, _, text), name) in [(base, "seed"), (derived, "start")] {
                let read = text.rfind(name).unwrap() as u32;
                let declaration = text.find(name).unwrap() as u32;
                let p = index.constructor_parameter_at(source, read).unwrap();
                let site = index.symbol_at(source, read).unwrap();
                assert_eq!(site.symbol, p.parameter);
                assert_eq!(site.target.source, source);
                assert_eq!(site.target.name.start(), declaration);
                assert_eq!(index.references_to(p.parameter), &[site.usage.unwrap()]);
                let class = plan.classes.iter().find(|c| c.class == p.class).unwrap();
                let dotty::jvm::JvmClassInitializationPlan::PrimaryConstructor(ctor) =
                    &class.initialization
                else {
                    panic!("explicit constructor")
                };
                assert_eq!(ctor.symbol(), p.constructor);
                assert_eq!(ctor.parameters(), &[p.formal]);
            }
            super::checked_emission(plan, defs)
        })
    };
    let first = compile([base, derived]);
    let second = compile([derived, base]);
    assert!(first.iter().eq(second.iter().rev()));
    for artifacts in [first, second] {
        assert_super_call(&artifacts, "(I)V", &[0x2a, 0x1b]); // aload_0, iload_1
        assert_eq!(
            super::run_java_client("forwarded-parent", "ForwardingClient", &artifacts)
                .lines()
                .collect::<Vec<_>>(),
            ["40", "7"]
        );
    }
}

#[test]
fn header_chain_follows_dependencies_and_executes_in_both_file_orders() {
    use dotty::typer::SignatureCompletion;

    let base = (
        SourceId::from_index(0),
        "Base.scala",
        "package demo\nclass Base(seed: Int):\n  private val initial = seed\n  def read(): Int = initial",
    );
    let mid = (
        SourceId::from_index(1),
        "Mid.scala",
        "package demo\nclass Mid(start: Int) extends Base(start)",
    );
    let leaf = (
        SourceId::from_index(2),
        "Leaf.scala",
        "package demo\nclass Leaf(start: Int) extends Mid(start)",
    );

    for files in [[leaf, mid, base], [base, mid, leaf]] {
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let units = name_units(
            &files,
            parsing::parse_files(&files, 2),
            &mut store,
            &mut packages,
        );
        let mut attempts = Vec::new();
        let headers = super::complete_headers(
            &NamedClassSources::new(&units, &store).unwrap(),
            &mut store,
            defs,
            &packages,
            &mut |symbol, outcome| {
                let dependency = match outcome {
                    SignatureCompletion::BlockedOn(parent) => Some(parent),
                    SignatureCompletion::Complete(_) => None,
                };
                attempts.push((symbol, dependency));
            },
        )
        .unwrap();
        let first = units[0].classes()[0].symbol;
        let mid = units[1].classes()[0].symbol;
        let last = units[2].classes()[0].symbol;
        if files[0].0 == leaf.0 {
            assert_eq!(
                attempts,
                [
                    (first, Some(mid)),
                    (mid, Some(last)),
                    (last, None),
                    (mid, None),
                    (first, None)
                ]
            );
        } else {
            assert_eq!(attempts, [(first, None), (mid, None), (last, None)]);
        }

        for unit in &units {
            let class = &unit.classes()[0];
            for parent in &class.parents {
                assert!(headers.bindings.get(unit.source(), *parent).is_some());
            }
        }

        let artifacts = super::compile(files);
        assert_eq!(
            super::run_java_client("header-chain", "HeaderChainClient", &artifacts)
                .lines()
                .collect::<Vec<_>>(),
            ["40", "7"]
        );
    }
}

#[test]
fn one_completion_supplies_both_same_file_parent_calls_for_deferred_planning() {
    let derived = (
        SourceId::from_index(1),
        "Derived.scala",
        "package demo
class Leaf(x: Int) extends Mid(x)
class Mid(y: Int) extends Base(y)",
    );
    let base = (
        SourceId::from_index(0),
        "Base.scala",
        "package demo
class Base(seed: Int) { private val initial = seed; def read(): Int = initial }",
    );
    for (files, derived_index) in [([derived, base], 0), ([base, derived], 1)] {
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let units = name_units(
            &files,
            parsing::parse_files(&files, 2),
            &mut store,
            &mut packages,
        );
        let [leaf, mid] = units[derived_index].classes() else {
            panic!("expected Leaf and Mid in one file")
        };
        let base = units[1 - derived_index].classes()[0].symbol;
        let mut attempts = Vec::new();
        let headers = super::complete_signatures_in_order(
            &units,
            &units,
            &mut store,
            defs,
            &packages,
            |symbol, outcome| {
                if [leaf.symbol, mid.symbol, base].contains(&symbol) {
                    let blocked = match outcome {
                        dotty::typer::SignatureCompletion::BlockedOn(parent) => Some(parent),
                        dotty::typer::SignatureCompletion::Complete(_) => None,
                    };
                    attempts.push((symbol, blocked));
                }
            },
        )
        .unwrap();
        if derived_index == 0 {
            assert_eq!(
                attempts,
                [(leaf.symbol, Some(base)), (base, None), (leaf.symbol, None)]
            );
        } else {
            assert_eq!(attempts, [(base, None), (leaf.symbol, None)]);
        }

        let (output, bodies) = type_bodies(
            &dotty::NamedClassSources::new(&units, &store).unwrap(),
            &mut store,
            defs,
            &packages,
            headers,
        );
        let roots = [leaf, mid].map(|class| {
            output
                .bindings
                .get(derived.0, class.parents[0])
                .expect("completed parent application")
        });
        assert_ne!(roots[0], roots[1]);
        // Plan Mid before Leaf using the same retained header and body bindings.
        let classes = super::named_classes(&units)
            .rev()
            .map(|(unit, class)| {
                JvmClassPlanner::new(&store, unit.ast(), unit.index())
                    .with_header_bindings(&output.arena, &output.bindings)
                    .plan_typed_class(class, &output.arena, bodies.complete(class.symbol).unwrap())
                    .unwrap()
            })
            .collect();
        let plan = dotty::jvm::JvmCompilationUnit {
            store: &store,
            typed_ast: &output.arena,
            classes,
        };
        let prepared = plan.prepare_emission(&defs).unwrap();
        let (artifacts, _) = super::emit_batch(&prepared, &plan.classes, 2);
        assert_eq!(
            super::run_java_client("shared-header-bindings", "HeaderChainClient", &artifacts)
                .lines()
                .collect::<Vec<_>>(),
            ["40", "7"]
        );
    }
}

#[test]
fn header_cycle_reports_only_the_active_cycle_and_leaves_headers_incomplete() {
    let leaf = (
        SourceId::from_index(0),
        "Leaf.scala",
        "class Leaf(x: Int) extends A(x)",
    );
    let a = (
        SourceId::from_index(1),
        "A.scala",
        "class A(x: Int) extends B(x)",
    );
    let b = (
        SourceId::from_index(2),
        "B.scala",
        "class B(x: Int) extends A(x)",
    );
    for (files, cycle_start) in [([leaf, a, b], 1), ([b, a, leaf], 0)] {
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let units = name_units(
            &files,
            parsing::parse_files(&files, 2),
            &mut store,
            &mut packages,
        );
        let error = complete_signatures(&units, &mut store, defs, &packages).unwrap_err();
        let super::SignatureError::Cycle(cycle) = error else {
            panic!("expected a cycle, got {error:?}");
        };
        let first = units[cycle_start].classes()[0].symbol;
        let second = units[cycle_start + 1].classes()[0].symbol;
        assert_eq!(cycle.code(), "signature.header_cycle");
        assert_eq!(cycle.path(), [first, second, first]);
        let locations: Vec<_> = cycle.declarations(&store).collect();
        assert_eq!(
            locations,
            [
                (first, store.symbols.get(first).position),
                (second, store.symbols.get(second).position)
            ]
        );
        assert!(locations.iter().all(|(_, position)| position.is_some()));
        for (_, class) in super::named_classes(&units) {
            assert_eq!(
                store.symbols.info(class.symbol),
                &dotty::core::SymbolInfo::Missing
            );
        }
    }
}

#[test]
fn wrong_parent_argument_reports_the_same_error_in_both_file_orders() {
    use dotty::typer::{SignatureCompletion, TyperError};

    let base = (
        SourceId::from_index(0),
        "Base.scala",
        "class Base(seed: Int)",
    );
    let derived = (
        SourceId::from_index(1),
        "Derived.scala",
        "class Derived(start: Boolean) extends Base(start)",
    );
    for (files, child_index, parent_index) in [([base, derived], 1, 0), ([derived, base], 0, 1)] {
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let units = name_units(
            &files,
            parsing::parse_files(&files, 2),
            &mut store,
            &mut packages,
        );
        let child = &units[child_index].classes()[0];
        let parent = units[parent_index].classes()[0].symbol;
        let mut blocked = Vec::new();
        let sources = NamedClassSources::new(&units, &store).unwrap();
        let error = super::complete_headers(
            &sources,
            &mut store,
            defs,
            &packages,
            &mut |class, result| {
                if let SignatureCompletion::BlockedOn(dependency) = result {
                    blocked.push((class, dependency));
                }
            },
        )
        .unwrap_err();

        if child_index == 0 {
            assert_eq!(blocked, [(child.symbol, parent)]);
        } else {
            assert!(blocked.is_empty());
        }
        let diagnostic = error.into_semantic_diagnostic(&sources).unwrap();
        assert_eq!(diagnostic.phase, dotty::SemanticPhase::ClassHeader);
        assert_eq!(diagnostic.owner_source, Some(derived.0));
        let dotty::SemanticIssue::Typing(report) = diagnostic.issue else {
            panic!("expected header typer error")
        };
        assert_eq!(report.class(), Some(child.symbol));
        let error = report.error();
        assert!(
            matches!(error, TyperError::ApplicationArgumentTypeMismatch {
                argument_index: 0, actual, expected, ..
            } if *actual == defs.boolean && *expected == defs.int),
            "{error:?}"
        );
        let diagnostic = report.render();
        assert_eq!(diagnostic.message, "Expected Int, found Boolean");
        assert_eq!(diagnostic.severity, dotty::core::DiagnosticSeverity::Error);
        let span = diagnostic
            .location
            .expect("parent argument in Derived.scala");
        let start = derived.2.rfind("start").unwrap() as u32;
        assert_eq!(span.source(), derived.0);
        assert_eq!(
            span.span().range(),
            dotty::core::TextRange::new(start, start + 5).unwrap()
        );
        for ast in [Some(units[parent_index].ast()), None] {
            let unlocated = dotty::TypingDiagnostic::from_typer(error, |_| ast);
            assert_eq!(unlocated.location, None);
            assert_eq!(unlocated.message, diagnostic.message);
        }
        assert_eq!(
            store.symbols.info(child.symbol),
            &dotty::core::SymbolInfo::Missing
        );
    }
}

#[test]
fn header_attempt_checks_class_identity_even_for_completed_symbols() {
    let (mut store, defs, packages, units) = body_units(VALID_B);
    let mut headers = complete_signatures(&units, &mut store, defs, &packages).unwrap();
    let a = &units[0];
    let b = &units[1];
    let class = &a.classes()[0];
    let before = headers.arena.checkpoint();
    let mut attempt = |symbol, ast| {
        dotty::try_complete_class_header(
            symbol,
            ast,
            a.index(),
            &mut store,
            defs,
            &packages,
            &mut headers.arena,
            &mut headers.bindings,
            |_| false,
        )
    };
    let error = attempt(class.symbol, b.ast()).unwrap_err();
    assert!(
        matches!(error, dotty::typer::TyperError::MalformedSourceAst { source, tree_index, symbol, .. }
        if source == a.source() && tree_index == class.tree.index() && symbol == class.symbol)
    );
    let method = class.methods().next().unwrap();
    let error = attempt(method.symbol, a.ast()).unwrap_err();
    assert!(
        matches!(error, dotty::typer::TyperError::SymbolSourceKindMismatch { symbol, kind: dotty::core::SymbolKind::Method, .. } if symbol == method.symbol)
    );
    assert_eq!(headers.arena.checkpoint(), before);
}

#[test]
fn failed_header_preserves_good_parent_call_for_execution() {
    let files = [
        (
            SourceId::from_index(0),
            "Base.scala",
            "package demo\nclass Base(val value: Int)",
        ),
        (
            SourceId::from_index(1),
            "Good.scala",
            "package demo\nclass Good(start: Int) extends Base(start)",
        ),
        (
            SourceId::from_index(2),
            "Bad.scala",
            "package demo\nclass Bad(start: Boolean) extends Base(start)",
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
    let mut headers = complete_signatures(&units[..2], &mut store, defs, &packages).unwrap();
    let good = &units[1];
    let good_parent = good.classes()[0].parents[0];
    let good_root = headers.bindings.get(good.source(), good_parent).unwrap();
    let bad = &units[2];
    let class = &bad.classes()[0];
    let sources = NamedClassSources::new(&units, &store).unwrap();
    sources
        .complete_constructor_signature(class.symbol, &mut store, defs, &packages)
        .unwrap();
    let parent = class.parents[0];
    let TreeKind::Apply(call) = &bad.ast().get(parent).kind else {
        panic!("expected Base(start)")
    };
    let argument = call.args[0];
    let before = headers.arena.checkpoint();
    let error = sources
        .try_complete_class_header_reported(
            class.symbol,
            &mut store,
            defs,
            &packages,
            &mut headers.arena,
            &mut headers.bindings,
        )
        .unwrap_err();
    let dotty::ClassHeaderError::Typing(report) = error else {
        panic!("expected a typed rejection")
    };
    let dotty::typer::TyperError::ApplicationArgumentTypeMismatch {
        source,
        tree_index,
        argument_index,
        actual,
        expected,
        ..
    } = report.error()
    else {
        panic!("unexpected error for Bad: {report:?}")
    };
    assert_eq!(*source, bad.source());
    assert_eq!(*tree_index, parent.index());
    assert_eq!(*argument_index, 0);
    assert_eq!((*actual, *expected), (defs.boolean, defs.int));
    assert_eq!(
        store.symbols.info(class.symbol),
        &dotty::core::SymbolInfo::Missing
    );
    assert_eq!(headers.arena.checkpoint(), before);
    assert_eq!(headers.bindings.get(bad.source(), parent), None);
    assert_eq!(headers.bindings.get(bad.source(), argument), None);
    assert_eq!(
        headers.bindings.get(good.source(), good_parent),
        Some(good_root)
    );

    let (output, bodies) = type_bodies(
        &dotty::NamedClassSources::new(&units[..2], &store).unwrap(),
        &mut store,
        defs,
        &packages,
        headers,
    );
    let classes = class_plans(&units[..2], &bodies, &output, &store).unwrap();
    let plan = dotty::jvm::JvmCompilationUnit {
        store: &store,
        typed_ast: &output.arena,
        classes,
    };
    let prepared = plan.prepare_emission(&defs).unwrap();
    let (artifacts, _) = super::emit_batch(&prepared, &plan.classes, 2);
    assert_eq!(
        super::run_java_client("surviving-header", "GoodHeaderClient", &artifacts).trim(),
        "40"
    );
}

#[test]
fn forwarded_parent_rejects_an_int_argument_from_another_parent_call() {
    let (mut store, defs, packages, unit) = named(
        "class Base(seed: Int)
class First(start: Int) extends Base(start)
class Second(start: Int) extends Base(start)",
    );
    let mut headers =
        complete_signatures(std::slice::from_ref(&unit), &mut store, defs, &packages).unwrap();
    let [_, first, second] = unit.classes() else {
        panic!("expected three classes")
    };
    let typed_call = |class: &super::SourceClass| {
        headers
            .bindings
            .get(unit.source(), class.parents[0])
            .unwrap()
    };
    let first_call = typed_call(first);
    let second_call = typed_call(second);
    let argument = |call| {
        let TreeKind::Apply(app) = &headers.arena.get(call).kind else {
            panic!("expected a typed parent application")
        };
        let [arg] = app.args[..] else {
            panic!("expected a single argument")
        };
        let Some(dotty::core::Type::TermRef {
            target: dotty::core::TermRefTarget::Symbol(formal),
            ..
        }) = store.types.try_get(headers.arena.get(arg).ty)
        else {
            panic!("expected a constructor parameter reference")
        };
        assert_eq!(
            *store.symbols.info(*formal),
            dotty::core::SymbolInfo::Complete(defs.int)
        );
        arg
    };
    let original = argument(first_call);
    let foreign = argument(second_call);
    assert_ne!(original, foreign);
    let planner = JvmClassPlanner::new(&store, unit.ast(), unit.index());
    let bindings = &headers.bindings;
    planner
        .with_header_bindings(&headers.arena, bindings)
        .plan(first.symbol, [])
        .unwrap();
    let TreeKind::Apply(app) = &mut headers.arena.get_mut(first_call).kind else {
        unreachable!()
    };
    app.args[0] = foreign;
    assert_eq!(
        JvmClassPlanner::new(&store, unit.ast(), unit.index())
            .with_header_bindings(&headers.arena, bindings)
            .plan(first.symbol, []),
        Err(ClassPlanError::InvalidParentApplication {
            class: first.symbol,
            tree: first.parents[0]
        })
    );
}

#[test]
fn parent_constructor_must_belong_to_its_declaring_source() {
    let base = SourceId::from_index(1);
    let derived = SourceId::from_index(2);
    let files = [
        (base, "Base.scala", "class Base(seed: Int)"),
        (
            derived,
            "Derived.scala",
            "class Derived(start: Int) extends Base(start)",
        ),
    ];
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let units = name_units(
        &files,
        parsing::parse_files(&files, 1),
        &mut store,
        &mut packages,
    );
    let headers = complete_signatures(&units, &mut store, defs, &packages).unwrap();
    let unit = &units[1];
    let child = &unit.classes()[0];
    let constructor = units[0].classes()[0].constructor;
    store.symbols.get_mut(constructor).origin = dotty::core::SymbolOrigin::Source(derived);
    assert_eq!(
        JvmClassPlanner::new(&store, unit.ast(), unit.index())
            .with_header_bindings(&headers.arena, &headers.bindings)
            .plan(child.symbol, []),
        Err(ClassPlanError::InvalidParentApplication {
            class: child.symbol,
            tree: child.parents[0]
        })
    );
}

#[test]
fn private_field_initializers_follow_declarations_and_reject_same_type_substitution() {
    use dotty::jvm_class_plan::TypedFieldInitializer;

    let (mut store, defs, packages, unit) = named(
        "class Pair { private val base: Int = 40; private val answer: Int = base + 2 }\nclass Other { private val base: Int = 1 }",
    );
    let class = unit.classes()[0].symbol;
    let mut typer = unit.typer(&mut store, defs, &packages);
    let mut roots = Vec::new();
    for field in unit
        .classes()
        .iter()
        .flat_map(|class| class.fields())
        .map(|d| d.symbol)
    {
        let root = typer.type_field_initializer(field).unwrap();
        roots.push((field, root));
    }
    let (arena, index) = typer.into_typed_ast_and_index();
    let [a, b, foreign] = roots[..] else {
        panic!("expected three fields")
    };
    let input = |(field, root)| TypedFieldInitializer {
        field,
        root,
        index: &index,
    };
    let planner = JvmClassPlanner::new(&store, unit.ast(), unit.index());
    let empty_index = dotty::core::SourceTypedIndex::new();
    assert_eq!(
        planner.plan_with_fields(
            class,
            [],
            &arena,
            [TypedFieldInitializer {
                index: &empty_index,
                ..input(a)
            }]
        ),
        Err(ClassPlanError::InvalidFieldInitializer { field: a.0 })
    );
    let plan = planner
        .plan_with_fields(class, [], &arena, [input(b), input(a)])
        .unwrap();
    assert_eq!(
        plan.fields
            .iter()
            .map(|p| (p.field, p.initializer.unwrap()))
            .collect::<Vec<_>>(),
        [a, b]
    );
    assert_eq!(
        planner.plan_with_fields(class, [], &arena, [input(a), input((b.0, a.1))]),
        Err(ClassPlanError::FieldInitializerMismatch {
            field: b.0,
            expected: b.1,
            actual: a.1
        })
    );
    assert_eq!(
        planner.plan_with_fields(class, [], &arena, [input(a), input(foreign)]),
        Err(ClassPlanError::UnexpectedField {
            class,
            field: foreign.0
        })
    );
    assert_eq!(
        planner.plan_with_fields(class, [], &arena, [input(b)]),
        Err(ClassPlanError::MissingField { class, field: a.0 })
    );
    assert_eq!(
        planner.plan_with_fields(class, [], &arena, [input(a), input(a)]),
        Err(ClassPlanError::DuplicateField { class, field: a.0 })
    );
}

#[test]
fn private_fields_encode_mutability_and_keep_per_instance_state_on_jdk21() {
    use dotty::classfile::{ACC_FINAL, ACC_PRIVATE, ClassFile, FieldAccessFlags, Reader};

    let (mut store, defs, packages, unit) = named(include_str!("../../Counter.scala"));
    let units = [unit];
    let headers = complete_signatures(&units, &mut store, defs, &packages).unwrap();
    let (output, bindings) = type_bodies(
        &dotty::NamedClassSources::new(&units, &store).unwrap(),
        &mut store,
        defs,
        &packages,
        headers,
    );
    let unit = &units[0];
    let mut view = editable_classes(unit, &store);
    view[0].members.reverse();
    let planner = JvmClassPlanner::new(&store, unit.ast(), unit.index())
        .with_header_bindings(&output.arena, &output.bindings);
    let classes = vec![
        planner
            .plan_typed_class(
                &view[0],
                &output.arena,
                bindings.complete(view[0].symbol).unwrap(),
            )
            .unwrap(),
    ];
    let class = classes[0].class;
    let unit = dotty::jvm::JvmCompilationUnit {
        store: &store,
        typed_ast: &output.arena,
        classes,
    };
    let artifact = unit
        .prepare_emission(&defs)
        .unwrap()
        .emit_class(class)
        .unwrap();
    let decoded = ClassFile::decode(&mut Reader::new(artifact.bytes())).unwrap();
    let fields: Vec<_> = decoded
        .fields
        .iter()
        .map(|field| {
            (
                decoded.constant_pool.utf8(field.name_index).unwrap(),
                field.access_flags,
            )
        })
        .collect();
    assert_eq!(
        fields,
        [
            ("base", FieldAccessFlags(ACC_PRIVATE | ACC_FINAL)),
            ("current", FieldAccessFlags(ACC_PRIVATE)),
        ]
    );
    assert_eq!(
        super::run_java_client("private-fields", "CounterClient", &[artifact])
            .lines()
            .collect::<Vec<_>>(),
        ["42", "45", "44", "42"]
    );
}

#[test]
fn private_var_rejects_the_initializer_of_another_int_field() {
    assert_substituted_field_root_is_rejected(include_str!("../../Counter.scala"), 1);
}

#[test]
fn field_planning_rejects_a_mutability_mismatch_between_source_and_symbol() {
    use dotty::core::SymbolFlags;

    for (keyword, flags) in [("val", SymbolFlags::MUTABLE), ("var", SymbolFlags::EMPTY)] {
        let (mut store, _, _, unit) = named(&format!("class C {{ private {keyword} n: Int = 1 }}"));
        let class = &unit.classes()[0];
        let field = class.fields().next().unwrap().symbol;
        store.symbols.get_mut(field).flags = flags;
        let Some(dotty::core::SourceDefinition::Canonical { tree, .. }) =
            unit.index().definition_of(field)
        else {
            panic!("expected field declaration")
        };
        assert_eq!(
            JvmClassPlanner::new(&store, unit.ast(), unit.index()).plan_with_fields(
                class.symbol,
                [],
                &dotty::core::AstArena::new(),
                []
            ),
            Err(ClassPlanError::UnsupportedStatement {
                class: class.symbol,
                tree
            }),
            "{keyword}"
        );
    }
}

#[test]
fn field_planning_rejects_qualified_and_modified_fields() {
    for declaration in [
        "protected var n: Int = 1",
        "lazy val n: Int = 1",
        "final val n: Int = 1",
        "private[this] val n: Int = 1",
        "private lazy val n: Int = 1",
    ] {
        let (store, _, _, unit) = named(&format!("class C {{ {declaration} }}"));
        let class = &unit.classes()[0];
        let field = class.fields().next().unwrap().symbol;
        let Some(dotty::core::SourceDefinition::Canonical { tree, .. }) =
            unit.index().definition_of(field)
        else {
            panic!("expected field declaration")
        };
        let expected = Err(ClassPlanError::UnsupportedStatement {
            class: class.symbol,
            tree,
        });
        let planner = JvmClassPlanner::new(&store, unit.ast(), unit.index());
        assert_eq!(
            planner.plan_with_fields(class.symbol, [], &dotty::core::AstArena::new(), []),
            expected,
            "{declaration}"
        );
    }
}

#[test]
fn waiter_identifies_provider_with_original_bad_call_report() {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let text = "class Provider { def value(): Int = 7; val seed = value(1) }";
    let units = field_signature_units(CONSUMER_OF_SEED, text, false, &mut store, &mut packages);
    let sources = NamedClassSources::new(&units, &store).unwrap();
    super::complete_headers(&sources, &mut store, defs, &packages, &mut |_, _| {}).unwrap();
    let answer = units[0].classes()[0].fields().next().unwrap().symbol;
    let seed = units[1].classes()[0].fields().next().unwrap().symbol;
    let mut graph = dotty::SignatureDependencies::new(dotty::SignaturePhase::Members);
    let mut attempt = |file: usize, symbol| {
        let u = &units[file];
        dotty::try_complete_member_signature(
            symbol,
            sources.declarations(),
            u.ast(),
            u.index(),
            &mut store,
            defs,
            &packages,
        )
    };
    graph.record(answer, attempt(0, answer).unwrap());
    let failure = attempt(1, seed).unwrap_err();
    graph.record_failure(seed);
    assert_eq!(
        graph.blockages([answer]),
        [(answer, dotty::SignatureBlockage::Failed(seed))]
    );
    let report = failure
        .into_reported_error(|id| sources.file(id).map(|u| u.ast()))
        .unwrap();
    assert!(matches!(
        report.error(),
        dotty::typer::TyperError::ApplicationArityMismatch {
            expected: 0,
            actual: 1,
            ..
        }
    ));
    assert_eq!(report.code(), Some("typer.application_arity_mismatch"));
    assert_eq!(report.member(), Some(seed));
    let span = report.location().unwrap();
    assert_eq!(span.source(), units[1].source());
    let range = span.span().range();
    assert_eq!(
        &text[range.start() as usize..range.end() as usize],
        "value(1)"
    );
}
