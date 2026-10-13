use dotty_classfile::classfile::{
    BranchCondition, BranchInstruction, CodeElement, CodeSequence, CodeSequenceError, Instruction,
    SmallIntConstant,
};

#[test]
fn validates_forward_backward_repeated_and_co_located_labels() {
    let mut sequence = CodeSequence::new();
    let start = sequence.new_label().unwrap();
    let same_start = sequence.new_label().unwrap();
    let end = sequence.new_label().unwrap();

    sequence.bind(start.clone()).unwrap();
    sequence.bind(same_start.clone()).unwrap();
    sequence
        .branch(BranchInstruction::IfEq(end.clone()))
        .instruction(Instruction::IConst(SmallIntConstant::Zero))
        .branch(BranchInstruction::Goto(start.clone()))
        .branch(BranchInstruction::IfNe(end.clone()))
        .bind(end.clone())
        .unwrap()
        .instruction(Instruction::IConst(SmallIntConstant::One));

    let validated = sequence.validate().unwrap();
    assert_eq!(validated.instruction_count(), 5);
    assert_eq!(validated.instruction_index(&start), Some(0));
    assert_eq!(validated.instruction_index(&same_start), Some(0));
    assert_eq!(validated.instruction_index(&end), Some(4));
    assert_eq!(
        validated
            .elements()
            .iter()
            .filter(|element| matches!(element, CodeElement::Label(_)))
            .count(),
        3
    );
}

#[test]
fn rejects_unbound_and_terminal_end_targets() {
    let mut unbound_sequence = CodeSequence::new();
    let unbound = unbound_sequence.new_label().unwrap();
    unbound_sequence.branch(BranchInstruction::Goto(unbound.clone()));
    assert_eq!(
        unbound_sequence.validate().unwrap_err(),
        CodeSequenceError::UnboundLabel { label: unbound }
    );

    let mut end_sequence = CodeSequence::new();
    let end = end_sequence.new_label().unwrap();
    end_sequence
        .instruction(Instruction::IConst(SmallIntConstant::Zero))
        .branch(BranchInstruction::Goto(end.clone()))
        .bind(end.clone())
        .unwrap();
    assert_eq!(
        end_sequence.validate().unwrap_err(),
        CodeSequenceError::BranchToTerminalEnd { label: end }
    );
}

#[test]
fn rejects_duplicate_and_foreign_label_definitions() {
    let mut sequence = CodeSequence::new();
    let label = sequence.new_label().unwrap();
    sequence.bind(label.clone()).unwrap();
    assert_eq!(
        sequence.bind(label.clone()).unwrap_err(),
        CodeSequenceError::DuplicateLabelDefinition { label }
    );

    let mut owner = CodeSequence::new();
    let foreign_label = owner.new_label().unwrap();
    assert_eq!(
        sequence.bind(foreign_label.clone()).unwrap_err(),
        CodeSequenceError::ForeignLabel {
            label: foreign_label
        }
    );
}

#[test]
fn rejects_foreign_branch_targets_even_when_local_indexes_match() {
    let mut sequence = CodeSequence::new();
    let local = sequence.new_label().unwrap();
    sequence.bind(local).unwrap();

    let mut other = CodeSequence::new();
    let foreign = other.new_label().unwrap();
    sequence.branch(BranchInstruction::IfEq(foreign.clone()));

    assert_eq!(
        sequence.validate().unwrap_err(),
        CodeSequenceError::ForeignLabel { label: foreign }
    );
}

#[test]
fn exposes_semantics_for_every_supported_branch_condition() {
    let mut sequence = CodeSequence::new();
    let target = sequence.new_label().unwrap();
    let branches = [
        (
            BranchInstruction::Goto(target.clone()),
            BranchCondition::Always,
        ),
        (
            BranchInstruction::GotoWide(target.clone()),
            BranchCondition::Always,
        ),
        (
            BranchInstruction::IfEq(target.clone()),
            BranchCondition::EqualZero,
        ),
        (
            BranchInstruction::IfNe(target.clone()),
            BranchCondition::NotEqualZero,
        ),
        (
            BranchInstruction::IfICmpEq(target.clone()),
            BranchCondition::IntEqual,
        ),
        (
            BranchInstruction::IfICmpNe(target.clone()),
            BranchCondition::IntNotEqual,
        ),
        (
            BranchInstruction::IfICmpLt(target.clone()),
            BranchCondition::IntLessThan,
        ),
        (
            BranchInstruction::IfICmpGe(target.clone()),
            BranchCondition::IntGreaterOrEqual,
        ),
        (
            BranchInstruction::IfICmpGt(target.clone()),
            BranchCondition::IntGreaterThan,
        ),
        (
            BranchInstruction::IfICmpLe(target.clone()),
            BranchCondition::IntLessOrEqual,
        ),
    ];
    for (branch, _) in &branches {
        sequence.branch(branch.clone());
    }
    sequence.bind(target).unwrap();
    sequence.instruction(Instruction::IConst(SmallIntConstant::Zero));

    let validated = sequence.validate().unwrap();
    let actual: Vec<_> = validated
        .elements()
        .iter()
        .filter_map(|element| match element {
            CodeElement::Branch(branch) => Some((branch.condition(), branch.is_conditional())),
            _ => None,
        })
        .collect();
    let expected: Vec<_> = branches
        .iter()
        .map(|(branch, condition)| (*condition, branch.is_conditional()))
        .collect();
    assert_eq!(actual, expected);
}

#[test]
fn preserves_existing_straight_line_instruction_values() {
    let instruction = Instruction::IConst(SmallIntConstant::Five);
    let mut sequence = CodeSequence::new();
    sequence.instruction(instruction);
    let validated = sequence.validate().unwrap();

    assert_eq!(validated.instruction_count(), 1);
    assert_eq!(
        validated.elements(),
        &[CodeElement::Instruction(instruction)]
    );
}
