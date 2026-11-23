//! The consumer owns phase ordering, plans and output for classes with a no-arg constructor.
#[path = "support/class_planning.rs"]
mod class_planning;
#[path = "support/parsing.rs"]
mod parsing;
#[path = "support/pipeline_timing.rs"]
mod pipeline_timing;
#[path = "support/typer_error_spans.rs"]
mod typer_error_spans;

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use dotty::ClassHeaderBindings;
use dotty::NamedClassSource as NamedUnit;
use dotty::core::{
    AstArena, Definitions, Packages, SemanticStore, SourceId, SymbolId, SymbolInfo, TreeKind, Typed,
};
use dotty::jvm::{
    GeneratedClass, JvmClassPlan, JvmCompilationUnit, PreparedJvmEmission, emit_class,
};
use dotty::jvm_class_plan::{ClassPlanError, JvmClassPlanner, TypedClassBodies};
use dotty::jvm_method_plan::JvmMethodPlanner;
use dotty::source_declarations::{Declaration, SourceClass};
use dotty::typer::{SignatureCompletion, SourceTyper, TyperError};
use dotty::{DetachedSource, NamedClassSources, PreparationDiagnostic, SyntaxDiagnostic};

trait FixtureMethods {
    fn methods(&self) -> impl Iterator<Item = Declaration> + '_;
    fn method_symbol(&self) -> SymbolId;
    fn method_symbols(&self) -> impl Iterator<Item = SymbolId> + '_;
    fn typer<'a>(
        &'a self,
        store: &'a mut SemanticStore,
        definitions: Definitions,
        packages: &'a Packages,
    ) -> SourceTyper<'a>;
}

impl FixtureMethods for NamedUnit {
    fn methods(&self) -> impl Iterator<Item = Declaration> + '_ {
        self.classes().iter().flat_map(SourceClass::methods)
    }

    fn method_symbol(&self) -> SymbolId {
        let mut methods = self.methods();
        let method = methods.next().expect("expected one method");
        assert!(methods.next().is_none());
        method.symbol
    }

    fn method_symbols(&self) -> impl Iterator<Item = SymbolId> + '_ {
        self.methods().map(|method| method.symbol)
    }

    fn typer<'a>(
        &'a self,
        store: &'a mut SemanticStore,
        definitions: Definitions,
        packages: &'a Packages,
    ) -> SourceTyper<'a> {
        SourceTyper::new(
            self.ast(),
            self.source(),
            self.index(),
            store,
            definitions,
            packages,
        )
    }
}

#[test]
fn source_diagnostics_follow_a_field_from_parsing_through_typing() {
    let source = SourceId::from_index(47);
    let file = "Outer.scala";
    let broken = "class Outer {
  /* 😀 */ private[Missing] val hidden: Int 1
}";
    let parsed_ok = broken.replace("Int 1", "Int = 1");
    let fixed = parsed_ok.replace("[Missing]", "");
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    for (text, token, code, message) in [
        (
            broken,
            "1",
            "parser.declaration.expected_value_equals",
            "Expected '=' before the initializer",
        ),
        (
            parsed_ok.as_str(),
            "Missing",
            "namer.visibility.invalid_qualifier",
            "Invalid 'private' qualifier 'Missing': expected an enclosing class, object or package",
        ),
    ] {
        let files = [(source, file, text)];
        let (units, reports) = name_units_with_diagnostics(
            &files,
            parsing::parse_files(&files, 1),
            &mut store,
            &mut packages,
        );
        assert!(units.is_empty());
        let start = text.find(token).unwrap() as u32;
        let range = dotty::core::TextRange::new(start, start + token.len() as u32).unwrap();
        let location =
            dotty::core::SourceSpan::new(source, dotty::core::Span::without_point(range));
        let [report] = reports.as_slice() else {
            panic!("expected one diagnostic: {reports:?}")
        };
        let report = report.render();
        assert_eq!(report.code, Some(code));
        assert_eq!(report.message, message);
        assert_eq!(report.severity, dotty::core::DiagnosticSeverity::Error);
        assert_eq!(report.location, Some(location));
    }
    let files = [(source, file, fixed.as_str())];
    let units = name_units(
        &files,
        parsing::parse_files(&files, 1),
        &mut store,
        &mut packages,
    );
    let class = &units[0].classes()[0];
    let headers = complete_signatures(&units, &mut store, defs, &packages).unwrap();
    let (_, bodies) = type_bodies(
        &dotty::NamedClassSources::new(&units, &store).unwrap(),
        &mut store,
        defs,
        &packages,
        headers,
    );
    let field = class.fields().next().unwrap();
    assert_eq!(
        store.symbols.info(field.symbol),
        &SymbolInfo::Complete(defs.int)
    );
    bodies.get(class.symbol).unwrap();
}

fn name_units_with_diagnostics(
    files: &[(SourceId, &str, &str)],
    parsed: Vec<DetachedSource>,
    store: &mut SemanticStore,
    packages: &mut Packages,
) -> (Vec<NamedUnit>, Vec<PreparationDiagnostic>) {
    let mut units = Vec::new();
    let mut reports = Vec::new();
    for (&(_, name, _), detached) in files.iter().zip(parsed) {
        let (file, diagnostics) = detached
            .bind(&mut store.names)
            .prepare_classes(name, store, packages)
            .into_parts();
        units.extend(file);
        reports.extend(diagnostics);
    }
    (units, reports)
}

fn name_units(
    files: &[(SourceId, &str, &str)],
    parsed: Vec<DetachedSource>,
    store: &mut SemanticStore,
    packages: &mut Packages,
) -> Vec<NamedUnit> {
    let (units, reports) = name_units_with_diagnostics(files, parsed, store, packages);
    assert!(reports.is_empty(), "{reports:?}");
    units
}

