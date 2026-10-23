use dotty_classfile::bytecode::{
    AnalysisError, BranchInstruction, CfgCodeAnalyzer, ClassHierarchy, ClassHierarchyError,
    CodeSequence, ControlFlowGraph, ControlFlowGraphError, FlowLocalState, FlowValue, Instruction,
    Ldc2Constant, LocalIndex, SmallIntConstant, SmallLongConstant, UnsupportedFlowInstructionKind,
    ValueKind, VerificationTypeInfo, assemble_labeled, generate_stack_map_frames,
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
fn reference_branches_pop_initialized_references_and_null() {
    let mut pool_builder = ConstantPoolBuilder::new();
    let owner = pool_builder.class("example/Client").unwrap();
    let pool = pool_builder.finish().unwrap();

    let mut identity = CodeSequence::new();
    let equal = identity.new_label().unwrap();
    identity
        .instruction(Instruction::ALoad(local(1)))
        .instruction(Instruction::ALoad(local(2)))
        .branch(BranchInstruction::IfACmpEq(equal.clone()))
        .instruction(Instruction::IConst(SmallIntConstant::Zero))
        .instruction(Instruction::IReturn)
        .bind(equal)
        .unwrap()
        .instruction(Instruction::IConst(SmallIntConstant::One))
        .instruction(Instruction::IReturn);
    let graph = ControlFlowGraph::build(&identity).unwrap();
    let flow = CfgCodeAnalyzer::analyze_with_current_class(
        &graph,
        "sameIdentity",
        "(Lexample/Left;Lexample/Right;)I",
        false,
        &pool,
        owner,
    )
    .unwrap();
    assert!(graph.blocks()[0].successors.len() == 2);
    for successor in &graph.blocks()[0].successors {
        assert!(
            flow.entry_state(*successor)
                .unwrap()
                .operand_stack
                .is_empty()
        );
    }

    let mut not_identity = CodeSequence::new();
    let different = not_identity.new_label().unwrap();
    not_identity
        .instruction(Instruction::ALoad(local(1)))
        .instruction(Instruction::ALoad(local(2)))
        .branch(BranchInstruction::IfACmpNe(different.clone()))
        .instruction(Instruction::IConst(SmallIntConstant::Zero))
        .instruction(Instruction::IReturn)
        .bind(different)
        .unwrap()
        .instruction(Instruction::IConst(SmallIntConstant::One))
        .instruction(Instruction::IReturn);
    let graph = ControlFlowGraph::build(&not_identity).unwrap();
    assert!(
        CfgCodeAnalyzer::analyze_with_current_class(
            &graph,
            "differentIdentity",
            "(Ljava/lang/Object;Ljava/lang/Object;)I",
            false,
            &pool,
            owner,
        )
        .is_ok()
    );

    let mut null = CodeSequence::new();
    let is_null = null.new_label().unwrap();
    null.instruction(Instruction::AConstNull)
        .branch(BranchInstruction::IfNull(is_null.clone()))
        .instruction(Instruction::IConst(SmallIntConstant::Zero))
        .instruction(Instruction::IReturn)
        .bind(is_null)
        .unwrap()
        .instruction(Instruction::IConst(SmallIntConstant::One))
        .instruction(Instruction::IReturn);
    let graph = ControlFlowGraph::build(&null).unwrap();
    let flow =
        CfgCodeAnalyzer::analyze_with_current_class(&graph, "isNull", "()I", false, &pool, owner)
            .unwrap();
    for successor in &graph.blocks()[0].successors {
        assert!(
            flow.entry_state(*successor)
                .unwrap()
                .operand_stack
                .is_empty()
        );
    }

    let mut nonnull = CodeSequence::new();
    let has_value = nonnull.new_label().unwrap();
    nonnull
        .instruction(Instruction::ALoad(local(1)))
        .branch(BranchInstruction::IfNonNull(has_value.clone()))
        .instruction(Instruction::IConst(SmallIntConstant::Zero))
        .instruction(Instruction::IReturn)
        .bind(has_value)
        .unwrap()
        .instruction(Instruction::IConst(SmallIntConstant::One))
        .instruction(Instruction::IReturn);
    let graph = ControlFlowGraph::build(&nonnull).unwrap();
    assert!(
        CfgCodeAnalyzer::analyze_with_current_class(
            &graph,
            "isNonNull",
            "(Ljava/lang/Object;)I",
            false,
            &pool,
            owner,
        )
        .is_ok()
    );
}

#[test]
fn reference_branches_reject_integer_and_uninitialized_operands() {
    let mut pool_builder = ConstantPoolBuilder::new();
    let owner = pool_builder.class("example/Client").unwrap();
    let fresh_class = pool_builder.class("example/Fresh").unwrap();
    let pool = pool_builder.finish().unwrap();

    let mut integer = CodeSequence::new();
    let target = integer.new_label().unwrap();
    integer
        .instruction(Instruction::IConst(SmallIntConstant::Zero))
        .branch(BranchInstruction::IfNull(target.clone()))
        .bind(target)
        .unwrap()
        .instruction(Instruction::Return);
    let graph = ControlFlowGraph::build(&integer).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze_with_current_class(
            &graph,
            "badNullOperand",
            "()V",
            false,
            &pool,
            owner
        ),
        Err(AnalysisError::StackTypeMismatch {
            instruction: 1,
            expected: ValueKind::Reference,
            actual: ValueKind::Int,
        })
    ));

    let mut missing_reference = CodeSequence::new();
    let target = missing_reference.new_label().unwrap();
    missing_reference
        .instruction(Instruction::ALoad(local(1)))
        .branch(BranchInstruction::IfACmpEq(target.clone()))
        .bind(target)
        .unwrap()
        .instruction(Instruction::Return);
    let graph = ControlFlowGraph::build(&missing_reference).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze_with_current_class(
            &graph,
            "missingReferenceOperand",
            "(Ljava/lang/Object;)V",
            false,
            &pool,
            owner
        ),
        Err(AnalysisError::StackUnderflow {
            instruction: 1,
            needed: 1,
            available: 0,
        })
    ));

    let mut uninitialized = CodeSequence::new();
    let target = uninitialized.new_label().unwrap();
    uninitialized
        .instruction(Instruction::New(
            dotty_classfile::bytecode::ClassReference::new(
                dotty_classfile::bytecode::ConstantPoolIndexOperand::try_from(fresh_class).unwrap(),
            ),
        ))
        .branch(BranchInstruction::IfNonNull(target.clone()))
        .bind(target)
        .unwrap()
        .instruction(Instruction::Return);
    let graph = ControlFlowGraph::build(&uninitialized).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze_with_current_class(
            &graph,
            "badUninitializedOperand",
            "()V",
            false,
            &pool,
            owner
        ),
        Err(AnalysisError::UninitializedReferenceUse {
            instruction: 1,
            allocation_site: 0,
        })
    ));
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
fn sibling_references_join_to_their_checked_shared_ancestor() {
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
    let flow = CfgCodeAnalyzer::analyze_with_hierarchy(
        &graph,
        "checkedSiblingJoin",
        "(ILexample/Derived;Lexample/Other;)Lexample/Base;",
        false,
        &pool,
        owner,
        &hierarchy,
    )
    .unwrap();
    let join = graph.blocks().last().unwrap().id;
    assert!(matches!(
        flow.entry_state(join).unwrap().operand_stack.as_slice(),
        [FlowValue::Object(class)] if class.internal_name() == "example/Base"
    ));
}

