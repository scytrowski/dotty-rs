use dotty::jvm_tasty_artifacts::{PairedBatchArtifactError, emit_paired_jvm_tasty_batch};
use dotty::jvm_tasty_cross_class::{
    ConditionalMutableReferenceAssignmentProofInput,
    validate_conditional_mutable_reference_assignment_proof,
};
use dotty::jvm_tasty_plan::{PairedPlanError, with_paired_jvm_tasty_batch_plan};
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
enum ExpectedControl {
    Accepted,
    PlanFailure(&'static str),
    Diagnostic(&'static str),
}

const CONTROLS: [(&str, &str, ExpectedControl); 11] = [
    (
        "renamed locals",
        include_str!(
            "fixtures/jvm_tasty_conditional_mutable_reference_assignment/controls/RenamedLocals.scala"
        ),
        ExpectedControl::Accepted,
    ),
    (
        "reversed declarations",
        include_str!(
            "fixtures/jvm_tasty_conditional_mutable_reference_assignment/controls/ReversedDeclarations.scala"
        ),
        ExpectedControl::Accepted,
    ),
    (
        "immutable target",
        include_str!(
            "fixtures/jvm_tasty_conditional_mutable_reference_assignment/controls/ImmutableSelected.scala"
        ),
        ExpectedControl::Diagnostic("AssignmentTargetImmutable"),
    ),
    (
        "foreign reference",
        include_str!(
            "fixtures/jvm_tasty_conditional_mutable_reference_assignment/controls/ForeignReference.scala"
        ),
        ExpectedControl::Diagnostic("TermNameNotFound"),
    ),
    (
        "uninitialized reference",
        include_str!(
            "fixtures/jvm_tasty_conditional_mutable_reference_assignment/controls/UninitializedReference.scala"
        ),
        ExpectedControl::Diagnostic("TermNameNotFound"),
    ),
    (
        "wrong type",
        include_str!(
            "fixtures/jvm_tasty_conditional_mutable_reference_assignment/controls/WrongType.scala"
        ),
        ExpectedControl::Diagnostic("ExpectedExpressionTypeMismatch"),
    ),
    (
        "extra assignment",
        include_str!(
            "fixtures/jvm_tasty_conditional_mutable_reference_assignment/controls/ExtraAssignment.scala"
        ),
        ExpectedControl::PlanFailure(SOURCE_SHAPE_REJECTION),
    ),
    (
        "nested if",
        include_str!(
            "fixtures/jvm_tasty_conditional_mutable_reference_assignment/controls/NestedIf.scala"
        ),
        ExpectedControl::PlanFailure(
            "SourceShape(\"both original If arms must be direct Assign statements\")",
        ),
    ),
    (
        "allocation in branch",
        include_str!(
            "fixtures/jvm_tasty_conditional_mutable_reference_assignment/controls/AllocationInBranch.scala"
        ),
        ExpectedControl::PlanFailure(
            "TypedBinding(\"ordered Assign arms must target the same selected local with second then first RHS\")",
        ),
    ),
    (
        "loop",
        include_str!(
            "fixtures/jvm_tasty_conditional_mutable_reference_assignment/controls/Loop.scala"
        ),
        ExpectedControl::PlanFailure(SOURCE_SHAPE_REJECTION),
    ),
    (
        "missing else",
        include_str!(
            "fixtures/jvm_tasty_conditional_mutable_reference_assignment/controls/MissingElse.scala"
        ),
        ExpectedControl::PlanFailure(
            "SourceShape(\"both original If arms must be direct Assign statements\")",
        ),
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

fn copy_untyped_arena(source: &AstArena<Untyped>) -> AstArena<Untyped> {
    let mut copy = AstArena::<Untyped>::new();
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
fn source_if_preserves_both_assignment_identities_and_reaches_paired_contract() {
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
    let source_typed_count_before = typed_bindings.len();
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
    assert_eq!(typed_bindings.len(), source_typed_count_before);
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
    let reject_source = |candidate: &AstArena<Untyped>| {
        let candidate_unit = dotty_jvm::JvmCompilationUnit {
            store: &store,
            typed_ast: &typed_ast,
            classes: Vec::new(),
        };
        validate_conditional_mutable_reference_assignment_proof(
            ConditionalMutableReferenceAssignmentProofInput {
                source_id,
                method_tree,
                store: &store,
                definitions: &definitions,
                source_ast: candidate,
                typed_ast: &typed_ast,
                source_index: &source_index,
                source_typed_index: &typed_bindings,
                original_bindings: [Some(first), Some(second)],
                selected_binding: Some(selected),
                jvm_unit: &candidate_unit,
            },
        )
        .is_err()
    };

    let mut renamed_source_method = copy_untyped_arena(&parsed.ast);
    let TreeKind::DefDef(source_method) = &mut renamed_source_method.get_mut(method_tree).kind
    else {
        unreachable!()
    };
    source_method.name = dotty_core::TermName::new(store.symbols.get(local_symbols[0]).name.text());
    assert!(
        reject_source(&renamed_source_method),
        "source factory method name must match its canonical method symbol"
    );

    let TreeKind::ValDef(first_formal) = &parsed.ast.get(method_def.value_param_clauses[0][0]).kind
    else {
        unreachable!()
    };
    let TreeKind::Ident(first_formal_type) = &parsed.ast.get(first_formal.tpt).kind else {
        unreachable!()
    };
    let mut wrong_source_result_type = copy_untyped_arena(&parsed.ast);
    let TreeKind::Ident(source_result_type) =
        &mut wrong_source_result_type.get_mut(method_def.tpt).kind
    else {
        unreachable!()
    };
    source_result_type.name = first_formal_type.name;
    assert!(
        reject_source(&wrong_source_result_type),
        "source factory result annotation must remain the canonical Pair type"
    );

    let mut missing_source_result_position = copy_untyped_arena(&parsed.ast);
    missing_source_result_position
        .get_mut(method_def.tpt)
        .position = None;
    assert!(
        reject_source(&missing_source_result_position),
        "source factory result annotation must retain its original source position"
    );

    let TreeKind::Apply(pair_result_apply) = &parsed.ast.get(result).kind else {
        unreachable!()
    };
    let TreeKind::Select(pair_result_select) = &parsed.ast.get(pair_result_apply.function).kind
    else {
        unreachable!()
    };
    let TreeKind::New(pair_result_new) = &parsed.ast.get(pair_result_select.qualifier).kind else {
        unreachable!()
    };
    let TreeKind::Ident(pair_type_ident) = &parsed.ast.get(pair_result_new.tpt).kind else {
        unreachable!()
    };
    let pair_type_name = pair_type_ident.name;
    for (declaration_index, &declaration_tree) in stats.iter().take(3).enumerate() {
        let TreeKind::ValDef(original_definition) = &parsed.ast.get(declaration_tree).kind else {
            unreachable!()
        };
        let annotation_tree = original_definition.tpt;
        let mut wrong_source_annotation = copy_untyped_arena(&parsed.ast);
        let TreeKind::Ident(annotation) =
            &mut wrong_source_annotation.get_mut(annotation_tree).kind
        else {
            unreachable!()
        };
        annotation.name = pair_type_name;
        assert!(
            reject_source(&wrong_source_annotation),
            "source Counter declaration {declaration_index} must reject a Pair annotation"
        );
    }

    for (declaration_index, &declaration_tree) in stats.iter().take(2).enumerate() {
        let TreeKind::ValDef(definition) = &parsed.ast.get(declaration_tree).kind else {
            unreachable!()
        };
        let initializer = definition.rhs.unwrap();
        let TreeKind::Apply(apply) = &parsed.ast.get(initializer).kind else {
            unreachable!()
        };
        let TreeKind::Select(select) = &parsed.ast.get(apply.function).kind else {
            unreachable!()
        };
        let TreeKind::New(new) = &parsed.ast.get(select.qualifier).kind else {
            unreachable!()
        };
        let mut wrong_allocation_type = copy_untyped_arena(&parsed.ast);
        let TreeKind::Ident(allocation_type) = &mut wrong_allocation_type.get_mut(new.tpt).kind
        else {
            unreachable!()
        };
        allocation_type.name = pair_type_name;
        assert!(
            reject_source(&wrong_allocation_type),
            "original Counter initializer {declaration_index} must reject a Pair allocation"
        );
    }

    for (formal_index, &formal_tree) in method_def.value_param_clauses[0].iter().enumerate() {
        let TreeKind::ValDef(formal) = &parsed.ast.get(formal_tree).kind else {
            unreachable!()
        };
        let formal_type_tree = formal.tpt;

        let mut wrong_formal_name = copy_untyped_arena(&parsed.ast);
        let TreeKind::ValDef(formal) = &mut wrong_formal_name.get_mut(formal_tree).kind else {
            unreachable!()
        };
        formal.name = dotty_core::TermName::new(store.symbols.get(local_symbols[0]).name.text());
        assert!(
            reject_source(&wrong_formal_name),
            "source formal {formal_index} must retain its canonical parameter name"
        );

        let mut wrong_formal_type = copy_untyped_arena(&parsed.ast);
        let TreeKind::Ident(formal_type) = &mut wrong_formal_type.get_mut(formal_type_tree).kind
        else {
            unreachable!()
        };
        formal_type.name = pair_type_name;
        assert!(
            reject_source(&wrong_formal_type),
            "source formal {formal_index} must retain its canonical signature type"
        );
    }

    let mut source_without_var = copy_untyped_arena(&parsed.ast);
    let TreeKind::ValDef(selected_without_var) = &mut source_without_var.get_mut(stats[2]).kind
    else {
        unreachable!()
    };
    selected_without_var
        .metadata
        .modifiers
        .retain(|modifier| *modifier != dotty_core::ast::Modifier::Var);
    assert!(
        reject_source(&source_without_var),
        "semantic mutability cannot substitute for source `var` metadata"
    );

    let mut first_source_is_var = copy_untyped_arena(&parsed.ast);
    let TreeKind::ValDef(first_with_var) = &mut first_source_is_var.get_mut(stats[0]).kind else {
        unreachable!()
    };
    first_with_var
        .metadata
        .modifiers
        .push(dotty_core::ast::Modifier::Var);
    assert!(
        reject_source(&first_source_is_var),
        "the first original Counter local must remain immutable in source syntax"
    );

    let mut second_source_is_var = copy_untyped_arena(&parsed.ast);
    let TreeKind::ValDef(second_with_var) = &mut second_source_is_var.get_mut(stats[1]).kind else {
        unreachable!()
    };
    second_with_var
        .metadata
        .modifiers
        .push(dotty_core::ast::Modifier::Var);
    assert!(
        reject_source(&second_source_is_var),
        "the second original Counter local must remain immutable in source syntax"
    );

    let mut renamed_typed_selected = copy_typed_arena(&typed_ast);
    let typed_selected = typed_tree(&typed_bindings, source_id, stats[2]);
    let TreeKind::ValDef(typed_selected_def) =
        &mut renamed_typed_selected.get_mut(typed_selected).kind
    else {
        unreachable!()
    };
    typed_selected_def.name =
        dotty_core::TermName::new(store.symbols.get(local_symbols[0]).name.text());
    assert!(
        reject_typed(&renamed_typed_selected),
        "typed selected declaration name must match its canonical selected symbol"
    );

    let mut renamed_typed_condition = copy_typed_arena(&typed_ast);
    let TreeKind::Ident(condition) = &mut renamed_typed_condition.get_mut(typed_if.cond).kind
    else {
        unreachable!()
    };
    condition.name = store.symbols.get(local_symbols[0]).name;
    assert!(
        reject_typed(&renamed_typed_condition),
        "typed If condition must retain the canonical Boolean formal name"
    );

    let mut renamed_typed_first = copy_typed_arena(&typed_ast);
    let TreeKind::ValDef(typed_first) = &mut renamed_typed_first
        .get_mut(local_typed_declarations[0])
        .kind
    else {
        unreachable!()
    };
    typed_first.name = dotty_core::TermName::new(store.symbols.get(local_symbols[1]).name.text());
    assert!(
        reject_typed(&renamed_typed_first),
        "typed first Counter declaration name must match its canonical local"
    );

    let mut renamed_typed_second = copy_typed_arena(&typed_ast);
    let TreeKind::ValDef(typed_second) = &mut renamed_typed_second
        .get_mut(local_typed_declarations[1])
        .kind
    else {
        unreachable!()
    };
    typed_second.name = dotty_core::TermName::new(store.symbols.get(local_symbols[0]).name.text());
    assert!(
        reject_typed(&renamed_typed_second),
        "typed second Counter declaration name must match its canonical local"
    );

    let TreeKind::Apply(source_setter_apply) = &parsed.ast.get(stats[4]).kind else {
        unreachable!()
    };
    let mut renamed_source_setter = copy_untyped_arena(&parsed.ast);
    let TreeKind::Select(source_setter_select) = &mut renamed_source_setter
        .get_mut(source_setter_apply.function)
        .kind
    else {
        unreachable!()
    };
    source_setter_select.name = store.symbols.get(local_symbols[0]).name;
    assert!(
        reject_source(&renamed_source_setter),
        "source setter name must retain the canonical set method identity"
    );

    let mut redirected_source_setter = copy_untyped_arena(&parsed.ast);
    let TreeKind::Select(source_setter_select) = &mut redirected_source_setter
        .get_mut(source_setter_apply.function)
        .kind
    else {
        unreachable!()
    };
    let TreeKind::Block(source_body) = &parsed.ast.get(method_def.rhs.unwrap()).kind else {
        unreachable!()
    };
    let TreeKind::Apply(source_pair_apply) = &parsed.ast.get(source_body.expr).kind else {
        unreachable!()
    };
    source_setter_select.qualifier = source_pair_apply.args[0];
    assert!(
        reject_source(&redirected_source_setter),
        "source setter receiver must retain the canonical selected local identity"
    );

    let mut redirected_first_pair_argument = copy_untyped_arena(&parsed.ast);
    let TreeKind::Apply(source_pair_apply) = &mut redirected_first_pair_argument
        .get_mut(source_body.expr)
        .kind
    else {
        unreachable!()
    };
    source_pair_apply.args[0] = source_pair_apply.args[1];
    assert!(
        reject_source(&redirected_first_pair_argument),
        "first source Pair argument must retain the first canonical original local"
    );

    let TreeKind::Select(source_setter_select) = &parsed.ast.get(source_setter_apply.function).kind
    else {
        unreachable!()
    };
    let reference_edges = [
        (
            source_setter_apply.function,
            typed_tree(&typed_bindings, source_id, source_setter_apply.function),
            "setter function",
        ),
        (
            source_setter_select.qualifier,
            typed_tree(&typed_bindings, source_id, source_setter_select.qualifier),
            "setter receiver",
        ),
        (
            source_setter_apply.args[0],
            typed_tree(&typed_bindings, source_id, source_setter_apply.args[0]),
            "setter argument",
        ),
        (
            pair_apply.args[0],
            typed_tree(&typed_bindings, source_id, pair_apply.args[0]),
            "first Pair argument",
        ),
        (
            pair_apply.args[1],
            typed_tree(&typed_bindings, source_id, pair_apply.args[1]),
            "second Pair argument",
        ),
    ];
    for (source_reference, typed_reference, description) in reference_edges {
        let mut source_without_position = copy_untyped_arena(&parsed.ast);
        source_without_position.get_mut(source_reference).position = None;
        assert!(
            reject_source(&source_without_position),
            "source {description} must retain its original source position"
        );

        let mut typed_without_position = copy_typed_arena(&typed_ast);
        typed_without_position.get_mut(typed_reference).position = None;
        assert!(
            reject_typed(&typed_without_position),
            "typed {description} must retain its exact mapped source position"
        );
    }

    let mut local_provenance_trees = Vec::new();
    for &declaration in stats.iter().take(2) {
        let TreeKind::ValDef(local_definition) = &parsed.ast.get(declaration).kind else {
            unreachable!()
        };
        let initializer = local_definition.rhs.unwrap();
        let TreeKind::Apply(initializer_apply) = &parsed.ast.get(initializer).kind else {
            unreachable!()
        };
        let TreeKind::Select(initializer_select) = &parsed.ast.get(initializer_apply.function).kind
        else {
            unreachable!()
        };
        let TreeKind::New(initializer_new) = &parsed.ast.get(initializer_select.qualifier).kind
        else {
            unreachable!()
        };
        local_provenance_trees.extend([
            (declaration, "original Counter declaration"),
            (local_definition.tpt, "original Counter annotation"),
            (initializer, "original Counter initializer"),
            (initializer_apply.function, "Counter constructor Select"),
            (initializer_select.qualifier, "Counter constructor New"),
            (initializer_new.tpt, "Counter constructor type tree"),
            (initializer_apply.args[0], "Counter constructor argument"),
        ]);
    }
    let TreeKind::ValDef(selected_source_definition) = &parsed.ast.get(stats[2]).kind else {
        unreachable!()
    };
    local_provenance_trees.push((
        selected_source_definition.tpt,
        "selected Counter annotation",
    ));
    for (source_tree, description) in local_provenance_trees {
        let typed_tree_id = typed_tree(&typed_bindings, source_id, source_tree);
        let mut source_without_position = copy_untyped_arena(&parsed.ast);
        source_without_position.get_mut(source_tree).position = None;
        assert!(
            reject_source(&source_without_position),
            "source {description} must retain its original source position"
        );

        let mut typed_without_position = copy_typed_arena(&typed_ast);
        typed_without_position.get_mut(typed_tree_id).position = None;
        assert!(
            reject_typed(&typed_without_position),
            "typed {description} must retain its exact mapped source position"
        );
    }

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

    let mut source_pair_using = copy_untyped_arena(&parsed.ast);
    let TreeKind::Apply(source_pair_apply) = &mut source_pair_using.get_mut(result).kind else {
        unreachable!()
    };
    source_pair_apply.kind = dotty_core::ast::ApplyKind::Using;
    assert!(
        reject_source(&source_pair_using),
        "source Pair construction must remain a Regular Apply"
    );

    let mut typed_pair_using = copy_typed_arena(&typed_ast);
    let TreeKind::Apply(typed_pair_apply) = &mut typed_pair_using.get_mut(typed_pair).kind else {
        unreachable!()
    };
    typed_pair_apply.kind = dotty_core::ast::ApplyKind::Using;
    assert!(
        reject_typed(&typed_pair_using),
        "typed Pair construction must match the Regular source Apply"
    );

    let typed_pair_function = match &typed_ast.get(typed_pair).kind {
        TreeKind::Apply(apply) => apply.function,
        _ => unreachable!(),
    };
    let typed_pair_new = match &typed_ast.get(typed_pair_function).kind {
        TreeKind::Select(select) => select.qualifier,
        _ => unreachable!(),
    };
    let mut redirected_pair_qualifier = copy_typed_arena(&typed_ast);
    let TreeKind::Select(pair_select) =
        &mut redirected_pair_qualifier.get_mut(typed_pair_function).kind
    else {
        unreachable!()
    };
    pair_select.qualifier = typed_tree(&typed_bindings, source_id, pair_apply.args[0]);
    assert!(
        reject_typed(&redirected_pair_qualifier),
        "typed Pair constructor selection cannot redirect its qualifier to a local receiver"
    );

    let mut redirected_pair_type = copy_typed_arena(&typed_ast);
    let TreeKind::New(pair_new) = &mut redirected_pair_type.get_mut(typed_pair_new).kind else {
        unreachable!()
    };
    pair_new.tpt = typed_tree(&typed_bindings, source_id, stats[2]);
    assert!(
        reject_typed(&redirected_pair_type),
        "typed Pair allocation retains the exact source mapped Pair type tree"
    );

    let mut missing_pair_new_mapping = typed_bindings.clone();
    let TreeKind::Select(source_pair_select) = &parsed.ast.get(pair_apply.function).kind else {
        unreachable!()
    };
    missing_pair_new_mapping.remove(source_id, source_pair_select.qualifier);
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
                source_typed_index: &missing_pair_new_mapping,
                original_bindings: [Some(first), Some(second)],
                selected_binding: Some(selected),
                jvm_unit: &jvm_unit,
            },
        )
        .is_err(),
        "Pair New requires its exact source-to-typed mapping"
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

    let mut non_unit_if_ast = copy_typed_arena(&typed_ast);
    non_unit_if_ast.get_mut(typed_if_id).ty = definitions.int;
    assert!(
        reject_typed(&non_unit_if_ast),
        "the original If must have canonical Unit type"
    );

    let typed_block = typed_tree(&typed_bindings, source_id, method_def.rhs.unwrap());
    let mut wrong_block_type = copy_typed_arena(&typed_ast);
    wrong_block_type.get_mut(typed_block).ty = definitions.int;
    assert!(
        reject_typed(&wrong_block_type),
        "typed Block must retain the canonical factory result type"
    );

    let mut wrong_setter_type = copy_typed_arena(&typed_ast);
    wrong_setter_type.get_mut(typed_setter).ty = definitions.int;
    assert!(
        reject_typed(&wrong_setter_type),
        "typed setter Apply must retain canonical Unit type"
    );

    let mut wrong_setter_kind = copy_typed_arena(&typed_ast);
    let TreeKind::Apply(setter_apply) = &mut wrong_setter_kind.get_mut(typed_setter).kind else {
        unreachable!()
    };
    setter_apply.kind = dotty_core::ast::ApplyKind::Using;
    assert!(
        reject_typed(&wrong_setter_kind),
        "typed setter Apply must preserve the source Regular call kind"
    );

    let mut duplicate_arm_source = copy_untyped_arena(&parsed.ast);
    let TreeKind::If(duplicate_arm_if) = &mut duplicate_arm_source.get_mut(stats[3]).kind else {
        unreachable!()
    };
    duplicate_arm_if.else_branch = duplicate_arm_if.then_branch;
    assert!(
        validate_conditional_mutable_reference_assignment_proof(
            ConditionalMutableReferenceAssignmentProofInput {
                source_id,
                method_tree,
                store: &store,
                definitions: &definitions,
                source_ast: &duplicate_arm_source,
                typed_ast: &typed_ast,
                source_index: &source_index,
                source_typed_index: &typed_bindings,
                original_bindings: [Some(first), Some(second)],
                selected_binding: Some(selected),
                jvm_unit: &jvm_unit,
            },
        )
        .is_err(),
        "missing/duplicated If arm cannot be authenticated"
    );

    let wrong_owner_method = package
        .stats
        .iter()
        .find_map(|class_tree| {
            let TreeKind::TypeDef(class_def) = &parsed.ast.get(*class_tree).kind else {
                return None;
            };
            let TreeKind::Template(template) = &parsed.ast.get(class_def.rhs).kind else {
                return None;
            };
            template.body.iter().find_map(|member| {
                let TreeKind::DefDef(definition) = &parsed.ast.get(*member).kind else {
                    return None;
                };
                (store.names.resolve(definition.name.as_name().text()) == "set").then_some(*member)
            })
        })
        .expect("Counter.set exists as a foreign-owner control");
    assert!(
        validate_conditional_mutable_reference_assignment_proof(
            ConditionalMutableReferenceAssignmentProofInput {
                source_id,
                method_tree: wrong_owner_method,
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
        .is_err(),
        "a foreign method owner is rejected"
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

    let mut found = false;
    with_paired_jvm_tasty_batch_plan(
        SOURCE,
        "ConditionalMutableReferenceAssignment.scala",
        SourceId::from_index(23_451),
        |plan| {
            found = plan
                .constructor_call_proofs
                .iter()
                .any(|proof| proof.conditional_mutable_reference_assignment().is_some());
        },
    )
    .expect("the frozen source validates through the paired structural contract");
    assert!(
        found,
        "the paired plan retains the canonical assignment proof"
    );
}

#[test]
fn paired_batch_artifact_producer_defers_conditional_assignment_to_issue_2352() {
    let error = emit_paired_jvm_tasty_batch(
        SOURCE,
        "ConditionalMutableReferenceAssignment.scala",
        SourceId::from_index(23_452),
    )
    .expect_err("#2351 validates the shape but defers full artifact production to #2352");
    let PairedBatchArtifactError::Plan(PairedPlanError::BatchUnit { phase, reason }) = error else {
        panic!("conditional artifact production is rejected at its admission gate: {error:?}");
    };
    assert_eq!(phase, "artifact producer admission");
    assert_eq!(
        reason,
        "conditional mutable assignment artifacts are deferred to issue #2352"
    );
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
        with_paired_jvm_tasty_batch_plan(
            &source,
            "ConditionalMutableReferenceAssignment-control.scala",
            SourceId::from_index(23_485 + index as u32),
            |_| {},
        )
        .expect("renaming or reversing declarations preserves semantic role proofs");
    }
}

#[test]
fn controls_require_the_complete_paired_contract_without_publishing_artifacts() {
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
        let result = with_paired_jvm_tasty_batch_plan(
            &source,
            "ConditionalMutableReferenceAssignment-control.scala",
            SourceId::from_index(23_460 + index as u32),
            |_| {},
        );
        match expectation {
            ExpectedControl::Accepted => {
                result.unwrap_or_else(|error| panic!("valid control {name} is accepted: {error:?}"))
            }
            ExpectedControl::PlanFailure(expected_reason) => {
                let Err(error) = result else {
                    panic!("out-of-scope control {name} unexpectedly validates")
                };
                let PairedPlanError::BatchUnit { phase, reason } = error else {
                    panic!("out-of-scope control {name} reaches the batch-unit gate: {error:?}");
                };
                assert_eq!(
                    phase, "cross-class constructor-call identity proof",
                    "control {name} is rejected before contract acceptance"
                );
                assert_eq!(reason, expected_reason, "control {name}");
            }
            ExpectedControl::Diagnostic(expected_diagnostic) => {
                let Err(error) = result else {
                    panic!("Scala-invalid control {name} unexpectedly validates")
                };
                assert_control_diagnostic(
                    &error,
                    SourceId::from_index(23_460 + index as u32),
                    expected_diagnostic,
                );
            }
        }
    }
}