#[test]
fn mixed_syntax_errors_leave_the_other_file_executable_in_either_order() {
    let bad = (
        SourceId::from_index(17),
        "Broken.scala",
        "class Broken { val hidden: Int 1 } /* open",
    );
    let good = (
        SourceId::from_index(2),
        "A.scala",
        "package demo\nclass A { def value(): Int = 40 }",
    );
    for files in [[bad, good], [good, bad]] {
        for workers in [1, 2] {
            let mut store = SemanticStore::new();
            let defs = Definitions::bootstrap(&mut store);
            let mut packages = Packages::new();
            let (units, reports) = name_units_with_diagnostics(
                &files,
                parsing::parse_files(&files, workers),
                &mut store,
                &mut packages,
            );
            assert_eq!(reports.len(), 2, "{reports:?}");
            for (report, (code, site)) in reports.iter().zip([
                ("parser.declaration.expected_value_equals", "1"),
                ("lexer.unterminated_block_comment", "/* open"),
            ]) {
                let report = report.render();
                assert_eq!(report.code, Some(code));
                let span = report.location.unwrap();
                assert_eq!(span.source(), bad.0);
                assert_eq!(
                    dotty::core::SourceText::new(bad.2)
                        .unwrap()
                        .slice(span.span().range())
                        .unwrap(),
                    site
                );
            }
            let [unit] = units.as_slice() else {
                panic!("expected the clean file")
            };
            assert_eq!(unit.source(), good.0);
            let headers = complete_signatures(&units, &mut store, defs, &packages).unwrap();
            let (output, bodies) = type_bodies(
                &dotty::NamedClassSources::new(&units, &store).unwrap(),
                &mut store,
                defs,
                &packages,
                headers,
            );
            let plan = JvmCompilationUnit {
                classes: class_plans(&units, &bodies, &output, &store).unwrap(),
                store: &store,
                typed_ast: &output.arena,
            };
            let artifact = emit_class(&plan, unit.classes()[0].symbol, &defs).unwrap();
            assert_eq!(
                run_java_client("mixed-syntax", "SurvivingClassClient", &[artifact]).trim(),
                "40"
            );
        }
    }
}

#[test]
fn projected_siblings_emit_and_the_scala_consumer_runs_in_both_input_orders() {
    let model = (
        SourceId::from_index(17),
        "Model.scala",
        "package demo\nclass A { def value(): Int = 40 }; trait T; class B { def value(): Int = 7 }",
    );
    let caller = (
        SourceId::from_index(2),
        "Consumer.scala",
        "package demo\nclass Consumer { def first(a: A): Int = a.value(); def second(b: B): Int = b.value() }",
    );
    let broken = (
        SourceId::from_index(43),
        "Broken.scala",
        "class Broken { val n: Int 1 }",
    );
    for files in [[model, caller, broken], [broken, caller, model]] {
        for workers in [1, 2] {
            let (artifacts, reports) = {
                let mut store = SemanticStore::new();
                let defs = Definitions::bootstrap(&mut store);
                let mut packages = Packages::new();
                let (units, reports) = name_units_with_diagnostics(
                    &files,
                    parsing::parse_files(&files, workers),
                    &mut store,
                    &mut packages,
                );
                let sources = NamedClassSources::new(&units, &store).unwrap();
                let headers = complete_signatures(&units, &mut store, defs, &packages).unwrap();
                let (output, bodies) = type_bodies(&sources, &mut store, defs, &packages, headers);
                let plan = JvmCompilationUnit {
                    classes: class_plans(&units, &bodies, &output, &store).unwrap(),
                    store: &store,
                    typed_ast: &output.arena,
                };
                let prepared = plan.prepare_emission(&defs).unwrap();
                (emit_batch(&prepared, &plan.classes, 2).0, reports)
            };
            let mut names: Vec<_> = artifacts
                .iter()
                .map(GeneratedClass::internal_name)
                .collect();
            names.sort_unstable();
            assert_eq!(names, ["demo/A", "demo/B", "demo/Consumer"]);
            assert_eq!(
                run_java_client("projected-siblings", "ProjectedClassClient", &artifacts)
                    .lines()
                    .collect::<Vec<_>>(),
                ["40", "7"]
            );
            assert_eq!(reports.len(), 2);
            let (omission, syntax) = if files[0].0 == model.0 {
                (&reports[0], &reports[1])
            } else {
                (&reports[1], &reports[0])
            };
            assert!(
                matches!(omission, PreparationDiagnostic::Declaration(report) if matches!(report.error, dotty::source_declarations::DeclarationError::Unsupported {source,..} if source == model.0))
            );
            assert!(matches!(
                syntax,
                PreparationDiagnostic::Syntax(SyntaxDiagnostic::Parser(_))
            ));
            for (diagnostic, input, site, code) in [
                (omission, model, "trait T", "declaration.unsupported"),
                (
                    syntax,
                    broken,
                    "1",
                    "parser.declaration.expected_value_equals",
                ),
            ] {
                assert_eq!(diagnostic.code(), Some(code));
                let span = diagnostic.location().unwrap();
                assert_eq!((diagnostic.source(), span.source()), (input.0, input.0));
                assert_eq!(
                    dotty::core::SourceText::new(input.2)
                        .unwrap()
                        .slice(span.span().range())
                        .unwrap(),
                    site
                );
            }
        }
    }
}

