use dotty_classfile::bytecode::{
    AnalysisError, BranchInstruction, CfgCodeAnalyzer, ClassHierarchy, ClassHierarchyError,
    CodeSequence, ControlFlowGraph, ControlFlowGraphError, FlowLocalState, FlowValue, Instruction,
    LocalIndex, SmallIntConstant, SmallLongConstant, UnsupportedFlowInstructionKind, ValueKind,
    VerificationTypeInfo, assemble_labeled, generate_stack_map_frames,
};
use dotty_classfile::classfile::ConstantPool;
use dotty_classfile::constant_pool_builder::ConstantPoolBuilder;

fn local(index: u16) -> LocalIndex {
    LocalIndex::new(index)
}

#[test]
fn explicit_superclass_proofs_cover_calls_fields_and_reference_returns() {
    let mut pool_builder = ConstantPoolBuilder::new();
    let owner = pool_builder.class("example/Client").unwrap();
    let call = pool_builder
        .method_ref("example/Base", "call", "(Lexample/Base;)Lexample/Base;")
        .unwrap();
    let receiver_call = pool_builder
        .method_ref("example/Base", "touch", "()V")
        .unwrap();
    let field = pool_builder
        .field_ref("example/Base", "value", "Lexample/Base;")
        .unwrap();
    let pool = pool_builder.finish().unwrap();
    let hierarchy = ClassHierarchy::from_edges(vec![(
        "example/Derived".to_owned(),
        "example/Base".to_owned(),
    )])
    .unwrap();

    let mut call_sequence = CodeSequence::new();
    call_sequence
        .instruction(Instruction::ALoad(local(1)))
        .instruction(Instruction::ALoad(local(1)))
        .instruction(Instruction::InvokeVirtual(
            dotty_classfile::bytecode::VirtualMethodReference::new(
                dotty_classfile::bytecode::ConstantPoolIndexOperand::try_from(call).unwrap(),
            ),
        ))
        .instruction(Instruction::AReturn);
    let call_graph = ControlFlowGraph::build(&call_sequence).unwrap();
    let call_result = CfgCodeAnalyzer::analyze_with_hierarchy(
        &call_graph,
        "callBase",
        "(Lexample/Derived;)Lexample/Base;",
        false,
        &pool,
        owner,
        &hierarchy,
    );
    assert!(call_result.is_ok(), "{call_result:?}");
    assert!(matches!(
        CfgCodeAnalyzer::analyze_with_current_class(
            &call_graph,
            "callBase",
            "(Lexample/Derived;)Lexample/Base;",
            false,
            &pool,
            owner,
        ),
        Err(AnalysisError::WrongExactMethodArgument { .. })
    ));
    let mut receiver_sequence = CodeSequence::new();
    receiver_sequence
        .instruction(Instruction::ALoad(local(1)))
        .instruction(Instruction::InvokeVirtual(
            dotty_classfile::bytecode::VirtualMethodReference::new(
                dotty_classfile::bytecode::ConstantPoolIndexOperand::try_from(receiver_call)
                    .unwrap(),
            ),
        ))
        .instruction(Instruction::Return);
    let receiver_graph = ControlFlowGraph::build(&receiver_sequence).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze_with_current_class(
            &receiver_graph,
            "touchBase",
            "(Lexample/Derived;)V",
            false,
            &pool,
            owner,
        ),
        Err(AnalysisError::MethodOwnerMismatch { .. })
    ));
    assert!(
        CfgCodeAnalyzer::analyze_with_hierarchy(
            &receiver_graph,
            "touchBase",
            "(Lexample/Derived;)V",
            false,
            &pool,
            owner,
            &hierarchy,
        )
        .is_ok()
    );
    let reverse_hierarchy = ClassHierarchy::from_edges(vec![(
        "example/Base".to_owned(),
        "example/Derived".to_owned(),
    )])
    .unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze_with_hierarchy(
            &call_graph,
            "callBase",
            "(Lexample/Derived;)Lexample/Base;",
            false,
            &pool,
            owner,
            &reverse_hierarchy,
        ),
        Err(AnalysisError::WrongExactMethodArgument { .. })
    ));
    let unrelated_hierarchy = ClassHierarchy::from_edges(vec![(
        "example/Other".to_owned(),
        "example/Base".to_owned(),
    )])
    .unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze_with_hierarchy(
            &call_graph,
            "callBase",
            "(Lexample/Derived;)Lexample/Base;",
            false,
            &pool,
            owner,
            &unrelated_hierarchy,
        ),
        Err(AnalysisError::WrongExactMethodArgument { .. })
    ));

    let mut field_sequence = CodeSequence::new();
    field_sequence
        .instruction(Instruction::ALoad(local(1)))
        .instruction(Instruction::GetField(
            dotty_classfile::bytecode::FieldReference::new(
                dotty_classfile::bytecode::ConstantPoolIndexOperand::try_from(field).unwrap(),
            ),
        ))
        .instruction(Instruction::AReturn);
    let field_graph = ControlFlowGraph::build(&field_sequence).unwrap();
    assert!(
        CfgCodeAnalyzer::analyze_with_hierarchy(
            &field_graph,
            "readBaseField",
            "(Lexample/Derived;)Lexample/Base;",
            false,
            &pool,
            owner,
            &hierarchy,
        )
        .is_ok()
    );

    let mut write_sequence = CodeSequence::new();
    write_sequence
        .instruction(Instruction::ALoad(local(1)))
        .instruction(Instruction::ALoad(local(2)))
        .instruction(Instruction::PutField(
            dotty_classfile::bytecode::FieldReference::new(
                dotty_classfile::bytecode::ConstantPoolIndexOperand::try_from(field).unwrap(),
            ),
        ))
        .instruction(Instruction::Return);
    let write_graph = ControlFlowGraph::build(&write_sequence).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze_with_current_class(
            &write_graph,
            "writeBaseField",
            "(Lexample/Derived;Lexample/Derived;)V",
            false,
            &pool,
            owner,
        ),
        Err(AnalysisError::WrongExactFieldValue { .. })
    ));
    assert!(
        CfgCodeAnalyzer::analyze_with_hierarchy(
            &write_graph,
            "writeBaseField",
            "(Lexample/Derived;Lexample/Derived;)V",
            false,
            &pool,
            owner,
            &hierarchy,
        )
        .is_ok()
    );

    let mut return_sequence = CodeSequence::new();
    return_sequence
        .instruction(Instruction::ALoad(local(1)))
        .instruction(Instruction::AReturn);
    let return_graph = ControlFlowGraph::build(&return_sequence).unwrap();
    assert!(
        CfgCodeAnalyzer::analyze_with_hierarchy(
            &return_graph,
            "returnBase",
            "(Lexample/Derived;)Lexample/Base;",
            false,
            &pool,
            owner,
            &hierarchy,
        )
        .is_ok()
    );

    let mut loop_sequence = CodeSequence::new();
    let head = loop_sequence.new_label().unwrap();
    let done = loop_sequence.new_label().unwrap();
    loop_sequence
        .bind(head.clone())
        .unwrap()
        .instruction(Instruction::ALoad(local(2)))
        .instruction(Instruction::ALoad(local(2)))
        .instruction(Instruction::InvokeVirtual(
            dotty_classfile::bytecode::VirtualMethodReference::new(
                dotty_classfile::bytecode::ConstantPoolIndexOperand::try_from(call).unwrap(),
            ),
        ))
        .instruction(Instruction::Pop)
        .instruction(Instruction::ILoad(local(1)))
        .branch(BranchInstruction::IfEq(done.clone()))
        .instruction(Instruction::IConst(SmallIntConstant::Zero))
        .instruction(Instruction::IStore(local(1)))
        .branch(BranchInstruction::Goto(head))
        .bind(done)
        .unwrap()
        .instruction(Instruction::Return);
    let loop_graph = ControlFlowGraph::build(&loop_sequence).unwrap();
    assert!(
        CfgCodeAnalyzer::analyze_with_hierarchy(
            &loop_graph,
            "callBaseInLoop",
            "(ILexample/Derived;)V",
            false,
            &pool,
            owner,
            &hierarchy,
        )
        .is_ok()
    );
}

