use dotty_classfile::bytecode::{
    AnalysisError, BranchInstruction, CfgCodeAnalyzer, CodeSequence, ControlFlowGraph,
    ControlFlowGraphError, FlowLocalState, Instruction, LocalIndex, SmallIntConstant,
    SmallLongConstant, UnsupportedFlowInstructionKind, ValueKind, VerificationTypeInfo,
    assemble_labeled, generate_stack_map_frames,
};
use dotty_classfile::classfile::ConstantPool;

fn local(index: u16) -> LocalIndex {
    LocalIndex::new(index)
}

#[test]
fn diamond_merges_initialized_integer_locals_and_consumes_condition() {
    let mut sequence = CodeSequence::new();
    let otherwise = sequence.new_label().unwrap();
    let join = sequence.new_label().unwrap();

    sequence.instruction(Instruction::IConst(SmallIntConstant::Zero));
    sequence.branch(BranchInstruction::IfEq(otherwise.clone()));
    sequence.instruction(Instruction::IConst(SmallIntConstant::One));
    sequence.instruction(Instruction::IStore(local(0)));
    sequence.branch(BranchInstruction::Goto(join.clone()));
    sequence.bind(otherwise).unwrap();
    sequence.instruction(Instruction::IConst(SmallIntConstant::Zero));
    sequence.instruction(Instruction::IStore(local(0)));
    sequence.bind(join.clone()).unwrap();
    sequence.instruction(Instruction::ILoad(local(0)));
    sequence.instruction(Instruction::IReturn);

    let graph = ControlFlowGraph::build(&sequence).unwrap();
    let analysis = CfgCodeAnalyzer::analyze(&graph, "()I", true, &ConstantPool::default()).unwrap();
    let join_state = analysis
        .entry_state(graph.block_for_label(&join).unwrap())
        .unwrap();
    assert_eq!(join_state.locals, [FlowLocalState::Integer]);
    assert!(join_state.operand_stack.is_empty());
    assert_eq!(analysis.max_stack, 1);
    assert_eq!(analysis.max_locals, 1);
}

#[test]
fn pop_updates_branch_stack_state_and_preserves_stack_map_offsets() {
    let mut sequence = CodeSequence::new();
    let otherwise = sequence.new_label().unwrap();
    let join = sequence.new_label().unwrap();

    sequence.instruction(Instruction::IConst(SmallIntConstant::Zero));
    sequence.branch(BranchInstruction::IfEq(otherwise.clone()));
    sequence.instruction(Instruction::IConst(SmallIntConstant::One));
    sequence.instruction(Instruction::Pop);
    sequence.instruction(Instruction::IConst(SmallIntConstant::Five));
    sequence.branch(BranchInstruction::Goto(join.clone()));
    sequence.bind(otherwise.clone()).unwrap();
    sequence.instruction(Instruction::IConst(SmallIntConstant::Five));
    sequence.bind(join.clone()).unwrap();
    sequence.instruction(Instruction::IReturn);

    let graph = ControlFlowGraph::build(&sequence).unwrap();
    let flow = CfgCodeAnalyzer::analyze(&graph, "()I", true, &ConstantPool::default()).unwrap();
    let join_state = flow
        .entry_state(graph.block_for_label(&join).unwrap())
        .unwrap();
    assert_eq!(join_state.operand_stack, [ValueKind::Int]);
    assert_eq!(flow.max_stack, 1);

    let assembled = assemble_labeled(&sequence, &ConstantPool::default()).unwrap();
    assert_eq!(
        assembled.bytes,
        [
            0x03, 0x99, 0x00, 0x09, 0x04, 0x57, 0x08, 0xa7, 0x00, 0x04, 0x08, 0xac
        ]
    );
    assert_eq!(
        assembled.layout.instruction_offsets(),
        [0, 1, 4, 5, 6, 7, 10, 11]
    );
    assert_eq!(assembled.layout.label_offset(&otherwise), Some(10));
    assert_eq!(assembled.layout.label_offset(&join), Some(11));

    let frames = generate_stack_map_frames(&flow, &graph, &assembled).unwrap();
    assert_eq!(
        frames.code_analysis,
        dotty_classfile::bytecode::CodeAnalysis {
            max_stack: 1,
            max_locals: 0,
        }
    );
    assert_eq!(
        frames
            .frames
            .iter()
            .map(|frame| frame.pc)
            .collect::<Vec<_>>(),
        [10, 11]
    );
    assert!(frames.frames[0].stack.is_empty());
    assert_eq!(frames.frames[1].stack, [VerificationTypeInfo::Integer]);
}

