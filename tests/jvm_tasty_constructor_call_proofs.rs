use dotty::jvm_tasty_cross_class::{
    CrossClassConstructorCallProofInput, ImmutableLocalInitializationProofInput,
    ImmutableLocalInitializerPairProofInput, ValidatedCrossClassConstructorCallProof,
    validate_cross_class_constructor_call_proofs, validate_immutable_local_counter_set_call_proof,
    validate_immutable_local_counter_set_calls_proof,
    validate_immutable_local_initialization_proof, validate_immutable_local_initializer_pair_proof,
    validate_ordered_constructor_argument_bindings,
};
use dotty::jvm_tasty_plan::{PairedPlanError, with_paired_jvm_tasty_batch_plan};
use dotty_core::ast::{AstArena, AstPhase, TreeKind};
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceDefinition, SourceId, SourceSpan, SourceText,
    SymbolInfo, SymbolOrigin, TermRefTarget, Type, TypeRefTarget,
};
use dotty_jvm::{
    JvmClassInitializationPlan, JvmClassPlan, JvmCompilationUnit, JvmMethodPlan,
    JvmPrimaryConstructorPlan,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_typer::{SourceTyper, TypedLocalBinding};

const SOURCE: &str =
    include_str!("fixtures/jvm_tasty_reference_constructor_calls/CounterOwnerFactory.scala");
const TWO_ARGUMENT_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_multi_argument_constructor_calls/PairFactory.scala");
const MIXED_ORIGIN_SOURCE: &str = include_str!(
    "fixtures/jvm_tasty_mixed_origin_constructor_arguments/wrap-and-create/PairFactory.scala"
);
const IMMUTABLE_LOCAL_PAIR_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_immutable_reference_local_pair/PairFactory.scala");
const IMMUTABLE_LOCAL_PAIR_REVERSED_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_immutable_reference_local_pair/PairFactoryReversed.scala");
const TWO_IMMUTABLE_LOCALS_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_two_immutable_reference_locals/TwoLocals.scala");
const COUNTER_SET_CALL_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_local_receiver_call/LocalReceiverCall.scala");
const ORDERED_COUNTER_SET_CALLS_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_ordered_local_unit_calls/OrderedLocalUnitCalls.scala");
const TWO_IMMUTABLE_LOCALS_REVERSED_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_two_immutable_reference_locals/TwoLocalsReversed.scala");

fn clone_arena<P: AstPhase + Clone>(source: &AstArena<P>) -> AstArena<P> {
    let mut cloned = AstArena::new();
    for (_, tree) in source.iter() {
        cloned.alloc(tree.clone());
    }
    cloned
}

fn with_two_local_proof_session<R>(
    source: &str,
    source_number: u32,
    consume: impl for<'session> FnOnce(
        &SemanticStore,
        &Definitions,
        &AstArena<dotty_core::ast::Untyped>,
        &dotty_core::ast::TypedAst,
        &dotty_core::SourceSemanticIndex,
        &dotty_core::SourceTypedIndex,
        SourceId,
        dotty_core::TreeId<dotty_core::ast::Untyped>,
        [TypedLocalBinding; 2],
        Option<TypedLocalBinding>,
        Option<&SourceTyper<'session>>,
        &JvmCompilationUnit<'session>,
    ) -> R,
) -> R {
    with_two_local_proof_session_internal(source, source_number, |_, _| {}, false, consume)
}

fn with_two_local_proof_session_retaining_typer<R>(
    source: &str,
    source_number: u32,
    consume: impl for<'session> FnOnce(
        &SemanticStore,
        &Definitions,
        &AstArena<dotty_core::ast::Untyped>,
        &dotty_core::ast::TypedAst,
        &dotty_core::SourceSemanticIndex,
        &dotty_core::SourceTypedIndex,
        SourceId,
        dotty_core::TreeId<dotty_core::ast::Untyped>,
        [TypedLocalBinding; 2],
        Option<TypedLocalBinding>,
        Option<&SourceTyper<'session>>,
        &JvmCompilationUnit<'session>,
    ) -> R,
) -> R {
    with_two_local_proof_session_internal(source, source_number, |_, _| {}, true, consume)
}

fn with_two_local_proof_session_mutating<R>(
    source: &str,
    source_number: u32,
    mutate_store: impl FnOnce(&mut SemanticStore, [TypedLocalBinding; 2]),
    consume: impl for<'session> FnOnce(
        &SemanticStore,
        &Definitions,
        &AstArena<dotty_core::ast::Untyped>,
        &dotty_core::ast::TypedAst,
        &dotty_core::SourceSemanticIndex,
        &dotty_core::SourceTypedIndex,
        SourceId,
        dotty_core::TreeId<dotty_core::ast::Untyped>,
        [TypedLocalBinding; 2],
        Option<TypedLocalBinding>,
        Option<&SourceTyper<'session>>,
        &JvmCompilationUnit<'session>,
    ) -> R,
) -> R {
    with_two_local_proof_session_internal(source, source_number, mutate_store, false, consume)
}

fn with_two_local_proof_session_internal<R>(
    source: &str,
    source_number: u32,
    mutate_store: impl FnOnce(&mut SemanticStore, [TypedLocalBinding; 2]),
    retain_typer: bool,
    consume: impl for<'session> FnOnce(
        &SemanticStore,
        &Definitions,
        &AstArena<dotty_core::ast::Untyped>,
        &dotty_core::ast::TypedAst,
        &dotty_core::SourceSemanticIndex,
        &dotty_core::SourceTypedIndex,
        SourceId,
        dotty_core::TreeId<dotty_core::ast::Untyped>,
        [TypedLocalBinding; 2],
        Option<TypedLocalBinding>,
        Option<&SourceTyper<'session>>,
        &JvmCompilationUnit<'session>,
    ) -> R,
) -> R {
    let source_id = SourceId::from_index(source_number);
    let source_text = SourceText::new(source).unwrap();
    let scanner = ContextualScanner::new(source).unwrap();
    assert!(scanner.diagnostics().is_empty());
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = parse_compilation_unit(source_text, source_id, scanner, &mut store.names);
    assert!(parsed.diagnostics.is_empty());
    let mut packages = Packages::new();
    let source_index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        "two-local-initializer-proof.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        result => panic!("source names without diagnostics: {result:?}"),
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("source has one package root");
    };
    let mut class_data = Vec::new();
    for &class_tree in &package.stats {
        let TreeKind::TypeDef(definition) = &parsed.ast.get(class_tree).kind else {
            continue;
        };
        let class = source_index.symbol_at(source_id, class_tree).unwrap();
        let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
            panic!("class has an ordinary template");
        };
        let constructor = source_index
            .symbol_at(source_id, template.constructor)
            .unwrap();
        let TreeKind::DefDef(constructor_definition) = &parsed.ast.get(template.constructor).kind
        else {
            panic!("primary constructor has a source definition");
        };
        let parameters = constructor_definition
            .value_param_clauses
            .iter()
            .flatten()
            .map(|tree| {
                source_index
                    .derived_symbol_at(constructor, source_id, *tree)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        class_data.push((class_tree, class, constructor, parameters));
    }
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &source_index,
        &mut store,
        definitions,
        &packages,
    );
    for (_, class, constructor, parameters) in &class_data {
        typer.complete_symbol(*class).unwrap();
        typer.complete_symbol(*constructor).unwrap();
        for parameter in parameters {
            typer.complete_symbol(*parameter).unwrap();
        }
    }
    let mut found = None;
    for (class_tree, _, _, _) in &class_data {
        let TreeKind::TypeDef(definition) = &parsed.ast.get(*class_tree).kind else {
            unreachable!();
        };
        let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
            unreachable!();
        };
        for &member in &template.body {
            let TreeKind::DefDef(method_definition) = &parsed.ast.get(member).kind else {
                continue;
            };
            let is_create = typer
                .store()
                .names
                .resolve(method_definition.name.as_name().text())
                == "create";
            let Some(rhs) = method_definition.rhs else {
                continue;
            };
            let TreeKind::Block(block) = &parsed.ast.get(rhs).kind else {
                continue;
            };
            if block.stats.len() < 2 {
                continue;
            }
            let method = source_index.symbol_at(source_id, member).unwrap();
            typer.complete_symbol(method).unwrap();
            let formal_trees = method_definition.value_param_clauses[0].clone();
            let formals = formal_trees
                .iter()
                .map(|tree| source_index.symbol_at(source_id, *tree).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(formals.len(), 2);
            for formal in &formals {
                typer.complete_symbol(*formal).unwrap();
            }
            let context = typer.expression_context_for(method).unwrap();
            let typed_body = typer.type_expression(rhs, context).unwrap();
            let local_declarations = block
                .stats
                .iter()
                .copied()
                .filter(|declaration| {
                    matches!(parsed.ast.get(*declaration).kind, TreeKind::ValDef(_))
                })
                .collect::<Vec<_>>();
            let local_bindings = [
                typer
                    .typed_local_binding_at(source_id, local_declarations[0])
                    .unwrap(),
                typer
                    .typed_local_binding_at(source_id, local_declarations[1])
                    .unwrap(),
            ];
            let all_local_bindings = block
                .stats
                .iter()
                .filter(|declaration| {
                    matches!(parsed.ast.get(**declaration).kind, TreeKind::ValDef(_))
                })
                .map(|declaration| {
                    typer
                        .typed_local_binding_at(source_id, *declaration)
                        .unwrap()
                })
                .collect::<Vec<_>>();
            if is_create || found.is_none() {
                found = Some((
                    member,
                    method,
                    formals,
                    local_bindings,
                    all_local_bindings,
                    typed_body,
                ));
            }
        }
    }
    let (method_tree, method, formals, local_bindings, all_local_bindings, typed_body) =
        found.expect("fixture has one two-local factory method");
    let mut other_local_binding = None;
    for (class_tree, _, _, _) in &class_data {
        let TreeKind::TypeDef(definition) = &parsed.ast.get(*class_tree).kind else {
            unreachable!();
        };
        let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
            unreachable!();
        };
        for &member in &template.body {
            let TreeKind::DefDef(other_definition) = &parsed.ast.get(member).kind else {
                continue;
            };
            if typer
                .store()
                .names
                .resolve(other_definition.name.as_name().text())
                != "other"
            {
                continue;
            }
            let other_rhs = other_definition
                .rhs
                .expect("other method fixture includes a block body");
            let TreeKind::Block(other_block) = &parsed.ast.get(other_rhs).kind else {
                panic!("other method fixture includes a Block");
            };
            assert!(!other_block.stats.is_empty());
            let other_method = source_index.symbol_at(source_id, member).unwrap();
            typer.complete_symbol(other_method).unwrap();
            for formal_tree in other_definition.value_param_clauses.iter().flatten() {
                let formal = source_index.symbol_at(source_id, *formal_tree).unwrap();
                typer.complete_symbol(formal).unwrap();
            }
            let context = typer.expression_context_for(other_method).unwrap();
            typer.type_expression(other_rhs, context).unwrap();
            other_local_binding = Some(
                typer
                    .typed_local_binding_at(source_id, other_block.stats[0])
                    .unwrap(),
            );
        }
    }
    let mut method_plans = Vec::new();
    for (class_tree, class, _, _) in &class_data {
        let TreeKind::TypeDef(definition) = &parsed.ast.get(*class_tree).kind else {
            unreachable!();
        };
        let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
            unreachable!();
        };
        for &member in &template.body {
            if source_index.symbol_at(source_id, member) != Some(method) {
                continue;
            }
            let local_plan = all_local_bindings
                .iter()
                .map(|binding| dotty_jvm::JvmLocalBinding {
                    declaration: binding.declaration(),
                    symbol: binding.symbol(),
                    mutable: typer
                        .store()
                        .symbols
                        .get(binding.symbol())
                        .flags
                        .contains(dotty_core::SymbolFlags::MUTABLE),
                })
                .collect();
            method_plans.push((
                *class,
                JvmMethodPlan {
                    method,
                    parameters: formals.clone(),
                    locals: local_plan,
                    body: Some(typed_body),
                },
            ));
        }
    }
    let classes: Vec<JvmClassPlan> = class_data
        .iter()
        .map(|(_, class, constructor, parameters)| JvmClassPlan {
            class: *class,
            methods: method_plans
                .iter()
                .filter(|(owner, _)| owner == class)
                .map(|(_, plan)| plan.clone())
                .collect(),
            fields: Vec::new(),
            initialization: JvmClassInitializationPlan::PrimaryConstructor(
                JvmPrimaryConstructorPlan::from_source(
                    *class,
                    *constructor,
                    parameters.clone(),
                    &parsed.ast,
                    &source_index,
                )
                .unwrap(),
            ),
        })
        .collect();
    let typed_ast = clone_arena(typer.typed_ast());
    let source_typed_index = typer.source_typed_index().clone();
    let metadata_check = JvmCompilationUnit {
        store: typer.store(),
        typed_ast: typer.typed_ast(),
        classes: classes.clone(),
    };
    metadata_check.metadata(&definitions).unwrap();
    drop(metadata_check);
    let _ = typed_body;
    if retain_typer {
        let jvm_unit = JvmCompilationUnit {
            store: typer.store(),
            typed_ast: typer.typed_ast(),
            classes,
        };
        consume(
            typer.store(),
            &definitions,
            &parsed.ast,
            &typed_ast,
            &source_index,
            &source_typed_index,
            source_id,
            method_tree,
            local_bindings,
            other_local_binding,
            Some(&typer),
            &jvm_unit,
        )
    } else {
        drop(typer);
        mutate_store(&mut store, local_bindings);
        let jvm_unit = JvmCompilationUnit {
            store: &store,
            typed_ast: &typed_ast,
            classes,
        };
        consume(
            &store,
            &definitions,
            &parsed.ast,
            &typed_ast,
            &source_index,
            &source_typed_index,
            source_id,
            method_tree,
            local_bindings,
            other_local_binding,
            None,
            &jvm_unit,
        )
    }
}