#[test]
fn initialized_sibling_references_join_in_the_same_local_slot() {
    let mut pool_builder = ConstantPoolBuilder::new();
    let owner = pool_builder.class("example/Client").unwrap();
    let pool = pool_builder.finish().unwrap();
    let hierarchy = ClassHierarchy::from_edges([
        ("example/Left".to_owned(), "example/Base".to_owned()),
        ("example/Right".to_owned(), "example/Base".to_owned()),
    ])
    .unwrap();

    for left_on_fallthrough in [true, false] {
        let mut sequence = CodeSequence::new();
        let alternate = sequence.new_label().unwrap();
        let join = sequence.new_label().unwrap();
        sequence
            .instruction(Instruction::ILoad(local(1)))
            .branch(BranchInstruction::IfEq(alternate.clone()))
            .instruction(Instruction::ALoad(local(if left_on_fallthrough {
                2
            } else {
                3
            })))
            .instruction(Instruction::AStore(local(4)))
            .branch(BranchInstruction::Goto(join.clone()))
            .bind(alternate)
            .unwrap()
            .instruction(Instruction::ALoad(local(if left_on_fallthrough {
                3
            } else {
                2
            })))
            .instruction(Instruction::AStore(local(4)))
            .bind(join.clone())
            .unwrap()
            .instruction(Instruction::ALoad(local(4)))
            .instruction(Instruction::Pop)
            .instruction(Instruction::Return);
        let graph = ControlFlowGraph::build(&sequence).unwrap();
        let flow = CfgCodeAnalyzer::analyze_with_hierarchy(
            &graph,
            "localJoin",
            "(ZLexample/Left;Lexample/Right;)V",
            false,
            &pool,
            owner,
            &hierarchy,
        )
        .unwrap();
        let join_state = flow
            .entry_state(graph.block_for_label(&join).unwrap())
            .unwrap();
        assert!(matches!(
            &join_state.locals[4],
            FlowLocalState::Object(class) if class.internal_name() == "example/Base"
        ));
    }
}

#[test]
fn local_reference_loop_header_widens_and_top_stays_unreadable() {
    let mut pool_builder = ConstantPoolBuilder::new();
    let owner = pool_builder.class("example/Client").unwrap();
    let pool = pool_builder.finish().unwrap();
    let hierarchy = ClassHierarchy::from_edges([
        ("example/Left".to_owned(), "example/Base".to_owned()),
        ("example/Right".to_owned(), "example/Base".to_owned()),
    ])
    .unwrap();

    let mut loop_sequence = CodeSequence::new();
    let head = loop_sequence.new_label().unwrap();
    let done = loop_sequence.new_label().unwrap();
    loop_sequence
        .instruction(Instruction::ALoad(local(2)))
        .instruction(Instruction::AStore(local(4)))
        .bind(head.clone())
        .unwrap()
        .instruction(Instruction::ILoad(local(1)))
        .branch(BranchInstruction::IfEq(done.clone()))
        .instruction(Instruction::ALoad(local(3)))
        .instruction(Instruction::AStore(local(4)))
        .branch(BranchInstruction::Goto(head.clone()))
        .bind(done)
        .unwrap()
        .instruction(Instruction::ALoad(local(4)))
        .instruction(Instruction::Pop)
        .instruction(Instruction::Return);
    let loop_graph = ControlFlowGraph::build(&loop_sequence).unwrap();
    let loop_flow = CfgCodeAnalyzer::analyze_with_hierarchy(
        &loop_graph,
        "loopLocalJoin",
        "(ZLexample/Left;Lexample/Right;)V",
        false,
        &pool,
        owner,
        &hierarchy,
    )
    .unwrap();
    let head_state = loop_flow
        .entry_state(loop_graph.block_for_label(&head).unwrap())
        .unwrap();
    assert!(matches!(
        &head_state.locals[4],
        FlowLocalState::Object(class) if class.internal_name() == "example/Base"
    ));

    let mut partial_sequence = CodeSequence::new();
    let unassigned = partial_sequence.new_label().unwrap();
    let join = partial_sequence.new_label().unwrap();
    partial_sequence
        .instruction(Instruction::ILoad(local(1)))
        .branch(BranchInstruction::IfEq(unassigned.clone()))
        .instruction(Instruction::ALoad(local(2)))
        .instruction(Instruction::AStore(local(4)))
        .branch(BranchInstruction::Goto(join.clone()))
        .bind(unassigned)
        .unwrap()
        .bind(join.clone())
        .unwrap()
        .instruction(Instruction::ALoad(local(4)))
        .instruction(Instruction::Pop)
        .instruction(Instruction::Return);
    let partial_graph = ControlFlowGraph::build(&partial_sequence).unwrap();
    let partial_result = CfgCodeAnalyzer::analyze_with_hierarchy(
        &partial_graph,
        "topLocalJoin",
        "(ZLexample/Left;)V",
        false,
        &pool,
        owner,
        &hierarchy,
    );
    assert!(
        matches!(
            &partial_result,
            Err(AnalysisError::UninitializedLocal {
                index: 4,
                instruction: 5
            })
        ),
        "unexpected analysis result: {partial_result:?}"
    );
}

