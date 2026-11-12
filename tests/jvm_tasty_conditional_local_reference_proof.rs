use dotty::jvm_tasty_cross_class::{
    CrossClassConstructorCallProofError, ImmutableLocalConditionalReferenceSelectionProofInput,
    ImmutableLocalInitializerPairProofInput,
    validate_immutable_local_conditional_reference_selection_proof,
};
use dotty::jvm_tasty_plan::{PairedPlanError, with_paired_jvm_tasty_batch_plan};
use dotty_core::ast::TreeKind;
use dotty_core::{Definitions, Packages, SemanticStore, SourceId, SourceText};
use dotty_jvm::JvmCompilationUnit;
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_typer::{SourceTyper, TypedLocalBinding};

const SOURCE: &str = r#"package example

class Counter(start: Int):
  private var current: Int = start
  def get(): Int = current
  def set(value: Int): Unit = current = value

class Pair(first: Counter, second: Counter):
  private val left: Counter = first
  private val right: Counter = second
  def leftValue(): Int = left.get()
  def rightValue(): Int = right.get()

class PairFactory:
  def create(firstSeed: Int, secondSeed: Int, chooseFirst: Boolean, nextValue: Int): Pair =
    val first: Counter = new Counter(firstSeed)
    val second: Counter = new Counter(secondSeed)
    val selected: Counter =
      if chooseFirst then first else second
    selected.set(nextValue)
    new Pair(first, second)
"#;

#[derive(Clone, Copy)]
enum IndexMutation {
    None,
    MissingCondition,
    SwappedBranches,
    ConditionAsIntFormal,
    BranchAsIntFormal,
    WrongSetterReceiver,
    WrongPairArgument,
    WrongSelectedBinding,
}

#[derive(Debug)]
struct ReceiptIds {
    source: SourceId,
    method: dotty_core::SymbolId,
    condition_formal: dotty_core::SymbolId,
    targets: [dotty_core::SymbolId; 2],
    selected: dotty_core::SymbolId,
    scope: dotty_core::ScopeId,
}