#[test]
fn missing_digits_report_the_numeric_component_before_a_successful_recompile() {
    let source = SourceId::from_index(7);
    let file = "A.scala";
    let prefix = "package demo\n/* 😀 */ class A { def value(): Int = ";
    use dotty::lexer::{LexerIssue, NumericComponent, NumericIssue};
    for (literal, component, message) in [
        (
            "0x",
            NumericComponent::BasePrefix { base: 16 },
            "Expected a base-16 digit after the prefix",
        ),
        (
            "1e+",
            NumericComponent::Exponent,
            "Expected a digit in the exponent",
        ),
    ] {
        let text = format!("{prefix}{literal} }}");
        let files = [(source, file, text.as_str())];
        let (units, reports) = name_units_with_diagnostics(
            &files,
            parsing::parse_files(&files, 1),
            &mut SemanticStore::new(),
            &mut Packages::new(),
        );
        assert!(units.is_empty());
        let [report] = reports.as_slice() else {
            panic!("{reports:?}")
        };
        let PreparationDiagnostic::Syntax(SyntaxDiagnostic::Lexer { diagnostic, .. }) = report
        else {
            panic!("expected a lexer issue: {report:?}");
        };
        assert_eq!(
            diagnostic.issue(),
            &LexerIssue::Numeric(NumericIssue::MissingDigits { component })
        );
        let report = report.render();
        assert_eq!(report.code, Some("lexer.numeric.missing_digits"));
        assert_eq!(report.message, message);
        assert_eq!(report.severity, dotty::core::DiagnosticSeverity::Error);
        let span = report.location.unwrap();
        assert_eq!(span.source(), source);
        assert_eq!(span.span().range().start() as usize, prefix.len());
        assert_eq!(
            span.span().range().end() as usize,
            prefix.len() + literal.len()
        );
    }
    let repaired = format!("{prefix}40 }}");
    let classes = compile([(source, file, repaired.as_str())]);
    assert_eq!(
        run_java_client("repaired-number", "SurvivingClassClient", &classes).trim(),
        "40"
    );
}

// Scheduling belongs to the consumer. The index identifies the owning source
// even when different files reuse the same input tree slots.
#[derive(Debug, Default)]
struct TypedHeaders {
    ready: Vec<SymbolId>,
    arena: AstArena<Typed>,
    bindings: ClassHeaderBindings,
}

#[derive(Debug)]
enum SignatureError {
    Header(dotty::ClassHeaderError),
    Member(dotty::MemberSignatureError),
    Cycle(std::sync::Arc<dotty::SignatureCycle>),
}

impl From<dotty::ClassHeaderError> for SignatureError {
    fn from(error: dotty::ClassHeaderError) -> Self {
        Self::Header(error)
    }
}

fn provider_cycle(
    dependencies: &dotty::SignatureDependencies,
    provider: SymbolId,
) -> Option<std::sync::Arc<dotty::SignatureCycle>> {
    dependencies.waiting_on(provider)?;
    match dependencies.blockages([provider]).pop()? {
        (_, dotty::SignatureBlockage::CycleMember(cycle)) => Some(cycle),
        _ => None,
    }
}

fn complete_headers(
    sources: &NamedClassSources,
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
    observe: &mut impl FnMut(SymbolId, SignatureCompletion),
) -> Result<TypedHeaders, SignatureError> {
    let mut headers = TypedHeaders::default();
    let mut phase = dotty::ClassHeaderPhase::try_prepare(
        sources,
        store,
        definitions,
        packages,
        &mut headers.arena,
        &mut headers.bindings,
    )
    .map_err(|failure| SignatureError::Header(failure.error))?;
    for (_, root) in sources.classes() {
        if phase.is_ready(root.symbol) {
            continue;
        }
        let mut ready = vec![root.symbol];
        while let Some(symbol) = ready.pop() {
            let Ok(attempt) = phase.attempt(symbol) else {
                let failure = phase.into_failure(symbol).expect("rejected header");
                return Err(SignatureError::Header(failure.error));
            };
            observe(symbol, attempt.completion);
            ready.extend(attempt.retry.into_iter().rev());
            if let SignatureCompletion::BlockedOn(provider) = attempt.completion {
                if let Some(cycle) = provider_cycle(phase.dependencies(), provider) {
                    return Err(SignatureError::Cycle(cycle));
                }
                ready.push(provider);
            }
        }
    }
    headers.ready = phase.finish().ready;
    Ok(headers)
}

fn complete_signatures(
    units: &[NamedUnit],
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
) -> Result<TypedHeaders, SignatureError> {
    complete_signatures_in_order(units, units, store, definitions, packages, |_, _| {})
}

fn complete_signatures_in_order<'a>(
    units: &'a [NamedUnit],
    roots: impl IntoIterator<Item = &'a NamedUnit>,
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
    observe: impl FnMut(SymbolId, SignatureCompletion),
) -> Result<TypedHeaders, SignatureError> {
    complete_methods_in_order(
        &NamedClassSources::new(units, store).unwrap(),
        roots.into_iter().flat_map(NamedUnit::method_symbols),
        store,
        definitions,
        packages,
        observe,
    )
}

fn complete_methods_in_order(
    sources: &NamedClassSources,
    roots: impl IntoIterator<Item = SymbolId>,
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
    mut observe: impl FnMut(SymbolId, SignatureCompletion),
) -> Result<TypedHeaders, SignatureError> {
    let headers = complete_headers(sources, store, definitions, packages, &mut observe)?;

    let selection = sources
        .member_signatures(headers.ready.iter().copied(), store)
        .expect("headers completed for these indexed classes");
    complete_member_paths(
        &selection,
        selection.field_roots().iter().copied().chain(roots),
        store,
        definitions,
        packages,
        &mut observe,
    )?;
    Ok(headers)
}

