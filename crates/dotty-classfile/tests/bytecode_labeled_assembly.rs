use dotty_classfile::classfile::{
    AssembleError, BranchInstruction, CodeElement, CodeSequence, CodeSequenceErrorKind,
    ConstantPool, ConstantPoolEntry, ConstantPoolIndex, ConstantPoolIndexOperand, Instruction,
    LabelId, LdcConstant, LocalIndex, SmallIntConstant, assemble, assemble_labeled,
};

fn empty_pool() -> ConstantPool {
    ConstantPool::default()
}

type BranchConstructor = fn(LabelId) -> BranchInstruction;

#[test]
fn encodes_forward_backward_zero_and_conditional_offsets_from_opcode_pc() {
    let pool = empty_pool();

    let mut forward = CodeSequence::new();
    let target = forward.new_label().unwrap();
    forward.branch(BranchInstruction::Goto(target.clone()));
    forward.bind(target.clone()).unwrap();
    forward.instruction(Instruction::AConstNull);
    let assembled = assemble_labeled(&forward, &pool).unwrap();
    assert_eq!(assembled.bytes, [0xa7, 0x00, 0x03, 0x01]);
    assert_eq!(assembled.layout.instruction_offsets(), [0, 3]);
    assert_eq!(assembled.layout.label_offset(&target), Some(3));
    assert_eq!(assembled.layout.code_length(), 4);

    let mut backward = CodeSequence::new();
    let start = backward.new_label().unwrap();
    backward.bind(start.clone()).unwrap();
    backward.instruction(Instruction::AConstNull);
    backward.branch(BranchInstruction::IfNe(start.clone()));
    let assembled = assemble_labeled(&backward, &pool).unwrap();
    assert_eq!(assembled.bytes, [0x01, 0x9a, 0xff, 0xff]);
    assert_eq!(assembled.layout.label_offset(&start), Some(0));

    let mut zero_offset = CodeSequence::new();
    let self_target = zero_offset.new_label().unwrap();
    zero_offset.bind(self_target.clone()).unwrap();
    zero_offset.branch(BranchInstruction::Goto(self_target.clone()));
    zero_offset.instruction(Instruction::AConstNull);
    let assembled = assemble_labeled(&zero_offset, &pool).unwrap();
    assert_eq!(assembled.bytes, [0xa7, 0x00, 0x00, 0x01]);
}

#[test]
fn encodes_pop_with_exact_linear_and_labeled_offsets() {
    let pool = empty_pool();
    let instructions = [
        Instruction::IConst(SmallIntConstant::One),
        Instruction::Pop,
        Instruction::Return,
    ];
    assert_eq!(assemble(&instructions, &pool).unwrap(), [0x04, 0x57, 0xb1]);

    let mut sequence = CodeSequence::new();
    for instruction in instructions {
        sequence.instruction(instruction);
    }
    let first = assemble_labeled(&sequence, &pool).unwrap();
    let second = assemble_labeled(&sequence, &pool).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.bytes, [0x04, 0x57, 0xb1]);
    assert_eq!(first.layout.instruction_offsets(), [0, 1, 2]);
    assert_eq!(first.layout.code_length(), 3);
}