#[test]
fn object_root_assignability_requires_an_explicit_proof() {
    let mut pool_builder = ConstantPoolBuilder::new();
    let owner = pool_builder.class("example/Client").unwrap();
    let pool = pool_builder.finish().unwrap();
    let mut sequence = CodeSequence::new();
    sequence
        .instruction(Instruction::ALoad(local(1)))
        .instruction(Instruction::AReturn);
    let graph = ControlFlowGraph::build(&sequence).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze_with_current_class(
            &graph,
            "objectResult",
            "(Lexample/Derived;)Ljava/lang/Object;",
            false,
            &pool,
            owner,
        ),
        Err(AnalysisError::WrongExactReferenceReturn { .. })
    ));
    let hierarchy = ClassHierarchy::from_edges(vec![(
        "example/Derived".to_owned(),
        "java/lang/Object".to_owned(),
    )])
    .unwrap();
    assert!(
        CfgCodeAnalyzer::analyze_with_hierarchy(
            &graph,
            "objectResult",
            "(Lexample/Derived;)Ljava/lang/Object;",
            false,
            &pool,
            owner,
            &hierarchy,
        )
        .is_ok()
    );
}

#[test]
fn hierarchy_graph_rejects_duplicates_conflicts_cycles_and_excessive_depth() {
    assert!(matches!(
        ClassHierarchy::from_edges(vec![("example.Bad".to_owned(), "example/Base".to_owned())]),
        Err(ClassHierarchyError::InvalidClassName(_))
    ));
    let edge = ("example/Derived".to_owned(), "example/Base".to_owned());
    assert!(matches!(
        ClassHierarchy::from_edges(vec![edge.clone(), edge]),
        Err(ClassHierarchyError::DuplicateEdge { .. })
    ));
    assert!(matches!(
        ClassHierarchy::from_edges(vec![
            ("example/Derived".to_owned(), "example/Base".to_owned()),
            ("example/Derived".to_owned(), "example/Other".to_owned()),
        ]),
        Err(ClassHierarchyError::ConflictingSuperclass { .. })
    ));
    assert!(matches!(
        ClassHierarchy::from_edges(vec![
            ("example/Derived".to_owned(), "example/Base".to_owned()),
            ("example/Base".to_owned(), "example/Derived".to_owned()),
        ]),
        Err(ClassHierarchyError::Cycle { .. })
    ));
    let chain = (0..9)
        .map(|index| {
            (
                format!("example/C{index}"),
                format!("example/C{}", index + 1),
            )
        })
        .collect::<Vec<_>>();
    assert!(matches!(
        ClassHierarchy::from_edges(chain),
        Err(ClassHierarchyError::DepthExceeded { .. })
    ));
}

