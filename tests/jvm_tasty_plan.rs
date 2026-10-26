use dotty::jvm_tasty_plan::{
    PairedPlanError, with_paired_jvm_tasty_batch_plan, with_paired_jvm_tasty_class_plan,
};
use dotty_core::ast::TreeKind;
use dotty_core::{
    SourceId, SymbolId, SymbolInfo, TermRefTarget, TreeId, Type, TypeRefTarget, Typed, Untyped,
};
use dotty_jvm::JvmClassInitializationPlan;
use dotty_tasty::tasty::*;
use dotty_tasty_pickler::{
    ClassMemberPicklePlan, ClassStatePicklePlan, PickleError, SourceTypedBindings,
};

const COUNTER: &str = include_str!("fixtures/jvm_tasty_interop/Counter.scala");
const CROSS_CLASS_READER_LITERAL: &str =
    include_str!("fixtures/jvm_tasty_cross_class/ReaderLiteral.scala");
const CROSS_CLASS_READER_CALL: &str = include_str!("fixtures/jvm_tasty_cross_class/Reader.scala");
const CROSS_CLASS_IDENTITY_RESULT: &str =
    include_str!("fixtures/jvm_tasty_cross_class_factories/CounterIdentitySource.scala");
const CROSS_CLASS_FACTORY_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_cross_class_factories/CounterFactory.scala");
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

struct WithoutTypedTree<'a> {
    inner: &'a dyn SourceTypedBindings,
    missing: TreeId<Untyped>,
}

fn cloned_typed_ast(ast: &dotty_core::ast::AstArena<Typed>) -> dotty_core::ast::AstArena<Typed> {
    let mut cloned = dotty_core::ast::AstArena::new();
    for (expected, tree) in ast.iter() {
        assert_eq!(cloned.alloc(tree.clone()), expected);
    }
    cloned
}

fn cloned_source_ast(
    ast: &dotty_core::ast::AstArena<Untyped>,
) -> dotty_core::ast::AstArena<Untyped> {
    let mut cloned = dotty_core::ast::AstArena::new();
    for (expected, tree) in ast.iter() {
        assert_eq!(cloned.alloc(tree.clone()), expected);
    }
    cloned
}

impl SourceTypedBindings for WithoutTypedTree<'_> {
    fn typed_at(&self, source: SourceId, tree: TreeId<Untyped>) -> Option<TreeId<Typed>> {
        (tree != self.missing)
            .then(|| self.inner.typed_at(source, tree))
            .flatten()
    }

    fn planned_class_symbols(&self) -> &[SymbolId] {
        self.inner.planned_class_symbols()
    }

    fn planned_method_symbols(&self) -> &[SymbolId] {
        self.inner.planned_method_symbols()
    }

    fn local_value_bindings(
        &self,
        source: SourceId,
        method: SymbolId,
    ) -> Vec<dotty_tasty_pickler::LocalValueBinding> {
        self.inner.local_value_bindings(source, method)
    }
}

struct WithStaleTypedTree<'a> {
    inner: &'a dyn SourceTypedBindings,
    stale: TreeId<Untyped>,
    replacement: TreeId<Typed>,
}

