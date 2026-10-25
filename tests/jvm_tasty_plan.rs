use dotty::jvm_tasty_plan::{PairedPlanError, with_paired_jvm_tasty_class_plan};
use dotty_core::ast::TreeKind;
use dotty_core::{SourceId, SymbolId};
use dotty_jvm::JvmClassInitializationPlan;
use dotty_tasty_pickler::ClassMemberPicklePlan;

const COUNTER: &str = include_str!("fixtures/jvm_tasty_interop/Counter.scala");

#[test]
fn counter_plans_share_one_source_session_and_exact_typed_roots() {
    with_paired_jvm_tasty_class_plan(
        COUNTER,
        "Counter.scala",
        SourceId::from_index(200),
        |pair| {
            assert_eq!(pair.compatibility.class(), pair.class);
            assert_eq!(pair.compatibility.internal_name(), "example/Counter");
            assert_eq!(pair.classfile_path, "example/Counter.class");
            assert_eq!(pair.tasty_path, "example/Counter.tasty");
            assert_eq!(pair.compatibility.plans().0.classes.len(), 1);
            assert_eq!(pair.compatibility.plans().1.plan().class, pair.class);
            assert!(std::ptr::eq(
                pair.jvm_unit.store,
                pair.tasty_plan.plan().store
            ));
            assert!(std::ptr::eq(
                pair.jvm_unit.typed_ast,
                pair.tasty_plan.plan().typed_ast
            ));

            let jvm_class = &pair.jvm_unit.classes[0];
            assert_eq!(jvm_class.class, pair.class);
            let JvmClassInitializationPlan::PrimaryConstructor(constructor) =
                &jvm_class.initialization
            else {
                panic!("expected source primary constructor");
            };
            assert_eq!(constructor.symbol(), pair.constructor);
            assert_eq!(constructor.parameters().len(), 1);
            assert_eq!(
                pair.jvm_unit
                    .store
                    .symbols
                    .get(constructor.parameters()[0])
                    .owner,
                Some(pair.constructor)
            );

            let tasty = pair.tasty_plan.plan();
            assert_eq!(
                tasty
                    .constructor
                    .parameter_clauses
                    .iter()
                    .flatten()
                    .copied()
                    .collect::<Vec<_>>(),
                constructor.parameters()
            );
            assert_eq!(
                tasty
                    .source_index
                    .symbol_at(pair.source_id, tasty.class_tree),
                Some(pair.class)
            );
            let source_member_order = tasty
                .members
                .iter()
                .map(|member| match member {
                    ClassMemberPicklePlan::Field(field) => field.symbol,
                    ClassMemberPicklePlan::Method { symbol, .. } => *symbol,
                })
                .collect::<Vec<SymbolId>>();
            assert_eq!(pair.source_member_order, source_member_order);
            assert_eq!(jvm_class.fields.len(), 1);
            assert_eq!(jvm_class.fields[0].field, source_member_order[0]);
            assert_eq!(
                pair.jvm_unit
                    .store
                    .symbols
                    .get(jvm_class.fields[0].field)
                    .owner,
                Some(pair.class)
            );
            assert_eq!(
                jvm_class.fields[0].initializer,
                Some(match tasty.members[0] {
                    ClassMemberPicklePlan::Field(field) => field.typed_initializer,
                    ClassMemberPicklePlan::Method { .. } =>
                        panic!("field must retain source order"),
                })
            );
            let ClassMemberPicklePlan::Field(field) = tasty.members[0] else {
                panic!("field must retain source order");
            };
            let TreeKind::ValDef(field_definition) = &tasty.source_ast.get(field.source_tree).kind
            else {
                panic!("field source tree must be a ValDef");
            };
            let source_initializer = field_definition.rhs.expect("field source initializer");
            assert_eq!(
                tasty
                    .typed_bindings
                    .typed_at(pair.source_id, source_initializer),
                Some(field.typed_initializer)
            );

            let method_symbols = jvm_class
                .methods
                .iter()
                .map(|method| method.method)
                .collect::<Vec<_>>();
            assert_eq!(method_symbols, source_member_order[1..]);
            for method in &jvm_class.methods {
                assert_eq!(
                    pair.jvm_unit.store.symbols.get(method.method).owner,
                    Some(pair.class)
                );
                let source_tree = tasty
                    .members
                    .iter()
                    .find_map(|member| match member {
                        ClassMemberPicklePlan::Method {
                            symbol,
                            source_tree,
                        } if *symbol == method.method => Some(*source_tree),
                        _ => None,
                    })
                    .expect("matching TASTy source method");
                let TreeKind::DefDef(definition) = &tasty.source_ast.get(source_tree).kind else {
                    panic!("method source tree must be a DefDef");
                };
                let source_rhs = definition.rhs.expect("concrete method body");
                let typed_rhs = tasty
                    .typed_bindings
                    .typed_at(tasty.source.id, source_rhs)
                    .expect("one-session typed body binding");
                assert_eq!(method.body, Some(typed_rhs));
            }
        },
    )
    .unwrap();
}

#[test]
fn separate_source_sessions_keep_deterministic_artifact_intent() {
    let first = with_paired_jvm_tasty_class_plan(
        COUNTER,
        "Counter.scala",
        SourceId::from_index(201),
        |pair| {
            (
                pair.classfile_path.clone(),
                pair.tasty_path.clone(),
                pair.source_member_order.len(),
                pair.source_id,
            )
        },
    )
    .unwrap();
    let second = with_paired_jvm_tasty_class_plan(
        COUNTER,
        "Counter.scala",
        SourceId::from_index(202),
        |pair| {
            (
                pair.classfile_path.clone(),
                pair.tasty_path.clone(),
                pair.source_member_order.len(),
                pair.source_id,
            )
        },
    )
    .unwrap();

    assert_eq!(first.0, second.0);
    assert_eq!(first.1, second.1);
    assert_eq!(first.2, second.2);
    assert_ne!(first.3, second.3);
    assert_eq!(first.0, "example/Counter.class");
    assert_eq!(first.1, "example/Counter.tasty");
    assert_eq!(first.2, 3);
}

#[test]
fn multi_class_source_is_rejected_without_a_partial_pair() {
    let source = "package example\nclass Counter(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\nclass Other:\n  def answer(): Int = 42\n";
    assert!(matches!(
        with_paired_jvm_tasty_class_plan(
            source,
            "Counter.scala",
            SourceId::from_index(203),
            |_| ()
        ),
        Err(PairedPlanError::UnsupportedSourceShape(
            "exactly one top-level class; multi-class source units are not supported"
        ))
    ));
}

#[test]
fn unsupported_immutable_field_is_rejected_from_source_facts() {
    let immutable = "package example\nclass Counter(start: Int):\n  private val current: Int = start\n  def get(): Int = current\n";
    assert!(matches!(
        with_paired_jvm_tasty_class_plan(
            immutable,
            "Counter.scala",
            SourceId::from_index(204),
            |_| ()
        ),
        Err(PairedPlanError::UnsupportedSourceShape(
            "unqualified private mutable class-body fields"
        ))
    ));
}