#[test]
fn two_early_returns_and_nested_conditionals_have_stable_states() {
    let mut sequence = CodeSequence::new();
    let outer_else = sequence.new_label().unwrap();
    let inner_else = sequence.new_label().unwrap();

    sequence.instruction(Instruction::ILoad(local(0)));
    sequence.branch(BranchInstruction::IfEq(outer_else.clone()));
    sequence.instruction(Instruction::ILoad(local(0)));
    sequence.branch(BranchInstruction::IfNe(inner_else.clone()));
    sequence.instruction(Instruction::IConst(SmallIntConstant::Zero));
    sequence.instruction(Instruction::IReturn);
    sequence.bind(inner_else).unwrap();
    sequence.instruction(Instruction::IConst(SmallIntConstant::One));
    sequence.instruction(Instruction::IReturn);
    sequence.bind(outer_else).unwrap();
    sequence.instruction(Instruction::ILoad(local(0)));
    sequence.instruction(Instruction::IReturn);
    let graph = ControlFlowGraph::build(&sequence).unwrap();
    let first = CfgCodeAnalyzer::analyze(&graph, "(I)I", true, &ConstantPool::default()).unwrap();
    let second = CfgCodeAnalyzer::analyze(&graph, "(I)I", true, &ConstantPool::default()).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.max_stack, 1);
    assert_eq!(first.max_locals, 1);
    assert_eq!(first.block_entries().len(), graph.blocks().len());
}

#[test]
fn integer_sum_loop_converges_across_a_backward_edge() {
    let mut sequence = CodeSequence::new();
    let loop_head = sequence.new_label().unwrap();
    let exit = sequence.new_label().unwrap();

    sequence.instruction(Instruction::IConst(SmallIntConstant::Zero));
    sequence.instruction(Instruction::IStore(local(1)));
    sequence.instruction(Instruction::IConst(SmallIntConstant::Zero));
    sequence.instruction(Instruction::IStore(local(2)));
    sequence.bind(loop_head.clone()).unwrap();
    sequence.instruction(Instruction::ILoad(local(1)));
    sequence.instruction(Instruction::ILoad(local(0)));
    sequence.branch(BranchInstruction::IfICmpGe(exit.clone()));
    sequence.instruction(Instruction::ILoad(local(2)));
    sequence.instruction(Instruction::ILoad(local(1)));
    sequence.instruction(Instruction::IAdd);
    sequence.instruction(Instruction::IStore(local(2)));
    sequence.instruction(Instruction::ILoad(local(1)));
    sequence.instruction(Instruction::IConst(SmallIntConstant::One));
    sequence.instruction(Instruction::IAdd);
    sequence.instruction(Instruction::IStore(local(1)));
    sequence.branch(BranchInstruction::Goto(loop_head.clone()));
    sequence.bind(exit).unwrap();
    sequence.instruction(Instruction::ILoad(local(2)));
    sequence.instruction(Instruction::IReturn);

    let graph = ControlFlowGraph::build(&sequence).unwrap();
    let analysis =
        CfgCodeAnalyzer::analyze(&graph, "(I)I", true, &ConstantPool::default()).unwrap();
    let loop_state = analysis
        .entry_state(graph.block_for_label(&loop_head).unwrap())
        .unwrap();
    assert_eq!(
        loop_state.locals,
        [
            FlowLocalState::Integer,
            FlowLocalState::Integer,
            FlowLocalState::Integer
        ]
    );
    assert!(loop_state.operand_stack.is_empty());
    assert_eq!(analysis.max_stack, 2);
    assert_eq!(analysis.max_locals, 3);
}