impl SourceTypedBindings for WithStaleTypedTree<'_> {
    fn typed_at(&self, source: SourceId, tree: TreeId<Untyped>) -> Option<TreeId<Typed>> {
        if tree == self.stale {
            Some(self.replacement)
        } else {
            self.inner.typed_at(source, tree)
        }
    }

    fn planned_class_symbols(&self) -> &[SymbolId] {
        self.inner.planned_class_symbols()
    }

    fn planned_method_symbols(&self) -> &[SymbolId] {
        self.inner.planned_method_symbols()
    }

    fn local_value_bindings(
        &self,
        source: SourceId,
        method: SymbolId,
    ) -> Vec<dotty_tasty_pickler::LocalValueBinding> {
        self.inner.local_value_bindings(source, method)
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
fn same_batch_class_result_emits_identity_tasty_and_rejects_nonformal_constructor_arguments() {
    with_paired_jvm_tasty_batch_plan(
        CROSS_CLASS_IDENTITY_RESULT,
        "tests/fixtures/jvm_tasty_cross_class_factories/CounterIdentitySource.scala",
        SourceId::from_index(1692),
        |batch| {
            let [counter, factory] = batch.classes else {
                panic!("fixture has Counter and CounterIdentity plans");
            };
            let plan = factory.tasty_plan.plan();
            let method = factory
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
                    ) == Some("identity")
                })
                .expect("identity is present in the JVM class plan");
            assert_eq!(
                method.descriptor().as_str(),
                "(Lexample/Counter;)Lexample/Counter;"
            );

            let ClassMemberPicklePlan::Method {
                symbol: method_symbol,
                source_tree,
            } = plan.members[0]
            else {
                panic!("CounterIdentity contains its identity method")
            };
            let TreeKind::DefDef(source_method) = &plan.source_ast.get(source_tree).kind else {
                panic!("identity source member is a method")
            };
            let rhs = source_method.rhs.expect("identity has its original RHS");
            let typed_root = plan
                .typed_bindings
                .typed_at(plan.source.id, rhs)
                .expect("identity RHS has its frontend binding");
            let TreeKind::Ident(_) = &plan.typed_ast.get(typed_root).kind else {
                panic!("identity returns the original direct parameter reference")
            };
            let Type::TermRef {
                target: TermRefTarget::Symbol(formal),
                ..
            } = plan.store.types.get(plan.typed_ast.get(typed_root).ty)
            else {
                panic!("identity RHS keeps its exact formal TermRef")
            };
            assert_eq!(plan.store.symbols.get(*formal).owner, Some(method_symbol));
            assert_eq!(
                plan.store.symbols.get(method_symbol).owner,
                Some(factory.class)
            );

            let SymbolInfo::Complete(signature) = plan.store.symbols.get(method_symbol).info else {
                panic!("identity signature is complete")
            };
            let Type::Method(signature) = plan.store.types.get(signature) else {
                panic!("identity has a method signature")
            };
            let referenced_class = |ty| match plan.store.types.get(ty) {
                Type::TypeRef {
                    target: TypeRefTarget::Symbol(class),
                    ..
                } => *class,
                _ => panic!("identity formal/result use a canonical class TypeRef"),
            };
            assert_eq!(signature.params.len(), 1);
            assert_eq!(referenced_class(signature.params[0].ty), counter.class);
            assert_eq!(referenced_class(signature.result), counter.class);
            assert_eq!(
                signature.params[0].name.as_name(),
                &plan.store.symbols.get(*formal).name
            );

            let output = factory
                .tasty_plan
                .pickle()
                .expect("same-batch Counter result serializes as semantic TASTy");
            let file = TastyFile::parse_and_validate_scala_3_9(output.bytes()).unwrap();
            let package = file
                .asts()
                .unwrap()
                .get(0)
                .unwrap()
                .decode_package()
                .unwrap();
            assert_eq!(package.stats.len(), 1, "no local Counter clone is emitted");
            let DefinitionBody::TypeDef { name, .. } = package
                .stats
                .get(0)
                .unwrap()
                .decode_definition_body()
                .unwrap()
            else {
                panic!("the sidecar contains one class declaration")
            };
            assert_eq!(file.names().render(name).unwrap(), "CounterIdentity");

            let declaration = output
                .declarations()
                .iter()
                .find(|declaration| declaration.symbol == method_symbol)
                .expect("the method has its canonical TASTy declaration");
            let body = file
                .ast_address_index()
                .unwrap()
                .get(declaration.address)
                .unwrap()
                .decode_defdef_body()
                .unwrap();
            assert_eq!(file.names().render(body.name).unwrap(), "identity");
            assert!(
                body.return_type
                    .nodes()
                    .iter()
                    .any(|node| node.tag == IDENTTPT_TAG)
            );
            assert!(
                body.return_type
                    .nodes()
                    .iter()
                    .any(|node| node.tag == TYPEREF_TAG)
            );
            assert!(
                body.return_type
                    .nodes()
                    .iter()
                    .any(|node| node.tag == TERMREFPKG_TAG)
            );
            let result_names = body
                .return_type
                .name_refs()
                .iter()
                .map(|name| file.names().render(*name).unwrap())
                .collect::<Vec<_>>();
            assert!(result_names.iter().any(|name| name == "Counter"));
            assert!(result_names.iter().any(|name| name == "example"));
            let formal_type_names = body.parameters[0]
                .decode_body()
                .unwrap()
                .type_tree
                .name_refs()
                .iter()
                .map(|name| file.names().render(*name).unwrap())
                .collect::<Vec<_>>();
            assert!(formal_type_names.iter().any(|name| name == "Counter"));
            assert!(formal_type_names.iter().any(|name| name == "example"));
            assert!(
                body.rhs
                    .as_ref()
                    .unwrap()
                    .nodes()
                    .iter()
                    .any(|node| node.tag == TERMREFDIRECT_TAG)
            );
            let formal_declaration = output
                .declarations()
                .iter()
                .find(|declaration| declaration.symbol == *formal)
                .expect("the original formal has a bound PARAM address");
            let references = file
                .ast_address_index()
                .unwrap()
                .get(declaration.address)
                .unwrap()
                .ast_refs()
                .unwrap();
            assert!(
                references.iter().any(|reference| {
                    reference.kind == AstRefKind::TermRefDirect
                        && reference.address == formal_declaration.address
                }),
                "references={references:?}; method={declaration:?}; formal={formal_declaration:?}"
            );

            let without_planned_classes = WithoutPlannedClasses(plan.typed_bindings);
            let rejected_plan = ClassStatePicklePlan {
                store: plan.store,
                definitions: plan.definitions,
                source_ast: plan.source_ast,
                typed_ast: plan.typed_ast,
                source_index: plan.source_index,
                typed_bindings: &without_planned_classes,
                source: plan.source.clone(),
                class: plan.class,
                class_tree: plan.class_tree,
                constructor: plan.constructor.clone(),
                members: plan.members,
            };
            assert!(matches!(
                rejected_plan.validate().unwrap().pickle(),
                Err(PickleError::MalformedType {
                    role: dotty_tasty_pickler::TypeRole::MethodResult,
                    ..
                }) | Err(PickleError::UnsupportedTypeReference { .. })
            ));

            if std::env::var_os(RUN_SCALA_ORACLE).is_some() {
                let counter_output = counter.tasty_plan.pickle().unwrap();
                run_identity_result_oracle(counter_output.bytes(), output.bytes());
            }
        },
    )
    .unwrap();

    let construction_source = CROSS_CLASS_IDENTITY_RESULT.replace(
        "def identity(counter: Counter): Counter = counter",
        "def identity(counter: Counter): Counter = new Counter(1)",
    );
    let construction = with_paired_jvm_tasty_batch_plan(
        &construction_source,
        "CounterIdentitySource.scala",
        SourceId::from_index(1693),
        |_| panic!("constructor literals are outside the validated source slice"),
    );
    let Err(PairedPlanError::BatchClass { phase, reason, .. }) = construction else {
        panic!(
            "an unsupported constructor argument must fail before a completed batch: {construction:?}"
        );
    };
    assert_eq!(phase, "JVM/TASTy cross-contract validation");
    assert!(reason.contains("TastyPlan(Unsupported"), "{reason}");
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
fn cross_class_call_rejects_an_overloaded_target_name() {
    let source = CROSS_CLASS_READER_CALL.replace(
        "  def set(value: Int): Unit = current = value",
        "  def get(value: Int): Int = value\n  def set(value: Int): Unit = current = value",
    );
    let Err(error) = with_paired_jvm_tasty_batch_plan(
        &source,
        "tests/fixtures/jvm_tasty_cross_class/Reader.scala",
        SourceId::from_index(1674),
        |_| (),
    ) else {
        panic!("an overloaded external method target must reject the batch");
    };
    let diagnostic = format!("{error:?}");
    assert!(
        diagnostic.contains("Ambiguous") || diagnostic.contains("Unsupported"),
        "overloaded method returned no contextual typed rejection: {diagnostic}"
    );
}