#[test]
fn missing_or_object_only_local_ancestry_does_not_create_a_join() {
    let mut pool_builder = ConstantPoolBuilder::new();
    let owner = pool_builder.class("example/Client").unwrap();
    let pool = pool_builder.finish().unwrap();
    let hierarchies = [
        ClassHierarchy::default(),
        ClassHierarchy::from_edges([
            ("example/Left".to_owned(), "java/lang/Object".to_owned()),
            ("example/Right".to_owned(), "java/lang/Object".to_owned()),
        ])
        .unwrap(),
    ];

    for hierarchy in hierarchies {
        let mut sequence = CodeSequence::new();
        let alternate = sequence.new_label().unwrap();
        let join = sequence.new_label().unwrap();
        sequence
            .instruction(Instruction::ILoad(local(1)))
            .branch(BranchInstruction::IfEq(alternate.clone()))
            .instruction(Instruction::ALoad(local(2)))
            .instruction(Instruction::AStore(local(4)))
            .branch(BranchInstruction::Goto(join.clone()))
            .bind(alternate)
            .unwrap()
            .instruction(Instruction::ALoad(local(3)))
            .instruction(Instruction::AStore(local(4)))
            .bind(join.clone())
            .unwrap()
            .instruction(Instruction::ALoad(local(4)))
            .instruction(Instruction::Pop)
            .instruction(Instruction::Return);
        let graph = ControlFlowGraph::build(&sequence).unwrap();
        let result = CfgCodeAnalyzer::analyze_with_hierarchy(
            &graph,
            "rejectUnprovenLocalJoin",
            "(ZLexample/Left;Lexample/Right;)V",
            false,
            &pool,
            owner,
            &hierarchy,
        );
        assert!(matches!(
            result,
            Err(AnalysisError::UninitializedLocal { index: 4, .. })
        ));
    }
}

#[test]
fn exact_object_locals_survive_diamonds_and_backedges_without_a_hierarchy() {
    let mut pool_builder = ConstantPoolBuilder::new();
    let owner = pool_builder.class("example/Client").unwrap();
    let pool = pool_builder.finish().unwrap();

    let mut diamond = CodeSequence::new();
    let alternate = diamond.new_label().unwrap();
    let join = diamond.new_label().unwrap();
    diamond
        .instruction(Instruction::ILoad(local(1)))
        .branch(BranchInstruction::IfEq(alternate.clone()))
        .branch(BranchInstruction::Goto(join.clone()))
        .bind(alternate)
        .unwrap()
        .branch(BranchInstruction::Goto(join.clone()))
        .bind(join.clone())
        .unwrap()
        .instruction(Instruction::ALoad(local(2)))
        .instruction(Instruction::AReturn);
    let diamond_graph = ControlFlowGraph::build(&diamond).unwrap();
    let diamond_flow = CfgCodeAnalyzer::analyze_with_current_class(
        &diamond_graph,
        "exactObjectDiamond",
        "(ZLjava/lang/Object;)Ljava/lang/Object;",
        false,
        &pool,
        owner,
    )
    .unwrap();
    let diamond_join = diamond_flow
        .entry_state(diamond_graph.block_for_label(&join).unwrap())
        .unwrap();
    assert!(matches!(
        &diamond_join.locals[2],
        FlowLocalState::Object(class) if class.internal_name() == "java/lang/Object"
    ));

    let mut loop_sequence = CodeSequence::new();
    let head = loop_sequence.new_label().unwrap();
    let done = loop_sequence.new_label().unwrap();
    loop_sequence
        .bind(head.clone())
        .unwrap()
        .instruction(Instruction::ILoad(local(1)))
        .branch(BranchInstruction::IfEq(done.clone()))
        .branch(BranchInstruction::Goto(head.clone()))
        .bind(done)
        .unwrap()
        .instruction(Instruction::ALoad(local(2)))
        .instruction(Instruction::AReturn);
    let loop_graph = ControlFlowGraph::build(&loop_sequence).unwrap();
    let loop_flow = CfgCodeAnalyzer::analyze_with_current_class(
        &loop_graph,
        "exactObjectBackedge",
        "(ZLjava/lang/Object;)Ljava/lang/Object;",
        false,
        &pool,
        owner,
    )
    .unwrap();
    let loop_head = loop_flow
        .entry_state(loop_graph.block_for_label(&head).unwrap())
        .unwrap();
    assert!(matches!(
        &loop_head.locals[2],
        FlowLocalState::Object(class) if class.internal_name() == "java/lang/Object"
    ));
}