fn complete_member_paths(
    selection: &dotty::MemberSignatureSelection<'_, '_>,
    roots: impl IntoIterator<Item = SymbolId>,
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
    observe: &mut impl FnMut(SymbolId, SignatureCompletion),
) -> Result<(), SignatureError> {
    let mut phase = dotty::MemberSignaturePhase::new(selection, store, definitions, packages);
    let mut ready = Vec::new();
    for root in roots {
        if phase.is_ready(root) {
            continue;
        }
        ready.push(root);
        while let Some(symbol) = ready.pop() {
            let Ok(attempt) = phase.attempt(symbol) else {
                return Err(SignatureError::Member(
                    phase.into_failure(symbol).expect("rejected member"),
                ));
            };
            observe(symbol, attempt.completion);
            ready.extend(attempt.retry.into_iter().rev());
            if let SignatureCompletion::BlockedOn(provider) = attempt.completion {
                if let Some(cycle) = provider_cycle(phase.dependencies(), provider) {
                    return Err(SignatureError::Cycle(cycle));
                }
                ready.push(provider);
            }
        }
    }
    Ok(())
}

fn with_plan<R>(
    files: &[(SourceId, &str, &str)],
    run: impl FnOnce(&JvmCompilationUnit<'_>, &Definitions) -> R,
) -> R {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let units = name_units(
        files,
        parsing::parse_files(files, 4),
        &mut store,
        &mut packages,
    );
    let sources = NamedClassSources::new(&units, &store).unwrap();
    let headers = complete_methods_in_order(
        &sources,
        sources.files().flat_map(NamedUnit::method_symbols),
        &mut store,
        definitions,
        &packages,
        |_, _| {},
    )
    .unwrap();
    let (output, typed_classes) =
        type_bodies(&sources, &mut store, definitions, &packages, headers);
    let classes = class_plans(&units, &typed_classes, &output, &store).unwrap();
    let plan = JvmCompilationUnit {
        store: &store,
        typed_ast: &output.arena,
        classes,
    };
    run(&plan, &definitions)
}

fn named_classes(
    units: &[NamedUnit],
) -> impl DoubleEndedIterator<Item = (&NamedUnit, &SourceClass)> {
    units
        .iter()
        .flat_map(|unit| unit.classes().iter().map(move |class| (unit, class)))
}

fn type_bodies(
    sources: &NamedClassSources,
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
    mut headers: TypedHeaders,
) -> (TypedHeaders, TypedClassBodies) {
    let mut typed_classes = TypedClassBodies::default();
    for (_, class) in sources.classes() {
        let body = sources
            .type_class_body(
                class.symbol,
                store,
                definitions,
                packages,
                &mut headers.arena,
                dotty::core::NoResolver,
            )
            .unwrap();
        typed_classes.record(body).unwrap();
    }

    (headers, typed_classes)
}

fn class_plans(
    units: &[NamedUnit],
    typed_classes: &TypedClassBodies,
    output: &TypedHeaders,
    store: &SemanticStore,
) -> Result<Vec<JvmClassPlan>, ClassPlanError> {
    let mut classes = Vec::new();
    for (unit, class) in named_classes(units) {
        let body = typed_classes.get(class.symbol)?;
        let planner = JvmClassPlanner::new(store, unit.ast(), unit.index())
            .with_header_bindings(&output.arena, &output.bindings);
        classes.push(planner.plan_typed_class(class, &output.arena, body)?);
    }
    Ok(classes)
}

fn emit_batch(
    prepared: &PreparedJvmEmission<'_>,
    classes: &[JvmClassPlan],
    workers: usize,
) -> (Vec<GeneratedClass>, Duration) {
    let chunk_size = classes.len().div_ceil(workers).max(1);
    std::thread::scope(|scope| {
        let tasks: Vec<_> = classes
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(|| {
                    let start = Instant::now();
                    let artifacts = chunk
                        .iter()
                        .map(|class| prepared.emit_class(class.class).unwrap())
                        .collect::<Vec<_>>();
                    (artifacts, start.elapsed())
                })
            })
            .collect();
        let mut artifacts = Vec::with_capacity(classes.len());
        let mut worker_time = Duration::ZERO;
        for task in tasks {
            let (chunk, elapsed) = task.join().unwrap();
            artifacts.extend(chunk);
            worker_time += elapsed;
        }
        (artifacts, worker_time)
    })
}

fn compile<const N: usize>(files: [(SourceId, &str, &str); N]) -> Vec<GeneratedClass> {
    with_plan(&files, |plan, definitions| {
        let prepared = plan.prepare_emission(definitions).unwrap();
        let (concurrent, _) = emit_batch(&prepared, &plan.classes, 2);
        for (class, artifact) in plan.classes.iter().zip(&concurrent) {
            assert_eq!(
                &emit_class(plan, class.class, definitions).unwrap(),
                artifact
            );
        }
        concurrent
    })
}

const OVERLOADS: &str = "class A {
  def take(n: Int): Int = n
  def take(flag: Boolean): Int = 7
}";

fn assert_overload_target(
    sources: &NamedClassSources<'_>,
    method: SymbolId,
    provider: &NamedUnit,
    expected: Declaration,
    declaration: &str,
) {
    use dotty::core::{SourceDefinition, TextRange};

    assert_eq!(method, expected.symbol);
    let (target, location) = sources.declaration(method).unwrap();
    assert_eq!(
        (location.source, location.class),
        (provider.source(), provider.classes()[0].symbol)
    );
    let Some(SourceDefinition::Canonical { source, tree }) = target.index().definition_of(method)
    else {
        panic!("expected source method")
    };
    assert_eq!((source, tree), (provider.source(), expected.tree));
    let span = target.ast().get(tree).position.unwrap();
    assert_eq!(span.source(), provider.source());
    let start = OVERLOADS.find(declaration).unwrap() as u32;
    assert_eq!(
        span.span().range(),
        TextRange::new(start, start + declaration.len() as u32).unwrap()
    );
}