#[test]
fn cross_class_call_rejects_a_missing_source_to_typed_body_binding() {
    with_paired_jvm_tasty_batch_plan(
        CROSS_CLASS_READER_CALL,
        "tests/fixtures/jvm_tasty_cross_class/Reader.scala",
        SourceId::from_index(1674),
        |batch| {
            let source_plan = batch.classes[1].tasty_plan.plan();
            let read_tree = source_plan
                .members
                .iter()
                .find_map(|member| match member {
                    dotty_tasty_pickler::ClassMemberPicklePlan::Method {
                        source_tree, ..
                    } if matches!(
                        &source_plan.source_ast.get(*source_tree).kind,
                        TreeKind::DefDef(definition)
                            if source_plan.store.names.resolve(definition.name.as_name().text()) == "read"
                    ) => Some(*source_tree),
                    _ => None,
                })
                .expect("the Reader plan includes its read method");
            let TreeKind::DefDef(read) = &source_plan.source_ast.get(read_tree).kind else {
                panic!("read is a source method");
            };
            let body = read.rhs.expect("read has an application body");
            let TreeKind::Apply(application) = &source_plan.source_ast.get(body).kind else {
                panic!("read body is a regular application");
            };
            let selected_function = application.function;
            let without_body = WithoutTypedTree {
                inner: source_plan.typed_bindings,
                missing: selected_function,
            };
            let plan = ClassStatePicklePlan {
                store: source_plan.store,
                definitions: source_plan.definitions,
                source_ast: source_plan.source_ast,
                typed_ast: source_plan.typed_ast,
                source_index: source_plan.source_index,
                typed_bindings: &without_body,
                source: source_plan.source.clone(),
                class: source_plan.class,
                class_tree: source_plan.class_tree,
                constructor: source_plan.constructor.clone(),
                members: source_plan.members,
            };
            let Err(error) = plan.validate().unwrap().pickle() else {
                panic!("a missing body root binding must reject before emission");
            };
            assert!(
                matches!(error, PickleError::BodyBinding { tree, .. } if tree == selected_function),
                "unexpected rejection for missing source-to-typed body binding: {error:?}"
            );

            let typed_body = source_plan
                .typed_bindings
                .typed_at(source_plan.source.id, body)
                .expect("the frontend binds the application root");
            let TreeKind::Apply(application) = &source_plan.typed_ast.get(typed_body).kind else {
                panic!("typed read body is an application");
            };
            let TreeKind::Select(selection) = &source_plan
                .typed_ast
                .get(application.function)
                .kind
            else {
                panic!("typed application selects get");
            };
            let stale = WithStaleTypedTree {
                inner: source_plan.typed_bindings,
                stale: selected_function,
                replacement: selection.qualifier,
            };
            let stale_plan = ClassStatePicklePlan {
                store: source_plan.store,
                definitions: source_plan.definitions,
                source_ast: source_plan.source_ast,
                typed_ast: source_plan.typed_ast,
                source_index: source_plan.source_index,
                typed_bindings: &stale,
                source: source_plan.source.clone(),
                class: source_plan.class,
                class_tree: source_plan.class_tree,
                constructor: source_plan.constructor.clone(),
                members: source_plan.members,
            };
            let Err(error) = stale_plan.validate().unwrap().pickle() else {
                panic!("a stale nested typed root must reject before emission");
            };
            assert!(
                matches!(error, PickleError::BodyBinding { tree, .. } if tree == selected_function),
                "unexpected rejection for stale nested typed root: {error:?}"
            );
        },
    )
    .unwrap();
}

