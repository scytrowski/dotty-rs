use dotty::jvm_tasty_plan::{
    PairedPlanError, with_paired_jvm_tasty_batch_plan, with_paired_jvm_tasty_class_plan,
};
use dotty_core::ast::TreeKind;
use dotty_core::{SourceId, SymbolId};
use dotty_jvm::JvmClassInitializationPlan;
use dotty_tasty::tasty::*;
use dotty_tasty_pickler::{
    ClassMemberPicklePlan, ClassStatePicklePlan, PickleError, SourceTypedBindings,
};

const COUNTER: &str = include_str!("fixtures/jvm_tasty_interop/Counter.scala");
const CROSS_CLASS_READER_LITERAL: &str =
    include_str!("fixtures/jvm_tasty_cross_class/ReaderLiteral.scala");
const CROSS_CLASS_READER_CALL: &str = include_str!("fixtures/jvm_tasty_cross_class/Reader.scala");
const RUN_SCALA_ORACLE: &str = "DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE";

struct WithoutPlannedClasses<'a>(&'a dyn SourceTypedBindings);

impl SourceTypedBindings for WithoutPlannedClasses<'_> {
    fn typed_at(
        &self,
        source: dotty_core::SourceId,
        tree: dotty_core::TreeId<dotty_core::Untyped>,
    ) -> Option<dotty_core::TreeId<dotty_core::Typed>> {
        self.0.typed_at(source, tree)
    }

    fn local_value_bindings(
        &self,
        source: dotty_core::SourceId,
        method: SymbolId,
    ) -> Vec<dotty_tasty_pickler::LocalValueBinding> {
        self.0.local_value_bindings(source, method)
    }
}

struct WithoutPlannedMethods<'a>(&'a dyn SourceTypedBindings);