#[test]
fn a_local_written_on_only_one_path_merges_to_top_and_rejects_a_later_read() {
    let mut sequence = CodeSequence::new();
    let join = sequence.new_label().unwrap();
    let no_write = sequence.new_label().unwrap();

    sequence.instruction(Instruction::ILoad(local(0)));
    sequence.branch(BranchInstruction::IfEq(no_write.clone()));
    sequence.instruction(Instruction::IConst(SmallIntConstant::One));
    sequence.instruction(Instruction::IStore(local(1)));
    sequence.branch(BranchInstruction::Goto(join.clone()));
    sequence.bind(no_write).unwrap();
    sequence.bind(join.clone()).unwrap();
    sequence.instruction(Instruction::ILoad(local(1)));
    sequence.instruction(Instruction::IReturn);

    let graph = ControlFlowGraph::build(&sequence).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze(&graph, "(I)I", true, &ConstantPool::default()),
        Err(AnalysisError::UninitializedLocal {
            instruction: 5,
            index: 1
        })
    ));
}

#[test]
fn a_local_written_on_one_path_is_exposed_as_top_at_the_join() {
    let mut sequence = CodeSequence::new();
    let no_write = sequence.new_label().unwrap();
    let join = sequence.new_label().unwrap();

    sequence.instruction(Instruction::ILoad(local(0)));
    sequence.branch(BranchInstruction::IfEq(no_write.clone()));
    sequence.instruction(Instruction::IConst(SmallIntConstant::One));
    sequence.instruction(Instruction::IStore(local(1)));
    sequence.branch(BranchInstruction::Goto(join.clone()));
    sequence.bind(no_write).unwrap();
    sequence.bind(join.clone()).unwrap();
    sequence.instruction(Instruction::Return);

    let graph = ControlFlowGraph::build(&sequence).unwrap();
    let analysis =
        CfgCodeAnalyzer::analyze(&graph, "(I)V", true, &ConstantPool::default()).unwrap();
    let state = analysis
        .entry_state(graph.block_for_label(&join).unwrap())
        .unwrap();
    assert_eq!(state.locals, [FlowLocalState::Integer, FlowLocalState::Top]);
}

#[test]
fn incompatible_operand_stack_heights_fail_at_the_join() {
    let mut sequence = CodeSequence::new();
    let target = sequence.new_label().unwrap();
    let empty_stack = sequence.new_label().unwrap();

    sequence.instruction(Instruction::IConst(SmallIntConstant::Zero));
    sequence.branch(BranchInstruction::IfEq(empty_stack.clone()));
    sequence.instruction(Instruction::IConst(SmallIntConstant::One));
    sequence.branch(BranchInstruction::Goto(target.clone()));
    sequence.bind(empty_stack).unwrap();
    sequence.branch(BranchInstruction::Goto(target.clone()));
    sequence.bind(target).unwrap();
    sequence.instruction(Instruction::IReturn);

    let graph = ControlFlowGraph::build(&sequence).unwrap();
    let error =
        CfgCodeAnalyzer::analyze(&graph, "()I", true, &ConstantPool::default()).unwrap_err();
    assert!(matches!(
        error,
        AnalysisError::IncompatibleStackMerge {
            block,
            expected_values: 0,
            incoming_values: 1
        } if block.index() == 3
    ));
}

