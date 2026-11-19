//! Class plans built through the single-dependency client's public API.
use super::{NamedUnit, class_plans, complete_signatures, name_units, parsing, type_bodies};
use dotty::core::{Definitions, Packages, SemanticStore, SourceId, TreeKind};
use dotty::jvm_class_plan::{ClassPlanError, JvmClassPlanner};
use dotty::jvm_method_plan::JvmMethodPlanner;

fn named(text: &str) -> (SemanticStore, Definitions, Packages, NamedUnit) {
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

#[test]
fn class_plans_require_exact_methods_and_restore_source_order() {
    let (mut store, defs, packages, unit) = named(
        "class A { def value(): Int = 17; def other(): Int = 5 }\nclass B { def value(): Int = 42 }",
    );
    let class = unit.classes[0].symbol;
    let mut typer = unit.typer(&mut store, defs, &packages);
    for &tree in &unit.methods {
        typer.type_method_definition(tree).unwrap();
    }
    let output = typer.into_typed_output();
    let method_planner = JvmMethodPlanner::new(&store, &unit.ast, &unit.index, &output);
    let plans: Vec<_> = unit
        .method_symbols()
        .map(|method| method_planner.plan(method).unwrap())
        .collect();
    let [first, second, foreign] = &plans[..] else {
        panic!("expected three methods")
    };
    let planner = JvmClassPlanner::new(&store, &unit.ast, &unit.index);

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
fn class_planner_rejects_parameterized_constructor() {
    let (store, _, _, unit) = named("class C(x: Int)");
    let class = unit.classes[0].symbol;
    assert_eq!(
        JvmClassPlanner::new(&store, &unit.ast, &unit.index).plan(class, []),
        Err(ClassPlanError::ConstructorParameters(class))
    );
}

#[test]
fn class_planner_rejects_template_initializer() {
    let (mut store, defs, packages, unit) = named("class C { def run(): Int = 1; run() }");
    let class = unit.classes[0].symbol;
    let tree = unit
        .ast
        .iter()
        .find_map(|(id, node)| matches!(node.kind, TreeKind::Apply(_)).then_some(id))
        .unwrap();
    let units = [unit];
    complete_signatures(&units, &mut store, defs, &packages).unwrap();
    let (arena, bindings) = type_bodies(&units, &mut store, defs, &packages);
    assert_eq!(
        class_plans(&units, bindings, &arena, &store),
        Err(ClassPlanError::UnsupportedStatement { class, tree })
    );
}

#[test]
fn class_planner_requires_a_public_constructor_and_an_implicit_parent() {
    let (store, _, _, unit) = named("class A()\nclass B private ()\nclass C extends A");
    let planner = JvmClassPlanner::new(&store, &unit.ast, &unit.index);
    let [plain, private, child] = unit.classes.as_slice() else {
        panic!("expected three classes")
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
    assert_eq!(
        planner.plan(child.symbol, []),
        Err(ClassPlanError::UnsupportedTemplate(child.symbol))
    );
}