#[test]
fn sibling_references_do_not_join_to_their_shared_ancestor() {
    let mut pool_builder = ConstantPoolBuilder::new();
    let owner = pool_builder.class("example/Client").unwrap();
    let pool = pool_builder.finish().unwrap();
    let hierarchy = ClassHierarchy::from_edges(vec![
        ("example/Derived".to_owned(), "example/Base".to_owned()),
        ("example/Other".to_owned(), "example/Base".to_owned()),
    ])
    .unwrap();
    let mut sequence = CodeSequence::new();
    let other = sequence.new_label().unwrap();
    let done = sequence.new_label().unwrap();
    sequence
        .instruction(Instruction::ILoad(local(1)))
        .branch(BranchInstruction::IfEq(other.clone()))
        .instruction(Instruction::ALoad(local(2)))
        .branch(BranchInstruction::Goto(done.clone()))
        .bind(other)
        .unwrap()
        .instruction(Instruction::ALoad(local(3)))
        .bind(done)
        .unwrap()
        .instruction(Instruction::AReturn);
    let graph = ControlFlowGraph::build(&sequence).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze_with_hierarchy(
            &graph,
            "noInferredJoin",
            "(ILexample/Derived;Lexample/Other;)Lexample/Base;",
            false,
            &pool,
            owner,
            &hierarchy,
        ),
        Err(AnalysisError::IncompatibleStackValue { .. })
    ));
}

