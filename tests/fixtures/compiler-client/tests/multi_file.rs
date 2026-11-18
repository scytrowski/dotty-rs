//! The consumer owns phase ordering, plans and output. Its two fixture classes
//! each have a public no-arg constructor and a single method with an explicit result.
#[path = "support/parsing.rs"]
mod parsing;

use std::fs;
use std::path::Path;
use std::process::Command;

use dotty::core::{
    AstArena, Definitions, Packages, SemanticStore, SourceId, SourceSemanticIndex, SymbolId,
    TreeId, TreeKind, Untyped,
};
use dotty::jvm::{
    GeneratedClass, JvmClassInitializationPlan, JvmClassPlan, JvmCompilationUnit, JvmMethodPlan,
    emit_class,
};
use dotty::namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty::parser::ParseResult;
use dotty::typer::SourceTyper;

struct NamedUnit {
    source: SourceId,
    ast: AstArena<Untyped>,
    index: SourceSemanticIndex,
    class: SymbolId,
    method: TreeId<Untyped>,
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
    let [method] = template.body[..] else {
        panic!("expected one method")
    };
    let class = index.symbol_at(id, class_tree).unwrap();
    NamedUnit {
        source: id,
        ast,
        index: *index,
        class,
        method,
    }
}

fn with_plan<R>(
    files: &[(SourceId, &str, &str)],
    run: impl FnOnce(&JvmCompilationUnit<'_>, &Definitions) -> R,
) -> R {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let units: Vec<_> = files
        .iter()
        .zip(parsing::parse_files(files, 4))
        .map(|(&(id, name, _), detached)| {
            let parsed = detached.into_session(&mut store.names);
            enter(id, name, parsed, &mut store, &mut packages)
        })
        .collect();

    // Both signatures are available before a body can request another file's method.
    for unit in &units {
        let mut typer = SourceTyper::new(
            &unit.ast,
            unit.source,
            &unit.index,
            &mut store,
            definitions,
            &packages,
        );
        typer.complete_symbol(unit.class).unwrap();
        typer
            .complete_symbol(unit.index.symbol_at(unit.source, unit.method).unwrap())
            .unwrap();
    }

    let mut output = AstArena::new();
    let mut typed_units = Vec::new();
    for unit in &units {
        let mut typer = SourceTyper::new_with_typed_ast(
            &unit.ast,
            unit.source,
            &unit.index,
            &mut store,
            definitions,
            &packages,
            output,
        );
        let root = typer.type_method_definition(unit.method).unwrap();
        let (arena, index) = typer.into_typed_ast_and_index();
        output = arena;
        typed_units.push((root, index));
    }

    let mut classes = Vec::new();
    for (unit, (root, index)) in units.iter().zip(typed_units) {
        let TreeKind::DefDef(method) = &unit.ast.get(unit.method).kind else {
            panic!("expected method")
        };
        let parameters = method
            .value_param_clauses
            .iter()
            .flatten()
            .map(|&tree| unit.index.symbol_at(unit.source, tree).unwrap())
            .collect();
        let TreeKind::DefDef(typed) = &output.get(root).kind else {
            panic!("expected typed method")
        };
        let body = typed.rhs.unwrap();
        assert_eq!(index.get(unit.source, method.rhs.unwrap()), Some(body));
        assert_eq!(output.get(body).position.unwrap().source(), unit.source);
        classes.push(JvmClassPlan {
            class: unit.class,
            fields: vec![],
            initialization: JvmClassInitializationPlan::ImplicitNoArg,
            methods: vec![JvmMethodPlan {
                method: unit.index.symbol_at(unit.source, unit.method).unwrap(),
                parameters,
                body: Some(body),
                locals: vec![],
            }],
        });
    }
    let plan = JvmCompilationUnit {
        store: &store,
        typed_ast: &output,
        classes,
    };
    run(&plan, &definitions)
}

fn compile(files: [(SourceId, &str, &str); 2]) -> Vec<GeneratedClass> {
    with_plan(&files, |plan, definitions| {
        let prepared = plan.prepare_emission(definitions).unwrap();
        let concurrent: Vec<_> = std::thread::scope(|scope| {
            let workers: Vec<_> = plan
                .classes
                .iter()
                .map(|c| scope.spawn(|| prepared.emit_class(c.class).unwrap()))
                .collect();
            workers.into_iter().map(|w| w.join().unwrap()).collect()
        });
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