impl SourceTypedBindings for WithoutPlannedMethods<'_> {
    fn typed_at(
        &self,
        source: dotty_core::SourceId,
        tree: dotty_core::TreeId<dotty_core::Untyped>,
    ) -> Option<dotty_core::TreeId<dotty_core::Typed>> {
        self.0.typed_at(source, tree)
    }

    fn planned_class_symbols(&self) -> &[SymbolId] {
        self.0.planned_class_symbols()
    }

    fn local_value_bindings(
        &self,
        source: dotty_core::SourceId,
        method: SymbolId,
    ) -> Vec<dotty_tasty_pickler::LocalValueBinding> {
        self.0.local_value_bindings(source, method)
    }
}

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
fn same_batch_class_formal_emits_reader_owned_counter_type_without_cloning_counter() {
    with_paired_jvm_tasty_batch_plan(
        CROSS_CLASS_READER_LITERAL,
        "tests/fixtures/jvm_tasty_cross_class/ReaderLiteral.scala",
        SourceId::from_index(1673),
        |batch| {
            let [counter, reader] = batch.classes else {
                panic!("fixture has Counter and Reader plans");
            };
            assert_eq!(counter.class, batch.jvm_unit.classes[0].class);
            assert_eq!(reader.class, batch.jvm_unit.classes[1].class);
            let read = reader
                .compatibility
                .jvm_metadata()
                .methods()
                .iter()
                .find(|method| {
                    batch.jvm_unit.store.names.try_resolve(
                        batch
                            .jvm_unit
                            .store
                            .symbols
                            .get(method.symbol())
                            .name
                            .text(),
                    ) == Some("read")
                })
                .unwrap();
            assert_eq!(read.descriptor().as_str(), "(Lexample/Counter;)I");
            let output = reader
                .tasty_plan
                .pickle()
                .expect("Counter-typed signature is supported");
            let file = TastyFile::parse_and_validate_scala_3_9(output.bytes()).unwrap();
            let package = file
                .asts()
                .unwrap()
                .get(0)
                .unwrap()
                .decode_package()
                .unwrap();
            assert_eq!(
                file.names().render(package.path_name().unwrap()).unwrap(),
                "example"
            );
            assert_eq!(package.stats.len(), 1);
            let DefinitionBody::TypeDef { name, .. } = package
                .stats
                .get(0)
                .unwrap()
                .decode_definition_body()
                .unwrap()
            else {
                panic!("Reader sidecar contains exactly the Reader class");
            };
            assert_eq!(file.names().render(name).unwrap(), "Reader");
            let address_index = file.ast_address_index().unwrap();
            let parameter_nodes = address_index
                .iter()
                .filter(|node| node.tag == PARAM_TAG)
                .collect::<Vec<_>>();
            assert_eq!(
                parameter_nodes.len(),
                1,
                "Reader.read owns exactly its counter formal"
            );
            let formal = parameter_nodes[0].decode_parameter().unwrap();
            assert_eq!(file.names().render(formal.name()).unwrap(), "counter");
            let body = formal.decode_body().unwrap();
            assert_eq!(body.type_tree.nodes()[0].tag, IDENTTPT_TAG);
            assert!(
                body.type_tree
                    .nodes()
                    .iter()
                    .any(|node| node.tag == TYPEREF_TAG)
            );
            assert!(
                body.type_tree
                    .nodes()
                    .iter()
                    .any(|node| node.tag == TERMREFPKG_TAG)
            );
            assert!(
                !body
                    .type_tree
                    .nodes()
                    .iter()
                    .any(|node| node.tag == TYPEREFDIRECT_TAG)
            );
            assert!(
                body.type_tree
                    .name_refs()
                    .iter()
                    .any(|name| file.names().render(*name).unwrap() == "Counter")
            );
            assert!(
                body.type_tree
                    .name_refs()
                    .iter()
                    .any(|name| file.names().render(*name).unwrap() == "example")
            );

            let source_plan = reader.tasty_plan.plan();
            let without_classes = WithoutPlannedClasses(source_plan.typed_bindings);
            let rejected_plan = ClassStatePicklePlan {
                store: source_plan.store,
                definitions: source_plan.definitions,
                source_ast: source_plan.source_ast,
                typed_ast: source_plan.typed_ast,
                source_index: source_plan.source_index,
                typed_bindings: &without_classes,
                source: source_plan.source.clone(),
                class: source_plan.class,
                class_tree: source_plan.class_tree,
                constructor: source_plan.constructor.clone(),
                members: source_plan.members,
            };
            assert!(matches!(
                rejected_plan.validate().unwrap().pickle(),
                Err(PickleError::UnsupportedTypeReference { .. })
            ));
            if std::env::var_os(RUN_SCALA_ORACLE).is_some() {
                run_reader_signature_oracle(output.bytes());
            }
        },
    )
    .unwrap();
}

#[test]
fn cross_class_call_requires_the_exact_planned_target_method() {
    with_paired_jvm_tasty_batch_plan(
        CROSS_CLASS_READER_CALL,
        "tests/fixtures/jvm_tasty_cross_class/Reader.scala",
        SourceId::from_index(1674),
        |batch| {
            let source_plan = batch.classes[1].tasty_plan.plan();
            let without_methods = WithoutPlannedMethods(source_plan.typed_bindings);
            let plan = ClassStatePicklePlan {
                store: source_plan.store,
                definitions: source_plan.definitions,
                source_ast: source_plan.source_ast,
                typed_ast: source_plan.typed_ast,
                source_index: source_plan.source_index,
                typed_bindings: &without_methods,
                source: source_plan.source.clone(),
                class: source_plan.class,
                class_tree: source_plan.class_tree,
                constructor: source_plan.constructor.clone(),
                members: source_plan.members,
            };
            let Err(error) = plan.validate().unwrap().pickle() else {
                panic!("missing planned method must be rejected");
            };
            assert!(
                matches!(
                    error,
                    PickleError::Reference {
                        reason: dotty_tasty_pickler::ReferenceError::NotRegistered,
                        ..
                    }
                ),
                "unexpected rejection: {error:?}"
            );
        },
    )
    .unwrap();
}

