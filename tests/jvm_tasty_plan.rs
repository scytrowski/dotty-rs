use dotty::jvm_tasty_plan::{
    PairedPlanError, with_paired_jvm_tasty_batch_plan, with_paired_jvm_tasty_class_plan,
};
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
    let source = include_str!("fixtures/jvm_tasty_multiclass/CounterGauge.scala");
    assert!(matches!(
        with_paired_jvm_tasty_class_plan(
            source,
            "CounterGauge.scala",
            SourceId::from_index(203),
            |_| ()
        ),
        Err(PairedPlanError::UnsupportedSourceShape(
            "exactly one top-level class; multi-class source units are not supported"
        ))
    ));
}

#[test]
fn counter_and_gauge_have_one_ordered_shared_batch_session() {
    let source = include_str!("fixtures/jvm_tasty_multiclass/CounterGauge.scala");
    with_paired_jvm_tasty_batch_plan(
        source,
        "tests/fixtures/jvm_tasty_multiclass/CounterGauge.scala",
        SourceId::from_index(205),
        |batch| {
            assert_eq!(batch.source_id, SourceId::from_index(205));
            assert_eq!(batch.jvm_unit.classes.len(), 2);
            assert_eq!(batch.classes.len(), 2);
            assert!(std::ptr::eq(
                batch.jvm_unit.store,
                batch.classes[0].tasty_plan.plan().store
            ));
            assert!(std::ptr::eq(
                batch.jvm_unit.typed_ast,
                batch.classes[1].tasty_plan.plan().typed_ast
            ));
            assert!(std::ptr::eq(
                batch.classes[0].tasty_plan.plan().source_index,
                batch.classes[1].tasty_plan.plan().source_index
            ));
            assert!(std::ptr::eq(
                batch.classes[0].tasty_plan.plan().source_ast,
                batch.classes[1].tasty_plan.plan().source_ast
            ));

            let first = &batch.classes[0];
            let second = &batch.classes[1];
            assert_eq!(first.classfile_path, "example/Counter.class");
            assert_eq!(first.tasty_path, "example/Counter.tasty");
            assert_eq!(second.classfile_path, "example/Gauge.class");
            assert_eq!(second.tasty_path, "example/Gauge.tasty");
            assert_eq!(first.class, batch.jvm_unit.classes[0].class);
            assert_eq!(second.class, batch.jvm_unit.classes[1].class);
            assert_ne!(first.class, second.class);
            assert_ne!(first.constructor, second.constructor);
            assert_eq!(first.compatibility.class(), first.class);
            assert_eq!(second.compatibility.class(), second.class);
            assert_eq!(first.tasty_plan.plan().class, first.class);
            assert_eq!(second.tasty_plan.plan().class, second.class);
            assert_eq!(first.compatibility.internal_name(), "example/Counter");
            assert_eq!(second.compatibility.internal_name(), "example/Gauge");
            let first_position = first
                .tasty_plan
                .plan()
                .source_ast
                .get(first.tasty_plan.plan().class_tree)
                .position
                .expect("Counter class source position");
            let second_position = second
                .tasty_plan
                .plan()
                .source_ast
                .get(second.tasty_plan.plan().class_tree)
                .position
                .expect("Gauge class source position");
            assert_eq!(first_position.source(), batch.source_id);
            assert_eq!(second_position.source(), batch.source_id);
            assert!(first_position.span().range().start() < second_position.span().range().start());

            let mut all_fields = Vec::new();
            let mut all_methods = Vec::new();
            let mut all_method_formals = Vec::new();
            for receipt in batch.classes {
                let tasty = receipt.tasty_plan.plan();
                let jvm_class = batch
                    .jvm_unit
                    .classes
                    .iter()
                    .find(|class| class.class == receipt.class)
                    .expect("every receipt has a class in the complete JVM unit");
                let JvmClassInitializationPlan::PrimaryConstructor(constructor) =
                    &jvm_class.initialization
                else {
                    panic!("source classes use their primary constructors");
                };
                assert_eq!(constructor.symbol(), receipt.constructor);
                let constructor_parameter = constructor.parameters()[0];
                assert_eq!(
                    batch
                        .jvm_unit
                        .store
                        .symbols
                        .get(constructor_parameter)
                        .owner,
                    Some(receipt.constructor)
                );
                assert_eq!(
                    tasty.constructor.parameter_clauses[0][0],
                    constructor_parameter
                );

                let source_order = tasty
                    .members
                    .iter()
                    .map(|member| match member {
                        ClassMemberPicklePlan::Field(field) => field.symbol,
                        ClassMemberPicklePlan::Method { symbol, .. } => *symbol,
                    })
                    .collect::<Vec<_>>();
                assert_eq!(receipt.source_member_order, source_order);
                assert_eq!(jvm_class.fields.len(), 1);
                assert_eq!(jvm_class.fields[0].field, source_order[0]);
                assert_eq!(
                    batch
                        .jvm_unit
                        .store
                        .symbols
                        .get(jvm_class.fields[0].field)
                        .owner,
                    Some(receipt.class)
                );
                let ClassMemberPicklePlan::Field(field) = tasty.members[0] else {
                    panic!("the field remains first in source order");
                };
                let TreeKind::ValDef(field_definition) =
                    &tasty.source_ast.get(field.source_tree).kind
                else {
                    panic!("the field plan retains its source declaration");
                };
                let initializer = field_definition.rhs.expect("field initializer");
                assert_eq!(
                    tasty.typed_bindings.typed_at(tasty.source.id, initializer),
                    Some(field.typed_initializer)
                );
                assert_eq!(
                    jvm_class.fields[0].initializer,
                    Some(field.typed_initializer)
                );
                all_fields.push(jvm_class.fields[0].field);

                let method_symbols = jvm_class
                    .methods
                    .iter()
                    .map(|method| method.method)
                    .collect::<Vec<_>>();
                assert_eq!(method_symbols, source_order[1..]);
                for method in &jvm_class.methods {
                    assert_eq!(
                        batch.jvm_unit.store.symbols.get(method.method).owner,
                        Some(receipt.class)
                    );
                    for parameter in &method.parameters {
                        assert_eq!(
                            batch.jvm_unit.store.symbols.get(*parameter).owner,
                            Some(method.method)
                        );
                        all_method_formals.push(*parameter);
                    }
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
                        .expect("source method is included in its class plan");
                    let TreeKind::DefDef(definition) = &tasty.source_ast.get(source_tree).kind
                    else {
                        panic!("method source tree is a DefDef");
                    };
                    let rhs = definition.rhs.expect("concrete method RHS");
                    assert_eq!(
                        tasty.typed_bindings.typed_at(tasty.source.id, rhs),
                        method.body
                    );
                    all_methods.push(method.method);
                }
            }
            assert_eq!(all_fields.len(), 2);
            assert_ne!(all_fields[0], all_fields[1]);
            assert_eq!(all_methods.len(), 4);
            assert_eq!(all_method_formals.len(), 2);
            assert_ne!(all_method_formals[0], all_method_formals[1]);
            for (left_index, left) in all_methods.iter().enumerate() {
                for right in &all_methods[left_index + 1..] {
                    assert_ne!(left, right);
                }
            }
            let method_names = all_methods
                .iter()
                .map(|method| {
                    batch
                        .jvm_unit
                        .store
                        .names
                        .resolve(batch.jvm_unit.store.symbols.get(*method).name.text())
                        .to_owned()
                })
                .collect::<Vec<_>>();
            assert_eq!(method_names, ["get", "set", "get", "set"]);
        },
    )
    .unwrap();
}