#[test]
fn sibling_join_requires_a_checked_non_object_path_from_both_inputs() {
    let mut pool_builder = ConstantPoolBuilder::new();
    let owner = pool_builder.class("example/Client").unwrap();
    let pool = pool_builder.finish().unwrap();
    let mut sequence = CodeSequence::new();
    let otherwise = sequence.new_label().unwrap();
    let join = sequence.new_label().unwrap();
    sequence
        .instruction(Instruction::ILoad(local(1)))
        .branch(BranchInstruction::IfEq(otherwise.clone()))
        .instruction(Instruction::ALoad(local(2)))
        .branch(BranchInstruction::Goto(join.clone()))
        .bind(otherwise)
        .unwrap()
        .instruction(Instruction::ALoad(local(3)))
        .bind(join)
        .unwrap()
        .instruction(Instruction::AReturn);
    let graph = ControlFlowGraph::build(&sequence).unwrap();

    for (hierarchy, return_type) in [
        (
            ClassHierarchy::from_edges([("example/Left".to_owned(), "example/Base".to_owned())])
                .unwrap(),
            "example/Base",
        ),
        (
            ClassHierarchy::from_edges([
                ("example/Left".to_owned(), "java/lang/Object".to_owned()),
                ("example/Right".to_owned(), "java/lang/Object".to_owned()),
            ])
            .unwrap(),
            "java/lang/Object",
        ),
    ] {
        let descriptor = format!("(ZLexample/Left;Lexample/Right;)L{return_type};");
        let result = CfgCodeAnalyzer::analyze_with_hierarchy(
            &graph,
            "rejectUnprovenSiblingJoin",
            &descriptor,
            false,
            &pool,
            owner,
            &hierarchy,
        );
        assert!(
            matches!(
                result,
                Err(AnalysisError::IncompatibleStackValue { value: 0, .. })
            ),
            "unexpected result: {result:?}"
        );
    }
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
fn sibling_and_base_inputs_to_a_multiway_join_use_the_same_base_in_every_order() {
    let mut pool_builder = ConstantPoolBuilder::new();
    let owner = pool_builder.class("example/Client").unwrap();
    let pool = pool_builder.finish().unwrap();
    let hierarchy = ClassHierarchy::from_edges([
        ("example/Left".to_owned(), "example/Base".to_owned()),
        ("example/Right".to_owned(), "example/Base".to_owned()),
    ])
    .unwrap();

    for order in [
        [3, 4, 5],
        [3, 5, 4],
        [4, 3, 5],
        [4, 5, 3],
        [5, 3, 4],
        [5, 4, 3],
    ] {
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
        let flow = CfgCodeAnalyzer::analyze_with_hierarchy(
            &graph,
            "chooseSiblingInputs",
            "(ZZLexample/Base;Lexample/Left;Lexample/Right;)Lexample/Base;",
            false,
            &pool,
            owner,
            &hierarchy,
        )
        .unwrap();
        let join_state = flow.entry_state(graph.blocks().last().unwrap().id).unwrap();
        assert!(matches!(
            join_state.operand_stack.as_slice(),
            [FlowValue::Object(class)] if class.internal_name() == "example/Base"
        ));
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
        let flow = CfgCodeAnalyzer::analyze_with_hierarchy(
            &graph,
            "connectedJoins",
            "(ZZLexample/Base;Lexample/Left;Lexample/Right;)Lexample/Base;",
            false,
            &pool,
            owner,
            &hierarchy,
        )
        .unwrap();
        assert!(flow.block_entries().iter().flatten().any(|state| matches!(
            state.operand_stack.as_slice(),
            [FlowValue::Object(class)] if class.internal_name() == "example/Base"
        )));
    }
}

#[test]
fn sibling_input_on_a_backedge_converges_to_the_shared_ancestor() {
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
    let flow = CfgCodeAnalyzer::analyze_with_hierarchy(
        &graph,
        "loopSiblingJoin",
        "(ZLexample/Base;Lexample/Left;Lexample/Right;)Lexample/Base;",
        false,
        &pool,
        owner,
        &hierarchy,
    )
    .unwrap();
    let head_state = flow
        .entry_state(graph.block_for_label(&head).unwrap())
        .unwrap();
    assert!(matches!(
        head_state.operand_stack.as_slice(),
        [FlowValue::Object(class)] if class.internal_name() == "example/Base"
    ));
}

#[test]
fn wide_sibling_evidence_and_shared_tail_stay_within_the_worklist_bound() {
    const SIBLING_COUNT: usize = 150;
    const TAIL_BLOCK_COUNT: usize = 150;

    let mut pool_builder = ConstantPoolBuilder::new();
    let owner = pool_builder.class("example/Client").unwrap();
    let methods = (0..SIBLING_COUNT)
        .map(|index| {
            pool_builder
                .method_ref(
                    "example/Client",
                    &format!("make{index}"),
                    &format!("()Lexample/Sibling{index};"),
                )
                .unwrap()
        })
        .collect::<Vec<_>>();
    let pool = pool_builder.finish().unwrap();
    let hierarchy = ClassHierarchy::from_edges(
        (0..SIBLING_COUNT)
            .map(|index| (format!("example/Sibling{index}"), "example/Base".to_owned()))
            .chain([("example/Base".to_owned(), "java/lang/Object".to_owned())]),
    )
    .unwrap();

    let mut sequence = CodeSequence::new();
    let next_paths = (0..SIBLING_COUNT - 1)
        .map(|_| sequence.new_label().unwrap())
        .collect::<Vec<_>>();
    let join = sequence.new_label().unwrap();
    for (index, next) in next_paths.iter().enumerate() {
        sequence
            .instruction(Instruction::ILoad(local(1)))
            .branch(BranchInstruction::IfEq(next.clone()))
            .instruction(Instruction::ALoad(local(0)))
            .instruction(Instruction::InvokeVirtual(
                dotty_classfile::bytecode::VirtualMethodReference::new(
                    dotty_classfile::bytecode::ConstantPoolIndexOperand::try_from(methods[index])
                        .unwrap(),
                ),
            ))
            .branch(BranchInstruction::Goto(join.clone()))
            .bind(next.clone())
            .unwrap();
    }
    sequence
        .instruction(Instruction::ALoad(local(0)))
        .instruction(Instruction::InvokeVirtual(
            dotty_classfile::bytecode::VirtualMethodReference::new(
                dotty_classfile::bytecode::ConstantPoolIndexOperand::try_from(
                    methods[SIBLING_COUNT - 1],
                )
                .unwrap(),
            ),
        ))
        .branch(BranchInstruction::Goto(join.clone()))
        .bind(join)
        .unwrap();
    let tail = (0..TAIL_BLOCK_COUNT)
        .map(|_| sequence.new_label().unwrap())
        .collect::<Vec<_>>();
    sequence.branch(BranchInstruction::Goto(tail[0].clone()));
    for (index, label) in tail.iter().enumerate() {
        sequence.bind(label.clone()).unwrap();
        if let Some(next) = tail.get(index + 1) {
            sequence.branch(BranchInstruction::Goto(next.clone()));
        }
    }
    sequence.instruction(Instruction::AReturn);

    let graph = ControlFlowGraph::build(&sequence).unwrap();
    let flow = CfgCodeAnalyzer::analyze_with_hierarchy(
        &graph,
        "chooseWideSibling",
        "(Z)Lexample/Base;",
        false,
        &pool,
        owner,
        &hierarchy,
    )
    .unwrap();
    assert!(flow.block_entries().iter().flatten().any(|state| matches!(
        state.operand_stack.as_slice(),
        [FlowValue::Object(class)] if class.internal_name() == "example/Base"
    )));
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
        Err(AnalysisError::StackTypeMismatch {
            instruction: 2,
            expected: ValueKind::Int,
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
fn reference_flow_is_rejected_and_category_two_opcode_errors_are_typed() {
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
    category_two.instruction(Instruction::Dup);
    category_two.instruction(Instruction::LReturn);
    let graph = ControlFlowGraph::build(&category_two).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze(&graph, "()J", true, &ConstantPool::default()),
        Err(AnalysisError::DupCategoryTwo {
            instruction: 1,
            actual: ValueKind::Long
        })
    ));
}

#[test]
fn unsupported_parameters_are_rejected_without_changing_linear_analysis() {
    let mut sequence = CodeSequence::new();
    sequence.instruction(Instruction::Return);
    let graph = ControlFlowGraph::build(&sequence).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze(&graph, "(F)V", true, &ConstantPool::default()),
        Err(AnalysisError::UnsupportedFlowParameter {
            parameter: 0,
            actual: ValueKind::Float
        })
    ));
}