#[test]
fn stack_underflow_and_nonempty_terminal_stack_are_reported() {
    let mut underflow = CodeSequence::new();
    let target = underflow.new_label().unwrap();
    underflow.branch(BranchInstruction::IfEq(target.clone()));
    underflow.bind(target).unwrap();
    underflow.instruction(Instruction::IReturn);
    let graph = ControlFlowGraph::build(&underflow).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze(&graph, "()I", true, &ConstantPool::default()),
        Err(AnalysisError::StackUnderflow {
            instruction: 0,
            needed: 1,
            available: 0
        })
    ));

    let mut nonempty = CodeSequence::new();
    nonempty.instruction(Instruction::IConst(SmallIntConstant::Zero));
    nonempty.instruction(Instruction::Return);
    let graph = ControlFlowGraph::build(&nonempty).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze(&graph, "()V", true, &ConstantPool::default()),
        Err(AnalysisError::NonEmptyStackAtReturn {
            instruction: 1,
            slots: 1
        })
    ));
}

#[test]
fn incorrect_return_opcode_is_reported_against_the_descriptor() {
    let mut sequence = CodeSequence::new();
    sequence.instruction(Instruction::Return);
    let graph = ControlFlowGraph::build(&sequence).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze(&graph, "()I", true, &ConstantPool::default()),
        Err(AnalysisError::WrongReturnOpcode {
            instruction: 0,
            expected: Some(ValueKind::Int),
            actual: None
        })
    ));
}

#[test]
fn unreachable_blocks_have_no_entry_state_and_are_not_transferred() {
    let mut sequence = CodeSequence::new();
    let target = sequence.new_label().unwrap();
    sequence.branch(BranchInstruction::Goto(target.clone()));
    sequence.instruction(Instruction::IConst(SmallIntConstant::Zero));
    sequence.instruction(Instruction::IStore(local(3)));
    sequence.instruction(Instruction::AConstNull);
    sequence.instruction(Instruction::AReturn);
    sequence.bind(target).unwrap();
    sequence.instruction(Instruction::Return);

    let graph = ControlFlowGraph::build(&sequence).unwrap();
    let analysis = CfgCodeAnalyzer::analyze(&graph, "()V", true, &ConstantPool::default()).unwrap();
    assert_eq!(analysis.block_entries()[1], None);
    assert!(analysis.entry_state(graph.entry_block()).is_some());
    assert_eq!(analysis.max_locals, 4);
}

#[test]
fn nonconverging_stack_cycle_and_reachable_fallthrough_are_rejected() {
    let mut cycle = CodeSequence::new();
    let start = cycle.new_label().unwrap();
    cycle.bind(start.clone()).unwrap();
    cycle.instruction(Instruction::IConst(SmallIntConstant::Zero));
    cycle.branch(BranchInstruction::Goto(start));
    let graph = ControlFlowGraph::build(&cycle).unwrap();
    let error =
        CfgCodeAnalyzer::analyze(&graph, "()V", true, &ConstantPool::default()).unwrap_err();
    assert!(matches!(
        error,
        AnalysisError::IncompatibleStackMerge {
            block,
            expected_values: 0,
            incoming_values: 1
        } if block.index() == 0
    ));

    let mut falls_through = CodeSequence::new();
    falls_through.instruction(Instruction::IConst(SmallIntConstant::Zero));
    assert!(matches!(
        ControlFlowGraph::build(&falls_through),
        Err(ControlFlowGraphError::FallthroughPastEnd { .. })
    ));
}

#[test]
fn reference_and_category_two_flows_are_rejected_explicitly() {
    let mut reference = CodeSequence::new();
    reference.instruction(Instruction::AConstNull);
    reference.instruction(Instruction::AReturn);
    let graph = ControlFlowGraph::build(&reference).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze(
            &graph,
            "()Ljava/lang/Object;",
            true,
            &ConstantPool::default()
        ),
        Err(AnalysisError::UnsupportedFlowInstruction {
            instruction: 0,
            kind: UnsupportedFlowInstructionKind::Reference
        })
    ));

    let mut category_two = CodeSequence::new();
    category_two.instruction(Instruction::LConst(SmallLongConstant::Zero));
    category_two.instruction(Instruction::LReturn);
    let graph = ControlFlowGraph::build(&category_two).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze(&graph, "()J", true, &ConstantPool::default()),
        Err(AnalysisError::UnsupportedFlowInstruction {
            instruction: 0,
            kind: UnsupportedFlowInstructionKind::CategoryTwo
        })
    ));
}