fn assert_generic_proof(source: &str, source_number: u32, expected_descriptor: &str) {
    let source_id = SourceId::from_index(source_number);
    let source_text = SourceText::new(source).unwrap();
    let scanner = ContextualScanner::new(source).unwrap();
    assert!(scanner.diagnostics().is_empty());
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = parse_compilation_unit(source_text, source_id, scanner, &mut store.names);
    assert!(parsed.diagnostics.is_empty());
    let mut packages = Packages::new();
    let source_index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        "constructor-proof.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        result => panic!("source names without diagnostics: {result:?}"),
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("fixture has one package root");
    };

    let mut class_data = Vec::new();
    for &class_tree in &package.stats {
        let class = source_index.symbol_at(source_id, class_tree).unwrap();
        let TreeKind::TypeDef(definition) = &parsed.ast.get(class_tree).kind else {
            panic!("batch members are classes");
        };
        let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
            panic!("class has an ordinary template");
        };
        let constructor = source_index
            .symbol_at(source_id, template.constructor)
            .unwrap();
        let TreeKind::DefDef(constructor_definition) = &parsed.ast.get(template.constructor).kind
        else {
            panic!("primary constructor has a source definition");
        };
        let constructor_parameters = constructor_definition
            .value_param_clauses
            .iter()
            .flatten()
            .map(|&tree| {
                source_index
                    .derived_symbol_at(constructor, source_id, tree)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        class_data.push((class_tree, class, constructor, constructor_parameters));
    }

    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &source_index,
        &mut store,
        definitions,
        &packages,
    );
    for (_, class, constructor, parameters) in &class_data {
        typer.complete_symbol(*class).unwrap();
        typer.complete_symbol(*constructor).unwrap();
        for parameter in parameters {
            typer.complete_symbol(*parameter).unwrap();
        }
    }

    let mut methods = Vec::new();
    let mut caller_class = None;
    for (class_tree, class, _, _) in &class_data {
        let TreeKind::TypeDef(definition) = &parsed.ast.get(*class_tree).kind else {
            unreachable!();
        };
        let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
            unreachable!();
        };
        for &member in &template.body {
            let TreeKind::DefDef(method_definition) = &parsed.ast.get(member).kind else {
                continue;
            };
            let Some(rhs) = method_definition.rhs else {
                continue;
            };
            let TreeKind::Apply(application) = &parsed.ast.get(rhs).kind else {
                continue;
            };
            if !matches!(
                parsed.ast.get(application.function).kind,
                TreeKind::Select(selection)
                    if matches!(parsed.ast.get(selection.qualifier).kind, TreeKind::New(_))
            ) {
                continue;
            }
            let method = source_index.symbol_at(source_id, member).unwrap();
            typer.complete_symbol(method).unwrap();
            let parameters = method_definition
                .value_param_clauses
                .iter()
                .flatten()
                .map(|&tree| source_index.symbol_at(source_id, tree).unwrap())
                .collect::<Vec<_>>();
            for parameter in &parameters {
                typer.complete_symbol(*parameter).unwrap();
            }
            let context = typer.expression_context_for(method).unwrap();
            let typed_rhs = typer.type_expression(rhs, context).unwrap();
            methods.push((
                *class,
                JvmMethodPlan {
                    method,
                    parameters,
                    locals: Vec::new(),
                    body: Some(typed_rhs),
                },
            ));
            caller_class = Some(*class);
        }
    }

    let classes = class_data
        .iter()
        .map(|(_class_tree, class, constructor, parameters)| {
            let method_plans = methods
                .iter()
                .filter(|(owner, _)| owner == class)
                .map(|(_, method)| method.clone())
                .collect::<Vec<_>>();
            JvmClassPlan {
                class: *class,
                methods: method_plans,
                fields: Vec::new(),
                initialization: JvmClassInitializationPlan::PrimaryConstructor(
                    JvmPrimaryConstructorPlan::from_source(
                        *class,
                        *constructor,
                        parameters.clone(),
                        &parsed.ast,
                        &source_index,
                    )
                    .unwrap(),
                ),
            }
        })
        .collect::<Vec<_>>();
    let jvm_unit = JvmCompilationUnit {
        store: typer.store(),
        typed_ast: typer.typed_ast(),
        classes,
    };
    jvm_unit.metadata(&definitions).unwrap();
    let input = CrossClassConstructorCallProofInput {
        source_id,
        source_root: parsed.root,
        store: typer.store(),
        definitions: &definitions,
        source_ast: &parsed.ast,
        typed_ast: typer.typed_ast(),
        source_index: &source_index,
        source_typed_index: typer.source_typed_index(),
        jvm_unit: &jvm_unit,
    };
    let proofs = validate_cross_class_constructor_call_proofs(input).unwrap();
    let [proof] = proofs.as_slice() else {
        panic!(
            "one canonical constructor call is proved, found {}",
            proofs.len()
        );
    };
    let caller_class = caller_class.unwrap();
    let caller_plan = class_data
        .iter()
        .find(|(_, class, _, _)| *class == caller_class)
        .unwrap();
    let target_class = proof.target_class();
    assert_eq!(
        proof.argument_proof().kind(),
        dotty::jvm_tasty_cross_class::ConstructorArgumentProofKind::Formal
    );
    let formal = proof
        .argument_proof()
        .formal()
        .expect("direct source formal has an explicit proof variant");
    assert_eq!(formal.formal(), proof.method_formal());
    assert_eq!(formal.source_argument(), proof.source_argument());
    assert_eq!(formal.typed_argument(), proof.typed_argument());
    assert_eq!(formal.formal_type(), proof.formal_type());
    assert_eq!(formal.result_type(), formal.formal_type());
    assert_ne!(proof.caller_class(), target_class);
    assert_eq!(proof.caller_class(), caller_plan.1);
    let formal_class = proof
        .formal_class()
        .expect("the source formal is a planned reference class");
    assert_ne!(formal_class, target_class);
    assert_eq!(proof.method_descriptor(), expected_descriptor);
    let (expected_parameters, _) = expected_descriptor.split_once(')').unwrap();
    let expected_constructor_descriptor = format!("{expected_parameters})V");
    assert_eq!(
        proof.constructor_descriptor(),
        expected_constructor_descriptor
    );
    assert_eq!(
        typer
            .source_typed_index()
            .get(source_id, proof.source_rhs()),
        Some(proof.typed_rhs())
    );
    assert_eq!(
        typer
            .source_typed_index()
            .get(source_id, proof.source_argument()),
        Some(proof.typed_argument())
    );
    assert!(matches!(
        typer.store().types.get(formal.result_type()),
        Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == formal_class
    ));
    assert!(matches!(
        typer.store().types.get(proof.result_type()),
        Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == target_class
    ));
    let (proof_store, _, proof_typed_ast, _, proof_typed_index, proof_jvm_unit) =
        proof.session_inputs();
    assert!(std::ptr::eq(proof_store, typer.store()));
    assert!(std::ptr::eq(proof_jvm_unit, &jvm_unit));
    assert_eq!(
        proof_store.symbols.get(proof.method()).owner,
        Some(proof.caller_class())
    );
    assert_eq!(
        proof_store.symbols.get(proof.method_formal()).owner,
        Some(proof.method())
    );
    assert_eq!(
        proof_store.symbols.get(proof.constructor()).owner,
        Some(proof.target_class())
    );
    assert_eq!(
        proof_typed_index.get(source_id, proof.source_select()),
        Some(proof.typed_select())
    );
    assert_eq!(
        proof_typed_index.get(source_id, proof.source_new()),
        Some(proof.typed_new())
    );
    assert!(matches!(
        proof_store
            .types
            .get(proof_typed_ast.get(proof.typed_select()).ty),
        Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
            if *symbol == proof.constructor()
    ));

    let wrong_source = CrossClassConstructorCallProofInput {
        source_id: SourceId::from_index(source_number.saturating_add(1)),
        source_root: parsed.root,
        store: typer.store(),
        definitions: &definitions,
        source_ast: &parsed.ast,
        typed_ast: typer.typed_ast(),
        source_index: &source_index,
        source_typed_index: typer.source_typed_index(),
        jvm_unit: &jvm_unit,
    };
    assert!(validate_cross_class_constructor_call_proofs(wrong_source).is_err());

    let without_target = JvmCompilationUnit {
        store: jvm_unit.store,
        typed_ast: jvm_unit.typed_ast,
        classes: jvm_unit
            .classes
            .iter()
            .filter(|plan| plan.class != target_class)
            .cloned()
            .collect(),
    };
    let missing_target = CrossClassConstructorCallProofInput {
        source_id,
        source_root: parsed.root,
        store: typer.store(),
        definitions: &definitions,
        source_ast: &parsed.ast,
        typed_ast: typer.typed_ast(),
        source_index: &source_index,
        source_typed_index: typer.source_typed_index(),
        jvm_unit: &without_target,
    };
    assert!(validate_cross_class_constructor_call_proofs(missing_target).is_err());

    let mut missing_method = JvmCompilationUnit {
        store: jvm_unit.store,
        typed_ast: jvm_unit.typed_ast,
        classes: jvm_unit.classes.clone(),
    };
    missing_method
        .classes
        .iter_mut()
        .find(|plan| plan.class == proof.caller_class())
        .unwrap()
        .methods
        .clear();
    let missing_caller_method = CrossClassConstructorCallProofInput {
        source_id,
        source_root: parsed.root,
        store: typer.store(),
        definitions: &definitions,
        source_ast: &parsed.ast,
        typed_ast: typer.typed_ast(),
        source_index: &source_index,
        source_typed_index: typer.source_typed_index(),
        jvm_unit: &missing_method,
    };
    assert!(validate_cross_class_constructor_call_proofs(missing_caller_method).is_err());

    let mut stale_bindings = typer.source_typed_index().clone();
    stale_bindings.remove(source_id, proof.source_rhs());
    let stale_rhs = CrossClassConstructorCallProofInput {
        source_id,
        source_root: parsed.root,
        store: typer.store(),
        definitions: &definitions,
        source_ast: &parsed.ast,
        typed_ast: typer.typed_ast(),
        source_index: &source_index,
        source_typed_index: &stale_bindings,
        jvm_unit: &jvm_unit,
    };
    assert!(validate_cross_class_constructor_call_proofs(stale_rhs).is_err());

    let mut mismatched_bindings = typer.source_typed_index().clone();
    mismatched_bindings.remove(source_id, proof.source_argument());
    mismatched_bindings
        .insert(source_id, proof.source_argument(), proof.typed_rhs())
        .unwrap();
    let mismatched_argument = CrossClassConstructorCallProofInput {
        source_id,
        source_root: parsed.root,
        store: typer.store(),
        definitions: &definitions,
        source_ast: &parsed.ast,
        typed_ast: typer.typed_ast(),
        source_index: &source_index,
        source_typed_index: &mismatched_bindings,
        jvm_unit: &jvm_unit,
    };
    assert!(validate_cross_class_constructor_call_proofs(mismatched_argument).is_err());

    let caller_method_tree = match source_index.definition_of(proof.method()) {
        Some(SourceDefinition::Canonical { source, tree }) if source == source_id => tree,
        _ => panic!("proof method retains its source owner"),
    };
    let mut foreign_source_positions = clone_arena(&parsed.ast);
    let source_position = foreign_source_positions
        .get(caller_method_tree)
        .position
        .unwrap();
    foreign_source_positions
        .get_mut(caller_method_tree)
        .position = Some(SourceSpan::new(
        SourceId::from_index(source_number.saturating_add(100)),
        source_position.span(),
    ));
    let foreign_method_origin = CrossClassConstructorCallProofInput {
        source_id,
        source_root: parsed.root,
        store: typer.store(),
        definitions: &definitions,
        source_ast: &foreign_source_positions,
        typed_ast: typer.typed_ast(),
        source_index: &source_index,
        source_typed_index: typer.source_typed_index(),
        jvm_unit: &jvm_unit,
    };
    assert!(matches!(
        validate_cross_class_constructor_call_proofs(foreign_method_origin),
        Err(dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceIdentity(_))
    ));

    let target_constructor_tree = match source_index.definition_of(proof.constructor()) {
        Some(SourceDefinition::Canonical { source, tree }) if source == source_id => tree,
        _ => panic!("proof constructor retains its source owner"),
    };
    let mut foreign_target_positions = clone_arena(&parsed.ast);
    let constructor_position = foreign_target_positions
        .get(target_constructor_tree)
        .position
        .unwrap();
    foreign_target_positions
        .get_mut(target_constructor_tree)
        .position = Some(SourceSpan::new(
        SourceId::from_index(source_number.saturating_add(101)),
        constructor_position.span(),
    ));
    let foreign_target_origin = CrossClassConstructorCallProofInput {
        source_id,
        source_root: parsed.root,
        store: typer.store(),
        definitions: &definitions,
        source_ast: &foreign_target_positions,
        typed_ast: typer.typed_ast(),
        source_index: &source_index,
        source_typed_index: typer.source_typed_index(),
        jvm_unit: &jvm_unit,
    };
    assert!(matches!(
        validate_cross_class_constructor_call_proofs(foreign_target_origin),
        Err(dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceIdentity(_))
    ));

    let target_source_tree = match source_index.definition_of(proof.target_class()) {
        Some(SourceDefinition::Canonical { source, tree }) if source == source_id => tree,
        _ => panic!("proof target retains its source owner"),
    };
    let mut stale_target_template_ast = clone_arena(&parsed.ast);
    let mut other_arena = clone_arena(&parsed.ast);
    let stale_tree_id = other_arena.alloc(parsed.ast.get(parsed.root).clone());
    let TreeKind::TypeDef(target_definition) =
        &mut stale_target_template_ast.get_mut(target_source_tree).kind
    else {
        panic!("proof target is a source class");
    };
    target_definition.rhs = stale_tree_id;
    let stale_target_template = CrossClassConstructorCallProofInput {
        source_id,
        source_root: parsed.root,
        store: typer.store(),
        definitions: &definitions,
        source_ast: &stale_target_template_ast,
        typed_ast: typer.typed_ast(),
        source_index: &source_index,
        source_typed_index: typer.source_typed_index(),
        jvm_unit: &jvm_unit,
    };
    assert!(validate_cross_class_constructor_call_proofs(stale_target_template).is_err());

    let mut positionless_typed_ast = clone_arena(typer.typed_ast());
    positionless_typed_ast.get_mut(proof.typed_new()).position = None;
    let positionless_unit = JvmCompilationUnit {
        store: jvm_unit.store,
        typed_ast: &positionless_typed_ast,
        classes: jvm_unit.classes.clone(),
    };
    let positionless_typed_tree = CrossClassConstructorCallProofInput {
        source_id,
        source_root: parsed.root,
        store: typer.store(),
        definitions: &definitions,
        source_ast: &parsed.ast,
        typed_ast: &positionless_typed_ast,
        source_index: &source_index,
        source_typed_index: typer.source_typed_index(),
        jvm_unit: &positionless_unit,
    };
    assert!(matches!(
        validate_cross_class_constructor_call_proofs(positionless_typed_tree),
        Err(dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::TypedBinding(_))
    ));

    let method_formal_tree = match source_index.definition_of(proof.method_formal()) {
        Some(SourceDefinition::Canonical { source, tree }) if source == source_id => tree,
        _ => panic!("proof formal retains its source owner"),
    };
    let TreeKind::ValDef(formal_definition) = &parsed.ast.get(method_formal_tree).kind else {
        panic!("method formal retains a ValDef source tree");
    };
    let mut extended_typed_ast = clone_arena(typer.typed_ast());
    let stale_typed_tree =
        extended_typed_ast.alloc(typer.typed_ast().get(proof.typed_rhs()).clone());
    let mut stale_formal_annotation = typer.source_typed_index().clone();
    stale_formal_annotation
        .insert(source_id, formal_definition.tpt, stale_typed_tree)
        .unwrap();
    let stale_annotation = CrossClassConstructorCallProofInput {
        source_id,
        source_root: parsed.root,
        store: typer.store(),
        definitions: &definitions,
        source_ast: &parsed.ast,
        typed_ast: typer.typed_ast(),
        source_index: &source_index,
        source_typed_index: &stale_formal_annotation,
        jvm_unit: &jvm_unit,
    };
    assert!(matches!(
        validate_cross_class_constructor_call_proofs(stale_annotation),
        Err(dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::TypedBinding(_))
    ));

    let caller_method = proof.method();
    let typed_ast_snapshot = clone_arena(typer.typed_ast());
    let source_typed_snapshot = typer.source_typed_index().clone();
    let class_snapshot = jvm_unit.classes.clone();
    drop(proofs);
    drop(jvm_unit);
    drop(typer);
    store.symbols.get_mut(caller_method).origin =
        SymbolOrigin::Source(SourceId::from_index(source_number.saturating_add(102)));
    let wrong_symbol_origin_unit = JvmCompilationUnit {
        store: &store,
        typed_ast: &typed_ast_snapshot,
        classes: class_snapshot,
    };
    let wrong_symbol_origin = CrossClassConstructorCallProofInput {
        source_id,
        source_root: parsed.root,
        store: &store,
        definitions: &definitions,
        source_ast: &parsed.ast,
        typed_ast: &typed_ast_snapshot,
        source_index: &source_index,
        source_typed_index: &source_typed_snapshot,
        jvm_unit: &wrong_symbol_origin_unit,
    };
    assert!(matches!(
        validate_cross_class_constructor_call_proofs(wrong_symbol_origin),
        Err(dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceIdentity(_))
    ));
}