#[test]
fn constructor_body_rejects_missing_type_tree_binding_and_inconsistent_new_types() {
    with_paired_jvm_tasty_batch_plan(
        CROSS_CLASS_FACTORY_SOURCE,
        "tests/fixtures/jvm_tasty_cross_class_factories/CounterFactory.scala",
        SourceId::from_index(1693),
        |batch| {
            let source_plan = batch.classes[1].tasty_plan.plan();
            let create_tree = source_plan
                .members
                .iter()
                .find_map(|member| match member {
                    ClassMemberPicklePlan::Method {
                        source_tree,
                        symbol,
                        ..
                    } if source_plan
                        .store
                        .names
                        .resolve(source_plan.store.symbols.get(*symbol).name.text())
                        == "create" =>
                    {
                        Some(*source_tree)
                    }
                    _ => None,
                })
                .expect("the factory plan contains create");
            let TreeKind::DefDef(create) = &source_plan.source_ast.get(create_tree).kind else {
                panic!("create is a source method");
            };
            let body = create
                .rhs
                .expect("create has a constructor application body");
            let TreeKind::Apply(application) = &source_plan.source_ast.get(body).kind else {
                panic!("create returns an application");
            };
            let TreeKind::Select(selection) =
                &source_plan.source_ast.get(application.function).kind
            else {
                panic!("constructor application selects the primary constructor");
            };
            let source_new_id = selection.qualifier;
            let TreeKind::New(source_new) = &source_plan.source_ast.get(source_new_id).kind else {
                panic!("selected constructor receiver is new");
            };
            let source_type_id = source_new.tpt;
            let expected_typed_type = source_plan
                .typed_bindings
                .typed_at(source_plan.source.id, source_type_id)
                .expect("the frontend binds the new expression type tree");

            let missing_type_binding = WithoutTypedTree {
                inner: source_plan.typed_bindings,
                missing: source_type_id,
            };
            let missing_plan = ClassStatePicklePlan {
                store: source_plan.store,
                definitions: source_plan.definitions,
                source_ast: source_plan.source_ast,
                typed_ast: source_plan.typed_ast,
                source_index: source_plan.source_index,
                typed_bindings: &missing_type_binding,
                source: source_plan.source.clone(),
                class: source_plan.class,
                class_tree: source_plan.class_tree,
                constructor: source_plan.constructor.clone(),
                members: source_plan.members,
            };
            let Err(error) = missing_plan.validate().unwrap().pickle() else {
                panic!("missing source-to-typed type-tree evidence must reject");
            };
            assert!(
                matches!(error, PickleError::BodyBinding { tree, .. } if tree == source_type_id),
                "missing constructor type-tree binding rejected unexpectedly: {error:?}"
            );

            let replacement = source_plan
                .typed_ast
                .iter()
                .map(|(tree, _)| tree)
                .find(|tree| *tree != expected_typed_type)
                .expect("the typed arena contains a distinct replacement tree");
            let substituted_type_binding = WithStaleTypedTree {
                inner: source_plan.typed_bindings,
                stale: source_type_id,
                replacement,
            };
            let substituted_plan = ClassStatePicklePlan {
                store: source_plan.store,
                definitions: source_plan.definitions,
                source_ast: source_plan.source_ast,
                typed_ast: source_plan.typed_ast,
                source_index: source_plan.source_index,
                typed_bindings: &substituted_type_binding,
                source: source_plan.source.clone(),
                class: source_plan.class,
                class_tree: source_plan.class_tree,
                constructor: source_plan.constructor.clone(),
                members: source_plan.members,
            };
            let Err(error) = substituted_plan.validate().unwrap().pickle() else {
                panic!("a substituted constructor type-tree binding must reject");
            };
            assert!(
                matches!(error, PickleError::BodyBinding { tree, .. } if tree == source_type_id),
                "substituted constructor type-tree binding rejected unexpectedly: {error:?}"
            );

            let typed_new_id = source_plan
                .typed_bindings
                .typed_at(source_plan.source.id, source_new_id)
                .expect("the frontend binds the new expression");
            let typed_body_id = source_plan
                .typed_bindings
                .typed_at(source_plan.source.id, body)
                .expect("the frontend binds the constructor application");
            for invalid_type in [
                source_plan.definitions.int,
                source_plan.definitions.object_type,
            ] {
                let mut typed_ast = cloned_typed_ast(source_plan.typed_ast);
                typed_ast.get_mut(typed_new_id).ty = invalid_type;
                let invalid_new_type_plan = ClassStatePicklePlan {
                    store: source_plan.store,
                    definitions: source_plan.definitions,
                    source_ast: source_plan.source_ast,
                    typed_ast: &typed_ast,
                    source_index: source_plan.source_index,
                    typed_bindings: source_plan.typed_bindings,
                    source: source_plan.source.clone(),
                    class: source_plan.class,
                    class_tree: source_plan.class_tree,
                    constructor: source_plan.constructor.clone(),
                    members: source_plan.members,
                };
                let Err(error) = invalid_new_type_plan.validate().unwrap().pickle() else {
                    panic!("an inconsistent typed New result must reject: {invalid_type:?}");
                };
                assert!(
                    matches!(error, PickleError::BodyType { tree, .. } if tree == typed_body_id),
                    "inconsistent typed New result rejected unexpectedly: {error:?}"
                );
            }
        },
    )
    .unwrap();
}