#[test]
fn long_parameter_branch_join_uses_physical_slots_and_one_frame_entry() {
    let mut sequence = CodeSequence::new();
    let otherwise = sequence.new_label().unwrap();
    let done = sequence.new_label().unwrap();
    sequence
        .instruction(Instruction::ILoad(local(0)))
        .branch(BranchInstruction::IfEq(otherwise.clone()))
        .instruction(Instruction::LLoad(local(1)))
        .branch(BranchInstruction::Goto(done.clone()))
        .bind(otherwise)
        .unwrap()
        .instruction(Instruction::LLoad(local(3)))
        .bind(done.clone())
        .unwrap()
        .instruction(Instruction::LReturn);

    let graph = ControlFlowGraph::build(&sequence).unwrap();
    let pool = ConstantPool::default();
    let flow = CfgCodeAnalyzer::analyze(&graph, "(ZJJ)J", true, &pool).unwrap();
    assert_eq!(flow.max_stack, 2);
    assert_eq!(flow.max_locals, 5);
    assert_eq!(
        flow.entry_state(graph.block_for_label(&done).unwrap())
            .unwrap()
            .operand_stack,
        [FlowValue::Long]
    );
    assert_eq!(
        flow.entry_state(graph.entry_block()).unwrap().locals,
        [
            FlowLocalState::Integer,
            FlowLocalState::Long,
            FlowLocalState::LongTail,
            FlowLocalState::Long,
            FlowLocalState::LongTail,
        ]
    );

    let assembled = assemble_labeled(&sequence, &pool).unwrap();
    let done_pc = assembled.layout.label_offset(&done).unwrap();
    assert!(done_pc > 0);
    let done_pc = u32::try_from(done_pc).unwrap();
    let frames = generate_stack_map_frames(&flow, &graph, &assembled).unwrap();
    assert!(frames.frames.iter().any(|frame| {
        frame.pc == done_pc
            && frame.locals
                == [
                    VerificationTypeInfo::Integer,
                    VerificationTypeInfo::Long,
                    VerificationTypeInfo::Long,
                ]
            && frame.stack == [VerificationTypeInfo::Long]
    }));

    let mut instance_parameter = CodeSequence::new();
    instance_parameter
        .instruction(Instruction::LLoad(local(1)))
        .instruction(Instruction::LStore(local(3)))
        .instruction(Instruction::Return);
    let graph = ControlFlowGraph::build(&instance_parameter).unwrap();
    let flow = CfgCodeAnalyzer::analyze(&graph, "(J)V", false, &ConstantPool::default()).unwrap();
    assert_eq!(flow.max_stack, 2);
    assert_eq!(flow.max_locals, 5);
    assert_eq!(
        flow.entry_state(graph.entry_block()).unwrap().locals,
        [
            FlowLocalState::Receiver,
            FlowLocalState::Long,
            FlowLocalState::LongTail,
            FlowLocalState::Top,
            FlowLocalState::Top,
        ]
    );
}