#[test]
fn class_typed_method_formal_is_limited_to_one_parameter() {
    for parameters in [
        "counter: Counter, other: Counter",
        "counter: Counter, value: Int",
    ] {
        let source = CROSS_CLASS_READER_LITERAL.replace("counter: Counter", parameters);
        let result = with_paired_jvm_tasty_batch_plan(
            &source,
            "tests/fixtures/jvm_tasty_cross_class/ReaderLiteral.scala",
            SourceId::from_index(1673),
            |_| (),
        );
        let Err(dotty::jvm_tasty_plan::PairedPlanError::BatchClass { phase, reason, .. }) = result
        else {
            panic!("unsupported multi-formal shape must reject before bytes exist");
        };
        assert_eq!(phase, "JVM/TASTy cross-contract validation");
        assert!(reason.contains("MalformedType"), "{reason}");
    }
}

#[test]
fn malformed_sibling_class_trees_are_rejected_before_pickling() {
    with_paired_jvm_tasty_batch_plan(
        CROSS_CLASS_READER_LITERAL,
        "tests/fixtures/jvm_tasty_cross_class/ReaderLiteral.scala",
        SourceId::from_index(1673),
        |batch| {
            let source_plan = batch.classes[1].tasty_plan.plan();
            let sibling = batch.classes[0].class;
            let Some(dotty_core::SourceDefinition::Canonical {
                source: _,
                tree: sibling_tree,
            }) = source_plan.source_index.definition_of(sibling)
            else {
                panic!("Counter has a canonical source tree");
            };

            let mut missing_template = dotty_core::ast::AstArena::new();
            for (_, tree) in source_plan.source_ast.iter() {
                missing_template.alloc(tree.clone());
            }
            let TreeKind::TypeDef(definition) = &mut missing_template.get_mut(sibling_tree).kind
            else {
                panic!("Counter source tree is a type definition");
            };
            definition.rhs = sibling_tree;
            let plan = ClassStatePicklePlan {
                store: source_plan.store,
                definitions: source_plan.definitions,
                source_ast: &missing_template,
                typed_ast: source_plan.typed_ast,
                source_index: source_plan.source_index,
                typed_bindings: source_plan.typed_bindings,
                source: source_plan.source.clone(),
                class: source_plan.class,
                class_tree: source_plan.class_tree,
                constructor: source_plan.constructor.clone(),
                members: source_plan.members,
            };
            assert!(matches!(
                plan.validate(),
                Err(PickleError::SourceShape { .. })
            ));

            let mut generic_constructor = dotty_core::ast::AstArena::new();
            for (_, tree) in source_plan.source_ast.iter() {
                generic_constructor.alloc(tree.clone());
            }
            let TreeKind::TypeDef(definition) = &generic_constructor.get(sibling_tree).kind else {
                panic!("Counter source tree is a type definition");
            };
            let TreeKind::Template(template) = &generic_constructor.get(definition.rhs).kind else {
                panic!("Counter has a template");
            };
            let constructor_tree = template.constructor;
            let TreeKind::DefDef(constructor) =
                &mut generic_constructor.get_mut(constructor_tree).kind
            else {
                panic!("Counter template has a primary constructor");
            };
            constructor.type_params.push(sibling_tree);
            let plan = ClassStatePicklePlan {
                store: source_plan.store,
                definitions: source_plan.definitions,
                source_ast: &generic_constructor,
                typed_ast: source_plan.typed_ast,
                source_index: source_plan.source_index,
                typed_bindings: source_plan.typed_bindings,
                source: source_plan.source.clone(),
                class: source_plan.class,
                class_tree: source_plan.class_tree,
                constructor: source_plan.constructor.clone(),
                members: source_plan.members,
            };
            assert!(matches!(
                plan.validate(),
                Err(PickleError::Unsupported {
                    feature: dotty_tasty_pickler::UnsupportedFeature::TypeParameters,
                    ..
                })
            ));
        },
    )
    .unwrap();
}