fn authenticate(
    source: &str,
    source_number: u32,
    mutation: IndexMutation,
) -> Result<ReceiptIds, CrossClassConstructorCallProofError> {
    let source_id = SourceId::from_index(source_number);
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
        "conditional-reference-selection.scala",
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
    let mut factory_method = None;
    for &class_tree in &package.stats {
        let TreeKind::TypeDef(class_def) = &parsed.ast.get(class_tree).kind else {
            continue;
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
        classes.push((class, constructor, constructor_formals));
        for &member in &template.body {
            let TreeKind::DefDef(def) = &parsed.ast.get(member).kind else {
                continue;
            };
            if store.names.resolve(def.name.as_name().text()) == "create" {
                factory_method = Some((member, def.clone()));
            }
        }
    }
    let (method_tree, method_def) = factory_method.expect("create method");
    let method = source_index.symbol_at(source_id, method_tree).unwrap();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &source_index,
        &mut store,
        definitions,
        &packages,
    );
    for (class, constructor, parameters) in &classes {
        typer.complete_symbol(*class).unwrap();
        typer.complete_symbol(*constructor).unwrap();
        for parameter in parameters {
            typer.complete_symbol(*parameter).unwrap();
        }
    }
    typer.complete_symbol(method).unwrap();
    let formals = method_def.value_param_clauses[0]
        .iter()
        .map(|tree| source_index.symbol_at(source_id, *tree).unwrap())
        .collect::<Vec<_>>();
    for formal in &formals {
        typer.complete_symbol(*formal).unwrap();
    }
    let rhs = method_def.rhs.unwrap();
    let context = typer.expression_context_for(method).unwrap();
    typer.type_expression(rhs, context).unwrap();
    let TreeKind::Block(block) = &parsed.ast.get(rhs).kind else {
        panic!("factory block")
    };
    let mut local_bindings = Vec::<TypedLocalBinding>::new();
    for declaration in &block.stats[..3] {
        local_bindings.push(
            typer
                .typed_local_binding_at(source_id, *declaration)
                .unwrap(),
        );
    }
    let mut source_typed_index = typer.source_typed_index().clone();
    let TreeKind::ValDef(selected_def) = &parsed.ast.get(block.stats[2]).kind else {
        panic!("selected declaration")
    };
    let TreeKind::If(if_node) = &parsed.ast.get(selected_def.rhs.unwrap()).kind else {
        panic!("selection IF")
    };
    match mutation {
        IndexMutation::None => {}
        IndexMutation::MissingCondition => {
            source_typed_index.remove(source_id, if_node.cond);
        }
        IndexMutation::SwappedBranches => {
            let then_typed = source_typed_index
                .remove(source_id, if_node.then_branch)
                .unwrap();
            let else_typed = source_typed_index
                .remove(source_id, if_node.else_branch)
                .unwrap();
            source_typed_index
                .insert(source_id, if_node.then_branch, else_typed)
                .unwrap();
            source_typed_index
                .insert(source_id, if_node.else_branch, then_typed)
                .unwrap();
        }
        IndexMutation::ConditionAsIntFormal => {
            let int_use = match &parsed.ast.get(block.stats[3]).kind {
                TreeKind::Apply(application) => application.args[0],
                _ => unreachable!(),
            };
            let int_typed = source_typed_index.get(source_id, int_use).unwrap();
            source_typed_index.remove(source_id, if_node.cond);
            source_typed_index
                .insert(source_id, if_node.cond, int_typed)
                .unwrap();
        }
        IndexMutation::BranchAsIntFormal => {
            let int_use = match &parsed.ast.get(block.stats[3]).kind {
                TreeKind::Apply(application) => application.args[0],
                _ => unreachable!(),
            };
            let int_typed = source_typed_index.get(source_id, int_use).unwrap();
            source_typed_index.remove(source_id, if_node.then_branch);
            source_typed_index
                .insert(source_id, if_node.then_branch, int_typed)
                .unwrap();
        }
        IndexMutation::WrongSetterReceiver => {
            let TreeKind::Apply(setter) = &parsed.ast.get(block.stats[3]).kind else {
                unreachable!()
            };
            let TreeKind::Select(select) = &parsed.ast.get(setter.function).kind else {
                unreachable!()
            };
            let first_typed = source_typed_index.get(source_id, block.stats[0]).unwrap();
            source_typed_index.remove(source_id, select.qualifier);
            source_typed_index
                .insert(source_id, select.qualifier, first_typed)
                .unwrap();
        }
        IndexMutation::WrongPairArgument => {
            let TreeKind::Apply(pair) = &parsed.ast.get(block.expr).kind else {
                unreachable!()
            };
            let selected_typed = source_typed_index.get(source_id, block.stats[2]).unwrap();
            source_typed_index.remove(source_id, pair.args[0]);
            source_typed_index
                .insert(source_id, pair.args[0], selected_typed)
                .unwrap();
        }
        IndexMutation::WrongSelectedBinding => {}
    }
    let typed_ast = typer.typed_ast();
    let jvm_unit = JvmCompilationUnit {
        store: typer.store(),
        typed_ast,
        classes: Vec::new(),
    };
    let proof_result = validate_immutable_local_conditional_reference_selection_proof(
        ImmutableLocalConditionalReferenceSelectionProofInput {
            initializers: ImmutableLocalInitializerPairProofInput {
                source_id,
                method_tree,
                store: typer.store(),
                definitions: &definitions,
                source_ast: &parsed.ast,
                typed_ast,
                source_index: &source_index,
                source_typed_index: &source_typed_index,
                local_bindings: [Some(local_bindings[0]), Some(local_bindings[1])],
                jvm_unit: &jvm_unit,
            },
            selected_binding: Some(if matches!(mutation, IndexMutation::WrongSelectedBinding) {
                local_bindings[0]
            } else {
                local_bindings[2]
            }),
        },
    );
    let proof = proof_result?;
    let binding = proof.binding();
    assert_eq!(binding.source_id, source_id);
    assert_eq!(binding.method, method);
    assert_eq!(binding.selected, proof.selected());
    assert_eq!(binding.scope, local_bindings[2].scope());
    assert_eq!(binding.condition_formal, formals[2]);
    assert_ne!(binding.local_type, definitions.boolean);
    Ok(ReceiptIds {
        source: proof.source_id(),
        method: proof.method(),
        condition_formal: proof.condition_formal(),
        targets: proof.targets(),
        selected: proof.selected(),
        scope: proof.scope(),
    })
}

#[test]
fn authentic_conditional_local_receipt_preserves_session_identities_for_source_controls() {
    let original = authenticate(SOURCE, 22_650, IndexMutation::None).unwrap();
    assert_eq!(original.source, SourceId::from_index(22_650));
    assert_eq!(original.method.index(), 60);
    assert_eq!(original.condition_formal.index(), 63);
    assert_eq!(original.targets.map(dotty_core::SymbolId::index), [65, 66]);
    assert_eq!(original.selected.index(), 67);
    assert_eq!(original.scope.index(), 18);
    assert_ne!(original.targets[0], original.targets[1]);
    assert!(!original.targets.contains(&original.selected));

    let renamed = SOURCE
        .replace("    val first: Counter", "    val leftLocal: Counter")
        .replace("    val second: Counter", "    val rightLocal: Counter")
        .replace("    val selected: Counter", "    val chosen: Counter")
        .replace("then first else second", "then leftLocal else rightLocal")
        .replace("selected.set", "chosen.set")
        .replace("new Pair(first, second)", "new Pair(leftLocal, rightLocal)");
    let renamed_receipt = authenticate(&renamed, 22_671, IndexMutation::None).unwrap();
    assert_eq!(renamed_receipt.source, SourceId::from_index(22_671));
    assert_ne!(renamed_receipt.targets[0], renamed_receipt.targets[1]);
    assert!(!renamed_receipt.targets.contains(&renamed_receipt.selected));

    let reversed = SOURCE.replace(
        "    val first: Counter = new Counter(firstSeed)\n    val second: Counter = new Counter(secondSeed)",
        "    val second: Counter = new Counter(secondSeed)\n    val first: Counter = new Counter(firstSeed)",
    );
    let reversed_receipt = authenticate(&reversed, 22_672, IndexMutation::None).unwrap();
    assert_eq!(
        reversed_receipt.targets,
        [original.targets[1], original.targets[0]]
    );

    let swapped = SOURCE.replace("then first else second", "then second else first");
    authenticate(&swapped, 22_673, IndexMutation::None).unwrap();
}