#[test]
fn cross_class_call_rejects_a_forged_selected_target_type() {
    with_paired_jvm_tasty_batch_plan(
        CROSS_CLASS_READER_CALL,
        "tests/fixtures/jvm_tasty_cross_class/Reader.scala",
        SourceId::from_index(1674),
        |batch| {
            let source_plan = batch.classes[1].tasty_plan.plan();
            let read_tree = source_plan
                .members
                .iter()
                .find_map(|member| match member {
                    dotty_tasty_pickler::ClassMemberPicklePlan::Method {
                        source_tree, ..
                    } if matches!(
                        &source_plan.source_ast.get(*source_tree).kind,
                        TreeKind::DefDef(definition)
                            if source_plan.store.names.resolve(definition.name.as_name().text()) == "read"
                    ) => Some(*source_tree),
                    _ => None,
                })
                .expect("the Reader plan includes its read method");
            let TreeKind::DefDef(read) = &source_plan.source_ast.get(read_tree).kind else {
                panic!("read is a source method");
            };
            let source_body = read.rhs.expect("read has an application body");
            let typed_body = source_plan
                .typed_bindings
                .typed_at(source_plan.source.id, source_body)
                .expect("the frontend binds the source application root");
            let TreeKind::Apply(application) = &source_plan.typed_ast.get(typed_body).kind else {
                panic!("typed read body is an application");
            };
            let typed_function = application.function;
            let TreeKind::Select(selection) =
                &source_plan.typed_ast.get(typed_function).kind
            else {
                panic!("typed application selects get");
            };
            let receiver_type = source_plan
                .typed_ast
                .get(selection.qualifier)
                .ty;
            let mut typed_ast = cloned_typed_ast(source_plan.typed_ast);
            typed_ast.get_mut(typed_function).ty = receiver_type;
            let plan = ClassStatePicklePlan {
                store: source_plan.store,
                definitions: source_plan.definitions,
                source_ast: source_plan.source_ast,
                typed_ast: &typed_ast,
                source_index: source_plan.source_index,
                typed_bindings: source_plan.typed_bindings,
                source: source_plan.source.clone(),
                class: source_plan.class,
                class_tree: source_plan.class_tree,
                constructor: source_plan.constructor.clone(),
                members: source_plan.members,
            };
            let Err(error) = plan.validate().unwrap().pickle() else {
                panic!("a forged selected target must reject before emission");
            };
            assert!(
                matches!(error, PickleError::Reference { .. } | PickleError::BodyType { .. }),
                "unexpected rejection for forged selected target: {error:?}"
            );
        },
    )
    .unwrap();
}

