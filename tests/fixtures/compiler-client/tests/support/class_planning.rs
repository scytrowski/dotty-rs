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
fn ordinary_constructor_parameters_initialize_instance_state_in_source_order_on_jdk21() {
    use dotty::classfile::{ACC_PUBLIC, ClassFile, MethodAccessFlags, Reader};
    let artifacts = super::compile([
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
    ]);
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
fn class_planner_rejects_nonordinary_parameters_and_multiple_lists() {
    for source in [
        "class C(val x: Int)",
        "class C(var x: Int)",
        "class C(x: Int)(y: Int)",
        "class C(x: => Int)",
        "class C(xs: Int*)",
        "class C(using x: Int)",
    ] {
        let (store, _, _, unit) = named(source);
        let class = unit.classes[0].symbol;
        assert_eq!(
            JvmClassPlanner::new(&store, &unit.ast, &unit.index).plan(class, []),
            Err(ClassPlanError::ConstructorParameters(class)),
            "{source}"
        );
    }
}

#[test]
fn method_only_class_plan_preserves_constructor_formal_identity_and_order() {
    let (store, _, _, unit) = named("class Pair(z: Int, a: Int)");
    let class = &unit.classes[0];
    let plan = JvmClassPlanner::new(&store, &unit.ast, &unit.index)
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
    assert_eq!(names, ["z", "a"]);
}

#[test]
fn class_planner_identifies_the_parameter_with_a_default() {
    let (store, _, _, unit) = named("class C(x: Int, y: Int = 3)");
    let class = &unit.classes[0];
    let Some(dotty::core::SourceDefinition::Canonical { tree, .. }) =
        unit.index.definition_of(class.constructor)
    else {
        panic!("expected a canonical constructor");
    };
    let TreeKind::DefDef(constructor) = &unit.ast.get(tree).kind else {
        panic!("expected a constructor")
    };
    let parameter = constructor.value_param_clauses[0][1];
    assert_eq!(
        JvmClassPlanner::new(&store, &unit.ast, &unit.index).plan(class.symbol, []),
        Err(ClassPlanError::DefaultConstructorParameter {
            class: class.symbol,
            tree: parameter
        })
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

#[test]
fn private_field_initializers_follow_declarations_and_reject_same_type_substitution() {
    use dotty::jvm_class_plan::TypedFieldInitializer;

    let (mut store, defs, packages, unit) = named(
        "class Pair { private val base: Int = 40; private val answer: Int = base + 2 }\nclass Other { private val base: Int = 1 }",
    );
    let class = unit.classes[0].symbol;
    let mut typer = unit.typer(&mut store, defs, &packages);
    let mut roots = Vec::new();
    for &(field, rhs) in unit.classes.iter().flat_map(|class| &class.fields) {
        let ty = typer.complete_symbol(field).unwrap();
        let ctx = typer.field_initializer_context_for(field).unwrap();
        let root = typer.type_expression_expected(rhs, ctx, ty).unwrap();
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
    let planner = JvmClassPlanner::new(&store, &unit.ast, &unit.index);
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
    complete_signatures(&units, &mut store, defs, &packages).unwrap();
    let (arena, mut bindings) = type_bodies(&units, &mut store, defs, &packages);
    bindings[0].fields.reverse();
    let classes = class_plans(&units, bindings, &arena, &store).unwrap();
    let class = classes[0].class;
    let unit = dotty::jvm::JvmCompilationUnit {
        store: &store,
        typed_ast: &arena,
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
    let (mut store, defs, packages, unit) = named(include_str!("../../Counter.scala"));
    let units = [unit];
    complete_signatures(&units, &mut store, defs, &packages).unwrap();
    let (arena, mut bindings) = type_bodies(&units, &mut store, defs, &packages);
    let [base, current] = bindings[0].fields.as_mut_slice() else {
        panic!("expected two fields")
    };
    let error = ClassPlanError::FieldInitializerMismatch {
        field: current.field,
        expected: current.root,
        actual: base.root,
    };
    current.root = base.root;
    assert_eq!(class_plans(&units, bindings, &arena, &store), Err(error));
}

#[test]
fn field_planning_rejects_a_mutability_mismatch_between_source_and_symbol() {
    use dotty::core::SymbolFlags;

    for (keyword, flags) in [("val", SymbolFlags::MUTABLE), ("var", SymbolFlags::EMPTY)] {
        let (mut store, _, _, unit) = named(&format!("class C {{ private {keyword} n: Int = 1 }}"));
        let class = &unit.classes[0];
        let field = class.fields[0].0;
        store.symbols.get_mut(field).flags = flags;
        let Some(dotty::core::SourceDefinition::Canonical { tree, .. }) =
            unit.index.definition_of(field)
        else {
            panic!("expected field declaration")
        };
        assert_eq!(
            JvmClassPlanner::new(&store, &unit.ast, &unit.index).plan_with_fields(
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
fn field_planning_rejects_public_qualified_and_lazy_fields() {
    for declaration in [
        "var n: Int = 1",
        "val n: Int = 1",
        "private[this] val n: Int = 1",
        "private lazy val n: Int = 1",
    ] {
        let (store, _, _, unit) = named(&format!("class C {{ {declaration} }}"));
        let class = &unit.classes[0];
        let field = class.fields[0].0;
        let Some(dotty::core::SourceDefinition::Canonical { tree, .. }) =
            unit.index.definition_of(field)
        else {
            panic!("expected field declaration")
        };
        let expected = Err(ClassPlanError::UnsupportedStatement {
            class: class.symbol,
            tree,
        });
        let planner = JvmClassPlanner::new(&store, &unit.ast, &unit.index);
        assert_eq!(
            planner.plan_with_fields(class.symbol, [], &dotty::core::AstArena::new(), []),
            expected,
            "{declaration}"
        );
    }
}