#[test]
fn ancestor_reference_stack_joins_converge_for_reversed_edges_and_backedges() {
    let mut pool_builder = ConstantPoolBuilder::new();
    let owner = pool_builder.class("example/Client").unwrap();
    let pool = pool_builder.finish().unwrap();
    let hierarchy = ClassHierarchy::from_edges([
        ("example/Leaf".to_owned(), "example/Middle".to_owned()),
        ("example/Middle".to_owned(), "example/Base".to_owned()),
    ])
    .unwrap();

    for middle_on_fallthrough in [true, false] {
        let mut sequence = CodeSequence::new();
        let otherwise = sequence.new_label().unwrap();
        let done = sequence.new_label().unwrap();
        sequence
            .instruction(Instruction::ILoad(local(1)))
            .branch(BranchInstruction::IfEq(otherwise.clone()))
            .instruction(Instruction::ALoad(local(if middle_on_fallthrough {
                2
            } else {
                3
            })))
            .branch(BranchInstruction::Goto(done.clone()))
            .bind(otherwise)
            .unwrap()
            .instruction(Instruction::ALoad(local(if middle_on_fallthrough {
                3
            } else {
                2
            })))
            .bind(done.clone())
            .unwrap()
            .instruction(Instruction::AReturn);
        let graph = ControlFlowGraph::build(&sequence).unwrap();
        let flow = CfgCodeAnalyzer::analyze_with_hierarchy(
            &graph,
            "choose",
            "(ZLexample/Middle;Lexample/Leaf;)Lexample/Base;",
            false,
            &pool,
            owner,
            &hierarchy,
        )
        .unwrap();
        let join = flow
            .entry_state(graph.block_for_label(&done).unwrap())
            .unwrap();
        assert!(matches!(
            join.operand_stack.as_slice(),
            [FlowValue::Object(class)] if class.internal_name() == "example/Middle"
        ));
    }

    let mut loop_sequence = CodeSequence::new();
    let head = loop_sequence.new_label().unwrap();
    let done = loop_sequence.new_label().unwrap();
    loop_sequence
        .instruction(Instruction::ALoad(local(3)))
        .branch(BranchInstruction::Goto(head.clone()))
        .bind(head.clone())
        .unwrap()
        .instruction(Instruction::ILoad(local(1)))
        .branch(BranchInstruction::IfEq(done.clone()))
        .instruction(Instruction::Pop)
        .instruction(Instruction::ALoad(local(2)))
        .branch(BranchInstruction::Goto(head.clone()))
        .bind(done.clone())
        .unwrap()
        .instruction(Instruction::AReturn);
    let loop_graph = ControlFlowGraph::build(&loop_sequence).unwrap();
    let loop_flow = CfgCodeAnalyzer::analyze_with_hierarchy(
        &loop_graph,
        "loopJoin",
        "(ZLexample/Middle;Lexample/Leaf;)Lexample/Base;",
        false,
        &pool,
        owner,
        &hierarchy,
    )
    .unwrap();
    let head_state = loop_flow
        .entry_state(loop_graph.block_for_label(&head).unwrap())
        .unwrap();
    let done_state = loop_flow
        .entry_state(loop_graph.block_for_label(&done).unwrap())
        .unwrap();
    for state in [head_state, done_state] {
        assert!(matches!(
            state.operand_stack.as_slice(),
            [FlowValue::Object(class)] if class.internal_name() == "example/Middle"
        ));
    }
}

#[test]
fn sibling_inputs_to_a_multiway_join_are_rejected_in_every_branch_order() {
    let mut pool_builder = ConstantPoolBuilder::new();
    let owner = pool_builder.class("example/Client").unwrap();
    let pool = pool_builder.finish().unwrap();
    let hierarchy = ClassHierarchy::from_edges([
        ("example/Left".to_owned(), "example/Base".to_owned()),
        ("example/Right".to_owned(), "example/Base".to_owned()),
    ])
    .unwrap();

    for order in [[3, 4, 5], [4, 5, 3], [5, 3, 4]] {
        let mut sequence = CodeSequence::new();
        let second = sequence.new_label().unwrap();
        let third = sequence.new_label().unwrap();
        let join = sequence.new_label().unwrap();
        sequence
            .instruction(Instruction::ILoad(local(1)))
            .branch(BranchInstruction::IfEq(second.clone()))
            .instruction(Instruction::ALoad(local(order[0])))
            .branch(BranchInstruction::Goto(join.clone()))
            .bind(second)
            .unwrap()
            .instruction(Instruction::ILoad(local(2)))
            .branch(BranchInstruction::IfEq(third.clone()))
            .instruction(Instruction::ALoad(local(order[1])))
            .branch(BranchInstruction::Goto(join.clone()))
            .bind(third)
            .unwrap()
            .instruction(Instruction::ALoad(local(order[2])))
            .bind(join)
            .unwrap()
            .instruction(Instruction::AReturn);
        let graph = ControlFlowGraph::build(&sequence).unwrap();
        let error = CfgCodeAnalyzer::analyze_with_hierarchy(
            &graph,
            "chooseSiblingInputs",
            "(ZZLexample/Base;Lexample/Left;Lexample/Right;)Lexample/Base;",
            false,
            &pool,
            owner,
            &hierarchy,
        )
        .unwrap_err();
        assert!(matches!(
            &error,
            AnalysisError::IncompatibleStackValue { value: 0, .. }
        ));
        if order == [3, 4, 5] {
            assert!(
                matches!(
                    &error,
                    AnalysisError::IncompatibleStackValue {
                        expected: FlowValue::Object(expected),
                        incoming: FlowValue::Object(incoming),
                        ..
                } if (expected.internal_name() == "example/Left"
                    && incoming.internal_name() == "example/Right")
                    || (expected.internal_name() == "example/Right"
                        && incoming.internal_name() == "example/Left")
                ),
                "unexpected sibling join diagnostic: {error:?}"
            );
        }
    }
}