#[test]
fn selected_method_tracks_an_edited_overload_in_both_file_orders() {
    const BEFORE: &str = "class B { def read(a: A): Int = a.take(42) }";
    let a = SourceId::from_index(7);
    let b = SourceId::from_index(2);
    let edited = BEFORE.replace("42", "false");
    for reverse in [false, true] {
        for (text, overload, declaration) in [
            (BEFORE, 0, "def take(n: Int): Int = n"),
            (edited.as_str(), 1, "def take(flag: Boolean): Int = 7"),
        ] {
            let mut files = [(a, "A.scala", OVERLOADS), (b, "B.scala", text)];
            if reverse {
                files.reverse();
            }
            let mut store = SemanticStore::new();
            let defs = Definitions::bootstrap(&mut store);
            let mut packages = Packages::new();
            let units = name_units(
                &files,
                parsing::parse_files(&files, 2),
                &mut store,
                &mut packages,
            );
            let headers = complete_signatures(&units, &mut store, defs, &packages).unwrap();
            let sources = NamedClassSources::new(&units, &store).unwrap();
            let (output, bodies) = type_bodies(&sources, &mut store, defs, &packages, headers);
            let caller = sources.file(b).unwrap();
            let provider = sources.file(a).unwrap();
            let calls = caller.navigation().unwrap();
            let method_at = |offset| {
                calls.selected_method_at(offset, caller.ast(), &bodies, &output.arena, &store)
            };
            let name_start = text.find("take(").unwrap() as u32;
            let method = method_at(name_start).unwrap();
            for offset in name_start..name_start + 4 {
                assert_eq!(method_at(offset), Some(method));
            }
            // Receiver, dot, opening parenthesis, argument and closing parenthesis.
            let close = text.rfind(')').unwrap() as u32;
            for offset in (name_start - 2..name_start).chain(name_start + 4..=close) {
                assert_eq!(method_at(offset), None, "offset {offset} in {text}");
            }
            assert_eq!(method_at(text.len() as u32), None);
            let expected = provider.methods().nth(overload).unwrap();
            assert_overload_target(&sources, method, provider, expected, declaration);

            let body = bodies.get(caller.classes()[0].symbol).unwrap();
            let read = caller.methods().next().unwrap();
            let TreeKind::DefDef(def) = &caller.ast().get(read.tree).kind else {
                panic!("expected B.read")
            };
            let call = def.rhs.unwrap();
            assert_eq!(
                body.selected_method(read.tree, caller.ast(), &output.arena, &store),
                None
            );
            let other_body = bodies.get(provider.classes()[0].symbol).unwrap();
            assert_eq!(
                other_body.selected_method_at(
                    name_start,
                    caller.navigation().unwrap().call_names(),
                    caller.ast(),
                    &output.arena,
                    &store
                ),
                None
            );
            assert_eq!(
                other_body.selected_method(call, caller.ast(), &output.arena, &store),
                None
            );
        }
    }
}

#[test]
fn checked_calls_survive_a_failing_sibling_in_both_file_orders() {
    let a = SourceId::from_index(7);
    let b = SourceId::from_index(2);
    for reverse in [false, true] {
        for (argument, broken) in [("false, 1", true), ("1", false)] {
            let text = format!(
                "class B {{ def good(a: A): Int = a.take(42); def bad(a: A): Int = a.take({argument}) }}"
            );
            let mut files = [(a, "A.scala", OVERLOADS), (b, "B.scala", text.as_str())];
            if reverse {
                files.reverse();
            }
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
            let sources = NamedClassSources::new(&units, &store).unwrap();
            let caller = sources.file(b).unwrap();
            let class = &caller.classes()[0];
            let check = dotty::check_class_body(
                class,
                caller.ast(),
                caller.index(),
                &mut store,
                defs,
                &packages,
                &mut headers.arena,
                dotty::core::NoResolver,
            );
            let at = |offset| {
                caller.navigation().unwrap().checked_method_at(
                    offset,
                    caller.ast(),
                    &headers.arena,
                    &store,
                    |owner| (owner == class.symbol).then_some(&check),
                )
            };
            let provider = sources.file(a).unwrap();
            let method = at(text.find("take").unwrap() as u32).unwrap();
            assert_overload_target(
                &sources,
                method,
                provider,
                provider.methods().next().unwrap(),
                "def take(n: Int): Int = n",
            );
            assert_eq!(
                at(text.rfind("take").unwrap() as u32),
                (!broken).then_some(method)
            );
            let target = caller
                .navigation()
                .unwrap()
                .definition_at(
                    text.find("take").unwrap() as u32,
                    caller.ast(),
                    &headers.arena,
                    &store,
                    |owner| (owner == class.symbol).then_some(&check),
                    |id| sources.file(id).map(|unit| unit.navigation().unwrap()),
                )
                .unwrap();
            assert_eq!(target.source, a);
            let slice = |range: dotty::core::TextRange| {
                &OVERLOADS[range.start() as usize..range.end() as usize]
            };
            assert_eq!(slice(target.declaration), "def take(n: Int): Int = n");
            assert_eq!(slice(target.name), "take");
            assert_eq!(target.name.start(), OVERLOADS.find("take").unwrap() as u32);
            if broken {
                let errors = check.body.unwrap_err();
                let [dotty::ClassBodyError::Member { member, .. }] = errors.as_slice() else {
                    panic!("expected one failed member")
                };
                assert_eq!(
                    *member,
                    dotty::source_declarations::ClassMember::Method(
                        class.methods().nth(1).unwrap()
                    )
                );
            } else {
                JvmClassPlanner::new(&store, caller.ast(), caller.index())
                    .with_header_bindings(&headers.arena, &headers.bindings)
                    .plan_typed_class(class, &headers.arena, &check.body.unwrap())
                    .unwrap();
            }
        }
    }
}

