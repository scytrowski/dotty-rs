//! The consumer owns phase ordering, plans and output for classes with a no-arg constructor.
#[path = "support/class_planning.rs"]
mod class_planning;
#[path = "support/parsing.rs"]
mod parsing;
#[path = "support/pipeline_timing.rs"]
mod pipeline_timing;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::ops::Range;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use dotty::core::{
    AstArena, Definitions, Packages, SemanticStore, SourceId, SourceSemanticIndex, SymbolId,
    SymbolInfo, TreeId, TreeKind, Typed, Untyped,
};
use dotty::jvm::{
    GeneratedClass, JvmClassPlan, JvmCompilationUnit, PreparedJvmEmission, emit_class,
};
use dotty::jvm_class_plan::{ClassPlanError, JvmClassPlanner, TypedFieldInitializer};
use dotty::jvm_method_plan::JvmMethodPlanner;
use dotty::namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty::parser::{DetachedParse, ParseResult};
use dotty::typer::{
    SignatureCompletion, SourceTypedIndex, SourceTyper, TypedLocalBinding, TyperError,
};

struct NamedUnit {
    source: SourceId,
    ast: AstArena<Untyped>,
    index: SourceSemanticIndex,
    classes: Vec<NamedClass>,
    methods: Vec<TreeId<Untyped>>,
}

struct NamedClass {
    parent_application: Option<TreeId<Untyped>>,
    symbol: SymbolId,
    constructor: SymbolId,
    constructor_fields: Vec<SymbolId>,
    method_range: Range<usize>,
    fields: Vec<SymbolId>,
}

impl NamedUnit {
    fn method_symbol(&self) -> SymbolId {
        let [method] = self.methods.as_slice() else {
            panic!("expected one method")
        };
        self.symbol(*method)
    }

    fn symbol(&self, method: TreeId<Untyped>) -> SymbolId {
        self.index.symbol_at(self.source, method).unwrap()
    }

