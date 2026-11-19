//! The consumer owns phase ordering, plans and output for classes with a no-arg constructor.
#[path = "support/class_planning.rs"]
mod class_planning;
#[path = "support/parsing.rs"]
mod parsing;
#[path = "support/pipeline_timing.rs"]
mod pipeline_timing;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use dotty::core::{
    AstArena, Definitions, Packages, SemanticStore, SourceId, SourceSemanticIndex, SymbolId,
    SymbolInfo, TreeKind, Typed, Untyped,
};
use dotty::jvm::{
    GeneratedClass, JvmClassPlan, JvmCompilationUnit, PreparedJvmEmission, emit_class,
};
use dotty::jvm_class_plan::{
    ClassHeaderBindings, ClassPlanError, JvmClassPlanner, TypedClassBodies,
};
use dotty::jvm_method_plan::JvmMethodPlanner;
use dotty::namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty::parser::{DetachedParse, ParseResult};
use dotty::source_declarations::{Declaration, SourceClass, source_classes};
use dotty::typer::{SignatureCompletion, SourceTyper, TyperError};

struct NamedUnit {
    source: SourceId,
    ast: AstArena<Untyped>,
    index: SourceSemanticIndex,
    classes: Vec<SourceClass>,
}

impl NamedUnit {
    fn methods(&self) -> impl Iterator<Item = Declaration> + '_ {
        self.classes.iter().flat_map(SourceClass::methods)
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
            &self.ast,
            self.source,
            &self.index,
            store,
            definitions,
            packages,
        )
    }
}

fn name_source(
    id: SourceId,
    name: &str,
    parsed: &ParseResult,
    store: &mut SemanticStore,
    packages: &mut Packages,
) -> SourceSemanticIndex {
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let NamingOutcome::Named { index, diagnostics } = name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        id,
        name,
        SourceParseStatus::Clean,
        store,
        packages,
    )
    .unwrap() else {
        panic!("fixture rejected during naming")
    };
    assert!(diagnostics.is_empty(), "{diagnostics:?}");

    *index
}

fn enter(
    id: SourceId,
    name: &str,
    parsed: ParseResult,
    store: &mut SemanticStore,
    packages: &mut Packages,
) -> NamedUnit {
    let index = name_source(id, name, &parsed, store, packages);
    let classes = source_classes(&parsed.ast, parsed.root, id, &index, store).unwrap();
    NamedUnit {
        source: id,
        ast: parsed.ast,
        index,
        classes,
    }
}

fn name_units(
    files: &[(SourceId, &str, &str)],
    parsed: Vec<DetachedParse>,
    store: &mut SemanticStore,
    packages: &mut Packages,
) -> Vec<NamedUnit> {
    files
        .iter()
        .zip(parsed)
        .map(|(&(id, name, _), detached)| {
            let parsed = detached.into_session(&mut store.names);
            enter(id, name, parsed, store, packages)
        })
        .collect()
}

// Scheduling belongs to the consumer. The index identifies the owning source
// even when different files reuse the same input tree slots.
#[derive(Debug, Default)]
struct TypedHeaders {
    arena: AstArena<Typed>,
    bindings: ClassHeaderBindings,
}

#[derive(Debug)]
enum SignatureError {
    Typing(TyperError),
    Field(dotty::ClassFieldSignatureError),
    HeaderCycle(Vec<SymbolId>),
}

impl From<TyperError> for SignatureError {
    fn from(error: TyperError) -> Self {
        Self::Typing(error)
    }
}

fn complete_headers(
    units: &[NamedUnit],
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
    observe: &mut impl FnMut(SymbolId, SignatureCompletion),
) -> Result<TypedHeaders, SignatureError> {
    // Parent applications need the declaring file's constructor signature.
    for unit in units {
        let mut typer = unit.typer(store, definitions, packages);
        for class in &unit.classes {
            typer.complete_symbol(class.constructor)?;
        }
    }
    let owners: HashMap<_, _> = named_classes(units)
        .map(|(unit, class)| (class.symbol, unit))
        .collect();
    let mut headers = TypedHeaders::default();
    let mut path = Vec::new();
    let mut active = HashMap::new();
    for (_, root) in named_classes(units) {
        if matches!(store.symbols.info(root.symbol), SymbolInfo::Complete(_)) {
            continue;
        }
        path.push(root.symbol);
        active.insert(root.symbol, 0);
        while let Some(&symbol) = path.last() {
            let unit = owners[&symbol];
            let completion = dotty::try_complete_class_header(
                symbol,
                &unit.ast,
                &unit.index,
                store,
                definitions,
                packages,
                &mut headers.arena,
                &mut headers.bindings,
                |dep| owners.contains_key(&dep),
            )?;
            observe(symbol, completion);
            match completion {
                SignatureCompletion::Complete(_) => {
                    path.pop();
                    active.remove(&symbol);
                }
                SignatureCompletion::BlockedOn(parent) => {
                    if let Some(&start) = active.get(&parent) {
                        let mut cycle = path[start..].to_vec();
                        cycle.push(parent);
                        return Err(SignatureError::HeaderCycle(cycle));
                    }
                    active.insert(parent, path.len());
                    path.push(parent);
                }
            }
        }
    }
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
        units,
        roots.into_iter().flat_map(NamedUnit::method_symbols),
        store,
        definitions,
        packages,
        observe,
    )
}