#[test]
fn long_constants_returns_and_mixed_stack_depth_use_physical_slots() {
    let mut constant = CodeSequence::new();
    constant
        .instruction(Instruction::LConst(SmallLongConstant::One))
        .instruction(Instruction::LReturn);
    let graph = ControlFlowGraph::build(&constant).unwrap();
    let flow = CfgCodeAnalyzer::analyze(&graph, "()J", true, &ConstantPool::default()).unwrap();
    assert_eq!(flow.max_stack, 2);

    let mut pool_builder = ConstantPoolBuilder::new();
    let long = pool_builder.long(9_000_000_000).unwrap();
    let pool = pool_builder.finish().unwrap();
    let mut ldc2 = CodeSequence::new();
    ldc2.instruction(Instruction::Ldc2(Ldc2Constant::Long(
        dotty_classfile::bytecode::ConstantPoolIndexOperand::try_from(long).unwrap(),
    )))
    .instruction(Instruction::LReturn);
    let graph = ControlFlowGraph::build(&ldc2).unwrap();
    let flow = CfgCodeAnalyzer::analyze(&graph, "()J", true, &pool).unwrap();
    assert_eq!(flow.max_stack, 2);

    let mut bad_pool_builder = ConstantPoolBuilder::new();
    let integer = bad_pool_builder.integer(9).unwrap();
    let bad_pool = bad_pool_builder.finish().unwrap();
    let mut bad_ldc2 = CodeSequence::new();
    bad_ldc2
        .instruction(Instruction::Ldc2(Ldc2Constant::Long(
            dotty_classfile::bytecode::ConstantPoolIndexOperand::try_from(integer).unwrap(),
        )))
        .instruction(Instruction::LReturn);
    let graph = ControlFlowGraph::build(&bad_ldc2).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze(&graph, "()J", true, &bad_pool),
        Err(AnalysisError::InvalidConstantPoolOperand {
            instruction: 0,
            expected: dotty_classfile::bytecode::ConstantPoolOperandKind::LongConstant,
            ..
        })
    ));

    let mut mixed = CodeSequence::new();
    mixed
        .instruction(Instruction::LConst(SmallLongConstant::Zero))
        .instruction(Instruction::IConst(SmallIntConstant::One))
        .instruction(Instruction::Pop);
    mixed.instruction(Instruction::LReturn);
    let graph = ControlFlowGraph::build(&mixed).unwrap();
    let flow = CfgCodeAnalyzer::analyze(&graph, "()J", true, &ConstantPool::default()).unwrap();
    assert_eq!(flow.max_stack, 3);

    let mut wrong_return = CodeSequence::new();
    wrong_return
        .instruction(Instruction::LConst(SmallLongConstant::Zero))
        .instruction(Instruction::LReturn);
    let graph = ControlFlowGraph::build(&wrong_return).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze(&graph, "()I", true, &ConstantPool::default()),
        Err(AnalysisError::WrongReturnOpcode {
            instruction: 1,
            expected: Some(ValueKind::Int),
            actual: Some(ValueKind::Long),
        })
    ));
}

#[test]
fn long_locals_are_pair_aware_at_joins_and_overlapping_stores() {
    let mut maybe_uninitialized = CodeSequence::new();
    let skip = maybe_uninitialized.new_label().unwrap();
    let join = maybe_uninitialized.new_label().unwrap();
    maybe_uninitialized
        .instruction(Instruction::ILoad(local(0)))
        .branch(BranchInstruction::IfEq(skip.clone()))
        .instruction(Instruction::LConst(SmallLongConstant::Zero))
        .instruction(Instruction::LStore(local(1)))
        .branch(BranchInstruction::Goto(join.clone()))
        .bind(skip)
        .unwrap()
        .bind(join.clone())
        .unwrap()
        .instruction(Instruction::Return);
    let graph = ControlFlowGraph::build(&maybe_uninitialized).unwrap();
    let flow = CfgCodeAnalyzer::analyze(&graph, "(I)V", true, &ConstantPool::default()).unwrap();
    let joined = flow
        .entry_state(graph.block_for_label(&join).unwrap())
        .unwrap();
    assert_eq!(
        joined.locals[1..],
        [FlowLocalState::Top, FlowLocalState::Top]
    );
    let mut read_after_join = CodeSequence::new();
    let skip = read_after_join.new_label().unwrap();
    let join = read_after_join.new_label().unwrap();
    read_after_join
        .instruction(Instruction::ILoad(local(0)))
        .branch(BranchInstruction::IfEq(skip.clone()))
        .instruction(Instruction::LConst(SmallLongConstant::Zero))
        .instruction(Instruction::LStore(local(1)))
        .branch(BranchInstruction::Goto(join.clone()))
        .bind(skip)
        .unwrap()
        .bind(join)
        .unwrap()
        .instruction(Instruction::LLoad(local(1)))
        .instruction(Instruction::LReturn);
    let graph = ControlFlowGraph::build(&read_after_join).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze(&graph, "(I)J", true, &ConstantPool::default()),
        Err(AnalysisError::UninitializedLocal {
            instruction: 5,
            index: 1,
        })
    ));

    let mut overwritten_tail = CodeSequence::new();
    overwritten_tail
        .instruction(Instruction::LConst(SmallLongConstant::Zero))
        .instruction(Instruction::LStore(local(0)))
        .instruction(Instruction::IConst(SmallIntConstant::One))
        .instruction(Instruction::IStore(local(1)))
        .instruction(Instruction::LLoad(local(0)))
        .instruction(Instruction::LReturn);
    let graph = ControlFlowGraph::build(&overwritten_tail).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze(&graph, "()J", true, &ConstantPool::default()),
        Err(AnalysisError::UninitializedLocal {
            instruction: 4,
            index: 0,
        })
    ));

    let mut tail_read = CodeSequence::new();
    tail_read
        .instruction(Instruction::LConst(SmallLongConstant::Zero))
        .instruction(Instruction::LStore(local(0)))
        .instruction(Instruction::LLoad(local(1)))
        .instruction(Instruction::LReturn);
    let graph = ControlFlowGraph::build(&tail_read).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze(&graph, "()J", true, &ConstantPool::default()),
        Err(AnalysisError::LocalSlotOverlap {
            instruction: 2,
            index: 1,
        })
    ));

    let mut shifted_pair = CodeSequence::new();
    shifted_pair
        .instruction(Instruction::LConst(SmallLongConstant::Zero))
        .instruction(Instruction::LStore(local(0)))
        .instruction(Instruction::LConst(SmallLongConstant::One))
        .instruction(Instruction::LStore(local(1)))
        .instruction(Instruction::LLoad(local(1)))
        .instruction(Instruction::LReturn);
    let graph = ControlFlowGraph::build(&shifted_pair).unwrap();
    let flow = CfgCodeAnalyzer::analyze(&graph, "()J", true, &ConstantPool::default()).unwrap();
    assert_eq!(flow.max_locals, 3);
}