#[test]
fn connected_joins_keep_incoming_type_evidence_across_empty_blocks() {
    let mut pool_builder = ConstantPoolBuilder::new();
    let owner = pool_builder.class("example/Client").unwrap();
    let pool = pool_builder.finish().unwrap();
    let hierarchy = ClassHierarchy::from_edges([
        ("example/Left".to_owned(), "example/Base".to_owned()),
        ("example/Right".to_owned(), "example/Base".to_owned()),
    ])
    .unwrap();

    for insert_empty_blocks in [false, true] {
        let mut sequence = CodeSequence::new();
        let left = sequence.new_label().unwrap();
        let right = sequence.new_label().unwrap();
        let upstream = sequence.new_label().unwrap();
        let downstream = sequence.new_label().unwrap();
        let empty_one = sequence.new_label().unwrap();
        let empty_two = sequence.new_label().unwrap();
        sequence
            .instruction(Instruction::ILoad(local(1)))
            .branch(BranchInstruction::IfEq(left.clone()))
            .instruction(Instruction::ALoad(local(3)))
            .branch(BranchInstruction::Goto(upstream.clone()))
            .bind(left)
            .unwrap()
            .instruction(Instruction::ALoad(local(4)))
            .bind(upstream)
            .unwrap()
            .instruction(Instruction::ILoad(local(2)))
            .branch(BranchInstruction::IfEq(right.clone()));
        if insert_empty_blocks {
            sequence
                .branch(BranchInstruction::Goto(empty_one.clone()))
                .bind(empty_one)
                .unwrap()
                .branch(BranchInstruction::Goto(empty_two.clone()))
                .bind(empty_two)
                .unwrap()
                .branch(BranchInstruction::Goto(downstream.clone()));
        } else {
            sequence.branch(BranchInstruction::Goto(downstream.clone()));
        }
        sequence
            .bind(right)
            .unwrap()
            .instruction(Instruction::Pop)
            .instruction(Instruction::ALoad(local(5)))
            .branch(BranchInstruction::Goto(downstream.clone()))
            .bind(downstream)
            .unwrap()
            .instruction(Instruction::AReturn);

        let graph = ControlFlowGraph::build(&sequence).unwrap();
        let error = CfgCodeAnalyzer::analyze_with_hierarchy(
            &graph,
            "connectedJoins",
            "(ZZLexample/Base;Lexample/Left;Lexample/Right;)Lexample/Base;",
            false,
            &pool,
            owner,
            &hierarchy,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            AnalysisError::IncompatibleStackValue { .. }
        ));
    }
}

#[test]
fn sibling_input_on_a_backedge_is_compared_with_earlier_join_evidence() {
    let mut pool_builder = ConstantPoolBuilder::new();
    let owner = pool_builder.class("example/Client").unwrap();
    let pool = pool_builder.finish().unwrap();
    let hierarchy = ClassHierarchy::from_edges([
        ("example/Left".to_owned(), "example/Base".to_owned()),
        ("example/Right".to_owned(), "example/Base".to_owned()),
    ])
    .unwrap();
    let mut sequence = CodeSequence::new();
    let left_path = sequence.new_label().unwrap();
    let head = sequence.new_label().unwrap();
    let done = sequence.new_label().unwrap();
    sequence
        .instruction(Instruction::ILoad(local(1)))
        .branch(BranchInstruction::IfEq(left_path.clone()))
        .instruction(Instruction::ALoad(local(2)))
        .branch(BranchInstruction::Goto(head.clone()))
        .bind(left_path)
        .unwrap()
        .instruction(Instruction::ALoad(local(3)))
        .branch(BranchInstruction::Goto(head.clone()))
        .bind(head.clone())
        .unwrap()
        .instruction(Instruction::ILoad(local(1)))
        .branch(BranchInstruction::IfEq(done.clone()))
        .instruction(Instruction::Pop)
        .instruction(Instruction::ALoad(local(4)))
        .branch(BranchInstruction::Goto(head.clone()))
        .bind(done)
        .unwrap()
        .instruction(Instruction::AReturn);
    let graph = ControlFlowGraph::build(&sequence).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze_with_hierarchy(
            &graph,
            "loopSiblingJoin",
            "(ZLexample/Base;Lexample/Left;Lexample/Right;)Lexample/Base;",
            false,
            &pool,
            owner,
            &hierarchy,
        ),
        Err(AnalysisError::IncompatibleStackValue { value: 0, .. })
    ));
}