#[test]
fn encodes_wide_goto_and_every_conditional_opcode() {
    let pool = empty_pool();
    let mut wide = CodeSequence::new();
    let target = wide.new_label().unwrap();
    wide.branch(BranchInstruction::GotoWide(target.clone()));
    wide.bind(target.clone()).unwrap();
    wide.instruction(Instruction::AConstNull);
    let assembled = assemble_labeled(&wide, &pool).unwrap();
    assert_eq!(assembled.bytes, [0xc8, 0x00, 0x00, 0x00, 0x05, 0x01]);
    assert_eq!(assembled.layout.label_offset(&target), Some(5));

    let mut wide_backward = CodeSequence::new();
    let start = wide_backward.new_label().unwrap();
    wide_backward.bind(start.clone()).unwrap();
    wide_backward.instruction(Instruction::AConstNull);
    wide_backward.branch(BranchInstruction::GotoWide(start));
    let assembled = assemble_labeled(&wide_backward, &pool).unwrap();
    assert_eq!(assembled.bytes, [0x01, 0xc8, 0xff, 0xff, 0xff, 0xff]);

    let branches: [(BranchConstructor, u8); 8] = [
        (BranchInstruction::IfEq, 0x99),
        (BranchInstruction::IfNe, 0x9a),
        (BranchInstruction::IfICmpEq, 0x9f),
        (BranchInstruction::IfICmpNe, 0xa0),
        (BranchInstruction::IfICmpLt, 0xa1),
        (BranchInstruction::IfICmpGe, 0xa2),
        (BranchInstruction::IfICmpGt, 0xa3),
        (BranchInstruction::IfICmpLe, 0xa4),
    ];
    for (make_branch, opcode) in branches {
        let mut sequence = CodeSequence::new();
        let target = sequence.new_label().unwrap();
        sequence.branch(make_branch(target.clone()));
        sequence.bind(target).unwrap();
        sequence.instruction(Instruction::AConstNull);
        let assembled = assemble_labeled(&sequence, &pool).unwrap();
        assert_eq!(assembled.bytes, [opcode, 0x00, 0x03, 0x01]);
    }
}

#[test]
fn uses_actual_ldc_and_wide_local_sizes_in_layout() {
    let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Integer(0)); 256]);
    let integer = ConstantPoolIndexOperand::try_from(ConstantPoolIndex(256)).unwrap();
    let mut sequence = CodeSequence::new();
    let first_target = sequence.new_label().unwrap();
    let second_target = sequence.new_label().unwrap();
    sequence
        .instruction(Instruction::Ldc(LdcConstant::Integer(integer)))
        .instruction(Instruction::ILoad(LocalIndex::new(256)));
    sequence.bind(first_target.clone()).unwrap();
    sequence.bind(second_target.clone()).unwrap();
    sequence.branch(BranchInstruction::Goto(first_target.clone()));
    sequence.instruction(Instruction::IConst(SmallIntConstant::Zero));

    let assembled = assemble_labeled(&sequence, &pool).unwrap();
    assert_eq!(
        assembled.bytes,
        [
            0x13, 0x01, 0x00, // ldc_w #256
            0xc4, 0x15, 0x01, 0x00, // wide iload 256
            0xa7, 0x00, 0x00, // goto with a zero displacement
            0x03,
        ]
    );
    assert_eq!(assembled.layout.instruction_offsets(), [0, 3, 7, 10]);
    assert_eq!(assembled.layout.label_offset(&first_target), Some(7));
    assert_eq!(assembled.layout.label_offset(&second_target), Some(7));
}

#[test]
fn reports_signed_i16_boundaries_and_both_directions_of_overflow() {
    let pool = empty_pool();

    let mut forward_max = CodeSequence::new();
    let target = forward_max.new_label().unwrap();
    forward_max.branch(BranchInstruction::Goto(target.clone()));
    for _ in 0..32_764 {
        forward_max.instruction(Instruction::AConstNull);
    }
    forward_max.bind(target).unwrap();
    forward_max.instruction(Instruction::AConstNull);
    let assembled = assemble_labeled(&forward_max, &pool).unwrap();
    assert_eq!(&assembled.bytes[1..3], &[0x7f, 0xff]);

    let mut backward_min = CodeSequence::new();
    let start = backward_min.new_label().unwrap();
    backward_min.bind(start.clone()).unwrap();
    for _ in 0..32_768 {
        backward_min.instruction(Instruction::AConstNull);
    }
    backward_min.branch(BranchInstruction::Goto(start.clone()));
    backward_min.instruction(Instruction::AConstNull);
    let assembled = assemble_labeled(&backward_min, &pool).unwrap();
    let branch = assembled.layout.instruction_offset(32_768).unwrap();
    assert_eq!(&assembled.bytes[branch + 1..branch + 3], &[0x80, 0x00]);

    let mut forward_overflow = CodeSequence::new();
    let target = forward_overflow.new_label().unwrap();
    forward_overflow.branch(BranchInstruction::IfEq(target.clone()));
    for _ in 0..32_765 {
        forward_overflow.instruction(Instruction::AConstNull);
    }
    forward_overflow.bind(target).unwrap();
    forward_overflow.instruction(Instruction::AConstNull);
    assert_eq!(
        assemble_labeled(&forward_overflow, &pool),
        Err(AssembleError::BranchOffsetOutOfRange {
            instruction: 0,
            offset: 32_768,
            min: i64::from(i16::MIN),
            max: i64::from(i16::MAX),
        })
    );

    let mut backward_overflow = CodeSequence::new();
    let start = backward_overflow.new_label().unwrap();
    backward_overflow.bind(start.clone()).unwrap();
    for _ in 0..32_769 {
        backward_overflow.instruction(Instruction::AConstNull);
    }
    backward_overflow.branch(BranchInstruction::Goto(start));
    backward_overflow.instruction(Instruction::AConstNull);
    assert_eq!(
        assemble_labeled(&backward_overflow, &pool),
        Err(AssembleError::BranchOffsetOutOfRange {
            instruction: 32_769,
            offset: -32_769,
            min: i64::from(i16::MIN),
            max: i64::from(i16::MAX),
        })
    );
}