#[test]
fn selected_method_routes_two_classes_in_one_file_as_their_bodies_arrive() {
    const B: &str = "class First { def read(a: A): Int = a.take(42) }
class Second { def read(a: A): Int = a.take(false) }";
    let a = SourceId::from_index(7);
    let b = SourceId::from_index(2);
    for reverse in [false, true] {
        let mut files = [(a, "A.scala", OVERLOADS), (b, "B.scala", B)];
        if reverse {
            files.reverse();
        }
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
        let sources = NamedClassSources::new(&units, &store).unwrap();
        let caller = sources.file(b).unwrap();
        let provider = sources.file(a).unwrap();
        let calls = caller.navigation().unwrap();
        let offsets = [
            B.find("take(42)").unwrap() as u32,
            B.find("take(false)").unwrap() as u32,
        ];
        let targets = [
            provider.methods().next().unwrap(),
            provider.methods().nth(1).unwrap(),
        ];
        let mut bodies = TypedClassBodies::default();
        for (n, class) in caller.classes().iter().enumerate() {
            let body = dotty::type_class_body(
                class,
                caller.ast(),
                caller.index(),
                &mut store,
                defs,
                &packages,
                &mut headers.arena,
            )
            .unwrap();
            bodies.record(body).unwrap();
            assert_eq!(
                offsets.map(|offset| calls.selected_method_at(
                    offset,
                    caller.ast(),
                    &bodies,
                    &headers.arena,
                    &store
                )),
                [
                    Some(targets[0].symbol),
                    (n == 1).then_some(targets[1].symbol)
                ]
            );
        }
        for ((offset, expected), declaration) in offsets.into_iter().zip(targets).zip([
            "def take(n: Int): Int = n",
            "def take(flag: Boolean): Int = 7",
        ]) {
            let method = calls
                .selected_method_at(offset, caller.ast(), &bodies, &headers.arena, &store)
                .unwrap();
            assert_overload_target(&sources, method, provider, expected, declaration);
        }
    }
}

#[test]
fn selected_method_distinguishes_a_direct_call_from_an_inserted_apply() {
    let text = "class Callable {
           def apply(n: Int): Int = n
           def direct(): Int = apply(42)
           def indirect(c: Callable): Int = c(42)
         }";
    let (mut store, defs, packages, unit) = class_planning::named(text);
    let units = std::slice::from_ref(&unit);
    let headers = complete_signatures(units, &mut store, defs, &packages).unwrap();
    let (output, bodies) = type_bodies(
        &dotty::NamedClassSources::new(units, &store).unwrap(),
        &mut store,
        defs,
        &packages,
        headers,
    );
    let body = bodies.get(unit.classes()[0].symbol).unwrap();
    let mut methods = unit.methods();
    let apply = methods.next().unwrap().symbol;
    for (spelling, expected) in [("apply(42)", Some(apply)), ("c(42)", None)] {
        let TreeKind::DefDef(def) = &unit.ast().get(methods.next().unwrap().tree).kind else {
            panic!("expected method")
        };
        let call = def.rhs.unwrap();
        assert!(matches!(unit.ast().get(call).kind, TreeKind::Apply(_)));
        assert_eq!(
            body.selected_method(call, unit.ast(), &output.arena, &store),
            expected
        );
        let offset = text.find(spelling).unwrap() as u32;
        assert_eq!(
            unit.navigation().unwrap().call_names().call_at(offset),
            Some(call)
        );
        assert_eq!(
            body.selected_method_at(
                offset,
                unit.navigation().unwrap().call_names(),
                unit.ast(),
                &output.arena,
                &store
            ),
            expected
        );
    }
}

#[test]
fn separate_sources_compile_in_either_order_and_execute_on_jdk21() {
    let a = (
        SourceId::from_index(0),
        "A.scala",
        include_str!("../A.scala"),
    );
    let b = (
        SourceId::from_index(1),
        "B.scala",
        include_str!("../B.scala"),
    );
    let first = compile([a, b]);
    let second = compile([b, a]);
    assert!(
        first
            .iter()
            .map(|c| c.internal_name())
            .eq(["demo/A", "demo/B"])
    );
    assert!(first.iter().eq(second.iter().rev()));

    assert_eq!(run_java_client("separate", "Client", &first).trim(), "7");
}

#[test]
fn same_named_methods_in_one_source_execute_on_jdk21_in_either_file_order() {
    let providers = (
        SourceId::from_index(0),
        "Providers.scala",
        include_str!("../Providers.scala"),
    );
    let consumer = (
        SourceId::from_index(1),
        "Use.scala",
        include_str!("../Use.scala"),
    );
    for (order, files) in [
        ("providers-first", [providers, consumer]),
        ("consumer-first", [consumer, providers]),
    ] {
        let classes = compile(files);
        let mut names: Vec<_> = classes.iter().map(|class| class.internal_name()).collect();
        names.sort_unstable();
        assert_eq!(names, ["demo/A", "demo/B", "demo/Use"], "{order}");
        let stdout = run_java_client(order, "ProviderClient", &classes);
        assert_eq!(
            stdout.lines().collect::<Vec<_>>(),
            ["17", "42", "5"],
            "{order}"
        );
    }
}