#[test]
fn cross_class_call_rejects_unsupported_invocation_and_result_shapes() {
    with_paired_jvm_tasty_batch_plan(
        CROSS_CLASS_READER_CALL,
        "tests/fixtures/jvm_tasty_cross_class/Reader.scala",
        SourceId::from_index(1674),
        |batch| {
            let source_plan = batch.classes[1].tasty_plan.plan();
            let read_tree = source_plan
                .members
                .iter()
                .find_map(|member| match member {
                    dotty_tasty_pickler::ClassMemberPicklePlan::Method {
                        source_tree, ..
                    } if matches!(
                        &source_plan.source_ast.get(*source_tree).kind,
                        TreeKind::DefDef(definition)
                            if source_plan.store.names.resolve(definition.name.as_name().text()) == "read"
                    ) => Some(*source_tree),
                    _ => None,
                })
                .expect("the Reader plan includes its read method");
            let TreeKind::DefDef(read) = &source_plan.source_ast.get(read_tree).kind else {
                panic!("read is a source method");
            };
            let source_body = read.rhs.expect("read has a source body");
            let typed_body = source_plan
                .typed_bindings
                .typed_at(source_plan.source.id, source_body)
                .expect("the frontend binds the application root");
            let TreeKind::Apply(application) = &source_plan.typed_ast.get(typed_body).kind else {
                panic!("typed read body is an application");
            };
            let typed_function = application.function;
            let TreeKind::Select(selection) =
                &source_plan.typed_ast.get(typed_function).kind
            else {
                panic!("typed call selects get");
            };
            let typed_receiver = selection.qualifier;
            let selected_type = source_plan.typed_ast.get(typed_function).ty;
            let dotty_core::Type::TermRef {
                target: dotty_core::TermRefTarget::Symbol(target),
                ..
            } = source_plan.store.types.get(selected_type)
            else {
                panic!("selected function has a canonical method TermRef");
            };
            let target = *target;
            let dotty_core::SourceDefinition::Canonical { tree: target_tree, .. } =
                source_plan.source_index.definition_of(target).unwrap()
            else {
                panic!("Counter.get has a canonical source declaration");
            };
            let source_select = match &source_plan.source_ast.get(source_body).kind {
                TreeKind::Apply(application) => application.function,
                _ => unreachable!("source read body is an application"),
            };

            for label in [
                "nonempty argument clause",
                "Boolean result",
                "unsupported receiver expression",
                "missing explicit empty source clause",
                "foreign receiver owner",
                "wrong target signature",
                "private target",
                "abstract target",
            ] {
                let mut typed_ast = cloned_typed_ast(source_plan.typed_ast);
                let mut source_ast = cloned_source_ast(source_plan.source_ast);
                match label {
                    "nonempty argument clause" => {
                        let TreeKind::Apply(application) =
                            &mut typed_ast.get_mut(typed_body).kind
                        else {
                            unreachable!("typed body is an application")
                        };
                        application.args.push(typed_receiver);
                    }
                    "Boolean result" => {
                        typed_ast.get_mut(typed_body).ty = source_plan.definitions.boolean;
                    }
                    "unsupported receiver expression" => {
                        let nested = typed_ast.get(typed_body).kind.clone();
                        typed_ast.get_mut(typed_receiver).kind = nested;
                    }
                    "missing explicit empty source clause" => {
                        let selected = source_ast.get(source_select).kind.clone();
                        source_ast.get_mut(source_body).kind = selected;
                    }
                    "foreign receiver owner" => {
                        let selected_type = typed_ast.get(typed_function).ty;
                        typed_ast.get_mut(typed_receiver).ty = selected_type;
                    }
                    "wrong target signature" => {
                        let dotty_core::SymbolInfo::Complete(signature) =
                            source_plan.store.symbols.get(target).info
                        else {
                            unreachable!("Counter.get has a completed signature")
                        };
                        typed_ast.get_mut(typed_function).ty = signature;
                    }
                    "private target" => {
                        let TreeKind::DefDef(definition) =
                            &mut source_ast.get_mut(target_tree).kind
                        else {
                            unreachable!("Counter.get has a source method tree")
                        };
                        definition.metadata.visibility = Some(
                            dotty_core::ast::VisibilitySyntax::Private { qualifier: None },
                        );
                    }
                    "abstract target" => {
                        let TreeKind::DefDef(definition) =
                            &mut source_ast.get_mut(target_tree).kind
                        else {
                            unreachable!("Counter.get has a source method tree")
                        };
                        definition.rhs = None;
                    }
                    _ => unreachable!(),
                }
                let plan = ClassStatePicklePlan {
                    store: source_plan.store,
                    definitions: source_plan.definitions,
                    source_ast: &source_ast,
                    typed_ast: &typed_ast,
                    source_index: source_plan.source_index,
                    typed_bindings: source_plan.typed_bindings,
                    source: source_plan.source.clone(),
                    class: source_plan.class,
                    class_tree: source_plan.class_tree,
                    constructor: source_plan.constructor.clone(),
                    members: source_plan.members,
                };
                let error = match plan.validate() {
                    Err(error) => error,
                    Ok(validated) => match validated.pickle() {
                        Err(error) => error,
                        Ok(_) => panic!("unsupported {label} was accepted by the pickler"),
                    },
                };
                assert!(
                    matches!(
                        error,
                        PickleError::Unsupported { .. }
                            | PickleError::BodyType { .. }
                            | PickleError::Reference { .. }
                            | PickleError::SourceShape { .. }
                    ),
                    "unsupported {label} returned no contextual typed rejection: {error:?}"
                );
            }
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

fn run_identity_result_oracle(counter_tasty: &[u8], identity_tasty: &[u8]) {
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
    assert!(String::from_utf8_lossy(&version.stdout).contains("3.9.0-bin-SNAPSHOT-git-777528f"));

    let temporary_root = std::env::var_os("DOTTY_TASTY_PICKLER_ORACLE_TMP")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("TMPDIR").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir);
    let temporary = temporary_root.join(format!(
        "dotty-issue-1692-identity-oracle-{}",
        std::process::id()
    ));
    let candidate = temporary.join("candidate/example");
    let compiled = temporary.join("compiled");
    fs::remove_dir_all(&temporary).ok();
    fs::create_dir_all(&candidate).unwrap();
    fs::create_dir_all(&compiled).unwrap();
    fs::write(candidate.join("Counter.tasty"), counter_tasty).unwrap();
    fs::write(candidate.join("CounterIdentity.tasty"), identity_tasty).unwrap();

    let from_tasty = Command::new(&compiler)
        .arg("-from-tasty")
        .arg("-classpath")
        .arg(temporary.join("candidate"))
        .arg("-d")
        .arg(&compiled)
        .arg(candidate.join("Counter.tasty"))
        .arg(candidate.join("CounterIdentity.tasty"))
        .output()
        .expect("consume both Rust-produced TASTy sidecars with Scala 3.9");
    assert!(
        from_tasty.status.success(),
        "Scala -from-tasty rejected reference-result sidecars: {}",
        String::from_utf8_lossy(&from_tasty.stderr)
    );

    let javap = Command::new("javap")
        .arg("-classpath")
        .arg(&compiled)
        .arg("-s")
        .arg("example.CounterIdentity")
        .output()
        .expect("inspect Scala-rebuilt identity descriptor");
    assert!(javap.status.success());
    assert!(
        String::from_utf8_lossy(&javap.stdout).contains("(Lexample/Counter;)Lexample/Counter;"),
        "Scala-rebuilt identity has the wrong descriptor: {}",
        String::from_utf8_lossy(&javap.stdout)
    );

    let client = temporary.join("IdentityClient.java");
    fs::write(
        &client,
        "package example; public final class IdentityClient { public static void main(String[] args) { Counter counter = new Counter(7); if (new CounterIdentity().identity(counter) != counter || counter.get() != 7) throw new AssertionError(\"identity result\"); } }\n",
    )
    .unwrap();
    let javac = Command::new("javac")
        .arg("-classpath")
        .arg(&compiled)
        .arg("-d")
        .arg(&compiled)
        .arg(&client)
        .status()
        .expect("compile Java identity-result verification client");
    assert!(javac.success(), "Java client compilation failed");
    let java = Command::new("java")
        .arg("-Xverify:all")
        .arg("-classpath")
        .arg(&compiled)
        .arg("example.IdentityClient")
        .status()
        .expect("run Java 21 identity-result verification client");
    assert!(
        java.success(),
        "Scala-rebuilt identity result failed at runtime"
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
