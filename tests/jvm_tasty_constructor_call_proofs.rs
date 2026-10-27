use dotty::jvm_tasty_cross_class::{
    CrossClassConstructorCallProofInput, validate_cross_class_constructor_call_proofs,
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
                if reason.contains("exact caller formal identifier")
        ),
        "unexpected rejection: {nested_error:?}"
    );
}