    fn method_symbols(&self) -> impl Iterator<Item = SymbolId> + '_ {
        self.methods.iter().map(|&method| self.symbol(method))
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

fn enter(
    id: SourceId,
    name: &str,
    parsed: ParseResult,
    store: &mut SemanticStore,
    packages: &mut Packages,
) -> NamedUnit {
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

    let ast = parsed.ast;
    let TreeKind::PackageDef(package) = &ast.get(parsed.root).kind else {
        panic!("expected package")
    };
    let mut methods = Vec::new();
    let mut classes = Vec::with_capacity(package.stats.len());
    for &tree in &package.stats {
        let TreeKind::TypeDef(class) = &ast.get(tree).kind else {
            panic!("expected class")
        };
        let TreeKind::Template(template) = &ast.get(class.rhs).kind else {
            panic!("expected template")
        };
        let start = methods.len();
        let mut fields = Vec::new();
        for &member in &template.body {
            let Some(symbol) = index.symbol_at(id, member) else {
                continue;
            };
            match (&ast.get(member).kind, store.symbols.get(symbol).kind) {
                (TreeKind::DefDef(method), dotty::core::SymbolKind::Method)
                    if method.rhs.is_some() =>
                {
                    methods.push(member)
                }
                (TreeKind::ValDef(field), dotty::core::SymbolKind::Field)
                    if field.rhs.is_some() =>
                {
                    fields.push(symbol);
                }
                _ => {}
            }
        }
        let TreeKind::DefDef(constructor) = &ast.get(template.constructor).kind else {
            panic!("expected constructor")
        };
        let constructor_fields = constructor
            .value_param_clauses
            .iter()
            .flatten()
            .map(|&tree| index.symbol_at(id, tree).unwrap())
            .filter(|&symbol| store.symbols.get(symbol).kind == dotty::core::SymbolKind::Field)
            .collect();
        let parent_application = match template.parents.as_slice() {
            [tree] if matches!(ast.get(*tree).kind, TreeKind::Apply(_)) => Some(*tree),
            _ => None,
        };
        classes.push(NamedClass {
            parent_application,
            constructor_fields,
            symbol: index.symbol_at(id, tree).unwrap(),
            constructor: index.symbol_at(id, template.constructor).unwrap(),
            method_range: start..methods.len(),
            fields,
        });
    }
    NamedUnit {
        source: id,
        ast,
        index: *index,
        classes,
        methods,
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
    by_class: HashMap<SymbolId, std::sync::Arc<SourceTypedIndex>>,
}

#[derive(Debug)]
enum SignatureError {
    Typing(TyperError),
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
            let mut typer = SourceTyper::new_with_typed_ast(
                &unit.ast,
                unit.source,
                &unit.index,
                store,
                definitions,
                packages,
                headers.arena,
            );
            let completion =
                typer.try_complete_signature(symbol, |dep| owners.contains_key(&dep))?;
            observe(symbol, completion);
            let (arena, index) = typer.into_typed_ast_and_index();
            headers.arena = arena;
            match completion {
                SignatureCompletion::Complete(_) => {
                    path.pop();
                    active.remove(&symbol);
                    // Completing one header can also type a parent in the same file.
                    if !index.is_empty() {
                        let index = std::sync::Arc::new(index);
                        for class in &unit.classes {
                            if class
                                .parent_application
                                .is_some_and(|tree| index.get(unit.source, tree).is_some())
                            {
                                headers.by_class.insert(class.symbol, index.clone());
                            }
                        }
                    }
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
    for unit in units {
        let mut typer = unit.typer(store, definitions, packages);
        for class in &unit.classes {
            for &field in &class.constructor_fields {
                typer.complete_symbol(field)?;
            }
            for &field in &class.fields {
                if typer.store().symbols.get(field).visibility == dotty::core::Visibility::Public {
                    typer.complete_symbol(field)?;
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

    for symbol in roots {
        if matches!(store.symbols.info(symbol), SymbolInfo::Complete(_)) {
            continue;
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
                        return Err(TyperError::RecursiveInferredMethodResult {
                            symbol: dependency,
                        }
                        .into());
                    }
                    pending.push(dependency);
                }
            }
        }
    }
    Ok(headers)
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
    let classes = class_plans(&units, typed_classes, &output, &store).unwrap();
    let plan = JvmCompilationUnit {
        store: &store,
        typed_ast: &output,
        classes,
    };
    run(&plan, &definitions)
}

struct ClassBindings {
    header: Option<std::sync::Arc<SourceTypedIndex>>,
    methods: Vec<MethodBindings>,
    fields: Vec<FieldBindings>,
}

struct FieldBindings {
    field: SymbolId,
    root: TreeId<Typed>,
    index: SourceTypedIndex,
}

struct MethodBindings {
    index: SourceTypedIndex,
    locals: Vec<TypedLocalBinding>,
}

fn named_classes(units: &[NamedUnit]) -> impl Iterator<Item = (&NamedUnit, &NamedClass)> {
    units
        .iter()
        .flat_map(|unit| unit.classes.iter().map(move |class| (unit, class)))
}

fn type_bodies(
    units: &[NamedUnit],
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
    headers: TypedHeaders,
) -> (AstArena<Typed>, Vec<ClassBindings>) {
    let TypedHeaders {
        arena: mut output,
        mut by_class,
    } = headers;
    let mut typed_classes = Vec::new();
    for (unit, class) in named_classes(units) {
        let mut fields = Vec::with_capacity(class.fields.len());
        for &field in &class.fields {
            let mut typer = SourceTyper::new_with_typed_ast(
                &unit.ast,
                unit.source,
                &unit.index,
                store,
                definitions,
                packages,
                output,
            );
            let root = typer.type_field_initializer(field).unwrap();
            let (arena, index) = typer.into_typed_ast_and_index();
            output = arena;
            fields.push(FieldBindings { field, root, index });
        }
        let mut methods = Vec::with_capacity(class.method_range.len());
        for &method in &unit.methods[class.method_range.clone()] {
            let mut typer = SourceTyper::new_with_typed_ast(
                &unit.ast,
                unit.source,
                &unit.index,
                store,
                definitions,
                packages,
                output,
            );
            typer.type_method_definition(method).unwrap();
            let result = typer.into_typed_output();
            output = result.typed_ast;
            methods.push(MethodBindings {
                index: result.source_typed_index,
                locals: result.local_bindings,
            });
        }

        typed_classes.push(ClassBindings {
            methods,
            fields,
            header: by_class.remove(&class.symbol),
        });
    }

    (output, typed_classes)
}

fn class_plans(
    units: &[NamedUnit],
    typed_classes: Vec<ClassBindings>,
    output: &AstArena<Typed>,
    store: &SemanticStore,
) -> Result<Vec<JvmClassPlan>, ClassPlanError> {
    let mut classes = Vec::new();
    for ((unit, class), typed) in named_classes(units).zip(typed_classes) {
        assert_eq!(class.method_range.len(), typed.methods.len());
        let methods = unit.methods[class.method_range.clone()]
            .iter()
            .zip(typed.methods)
            .map(|(&tree, typed)| {
                JvmMethodPlanner {
                    store,
                    source_ast: &unit.ast,
                    source_index: &unit.index,
                    typed_ast: output,
                    typed_index: &typed.index,
                    local_bindings: &typed.locals,
                }
                .plan(unit.symbol(tree))
                .unwrap()
            });
        let fields = typed.fields.iter().map(|binding| TypedFieldInitializer {
            field: binding.field,
            root: binding.root,
            index: &binding.index,
        });
        let mut planner = JvmClassPlanner::new(store, &unit.ast, &unit.index);
        if let Some(index) = &typed.header {
            planner = planner.with_typed_header(output, index);
        }
        classes.push(planner.plan_with_fields(class.symbol, methods, output, fields)?);
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
    let [first, second] = unit.methods[..] else {
        panic!("expected two methods")
    };
    let mut typer = unit.typer(&mut store, definitions, &packages);
    let first_root = typer.type_method_definition(first).unwrap();
    typer.type_method_definition(second).unwrap();
    let mut output = typer.into_typed_output();
    let planner = JvmMethodPlanner::new(&store, &unit.ast, &unit.index, &output);
    let first_plan = planner.plan(unit.symbol(first)).unwrap();
    let second_plan = planner.plan(unit.symbol(second)).unwrap();
    assert_eq!(first_plan.locals.len(), 1);
    assert_eq!(second_plan.locals.len(), 1);
    let first_local = first_plan.locals[0];
    let second_local = second_plan.locals[0];
    assert_eq!(
        store.symbols.owner(first_local.symbol),
        Some(unit.symbol(first))
    );
    assert_eq!(
        store.symbols.owner(second_local.symbol),
        Some(unit.symbol(second))
    );
    assert!(first_local.mutable);
    assert!(!second_local.mutable);
    assert_ne!(first_plan.body, second_plan.body);

    let TreeKind::DefDef(definition) = &mut output.typed_ast.get_mut(first_root).kind else {
        panic!("expected typed method")
    };
    definition.rhs = second_plan.body;
    assert_eq!(
        JvmMethodPlanner::new(&store, &unit.ast, &unit.index, &output).plan(unit.symbol(first)),
        Err(MethodPlanError::BodyMismatch {
            method: unit.symbol(first),
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
    let TreeKind::DefDef(definition) = &caller.ast.get(caller.methods[0]).kind else {
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