#[test]
fn stack_map_long_projection_preserves_real_local_holes() {
    let mut sequence = CodeSequence::new();
    let target = sequence.new_label().unwrap();
    sequence
        .instruction(Instruction::LConst(SmallLongConstant::Zero))
        .instruction(Instruction::LStore(local(0)))
        .instruction(Instruction::IConst(SmallIntConstant::One))
        .instruction(Instruction::IStore(local(3)))
        .instruction(Instruction::IConst(SmallIntConstant::Zero))
        .branch(BranchInstruction::IfEq(target.clone()))
        .instruction(Instruction::Return)
        .bind(target.clone())
        .unwrap()
        .instruction(Instruction::Return);
    let graph = ControlFlowGraph::build(&sequence).unwrap();
    let pool = ConstantPool::default();
    let flow = CfgCodeAnalyzer::analyze(&graph, "()V", true, &pool).unwrap();
    let assembled = assemble_labeled(&sequence, &pool).unwrap();
    let frames = generate_stack_map_frames(&flow, &graph, &assembled).unwrap();
    let pc = u32::try_from(assembled.layout.label_offset(&target).unwrap()).unwrap();
    assert!(frames.frames.iter().any(|frame| {
        frame.pc == pc
            && frame.locals
                == [
                    VerificationTypeInfo::Long,
                    VerificationTypeInfo::Top,
                    VerificationTypeInfo::Integer,
                ]
    }));
}

#[test]
fn long_stack_joins_reject_integer_values_in_both_predecessor_orders() {
    for long_first in [false, true] {
        let mut sequence = CodeSequence::new();
        let alternate = sequence.new_label().unwrap();
        let join = sequence.new_label().unwrap();
        sequence
            .instruction(Instruction::ILoad(local(0)))
            .branch(BranchInstruction::IfEq(alternate.clone()));
        let push_long = |sequence: &mut CodeSequence| {
            sequence.instruction(Instruction::LConst(SmallLongConstant::Zero));
        };
        let push_int = |sequence: &mut CodeSequence| {
            sequence.instruction(Instruction::IConst(SmallIntConstant::Zero));
        };
        if long_first {
            push_long(&mut sequence);
        } else {
            push_int(&mut sequence);
        }
        sequence.branch(BranchInstruction::Goto(join.clone()));
        sequence.bind(alternate).unwrap();
        if long_first {
            push_int(&mut sequence);
        } else {
            push_long(&mut sequence);
        }
        sequence.bind(join).unwrap();
        sequence.instruction(Instruction::Pop);
        sequence.instruction(Instruction::Return);

        let graph = ControlFlowGraph::build(&sequence).unwrap();
        assert!(matches!(
            CfgCodeAnalyzer::analyze(&graph, "(I)V", true, &ConstantPool::default()),
            Err(AnalysisError::IncompatibleStackValue { .. })
        ));
    }
}