#[test]
fn method_plan_rejects_another_methods_body_in_the_same_source() {
    use dotty::jvm_method_plan::MethodPlanError;

    let files = [(
        SourceId::from_index(0),
        "Pair.scala",
        "class Pair { def first(): Int = { var n = 1; n }; def second(): Int = { val n = 2; n } }",
    )];
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let units = name_units(
        &files,
        parsing::parse_files(&files, 1),
        &mut store,
        &mut packages,
    );
    let unit = &units[0];
    let [first, second] = unit.methods().collect::<Vec<_>>()[..] else {
        panic!("expected two methods")
    };
    let mut typer = unit.typer(&mut store, definitions, &packages);
    let first_root = typer.type_method_definition(first.tree).unwrap();
    typer.type_method_definition(second.tree).unwrap();
    let mut output = typer.into_typed_output();
    let planner = JvmMethodPlanner::new(&store, unit.ast(), unit.index(), &output);
    let first_plan = planner.plan(first.symbol).unwrap();
    let second_plan = planner.plan(second.symbol).unwrap();
    assert_eq!(first_plan.locals.len(), 1);
    assert_eq!(second_plan.locals.len(), 1);
    let first_local = first_plan.locals[0];
    let second_local = second_plan.locals[0];
    assert_eq!(store.symbols.owner(first_local.symbol), Some(first.symbol));
    assert_eq!(
        store.symbols.owner(second_local.symbol),
        Some(second.symbol)
    );
    assert!(first_local.mutable);
    assert!(!second_local.mutable);
    assert_ne!(first_plan.body, second_plan.body);

    let TreeKind::DefDef(definition) = &mut output.typed_ast.get_mut(first_root).kind else {
        panic!("expected typed method")
    };
    definition.rhs = second_plan.body;
    assert_eq!(
        JvmMethodPlanner::new(&store, unit.ast(), unit.index(), &output).plan(first.symbol),
        Err(MethodPlanError::BodyMismatch {
            method: first.symbol,
            expected: first_plan.body.unwrap(),
            actual: second_plan.body.unwrap()
        })
    );
}

fn run_java_client(label: &str, main_class: &str, classes: &[GeneratedClass]) -> String {
    let output = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("compiler-client-{label}-{}", std::process::id()));
    fs::create_dir(&output).unwrap();
    fs::create_dir(output.join("demo")).unwrap();
    for class in classes {
        fs::write(
            output.join(format!("{}.class", class.internal_name())),
            class.bytes(),
        )
        .unwrap();
    }
    let java_source = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("{main_class}.java"));
    let javac = Command::new("javac")
        .current_dir(&output)
        .args(["--release", "21", "-cp", ".", "-d", "."])
        .arg(java_source)
        .output()
        .unwrap();
    assert!(
        javac.status.success(),
        "{}",
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = Command::new("java")
        .current_dir(&output)
        .args(["-Xverify:all", "-cp", ".", main_class])
        .output()
        .unwrap();
    assert!(
        java.status.success(),
        "{}",
        String::from_utf8_lossy(&java.stderr)
    );
    fs::remove_dir_all(output).unwrap();
    String::from_utf8(java.stdout).unwrap()
}

#[test]
#[ignore = "release-mode JVM scaling measurement"]
fn measure_jvm_batch() {
    use std::{hint::black_box, time::Instant};
    for count in [1, 10, 100] {
        let sources: Vec<_> = (0..count)
            .map(|i| format!("package demo\nclass C{i}:\n  def value(): Int = 7\n"))
            .collect();
        let files: Vec<_> = sources
            .iter()
            .enumerate()
            .map(|(i, text)| (SourceId::from_index(i as u32), "Batch.scala", text.as_str()))
            .collect();
        with_plan(&files, |plan, defs| {
            for _ in 0..5 {
                let start = Instant::now();
                plan.validate().unwrap();
                let validated = Instant::now();
                black_box(plan.metadata(defs).unwrap());
                let projected = Instant::now();
                for class in &plan.classes {
                    black_box(emit_class(plan, class.class, defs).unwrap());
                }
                let emitted = Instant::now();
                let prepared = plan.prepare_emission(defs).unwrap();
                let ready = Instant::now();
                for class in &plan.classes {
                    black_box(prepared.emit_class(class.class).unwrap());
                }
                eprintln!(
                    "classes={count} validate={:?} metadata_inclusive={:?} one_shot_batch={:?} prepare={:?} prepared_batch={:?}",
                    validated - start,
                    projected - validated,
                    emitted - projected,
                    ready - emitted,
                    ready.elapsed()
                );
            }
        });
    }
}

fn signature_pair(
    caller: &str,
    store: &mut SemanticStore,
    packages: &mut Packages,
) -> Vec<NamedUnit> {
    let files = [
        (SourceId::from_index(7), "A.scala", caller),
        (
            SourceId::from_index(2),
            "B.scala",
            "class B { def g() = 7 }",
        ),
    ];
    name_units(&files, parsing::parse_files(&files, 2), store, packages)
}

#[test]
fn cross_file_signature_completes_the_dependency_then_retries_the_original_method() {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let units = signature_pair("class A { def f(b: B) = b.g() }", &mut store, &mut packages);
    let f = units[0].method_symbol();
    let g = units[1].method_symbol();
    let mut attempts = Vec::new();
    complete_methods_in_order(
        &NamedClassSources::new(&units, &store).unwrap(),
        [f],
        &mut store,
        defs,
        &packages,
        |method, outcome| {
            if method == f || method == g {
                attempts.push((method, outcome));
            }
        },
    )
    .unwrap();
    let signature = |method| {
        let SymbolInfo::Complete(ty) = *store.symbols.info(method) else {
            panic!("missing signature")
        };
        let dotty::core::Type::Method(info) = store.types.get(ty) else {
            panic!("expected method")
        };
        assert_eq!(info.result, defs.int);
        SignatureCompletion::Complete(ty)
    };
    assert_eq!(
        attempts,
        [
            (f, SignatureCompletion::BlockedOn(g)),
            (g, signature(g)),
            (f, signature(f))
        ]
    );
}

