use dotty::jvm_tasty_cross_class::{
    CrossClassConstructorCallProofInput, ValidatedCrossClassConstructorCallProof,
    validate_cross_class_constructor_call_proofs,
};
use dotty::jvm_tasty_plan::{PairedPlanError, with_paired_jvm_tasty_batch_plan};
use dotty_core::ast::{AstArena, AstPhase, TreeKind};
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceDefinition, SourceId, SourceSpan, SourceText,
    SymbolOrigin, TermRefTarget, Type, TypeRefTarget,
};
use dotty_jvm::{
    JvmClassInitializationPlan, JvmClassPlan, JvmCompilationUnit, JvmMethodPlan,
    JvmPrimaryConstructorPlan,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_typer::SourceTyper;

const SOURCE: &str =
    include_str!("fixtures/jvm_tasty_reference_constructor_calls/CounterOwnerFactory.scala");
const TWO_ARGUMENT_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_multi_argument_constructor_calls/PairFactory.scala");

fn clone_arena<P: AstPhase + Clone>(source: &AstArena<P>) -> AstArena<P> {
    let mut cloned = AstArena::new();
    for (_, tree) in source.iter() {
        cloned.alloc(tree.clone());
    }
    cloned
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
            if !matches!(parsed.ast.get(rhs).kind, TreeKind::Apply(_)) {
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
            method_plans.push((
                *class,
                JvmMethodPlan {
                    method,
                    parameters,
                    locals: Vec::new(),
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
        validate_cross_class_constructor_call_proofs(CrossClassConstructorCallProofInput {
            source_id,
            source_root: parsed.root,
            store: typer.store(),
            definitions: &definitions,
            source_ast: &parsed.ast,
            typed_ast: typer.typed_ast(),
            source_index: &source_index,
            source_typed_index: typer.source_typed_index(),
            jvm_unit: &jvm_unit,
        })
        .unwrap();
    assert_eq!(proofs.len(), 1, "one source factory body has one proof");
    consume(
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
    )
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