#[test]
fn missing_or_forged_source_typed_if_edges_reject_at_the_typed_binding_stage() {
    let missing = authenticate(SOURCE, 22_674, IndexMutation::MissingCondition);
    assert!(
        matches!(
            missing,
            Err(CrossClassConstructorCallProofError::TypedBinding(_))
        ),
        "unexpected failure: {missing:?}"
    );
    assert!(matches!(
        authenticate(SOURCE, 22_675, IndexMutation::SwappedBranches),
        Err(CrossClassConstructorCallProofError::TypedBinding(_))
    ));
    assert!(matches!(
        authenticate(SOURCE, 22_676, IndexMutation::ConditionAsIntFormal),
        Err(CrossClassConstructorCallProofError::TypedBinding(_))
    ));
    assert!(matches!(
        authenticate(SOURCE, 22_677, IndexMutation::BranchAsIntFormal),
        Err(CrossClassConstructorCallProofError::TypedBinding(_))
    ));
    assert!(matches!(
        authenticate(SOURCE, 22_678, IndexMutation::WrongSetterReceiver),
        Err(CrossClassConstructorCallProofError::TypedBinding(_))
    ));
    assert!(matches!(
        authenticate(SOURCE, 22_679, IndexMutation::WrongPairArgument),
        Err(CrossClassConstructorCallProofError::TypedBinding(_))
    ));
    assert!(matches!(
        authenticate(SOURCE, 22_680, IndexMutation::WrongSelectedBinding),
        Err(CrossClassConstructorCallProofError::SourceIdentity(_))
    ));
}

#[test]
fn conditional_shape_keeps_the_original_four_formal_producer_rejection_before_callback() {
    let mut callback_reached = false;
    let result = with_paired_jvm_tasty_batch_plan(
        SOURCE,
        "conditional-reference-selection.scala",
        SourceId::from_index(22_681),
        |_| callback_reached = true,
    );
    assert!(
        matches!(
            result,
            Err(PairedPlanError::BatchUnit {
                phase: "cross-class constructor-call identity proof",
                ref reason,
            }) if reason.contains("SourceShape")
                && reason.contains("candidate method must have one plain formal clause")
        ),
        "unexpected producer failure: {result:?}"
    );
    assert!(
        !callback_reached,
        "the rejected source must return no callback plan"
    );
}

#[test]
fn frontend_invalid_condition_branch_and_result_inputs_fail_during_typing() {
    let controls = [
        SOURCE.replace("if chooseFirst then", "if firstSeed then"),
        SOURCE.replace("then first else second", "then firstSeed else second"),
        SOURCE.replace("then first else second", "then missingLocal else second"),
        SOURCE.replace("val selected: Counter", "val selected: Int"),
    ];
    let expected_failures = [
        "IfConditionTypeMismatch",
        "LocalValueTypeMismatch",
        "TermNameNotFound",
        "LocalValueTypeMismatch",
    ];
    for (index, (source, expected)) in controls.iter().zip(expected_failures).enumerate() {
        let mut callback_reached = false;
        let result = with_paired_jvm_tasty_batch_plan(
            source,
            "conditional-reference-invalid-control.scala",
            SourceId::from_index(22_690 + index as u32),
            |_| callback_reached = true,
        );
        assert!(
            matches!(
                result,
                Err(PairedPlanError::BatchClass {
                    phase: "method body typing",
                    ref reason,
                    ..
                }) if reason.contains(expected)
            ),
            "unexpected first failure for invalid control {index}: {result:?}"
        );
        assert!(!callback_reached);
    }
}

#[test]
fn mutable_or_nested_selections_are_outside_the_receipt_contract() {
    let mutable = SOURCE.replace("val selected: Counter", "var selected: Counter");
    assert!(matches!(
        authenticate(&mutable, 22_695, IndexMutation::None),
        Err(CrossClassConstructorCallProofError::SourceIdentity(_))
    ));

    let nested = SOURCE.replace(
        "if chooseFirst then first else second",
        "if chooseFirst then (if chooseFirst then first else second) else second",
    );
    assert!(matches!(
        authenticate(&nested, 22_696, IndexMutation::None),
        Err(CrossClassConstructorCallProofError::SourceShape(_))
    ));

    let allocation = SOURCE.replace(
        "if chooseFirst then first else second",
        "if chooseFirst then new Counter(firstSeed) else second",
    );
    assert!(matches!(
        authenticate(&allocation, 22_697, IndexMutation::None),
        Err(CrossClassConstructorCallProofError::SourceShape(_))
    ));
}