#[test]
fn stable_long_local_converges_across_a_counted_loop_backedge() {
    let mut sequence = CodeSequence::new();
    let head = sequence.new_label().unwrap();
    let done = sequence.new_label().unwrap();
    sequence
        .instruction(Instruction::LConst(SmallLongConstant::Zero))
        .instruction(Instruction::LStore(local(1)))
        .bind(head.clone())
        .unwrap()
        .instruction(Instruction::ILoad(local(0)))
        .branch(BranchInstruction::IfEq(done.clone()))
        .instruction(Instruction::ILoad(local(0)))
        .instruction(Instruction::IConst(SmallIntConstant::One))
        .instruction(Instruction::ISub)
        .instruction(Instruction::IStore(local(0)))
        .branch(BranchInstruction::Goto(head.clone()))
        .bind(done)
        .unwrap()
        .instruction(Instruction::LLoad(local(1)))
        .instruction(Instruction::LReturn);

    let graph = ControlFlowGraph::build(&sequence).unwrap();
    let flow = CfgCodeAnalyzer::analyze(&graph, "(I)J", true, &ConstantPool::default()).unwrap();
    let loop_state = flow
        .entry_state(graph.block_for_label(&head).unwrap())
        .unwrap();
    assert_eq!(
        loop_state.locals,
        [
            FlowLocalState::Integer,
            FlowLocalState::Long,
            FlowLocalState::LongTail,
        ]
    );
    assert_eq!(flow.max_stack, 2);

    let mut invalidated = CodeSequence::new();
    let head = invalidated.new_label().unwrap();
    let done = invalidated.new_label().unwrap();
    invalidated
        .instruction(Instruction::LConst(SmallLongConstant::Zero))
        .instruction(Instruction::LStore(local(1)))
        .bind(head.clone())
        .unwrap()
        .instruction(Instruction::ILoad(local(0)))
        .branch(BranchInstruction::IfEq(done.clone()))
        .instruction(Instruction::IConst(SmallIntConstant::One))
        .instruction(Instruction::IStore(local(2)))
        .instruction(Instruction::IConst(SmallIntConstant::Zero))
        .instruction(Instruction::IStore(local(0)))
        .branch(BranchInstruction::Goto(head))
        .bind(done)
        .unwrap()
        .instruction(Instruction::LLoad(local(1)))
        .instruction(Instruction::LReturn);
    let graph = ControlFlowGraph::build(&invalidated).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze(&graph, "(I)J", true, &ConstantPool::default()),
        Err(AnalysisError::UninitializedLocal {
            instruction: 9,
            index: 1,
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

    let mut long_sequence = CodeSequence::new();
    long_sequence
        .instruction(Instruction::LConst(SmallLongConstant::Zero))
        .instruction(Instruction::LStore(local(u16::MAX - 1)))
        .instruction(Instruction::LReturn);
    let graph = ControlFlowGraph::build(&long_sequence).unwrap();
    assert!(matches!(
        CfgCodeAnalyzer::analyze(&graph, "()J", true, &ConstantPool::default()),
        Err(AnalysisError::LocalLimitExceeded {
            instruction: 1,
            index: 65534,
            width: 2,
        })
    ));
}

#[test]
fn identical_object_stack_values_join_without_a_hierarchy_proof() {
    let mut pool_builder = ConstantPoolBuilder::new();
    let owner = pool_builder.class("example/Client").unwrap();
    let pool = pool_builder.finish().unwrap();
    let mut sequence = CodeSequence::new();
    let otherwise = sequence.new_label().unwrap();
    let join = sequence.new_label().unwrap();
    sequence
        .instruction(Instruction::ILoad(local(1)))
        .branch(BranchInstruction::IfEq(otherwise.clone()))
        .instruction(Instruction::ALoad(local(2)))
        .branch(BranchInstruction::Goto(join.clone()))
        .bind(otherwise)
        .unwrap()
        .instruction(Instruction::ALoad(local(2)))
        .bind(join)
        .unwrap()
        .instruction(Instruction::AReturn);
    let graph = ControlFlowGraph::build(&sequence).unwrap();
    let flow = CfgCodeAnalyzer::analyze_with_current_class(
        &graph,
        "chooseObject",
        "(ZLjava/lang/Object;)Ljava/lang/Object;",
        false,
        &pool,
        owner,
    )
    .unwrap();
    assert!(matches!(
        flow.entry_state(graph.blocks().last().unwrap().id)
            .unwrap()
            .operand_stack
            .as_slice(),
        [FlowValue::Object(class)] if class.internal_name() == "java/lang/Object"
    ));
}

#[test]
fn explicit_base_object_ancestor_joins_work_in_both_operand_orders() {
    let mut pool_builder = ConstantPoolBuilder::new();
    let owner = pool_builder.class("example/Client").unwrap();
    let pool = pool_builder.finish().unwrap();
    let hierarchy =
        ClassHierarchy::from_edges([("example/Base".to_owned(), "java/lang/Object".to_owned())])
            .unwrap();

    for base_on_fallthrough in [true, false] {
        let mut sequence = CodeSequence::new();
        let otherwise = sequence.new_label().unwrap();
        let join = sequence.new_label().unwrap();
        sequence
            .instruction(Instruction::ILoad(local(1)))
            .branch(BranchInstruction::IfEq(otherwise.clone()))
            .instruction(Instruction::ALoad(local(if base_on_fallthrough {
                2
            } else {
                3
            })))
            .branch(BranchInstruction::Goto(join.clone()))
            .bind(otherwise)
            .unwrap()
            .instruction(Instruction::ALoad(local(if base_on_fallthrough {
                3
            } else {
                2
            })))
            .bind(join)
            .unwrap()
            .instruction(Instruction::AReturn);
        let graph = ControlFlowGraph::build(&sequence).unwrap();
        let flow = CfgCodeAnalyzer::analyze_with_hierarchy(
            &graph,
            "chooseBaseOrObject",
            "(ZLexample/Base;Ljava/lang/Object;)Ljava/lang/Object;",
            false,
            &pool,
            owner,
            &hierarchy,
        )
        .unwrap();
        assert!(matches!(
            flow.entry_state(graph.blocks().last().unwrap().id)
                .unwrap()
                .operand_stack
                .as_slice(),
            [FlowValue::Object(class)] if class.internal_name() == "java/lang/Object"
        ));
    }
}

#[test]
fn actual_object_multiway_joins_are_independent_of_predecessor_order() {
    let mut pool_builder = ConstantPoolBuilder::new();
    let owner = pool_builder.class("example/Client").unwrap();
    let pool = pool_builder.finish().unwrap();
    let hierarchy = ClassHierarchy::from_edges([
        ("example/Left".to_owned(), "java/lang/Object".to_owned()),
        ("example/Right".to_owned(), "java/lang/Object".to_owned()),
    ])
    .unwrap();

    for order in [
        [3, 4, 5],
        [3, 5, 4],
        [4, 3, 5],
        [4, 5, 3],
        [5, 3, 4],
        [5, 4, 3],
    ] {
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
        let flow = CfgCodeAnalyzer::analyze_with_hierarchy(
            &graph,
            "chooseObjectOrSibling",
            "(ZZLjava/lang/Object;Lexample/Left;Lexample/Right;)Ljava/lang/Object;",
            false,
            &pool,
            owner,
            &hierarchy,
        )
        .unwrap();
        let join_state = flow.entry_state(graph.blocks().last().unwrap().id).unwrap();
        assert!(matches!(
            join_state.operand_stack.as_slice(),
            [FlowValue::Object(class)] if class.internal_name() == "java/lang/Object"
        ));
    }
}

#[test]
fn provisional_object_join_evidence_cannot_become_an_object_input_through_locals() {
    let mut pool_builder = ConstantPoolBuilder::new();
    let owner = pool_builder.class("example/Client").unwrap();
    let pool = pool_builder.finish().unwrap();
    let hierarchy = ClassHierarchy::from_edges([
        ("example/Left".to_owned(), "java/lang/Object".to_owned()),
        ("example/Right".to_owned(), "java/lang/Object".to_owned()),
    ])
    .unwrap();

    for (left_slot, right_slot) in [(2, 3), (3, 2)] {
        for add_backedge in [false, true] {
            let mut sequence = CodeSequence::new();
            let right = sequence.new_label().unwrap();
            let join = sequence.new_label().unwrap();
            sequence
                .instruction(Instruction::ILoad(local(1)))
                .branch(BranchInstruction::IfEq(right.clone()))
                .instruction(Instruction::ALoad(local(left_slot)))
                .branch(BranchInstruction::Goto(join.clone()))
                .bind(right)
                .unwrap()
                .instruction(Instruction::ALoad(local(right_slot)))
                .bind(join.clone())
                .unwrap()
                .instruction(Instruction::AStore(local(4)))
                .instruction(Instruction::ALoad(local(4)));
            if add_backedge {
                sequence
                    .instruction(Instruction::ILoad(local(1)))
                    .branch(BranchInstruction::IfEq(join));
            }
            sequence.instruction(Instruction::AReturn);

            let graph = ControlFlowGraph::build(&sequence).unwrap();
            assert!(matches!(
                CfgCodeAnalyzer::analyze_with_hierarchy(
                    &graph,
                    "rejectProvisionalObjectLoop",
                    "(ZLexample/Left;Lexample/Right;)Ljava/lang/Object;",
                    false,
                    &pool,
                    owner,
                    &hierarchy,
                ),
                Err(AnalysisError::IncompatibleStackValue { .. })
            ));
        }
    }
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