#[test]
fn non_integer_parameters_are_rejected_without_changing_linear_analysis() {
    let mut sequence = CodeSequence::new();
    sequence.instruction(Instruction::Return);
    let graph = ControlFlowGraph::build(&sequence).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze(&graph, "(J)V", true, &ConstantPool::default()),
        Err(AnalysisError::UnsupportedFlowParameter {
            parameter: 0,
            actual: ValueKind::Long
        })
    ));
}

#[test]
fn instance_receiver_is_preserved_and_int_parameters_keep_jvm_local_indexes() {
    let mut sequence = CodeSequence::new();
    sequence.instruction(Instruction::ILoad(local(1)));
    sequence.instruction(Instruction::IReturn);
    let graph = ControlFlowGraph::build(&sequence).unwrap();
    let analysis =
        CfgCodeAnalyzer::analyze(&graph, "(I)I", false, &ConstantPool::default()).unwrap();
    let entry = analysis.entry_state(graph.entry_block()).unwrap();
    assert_eq!(
        entry.locals,
        [FlowLocalState::Receiver, FlowLocalState::Integer]
    );
    assert_eq!(analysis.max_locals, 2);
}

#[test]
fn max_locals_limit_rejects_an_unrepresentable_index() {
    let mut sequence = CodeSequence::new();
    sequence.instruction(Instruction::IConst(SmallIntConstant::Zero));
    sequence.instruction(Instruction::IStore(local(u16::MAX)));
    sequence.instruction(Instruction::Return);
    let graph = ControlFlowGraph::build(&sequence).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze(&graph, "()V", true, &ConstantPool::default()),
        Err(AnalysisError::LocalLimitExceeded {
            instruction: 1,
            index: u16::MAX,
            width: 1
        })
    ));
}

#[test]
fn parameter_slot_limit_matches_the_linear_analyzer() {
    let descriptor = format!("({})V", "I".repeat(256));
    let mut sequence = CodeSequence::new();
    sequence.instruction(Instruction::Return);
    let graph = ControlFlowGraph::build(&sequence).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze(&graph, &descriptor, true, &ConstantPool::default()),
        Err(AnalysisError::TooManyMethodParameterSlots {
            instruction: None,
            slots: 256
        })
    ));
}

#[test]
fn max_stack_limit_is_measured_in_jvm_slots() {
    let mut sequence = CodeSequence::new();
    for _ in 0..=u16::MAX {
        sequence.instruction(Instruction::IConst(SmallIntConstant::Zero));
    }
    sequence.instruction(Instruction::Return);
    let graph = ControlFlowGraph::build(&sequence).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze(&graph, "()V", true, &ConstantPool::default()),
        Err(AnalysisError::StackLimitExceeded {
            instruction: 65_535,
            slots: 65_536
        })
    ));
}

#[test]
fn aggregate_block_state_storage_has_a_typed_limit() {
    let mut sequence = CodeSequence::new();
    let entry = sequence.new_label().unwrap();
    sequence.branch(BranchInstruction::Goto(entry.clone()));
    sequence.instruction(Instruction::IConst(SmallIntConstant::Zero));
    sequence.instruction(Instruction::IStore(local(u16::MAX - 1)));
    sequence.bind(entry).unwrap();
    for _ in 0..130 {
        let next = sequence.new_label().unwrap();
        sequence.instruction(Instruction::IConst(SmallIntConstant::Zero));
        sequence.branch(BranchInstruction::IfEq(next.clone()));
        sequence.bind(next).unwrap();
    }
    sequence.instruction(Instruction::Return);

    let graph = ControlFlowGraph::build(&sequence).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze(&graph, "()V", true, &ConstantPool::default()),
        Err(AnalysisError::FlowStateSizeLimitExceeded {
            slots,
            limit: 8_388_608
        }) if slots > 8_388_608
    ));
}