#[test]
fn rejects_invalid_targets_and_code_arrays_over_65535_bytes() {
    let pool = empty_pool();
    let mut unbound = CodeSequence::new();
    let label = unbound.new_label().unwrap();
    unbound.branch(BranchInstruction::Goto(label.clone()));
    assert_eq!(
        assemble_labeled(&unbound, &pool),
        Err(AssembleError::InvalidCodeSequence(
            CodeSequenceErrorKind::UnboundLabel
        ))
    );

    let mut end_target = CodeSequence::new();
    let label = end_target.new_label().unwrap();
    end_target.branch(BranchInstruction::Goto(label.clone()));
    end_target.bind(label.clone()).unwrap();
    assert_eq!(
        assemble_labeled(&end_target, &pool),
        Err(AssembleError::InvalidCodeSequence(
            CodeSequenceErrorKind::BranchToTerminalEnd
        ))
    );

    let mut foreign_target = CodeSequence::new();
    let mut owner = CodeSequence::new();
    let label = owner.new_label().unwrap();
    foreign_target.branch(BranchInstruction::IfEq(label));
    assert_eq!(
        assemble_labeled(&foreign_target, &pool),
        Err(AssembleError::InvalidCodeSequence(
            CodeSequenceErrorKind::ForeignLabel
        ))
    );

    let mut maximum = CodeSequence::new();
    for _ in 0..usize::from(u16::MAX) {
        maximum.instruction(Instruction::AConstNull);
    }
    let assembled = assemble_labeled(&maximum, &pool).unwrap();
    assert_eq!(assembled.layout.code_length(), usize::from(u16::MAX));
    assert_eq!(assembled.bytes.len(), usize::from(u16::MAX));

    let mut too_long = CodeSequence::new();
    let start = too_long.new_label().unwrap();
    too_long.bind(start.clone()).unwrap();
    too_long.branch(BranchInstruction::GotoWide(start));
    for _ in 0..65_531 {
        too_long.instruction(Instruction::AConstNull);
    }
    assert_eq!(
        assemble_labeled(&too_long, &pool),
        Err(AssembleError::InvalidCodeLength { length: 65_536 })
    );
}

#[test]
fn leaves_the_existing_linear_assembler_output_unchanged() {
    let pool = empty_pool();
    let instructions = [
        Instruction::IConst(SmallIntConstant::Five),
        Instruction::AConstNull,
    ];
    assert_eq!(assemble(&instructions, &pool).unwrap(), [0x08, 0x01]);
}

#[test]
fn code_elements_keep_labels_out_of_the_instruction_layout() {
    let mut sequence = CodeSequence::new();
    let first = sequence.new_label().unwrap();
    let second = sequence.new_label().unwrap();
    sequence.bind(first).unwrap();
    sequence.bind(second).unwrap();
    sequence.instruction(Instruction::AConstNull);

    let assembled = assemble_labeled(&sequence, &empty_pool()).unwrap();
    assert_eq!(assembled.layout.instruction_count(), 1);
    assert_eq!(
        sequence
            .validate()
            .unwrap()
            .elements()
            .iter()
            .filter(|element| matches!(element, CodeElement::Label(_)))
            .count(),
        2
    );
}
