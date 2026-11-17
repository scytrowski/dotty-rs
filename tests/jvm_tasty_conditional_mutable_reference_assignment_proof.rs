use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_batch;
use dotty::jvm_tasty_cross_class::{
    ConditionalMutableReferenceAssignmentProofInput,
    validate_conditional_mutable_reference_assignment_proof,
};
use dotty::jvm_tasty_plan::PairedPlanError;
use dotty_core::ast::{AstArena, TreeKind, Typed, Untyped};
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceId, SourceText, SourceTypedIndex, SymbolId,
    TermRefTarget,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_typer::{SourceTyper, TypedLocalBinding};

const SOURCE: &str = include_str!(
    "fixtures/jvm_tasty_conditional_mutable_reference_assignment/ConditionalMutableReferenceAssignment.scala"
);
#[derive(Clone, Copy)]
enum ExpectedControlRejection {
    SourceShape,
    Diagnostic(&'static str),
}

const CONTROLS: [(&str, &str, ExpectedControlRejection); 11] = [
    (
        "renamed locals",
        include_str!(
            "fixtures/jvm_tasty_conditional_mutable_reference_assignment/controls/RenamedLocals.scala"
        ),
        ExpectedControlRejection::SourceShape,
    ),
    (
        "reversed declarations",
        include_str!(
            "fixtures/jvm_tasty_conditional_mutable_reference_assignment/controls/ReversedDeclarations.scala"
        ),
        ExpectedControlRejection::SourceShape,
    ),
    (
        "immutable target",
        include_str!(
            "fixtures/jvm_tasty_conditional_mutable_reference_assignment/controls/ImmutableSelected.scala"
        ),
        ExpectedControlRejection::Diagnostic("AssignmentTargetImmutable"),
    ),
    (
        "foreign reference",
        include_str!(
            "fixtures/jvm_tasty_conditional_mutable_reference_assignment/controls/ForeignReference.scala"
        ),
        ExpectedControlRejection::Diagnostic("TermNameNotFound"),
    ),
    (
        "uninitialized reference",
        include_str!(
            "fixtures/jvm_tasty_conditional_mutable_reference_assignment/controls/UninitializedReference.scala"
        ),
        ExpectedControlRejection::Diagnostic("TermNameNotFound"),
    ),
    (
        "wrong type",
        include_str!(
            "fixtures/jvm_tasty_conditional_mutable_reference_assignment/controls/WrongType.scala"
        ),
        ExpectedControlRejection::Diagnostic("ExpectedExpressionTypeMismatch"),
    ),
    (
        "extra assignment",
        include_str!(
            "fixtures/jvm_tasty_conditional_mutable_reference_assignment/controls/ExtraAssignment.scala"
        ),
        ExpectedControlRejection::SourceShape,
    ),
    (
        "nested if",
        include_str!(
            "fixtures/jvm_tasty_conditional_mutable_reference_assignment/controls/NestedIf.scala"
        ),
        ExpectedControlRejection::SourceShape,
    ),
    (
        "allocation in branch",
        include_str!(
            "fixtures/jvm_tasty_conditional_mutable_reference_assignment/controls/AllocationInBranch.scala"
        ),
        ExpectedControlRejection::SourceShape,
    ),
    (
        "loop",
        include_str!(
            "fixtures/jvm_tasty_conditional_mutable_reference_assignment/controls/Loop.scala"
        ),
        ExpectedControlRejection::SourceShape,
    ),
    (
        "missing else",
        include_str!(
            "fixtures/jvm_tasty_conditional_mutable_reference_assignment/controls/MissingElse.scala"
        ),
        ExpectedControlRejection::SourceShape,
    ),
];

const SOURCE_SHAPE_REJECTION: &str = "SourceShape(\"four-formal candidate requires two initialized locals, selected IF, one setter, then Pair\")";

fn assert_control_diagnostic(
    error: &PairedPlanError,
    source_id: SourceId,
    expected_diagnostic: &str,
) {
    let PairedPlanError::BatchClass {
        class_index,
        declaration_index,
        phase,
        reason,
    } = error
    else {
        panic!("invalid control reaches method body typing: {error:?}");
    };
    assert_eq!(*class_index, 2, "the producer rejects PairFactory.create");
    assert_eq!(
        *declaration_index, 0,
        "the producer rejects the first PairFactory declaration"
    );
    assert_eq!(*phase, "method body typing");

    // BatchClass stores the TyperError debug form; pin its stable diagnostic and source while
    // leaving allocation-sensitive tree, symbol, name, and type IDs unconstrained.
    let (diagnostic, fields) = reason
        .split_once(" {")
        .unwrap_or_else(|| panic!("diagnostic has a structured payload: {reason}"));
    assert_eq!(diagnostic, expected_diagnostic);
    assert!(
        fields.contains(&format!("source: {source_id:?}")),
        "diagnostic belongs to this control source: {reason}"
    );
}

fn referenced_symbol(
    store: &SemanticStore,
    typed_ast: &AstArena<Typed>,
    tree: dotty_core::TreeId<Typed>,
) -> SymbolId {
    match store.types.get(typed_ast.get(tree).ty) {
        dotty_core::Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        } => *symbol,
        other => panic!("typed local reference has a symbol TermRef: {other:?}"),
    }
}