fn with_two_argument_proof<R>(
    source: &str,
    source_number: u32,
    consume: impl for<'session> FnOnce(
        &ValidatedCrossClassConstructorCallProof<'session>,
        &SemanticStore,
        &AstArena<dotty_core::ast::Untyped>,
        &dotty_core::ast::TypedAst,
        &dotty_core::SourceSemanticIndex,
        &dotty_core::SourceTypedIndex,
        SourceId,
        dotty_core::TreeId<dotty_core::ast::Untyped>,
        &Definitions,
        &JvmCompilationUnit<'session>,
    ) -> R,
) -> R {
    let source_id = SourceId::from_index(source_number);
    let source_text = SourceText::new(source).unwrap();
    let scanner = ContextualScanner::new(source).unwrap();
    assert!(scanner.diagnostics().is_empty());
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = parse_compilation_unit(source_text, source_id, scanner, &mut store.names);
    assert!(parsed.diagnostics.is_empty());
    let mut packages = Packages::new();
    let source_index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        "two-argument-constructor-proof.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        result => panic!("source names without diagnostics: {result:?}"),
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("source has one package root");
    };

    let mut class_data = Vec::new();
    for &class_tree in &package.stats {
        let class = source_index.symbol_at(source_id, class_tree).unwrap();
        let TreeKind::TypeDef(definition) = &parsed.ast.get(class_tree).kind else {
            panic!("batch members are classes");
        };
        let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
            panic!("class has an ordinary template");
        };
        let constructor = source_index
            .symbol_at(source_id, template.constructor)
            .unwrap();
        let TreeKind::DefDef(constructor_definition) = &parsed.ast.get(template.constructor).kind
        else {
            panic!("primary constructor has a source definition");
        };
        let constructor_parameters = constructor_definition
            .value_param_clauses
            .iter()
            .flatten()
            .map(|&tree| {
                source_index
                    .derived_symbol_at(constructor, source_id, tree)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        class_data.push((class_tree, class, constructor, constructor_parameters));
    }

    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &source_index,
        &mut store,
        definitions,
        &packages,
    );
    for (_, class, constructor, parameters) in &class_data {
        typer.complete_symbol(*class).unwrap();
        typer.complete_symbol(*constructor).unwrap();
        for parameter in parameters {
            typer.complete_symbol(*parameter).unwrap();
        }
    }
    let mut method_plans = Vec::new();
    let mut factory_class = None;
    for (class_tree, class, _, _) in &class_data {
        let TreeKind::TypeDef(definition) = &parsed.ast.get(*class_tree).kind else {
            unreachable!();
        };
        let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
            unreachable!();
        };
        for &member in &template.body {
            let TreeKind::DefDef(method_definition) = &parsed.ast.get(member).kind else {
                continue;
            };
            let Some(rhs) = method_definition.rhs else {
                continue;
            };
            let candidate = match &parsed.ast.get(rhs).kind {
                TreeKind::Apply(_) => true,
                TreeKind::Block(block) => {
                    matches!(parsed.ast.get(block.expr).kind, TreeKind::Apply(_))
                }
                _ => false,
            };
            if !candidate {
                continue;
            }
            let method = source_index.symbol_at(source_id, member).unwrap();
            typer.complete_symbol(method).unwrap();
            let parameters = method_definition
                .value_param_clauses
                .iter()
                .flatten()
                .map(|&tree| source_index.symbol_at(source_id, tree).unwrap())
                .collect::<Vec<_>>();
            for parameter in &parameters {
                typer.complete_symbol(*parameter).unwrap();
            }
            let context = typer.expression_context_for(method).unwrap();
            let typed_rhs = typer.type_expression(rhs, context).unwrap();
            let locals = match &parsed.ast.get(rhs).kind {
                TreeKind::Block(block) => block
                    .stats
                    .iter()
                    .filter_map(|declaration| {
                        typer
                            .typed_local_binding_at(source_id, *declaration)
                            .map(|binding| dotty_jvm::JvmLocalBinding {
                                declaration: binding.declaration(),
                                symbol: binding.symbol(),
                                mutable: false,
                            })
                    })
                    .collect(),
                _ => Vec::new(),
            };
            method_plans.push((
                *class,
                JvmMethodPlan {
                    method,
                    parameters,
                    locals,
                    body: Some(typed_rhs),
                },
            ));
            factory_class = Some(*class);
        }
    }
    assert!(factory_class.is_some());
    let classes = class_data
        .iter()
        .map(|(_, class, constructor, parameters)| JvmClassPlan {
            class: *class,
            methods: method_plans
                .iter()
                .filter(|(owner, _)| owner == class)
                .map(|(_, method)| method.clone())
                .collect(),
            fields: Vec::new(),
            initialization: JvmClassInitializationPlan::PrimaryConstructor(
                JvmPrimaryConstructorPlan::from_source(
                    *class,
                    *constructor,
                    parameters.clone(),
                    &parsed.ast,
                    &source_index,
                )
                .unwrap(),
            ),
        })
        .collect();
    let jvm_unit = JvmCompilationUnit {
        store: typer.store(),
        typed_ast: typer.typed_ast(),
        classes,
    };
    jvm_unit.metadata(&definitions).unwrap();
    let proofs =
        dotty::jvm_tasty_cross_class::validate_cross_class_constructor_call_proofs_with_typer(
            CrossClassConstructorCallProofInput {
                source_id,
                source_root: parsed.root,
                store: typer.store(),
                definitions: &definitions,
                source_ast: &parsed.ast,
                typed_ast: typer.typed_ast(),
                source_index: &source_index,
                source_typed_index: typer.source_typed_index(),
                jvm_unit: &jvm_unit,
            },
            &typer,
        )
        .unwrap();
    if let Some(pair) = proofs[0].argument_proof().ordered_pair() {
        let arguments = pair.arguments();
        if arguments
            .iter()
            .any(|argument| argument.local_val().is_some())
        {
            let mut missing_local_edge = typer.source_typed_index().clone();
            let local = arguments
                .iter()
                .find_map(|argument| argument.local_val())
                .unwrap();
            assert_eq!(
                missing_local_edge.remove(source_id, local.source_use()),
                Some(local.typed_use())
            );
            let method_tree = match source_index.definition_of(proofs[0].method()) {
                Some(SourceDefinition::Canonical { source, tree }) if source == source_id => tree,
                _ => panic!("local proof method retains its original source declaration"),
            };
            let missing_result = validate_immutable_local_initialization_proof(
                ImmutableLocalInitializationProofInput {
                    source_id,
                    method_tree,
                    store: typer.store(),
                    definitions: &definitions,
                    source_ast: &parsed.ast,
                    typed_ast: typer.typed_ast(),
                    source_index: &source_index,
                    source_typed_index: &missing_local_edge,
                    local_binding: typer
                        .typed_local_binding_at(source_id, local.source_declaration()),
                    jvm_unit: &jvm_unit,
                },
            );
            assert_eq!(
                missing_result.err().unwrap(),
                dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::TypedBinding(
                    "final local Ident has no original typed mapping"
                )
            );

            let [first, second] = arguments;
            let missing_argument_result = validate_ordered_constructor_argument_bindings(
                &missing_local_edge,
                source_id,
                [first.source_argument(), second.source_argument()],
                [first.typed_argument(), second.typed_argument()],
            );
            assert_eq!(
                missing_argument_result.unwrap_err(),
                dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::TypedBinding(
                    if first.source_argument() == local.source_use() {
                        "first outer argument has no exact typed binding"
                    } else {
                        "second outer argument has no exact typed binding"
                    }
                )
            );
            let mut swapped_edges = typer.source_typed_index().clone();
            swapped_edges.remove(source_id, first.source_argument());
            swapped_edges.remove(source_id, second.source_argument());
            swapped_edges
                .insert(source_id, first.source_argument(), second.typed_argument())
                .unwrap();
            swapped_edges
                .insert(source_id, second.source_argument(), first.typed_argument())
                .unwrap();
            assert_eq!(
                validate_ordered_constructor_argument_bindings(
                    &swapped_edges,
                    source_id,
                    [first.source_argument(), second.source_argument()],
                    [first.typed_argument(), second.typed_argument()],
                )
                .unwrap_err(),
                dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::TypedBinding(
                    "typed outer Apply must preserve exact ordered source mappings"
                )
            );

            let foreign_index = typer.source_typed_index().clone();
            let foreign_index_result =
                dotty::jvm_tasty_cross_class::validate_cross_class_constructor_call_proofs_with_typer(
                    CrossClassConstructorCallProofInput {
                        source_id,
                        source_root: parsed.root,
                        store: typer.store(),
                        definitions: &definitions,
                        source_ast: &parsed.ast,
                        typed_ast: typer.typed_ast(),
                        source_index: &source_index,
                        source_typed_index: &foreign_index,
                        jvm_unit: &jvm_unit,
                    },
                    &typer,
                );
            assert_eq!(
                foreign_index_result.err().unwrap(),
                dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::TypedBinding(
                    "local binding resolver must borrow the exact proof session stores and indexes"
                )
            );
            let mut two_local_source = clone_arena(&parsed.ast);
            let rhs = match &two_local_source.get(method_tree).kind {
                TreeKind::DefDef(definition) => definition.rhs.unwrap(),
                _ => panic!("factory method remains a method"),
            };
            let TreeKind::Block(block) = &mut two_local_source.get_mut(rhs).kind else {
                panic!("factory source remains the original Block")
            };
            block.stats.push(block.stats[0]);
            let two_local_result =
                dotty::jvm_tasty_cross_class::validate_cross_class_constructor_call_proofs_with_typer(
                    CrossClassConstructorCallProofInput {
                        source_id,
                        source_root: parsed.root,
                        store: typer.store(),
                        definitions: &definitions,
                        source_ast: &two_local_source,
                        typed_ast: typer.typed_ast(),
                        source_index: &source_index,
                        source_typed_index: typer.source_typed_index(),
                        jvm_unit: &jvm_unit,
                    },
                    &typer,
                );
            assert!(matches!(
                two_local_result,
                Err(
                    dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SemanticIdentity(
                        "factory signature must preserve its two canonical Int formals"
                    )
                )
            ));
        }
    }
    assert_eq!(proofs.len(), 1, "one source factory body has one proof");
    let ownership_probe = proofs[0]
        .argument_proof()
        .ordered_pair()
        .and_then(|pair| {
            pair.arguments()
                .iter()
                .find_map(|argument| argument.local_val())
        })
        .map(|local| {
            let method = proofs[0].method();
            let method_tree = match source_index.definition_of(method) {
                Some(SourceDefinition::Canonical { source, tree }) if source == source_id => tree,
                _ => panic!("local proof method retains its original source declaration"),
            };
            let binding = typer
                .typed_local_binding_at(source_id, local.source_declaration())
                .expect("proof local retains its canonical typed binding");
            let local_symbol = binding.symbol();
            let pair_class = typer
                .store()
                .symbols
                .get(proofs[0].constructor())
                .owner
                .expect("proof constructor has its canonical class owner");
            assert_ne!(pair_class, method);
            assert_ne!(pair_class, local_symbol);
            (
                method,
                method_tree,
                binding,
                pair_class,
                clone_arena(typer.typed_ast()),
                typer.source_typed_index().clone(),
                jvm_unit.classes.clone(),
            )
        });
    let result = consume(
        &proofs[0],
        typer.store(),
        &parsed.ast,
        typer.typed_ast(),
        &source_index,
        typer.source_typed_index(),
        source_id,
        parsed.root,
        &definitions,
        &jvm_unit,
    );

    drop(proofs);
    drop(jvm_unit);
    drop(typer);

    if let Some((
        method,
        method_tree,
        binding,
        pair_class,
        typed_ast,
        source_typed_index,
        classes,
    )) = ownership_probe
    {
        let local_symbol = binding.symbol();
        let local_scope = binding.scope();
        assert_eq!(store.symbols.get(local_symbol).owner, Some(method));
        assert_eq!(store.scopes.get(local_scope).owner, Some(method));

        // Keep the canonical source/typed roots and valid references intact;
        // corrupt only each owner edge before the authoritative proof check.
        let original_symbol_owner = store.symbols.get(local_symbol).owner;
        store.symbols.get_mut(local_symbol).owner = Some(pair_class);
        assert_eq!(store.symbols.get(local_symbol).owner, Some(pair_class));
        let wrong_symbol_owner_unit = JvmCompilationUnit {
            store: &store,
            typed_ast: &typed_ast,
            classes: classes.clone(),
        };
        let wrong_symbol_owner =
            validate_immutable_local_initialization_proof(ImmutableLocalInitializationProofInput {
                source_id,
                method_tree,
                store: &store,
                definitions: &definitions,
                source_ast: &parsed.ast,
                typed_ast: &typed_ast,
                source_index: &source_index,
                source_typed_index: &source_typed_index,
                local_binding: Some(binding),
                jvm_unit: &wrong_symbol_owner_unit,
            });
        assert_eq!(
            wrong_symbol_owner
                .err()
                .expect("wrong symbol owner is rejected"),
            dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceIdentity(
                "local must be the original immutable method-owned local"
            )
        );
        store.symbols.get_mut(local_symbol).owner = original_symbol_owner;

        let original_scope_owner = store.scopes.get(local_scope).owner;
        store.scopes.get_mut(local_scope).owner = Some(pair_class);
        assert_eq!(store.scopes.get(local_scope).owner, Some(pair_class));
        let wrong_scope_owner_unit = JvmCompilationUnit {
            store: &store,
            typed_ast: &typed_ast,
            classes,
        };
        let wrong_scope_owner =
            validate_immutable_local_initialization_proof(ImmutableLocalInitializationProofInput {
                source_id,
                method_tree,
                store: &store,
                definitions: &definitions,
                source_ast: &parsed.ast,
                typed_ast: &typed_ast,
                source_index: &source_index,
                source_typed_index: &source_typed_index,
                local_binding: Some(binding),
                jvm_unit: &wrong_scope_owner_unit,
            });
        assert_eq!(
            wrong_scope_owner
                .err()
                .expect("wrong scope owner is rejected"),
            dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceIdentity(
                "local must occupy its exact single-entry method-owned Block scope"
            )
        );
        store.scopes.get_mut(local_scope).owner = original_scope_owner;
    }

    result
}

struct PairReceiptContext<'a> {
    store: &'a SemanticStore,
    source_ast: &'a AstArena<dotty_core::ast::Untyped>,
    typed_ast: &'a dotty_core::ast::TypedAst,
    source_index: &'a dotty_core::SourceSemanticIndex,
    source_typed_index: &'a dotty_core::SourceTypedIndex,
    source_id: SourceId,
}