fn run_reader_signature_oracle(reader_tasty: &[u8]) {
    let scala_root = PathBuf::from(
        std::env::var_os("SCALA39_ROOT").expect("pinned Scala root for explicit oracle"),
    );
    let revision = Command::new("git")
        .arg("-C")
        .arg(&scala_root)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("read pinned Scala source revision");
    assert!(revision.status.success());
    assert_eq!(
        String::from_utf8_lossy(&revision.stdout).trim(),
        "777528f19a58e794c9954a42f433373472ec57f8"
    );
    let compiler = scala_root.join("dist/target/universal/stage/bin/scalac");
    let version = Command::new(&compiler)
        .arg("-version")
        .output()
        .expect("query pinned Scala compiler version");
    assert!(version.status.success());
    assert!(
        String::from_utf8_lossy(&version.stdout).contains("3.9.0-bin-SNAPSHOT-git-777528f"),
        "unexpected pinned compiler version: {}",
        String::from_utf8_lossy(&version.stdout)
    );

    let temporary_root = std::env::var_os("DOTTY_TASTY_PICKLER_ORACLE_TMP")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("TMPDIR").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir);
    let temporary = temporary_root.join(format!(
        "dotty-issue-1673-reader-oracle-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&temporary);
    let oracle_classes = temporary.join("oracle");
    let candidate = temporary.join("candidate/example");
    let compiled = temporary.join("compiled");
    fs::create_dir_all(&oracle_classes).unwrap();
    fs::create_dir_all(&candidate).unwrap();
    fs::create_dir_all(&compiled).unwrap();
    let source_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/jvm_tasty_cross_class/ReaderLiteral.scala");
    let oracle = Command::new(&compiler)
        .arg("-d")
        .arg(&oracle_classes)
        .arg(&source_path)
        .output()
        .expect("compile matching Counter dependency with pinned Scala");
    assert!(
        oracle.status.success(),
        "Scala dependency compile failed: {}",
        String::from_utf8_lossy(&oracle.stderr)
    );
    fs::copy(
        oracle_classes.join("example/Counter.class"),
        candidate.join("Counter.class"),
    )
    .unwrap();
    fs::copy(
        oracle_classes.join("example/Counter.tasty"),
        candidate.join("Counter.tasty"),
    )
    .unwrap();
    fs::write(candidate.join("Reader.tasty"), reader_tasty).unwrap();

    let from_tasty = Command::new(&compiler)
        .arg("-from-tasty")
        .arg("-classpath")
        .arg(temporary.join("candidate"))
        .arg("-d")
        .arg(&compiled)
        .arg(candidate.join("Reader.tasty"))
        .output()
        .expect("read Rust-produced Reader.tasty with pinned Scala");
    assert!(
        from_tasty.status.success(),
        "Scala -from-tasty rejected Reader signature: {}",
        String::from_utf8_lossy(&from_tasty.stderr)
    );
    let classpath = format!(
        "{}:{}",
        compiled.display(),
        temporary.join("candidate").display()
    );
    let javap = Command::new("javap")
        .arg("-classpath")
        .arg(&classpath)
        .arg("example.Reader")
        .output()
        .expect("inspect Scala-rebuilt Reader method descriptor");
    assert!(javap.status.success());
    assert!(
        String::from_utf8_lossy(&javap.stdout).contains("read(example.Counter)"),
        "compiled Reader has unexpected signature: {}",
        String::from_utf8_lossy(&javap.stdout)
    );
    fs::remove_dir_all(temporary).unwrap();
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
    let PairedPlanError::BatchUnit { phase, reason } = error else {
        panic!("expected complete-unit JVM validation error, got {error:?}");
    };
    assert_eq!(phase, "complete JVM unit validation");
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
use std::fs;
use std::path::PathBuf;
use std::process::Command;