fn typed_tree(
    bindings: &SourceTypedIndex,
    source: SourceId,
    tree: dotty_core::TreeId<Untyped>,
) -> dotty_core::TreeId<Typed> {
    bindings
        .get(source, tree)
        .unwrap_or_else(|| panic!("source tree {:?} has an exact typed edge", tree))
}

fn copy_typed_arena(source: &AstArena<Typed>) -> AstArena<Typed> {
    let mut copy = AstArena::<Typed>::new();
    for (_, node) in source.iter() {
        copy.alloc(node.clone());
    }
    copy
}

fn validate_conditional_fixture(
    source: &str,
    source_id: SourceId,
) -> (
    dotty_tasty_pickler::ConditionalMutableReferenceAssignmentBinding,
    [SymbolId; 2],
) {
    let source_text = SourceText::new(source).unwrap();
    let scanner = ContextualScanner::new(source).unwrap();
    assert!(scanner.diagnostics().is_empty());
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = parse_compilation_unit(source_text, source_id, scanner, &mut store.names);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let source_index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        "ConditionalMutableReferenceAssignment-positive.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        outcome => panic!("source names without diagnostics: {outcome:?}"),
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("one package")
    };
    let mut declarations = Vec::new();
    let mut method_tree = None;
    let mut method_def = None;
    for &class_tree in &package.stats {
        let TreeKind::TypeDef(class_def) = &parsed.ast.get(class_tree).kind else {
            panic!("class declaration")
        };
        let class = source_index.symbol_at(source_id, class_tree).unwrap();
        let TreeKind::Template(template) = &parsed.ast.get(class_def.rhs).kind else {
            panic!("class template")
        };
        let constructor = source_index
            .symbol_at(source_id, template.constructor)
            .unwrap();
        let TreeKind::DefDef(constructor_def) = &parsed.ast.get(template.constructor).kind else {
            panic!("constructor")
        };
        let constructor_formals = constructor_def
            .value_param_clauses
            .iter()
            .flatten()
            .map(|tree| source_index.symbol_at(source_id, *tree).unwrap())
            .collect::<Vec<_>>();
        declarations.push((class, constructor, constructor_formals));
        for &member in &template.body {
            let TreeKind::DefDef(definition) = &parsed.ast.get(member).kind else {
                continue;
            };
            if store.names.resolve(definition.name.as_name().text()) == "create" {
                method_tree = Some(member);
                method_def = Some(definition.clone());
            }
        }
    }
    let method_tree = method_tree.expect("PairFactory.create");
    let method_def = method_def.unwrap();
    let method = source_index.symbol_at(source_id, method_tree).unwrap();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &source_index,
        &mut store,
        definitions.clone(),
        &packages,
    );
    for (class, constructor, formals) in &declarations {
        typer.complete_symbol(*class).unwrap();
        typer.complete_symbol(*constructor).unwrap();
        for formal in formals {
            typer.complete_symbol(*formal).unwrap();
        }
    }
    typer.complete_symbol(method).unwrap();
    for formal in method_def.value_param_clauses.iter().flatten() {
        typer
            .complete_symbol(source_index.symbol_at(source_id, *formal).unwrap())
            .unwrap();
    }
    let method_body = method_def.rhs.unwrap();
    let context = typer.expression_context_for(method).unwrap();
    typer.type_expression(method_body, context).unwrap();
    let TreeKind::Block(block) = &parsed.ast.get(method_body).kind else {
        panic!("method Block")
    };
    let bindings = [
        typer
            .typed_local_binding_at(source_id, block.stats[0])
            .unwrap(),
        typer
            .typed_local_binding_at(source_id, block.stats[1])
            .unwrap(),
        typer
            .typed_local_binding_at(source_id, block.stats[2])
            .unwrap(),
    ];
    let source_typed_index = typer.source_typed_index().clone();
    let typed_ast = copy_typed_arena(typer.typed_ast());
    drop(typer);
    let TreeKind::Apply(pair_apply) = &parsed.ast.get(block.expr).kind else {
        panic!("Pair result")
    };
    let pair_locals = [
        referenced_symbol(
            &store,
            &typed_ast,
            typed_tree(&source_typed_index, source_id, pair_apply.args[0]),
        ),
        referenced_symbol(
            &store,
            &typed_ast,
            typed_tree(&source_typed_index, source_id, pair_apply.args[1]),
        ),
    ];
    let jvm_unit = dotty_jvm::JvmCompilationUnit {
        store: &store,
        typed_ast: &typed_ast,
        classes: Vec::new(),
    };
    let proof = validate_conditional_mutable_reference_assignment_proof(
        ConditionalMutableReferenceAssignmentProofInput {
            source_id,
            method_tree,
            store: &store,
            definitions: &definitions,
            source_ast: &parsed.ast,
            typed_ast: &typed_ast,
            source_index: &source_index,
            source_typed_index: &source_typed_index,
            original_bindings: [Some(bindings[0]), Some(bindings[1])],
            selected_binding: Some(bindings[2]),
            jvm_unit: &jvm_unit,
        },
    )
    .expect("fixture has a canonical borrowed proof");
    (proof.binding(), pair_locals)
}