fn assert_local_object(state: &FlowLocalState, expected: &str) {
    assert!(matches!(state, FlowLocalState::Object(class) if class.internal_name() == expected));
}

#[test]
fn exact_reference_parameters_seed_instance_slots_and_survive_cfg_edges() {
    let mut pool_builder = ConstantPoolBuilder::new();
    let owner = pool_builder.class("example/Client").unwrap();
    let pool = pool_builder.finish().unwrap();

    let mut sequence = CodeSequence::new();
    let otherwise = sequence.new_label().unwrap();
    sequence.instruction(Instruction::ILoad(local(1)));
    sequence.branch(BranchInstruction::IfEq(otherwise.clone()));
    sequence.instruction(Instruction::ALoad(local(2)));
    sequence.instruction(Instruction::AReturn);
    sequence.bind(otherwise.clone()).unwrap();
    sequence.instruction(Instruction::ALoad(local(3)));
    sequence.instruction(Instruction::AReturn);

    let graph = ControlFlowGraph::build(&sequence).unwrap();
    let flow = CfgCodeAnalyzer::analyze_with_current_class(
        &graph,
        "choose",
        "(ZLexample/Counter;Lexample/Counter;)Lexample/Counter;",
        false,
        &pool,
        owner,
    )
    .unwrap();
    let entry_locals = &flow.entry_state(graph.entry_block()).unwrap().locals;
    assert_local_object(&entry_locals[0], "example/Client");
    assert_eq!(entry_locals[1], FlowLocalState::Integer);
    assert_local_object(&entry_locals[2], "example/Counter");
    assert_local_object(&entry_locals[3], "example/Counter");
    let branch_locals = &flow
        .entry_state(graph.block_for_label(&otherwise).unwrap())
        .unwrap()
        .locals;
    assert_local_object(&branch_locals[0], "example/Client");
    assert_eq!(branch_locals[1], FlowLocalState::Integer);
    assert_local_object(&branch_locals[2], "example/Counter");
    assert_local_object(&branch_locals[3], "example/Counter");

    let error = CfgCodeAnalyzer::analyze_with_current_class(
        &graph,
        "choose",
        "(ZLexample/Counter;Lexample/Counter;)Lexample/Other;",
        false,
        &pool,
        owner,
    )
    .unwrap_err();
    assert!(
        matches!(
            &error,
            AnalysisError::WrongExactReferenceReturn {
                instruction: 5,
                expected,
                actual,
            } if expected.internal_name() == "example/Other"
                && actual.internal_name() == "example/Counter"
        ),
        "unexpected exact return error: {error:?}"
    );
}

#[test]
fn exact_reference_descriptor_shapes_and_local_errors_are_typed() {
    let mut pool_builder = ConstantPoolBuilder::new();
    let owner = pool_builder.class("example/Client").unwrap();
    let pool = pool_builder.finish().unwrap();

    let mut empty = CodeSequence::new();
    empty.instruction(Instruction::Return);
    let empty_graph = ControlFlowGraph::build(&empty).unwrap();
    assert_eq!(
        CfgCodeAnalyzer::analyze_with_current_class(
            &empty_graph,
            "invalidName",
            "(Lbad.name;)V",
            false,
            &pool,
            owner,
        ),
        Err(AnalysisError::InvalidDescriptorClassName {
            parameter: Some(0),
            name: "bad.name".to_owned(),
        })
    );
    assert_eq!(
        CfgCodeAnalyzer::analyze_with_current_class(
            &empty_graph,
            "arrayParameter",
            "([Lexample/Counter;)V",
            false,
            &pool,
            owner,
        ),
        Err(AnalysisError::UnsupportedMethodDescriptorType {
            parameter: Some(0),
            actual: dotty_classfile::descriptor::FieldType::Array(Box::new(
                dotty_classfile::descriptor::FieldType::Object("example/Counter".to_owned())
            )),
        })
    );
    assert_eq!(
        CfgCodeAnalyzer::analyze_with_current_class(
            &empty_graph,
            "categoryTwoParameter",
            "(JLexample/Counter;)V",
            false,
            &pool,
            owner,
        ),
        Err(AnalysisError::UnsupportedMethodDescriptorType {
            parameter: Some(0),
            actual: dotty_classfile::descriptor::FieldType::Long,
        })
    );

    let mut wrong_slot = CodeSequence::new();
    wrong_slot.instruction(Instruction::ALoad(local(1)));
    wrong_slot.instruction(Instruction::AReturn);
    let wrong_slot_graph = ControlFlowGraph::build(&wrong_slot).unwrap();
    assert_eq!(
        CfgCodeAnalyzer::analyze_with_current_class(
            &wrong_slot_graph,
            "wrongSlot",
            "(ZLexample/Counter;)Lexample/Counter;",
            false,
            &pool,
            owner,
        ),
        Err(AnalysisError::LocalTypeMismatch {
            instruction: 0,
            index: 1,
            expected: ValueKind::Reference,
            actual: ValueKind::Int,
        })
    );
}