#[test]
fn cross_file_call_failures_survive_retries_and_leave_the_provider_executable() {
    let provider = (
        SourceId::from_index(2),
        "A.scala",
        "package demo\nclass A { def value() = 40; def take(n: Int) = n }",
    );
    let caller = (
        SourceId::from_index(7),
        "B.scala",
        "package demo\nclass B { def f(a: A) = a.value(1); def h(a: A) = a.take(false) }",
    );
    for files in [[caller, provider], [provider, caller]] {
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
        let lookup = |source| sources.file(source).map(|u| u.ast());
        let mut headers =
            complete_headers(&sources, &mut store, defs, &packages, &mut |_, _| {}).unwrap();
        let selection = sources
            .member_signatures(headers.ready.iter().copied(), &store)
            .unwrap();
        let mut blocked = Vec::new();
        let mut reports = Vec::new();
        for unit in &units {
            for member in unit.method_symbols() {
                let result = complete_member_paths(
                    &selection,
                    [member],
                    &mut store,
                    defs,
                    &packages,
                    &mut |owner, outcome| {
                        if let SignatureCompletion::BlockedOn(dependency) = outcome {
                            blocked.push((owner, dependency));
                        }
                    },
                );
                if unit.source() == provider.0 {
                    result.unwrap();
                    continue;
                }
                let SignatureError::Member(error) = result.unwrap_err() else {
                    panic!("expected a member failure")
                };
                let report = error.into_reported_error(lookup).unwrap();
                assert_eq!(report.member(), Some(member));
                assert_eq!(report.source(), Some(caller.0));
                assert!(matches!(store.symbols.info(member), SymbolInfo::Missing));
                reports.push(report);
            }
        }
        let bad = sources.file(caller.0).unwrap();
        let good = sources.file(provider.0).unwrap();
        let expected: Vec<_> = if files[0].0 == caller.0 {
            bad.method_symbols().zip(good.method_symbols()).collect()
        } else {
            vec![]
        };
        assert_eq!(blocked, expected);

        let check = dotty::check_class_body(
            &bad.classes()[0],
            bad.ast(),
            bad.index(),
            &mut store,
            defs,
            &packages,
            &mut headers.arena,
            dotty::core::NoResolver,
        );
        let retries = check.body.unwrap_err();
        assert_eq!((reports.len(), retries.len()), (2, 2));
        assert!(matches!(
            reports[0].error(),
            TyperError::ApplicationArityMismatch {
                expected: 0,
                actual: 1,
                ..
            }
        ));
        assert!(
            matches!(reports[1].error(), TyperError::ApplicationArgumentTypeMismatch { argument_index:0, expected, actual,.. } if *expected == defs.int && *actual == defs.boolean)
        );
        for ((original, retry), (site, code)) in reports.iter().zip(retries).zip([
            ("a.value(1)", "typer.application_arity_mismatch"),
            ("false", "typer.application_argument_type_mismatch"),
        ]) {
            let retry = retry.into_reported_error(lookup).unwrap();
            assert_eq!(
                retry.call_identity(&defs),
                Some(original.call_identity(&defs).unwrap())
            );
            assert_eq!(retry.render(), original.render());
            assert_eq!(original.code(), Some(code));
            let offset = caller.2.find(site).unwrap() as u32;
            let span = original.location().unwrap();
            assert_eq!(span.source(), caller.0);
            assert_eq!(
                span.span().range(),
                dotty::core::TextRange::new(offset, offset + site.len() as u32).unwrap()
            );
        }
        let (output, bodies) = type_bodies(
            &dotty::NamedClassSources::new(std::slice::from_ref(good), &store).unwrap(),
            &mut store,
            defs,
            &packages,
            headers,
        );
        let plan = JvmCompilationUnit {
            classes: class_plans(std::slice::from_ref(good), &bodies, &output, &store).unwrap(),
            store: &store,
            typed_ast: &output.arena,
        };
        let artifact = emit_class(&plan, good.classes()[0].symbol, &defs).unwrap();
        assert_eq!(
            run_java_client("call-failures", "SurvivingClassClient", &[artifact]).trim(),
            "40"
        );
    }
}

#[test]
fn cross_file_inference_cycle_reports_the_reentered_method_and_rolls_back() {
    let a = (
        SourceId::from_index(0),
        "A.scala",
        "class A { def left(b: B) = b.right(this) }",
    );
    let b = (
        SourceId::from_index(1),
        "B.scala",
        "class B { def right(a: A) = a.left(this) }",
    );
    for files in [[a, b], [b, a]] {
        for first in [0, 1] {
            let mut store = SemanticStore::new();
            let definitions = Definitions::bootstrap(&mut store);
            let mut packages = Packages::new();
            let units = name_units(
                &files,
                parsing::parse_files(&files, 4),
                &mut store,
                &mut packages,
            );
            let error = complete_signatures_in_order(
                &units,
                [&units[first], &units[1 - first]],
                &mut store,
                definitions,
                &packages,
                |_, _| {},
            )
            .unwrap_err();
            assert!(
                matches!(error, SignatureError::Cycle(ref cycle) if cycle.path() == [units[first].method_symbol(), units[1-first].method_symbol(), units[first].method_symbol()]),
                "{error:?}"
            );
            for unit in &units {
                assert!(matches!(
                    store.symbols.info(unit.method_symbol()),
                    SymbolInfo::Missing
                ));
            }
        }
    }
}