#[test]
fn source_if_preserves_both_assignment_identities_and_records_first_paired_rejection() {
    let source_id = SourceId::from_index(23_450);
    let source_text = SourceText::new(SOURCE).unwrap();
    let scanner = ContextualScanner::new(SOURCE).unwrap();
    assert!(scanner.diagnostics().is_empty());

    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let unit_type = definitions.unit;
    let parsed = parse_compilation_unit(source_text, source_id, scanner, &mut store.names);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let source_index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        "ConditionalMutableReferenceAssignment.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        outcome => panic!("source names without diagnostics: {outcome:?}"),
    };

    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("one package root")
    };
    let mut declarations = Vec::new();
    let mut method = None;
    for &class_tree in &package.stats {
        let TreeKind::TypeDef(class_def) = &parsed.ast.get(class_tree).kind else {
            panic!("top-level source declaration is a class")
        };
        let class = source_index.symbol_at(source_id, class_tree).unwrap();
        let TreeKind::Template(template) = &parsed.ast.get(class_def.rhs).kind else {
            panic!("class has a template")
        };
        let constructor = source_index
            .symbol_at(source_id, template.constructor)
            .unwrap();
        let TreeKind::DefDef(constructor_def) = &parsed.ast.get(template.constructor).kind else {
            panic!("class constructor is a definition")
        };
        let constructor_formals = constructor_def
            .value_param_clauses
            .iter()
            .flatten()
            .map(|tree| source_index.symbol_at(source_id, *tree).unwrap())
            .collect::<Vec<_>>();
        declarations.push((class, constructor, constructor_formals));
        for &member in &template.body {
            let TreeKind::DefDef(definition) = &parsed.ast.get(member).kind else {
                continue;
            };
            if store.names.resolve(definition.name.as_name().text()) == "create" {
                method = Some((member, definition.clone()));
            }
        }
    }
    let (method_tree, method_def) = method.expect("PairFactory.create is present");
    let method_symbol = source_index.symbol_at(source_id, method_tree).unwrap();

    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &source_index,
        &mut store,
        definitions,
        &packages,
    );
    for (class, constructor, formals) in &declarations {
        typer.complete_symbol(*class).unwrap();
        typer.complete_symbol(*constructor).unwrap();
        for formal in formals {
            typer.complete_symbol(*formal).unwrap();
        }
    }
    typer.complete_symbol(method_symbol).unwrap();
    for formal in method_def.value_param_clauses.iter().flatten() {
        typer
            .complete_symbol(source_index.symbol_at(source_id, *formal).unwrap())
            .unwrap();
    }
    let method_body = method_def.rhs.unwrap();
    let context = typer.expression_context_for(method_symbol).unwrap();
    typer.type_expression(method_body, context).unwrap();

    let TreeKind::Block(block) = &parsed.ast.get(method_body).kind else {
        panic!("PairFactory.create body is a block")
    };
    assert_eq!(block.stats.len(), 5);
    let stats = block.stats.clone();
    let result = block.expr;
    let first = typer.typed_local_binding_at(source_id, stats[0]).unwrap();
    let second = typer.typed_local_binding_at(source_id, stats[1]).unwrap();
    let selected = typer.typed_local_binding_at(source_id, stats[2]).unwrap();
    let locals: [TypedLocalBinding; 3] = [first, second, selected];
    let local_symbols = locals.map(|binding| binding.symbol());
    let local_typed_declarations = locals.map(|binding| binding.declaration());
    let typed_bindings = typer.source_typed_index().clone();
    let mut typed_ast = AstArena::<Typed>::new();
    for (_, node) in typer.typed_ast().iter() {
        typed_ast.alloc(node.clone());
    }
    drop(typer);

    assert_eq!(
        local_symbols.map(|symbol| store.symbols.get(symbol).owner),
        [
            Some(method_symbol),
            Some(method_symbol),
            Some(method_symbol)
        ]
    );
    assert_ne!(local_symbols[0], local_symbols[1]);
    assert_ne!(local_symbols[0], local_symbols[2]);
    assert_ne!(local_symbols[1], local_symbols[2]);
    for (declaration, typed_declaration) in stats[..3].iter().zip(local_typed_declarations) {
        assert_eq!(
            typed_tree(&typed_bindings, source_id, *declaration),
            typed_declaration
        );
    }

    let TreeKind::If(source_if) = &parsed.ast.get(stats[3]).kind else {
        panic!("the fourth source stat is the two-arm if")
    };
    let typed_if_id = typed_tree(&typed_bindings, source_id, stats[3]);
    let TreeKind::If(typed_if) = &typed_ast.get(typed_if_id).kind else {
        panic!("the source if maps to the same typed If shape")
    };
    assert_eq!(typed_ast.get(typed_if_id).ty, unit_type);
    assert_eq!(
        typed_if.cond,
        typed_tree(&typed_bindings, source_id, source_if.cond)
    );

    let branches = [source_if.then_branch, source_if.else_branch];
    let typed_branches = [typed_if.then_branch, typed_if.else_branch];
    let expected_rhs = [local_symbols[1], local_symbols[0]];
    for ((source_branch, typed_branch), expected) in
        branches.into_iter().zip(typed_branches).zip(expected_rhs)
    {
        let TreeKind::Assign(source_assign) = &parsed.ast.get(source_branch).kind else {
            panic!("both source if arms are assignments")
        };
        let source_lhs = source_assign.lhs;
        let source_rhs = source_assign.rhs;
        let TreeKind::Assign(typed_assign) = &typed_ast.get(typed_branch).kind else {
            panic!("both typed if arms remain assignments")
        };
        assert_eq!(
            typed_branch,
            typed_tree(&typed_bindings, source_id, source_branch),
            "each source Assign has an exact SourceTypedIndex edge to its branch"
        );
        assert_eq!(
            typed_assign.lhs,
            typed_tree(&typed_bindings, source_id, source_lhs)
        );
        assert_eq!(
            typed_assign.rhs,
            typed_tree(&typed_bindings, source_id, source_rhs)
        );
        assert_eq!(
            referenced_symbol(&store, &typed_ast, typed_assign.lhs),
            local_symbols[2],
            "both assignments target the canonical selected symbol"
        );
        assert_eq!(
            referenced_symbol(&store, &typed_ast, typed_assign.rhs),
            expected,
            "each assignment retains its branch-local canonical RHS identity"
        );
    }

    let TreeKind::Apply(setter_apply) = &parsed.ast.get(stats[4]).kind else {
        panic!("the setter call follows the conditional")
    };
    let TreeKind::Select(setter_select) = &parsed.ast.get(setter_apply.function).kind else {
        panic!("setter call selects set on the local receiver")
    };
    let typed_receiver = typed_tree(&typed_bindings, source_id, setter_select.qualifier);
    assert_eq!(
        referenced_symbol(&store, &typed_ast, typed_receiver),
        local_symbols[2]
    );

    let TreeKind::Apply(pair_apply) = &parsed.ast.get(result).kind else {
        panic!("the block result constructs Pair")
    };
    let pair_symbols = pair_apply
        .args
        .iter()
        .map(|argument| {
            referenced_symbol(
                &store,
                &typed_ast,
                typed_tree(&typed_bindings, source_id, *argument),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(pair_symbols, local_symbols[..2]);

    let jvm_unit = dotty_jvm::JvmCompilationUnit {
        store: &store,
        typed_ast: &typed_ast,
        classes: Vec::new(),
    };
    let proof = validate_conditional_mutable_reference_assignment_proof(
        ConditionalMutableReferenceAssignmentProofInput {
            source_id,
            method_tree,
            store: &store,
            definitions: &definitions,
            source_ast: &parsed.ast,
            typed_ast: &typed_ast,
            source_index: &source_index,
            source_typed_index: &typed_bindings,
            original_bindings: [Some(first), Some(second)],
            selected_binding: Some(selected),
            jvm_unit: &jvm_unit,
        },
    )
    .expect("the original source has a complete borrowed conditional-assignment proof");
    assert_eq!(proof.source_id(), source_id);
    assert_eq!(
        proof.owner(),
        store.symbols.get(method_symbol).owner.unwrap()
    );
    assert_eq!(proof.method(), method_symbol);
    assert_eq!(proof.selected(), local_symbols[2]);
    assert_eq!(proof.rhs_locals(), [local_symbols[1], local_symbols[0]]);
    let binding = proof.binding();
    assert_eq!(binding.source_assignments, branches);
    assert_eq!(binding.typed_assignments, typed_branches);
    assert_eq!(binding.local, local_symbols[2]);
    assert_eq!(binding.rhs_locals, [local_symbols[1], local_symbols[0]]);
    drop(proof);
    let reject_typed = |candidate: &AstArena<Typed>| {
        let candidate_unit = dotty_jvm::JvmCompilationUnit {
            store: &store,
            typed_ast: candidate,
            classes: Vec::new(),
        };
        validate_conditional_mutable_reference_assignment_proof(
            ConditionalMutableReferenceAssignmentProofInput {
                source_id,
                method_tree,
                store: &store,
                definitions: &definitions,
                source_ast: &parsed.ast,
                typed_ast: candidate,
                source_index: &source_index,
                source_typed_index: &typed_bindings,
                original_bindings: [Some(first), Some(second)],
                selected_binding: Some(selected),
                jvm_unit: &candidate_unit,
            },
        )
        .is_err()
    };

    // A source-to-typed table entry alone cannot authenticate a conditional
    // arm; the exact parent-child edge must agree as well.
    let mut missing_edge = typed_bindings.clone();
    missing_edge.remove(source_id, branches[0]);
    assert!(
        validate_conditional_mutable_reference_assignment_proof(
            ConditionalMutableReferenceAssignmentProofInput {
                source_id,
                method_tree,
                store: &store,
                definitions: &definitions,
                source_ast: &parsed.ast,
                typed_ast: &typed_ast,
                source_index: &source_index,
                source_typed_index: &missing_edge,
                original_bindings: [Some(first), Some(second)],
                selected_binding: Some(selected),
                jvm_unit: &jvm_unit,
            },
        )
        .is_err()
    );

    assert!(
        validate_conditional_mutable_reference_assignment_proof(
            ConditionalMutableReferenceAssignmentProofInput {
                source_id,
                method_tree,
                store: &store,
                definitions: &definitions,
                source_ast: &parsed.ast,
                typed_ast: &typed_ast,
                source_index: &source_index,
                source_typed_index: &typed_bindings,
                original_bindings: [Some(first), Some(second)],
                selected_binding: Some(first),
                jvm_unit: &jvm_unit,
            },
        )
        .is_err(),
        "a mismatched/immutable selected binding must reject"
    );

    let mut forged_edge = typed_bindings.clone();
    forged_edge.remove(source_id, branches[0]);
    forged_edge
        .insert(source_id, branches[0], typed_branches[1])
        .unwrap();
    assert!(
        validate_conditional_mutable_reference_assignment_proof(
            ConditionalMutableReferenceAssignmentProofInput {
                source_id,
                method_tree,
                store: &store,
                definitions: &definitions,
                source_ast: &parsed.ast,
                typed_ast: &typed_ast,
                source_index: &source_index,
                source_typed_index: &forged_edge,
                original_bindings: [Some(first), Some(second)],
                selected_binding: Some(selected),
                jvm_unit: &jvm_unit,
            },
        )
        .is_err()
    );

    let mut wrong_rhs_ast = copy_typed_arena(&typed_ast);
    let TreeKind::Assign(wrong_rhs_then) = &mut wrong_rhs_ast.get_mut(typed_branches[0]).kind
    else {
        unreachable!()
    };
    wrong_rhs_then.rhs = typed_tree(&typed_bindings, source_id, branches[1]);
    assert!(
        reject_typed(&wrong_rhs_ast),
        "a swapped/wrong branch RHS is rejected"
    );

    let mut wrong_pair_ast = copy_typed_arena(&typed_ast);
    let typed_pair = typed_tree(&typed_bindings, source_id, result);
    let TreeKind::Apply(wrong_pair_apply) = &mut wrong_pair_ast.get_mut(typed_pair).kind else {
        unreachable!()
    };
    wrong_pair_apply.args.swap(0, 1);
    assert!(
        reject_typed(&wrong_pair_ast),
        "Pair arguments keep source identity order"
    );

    let mut wrong_setter_ast = copy_typed_arena(&typed_ast);
    let typed_setter = typed_tree(&typed_bindings, source_id, stats[4]);
    let TreeKind::Apply(setter_call) = &wrong_setter_ast.get(typed_setter).kind else {
        unreachable!()
    };
    let typed_setter_function = setter_call.function;
    let TreeKind::Select(setter_selection) = &wrong_setter_ast.get(typed_setter_function).kind
    else {
        unreachable!()
    };
    let old_receiver = setter_selection.qualifier;
    let typed_first_pair_arg = typed_tree(&typed_bindings, source_id, pair_apply.args[0]);
    let TreeKind::Select(setter_selection) =
        &mut wrong_setter_ast.get_mut(typed_setter_function).kind
    else {
        unreachable!()
    };
    setter_selection.qualifier = typed_first_pair_arg;
    assert_ne!(old_receiver, typed_first_pair_arg);
    assert!(
        reject_typed(&wrong_setter_ast),
        "setter receiver must be selected"
    );

    let mut wrong_target_ast = copy_typed_arena(&typed_ast);
    let TreeKind::Assign(source_then_assignment) = &parsed.ast.get(branches[0]).kind else {
        unreachable!()
    };
    let typed_then_rhs = typed_tree(&typed_bindings, source_id, source_then_assignment.rhs);
    let TreeKind::Assign(then_assignment) = &mut wrong_target_ast.get_mut(typed_branches[0]).kind
    else {
        unreachable!()
    };
    then_assignment.lhs = typed_then_rhs;
    let wrong_target_unit = dotty_jvm::JvmCompilationUnit {
        store: &store,
        typed_ast: &wrong_target_ast,
        classes: Vec::new(),
    };
    assert!(
        validate_conditional_mutable_reference_assignment_proof(
            ConditionalMutableReferenceAssignmentProofInput {
                source_id,
                method_tree,
                store: &store,
                definitions: &definitions,
                source_ast: &parsed.ast,
                typed_ast: &wrong_target_ast,
                source_index: &source_index,
                source_typed_index: &typed_bindings,
                original_bindings: [Some(first), Some(second)],
                selected_binding: Some(selected),
                jvm_unit: &wrong_target_unit,
            },
        )
        .is_err()
    );

    let mut wrong_branch_order_ast = copy_typed_arena(&typed_ast);
    let TreeKind::If(wrong_branch_order_if) = &mut wrong_branch_order_ast.get_mut(typed_if_id).kind
    else {
        unreachable!()
    };
    std::mem::swap(
        &mut wrong_branch_order_if.then_branch,
        &mut wrong_branch_order_if.else_branch,
    );
    let wrong_branch_order_unit = dotty_jvm::JvmCompilationUnit {
        store: &store,
        typed_ast: &wrong_branch_order_ast,
        classes: Vec::new(),
    };
    assert!(
        validate_conditional_mutable_reference_assignment_proof(
            ConditionalMutableReferenceAssignmentProofInput {
                source_id,
                method_tree,
                store: &store,
                definitions: &definitions,
                source_ast: &parsed.ast,
                typed_ast: &wrong_branch_order_ast,
                source_index: &source_index,
                source_typed_index: &typed_bindings,
                original_bindings: [Some(first), Some(second)],
                selected_binding: Some(selected),
                jvm_unit: &wrong_branch_order_unit,
            },
        )
        .is_err()
    );

    let error = emit_paired_jvm_tasty_batch(
        SOURCE,
        "ConditionalMutableReferenceAssignment.scala",
        SourceId::from_index(23_451),
    )
    .expect_err("current paired producer must expose its measured first rejection");
    let dotty::jvm_tasty_artifacts::PairedBatchArtifactError::Plan(error) = error else {
        panic!("canonical candidate is rejected by the paired planner: {error:?}");
    };
    let PairedPlanError::BatchUnit { phase, reason } = error else {
        panic!("canonical candidate reaches a batch-unit rejection: {error:?}");
    };
    assert_eq!(phase, "cross-class constructor-call identity proof");
    assert_eq!(reason, SOURCE_SHAPE_REJECTION);
}

#[test]
fn proof_accepts_renamed_locals_and_reversed_declarations_without_reordering_semantics() {
    let support = SOURCE.split_once("class PairFactory:").unwrap().0;
    let renamed = include_str!(
        "fixtures/jvm_tasty_conditional_mutable_reference_assignment/controls/RenamedLocals.scala"
    );
    let reversed = include_str!(
        "fixtures/jvm_tasty_conditional_mutable_reference_assignment/controls/ReversedDeclarations.scala"
    );
    for (index, fixture) in [renamed, reversed].into_iter().enumerate() {
        let body = fixture.strip_prefix("package example\n\n").unwrap();
        let source = format!("{support}{body}");
        let (binding, pair_locals) =
            validate_conditional_fixture(&source, SourceId::from_index(23_480 + index as u32));
        assert_eq!(
            binding.rhs_locals,
            [pair_locals[1], pair_locals[0]],
            "THEN and ELSE identities remain semantic second then first"
        );
        assert_ne!(binding.rhs_locals[0], binding.rhs_locals[1]);
        assert_ne!(binding.source_assignments[0], binding.source_assignments[1]);
        assert_ne!(binding.typed_assignments[0], binding.typed_assignments[1]);
    }
}

#[test]
fn controls_classify_without_accepting_partial_paired_batches() {
    let support = SOURCE
        .split_once("class PairFactory:")
        .expect("canonical Counter/Pair support precedes PairFactory")
        .0;
    for (index, (name, source, expectation)) in CONTROLS.into_iter().enumerate() {
        let source = format!(
            "{support}{}",
            source
                .strip_prefix("package example\n\n")
                .expect("control fixture shares the example package")
        );
        let result = emit_paired_jvm_tasty_batch(
            &source,
            "ConditionalMutableReferenceAssignment-control.scala",
            SourceId::from_index(23_460 + index as u32),
        );
        let Err(error) = result else {
            panic!("control {name} unexpectedly returned a paired artifact batch")
        };
        if let ExpectedControlRejection::SourceShape = expectation {
            let dotty::jvm_tasty_artifacts::PairedBatchArtifactError::Plan(error) = error else {
                panic!("Scala-valid deferred control {name} reaches the paired planner: {error:?}");
            };
            let PairedPlanError::BatchUnit { phase, reason } = error else {
                panic!(
                    "Scala-valid deferred control {name} reaches the batch-unit gate: {error:?}"
                );
            };
            assert_eq!(
                phase, "cross-class constructor-call identity proof",
                "Scala-valid deferred control {name} reaches the measured stage"
            );
            assert_eq!(
                reason, SOURCE_SHAPE_REJECTION,
                "Scala-valid deferred control {name} reaches the measured source-shape rejection"
            );
        } else {
            let dotty::jvm_tasty_artifacts::PairedBatchArtifactError::Plan(error) = error else {
                panic!("Scala-invalid control {name} reaches the paired planner: {error:?}");
            };
            let ExpectedControlRejection::Diagnostic(expected_diagnostic) = expectation else {
                unreachable!("handled source-shape rejection above")
            };
            assert_control_diagnostic(
                &error,
                SourceId::from_index(23_460 + index as u32),
                expected_diagnostic,
            );
        }
    }
}
