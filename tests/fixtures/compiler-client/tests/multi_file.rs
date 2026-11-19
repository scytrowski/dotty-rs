//! The consumer owns phase ordering, plans and output for classes with a no-arg constructor.
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
    AstArena, Definitions, Packages, SemanticStore, SourceId, SourceSemanticIndex, SymbolFlags,
    SymbolId, SymbolInfo, TreeId, TreeKind, Typed, Untyped,
};
use dotty::jvm::{
    GeneratedClass, JvmClassInitializationPlan, JvmClassPlan, JvmCompilationUnit, JvmLocalBinding,
    JvmMethodPlan, PreparedJvmEmission, emit_class,
};
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
    symbol: SymbolId,
    method_range: Range<usize>,
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
        methods.extend_from_slice(&template.body);
        classes.push(NamedClass {
            symbol: index.symbol_at(id, tree).unwrap(),
            method_range: start..methods.len(),
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
fn complete_signatures(
    units: &[NamedUnit],
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
) -> Result<(), TyperError> {
    complete_signatures_in_order(units, units, store, definitions, packages, |_, _| {})
}

fn complete_signatures_in_order<'a>(
    units: &'a [NamedUnit],
    roots: impl IntoIterator<Item = &'a NamedUnit>,
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
    observe: impl FnMut(SymbolId, SignatureCompletion),
) -> Result<(), TyperError> {
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
) -> Result<(), TyperError> {
    let mut owners = HashMap::new();
    for unit in units {
        for class in &unit.classes {
            let header = unit
                .typer(store, definitions, packages)
                .complete_symbol(class.symbol)?;
            observe(class.symbol, SignatureCompletion::Complete(header));
        }
        owners.extend(unit.method_symbols().map(|method| (method, unit)));
    }

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
                        });
                    }
                    pending.push(dependency);
                }
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
    complete_signatures(&units, &mut store, definitions, &packages).unwrap();

    let (output, typed_classes) = type_bodies(&units, &mut store, definitions, &packages);
    let classes = class_plans(&units, typed_classes, &output, &store);
    let plan = JvmCompilationUnit {
        store: &store,
        typed_ast: &output,
        classes,
    };
    run(&plan, &definitions)
}

struct TypedMethod {
    root: TreeId<Typed>,
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
) -> (AstArena<Typed>, Vec<Vec<TypedMethod>>) {
    let mut output = AstArena::new();
    let mut typed_classes = Vec::new();
    for (unit, class) in named_classes(units) {
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
            let root = typer.type_method_definition(method).unwrap();
            let result = typer.into_typed_output();
            output = result.typed_ast;
            methods.push(TypedMethod {
                root,
                index: result.source_typed_index,
                locals: result.local_bindings,
            });
        }

        typed_classes.push(methods);
    }

    (output, typed_classes)
}

fn class_plans(
    units: &[NamedUnit],
    typed_classes: Vec<Vec<TypedMethod>>,
    output: &AstArena<Typed>,
    store: &SemanticStore,
) -> Vec<JvmClassPlan> {
    let mut classes = Vec::new();
    for ((unit, class), typed) in named_classes(units).zip(typed_classes) {
        assert_eq!(class.method_range.len(), typed.len());
        let methods = unit.methods[class.method_range.clone()]
            .iter()
            .zip(typed)
            .map(|(&tree, typed)| {
                let TreeKind::DefDef(method) = &unit.ast.get(tree).kind else {
                    panic!("expected method")
                };
                let parameters = method
                    .value_param_clauses
                    .iter()
                    .flatten()
                    .map(|&tree| unit.index.symbol_at(unit.source, tree).unwrap())
                    .collect();
                let TreeKind::DefDef(definition) = &output.get(typed.root).kind else {
                    panic!("expected typed method")
                };
                let body = definition.rhs.unwrap();
                assert_eq!(
                    typed.index.get(unit.source, method.rhs.unwrap()),
                    Some(body)
                );
                assert_eq!(output.get(body).position.unwrap().source(), unit.source);
                JvmMethodPlan {
                    method: unit.symbol(tree),
                    parameters,
                    body: Some(body),
                    locals: typed
                        .locals
                        .into_iter()
                        .map(|binding| {
                            let symbol = store.symbols.get(binding.symbol());
                            assert_eq!(symbol.owner, Some(unit.symbol(tree)));
                            JvmLocalBinding {
                                declaration: binding.declaration(),
                                symbol: binding.symbol(),
                                mutable: symbol.flags.contains(SymbolFlags::MUTABLE),
                            }
                        })
                        .collect(),
                }
            })
            .collect();
        classes.push(JvmClassPlan {
            class: class.symbol,
            fields: vec![],
            initialization: JvmClassInitializationPlan::ImplicitNoArg,
            methods,
        });
    }
    classes
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

fn compile(files: [(SourceId, &str, &str); 2]) -> Vec<GeneratedClass> {
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
        assert_eq!(stdout.lines().collect::<Vec<_>>(), ["17", "42"], "{order}");
    }
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
        matches!(err, TyperError::ApplicationArityMismatch { source, tree_index, expected: 0, actual: 1 }
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
                matches!(error, TyperError::RecursiveInferredMethodResult { symbol } if symbol == units[first].method_symbol()),
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
