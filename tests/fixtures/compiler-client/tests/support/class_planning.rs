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
    let artifacts = super::compile([counter, use_counter]);
    let reversed = super::compile([use_counter, counter]);
    assert!(artifacts.iter().eq(reversed.iter().rev()));
    assert_counter_abi(&artifacts, "id", "(II)V");
    assert_eq!(
        super::run_java_client("constructor-accessors", "AccessorClient", &artifacts)
            .lines()
            .collect::<Vec<_>>(),
        ["9", "4", "7", "2"]
    );
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
    let first = super::compile([counter, use_counter]);
    let second = super::compile([use_counter, counter]);
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
    let (mut store, defs, packages, unit) = named(BODY_COUNTER);
    let units = [unit];
    let headers = complete_signatures(&units, &mut store, defs, &packages).unwrap();
    let (arena, mut bindings) = type_bodies(&units, &mut store, defs, &packages, headers);
    let [initial, count] = bindings[0].fields.as_mut_slice() else {
        panic!("expected two fields")
    };
    let error = ClassPlanError::FieldInitializerMismatch {
        field: initial.field,
        expected: initial.root,
        actual: count.root,
    };
    std::mem::swap(&mut initial.root, &mut count.root);
    assert_eq!(class_plans(&units, bindings, &arena, &store), Err(error));
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
    let (store, _, _, unit) = named("class Pair(z: Int, val a: Int, var b: Int)");
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
    let headers = complete_signatures(&units, &mut store, defs, &packages).unwrap();
    let (arena, bindings) = type_bodies(&units, &mut store, defs, &packages, headers);
    assert_eq!(
        class_plans(&units, bindings, &arena, &store),
        Err(ClassPlanError::UnsupportedStatement { class, tree })
    );
}

#[test]
fn class_planner_requires_a_public_constructor() {
    let (store, _, _, unit) = named("class A()\nclass B private ()");
    let planner = JvmClassPlanner::new(&store, &unit.ast, &unit.index);
    let [plain, private] = unit.classes.as_slice() else {
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
    let child = unit.classes[1].symbol;
    let planner = JvmClassPlanner::new(&store, &unit.ast, &unit.index);
    assert_eq!(
        planner.plan(child, []),
        Err(ClassPlanError::UnsupportedTemplate(child))
    );
    assert_eq!(
        planner
            .with_typed_header(&headers.arena, &headers.by_class[&child])
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
    super::with_plan(&files, |unit, definitions| {
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
    let first = super::compile([base, derived]);
    let second = super::compile([derived, base]);
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
        super::complete_headers(
            &units,
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
        let first = units[0].classes[0].symbol;
        let mid = units[1].classes[0].symbol;
        let last = units[2].classes[0].symbol;
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
fn completing_a_child_preserves_its_same_file_parents_typed_application() {
    let (mut store, defs, packages, unit) = named(
        "class Leaf(x: Int) extends Mid(x)\nclass Mid(y: Int) extends Base(y)\nclass Base(z: Int)",
    );
    let headers =
        complete_signatures(std::slice::from_ref(&unit), &mut store, defs, &packages).unwrap();
    for class in &unit.classes[..2] {
        let index = &headers.by_class[&class.symbol];
        JvmClassPlanner::new(&store, &unit.ast, &unit.index)
            .with_typed_header(&headers.arena, index)
            .plan(class.symbol, [])
            .unwrap();
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
        let super::SignatureError::HeaderCycle(path) = error else {
            panic!("expected a cycle, got {error:?}");
        };
        let first = units[cycle_start].classes[0].symbol;
        let second = units[cycle_start + 1].classes[0].symbol;
        assert_eq!(path, [first, second, first]);
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
        let child = &units[child_index].classes[0];
        let parent = units[parent_index].classes[0].symbol;
        let mut blocked = Vec::new();
        let error =
            super::complete_headers(&units, &mut store, defs, &packages, &mut |class, result| {
                if let SignatureCompletion::BlockedOn(dependency) = result {
                    blocked.push((class, dependency));
                }
            })
            .unwrap_err();

        if child_index == 0 {
            assert_eq!(blocked, [(child.symbol, parent)]);
        } else {
            assert!(blocked.is_empty());
        }
        assert!(
            matches!(error, super::SignatureError::Typing(TyperError::ApplicationArgumentTypeMismatch {
            source, argument_index: 0, expected, ..
        }) if source == derived.0 && expected == defs.int),
            "{error:?}"
        );
        assert_eq!(
            store.symbols.info(child.symbol),
            &dotty::core::SymbolInfo::Missing
        );
    }
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
    let [_, first, second] = unit.classes.as_slice() else {
        panic!("expected three classes")
    };
    let typed_call = |class: &super::NamedClass| {
        headers.by_class[&class.symbol]
            .get(unit.source, class.parent_application.unwrap())
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
    let planner = JvmClassPlanner::new(&store, &unit.ast, &unit.index);
    let bindings = &headers.by_class[&first.symbol];
    planner
        .with_typed_header(&headers.arena, bindings)
        .plan(first.symbol, [])
        .unwrap();
    let TreeKind::Apply(app) = &mut headers.arena.get_mut(first_call).kind else {
        unreachable!()
    };
    app.args[0] = foreign;
    assert_eq!(
        JvmClassPlanner::new(&store, &unit.ast, &unit.index)
            .with_typed_header(&headers.arena, bindings)
            .plan(first.symbol, []),
        Err(ClassPlanError::InvalidParentApplication {
            class: first.symbol,
            tree: first.parent_application.unwrap()
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
    let child = &unit.classes[0];
    let constructor = units[0].classes[0].constructor;
    store.symbols.get_mut(constructor).origin = dotty::core::SymbolOrigin::Source(derived);
    assert_eq!(
        JvmClassPlanner::new(&store, &unit.ast, &unit.index)
            .with_typed_header(&headers.arena, &headers.by_class[&child.symbol])
            .plan(child.symbol, []),
        Err(ClassPlanError::InvalidParentApplication {
            class: child.symbol,
            tree: child.parent_application.unwrap()
        })
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
    for &field in unit.classes.iter().flat_map(|class| &class.fields) {
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
    let headers = complete_signatures(&units, &mut store, defs, &packages).unwrap();
    let (arena, mut bindings) = type_bodies(&units, &mut store, defs, &packages, headers);
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
    let headers = complete_signatures(&units, &mut store, defs, &packages).unwrap();
    let (arena, mut bindings) = type_bodies(&units, &mut store, defs, &packages, headers);
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
        let field = class.fields[0];
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
fn field_planning_rejects_qualified_and_modified_fields() {
    for declaration in [
        "protected var n: Int = 1",
        "lazy val n: Int = 1",
        "final val n: Int = 1",
        "private[this] val n: Int = 1",
        "private lazy val n: Int = 1",
    ] {
        let (store, _, _, unit) = named(&format!("class C {{ {declaration} }}"));
        let class = &unit.classes[0];
        let field = class.fields[0];
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
