//! The consumer owns phase ordering, plans and output for classes with a no-arg constructor.
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
    AstArena, Definitions, Packages, SemanticStore, SourceId, SourceSemanticIndex, SymbolFlags,
    SymbolId, SymbolInfo, TreeId, TreeKind, Typed, Untyped,
};
use dotty::jvm::{
    GeneratedClass, JvmClassInitializationPlan, JvmClassPlan, JvmCompilationUnit, JvmLocalBinding,
    JvmMethodPlan, PreparedJvmEmission, emit_class,
};
use dotty::namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty::parser::{DetachedParse, ParseResult};
use dotty::typer::{SourceTypedIndex, SourceTyper, TypedLocalBinding, TyperError};

struct NamedUnit {
    source: SourceId,
    ast: AstArena<Untyped>,
    index: SourceSemanticIndex,
    class: SymbolId,
    methods: Vec<TreeId<Untyped>>,
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
    let [class_tree] = package.stats[..] else {
        panic!("expected one class")
    };
    let TreeKind::TypeDef(class) = &ast.get(class_tree).kind else {
        panic!("expected class")
    };
    let TreeKind::Template(template) = &ast.get(class.rhs).kind else {
        panic!("expected template")
    };
    let methods = template.body.clone();
    let class = index.symbol_at(id, class_tree).unwrap();
    NamedUnit {
        source: id,
        ast,
        index: *index,
        class,
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
    mut observe: impl FnMut(SymbolId, Option<&TyperError>),
) -> Result<(), TyperError> {
    let mut owners = HashMap::new();
    for unit in units {
        let header = unit
            .typer(store, definitions, packages)
            .complete_symbol(unit.class);
        observe(unit.class, header.as_ref().err());
        header?;
        owners.extend(unit.method_symbols().map(|method| (method, unit)));
    }

    for symbol in roots.into_iter().flat_map(NamedUnit::method_symbols) {
        if matches!(store.symbols.info(symbol), SymbolInfo::Complete(_)) {
            continue;
        }
        let mut pending = vec![symbol];
        let mut active = HashSet::from([symbol]);
        while let Some(&symbol) = pending.last() {
            let unit = owners[&symbol];
            let completion = unit
                .typer(store, definitions, packages)
                .complete_symbol(symbol);
            observe(symbol, completion.as_ref().err());
            match completion {
                Ok(_) => {
                    pending.pop();
                    active.remove(&symbol);
                }
                Err(TyperError::MemberTypeUnavailable { symbol: dependency })
                    if owners.contains_key(&dependency)
                        && matches!(store.symbols.info(dependency), SymbolInfo::Missing) =>
                {
                    if !active.insert(dependency) {
                        return Err(TyperError::RecursiveInferredMethodResult {
                            symbol: dependency,
                        });
                    }
                    pending.push(dependency);
                }
                Err(error) => return Err(error),
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

    let (output, typed_units) = type_bodies(&units, &mut store, definitions, &packages);
    let classes = class_plans(&units, typed_units, &output, &store);
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

fn type_bodies(
    units: &[NamedUnit],
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
) -> (AstArena<Typed>, Vec<Vec<TypedMethod>>) {
    let mut output = AstArena::new();
    let mut typed_units = Vec::new();
    for unit in units {
        let mut methods = Vec::with_capacity(unit.methods.len());
        for &method in &unit.methods {
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

        typed_units.push(methods);
    }

    (output, typed_units)
}

fn class_plans(
    units: &[NamedUnit],
    typed_units: Vec<Vec<TypedMethod>>,
    output: &AstArena<Typed>,
    store: &SemanticStore,
) -> Vec<JvmClassPlan> {
    let mut classes = Vec::new();
    for (unit, typed) in units.iter().zip(typed_units) {
        assert_eq!(unit.methods.len(), typed.len());
        let methods = unit
            .methods
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
            class: unit.class,
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

    let output = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("compiler-client-{}", std::process::id()));
    fs::create_dir(&output).unwrap();
    fs::create_dir(output.join("demo")).unwrap();
    for class in first {
        fs::write(
            output.join(format!("{}.class", class.internal_name())),
            class.bytes(),
        )
        .unwrap();
    }
    let java_source = Path::new(env!("CARGO_MANIFEST_DIR")).join("Client.java");
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
        .args(["-Xverify:all", "-cp", ".", "Client"])
        .output()
        .unwrap();
    assert!(
        java.status.success(),
        "{}",
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(String::from_utf8(java.stdout).unwrap().trim(), "7");
    fs::remove_dir_all(output).unwrap();
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