#[test]
fn a_bad_second_class_rejects_the_entire_batch_with_class_context() {
    let source = "package example\nclass Counter(start: Int):\n  private var current: Int = start\n  def get(): Int = current\nclass Gauge(start: Int):\n  private val reading: Int = start\n  def get(): Int = reading\n";
    assert!(matches!(
        with_paired_jvm_tasty_batch_plan(
            source,
            "CounterGauge.scala",
            SourceId::from_index(206),
            |_| panic!("a partial Counter receipt must never be returned"),
        ),
        Err(PairedPlanError::BatchClass {
            class_index: 1,
            phase: "member planning",
            ..
        })
    ));
}

#[test]
fn jvm_failure_in_second_class_is_attributed_to_that_source_class() {
    let parameters = (0..256)
        .map(|index| format!("p{index}: Int"))
        .collect::<Vec<_>>()
        .join(", ");
    let source = format!(
        "package example\nclass Counter(start: Int):\n  private var current: Int = start\n  def get(): Int = current\nclass Gauge(start: Int):\n  private var reading: Int = start\n  def get({parameters}): Int = reading\n"
    );
    let error = with_paired_jvm_tasty_batch_plan(
        &source,
        "CounterGauge.scala",
        SourceId::from_index(210),
        |_| panic!("an invalid Gauge method must reject the whole batch"),
    )
    .expect_err("256 instance parameters exceed the JVM slot limit");
    let PairedPlanError::BatchClass {
        class_index,
        declaration_index,
        phase,
        reason,
    } = error
    else {
        panic!("expected a class-specific JVM validation error, got {error:?}");
    };
    assert_eq!(class_index, 1);
    assert!(declaration_index > 0);
    assert_eq!(phase, "JVM class-plan validation");
    assert!(reason.contains("TooManyParameterSlots"));
}

#[test]
fn independent_batch_sessions_have_distinct_arenas_and_same_ordered_intent() {
    let source = include_str!("fixtures/jvm_tasty_multiclass/CounterGauge.scala");
    with_paired_jvm_tasty_batch_plan(
        source,
        "CounterGauge.scala",
        SourceId::from_index(207),
        |first| {
            with_paired_jvm_tasty_batch_plan(
                source,
                "CounterGauge.scala",
                SourceId::from_index(208),
                |second| {
                    assert!(!std::ptr::eq(first.jvm_unit.store, second.jvm_unit.store));
                    assert!(!std::ptr::eq(
                        first.jvm_unit.typed_ast,
                        second.jvm_unit.typed_ast
                    ));
                    let summary = |batch: &dotty::jvm_tasty_plan::PairedJvmTastyBatchPlan<'_>| {
                        batch
                            .classes
                            .iter()
                            .map(|class| {
                                (
                                    class.compatibility.internal_name().to_owned(),
                                    class.classfile_path.clone(),
                                    class.tasty_path.clone(),
                                    class.source_member_order.len(),
                                )
                            })
                            .collect::<Vec<_>>()
                    };
                    assert_eq!(summary(&first), summary(&second));
                    assert_eq!(summary(&first)[0].0, "example/Counter");
                    assert_eq!(summary(&first)[1].0, "example/Gauge");
                    assert_ne!(first.source_id, second.source_id);
                },
            )
            .unwrap();
        },
    )
    .unwrap();
}

#[test]
fn batch_planner_requires_at_least_two_classes() {
    assert!(matches!(
        with_paired_jvm_tasty_batch_plan(
            COUNTER,
            "Counter.scala",
            SourceId::from_index(209),
            |_| ()
        ),
        Err(PairedPlanError::BatchShape(
            "at least two ordinary top-level classes"
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
