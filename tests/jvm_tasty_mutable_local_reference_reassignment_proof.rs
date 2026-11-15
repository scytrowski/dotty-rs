use dotty::jvm_tasty_cross_class::{
    CrossClassConstructorCallProofError, MutableLocalReferenceReassignmentProofInput,
    validate_mutable_local_reference_reassignment_proof,
};
use dotty_core::ast::{AstArena, TreeKind, Typed};
use dotty_core::{Definitions, Packages, SemanticStore, SourceId, SourceText};
use dotty_jvm::JvmCompilationUnit;
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_typer::{SourceTyper, TypedLocalBinding};

const SOURCE: &str =
    include_str!("fixtures/jvm_tasty_mutable_reference_rebinding/MutableReferenceRebinding.scala");

fn stale_symbol_id(store: &SemanticStore) -> dotty_core::SymbolId {
    let mut foreign = SemanticStore::new();
    loop {
        let name = foreign.names.intern("stale");
        let symbol = dotty_core::Symbol {
            name: dotty_core::Name::new(name, dotty_core::Namespace::Term),
            owner: None,
            kind: dotty_core::SymbolKind::Value,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: dotty_core::SymbolInfo::Missing,
            origin: dotty_core::SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        };
        let id = foreign.symbols.alloc(symbol);
        if !store.symbols.contains(id) {
            return id;
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Mutation {
    None,
    MissingAssignment,
    MissingRhs,
    SwapAssignmentOperands,
    WrongBinding,
    MissingSelectedBinding,
    ImmutableSelected,
    WrongOwner,
    WrongScope,
    WrongInitializer,
    MissingSelectedAnnotation,
    WrongSelectedAnnotation,
    SelectedInitializerNotIdent,
    WrongAssignmentTarget,
    StaleAssignmentTarget,
    WrongSetterReceiver,
    WrongPairArguments,
    StalePairConstructor,
    WrongResult,
    ReorderStatements,
    DuplicateAssignment,
    ForeignAssignedType,
    MissingSourceBlockPosition,
    MissingSourceSelectedPosition,
    ForeignSourceSelectedPosition,
    MissingTypedAssignmentPosition,
    MismatchedTypedAssignmentPosition,
}

#[derive(Debug)]
struct Receipt {
    selected: dotty_core::SymbolId,
    target: dotty_core::SymbolId,
    first: dotty_core::SymbolId,
    second: dotty_core::SymbolId,
    assignment: dotty_core::TreeId<dotty_core::Untyped>,
    typed_assignment: dotty_core::TreeId<Typed>,
    setter_receiver: dotty_core::TreeId<dotty_core::Untyped>,
    pair_args: [dotty_core::TreeId<dotty_core::Untyped>; 2],
}

fn validate(
    source: &str,
    source_number: u32,
    mutation: Mutation,
) -> Result<Receipt, CrossClassConstructorCallProofError> {
    let source_id = SourceId::from_index(source_number);
    let source_text = SourceText::new(source).unwrap();
    let scanner = ContextualScanner::new(source).unwrap();
    assert!(scanner.diagnostics().is_empty());
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut parsed = parse_compilation_unit(source_text, source_id, scanner, &mut store.names);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let source_index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        "mutable-reference-rebinding.scala",
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
        panic!("package root")
    };
    let mut classes = Vec::new();
    let mut method_tree = None;
    for &class_tree in &package.stats {
        let TreeKind::TypeDef(class_def) = &parsed.ast.get(class_tree).kind else {
            continue;
        };
        let class = source_index.symbol_at(source_id, class_tree).unwrap();
        let TreeKind::Template(template) = &parsed.ast.get(class_def.rhs).kind else {
            panic!("template")
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
        classes.push((class, constructor, constructor_formals));
        for &member in &template.body {
            let TreeKind::DefDef(def) = &parsed.ast.get(member).kind else {
                continue;
            };
            if store.names.resolve(def.name.as_name().text()) == "create" {
                method_tree = Some((member, def.clone()));
            }
        }
    }
    let (method_tree, method_def) = method_tree.expect("PairFactory.create");
    let method = source_index.symbol_at(source_id, method_tree).unwrap();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &source_index,
        &mut store,
        definitions,
        &packages,
    );
    for (class, constructor, formals) in &classes {
        typer.complete_symbol(*class).unwrap();
        typer.complete_symbol(*constructor).unwrap();
        for formal in formals {
            typer.complete_symbol(*formal).unwrap();
        }
    }
    typer.complete_symbol(method).unwrap();
    for tree in method_def.value_param_clauses.iter().flatten() {
        typer
            .complete_symbol(source_index.symbol_at(source_id, *tree).unwrap())
            .unwrap();
    }
    let rhs = method_def.rhs.unwrap();
    let context = typer.expression_context_for(method).unwrap();
    typer.type_expression(rhs, context).unwrap();
    let TreeKind::Block(block) = &parsed.ast.get(rhs).kind else {
        panic!("factory Block")
    };
    assert_eq!(block.stats.len(), 5);
    let block_stats = block.stats.clone();
    let block_expr = block.expr;
    let local_bindings: [TypedLocalBinding; 3] = block.stats[..3]
        .iter()
        .map(|tree| {
            typer
                .typed_local_binding_at(source_id, *tree)
                .expect("typed local binding")
        })
        .collect::<Vec<_>>()
        .try_into()
        .unwrap();
    let mut source_typed_index = typer.source_typed_index().clone();
    let mut typed_ast = AstArena::<Typed>::new();
    for (_, node) in typer.typed_ast().iter() {
        typed_ast.alloc(node.clone());
    }
    let assignment = block.stats[3];
    let selected = local_bindings[2].symbol();
    let selected_scope = local_bindings[2].scope();
    let typed_assignment = source_typed_index.get(source_id, assignment).unwrap();
    let (source_assignment_lhs, source_assignment_rhs) = {
        let TreeKind::Assign(assign) = &parsed.ast.get(assignment).kind else {
            panic!("assignment")
        };
        (assign.lhs, assign.rhs)
    };
    drop(typer);

    let mut supplied_selected = Some(local_bindings[2]);
    match mutation {
        Mutation::None => {}
        Mutation::MissingAssignment => {
            source_typed_index.remove(source_id, assignment);
        }
        Mutation::MissingRhs => {
            source_typed_index.remove(source_id, source_assignment_rhs);
        }
        Mutation::SwapAssignmentOperands => {
            let TreeKind::Assign(assign) = &mut parsed.ast.get_mut(assignment).kind else {
                unreachable!()
            };
            std::mem::swap(&mut assign.lhs, &mut assign.rhs);
        }
        Mutation::WrongBinding => {
            supplied_selected = Some(local_bindings[0]);
        }
        Mutation::MissingSelectedBinding => supplied_selected = None,
        Mutation::ImmutableSelected => {
            store.symbols.get_mut(selected).flags = store
                .symbols
                .get(selected)
                .flags
                .difference(dotty_core::SymbolFlags::MUTABLE);
        }
        Mutation::WrongOwner => {
            let owner = store.symbols.get(method).owner.unwrap();
            store.symbols.get_mut(selected).owner = Some(owner);
        }
        Mutation::WrongScope => {
            store.scopes.get_mut(selected_scope).owner = Some(selected);
        }
        Mutation::WrongInitializer => {
            let second_initializer = match &parsed.ast.get(block_stats[1]).kind {
                TreeKind::ValDef(def) => def.rhs,
                _ => unreachable!(),
            };
            let TreeKind::ValDef(selected_def) = &mut parsed.ast.get_mut(block_stats[2]).kind
            else {
                unreachable!()
            };
            selected_def.rhs = second_initializer;
        }
        Mutation::MissingSelectedAnnotation => {
            let selected_type = match &parsed.ast.get(block_stats[2]).kind {
                TreeKind::ValDef(def) => def.tpt,
                _ => unreachable!(),
            };
            source_typed_index.remove(source_id, selected_type);
        }
        Mutation::WrongSelectedAnnotation => {
            let selected_type = match &parsed.ast.get(block_stats[2]).kind {
                TreeKind::ValDef(def) => def.tpt,
                _ => unreachable!(),
            };
            let first_type = match &parsed.ast.get(block_stats[0]).kind {
                TreeKind::ValDef(def) => def.tpt,
                _ => unreachable!(),
            };
            let typed_first_type = source_typed_index.get(source_id, first_type).unwrap();
            source_typed_index.remove(source_id, selected_type);
            source_typed_index
                .insert(source_id, selected_type, typed_first_type)
                .unwrap();
        }
        Mutation::SelectedInitializerNotIdent => {
            let selected_initializer = match &parsed.ast.get(block_stats[2]).kind {
                TreeKind::ValDef(def) => def.rhs.unwrap(),
                _ => unreachable!(),
            };
            let typed_initializer = source_typed_index
                .get(source_id, selected_initializer)
                .unwrap();
            let assignment_kind = typed_ast.get(typed_assignment).kind.clone();
            let initializer_type = typed_ast.get(typed_initializer).ty;
            typed_ast.get_mut(typed_initializer).kind = assignment_kind;
            typed_ast.get_mut(typed_initializer).ty = initializer_type;
        }
        Mutation::WrongAssignmentTarget => {
            let TreeKind::Assign(assign) = &mut parsed.ast.get_mut(assignment).kind else {
                unreachable!()
            };
            assign.rhs = source_assignment_lhs;
        }
        Mutation::StaleAssignmentTarget => {
            let TreeKind::Assign(assign) = &typed_ast.get(typed_assignment).kind else {
                unreachable!()
            };
            let rhs = assign.rhs;
            let prefix = match store.types.get(typed_ast.get(rhs).ty) {
                dotty_core::Type::TermRef { prefix, .. } => *prefix,
                _ => unreachable!(),
            };
            let stale_type = store.types.alloc(dotty_core::Type::TermRef {
                prefix,
                target: dotty_core::TermRefTarget::Symbol(stale_symbol_id(&store)),
            });
            typed_ast.get_mut(rhs).ty = stale_type;
        }
        Mutation::WrongSetterReceiver => {
            let TreeKind::Apply(setter) = &parsed.ast.get(block_stats[4]).kind else {
                unreachable!()
            };
            let function = setter.function;
            let TreeKind::Select(selection) = &mut parsed.ast.get_mut(function).kind else {
                unreachable!()
            };
            selection.qualifier = source_assignment_rhs;
        }
        Mutation::WrongPairArguments => {
            let TreeKind::Apply(pair) = &mut parsed.ast.get_mut(block_expr).kind else {
                unreachable!()
            };
            pair.args.swap(0, 1);
        }
        Mutation::StalePairConstructor => {
            let TreeKind::Apply(pair) = &parsed.ast.get(block_expr).kind else {
                unreachable!()
            };
            let typed_function = source_typed_index.get(source_id, pair.function).unwrap();
            let prefix = match store.types.get(typed_ast.get(typed_function).ty) {
                dotty_core::Type::TermRef { prefix, .. } => *prefix,
                _ => unreachable!(),
            };
            let stale_type = store.types.alloc(dotty_core::Type::TermRef {
                prefix,
                target: dotty_core::TermRefTarget::Symbol(stale_symbol_id(&store)),
            });
            typed_ast.get_mut(typed_function).ty = stale_type;
        }
        Mutation::WrongResult => {
            let typed_pair = source_typed_index.get(source_id, block_expr).unwrap();
            let local_type = match store.symbols.get(selected).info {
                dotty_core::SymbolInfo::Complete(ty) => ty,
                _ => unreachable!(),
            };
            typed_ast.get_mut(typed_pair).ty = local_type;
        }
        Mutation::ReorderStatements => {
            let TreeKind::Block(block) = &mut parsed.ast.get_mut(rhs).kind else {
                unreachable!()
            };
            block.stats.swap(3, 4);
        }
        Mutation::DuplicateAssignment => {
            let TreeKind::Block(block) = &mut parsed.ast.get_mut(rhs).kind else {
                unreachable!()
            };
            block.stats.push(assignment);
        }
        Mutation::ForeignAssignedType => {
            let TreeKind::Assign(assign) = &typed_ast.get(typed_assignment).kind else {
                unreachable!()
            };
            let rhs = assign.rhs;
            let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
                unreachable!()
            };
            let foreign = package
                .stats
                .iter()
                .find_map(|tree| {
                    let TreeKind::TypeDef(def) = &parsed.ast.get(*tree).kind else {
                        return None;
                    };
                    (store.names.resolve(def.name.as_name().text()) == "Pair")
                        .then(|| source_index.symbol_at(source_id, *tree).unwrap())
                })
                .unwrap();
            let foreign_type = store.types.alloc(dotty_core::Type::TermRef {
                prefix: definitions.no_prefix,
                target: dotty_core::TermRefTarget::Symbol(foreign),
            });
            typed_ast.get_mut(rhs).ty = foreign_type;
        }
        Mutation::MissingSourceBlockPosition => parsed.ast.get_mut(rhs).position = None,
        Mutation::MissingSourceSelectedPosition => {
            parsed.ast.get_mut(block_stats[2]).position = None;
        }
        Mutation::ForeignSourceSelectedPosition => {
            let position = parsed.ast.get(block_stats[2]).position.unwrap();
            parsed.ast.get_mut(block_stats[2]).position = Some(dotty_core::SourceSpan::new(
                SourceId::from_index(source_number.wrapping_add(1)),
                position.span(),
            ));
        }
        Mutation::MissingTypedAssignmentPosition => {
            typed_ast.get_mut(typed_assignment).position = None;
        }
        Mutation::MismatchedTypedAssignmentPosition => {
            let rhs_position = parsed.ast.get(source_assignment_rhs).position.unwrap();
            typed_ast.get_mut(typed_assignment).position = Some(rhs_position);
        }
    }
    let TreeKind::Block(current_block) = &parsed.ast.get(rhs).kind else {
        panic!("Block survives mutation")
    };
    let jvm_unit = JvmCompilationUnit {
        store: &store,
        typed_ast: &typed_ast,
        classes: Vec::new(),
    };
    let proof = validate_mutable_local_reference_reassignment_proof(
        MutableLocalReferenceReassignmentProofInput {
            source_id,
            method_tree,
            store: &store,
            definitions: &definitions,
            source_ast: &parsed.ast,
            typed_ast: &typed_ast,
            source_index: &source_index,
            source_typed_index: &source_typed_index,
            original_bindings: [Some(local_bindings[0]), Some(local_bindings[1])],
            selected_binding: supplied_selected,
            jvm_unit: &jvm_unit,
        },
    )?;
    assert_eq!(proof.scope(), selected_scope);
    assert_eq!(proof.source_block(), rhs);
    assert_eq!(proof.source_assignment(), assignment);
    assert_eq!(proof.typed_assignment(), typed_assignment);
    assert_eq!(proof.source_assignment_lhs(), source_assignment_lhs);
    assert_eq!(proof.source_assignment_rhs(), source_assignment_rhs);
    assert_eq!(
        current_block.stats.len(),
        5,
        "accepted Block has the frozen statement count"
    );
    Ok(Receipt {
        selected: proof.selected(),
        target: proof.assigned_target(),
        first: proof.original_locals()[0],
        second: proof.original_locals()[1],
        assignment: proof.source_assignment(),
        typed_assignment: proof.typed_assignment(),
        setter_receiver: proof.source_setter_receiver(),
        pair_args: proof.source_pair_arguments(),
    })
}

#[test]
fn mutable_reference_reassignment_receipt_accepts_canonical_variants() {
    let original = validate(SOURCE, 22_990, Mutation::None).unwrap();
    assert_ne!(original.first, original.second);
    assert_eq!(original.target, original.second);
    assert_ne!(original.setter_receiver, original.assignment);
    assert_ne!(original.pair_args[0], original.pair_args[1]);
    assert_ne!(
        original.assignment.index(),
        original.typed_assignment.index()
    );
    let renamed = SOURCE
        .replace("val first: Counter", "val alpha: Counter")
        .replace("selected: Counter = first", "selected: Counter = alpha")
        .replace("Pair(first, second)", "Pair(alpha, second)");
    let renamed = validate(&renamed, 22_991, Mutation::None).unwrap();
    assert_eq!(renamed.target, renamed.second);
    assert_ne!(renamed.selected, renamed.first);
    assert_ne!(renamed.selected, renamed.second);

    let reversed = SOURCE.replace(
        "    val first: Counter = new Counter(firstSeed)\n    val second: Counter = new Counter(secondSeed)",
        "    val second: Counter = new Counter(secondSeed)\n    val first: Counter = new Counter(firstSeed)",
    );
    let reversed = validate(&reversed, 22_992, Mutation::None).unwrap();
    assert_ne!(reversed.first, reversed.second);
    assert!(reversed.target == reversed.first || reversed.target == reversed.second);

    let changed_target = SOURCE.replace("selected = second", "selected = first");
    let changed_target = validate(&changed_target, 22_993, Mutation::None).unwrap();
    assert_eq!(changed_target.target, changed_target.first);
}

#[test]
fn mutable_reference_reassignment_rejects_forged_edges_and_shapes() {
    for mutation in [
        Mutation::MissingAssignment,
        Mutation::MissingRhs,
        Mutation::SwapAssignmentOperands,
        Mutation::WrongBinding,
        Mutation::MissingSelectedBinding,
        Mutation::ImmutableSelected,
        Mutation::WrongOwner,
        Mutation::WrongScope,
        Mutation::WrongInitializer,
        Mutation::MissingSelectedAnnotation,
        Mutation::WrongSelectedAnnotation,
        Mutation::SelectedInitializerNotIdent,
        Mutation::WrongAssignmentTarget,
        Mutation::StaleAssignmentTarget,
        Mutation::WrongSetterReceiver,
        Mutation::WrongPairArguments,
        Mutation::StalePairConstructor,
        Mutation::WrongResult,
        Mutation::ReorderStatements,
        Mutation::DuplicateAssignment,
        Mutation::ForeignAssignedType,
        Mutation::MissingSourceBlockPosition,
        Mutation::MissingSourceSelectedPosition,
        Mutation::ForeignSourceSelectedPosition,
        Mutation::MissingTypedAssignmentPosition,
        Mutation::MismatchedTypedAssignmentPosition,
    ] {
        assert!(
            validate(SOURCE, 23_000 + mutation as u32, mutation).is_err(),
            "{mutation:?}"
        );
    }
}

#[test]
fn assignment_mapping_failures_are_typed_binding_errors() {
    assert!(matches!(
        validate(SOURCE, 23_020, Mutation::MissingAssignment),
        Err(CrossClassConstructorCallProofError::TypedBinding(_))
    ));
    assert!(matches!(
        validate(SOURCE, 23_021, Mutation::MissingRhs),
        Err(CrossClassConstructorCallProofError::TypedBinding(_))
    ));
    for (offset, mutation) in [
        Mutation::MissingSelectedAnnotation,
        Mutation::WrongSelectedAnnotation,
        Mutation::SelectedInitializerNotIdent,
        Mutation::StaleAssignmentTarget,
        Mutation::StalePairConstructor,
        Mutation::MissingTypedAssignmentPosition,
        Mutation::MismatchedTypedAssignmentPosition,
    ]
    .into_iter()
    .enumerate()
    {
        assert!(
            matches!(
                validate(SOURCE, 23_030 + offset as u32, mutation),
                Err(CrossClassConstructorCallProofError::TypedBinding(_))
            ),
            "{mutation:?}"
        );
    }
    for (offset, mutation) in [
        Mutation::MissingSourceBlockPosition,
        Mutation::MissingSourceSelectedPosition,
        Mutation::ForeignSourceSelectedPosition,
    ]
    .into_iter()
    .enumerate()
    {
        assert!(
            matches!(
                validate(SOURCE, 23_040 + offset as u32, mutation),
                Err(CrossClassConstructorCallProofError::SourceIdentity(_))
            ),
            "{mutation:?}"
        );
    }
}