fn assert_ordered_pair_receipt(
    proof: &ValidatedCrossClassConstructorCallProof<'_>,
    context: PairReceiptContext<'_>,
    expected_formals: [&str; 2],
    expected_method_descriptor: &str,
    expected_constructor_descriptor: &str,
) {
    use dotty::jvm_tasty_cross_class::ConstructorArgumentProofKind;
    let PairReceiptContext {
        store,
        source_ast,
        typed_ast,
        source_index,
        source_typed_index,
        source_id,
    } = context;

    assert_eq!(
        proof.argument_proof().kind(),
        ConstructorArgumentProofKind::NestedNewPair
    );
    assert_eq!(proof.method_descriptor(), expected_method_descriptor);
    assert_eq!(
        proof.constructor_descriptor(),
        expected_constructor_descriptor
    );
    let [first, second] = proof.nested_argument_pair().unwrap();
    let (proof_store, proof_source, proof_typed, _proof_index, proof_bindings, _proof_jvm) =
        proof.session_inputs();
    assert!(std::ptr::eq(proof_store, store));
    assert!(std::ptr::eq(proof_source, source_ast));
    assert!(std::ptr::eq(proof_typed, typed_ast));
    assert!(std::ptr::eq(proof_bindings, source_typed_index));
    let receipts = [first, second];
    assert_eq!(first.class(), second.class());
    assert_eq!(first.constructor(), second.constructor());
    assert_ne!(
        first.source_outer_argument(),
        second.source_outer_argument()
    );
    assert_ne!(first.source_apply(), second.source_apply());
    assert_ne!(first.source_select(), second.source_select());
    assert_ne!(first.source_new(), second.source_new());
    assert_ne!(first.source_argument(), second.source_argument());
    assert_ne!(first.typed_outer_argument(), second.typed_outer_argument());
    assert_ne!(first.typed_apply(), second.typed_apply());
    assert_ne!(first.typed_select(), second.typed_select());
    assert_ne!(first.typed_new(), second.typed_new());
    assert_ne!(first.typed_argument(), second.typed_argument());
    assert_eq!(proof.source_argument(), first.source_outer_argument());
    assert_eq!(proof.typed_argument(), first.typed_outer_argument());

    let method_tree = match source_index.definition_of(proof.method()) {
        Some(SourceDefinition::Canonical { source, tree }) if source == source_id => tree,
        _ => panic!("factory method has its original source definition"),
    };
    let TreeKind::DefDef(method) = &source_ast.get(method_tree).kind else {
        panic!("factory method definition is DefDef");
    };
    let method_formals = method.value_param_clauses[0]
        .iter()
        .map(|tree| source_index.symbol_at(source_id, *tree).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(method_formals.len(), 2);
    let pair_context = proof.argument_proof().nested_pair_context().unwrap();
    assert_eq!(
        pair_context.source_method_formals(),
        [
            method.value_param_clauses[0][0],
            method.value_param_clauses[0][1]
        ]
    );
    assert_eq!(
        pair_context.method_formals(),
        [method_formals[0], method_formals[1]]
    );
    for (index, source_parameter) in pair_context.source_method_formals().into_iter().enumerate() {
        assert_eq!(
            source_index.symbol_at(source_id, source_parameter),
            Some(pair_context.method_formals()[index])
        );
        let dotty_core::SymbolInfo::Complete(formal_type) =
            store.symbols.get(pair_context.method_formals()[index]).info
        else {
            panic!("each outer formal type is complete");
        };
        assert_eq!(formal_type, pair_context.method_formal_types()[index]);
        assert_eq!(
            source_ast.get(source_parameter).position.unwrap().source(),
            source_id
        );
    }
    let TreeKind::Apply(outer_apply) = &source_ast.get(proof.source_rhs()).kind else {
        panic!("factory body is the original outer Apply");
    };
    assert_eq!(
        outer_apply.args,
        [
            first.source_outer_argument(),
            second.source_outer_argument()
        ]
    );
    let outer_type = store.symbols.get(proof.constructor()).info;
    let dotty_core::SymbolInfo::Complete(outer_type) = outer_type else {
        panic!("Pair constructor signature is complete");
    };
    let Type::Method(outer_signature) = store.types.get(outer_type) else {
        panic!("Pair constructor has one method type");
    };
    assert_eq!(outer_signature.params.len(), 2);
    let constructor_tree = match source_index.definition_of(proof.constructor()) {
        Some(SourceDefinition::Canonical { source, tree }) if source == source_id => tree,
        _ => panic!("Pair constructor retains its canonical source definition"),
    };
    let TreeKind::DefDef(constructor_definition) = &source_ast.get(constructor_tree).kind else {
        panic!("Pair primary constructor is a source DefDef");
    };
    let constructor_formal_trees = [
        constructor_definition.value_param_clauses[0][0],
        constructor_definition.value_param_clauses[0][1],
    ];
    assert_eq!(
        pair_context.source_constructor_formals(),
        constructor_formal_trees
    );
    for (index, constructor_formal_tree) in constructor_formal_trees.iter().copied().enumerate() {
        assert_eq!(
            source_index.derived_symbol_at(proof.constructor(), source_id, constructor_formal_tree),
            Some(pair_context.constructor_formals()[index])
        );
        assert_eq!(
            pair_context.constructor_formal_types()[index],
            outer_signature.params[index].ty
        );
        assert_eq!(
            source_ast
                .get(constructor_formal_tree)
                .position
                .unwrap()
                .source(),
            source_id
        );
    }

    for (index, receipt) in receipts.into_iter().enumerate() {
        assert_eq!(
            store
                .names
                .resolve(store.symbols.get(receipt.argument_formal()).name.text()),
            expected_formals[index]
        );
        assert!(method_formals.contains(&receipt.argument_formal()));
        let dotty_core::SymbolInfo::Complete(formal_type) =
            store.symbols.get(receipt.argument_formal()).info
        else {
            panic!("the original caller formal type is complete");
        };
        assert_eq!(receipt.argument_formal_type(), formal_type);
        assert_eq!(
            source_typed_index.get(source_id, receipt.source_outer_argument()),
            Some(receipt.typed_outer_argument())
        );
        assert_eq!(
            source_typed_index.get(source_id, receipt.source_apply()),
            Some(receipt.typed_apply())
        );
        assert_eq!(
            source_typed_index.get(source_id, receipt.source_select()),
            Some(receipt.typed_select())
        );
        assert_eq!(
            source_typed_index.get(source_id, receipt.source_new()),
            Some(receipt.typed_new())
        );
        assert_eq!(
            source_typed_index.get(source_id, receipt.source_argument()),
            Some(receipt.typed_argument())
        );
        for (source, typed) in [
            (
                receipt.source_outer_argument(),
                receipt.typed_outer_argument(),
            ),
            (receipt.source_apply(), receipt.typed_apply()),
            (receipt.source_select(), receipt.typed_select()),
            (receipt.source_new(), receipt.typed_new()),
            (receipt.source_argument(), receipt.typed_argument()),
        ] {
            assert_eq!(
                source_ast.get(source).position,
                typed_ast.get(typed).position
            );
        }
        assert!(matches!(
            store.types.get(typed_ast.get(receipt.typed_argument()).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == receipt.argument_formal()
        ));
    }
}

#[test]
fn two_original_immutable_local_initializers_prove_atomically_in_one_block() {
    let renamed = TWO_IMMUTABLE_LOCALS_SOURCE
        .replace("PairFactory", "Assembler")
        .replace("create", "assemble")
        .replace("first", "left")
        .replace("second", "right");
    let counter_at = TWO_IMMUTABLE_LOCALS_SOURCE.find("class Counter(").unwrap();
    let pair_at = TWO_IMMUTABLE_LOCALS_SOURCE.find("class Pair(").unwrap();
    let factory_at = TWO_IMMUTABLE_LOCALS_SOURCE
        .find("class PairFactory:")
        .unwrap();
    let reordered = format!(
        "{}{}{}{}",
        &TWO_IMMUTABLE_LOCALS_SOURCE[..counter_at],
        &TWO_IMMUTABLE_LOCALS_SOURCE[factory_at..],
        &TWO_IMMUTABLE_LOCALS_SOURCE[pair_at..factory_at],
        &TWO_IMMUTABLE_LOCALS_SOURCE[counter_at..pair_at],
    );
    let reversed_declarations = TWO_IMMUTABLE_LOCALS_SOURCE.replace(
        "    val first: Counter = new Counter(firstSeed)\n    val second: Counter = new Counter(secondSeed)",
        "    val second: Counter = new Counter(secondSeed)\n    val first: Counter = new Counter(firstSeed)",
    );
    let swapped_initializers = TWO_IMMUTABLE_LOCALS_SOURCE
        .replace("new Counter(firstSeed)", "new Counter($swap)")
        .replace("new Counter(secondSeed)", "new Counter(firstSeed)")
        .replace("new Counter($swap)", "new Counter(secondSeed)");
    for (source, source_number) in [
        (TWO_IMMUTABLE_LOCALS_SOURCE, 2123),
        (TWO_IMMUTABLE_LOCALS_REVERSED_SOURCE, 2124),
        (renamed.as_str(), 2125),
        (reordered.as_str(), 2130),
        (reversed_declarations.as_str(), 2134),
        (swapped_initializers.as_str(), 2136),
    ] {
        with_two_local_proof_session(
            source,
            source_number,
            |store,
             definitions,
             source_ast,
             typed_ast,
             source_index,
             source_typed_index,
             source_id,
             method_tree,
             bindings,
             _other_local_binding,
             _typer,
             jvm_unit| {
                let proof = validate_immutable_local_initializer_pair_proof(
                    ImmutableLocalInitializerPairProofInput {
                        source_id,
                        method_tree,
                        store,
                        definitions,
                        source_ast,
                        typed_ast,
                        source_index,
                        source_typed_index,
                        local_bindings: [Some(bindings[0]), Some(bindings[1])],
                        jvm_unit,
                    },
                )
                .unwrap();
                let TreeKind::DefDef(method) = &source_ast.get(method_tree).kind else {
                    panic!("proof root is the original factory method");
                };
                let source_block = method.rhs.unwrap();
                let TreeKind::Block(block) = &source_ast.get(source_block).kind else {
                    panic!("method body remains the original Block");
                };
                assert_eq!(proof.source_id(), source_id);
                assert_eq!(proof.source_block(), source_block);
                assert_eq!(
                    proof.typed_block(),
                    source_typed_index.get(source_id, source_block).unwrap()
                );
                assert_eq!(proof.source_final_expression(), block.expr);
                assert_eq!(
                    proof.typed_final_expression(),
                    source_typed_index.get(source_id, block.expr).unwrap()
                );
                let receipts = proof.initializers();
                let counter = proof.counter();
                assert_ne!(counter, proof.method());
                assert_ne!(receipts[0].symbol(), receipts[1].symbol());
                assert_eq!(receipts[0].source_declaration(), block.stats[0]);
                assert_eq!(receipts[1].source_declaration(), block.stats[1]);
                assert_eq!(
                    receipts.map(|receipt| receipt.typed_declaration()),
                    [bindings[0].declaration(), bindings[1].declaration()]
                );
                assert_eq!(receipts[0].scope(), receipts[1].scope());
                assert_eq!(
                    store.scopes.get(receipts[0].scope()).owner,
                    Some(proof.method())
                );
                for (index, receipt) in receipts.into_iter().enumerate() {
                    let TreeKind::ValDef(local) = &source_ast.get(block.stats[index]).kind else {
                        panic!("receipt declaration is a ValDef");
                    };
                    let initializer = local.rhs.unwrap();
                    let TreeKind::Apply(apply) = &source_ast.get(initializer).kind else {
                        panic!("initializer is an Apply");
                    };
                    assert_eq!(receipt.source_initializer(), initializer);
                    assert_eq!(receipt.source_select(), apply.function);
                    assert_eq!(receipt.source_argument(), apply.args[0]);
                    assert_eq!(
                        receipt.typed_initializer(),
                        source_typed_index.get(source_id, initializer).unwrap()
                    );
                    assert_eq!(
                        receipt.typed_argument(),
                        source_typed_index.get(source_id, apply.args[0]).unwrap()
                    );
                    for ty in [receipt.local_type(), receipt.initializer_result_type()] {
                        assert!(matches!(
                            store.types.try_get(ty),
                            Some(Type::TypeRef { target: TypeRefTarget::Symbol(class), .. })
                                if *class == counter
                        ));
                    }
                    assert_eq!(receipt.seed_formal_type(), definitions.int);
                    assert_eq!(receipt.constructor_formal_type(), definitions.int);
                    assert_eq!(
                        store.symbols.get(receipt.seed_formal()).owner,
                        Some(proof.method())
                    );
                    assert_eq!(
                        store.symbols.get(receipt.constructor()).owner,
                        Some(counter)
                    );
                }
                if source_number == 2134 {
                    let TreeKind::DefDef(method) = &source_ast.get(method_tree).kind else {
                        unreachable!();
                    };
                    let original_formals = method.value_param_clauses[0]
                        .iter()
                        .map(|tree| source_index.symbol_at(source_id, *tree).unwrap())
                        .collect::<Vec<_>>();
                    assert_eq!(
                        store
                            .names
                            .resolve(store.symbols.get(receipts[0].symbol()).name.text()),
                        "second"
                    );
                    assert_eq!(
                        store
                            .names
                            .resolve(store.symbols.get(receipts[1].symbol()).name.text()),
                        "first"
                    );
                    assert_eq!(
                        store
                            .names
                            .resolve(store.symbols.get(receipts[0].seed_formal()).name.text()),
                        "secondSeed"
                    );
                    assert_eq!(
                        store
                            .names
                            .resolve(store.symbols.get(receipts[1].seed_formal()).name.text()),
                        "firstSeed"
                    );
                    assert_eq!(receipts[0].seed_formal(), original_formals[1]);
                    assert_eq!(receipts[1].seed_formal(), original_formals[0]);
                    assert_eq!(
                        receipts.map(|receipt| receipt.source_declaration()),
                        [block.stats[0], block.stats[1]]
                    );
                }
                if source_number == 2136 {
                    let TreeKind::DefDef(method) = &source_ast.get(method_tree).kind else {
                        unreachable!();
                    };
                    let original_formals = method.value_param_clauses[0]
                        .iter()
                        .map(|tree| source_index.symbol_at(source_id, *tree).unwrap())
                        .collect::<Vec<_>>();
                    assert_eq!(receipts[0].seed_formal(), original_formals[1]);
                    assert_eq!(receipts[1].seed_formal(), original_formals[0]);
                    assert_eq!(
                        receipts.map(|receipt| receipt.source_declaration()),
                        [block.stats[0], block.stats[1]]
                    );
                }
            },
        );
    }
    with_paired_jvm_tasty_batch_plan(
        TWO_IMMUTABLE_LOCALS_SOURCE,
        "TwoLocals.scala",
        SourceId::from_index(2126),
        |batch| {
            let proof = batch
                .constructor_call_proofs
                .iter()
                .find(|proof| {
                    proof.argument_proof().kind()
                        == dotty::jvm_tasty_cross_class::ConstructorArgumentProofKind::LocalValPair
                })
                .expect("the original source retains its LocalValPair proof");
            let owner = batch
                .jvm_unit
                .store
                .symbols
                .get(proof.method())
                .owner
                .unwrap();
            let factory = batch
                .classes
                .iter()
                .find(|class| class.class == owner)
                .expect("the source batch retains PairFactory");
            factory
                .tasty_plan
                .validate_method_bodies()
                .expect("the authenticated two-local method body validates");
            assert!(
                !factory
                    .tasty_plan
                    .pickle()
                    .expect("the two-local body now emits its TASTy sidecar")
                    .bytes()
                    .is_empty()
            );
        },
    )
    .expect("the authenticated two-local source/typed body reaches TASTy output");
}

#[test]
fn ordered_two_local_pair_arguments_reuse_initializer_receipts_without_authorizing_output() {
    let aliased =
        TWO_IMMUTABLE_LOCALS_SOURCE.replace("new Pair(first, second)", "new Pair(first, first)");
    let cases = [
        (TWO_IMMUTABLE_LOCALS_SOURCE, 21240, [0usize, 1usize]),
        (
            TWO_IMMUTABLE_LOCALS_REVERSED_SOURCE,
            21241,
            [1usize, 0usize],
        ),
        (aliased.as_str(), 21242, [0usize, 0usize]),
    ];
    let mut normalized = Vec::new();
    for (source, source_number, expected_positions) in cases {
        with_two_local_proof_session_retaining_typer(
            source,
            source_number,
            |store,
             definitions,
             source_ast,
             _typed_ast,
             source_index,
             _source_typed_index,
             source_id,
             method_tree,
             bindings,
             _other_local_binding,
             typer,
             jvm_unit| {
                let typer = typer.expect("unmodified session retains its original Typer");
                let source_root = source_ast
                    .iter()
                    .find_map(|(tree, node)| {
                        matches!(node.kind, TreeKind::PackageDef(_)).then_some(tree)
                    })
                    .expect("fixture retains its original package root");
                let proofs = dotty::jvm_tasty_cross_class::
                    validate_cross_class_constructor_call_proofs_with_typer(
                        CrossClassConstructorCallProofInput {
                            source_id,
                            source_root,
                            store,
                            definitions,
                            source_ast,
                            typed_ast: typer.typed_ast(),
                            source_index,
                            source_typed_index: typer.source_typed_index(),
                            jvm_unit,
                        },
                        typer,
                    )
                    .unwrap();
                let method = source_index.symbol_at(source_id, method_tree).unwrap();
                let proof = proofs
                    .iter()
                    .find(|proof| proof.method() == method)
                    .expect("the original factory method has a proof");
                let argument_proof = proof.argument_proof();
                assert_eq!(
                    argument_proof.kind(),
                    dotty::jvm_tasty_cross_class::ConstructorArgumentProofKind::LocalValPair
                );
                let pair = argument_proof
                    .ordered_pair()
                    .expect("both local arguments retain ordered receipts");
                let arguments = pair.arguments();
                let local_symbols = [bindings[0].symbol(), bindings[1].symbol()];
                let initializer_symbols = pair
                    .local_initializers()
                    .expect("the pair proof retains both initializer receipts")
                    .map(|receipt| receipt.symbol());
                assert_eq!(initializer_symbols, local_symbols);
                let initializer_positions = initializer_symbols.map(|symbol| {
                    local_symbols
                        .iter()
                        .position(|candidate| *candidate == symbol)
                        .expect("initializer receipt is one of the original local declarations")
                });
                assert_eq!(initializer_positions, [0, 1]);
                let actual = arguments.map(|argument| {
                    argument
                        .local_val()
                        .expect("each Pair position is an initialized local")
                        .symbol()
                });
                assert_eq!(actual, expected_positions.map(|index| local_symbols[index]));
                let actual_positions = actual.map(|symbol| {
                    local_symbols
                        .iter()
                        .position(|candidate| *candidate == symbol)
                        .expect("argument target is one of the original local symbols")
                });
                assert_eq!(actual_positions, expected_positions);
                assert!(local_symbols.contains(&actual[0]));
                assert!(local_symbols.contains(&actual[1]));
                assert!(!local_symbols.contains(&pair.constructor_formals()[0]));
                assert!(!local_symbols.contains(&pair.constructor_formals()[1]));
                assert_ne!(pair.constructor_formals()[0], pair.constructor_formals()[1]);
                assert!(arguments.iter().all(|argument| {
                    argument.formal().is_none() && argument.nested_new().is_none()
                }));
                normalized.push(actual_positions);
            },
        );
    }
    assert_eq!(normalized, [[0, 1], [1, 0], [0, 0]]);
}

#[test]
fn two_local_pair_rejects_nested_argument_even_when_other_local_is_authenticated() {
    let nested = TWO_IMMUTABLE_LOCALS_SOURCE.replace(
        "new Pair(first, second)",
        "new Pair(new Counter(firstSeed), second)",
    );
    with_two_local_proof_session_retaining_typer(
        &nested,
        21243,
        |store,
         definitions,
         source_ast,
         _typed_ast,
         source_index,
         _source_typed_index,
         source_id,
         _method_tree,
         _bindings,
         _other_local_binding,
         typer,
         jvm_unit| {
            let typer = typer.expect("the authentic source session retains its Typer");
            let source_root = source_ast
                .iter()
                .find_map(|(tree, node)| {
                    matches!(node.kind, TreeKind::PackageDef(_)).then_some(tree)
                })
                .expect("fixture retains its package root");
            let result = dotty::jvm_tasty_cross_class::
                validate_cross_class_constructor_call_proofs_with_typer(
                    CrossClassConstructorCallProofInput {
                        source_id,
                        source_root,
                        store,
                        definitions,
                        source_ast,
                        typed_ast: typer.typed_ast(),
                        source_index,
                        source_typed_index: typer.source_typed_index(),
                        jvm_unit,
                    },
                    typer,
                );
            let error = match result {
                Ok(_) => panic!("a nested argument cannot borrow the second local receipt"),
                Err(error) => error,
            };
            assert_eq!(
                error,
                dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceShape(
                    "two-local Pair arguments must be direct original local Idents"
                )
            );
        },
    );
}

#[test]
fn original_counter_set_call_is_proved_without_widening_pair_contract() {
    let renamed = COUNTER_SET_CALL_SOURCE
        .replace("PairFactory", "Assembler")
        .replace("create", "assemble")
        .replace("firstSeed", "leftSeed")
        .replace("secondSeed", "rightSeed")
        .replace("first", "left")
        .replace("second", "right");
    let reversed = COUNTER_SET_CALL_SOURCE.replace(
        "    val first: Counter = new Counter(firstSeed)\n    val second: Counter = new Counter(secondSeed)",
        "    val second: Counter = new Counter(secondSeed)\n    val first: Counter = new Counter(firstSeed)",
    );
    let second_receiver =
        COUNTER_SET_CALL_SOURCE.replace("first.set(secondSeed)", "second.set(secondSeed)");

    for (source, source_id, receiver_index) in [
        (COUNTER_SET_CALL_SOURCE, 21650, 0),
        (renamed.as_str(), 21651, 0),
        (reversed.as_str(), 21652, 1),
        (second_receiver.as_str(), 21653, 1),
    ] {
        with_two_local_proof_session(
            source,
            source_id,
            |store,
             definitions,
             source_ast,
             typed_ast,
             source_index,
             source_typed_index,
             source_id,
             method_tree,
             bindings,
             _other_local_binding,
             _typer,
             jvm_unit| {
                let input = || ImmutableLocalInitializerPairProofInput {
                    source_id,
                    method_tree,
                    store,
                    definitions,
                    source_ast,
                    typed_ast,
                    source_index,
                    source_typed_index,
                    local_bindings: [Some(bindings[0]), Some(bindings[1])],
                    jvm_unit,
                };
                assert!(matches!(
                    validate_immutable_local_initializer_pair_proof(input()),
                    Err(dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceShape(_))
                ), "the existing two-statement proof stays strict");
                let proof = validate_immutable_local_counter_set_call_proof(input())
                    .unwrap_or_else(|error| panic!("source {source_id:?}: {error:?}"));
                let TreeKind::DefDef(method) = &source_ast.get(method_tree).kind else {
                    panic!("factory method remains original");
                };
                let TreeKind::Block(block) = &source_ast.get(method.rhs.unwrap()).kind else {
                    panic!("factory body remains original Block");
                };
                assert_eq!(proof.source_call(), block.stats[2]);
                assert_eq!(
                    proof.typed_call(),
                    source_typed_index.get(source_id, block.stats[2]).unwrap()
                );
                assert_eq!(proof.receiver(), bindings[receiver_index].symbol());
                assert_eq!(
                    proof.formal(),
                    source_index
                        .symbol_at(source_id, method.value_param_clauses[0][1])
                        .unwrap()
                );
                assert_eq!(proof.argument_type(), definitions.int);
                assert_eq!(proof.result_type(), definitions.unit);
                assert_eq!(
                    store.symbols.get(proof.method()).owner,
                    Some(proof.initializer_pair().counter())
                );

                let TreeKind::Apply(final_apply) = &source_ast
                    .get(proof.initializer_pair().source_final_expression())
                    .kind
                else {
                    panic!("final Pair expression remains an Apply");
                };
                let final_typed_select = source_typed_index
                    .get(source_id, final_apply.function)
                    .unwrap();
                let mut foreign_owner = clone_arena(typed_ast);
                foreign_owner.get_mut(proof.typed_select()).ty =
                    typed_ast.get(final_typed_select).ty;
                let foreign_owner_unit = JvmCompilationUnit {
                    store,
                    typed_ast: &foreign_owner,
                    classes: jvm_unit.classes.clone(),
                };
                let wrong_owner = validate_immutable_local_counter_set_call_proof(
                    ImmutableLocalInitializerPairProofInput {
                        typed_ast: &foreign_owner,
                        jvm_unit: &foreign_owner_unit,
                        ..input()
                    },
                );
                assert_eq!(
                    wrong_owner.err().unwrap(),
                    dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SemanticIdentity(
                        "selected Counter.set must be the original accessible Counter member"
                    )
                );

                let TreeKind::Apply(apply) = &source_ast.get(proof.source_call()).kind else {
                    panic!("call receipt preserves the source Apply");
                };
                let mut stale_index = source_typed_index.clone();
                assert!(stale_index.remove(source_id, apply.function).is_some());
                let stale = validate_immutable_local_counter_set_call_proof(
                    ImmutableLocalInitializerPairProofInput {
                        source_typed_index: &stale_index,
                        ..input()
                    },
                );
                assert_eq!(
                    stale.err().unwrap(),
                    dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::TypedBinding(
                        "Counter.set Select has no typed mapping"
                    )
                );

                let mut swapped_final_arguments = clone_arena(typed_ast);
                let typed_final = swapped_final_arguments
                    .get_mut(proof.initializer_pair().typed_final_expression());
                let TreeKind::Apply(final_apply) = &mut typed_final.kind else {
                    panic!("typed final Pair expression remains Apply");
                };
                final_apply.args.swap(0, 1);
                let swapped_unit = JvmCompilationUnit {
                    store,
                    typed_ast: &swapped_final_arguments,
                    classes: jvm_unit.classes.clone(),
                };
                assert!(matches!(
                    validate_immutable_local_counter_set_call_proof(
                        ImmutableLocalInitializerPairProofInput {
                            typed_ast: &swapped_final_arguments,
                            jvm_unit: &swapped_unit,
                            ..input()
                        }
                    ),
                    Err(dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::TypedBinding(_))
                ), "frozen source order rejects swapped final typed argument edges");

                let replacement_new_kind = typed_ast
                    .iter()
                    .find_map(|(_, tree)| match &tree.kind {
                        TreeKind::New(new) => Some(TreeKind::New(*new)),
                        _ => None,
                    })
                    .expect("fixture has a typed constructor New");
                let TreeKind::Apply(typed_final) = &typed_ast
                    .get(proof.initializer_pair().typed_final_expression())
                    .kind
                else {
                    panic!("typed final Pair expression remains Apply");
                };
                let corrupted_references = [
                    proof.typed_receiver(),
                    proof.typed_argument(),
                    typed_final.args[0],
                    typed_final.args[1],
                ];
                for corrupted_reference in corrupted_references {
                    let mut corrupted_arena = clone_arena(typed_ast);
                    corrupted_arena.get_mut(corrupted_reference).kind =
                        replacement_new_kind.clone();
                    let corrupted_unit = JvmCompilationUnit {
                        store,
                        typed_ast: &corrupted_arena,
                        classes: jvm_unit.classes.clone(),
                    };
                    assert!(matches!(
                        validate_immutable_local_counter_set_call_proof(
                            ImmutableLocalInitializerPairProofInput {
                                typed_ast: &corrupted_arena,
                                jvm_unit: &corrupted_unit,
                                ..input()
                            }
                        ),
                        Err(dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::TypedBinding(_))
                    ), "a mapped typed New cannot stand in for a direct reference");
                }
            },
        );
    }

    let wrong_method_source = COUNTER_SET_CALL_SOURCE
        .replace(
            "  def set(value: Int): Unit = current = value",
            "  def set(value: Int): Unit = current = value\n  def ignore(value: Int): Unit = ()",
        )
        .replace("first.set(secondSeed)", "first.ignore(secondSeed)");
    with_two_local_proof_session(
        &wrong_method_source,
        21657,
        |store,
         definitions,
         source_ast,
         typed_ast,
         source_index,
         source_typed_index,
         source_id,
         method_tree,
         bindings,
         _,
         _,
         jvm_unit| {
            let result = validate_immutable_local_counter_set_call_proof(
                ImmutableLocalInitializerPairProofInput {
                    source_id,
                    method_tree,
                    store,
                    definitions,
                    source_ast,
                    typed_ast,
                    source_index,
                    source_typed_index,
                    local_bindings: [Some(bindings[0]), Some(bindings[1])],
                    jvm_unit,
                },
            );
            assert!(
                matches!(
            result,
            Err(dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceShape(
                "selected member must be the accessible original one-argument set method"
            ))
        ),
                "a same-owner, same-signature method is not Counter.set"
            );
        },
    );

    let wrong_formal =
        COUNTER_SET_CALL_SOURCE.replace("first.set(secondSeed)", "first.set(firstSeed)");
    let call_before_second_local = COUNTER_SET_CALL_SOURCE.replace(
        "    val first: Counter = new Counter(firstSeed)\n    val second: Counter = new Counter(secondSeed)\n    first.set(secondSeed)\n",
        "    val first: Counter = new Counter(firstSeed)\n    first.set(secondSeed)\n    val second: Counter = new Counter(secondSeed)\n",
    );
    let duplicate_call = COUNTER_SET_CALL_SOURCE.replace(
        "    first.set(secondSeed)\n",
        "    first.set(secondSeed)\n    second.set(firstSeed)\n",
    );
    for (source, source_id, expected) in [
        (
            wrong_formal.as_str(),
            21654,
            dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceShape(
                "Counter.set argument must be secondSeed",
            ),
        ),
        (
            call_before_second_local.as_str(),
            21655,
            dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceShape(
                "third factory Block statistic must be the original Counter.set Apply",
            ),
        ),
        (
            duplicate_call.as_str(),
            21656,
            dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceShape(
                "factory Block must contain exactly two local ValDefs and one Counter.set call",
            ),
        ),
    ] {
        with_two_local_proof_session(
            source,
            source_id,
            |store,
             definitions,
             source_ast,
             typed_ast,
             source_index,
             source_typed_index,
             source_id,
             method_tree,
             bindings,
             _other_local_binding,
             _typer,
             jvm_unit| {
                let result = validate_immutable_local_counter_set_call_proof(
                    ImmutableLocalInitializerPairProofInput {
                        source_id,
                        method_tree,
                        store,
                        definitions,
                        source_ast,
                        typed_ast,
                        source_index,
                        source_typed_index,
                        local_bindings: [Some(bindings[0]), Some(bindings[1])],
                        jvm_unit,
                    },
                );
                assert_eq!(result.err().unwrap(), expected);
            },
        );
    }
}

#[test]
fn ordered_counter_set_pair_receipt_keeps_source_typed_order_and_local_identity() {
    let swapped_calls = ORDERED_COUNTER_SET_CALLS_SOURCE.replace(
        "    first.set(secondSeed)\n    second.set(firstSeed)",
        "    second.set(firstSeed)\n    first.set(secondSeed)",
    );
    let reversed_declarations = ORDERED_COUNTER_SET_CALLS_SOURCE.replace(
        "    val first: Counter = new Counter(firstSeed)\n    val second: Counter = new Counter(secondSeed)",
        "    val second: Counter = new Counter(secondSeed)\n    val first: Counter = new Counter(firstSeed)",
    );
    for (source, source_id) in [
        (ORDERED_COUNTER_SET_CALLS_SOURCE, 21891),
        (swapped_calls.as_str(), 21892),
        (reversed_declarations.as_str(), 21893),
    ] {
        with_two_local_proof_session(
            source,
            source_id,
            |store,
             definitions,
             source_ast,
             typed_ast,
             source_index,
             source_typed_index,
             source_id,
             method_tree,
             bindings,
             _,
             _,
             jvm_unit| {
                let input = ImmutableLocalInitializerPairProofInput {
                    source_id,
                    method_tree,
                    store,
                    definitions,
                    source_ast,
                    typed_ast,
                    source_index,
                    source_typed_index,
                    local_bindings: [Some(bindings[0]), Some(bindings[1])],
                    jvm_unit,
                };
                let proof = validate_immutable_local_counter_set_calls_proof(input).expect(
                    "both original calls retain canonical ordered source and typed identity",
                );
                let calls = proof.calls();
                let TreeKind::DefDef(method) = &source_ast.get(method_tree).kind else {
                    panic!("factory method root is DefDef");
                };
                let TreeKind::Block(block) = &source_ast.get(method.rhs.unwrap()).kind else {
                    panic!("factory body is Block");
                };
                assert_eq!(block.stats.len(), 4);
                let typed_block = source_typed_index
                    .get(source_id, method.rhs.unwrap())
                    .and_then(|tree| match &typed_ast.get(tree).kind {
                        TreeKind::Block(block) => Some(block),
                        _ => None,
                    })
                    .unwrap();
                for (index, call) in calls.iter().enumerate() {
                    assert_eq!(call.source_call(), block.stats[index + 2]);
                    assert_eq!(call.typed_call(), typed_block.stats[index + 2]);
                    assert_eq!(call.method(), calls[0].method());
                    assert_eq!(call.argument_type(), definitions.int);
                    assert_eq!(call.result_type(), definitions.unit);
                    let local = bindings
                        .iter()
                        .find(|binding| binding.symbol() == call.receiver())
                        .expect("receiver resolves to one initialized local symbol");
                    assert_eq!(call.receiver_scope(), local.scope());
                }
                assert_ne!(calls[0].source_call(), calls[1].source_call());
                assert_ne!(calls[0].typed_call(), calls[1].typed_call());
                assert_ne!(calls[0].formal(), calls[1].formal());
            },
        );
    }

    with_two_local_proof_session(
        ORDERED_COUNTER_SET_CALLS_SOURCE,
        21894,
        |store,
         definitions,
         source_ast,
         typed_ast,
         source_index,
         source_typed_index,
         source_id,
         method_tree,
         bindings,
         _,
         _,
         jvm_unit| {
            let TreeKind::DefDef(method) = &source_ast.get(method_tree).kind else {
                panic!("factory method root is DefDef");
            };
            let TreeKind::Block(block) = &source_ast.get(method.rhs.unwrap()).kind else {
                panic!("factory body is Block");
            };
            let first = source_typed_index.get(source_id, block.stats[2]).unwrap();
            let second = source_typed_index.get(source_id, block.stats[3]).unwrap();
            let mut swapped = source_typed_index.clone();
            swapped.remove(source_id, block.stats[2]);
            swapped.remove(source_id, block.stats[3]);
            swapped.insert(source_id, block.stats[2], second).unwrap();
            swapped.insert(source_id, block.stats[3], first).unwrap();
            let result = validate_immutable_local_counter_set_calls_proof(
                ImmutableLocalInitializerPairProofInput {
                    source_id,
                    method_tree,
                    store,
                    definitions,
                    source_ast,
                    typed_ast,
                    source_index,
                    source_typed_index: &swapped,
                    local_bindings: [Some(bindings[0]), Some(bindings[1])],
                    jvm_unit,
                },
            );
            assert!(matches!(
                result,
                Err(
                    dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::TypedBinding(
                        _
                    )
                )
            ));

            for call_offset in 0..2 {
                let source_call = block.stats[2 + call_offset];
                let TreeKind::Apply(source_apply) = &source_ast.get(source_call).kind else {
                    panic!("ordered source call remains an Apply");
                };
                let source_select = source_apply.function;
                let TreeKind::Select(source_selection) = &source_ast.get(source_select).kind else {
                    panic!("ordered source call retains its Select");
                };
                let TreeKind::Ident(source_receiver) =
                    &source_ast.get(source_selection.qualifier).kind
                else {
                    panic!("ordered source receiver remains an Ident");
                };
                let typed_select = source_typed_index.get(source_id, source_select).unwrap();
                let mut mismatched_name_ast = clone_arena(typed_ast);
                let TreeKind::Select(typed_selection) =
                    &mut mismatched_name_ast.get_mut(typed_select).kind
                else {
                    panic!("mapped typed call function remains a Select");
                };
                typed_selection.name = source_receiver.name;
                let mismatched_jvm_unit = JvmCompilationUnit {
                    store: jvm_unit.store,
                    typed_ast: &mismatched_name_ast,
                    classes: jvm_unit.classes.clone(),
                };

                let result = validate_immutable_local_counter_set_calls_proof(
                    ImmutableLocalInitializerPairProofInput {
                        source_id,
                        method_tree,
                        store,
                        definitions,
                        source_ast,
                        typed_ast: &mismatched_name_ast,
                        source_index,
                        source_typed_index,
                        local_bindings: [Some(bindings[0]), Some(bindings[1])],
                        jvm_unit: &mismatched_jvm_unit,
                    },
                );
                let error = match result {
                    Err(error) => error,
                    Ok(_) => {
                        panic!("typed Select name mismatch in call slot {call_offset} was accepted")
                    }
                };
                assert_eq!(
                    error,
                    dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::TypedBinding(
                        "typed Counter.set Select name must match the original canonical set member"
                    ),
                    "call slot {call_offset} rejects the name mismatch before later proof checks"
                );
            }
        },
    );
}

#[test]
fn two_local_initializer_receipt_rejects_stale_and_swapped_typed_edges() {
    with_two_local_proof_session(
        TWO_IMMUTABLE_LOCALS_SOURCE,
        2127,
        |store,
         definitions,
         source_ast,
         typed_ast,
         source_index,
         source_typed_index,
         source_id,
         method_tree,
         bindings,
         _other_local_binding,
         _typer,
         jvm_unit| {
            macro_rules! proof_input {
                ($index:expr) => {
                    ImmutableLocalInitializerPairProofInput {
                        source_id,
                        method_tree,
                        store,
                        definitions,
                        source_ast,
                        typed_ast,
                        source_index,
                        source_typed_index: $index,
                        local_bindings: [Some(bindings[0]), Some(bindings[1])],
                        jvm_unit,
                    }
                };
            }
            let proof =
                validate_immutable_local_initializer_pair_proof(proof_input!(source_typed_index))
                    .unwrap();
            let second_select = proof.initializers()[1].source_select();
            let second_typed_select = proof.initializers()[1].typed_select();
            let TreeKind::DefDef(method) = &source_ast.get(method_tree).kind else {
                panic!("method root is DefDef");
            };
            let TreeKind::Block(block) = &source_ast.get(method.rhs.unwrap()).kind else {
                panic!("method body is Block");
            };
            let declaration_seed = |declaration| {
                let TreeKind::ValDef(local) = &source_ast.get(declaration).kind else {
                    unreachable!();
                };
                let TreeKind::Apply(apply) = &source_ast.get(local.rhs.unwrap()).kind else {
                    unreachable!();
                };
                apply.args[0]
            };
            let first_seed = declaration_seed(block.stats[0]);
            let second_seed = declaration_seed(block.stats[1]);
            let first_typed_seed = source_typed_index.get(source_id, first_seed).unwrap();
            let second_typed_seed = source_typed_index.get(source_id, second_seed).unwrap();
            let mut swapped_seed_edges = source_typed_index.clone();
            swapped_seed_edges.remove(source_id, first_seed).unwrap();
            swapped_seed_edges.remove(source_id, second_seed).unwrap();
            swapped_seed_edges
                .insert(source_id, first_seed, second_typed_seed)
                .unwrap();
            swapped_seed_edges
                .insert(source_id, second_seed, first_typed_seed)
                .unwrap();
            assert_eq!(
                validate_immutable_local_initializer_pair_proof(proof_input!(&swapped_seed_edges))
                    .err()
                    .unwrap(),
                dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceShape(
                    "initializer must use the original seed formal"
                )
            );
            let TreeKind::Apply(final_apply) = &source_ast.get(block.expr).kind else {
                panic!("final expression is a constructor Apply");
            };
            let final_select = final_apply.function;
            let mut wrong_final_select = source_typed_index.clone();
            wrong_final_select.remove(source_id, final_select).unwrap();
            wrong_final_select
                .insert(
                    source_id,
                    final_select,
                    proof.initializers()[0].typed_select(),
                )
                .unwrap();
            assert_eq!(
                validate_immutable_local_initializer_pair_proof(proof_input!(&wrong_final_select))
                    .err()
                    .unwrap(),
                dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::TypedBinding(
                    "typed final constructor Apply must retain its original Select edge"
                )
            );

            let mut missing_edge = source_typed_index.clone();
            assert_eq!(
                missing_edge.remove(source_id, second_select),
                Some(second_typed_select)
            );
            assert_eq!(
                validate_immutable_local_initializer_pair_proof(proof_input!(&missing_edge))
                    .err()
                    .unwrap(),
                dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::TypedBinding(
                    "initializer Select has no typed mapping"
                )
            );

            let TreeKind::DefDef(method) = &source_ast.get(method_tree).kind else {
                panic!("method root is DefDef");
            };
            let TreeKind::Block(block) = &source_ast.get(method.rhs.unwrap()).kind else {
                panic!("method body is Block");
            };
            let mut swapped = source_typed_index.clone();
            let first_typed = swapped.remove(source_id, block.stats[0]).unwrap();
            let second_typed = swapped.remove(source_id, block.stats[1]).unwrap();
            swapped
                .insert(source_id, block.stats[0], second_typed)
                .unwrap();
            swapped
                .insert(source_id, block.stats[1], first_typed)
                .unwrap();
            assert_eq!(
                validate_immutable_local_initializer_pair_proof(proof_input!(&swapped))
                    .err()
                    .unwrap(),
                dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::TypedBinding(
                    "typed Block must preserve both original ValDefs and final expression"
                )
            );
        },
    );
}

#[test]
fn two_local_initializer_receipt_rejects_mutable_and_wrong_formal_initializers() {
    let mutable = TWO_IMMUTABLE_LOCALS_SOURCE.replace("val second: Counter", "var second: Counter");
    with_two_local_proof_session(
        &mutable,
        2128,
        |store,
         definitions,
         source_ast,
         typed_ast,
         source_index,
         source_typed_index,
         source_id,
         method_tree,
         bindings,
         _other_local_binding,
         _typer,
         jvm_unit| {
            assert_eq!(
                validate_immutable_local_initializer_pair_proof(
                    ImmutableLocalInitializerPairProofInput {
                        source_id,
                        method_tree,
                        store,
                        definitions,
                        source_ast,
                        typed_ast,
                        source_index,
                        source_typed_index,
                        local_bindings: [Some(bindings[0]), Some(bindings[1])],
                        jvm_unit,
                    }
                )
                .err()
                .unwrap(),
                dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceIdentity(
                    "local must be the original immutable method-owned local"
                )
            );
        },
    );
    let swapped_formal =
        TWO_IMMUTABLE_LOCALS_SOURCE.replace("new Counter(secondSeed)", "new Counter(firstSeed)");
    with_two_local_proof_session(
        &swapped_formal,
        2129,
        |store,
         definitions,
         source_ast,
         typed_ast,
         source_index,
         source_typed_index,
         source_id,
         method_tree,
         bindings,
         _other_local_binding,
         _typer,
         jvm_unit| {
            assert_eq!(
                validate_immutable_local_initializer_pair_proof(
                    ImmutableLocalInitializerPairProofInput {
                        source_id,
                        method_tree,
                        store,
                        definitions,
                        source_ast,
                        typed_ast,
                        source_index,
                        source_typed_index,
                        local_bindings: [Some(bindings[0]), Some(bindings[1])],
                        jvm_unit,
                    }
                )
                .err()
                .unwrap(),
                dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceShape(
                    "two local initializers must use distinct original method formals"
                )
            );
        },
    );
    let conditional = TWO_IMMUTABLE_LOCALS_SOURCE.replace(
        "new Counter(firstSeed)",
        "if firstSeed > 0 then new Counter(firstSeed) else new Counter(secondSeed)",
    );
    with_two_local_proof_session(
        &conditional,
        2133,
        |store,
         definitions,
         source_ast,
         typed_ast,
         source_index,
         source_typed_index,
         source_id,
         method_tree,
         bindings,
         _other_local_binding,
         _typer,
         jvm_unit| {
            assert_eq!(
                validate_immutable_local_initializer_pair_proof(
                    ImmutableLocalInitializerPairProofInput {
                        source_id,
                        method_tree,
                        store,
                        definitions,
                        source_ast,
                        typed_ast,
                        source_index,
                        source_typed_index,
                        local_bindings: [Some(bindings[0]), Some(bindings[1])],
                        jvm_unit,
                    }
                )
                .err()
                .unwrap(),
                dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceShape(
                    "local initializer must be the original direct constructor Apply"
                )
            );
        },
    );
}

#[test]
fn two_local_initializer_receipt_rejects_third_local_and_missing_binding() {
    let third_local = TWO_IMMUTABLE_LOCALS_SOURCE.replace(
        "    new Pair(first, second)",
        "    val third: Counter = new Counter(firstSeed)\n    new Pair(first, second)",
    );
    with_two_local_proof_session(
        &third_local,
        2131,
        |store,
         definitions,
         source_ast,
         typed_ast,
         source_index,
         source_typed_index,
         source_id,
         method_tree,
         bindings,
         _other_local_binding,
         _typer,
         jvm_unit| {
            let error = validate_immutable_local_initializer_pair_proof(
                ImmutableLocalInitializerPairProofInput {
                    source_id,
                    method_tree,
                    store,
                    definitions,
                    source_ast,
                    typed_ast,
                    source_index,
                    source_typed_index,
                    local_bindings: [Some(bindings[0]), Some(bindings[1])],
                    jvm_unit,
                },
            )
            .err()
            .unwrap();
            assert_eq!(
                error,
                dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceShape(
                    "factory Block must contain exactly two local ValDefs"
                )
            );
        },
    );
    with_two_local_proof_session(
        TWO_IMMUTABLE_LOCALS_SOURCE,
        2132,
        |store,
         definitions,
         source_ast,
         typed_ast,
         source_index,
         source_typed_index,
         source_id,
         method_tree,
         bindings,
         _other_local_binding,
         _typer,
         jvm_unit| {
            assert_eq!(
                validate_immutable_local_initializer_pair_proof(
                    ImmutableLocalInitializerPairProofInput {
                        source_id,
                        method_tree,
                        store,
                        definitions,
                        source_ast,
                        typed_ast,
                        source_index,
                        source_typed_index,
                        local_bindings: [None, Some(bindings[1])],
                        jvm_unit,
                    }
                )
                .err()
                .unwrap(),
                dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceIdentity(
                    "first local declaration has no original Typer binding"
                )
            );
        },
    );
}

#[test]
fn two_local_initializer_receipt_rejects_a_non_constructor_final_apply() {
    let wrong_result = TWO_IMMUTABLE_LOCALS_SOURCE
        .replace(
            "def create(firstSeed: Int, secondSeed: Int): Pair =",
            "def create(firstSeed: Int, secondSeed: Int): Int =",
        )
        .replace("new Pair(first, second)", "first.get()");
    with_two_local_proof_session(
        &wrong_result,
        2140,
        |store,
         definitions,
         source_ast,
         typed_ast,
         source_index,
         source_typed_index,
         source_id,
         method_tree,
         bindings,
         _other_local_binding,
         _typer,
         jvm_unit| {
            assert_eq!(
                validate_immutable_local_initializer_pair_proof(
                    ImmutableLocalInitializerPairProofInput {
                        source_id,
                        method_tree,
                        store,
                        definitions,
                        source_ast,
                        typed_ast,
                        source_index,
                        source_typed_index,
                        local_bindings: [Some(bindings[0]), Some(bindings[1])],
                        jvm_unit,
                    }
                )
                .err()
                .unwrap(),
                dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceShape(
                    "final Block expression must select a primary constructor"
                )
            );
        },
    );
}

#[test]
fn two_local_initializer_receipt_rejects_three_argument_result_constructors() {
    let three_argument = TWO_IMMUTABLE_LOCALS_SOURCE
        .replace(
            "class Pair(first: Counter, second: Counter):",
            "class Pair(first: Counter, second: Counter, extra: Int):",
        )
        .replace(
            "new Pair(first, second)",
            "new Pair(first, second, firstSeed)",
        );
    with_two_local_proof_session(
        &three_argument,
        2147,
        |store,
         definitions,
         source_ast,
         typed_ast,
         source_index,
         source_typed_index,
         source_id,
         method_tree,
         bindings,
         _,
         _typer,
         jvm_unit| {
            assert_eq!(
                validate_immutable_local_initializer_pair_proof(
                    ImmutableLocalInitializerPairProofInput {
                        source_id,
                        method_tree,
                        store,
                        definitions,
                        source_ast,
                        typed_ast,
                        source_index,
                        source_typed_index,
                        local_bindings: [Some(bindings[0]), Some(bindings[1])],
                        jvm_unit,
                    },
                )
                .err()
                .unwrap(),
                dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceShape(
                    "final constructor Apply must have exactly two arguments"
                )
            );
        },
    );
}

#[test]
fn two_local_initializer_receipt_rejects_out_of_range_and_other_owner_bindings() {
    // The foreign binding is created from another genuine Typer session with
    // the same Counter declaration name as this session.
    let mut padded = (0..40)
        .map(|index| format!("class Padding{index}\n"))
        .collect::<String>();
    padded.push_str("class PairFactory:\n  def other(seed: Int): Counter =\n    val outsider: Counter = new Counter(seed)\n    outsider\n");
    let foreign_source = TWO_IMMUTABLE_LOCALS_SOURCE.replace("class PairFactory:\n", &padded);
    let foreign_binding = with_two_local_proof_session(
        &foreign_source,
        2141,
        |_, _, _, _, _, _, _, _, _, other_local_binding, _, _| {
            other_local_binding.expect("helper method exposes its authentic Typer binding")
        },
    );
    with_two_local_proof_session(
        TWO_IMMUTABLE_LOCALS_SOURCE,
        2142,
        |store,
         definitions,
         source_ast,
         typed_ast,
         source_index,
         source_typed_index,
         source_id,
         method_tree,
         bindings,
         _other_local_binding,
         _typer,
         jvm_unit| {
            assert!(!store.symbols.contains(foreign_binding.symbol()));
            assert_eq!(
                validate_immutable_local_initializer_pair_proof(
                    ImmutableLocalInitializerPairProofInput {
                        source_id,
                        method_tree,
                        store,
                        definitions,
                        source_ast,
                        typed_ast,
                        source_index,
                        source_typed_index,
                        local_bindings: [Some(foreign_binding), Some(bindings[1])],
                        jvm_unit,
                    }
                )
                .err()
                .unwrap(),
                dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceIdentity(
                    "Typer local binding names a stale or foreign symbol"
                )
            );
        },
    );

    let wrong_owner_source = TWO_IMMUTABLE_LOCALS_SOURCE.replace(
        "class PairFactory:\n",
        "class PairFactory:\n  def other(seed: Int): Counter =\n    val outsider: Counter = new Counter(seed)\n    outsider\n",
    );
    with_two_local_proof_session(
        &wrong_owner_source,
        2143,
        |store,
         definitions,
         source_ast,
         typed_ast,
         source_index,
         source_typed_index,
         source_id,
         method_tree,
         bindings,
         other_local_binding,
         _typer,
         jvm_unit| {
            let foreign_scope = other_local_binding.unwrap();
            assert!(store.symbols.contains(foreign_scope.symbol()));
            assert_ne!(foreign_scope.scope(), bindings[0].scope());
            assert_eq!(
                validate_immutable_local_initializer_pair_proof(
                    ImmutableLocalInitializerPairProofInput {
                        source_id,
                        method_tree,
                        store,
                        definitions,
                        source_ast,
                        typed_ast,
                        source_index,
                        source_typed_index,
                        local_bindings: [Some(foreign_scope), Some(bindings[1])],
                        jvm_unit,
                    }
                )
                .err()
                .unwrap(),
                dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceIdentity(
                    "Typer local binding must name this exact original ValDef"
                )
            );
        },
    );
}

#[test]
fn two_local_initializer_receipt_rejects_owner_scope_constructor_and_foreign_counter() {
    with_two_local_proof_session_mutating(
        TWO_IMMUTABLE_LOCALS_SOURCE,
        2148,
        |store, bindings| {
            store.symbols.get_mut(bindings[0].symbol()).owner = Some(bindings[1].symbol());
        },
        |store,
         definitions,
         source_ast,
         typed_ast,
         source_index,
         source_typed_index,
         source_id,
         method_tree,
         bindings,
         _,
         _typer,
         jvm_unit| {
            assert_eq!(
                validate_immutable_local_initializer_pair_proof(
                    ImmutableLocalInitializerPairProofInput {
                        source_id,
                        method_tree,
                        store,
                        definitions,
                        source_ast,
                        typed_ast,
                        source_index,
                        source_typed_index,
                        local_bindings: [Some(bindings[0]), Some(bindings[1])],
                        jvm_unit,
                    },
                )
                .err()
                .unwrap(),
                dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceIdentity(
                    "local must be the original immutable method-owned local"
                )
            );
        },
    );
    with_two_local_proof_session_mutating(
        TWO_IMMUTABLE_LOCALS_SOURCE,
        2149,
        |store, bindings| {
            store.scopes.get_mut(bindings[0].scope()).owner = Some(bindings[1].symbol());
        },
        |store,
         definitions,
         source_ast,
         typed_ast,
         source_index,
         source_typed_index,
         source_id,
         method_tree,
         bindings,
         _,
         _typer,
         jvm_unit| {
            assert_eq!(
                validate_immutable_local_initializer_pair_proof(
                    ImmutableLocalInitializerPairProofInput {
                        source_id,
                        method_tree,
                        store,
                        definitions,
                        source_ast,
                        typed_ast,
                        source_index,
                        source_typed_index,
                        local_bindings: [Some(bindings[0]), Some(bindings[1])],
                        jvm_unit,
                    },
                )
                .err()
                .unwrap(),
                dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceIdentity(
                    "local must occupy its exact method-owned Block scope"
                )
            );
        },
    );
    with_two_local_proof_session(
        TWO_IMMUTABLE_LOCALS_SOURCE,
        2150,
        |store,
         definitions,
         source_ast,
         typed_ast,
         source_index,
         source_typed_index,
         source_id,
         method_tree,
         bindings,
         _,
         _typer,
         jvm_unit| {
            let TreeKind::DefDef(method) = &source_ast.get(method_tree).kind else {
                unreachable!()
            };
            let TreeKind::Block(block) = &source_ast.get(method.rhs.unwrap()).kind else {
                unreachable!()
            };
            let TreeKind::ValDef(first) = &source_ast.get(block.stats[0]).kind else {
                unreachable!()
            };
            let initializer = first.rhs.unwrap();
            let TreeKind::Apply(apply) = &source_ast.get(initializer).kind else {
                unreachable!()
            };
            let typed_select = source_typed_index.get(source_id, apply.function).unwrap();
            let TreeKind::Apply(final_apply) = &source_ast.get(block.expr).kind else {
                unreachable!()
            };
            let typed_final_select = source_typed_index
                .get(source_id, final_apply.function)
                .unwrap();
            let mut wrong_constructor_ast = clone_arena(typed_ast);
            wrong_constructor_ast.get_mut(typed_select).ty = typed_ast.get(typed_final_select).ty;
            let wrong_constructor_unit = JvmCompilationUnit {
                store,
                typed_ast: &wrong_constructor_ast,
                classes: jvm_unit.classes.clone(),
            };
            assert_eq!(
                validate_immutable_local_initializer_pair_proof(
                    ImmutableLocalInitializerPairProofInput {
                        source_id,
                        method_tree,
                        store,
                        definitions,
                        source_ast,
                        typed_ast: &wrong_constructor_ast,
                        source_index,
                        source_typed_index,
                        local_bindings: [Some(bindings[0]), Some(bindings[1])],
                        jvm_unit: &wrong_constructor_unit,
                    },
                )
                .err()
                .unwrap(),
                dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SemanticIdentity(
                    "selected constructor must belong to the canonical Counter class"
                )
            );
        },
    );
    with_two_local_proof_session_mutating(
        TWO_IMMUTABLE_LOCALS_SOURCE,
        2151,
        |store, bindings| {
            let SymbolInfo::Complete(local_type) = store.symbols.get(bindings[0].symbol()).info
            else {
                panic!("local type is complete");
            };
            let Type::TypeRef {
                prefix,
                target: TypeRefTarget::Symbol(counter),
            } = store.types.get(local_type)
            else {
                panic!("local type is Counter TypeRef");
            };
            let mut foreign_counter = store.symbols.get(*counter).clone();
            foreign_counter.origin = SymbolOrigin::Source(SourceId::from_index(999_991));
            let foreign_counter_id = store.symbols.alloc(foreign_counter);
            let foreign_type = store.types.alloc(Type::TypeRef {
                prefix: *prefix,
                target: TypeRefTarget::Symbol(foreign_counter_id),
            });
            store.symbols.get_mut(bindings[0].symbol()).info = SymbolInfo::Complete(foreign_type);
        },
        |store,
         definitions,
         source_ast,
         typed_ast,
         source_index,
         source_typed_index,
         source_id,
         method_tree,
         bindings,
         _,
         _typer,
         jvm_unit| {
            assert_eq!(
                validate_immutable_local_initializer_pair_proof(
                    ImmutableLocalInitializerPairProofInput {
                        source_id,
                        method_tree,
                        store,
                        definitions,
                        source_ast,
                        typed_ast,
                        source_index,
                        source_typed_index,
                        local_bindings: [Some(bindings[0]), Some(bindings[1])],
                        jvm_unit,
                    },
                )
                .err()
                .unwrap(),
                dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceIdentity(
                    "Counter must be a canonical class in the original source batch"
                )
            );
        },
    );
}

#[test]
fn two_local_initializer_receipt_rejects_duplicate_roots_and_missing_rhs() {
    with_two_local_proof_session(
        TWO_IMMUTABLE_LOCALS_SOURCE,
        2146,
        |store,
         definitions,
         source_ast,
         typed_ast,
         source_index,
         source_typed_index,
         source_id,
         method_tree,
         bindings,
         _,
         _typer,
         jvm_unit| {
            let TreeKind::DefDef(method) = &source_ast.get(method_tree).kind else {
                panic!("method root is DefDef");
            };
            let TreeKind::Block(block) = &source_ast.get(method.rhs.unwrap()).kind else {
                panic!("method body is Block");
            };
            let validate = |source: &AstArena<dotty_core::ast::Untyped>,
                            typed_index: &dotty_core::SourceTypedIndex| {
                validate_immutable_local_initializer_pair_proof(
                    ImmutableLocalInitializerPairProofInput {
                        source_id,
                        method_tree,
                        store,
                        definitions,
                        source_ast: source,
                        typed_ast,
                        source_index,
                        source_typed_index: typed_index,
                        local_bindings: [Some(bindings[0]), Some(bindings[1])],
                        jvm_unit,
                    },
                )
                .err()
            };
            let mut duplicate_declaration = clone_arena(source_ast);
            if let TreeKind::Block(block) =
                &mut duplicate_declaration.get_mut(method.rhs.unwrap()).kind
            {
                block.stats[1] = block.stats[0];
            }
            assert_eq!(
                validate(&duplicate_declaration, source_typed_index).unwrap(),
                dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceIdentity(
                    "two local declarations must have distinct original source roots"
                )
            );
            let first_initializer = match &source_ast.get(block.stats[0]).kind {
                TreeKind::ValDef(local) => local.rhs.unwrap(),
                _ => unreachable!(),
            };
            let second_initializer = match &source_ast.get(block.stats[1]).kind {
                TreeKind::ValDef(local) => local.rhs.unwrap(),
                _ => unreachable!(),
            };
            let mut use_before_initialization = clone_arena(source_ast);
            use_before_initialization
                .get_mut(first_initializer)
                .position = source_ast.get(second_initializer).position;
            assert_eq!(
                validate(&use_before_initialization, source_typed_index).unwrap(),
                dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceShape(
                    "two local initializers must be sequential and precede the final expression"
                )
            );
            let mut missing_rhs = clone_arena(source_ast);
            if let TreeKind::ValDef(local) = &mut missing_rhs.get_mut(block.stats[0]).kind {
                local.rhs = None;
            }
            assert_eq!(
                validate(&missing_rhs, source_typed_index).unwrap(),
                dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceShape(
                    "local must have one initializer"
                )
            );
            let mut duplicate_mapping = source_typed_index.clone();
            duplicate_mapping.remove(source_id, block.stats[1]).unwrap();
            duplicate_mapping
                .insert(source_id, block.stats[1], bindings[0].declaration())
                .unwrap();
            assert_eq!(
                validate(source_ast, &duplicate_mapping).unwrap(),
                dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::TypedBinding(
                    "two source ValDefs must retain distinct typed roots"
                )
            );
        },
    );
}

#[test]
fn two_local_initializer_receipt_rejects_wrong_local_type() {
    let wrong_local_type = TWO_IMMUTABLE_LOCALS_SOURCE
        .replace(
            "class Counter(start: Int):",
            "class Base\n\nclass Counter(start: Int) extends Base:",
        )
        .replace(
            "class Pair(first: Counter, second: Counter):",
            "class Pair(first: Base, second: Base):",
        )
        .replace("private val left: Counter", "private val left: Base")
        .replace("private val right: Counter", "private val right: Base")
        .replace("val first: Counter", "val first: Base")
        .replace("val second: Counter", "val second: Base")
        .replace(
            "new Pair(first, second)",
            "new Pair(new Counter(firstSeed), new Counter(secondSeed))",
        );
    with_two_local_proof_session(
        &wrong_local_type,
        2144,
        |store,
         definitions,
         source_ast,
         typed_ast,
         source_index,
         source_typed_index,
         source_id,
         method_tree,
         bindings,
         _other_local_binding,
         _typer,
         jvm_unit| {
            assert_eq!(
                validate_immutable_local_initializer_pair_proof(
                    ImmutableLocalInitializerPairProofInput {
                        source_id,
                        method_tree,
                        store,
                        definitions,
                        source_ast,
                        typed_ast,
                        source_index,
                        source_typed_index,
                        local_bindings: [Some(bindings[0]), Some(bindings[1])],
                        jvm_unit,
                    }
                )
                .err()
                .unwrap(),
                dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::TypedBinding(
                    "typed New must retain the canonical Counter TypeRef"
                )
            );
        },
    );
}

#[test]
fn two_ordered_nested_arguments_keep_distinct_source_and_typed_provenance() {
    with_two_argument_proof(
        TWO_ARGUMENT_SOURCE,
        19140,
        |proof, store, source, typed, index, bindings, source_id, root, definitions, jvm_unit| {
            assert_ordered_pair_receipt(
                proof,
                PairReceiptContext {
                    store,
                    source_ast: source,
                    typed_ast: typed,
                    source_index: index,
                    source_typed_index: bindings,
                    source_id,
                },
                ["first", "second"],
                "(II)Lexample/Pair;",
                "(Lexample/Counter;Lexample/Counter;)V",
            );
            let [first, second] = proof.nested_argument_pair().unwrap();

            let mut wrong_body_classes = jvm_unit.classes.clone();
            let factory_plan = wrong_body_classes
                .iter_mut()
                .find(|class| class.class == proof.caller_class())
                .unwrap();
            let factory_method_plan = factory_plan
                .methods
                .iter_mut()
                .find(|method| method.method == proof.method())
                .unwrap();
            factory_method_plan.body = Some(proof.typed_new());
            let wrong_body_unit = JvmCompilationUnit {
                store: jvm_unit.store,
                typed_ast: jvm_unit.typed_ast,
                classes: wrong_body_classes,
            };
            wrong_body_unit.metadata(definitions).unwrap();
            let wrong_body_result =
                validate_cross_class_constructor_call_proofs(CrossClassConstructorCallProofInput {
                    source_id,
                    source_root: root,
                    store,
                    definitions,
                    source_ast: source,
                    typed_ast: typed,
                    source_index: index,
                    source_typed_index: bindings,
                    jvm_unit: &wrong_body_unit,
                });
            assert!(matches!(
                wrong_body_result,
                Err(dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::JvmPlan(_))
            ));

            let factory_method_tree = match index.definition_of(proof.method()) {
                Some(SourceDefinition::Canonical { source, tree }) if source == source_id => tree,
                _ => panic!("factory method has its exact original declaration"),
            };
            let pair_tree = match index.definition_of(proof.target_class()) {
                Some(SourceDefinition::Canonical { source, tree }) if source == source_id => tree,
                _ => panic!("Pair class has its exact original declaration"),
            };
            let factory_tree = match index.definition_of(proof.caller_class()) {
                Some(SourceDefinition::Canonical { source, tree }) if source == source_id => tree,
                _ => panic!("factory class has its exact original declaration"),
            };
            let TreeKind::TypeDef(factory_definition) = &source.get(factory_tree).kind else {
                panic!("factory source declaration is a class");
            };
            let pair_constructor_tree = match index.definition_of(proof.constructor()) {
                Some(SourceDefinition::Canonical { source, tree }) if source == source_id => tree,
                _ => panic!("Pair constructor has its exact original declaration"),
            };
            let counter_tree = match index.definition_of(first.class()) {
                Some(SourceDefinition::Canonical { source, tree }) if source == source_id => tree,
                _ => panic!("Counter class has its exact original declaration"),
            };
            let TreeKind::TypeDef(counter_definition) = &source.get(counter_tree).kind else {
                panic!("Counter source declaration is a class");
            };
            let TreeKind::Template(counter_template) = &source.get(counter_definition.rhs).kind
            else {
                panic!("Counter source declaration has a template");
            };
            let counter_constructor_tree = counter_template.constructor;
            let TreeKind::DefDef(counter_constructor_definition) =
                &source.get(counter_constructor_tree).kind
            else {
                panic!("Counter constructor is a source definition");
            };
            let counter_formal_trees = counter_constructor_definition
                .value_param_clauses
                .iter()
                .flatten()
                .copied()
                .collect::<Vec<_>>();
            assert_eq!(counter_formal_trees.len(), 1);
            let pair_context = proof.argument_proof().nested_pair_context().unwrap();
            let retained_positions = [
                ("factory method", factory_method_tree),
                ("factory template", factory_definition.rhs),
                ("Pair class", pair_tree),
                ("Pair constructor", pair_constructor_tree),
                ("Counter class", counter_tree),
                ("Counter constructor", counter_constructor_tree),
                (
                    "factory first formal",
                    pair_context.source_method_formals()[0],
                ),
                (
                    "factory second formal",
                    pair_context.source_method_formals()[1],
                ),
                (
                    "Pair first formal",
                    pair_context.source_constructor_formals()[0],
                ),
                (
                    "Pair second formal",
                    pair_context.source_constructor_formals()[1],
                ),
                ("Counter formal", counter_formal_trees[0]),
            ];
            for (label, tree) in retained_positions {
                let mut foreign_position_source = clone_arena(source);
                let source_position = foreign_position_source.get(tree).position.unwrap();
                foreign_position_source.get_mut(tree).position = Some(SourceSpan::new(
                    SourceId::from_index(19143),
                    source_position.span(),
                ));
                let foreign_position_result = validate_cross_class_constructor_call_proofs(
                    CrossClassConstructorCallProofInput {
                        source_id,
                        source_root: root,
                        store,
                        definitions,
                        source_ast: &foreign_position_source,
                        typed_ast: typed,
                        source_index: index,
                        source_typed_index: bindings,
                        jvm_unit,
                    },
                );
                assert!(
                    matches!(
                        foreign_position_result,
                        Err(dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceIdentity(_))
                    ),
                    "pair proof must reject a foreign {label} source position"
                );

                let mut missing_position_source = clone_arena(source);
                missing_position_source.get_mut(tree).position = None;
                let missing_position_result = validate_cross_class_constructor_call_proofs(
                    CrossClassConstructorCallProofInput {
                        source_id,
                        source_root: root,
                        store,
                        definitions,
                        source_ast: &missing_position_source,
                        typed_ast: typed,
                        source_index: index,
                        source_typed_index: bindings,
                        jvm_unit,
                    },
                );
                assert!(
                    matches!(
                        missing_position_result,
                        Err(dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::SourceIdentity(_))
                    ),
                    "pair proof must reject a missing {label} source position"
                );
            }

            let mut missing = bindings.clone();
            assert_eq!(
                missing.remove(source_id, first.source_select()),
                Some(first.typed_select())
            );
            let missing_binding =
                validate_cross_class_constructor_call_proofs(CrossClassConstructorCallProofInput {
                    source_id,
                    source_root: root,
                    store,
                    definitions,
                    source_ast: source,
                    typed_ast: typed,
                    source_index: index,
                    source_typed_index: &missing,
                    jvm_unit,
                });
            assert!(matches!(
                missing_binding,
                Err(
                    dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::TypedBinding(
                        "nested Counter Select has no exact typed binding"
                    )
                )
            ));

            let mut swapped_binding = bindings.clone();
            swapped_binding.remove(source_id, first.source_argument());
            swapped_binding.remove(source_id, second.source_argument());
            swapped_binding
                .insert(source_id, first.source_argument(), second.typed_argument())
                .unwrap();
            let forged_leaf =
                validate_cross_class_constructor_call_proofs(CrossClassConstructorCallProofInput {
                    source_id,
                    source_root: root,
                    store,
                    definitions,
                    source_ast: source,
                    typed_ast: typed,
                    source_index: index,
                    source_typed_index: &swapped_binding,
                    jvm_unit,
                });
            match forged_leaf {
                Err(error) => assert_eq!(
                    error,
                    dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::TypedBinding(
                        "nested Counter typed Apply must preserve exact original Select, New, and leaf roots"
                    )
                ),
                Ok(_) => panic!("forged leaf binding unexpectedly produced a proof"),
            }
        },
    );

    let swapped = TWO_ARGUMENT_SOURCE.replace(
        "new Counter(first), new Counter(second)",
        "new Counter(second), new Counter(first)",
    );
    with_two_argument_proof(
        &swapped,
        19141,
        |proof,
         store,
         source,
         typed,
         index,
         bindings,
         source_id,
         _root,
         _definitions,
         _jvm_unit| {
            assert_ordered_pair_receipt(
                proof,
                PairReceiptContext {
                    store,
                    source_ast: source,
                    typed_ast: typed,
                    source_index: index,
                    source_typed_index: bindings,
                    source_id,
                },
                ["second", "first"],
                "(II)Lexample/Pair;",
                "(Lexample/Counter;Lexample/Counter;)V",
            );
        },
    );

    let repeated = TWO_ARGUMENT_SOURCE.replace(
        "new Counter(first), new Counter(second)",
        "new Counter(first), new Counter(first)",
    );
    with_two_argument_proof(
        &repeated,
        19142,
        |proof,
         store,
         source,
         typed,
         index,
         bindings,
         source_id,
         _root,
         _definitions,
         _jvm_unit| {
            assert_ordered_pair_receipt(
                proof,
                PairReceiptContext {
                    store,
                    source_ast: source,
                    typed_ast: typed,
                    source_index: index,
                    source_typed_index: bindings,
                    source_id,
                },
                ["first", "first"],
                "(II)Lexample/Pair;",
                "(Lexample/Counter;Lexample/Counter;)V",
            );
            let [first, second] = proof.nested_argument_pair().unwrap();
            assert_eq!(first.argument_formal(), second.argument_formal());
            assert_ne!(first.source_argument(), second.source_argument());
            assert_ne!(first.typed_argument(), second.typed_argument());
            let context = proof.argument_proof().nested_pair_context().unwrap();
            assert_ne!(context.method_formals()[0], context.method_formals()[1]);
        },
    );

    let counter_start = TWO_ARGUMENT_SOURCE.find("class Counter(start").unwrap();
    let pair_start = TWO_ARGUMENT_SOURCE.find("class Pair(first").unwrap();
    let factory_start = TWO_ARGUMENT_SOURCE.find("class PairFactory:").unwrap();
    let reordered = format!(
        "package example\n\n{}\n{}\n{}",
        &TWO_ARGUMENT_SOURCE[factory_start..],
        &TWO_ARGUMENT_SOURCE[pair_start..factory_start],
        &TWO_ARGUMENT_SOURCE[counter_start..pair_start],
    )
    .replace("PairFactory", "Assembler")
    .replace("Counter", "Token")
    .replace("Pair", "Envelope")
    .replace("example", "renamed.example")
    .replace("def create(", "def assemble(");
    with_two_argument_proof(
        &reordered,
        19143,
        |proof,
         store,
         source,
         typed,
         index,
         bindings,
         source_id,
         _root,
         _definitions,
         _jvm_unit| {
            assert_ordered_pair_receipt(
                proof,
                PairReceiptContext {
                    store,
                    source_ast: source,
                    typed_ast: typed,
                    source_index: index,
                    source_typed_index: bindings,
                    source_id,
                },
                ["first", "second"],
                "(II)Lrenamed/example/Envelope;",
                "(Lrenamed/example/Token;Lrenamed/example/Token;)V",
            );
        },
    );
}

#[test]
fn one_initialized_local_and_one_formal_keep_both_argument_orders() {
    use dotty::jvm_tasty_cross_class::ConstructorArgumentProofKind;

    for (source, source_number, local_first) in [
        (IMMUTABLE_LOCAL_PAIR_SOURCE, 20681, false),
        (IMMUTABLE_LOCAL_PAIR_REVERSED_SOURCE, 20682, true),
    ] {
        for fresh_session in 0..2 {
            with_two_argument_proof(
                source,
                source_number + fresh_session,
                |proof, store, source_ast, typed_ast, _index, bindings, source_id, _, _, _| {
                    let argument_proof = proof.argument_proof();
                    assert_eq!(
                        argument_proof.kind(),
                        ConstructorArgumentProofKind::FormalLocalPair
                    );
                    assert_eq!(
                        proof.method_descriptor(),
                        "(Lexample/Counter;I)Lexample/Pair;"
                    );
                    assert_eq!(
                        proof.constructor_descriptor(),
                        "(Lexample/Counter;Lexample/Counter;)V"
                    );
                    let pair = argument_proof.ordered_pair().unwrap();
                    let [first, second] = pair.arguments();
                    let ordered = [first, second];
                    for (index, argument) in ordered.into_iter().enumerate() {
                        if (index == 0) == local_first {
                            let local = argument
                                .local_val()
                                .expect("the corresponding ordered argument is the proven local");
                            assert_eq!(argument.source_argument(), local.source_use());
                            assert_eq!(argument.typed_argument(), local.typed_use());
                            assert!(matches!(
                                store.types.get(typed_ast.get(local.typed_use()).ty),
                                Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == local.symbol()
                            ));
                            assert_eq!(
                                source_ast.get(local.source_block()).position,
                                typed_ast.get(local.typed_block()).position
                            );
                            assert_eq!(
                                bindings.get(source_id, local.source_initializer()),
                                Some(local.typed_initializer())
                            );
                            assert!(store.scopes.contains(local.scope()));
                            assert_eq!(
                                store.symbols.get(local.symbol()).owner,
                                Some(proof.method())
                            );
                            assert_ne!(local.constructor(), proof.constructor());
                        } else {
                            let formal = argument.formal().expect(
                                "the corresponding ordered argument is the original method formal",
                            );
                            assert_eq!(formal.formal(), pair.method_formals()[0]);
                            assert_eq!(formal.source_argument(), argument.source_argument());
                            assert_eq!(formal.typed_argument(), argument.typed_argument());
                        }
                    }
                },
            );
        }
    }
}

#[test]
fn local_pair_proof_survives_fresh_sessions_renames_and_class_reordering() {
    use dotty::jvm_tasty_cross_class::ConstructorArgumentProofKind;

    for (source, method_name, local_first) in [
        (IMMUTABLE_LOCAL_PAIR_SOURCE, "create", false),
        (IMMUTABLE_LOCAL_PAIR_REVERSED_SOURCE, "createReversed", true),
    ] {
        let renamed = source
            .replace("PairFactory", "Builder")
            .replace(method_name, "assemble")
            .replace("fresh", "allocated");
        let counter = source.find("class Counter").unwrap();
        let pair = source.find("class Pair(").unwrap();
        let factory = source.find("class PairFactory:").unwrap();
        let reordered = format!(
            "{}{}{}{}",
            &source[..counter],
            &source[pair..factory],
            &source[counter..pair],
            &source[factory..]
        );
        for (variant, source_number) in [(&renamed, 20691), (&reordered, 20692)] {
            with_two_argument_proof(
                variant,
                source_number,
                |proof, _, _, _, _, _, _, _, _, _| {
                    assert_eq!(
                        proof.argument_proof().kind(),
                        ConstructorArgumentProofKind::FormalLocalPair
                    );
                    let arguments = proof.argument_proof().ordered_pair().unwrap().arguments();
                    let local_index = if local_first { 0 } else { 1 };
                    let formal_index = 1 - local_index;
                    assert!(arguments[local_index].local_val().is_some());
                    assert!(arguments[formal_index].formal().is_some());
                },
            );
        }
    }
}

#[test]
fn three_class_reference_constructor_proof_uses_canonical_symbols_and_exact_trees() {
    assert_generic_proof(SOURCE, 1794, "(Lexample/Counter;)Lexample/CounterOwner;");
}

#[test]
fn renamed_reordered_constructor_proof_ignores_class_and_method_names() {
    let renamed = SOURCE
        .replace("CounterOwnerFactory", "WidgetBuilder")
        .replace("CounterOwner", "WidgetHolder")
        .replace("Counter", "Widget")
        .replace("wrap", "build")
        .replace(
            "class Widget(start: Int):",
            "class Widget(start: Int):\n  def build(widget: Widget): Widget = widget",
        );
    let declarations = renamed.split("\nclass ").collect::<Vec<_>>();
    assert_eq!(declarations.len(), 4);
    let reordered = format!(
        "{}\nclass {}\nclass {}\nclass {}",
        declarations[0], declarations[3], declarations[2], declarations[1]
    );
    assert_generic_proof(&reordered, 1795, "(Lexample/Widget;)Lexample/WidgetHolder;");
}

#[test]
fn mixed_origin_pair_receipt_keeps_both_ordered_source_and_typed_origins() {
    with_two_argument_proof(
        MIXED_ORIGIN_SOURCE,
        20120,
        |proof,
         store,
         source_ast,
         typed_ast,
         _index,
         bindings,
         source_id,
         _root,
         definitions,
         unit| {
            use dotty::jvm_tasty_cross_class::ConstructorArgumentProofKind;

            assert_eq!(
                proof.argument_proof().kind(),
                ConstructorArgumentProofKind::FormalNestedPair
            );
            assert_eq!(
                proof.method_descriptor(),
                "(Lexample/Counter;I)Lexample/Pair;"
            );
            assert_eq!(
                proof.constructor_descriptor(),
                "(Lexample/Counter;Lexample/Counter;)V"
            );
            let ordered = proof.argument_proof().ordered_pair().unwrap();
            let method_formals = ordered.method_formals();
            let method_formal_trees = ordered.source_method_formals();
            assert_ne!(method_formals[0], method_formals[1]);
            assert!(matches!(
                store.types.get(ordered.method_formal_types()[0]),
                Type::TypeRef { .. }
            ));
            assert_eq!(ordered.method_formal_types()[1], definitions.int);
            let (proof_store, proof_source, proof_typed, _, proof_bindings, _) =
                proof.session_inputs();
            assert!(std::ptr::eq(proof_store, store));
            assert!(std::ptr::eq(proof_source, source_ast));
            assert!(std::ptr::eq(proof_typed, typed_ast));
            assert!(std::ptr::eq(proof_bindings, bindings));
            let source_method = proof.source_rhs();
            let TreeKind::Apply(outer) = &source_ast.get(source_method).kind else {
                panic!("factory RHS is the original outer Apply")
            };
            let typed_method = proof.typed_rhs();
            let TreeKind::Apply(typed_outer) = &typed_ast.get(typed_method).kind else {
                panic!("factory typed RHS preserves the outer Apply")
            };
            let arguments = ordered.arguments();
            assert_eq!(arguments.len(), 2);
            let formal = arguments[0]
                .formal()
                .expect("slot zero retains the original Counter formal");
            assert_eq!(formal.formal(), method_formals[0]);
            assert_eq!(formal.source_argument(), outer.args[0]);
            assert_eq!(formal.typed_argument(), typed_outer.args[0]);
            assert_eq!(
                bindings.get(source_id, outer.args[0]),
                Some(typed_outer.args[0])
            );
            assert!(arguments[0].nested_new().is_none());
            let nested = arguments[1]
                .nested_new()
                .expect("slot one retains the original nested Counter application");
            assert_eq!(nested.source_outer_argument(), outer.args[1]);
            assert_eq!(nested.typed_outer_argument(), typed_outer.args[1]);
            assert_eq!(nested.source_apply(), outer.args[1]);
            assert_eq!(nested.typed_apply(), typed_outer.args[1]);
            assert_eq!(
                bindings.get(source_id, outer.args[1]),
                Some(typed_outer.args[1])
            );
            assert_eq!(nested.argument_formal(), method_formals[1]);
            assert_eq!(
                nested.argument_formal_type(),
                ordered.method_formal_types()[1]
            );
            assert_eq!(nested.class(), proof.formal_class().unwrap());
            assert_ne!(method_formal_trees[0], method_formal_trees[1]);

            let TreeKind::Apply(inner_apply) = &source_ast.get(outer.args[1]).kind else {
                panic!("nested argument is the original Counter Apply")
            };
            let source_select = inner_apply.function;
            let TreeKind::Select(selection) = &source_ast.get(source_select).kind else {
                panic!("nested constructor function is the original Select")
            };
            let source_new = selection.qualifier;
            let source_leaf = inner_apply.args[0];
            let result_for =
                |typed_ast: &dotty_core::ast::TypedAst,
                 source_typed_index: &dotty_core::SourceTypedIndex| {
                    let jvm_unit = JvmCompilationUnit {
                        store,
                        typed_ast,
                        classes: unit.classes.clone(),
                    };
                    match validate_cross_class_constructor_call_proofs(
                        CrossClassConstructorCallProofInput {
                            source_id,
                            source_root: _root,
                            store,
                            definitions,
                            source_ast,
                            typed_ast,
                            source_index: _index,
                            source_typed_index,
                            jvm_unit: &jvm_unit,
                        },
                    ) {
                        Ok(proofs) => format!("Ok({} proofs)", proofs.len()),
                        Err(error) => format!("Err({error:?})"),
                    }
                };
            for (tree, expected) in [
                (
                    source_select,
                    "Err(TypedBinding(\"nested Counter Select has no exact typed binding\"))",
                ),
                (
                    source_new,
                    "Err(TypedBinding(\"nested Counter New has no exact typed binding\"))",
                ),
                (
                    source_leaf,
                    "Err(TypedBinding(\"nested Counter leaf has no exact typed binding\"))",
                ),
            ] {
                let mut missing_binding = bindings.clone();
                missing_binding.remove(source_id, tree).unwrap();
                assert_eq!(result_for(typed_ast, &missing_binding), expected);
            }

            let typed_leaf = bindings.get(source_id, source_leaf).unwrap();
            let mut disconnected_ast = clone_arena(typed_ast);
            let disconnected_leaf = disconnected_ast.alloc(typed_ast.get(typed_leaf).clone());
            let mut disconnected_binding = bindings.clone();
            disconnected_binding.remove(source_id, source_leaf).unwrap();
            disconnected_binding
                .insert(source_id, source_leaf, disconnected_leaf)
                .unwrap();
            assert_eq!(
                result_for(&disconnected_ast, &disconnected_binding),
                "Err(TypedBinding(\"nested Counter typed Apply must preserve exact original Select, New, and leaf roots\"))"
            );

            let mut forged_constructor_ast = clone_arena(typed_ast);
            let typed_nested_select = bindings.get(source_id, source_select).unwrap();
            let typed_outer_select = bindings.get(source_id, outer.function).unwrap();
            forged_constructor_ast.get_mut(typed_nested_select).ty =
                typed_ast.get(typed_outer_select).ty;
            assert_eq!(
                result_for(&forged_constructor_ast, bindings),
                "Err(SemanticIdentity(\"nested Select must choose a source constructor owned by the exact Counter class\"))"
            );

            let TreeKind::Select(outer_selection) = &source_ast.get(outer.function).kind else {
                panic!("outer constructor function is Select")
            };
            let TreeKind::New(outer_new) = &source_ast.get(outer_selection.qualifier).kind else {
                panic!("outer constructor receiver is New")
            };
            let typed_outer_type = bindings.get(source_id, outer_new.tpt).unwrap();
            let TreeKind::New(inner_new) = &source_ast.get(selection.qualifier).kind else {
                panic!("nested constructor receiver is New")
            };
            let typed_inner_type = bindings.get(source_id, inner_new.tpt).unwrap();
            let mut wrong_pair_type_ast = clone_arena(typed_ast);
            wrong_pair_type_ast.get_mut(typed_outer_type).ty = typed_ast.get(typed_inner_type).ty;
            assert_eq!(
                result_for(&wrong_pair_type_ast, bindings),
                "Err(SemanticIdentity(\"factory signature result must retain the exact Pair TypeRef\"))"
            );

            let mut foreign_formal_ast = clone_arena(typed_ast);
            foreign_formal_ast.get_mut(typed_outer.args[0]).ty =
                typed_ast.get(typed_nested_select).ty;
            assert_eq!(
                result_for(&foreign_formal_ast, bindings),
                "Err(TypedBinding(\"Pair argument TermRef is not a method formal\"))"
            );
        },
    );
}

#[test]
fn constructor_call_candidates_with_unsupported_argument_shapes_fail_closed() {
    let multiple = SOURCE
        .replace(
            "class CounterOwner(counter: Counter):",
            "class CounterOwner(counter: Counter, other: Counter):",
        )
        .replace(
            "new CounterOwner(counter)",
            "new CounterOwner(counter, counter)",
        );
    let multiple_error = with_paired_jvm_tasty_batch_plan(
        &multiple,
        "MultipleConstructorArgs.scala",
        SourceId::from_index(1796),
        |_| (),
    )
    .expect_err("multiple constructor arguments cannot skip the source proof");
    assert!(
        matches!(
            multiple_error,
            PairedPlanError::BatchUnit { phase: "cross-class constructor-call identity proof", ref reason }
                if reason.contains("one regular argument")
        ),
        "unexpected rejection: {multiple_error:?}"
    );

    let nested = SOURCE.replace(
        "new CounterOwner(counter)",
        "new CounterOwner(new Counter(1))",
    );
    let nested_error = with_paired_jvm_tasty_batch_plan(
        &nested,
        "NestedConstructorArg.scala",
        SourceId::from_index(1797),
        |_| (),
    )
    .expect_err("nested constructor arguments cannot skip the source proof");
    assert!(
        matches!(
            nested_error,
            PairedPlanError::BatchUnit { phase: "cross-class constructor-call identity proof", ref reason }
                if reason.contains("resolve directly to the caller formal")
        ),
        "unexpected rejection: {nested_error:?}"
    );

    let deeper = "package example\nclass SeedBox(seed: Int)\nclass Counter(seed: SeedBox)\nclass CounterOwner(counter: Counter)\nclass CounterOwnerFactory:\n  def create(seed: Int): CounterOwner =\n    new CounterOwner(new Counter(new SeedBox(seed)))\n";
    let deeper_error = with_paired_jvm_tasty_batch_plan(
        deeper,
        "ThirdDepthConstructorArg.scala",
        SourceId::from_index(1798),
        |_| (),
    )
    .expect_err("a third constructor depth cannot be treated as the formal leaf");
    assert!(
        matches!(
            deeper_error,
            PairedPlanError::BatchUnit { phase: "cross-class constructor-call identity proof", ref reason }
                if reason.contains("resolve directly to the caller formal")
        ),
        "unexpected third-depth rejection: {deeper_error:?}"
    );

    let conditional = SOURCE.replace(
        "new CounterOwner(counter)",
        "new CounterOwner(if counter == counter then counter else counter)",
    );
    let conditional_error = with_paired_jvm_tasty_batch_plan(
        &conditional,
        "ConditionalConstructorArg.scala",
        SourceId::from_index(1799),
        |_| (),
    )
    .expect_err("a conditional constructor argument cannot be treated as a formal");
    assert!(matches!(
        conditional_error,
        PairedPlanError::BatchUnit { phase: "cross-class constructor-call identity proof", ref reason }
            if reason.contains("constructor argument must be")
    ));

    let method_result = SOURCE
        .replace(
            "class CounterOwnerFactory:\n  def wrap(counter: Counter): CounterOwner =",
            "class CounterOwnerFactory:\n  def fromCounter(counter: Counter): Counter = counter\n  def wrap(counter: Counter): CounterOwner =",
        )
        .replace("new CounterOwner(counter)", "new CounterOwner(fromCounter(counter))");
    let method_result_error = with_paired_jvm_tasty_batch_plan(
        &method_result,
        "MethodResultConstructorArg.scala",
        SourceId::from_index(1800),
        |_| (),
    )
    .expect_err("a method-result constructor argument cannot be treated as a formal");
    assert!(
        matches!(
            method_result_error,
            PairedPlanError::BatchUnit { phase: "cross-class constructor-call identity proof", ref reason }
                if reason.contains("planned helper body has no established same-batch constructor proof")
        ),
        "unexpected method-result rejection: {method_result_error:?}"
    );
}
