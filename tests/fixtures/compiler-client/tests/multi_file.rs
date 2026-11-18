//! The consumer owns phase ordering, plans and output. Its two fixture classes
//! each have a public no-arg constructor and a single method with an explicit result.
use std::fs;
use std::path::Path;
use std::process::Command;

use dotty::core::{
    AstArena, Definitions, Packages, SemanticStore, SourceId, SourceSemanticIndex, SourceText,
    SymbolId, TreeId, TreeKind, Untyped,
};
use dotty::jvm::{
    GeneratedClass, JvmClassInitializationPlan, JvmClassPlan, JvmCompilationUnit, JvmMethodPlan,
    emit_class,
};
use dotty::lexer::ContextualScanner;
use dotty::namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty::parser::parse_compilation_unit;
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
    text: &str,
    store: &mut SemanticStore,
    packages: &mut Packages,
) -> NamedUnit {
    let scanner = ContextualScanner::new(text).unwrap();
    assert!(
        scanner.diagnostics().is_empty(),
        "{:?}",
        scanner.diagnostics()
    );
    let parsed = parse_compilation_unit(
        SourceText::new(text).unwrap(),
        id,
        scanner,
        &mut store.names,
    );
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

fn compile(files: [(SourceId, &str, &str); 2]) -> Vec<GeneratedClass> {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let units = files.map(|(id, name, text)| enter(id, name, text, &mut store, &mut packages));

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
    plan.classes
        .iter()
        .map(|c| emit_class(&plan, c.class, &definitions).unwrap())
        .collect()
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