#[test]
fn exact_reference_stack_joins_reject_distinct_class_identities() {
    let mut pool_builder = ConstantPoolBuilder::new();
    let owner = pool_builder.class("example/Client").unwrap();
    let pool = pool_builder.finish().unwrap();

    let mut sequence = CodeSequence::new();
    let otherwise = sequence.new_label().unwrap();
    let join = sequence.new_label().unwrap();
    sequence.instruction(Instruction::ILoad(local(1)));
    sequence.branch(BranchInstruction::IfEq(otherwise.clone()));
    sequence.instruction(Instruction::ALoad(local(2)));
    sequence.branch(BranchInstruction::Goto(join.clone()));
    sequence.bind(otherwise).unwrap();
    sequence.instruction(Instruction::ALoad(local(3)));
    sequence.bind(join.clone()).unwrap();
    sequence.instruction(Instruction::AReturn);

    let graph = ControlFlowGraph::build(&sequence).unwrap();
    let error = CfgCodeAnalyzer::analyze_with_current_class(
        &graph,
        "joinDifferentClasses",
        "(ZLexample/A;Lexample/B;)Lexample/A;",
        false,
        &pool,
        owner,
    )
    .unwrap_err();
    assert!(
        matches!(
            &error,
            AnalysisError::IncompatibleStackValue {
                value: 0,
                expected: FlowValue::Object(expected),
                incoming: FlowValue::Object(incoming),
                ..
            } if expected.internal_name() == "example/B"
                && incoming.internal_name() == "example/A"
        ),
        "unexpected exact stack-join error: {error:?}"
    );
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
    assert_eq!(join_state.operand_stack, [FlowValue::Integer]);
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
fn integer_arithmetic_preserves_loop_and_if_join_frames_deterministically() {
    let mut sequence = CodeSequence::new();
    let loop_head = sequence.new_label().unwrap();
    let choose_division = sequence.new_label().unwrap();
    let otherwise = sequence.new_label().unwrap();
    let join = sequence.new_label().unwrap();

    sequence.instruction(Instruction::ILoad(local(0)));
    sequence.instruction(Instruction::IStore(local(1)));
    sequence.bind(loop_head.clone()).unwrap();
    sequence.instruction(Instruction::ILoad(local(1)));
    sequence.branch(BranchInstruction::IfEq(choose_division.clone()));
    sequence.instruction(Instruction::ILoad(local(1)));
    sequence.instruction(Instruction::IConst(SmallIntConstant::One));
    sequence.instruction(Instruction::ISub);
    sequence.instruction(Instruction::IStore(local(1)));
    sequence.branch(BranchInstruction::Goto(loop_head.clone()));

    sequence.bind(choose_division.clone()).unwrap();
    sequence.instruction(Instruction::ILoad(local(0)));
    sequence.branch(BranchInstruction::IfEq(otherwise.clone()));
    sequence.instruction(Instruction::ILoad(local(0)));
    sequence.instruction(Instruction::IConst(SmallIntConstant::Two));
    sequence.instruction(Instruction::IMul);
    sequence.branch(BranchInstruction::Goto(join.clone()));

    sequence.bind(otherwise.clone()).unwrap();
    sequence.instruction(Instruction::ILoad(local(0)));
    sequence.instruction(Instruction::IConst(SmallIntConstant::Two));
    sequence.instruction(Instruction::IDiv);
    sequence.bind(join.clone()).unwrap();
    sequence.instruction(Instruction::IConst(SmallIntConstant::Three));
    sequence.instruction(Instruction::IRem);
    sequence.instruction(Instruction::IReturn);

    let graph = ControlFlowGraph::build(&sequence).unwrap();
    let first = CfgCodeAnalyzer::analyze(&graph, "(I)I", true, &ConstantPool::default()).unwrap();
    let second = CfgCodeAnalyzer::analyze(&graph, "(I)I", true, &ConstantPool::default()).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.max_stack, 2);
    assert_eq!(first.max_locals, 2);
    for label in [&loop_head, &join] {
        let state = first
            .entry_state(graph.block_for_label(label).unwrap())
            .unwrap();
        assert_eq!(
            state.locals,
            [FlowLocalState::Integer, FlowLocalState::Integer]
        );
    }
    let assembled = assemble_labeled(&sequence, &ConstantPool::default()).unwrap();
    let first_frames = generate_stack_map_frames(&first, &graph, &assembled).unwrap();
    let second_frames = generate_stack_map_frames(&second, &graph, &assembled).unwrap();
    assert_eq!(first_frames, second_frames);
    assert_eq!(
        first_frames.code_analysis,
        dotty_classfile::bytecode::CodeAnalysis {
            max_stack: 2,
            max_locals: 2,
        }
    );
}