fn complete_methods_in_order(
    units: &[NamedUnit],
    roots: impl IntoIterator<Item = SymbolId>,
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
    mut observe: impl FnMut(SymbolId, SignatureCompletion),
) -> Result<TypedHeaders, SignatureError> {
    let headers = complete_headers(units, store, definitions, packages, &mut observe)?;
    let owners: HashMap<_, _> = units
        .iter()
        .flat_map(|unit| unit.method_symbols().map(move |symbol| (symbol, unit)))
        .collect();

    for (unit, class) in named_classes(units) {
        while let dotty::ClassFieldSignatureCompletion::BlockedOn { field, dependency } =
            dotty::try_complete_class_field_signatures(
                class.symbol,
                &unit.ast,
                &unit.index,
                store,
                definitions,
                packages,
                |dep| owners.contains_key(&dep),
            )
            .map_err(SignatureError::Field)?
        {
            observe(field, SignatureCompletion::BlockedOn(dependency));
            complete_method(
                dependency,
                &owners,
                store,
                definitions,
                packages,
                &mut observe,
            )?;
        }
    }
    for symbol in roots {
        complete_method(symbol, &owners, store, definitions, packages, &mut observe)?;
    }
    Ok(headers)
}

fn complete_method(
    symbol: SymbolId,
    owners: &HashMap<SymbolId, &NamedUnit>,
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
    observe: &mut impl FnMut(SymbolId, SignatureCompletion),
) -> Result<(), TyperError> {
    if matches!(store.symbols.info(symbol), SymbolInfo::Complete(_)) {
        return Ok(());
    }
    let mut pending = vec![symbol];
    let mut active = HashSet::from([symbol]);
    while let Some(&symbol) = pending.last() {
        let unit = owners[&symbol];
        let completion = unit
            .typer(store, definitions, packages)
            .try_complete_signature(symbol, |dependency| owners.contains_key(&dependency))?;
        observe(symbol, completion);
        match completion {
            SignatureCompletion::Complete(_) => {
                pending.pop();
                active.remove(&symbol);
            }
            SignatureCompletion::BlockedOn(dependency) => {
                if !active.insert(dependency) {
                    return Err(TyperError::RecursiveInferredMethodResult { symbol: dependency });
                }
                pending.push(dependency);
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
    let headers = complete_signatures(&units, &mut store, definitions, &packages).unwrap();

    let (output, typed_classes) = type_bodies(&units, &mut store, definitions, &packages, headers);
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
        .flat_map(|unit| unit.classes.iter().map(move |class| (unit, class)))
}

fn type_bodies(
    units: &[NamedUnit],
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
    mut headers: TypedHeaders,
) -> (TypedHeaders, TypedClassBodies) {
    let mut typed_classes = TypedClassBodies::default();
    for (unit, class) in named_classes(units) {
        let body = dotty::type_class_body(
            class,
            &unit.ast,
            &unit.index,
            store,
            definitions,
            packages,
            &mut headers.arena,
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
        let planner = JvmClassPlanner::new(store, &unit.ast, &unit.index)
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
    let planner = JvmMethodPlanner::new(&store, &unit.ast, &unit.index, &output);
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
        JvmMethodPlanner::new(&store, &unit.ast, &unit.index, &output).plan(first.symbol),
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
        &units,
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
fn cross_file_bad_call_reports_arity_at_the_call_after_dependency_completion() {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let units = signature_pair(
        "class A { def f(b: B) = b.g(1) }",
        &mut store,
        &mut packages,
    );
    let caller = &units[0];
    let f = caller.method_symbol();
    let g = units[1].method_symbol();
    let TreeKind::DefDef(definition) = &caller.ast.get(caller.methods().next().unwrap().tree).kind
    else {
        panic!("expected method")
    };
    let call = definition.rhs.unwrap();
    let mut dependencies = Vec::new();
    let err = complete_methods_in_order(
        &units,
        [f],
        &mut store,
        defs,
        &packages,
        |method, outcome| {
            if let SignatureCompletion::BlockedOn(dependency) = outcome {
                dependencies.push((method, dependency));
            }
        },
    )
    .unwrap_err();
    assert_eq!(dependencies, [(f, g)]);
    assert!(
        matches!(err, SignatureError::Typing(TyperError::ApplicationArityMismatch { source, tree_index, expected: 0, actual: 1 })
        if source == caller.source && tree_index == call.index()),
        "{err:?}"
    );
    assert!(matches!(store.symbols.info(f), SymbolInfo::Missing));
    assert!(matches!(store.symbols.info(g), SymbolInfo::Complete(_)));
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
                matches!(error, SignatureError::Typing(TyperError::RecursiveInferredMethodResult { symbol }) if symbol == units[first].method_symbol()),
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