#[test]
fn integer_arithmetic_underflow_is_reported_by_cfg_analysis() {
    for operation in [
        Instruction::ISub,
        Instruction::IMul,
        Instruction::IDiv,
        Instruction::IRem,
    ] {
        let mut sequence = CodeSequence::new();
        sequence.instruction(operation);
        sequence.instruction(Instruction::IReturn);
        let graph = ControlFlowGraph::build(&sequence).unwrap();
        assert_eq!(
            CfgCodeAnalyzer::analyze(&graph, "()I", true, &ConstantPool::default()),
            Err(AnalysisError::StackUnderflow {
                instruction: 0,
                needed: 1,
                available: 0,
            }),
            "{operation:?}"
        );
    }
}

#[test]
fn int_bitwise_ops_preserve_integer_branch_join_states_and_frames() {
    let mut sequence = CodeSequence::new();
    let otherwise = sequence.new_label().unwrap();
    let join = sequence.new_label().unwrap();
    sequence.instruction(Instruction::ILoad(local(0)));
    sequence.instruction(Instruction::ILoad(local(1)));
    sequence.instruction(Instruction::IAnd);
    sequence.branch(BranchInstruction::IfEq(otherwise.clone()));
    sequence.instruction(Instruction::ILoad(local(0)));
    sequence.instruction(Instruction::ILoad(local(1)));
    sequence.instruction(Instruction::IOr);
    sequence.branch(BranchInstruction::Goto(join.clone()));
    sequence.bind(otherwise.clone()).unwrap();
    sequence.instruction(Instruction::ILoad(local(0)));
    sequence.instruction(Instruction::ILoad(local(1)));
    sequence.instruction(Instruction::IXor);
    sequence.bind(join.clone()).unwrap();
    sequence.instruction(Instruction::IReturn);

    let graph = ControlFlowGraph::build(&sequence).unwrap();
    let flow = CfgCodeAnalyzer::analyze(&graph, "(II)I", true, &ConstantPool::default()).unwrap();
    assert_eq!(flow.max_stack, 2);
    assert_eq!(flow.max_locals, 2);
    let assembled = assemble_labeled(&sequence, &ConstantPool::default()).unwrap();
    assert_eq!(
        assembled
            .bytes
            .iter()
            .copied()
            .filter(|byte| [0x7e, 0x80, 0x82].contains(byte))
            .collect::<Vec<_>>(),
        [0x7e, 0x80, 0x82]
    );
    let frames = generate_stack_map_frames(&flow, &graph, &assembled).unwrap();
    assert_eq!(frames.frames.len(), 2);
    assert_eq!(frames.frames[0].stack, []);
    assert_eq!(frames.frames[1].stack, [VerificationTypeInfo::Integer]);
}

#[test]
fn int_bitwise_ops_reject_category_two_values_and_underflow_in_cfg() {
    for operation in [Instruction::IAnd, Instruction::IOr, Instruction::IXor] {
        let mut underflow = CodeSequence::new();
        underflow.instruction(operation);
        underflow.instruction(Instruction::IReturn);
        let graph = ControlFlowGraph::build(&underflow).unwrap();
        assert_eq!(
            CfgCodeAnalyzer::analyze(&graph, "()I", true, &ConstantPool::default()),
            Err(AnalysisError::StackUnderflow {
                instruction: 0,
                needed: 1,
                available: 0,
            }),
            "{operation:?}"
        );
    }
    let mut sequence = CodeSequence::new();
    sequence.instruction(Instruction::LLoad(local(0)));
    sequence.instruction(Instruction::IConst(SmallIntConstant::One));
    sequence.instruction(Instruction::IXor);
    sequence.instruction(Instruction::IReturn);
    let graph = ControlFlowGraph::build(&sequence).unwrap();
    assert_eq!(
        CfgCodeAnalyzer::analyze(&graph, "(J)I", true, &ConstantPool::default()),
        Err(AnalysisError::UnsupportedFlowParameter {
            parameter: 0,
            actual: ValueKind::Long,
        })
    );
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
        &error,
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
        &error,
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
