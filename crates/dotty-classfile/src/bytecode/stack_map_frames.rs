//! Derive typed `full_frame` records from bounded CFG states and final
//! assembled instruction offsets.
//!
//! [`generate_stack_map_frames`] only emits explicit frames for reachable
//! branch targets. It keeps the initial frame at PC 0 implicit, deduplicates
//! aliases to the same target, trims only trailing `Top` locals, and preserves
//! the `CodeAnalysis` maxima for later Code-attribute integration. It does not
//! infer object or category-two states, create a class-file attribute, or
//! change the linear analyzer.

use super::assembler::AssembledCode;
use super::stack_map::{FullFrame, StackMapTable, VerificationTypeInfo};
use super::{
    BasicBlockId, BlockEntryState, CodeAnalysis, CodeElement, ControlFlowGraph, FlowAnalysis,
    FlowLocalState, ValueKind,
};
use crate::constant_pool::{ConstantPool, ConstantPoolIndex};
use crate::encode_error::EncodeError;
use std::collections::BTreeMap;
use std::fmt;

/// A verifier value that the integer-only flow model cannot safely encode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedStackMapState {
    /// The flow analyzer uses `Top` as a placeholder for an instance receiver.
    InstanceReceiver,
    /// The operand stack contains a value other than an integer.
    OperandStackValue(ValueKind),
}

/// A typed inconsistency while combining CFG states with final code offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StackMapFrameGenerationError {
    /// CFG, analysis, and assembled layout were not derived from one sequence revision.
    DifferentCodeSequence,
    /// The analyzed state vector does not correspond to the CFG's blocks.
    FlowBlockCountMismatch {
        /// Number of CFG blocks.
        cfg_blocks: usize,
        /// Number of block-state slots in the analysis result.
        flow_blocks: usize,
    },
    /// A block's ID, range, or executable-element count is inconsistent.
    InvalidControlFlowBlock {
        /// The invalid block.
        block: BasicBlockId,
    },
    /// CFG instruction positions do not match the assembled instruction list.
    InstructionCountMismatch {
        /// Number of executable elements represented by the CFG.
        cfg_instructions: usize,
        /// Number of offsets in the assembled layout.
        layout_instructions: usize,
    },
    /// Assembled bytes do not match the code length recorded by their layout.
    CodeLengthMismatch {
        /// Actual number of bytes.
        bytes: usize,
        /// Length recorded in the layout.
        layout: usize,
    },
    /// The graph entry does not map to bytecode PC zero.
    InvalidEntryOffset,
    /// Reachability and the existence of an analyzed entry state disagree.
    ReachabilityMismatch {
        /// The affected block.
        block: BasicBlockId,
        /// Whether the CFG marks it reachable.
        reachable: bool,
        /// Whether the flow analysis has a state for it.
        has_state: bool,
    },
    /// A block state has a different local-slot width from the analysis result.
    LocalCountMismatch {
        /// The affected block.
        block: BasicBlockId,
        /// Expected local-slot count.
        expected: usize,
        /// Actual local-slot count.
        actual: usize,
    },
    /// A CFG branch target does not resolve to a block.
    MissingTargetBlock {
        /// The unresolved symbolic target.
        label: super::LabelId,
    },
    /// A branch target has no final offset in the assembled layout.
    MissingTargetOffset {
        /// The unresolved symbolic target.
        label: super::LabelId,
    },
    /// A final target offset cannot be represented by the frame's PC field.
    FrameOffsetOutOfRange {
        /// Final bytecode offset.
        offset: usize,
    },
    /// A label offset does not point to its CFG target block's first instruction.
    TargetOffsetMismatch {
        /// The target label.
        label: super::LabelId,
        /// The CFG block identified by the target.
        block: BasicBlockId,
        /// Final offset for the label.
        label_offset: usize,
        /// Final offset for the target block's first instruction.
        instruction_offset: usize,
    },
    /// Distinct CFG blocks unexpectedly map to the same frame PC.
    DuplicateFrameOffset {
        /// Shared bytecode PC.
        pc: usize,
        /// First target block at that offset.
        first: BasicBlockId,
        /// Second target block at that offset.
        second: BasicBlockId,
    },
    /// A loop edge to the implicit entry frame changes its analyzed state.
    ImplicitEntryStateMismatch,
    /// A reachable target requires verifier information outside the supported subset.
    UnsupportedFlow {
        /// The target block requiring the unsupported state.
        block: BasicBlockId,
        /// The unavailable or unsupported verifier value.
        state: UnsupportedStackMapState,
    },
    /// The generated frames failed the typed StackMapTable structural checks.
    InvalidStackMap(EncodeError),
}

impl fmt::Display for StackMapFrameGenerationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DifferentCodeSequence => formatter
                .write_str("CFG, flow analysis, and layout use different code sequence snapshots"),
            Self::FlowBlockCountMismatch {
                cfg_blocks,
                flow_blocks,
            } => write!(
                formatter,
                "CFG has {cfg_blocks} blocks but flow analysis has {flow_blocks} state slots"
            ),
            Self::InvalidControlFlowBlock { block } => {
                write!(
                    formatter,
                    "CFG block {} has an inconsistent instruction range",
                    block.index()
                )
            }
            Self::InstructionCountMismatch {
                cfg_instructions,
                layout_instructions,
            } => write!(
                formatter,
                "CFG has {cfg_instructions} executable elements but layout has {layout_instructions} instruction offsets"
            ),
            Self::CodeLengthMismatch { bytes, layout } => write!(
                formatter,
                "assembled code has {bytes} bytes but layout records length {layout}"
            ),
            Self::InvalidEntryOffset => {
                formatter.write_str("CFG entry does not start at bytecode PC 0")
            }
            Self::ReachabilityMismatch {
                block,
                reachable,
                has_state,
            } => write!(
                formatter,
                "CFG block {} reachability is {reachable}, but flow-state presence is {has_state}",
                block.index()
            ),
            Self::LocalCountMismatch {
                block,
                expected,
                actual,
            } => write!(
                formatter,
                "CFG block {} has {actual} local slots; expected {expected}",
                block.index()
            ),
            Self::MissingTargetBlock { label } => {
                write!(formatter, "branch target {label:?} has no CFG block")
            }
            Self::MissingTargetOffset { label } => {
                write!(
                    formatter,
                    "branch target {label:?} has no assembled byte offset"
                )
            }
            Self::FrameOffsetOutOfRange { offset } => write!(
                formatter,
                "branch target offset {offset} cannot be represented by a StackMapTable frame"
            ),
            Self::TargetOffsetMismatch {
                label,
                block,
                label_offset,
                instruction_offset,
            } => write!(
                formatter,
                "branch target {label:?} maps to PC {label_offset}, but CFG block {} starts at PC {instruction_offset}",
                block.index()
            ),
            Self::DuplicateFrameOffset { pc, first, second } => write!(
                formatter,
                "CFG blocks {} and {} both require a frame at PC {pc}",
                first.index(),
                second.index()
            ),
            Self::ImplicitEntryStateMismatch => {
                formatter.write_str("loop edge changes the method's implicit initial frame at PC 0")
            }
            Self::UnsupportedFlow { block, state } => write!(
                formatter,
                "CFG block {} requires unsupported StackMapTable state {state:?}",
                block.index()
            ),
            Self::InvalidStackMap(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for StackMapFrameGenerationError {}

/// Generated branch-target frames together with the analysis maxima needed by
/// a later `Code`-attribute integration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedStackMapFrames {
    /// `max_stack` and `max_locals` from the CFG analyzer.
    pub code_analysis: CodeAnalysis,
    /// Unique `full_frame` records sorted by their final absolute bytecode PC.
    pub frames: Vec<FullFrame>,
}

/// Generates verifier frames for the reachable branch targets in one symbolic
/// code sequence.
///
/// The CFG and flow result must come from `CfgCodeAnalyzer`, and the layout
/// must come from `assemble_labeled` on the same immutable sequence revision.
/// A private snapshot identity enforces this relationship. The method emits no frame at PC 0;
/// loops back to PC 0 must preserve the analyzer's implicit initial state.
/// Only integer stack values and integer/Top locals are representable through
/// the public API. Class-file construction can additionally resolve a
/// preserved instance receiver to its current-class type.
/// No frame is synthesized for an unreachable target or for ordinary
/// fallthrough blocks. The returned maxima are copied unchanged for a later
/// explicit class-file integration.
pub fn generate_stack_map_frames(
    flow: &FlowAnalysis,
    graph: &ControlFlowGraph,
    assembled: &AssembledCode,
) -> Result<GeneratedStackMapFrames, StackMapFrameGenerationError> {
    generate_stack_map_frames_inner(flow, graph, assembled, &ConstantPool::default(), None)
}

/// Generates stack-map frames while resolving an initialized instance receiver
/// to the current class. `receiver_class` must identify the class represented
/// by local slot zero in the constant pool. Constructor flow is not supported
/// because this analyzer does not track `uninitializedThis` initialization.
pub(crate) fn generate_stack_map_frames_with_receiver(
    flow: &FlowAnalysis,
    graph: &ControlFlowGraph,
    assembled: &AssembledCode,
    constant_pool: &ConstantPool,
    receiver_class: ConstantPoolIndex,
) -> Result<GeneratedStackMapFrames, StackMapFrameGenerationError> {
    generate_stack_map_frames_inner(flow, graph, assembled, constant_pool, Some(receiver_class))
}

fn generate_stack_map_frames_inner(
    flow: &FlowAnalysis,
    graph: &ControlFlowGraph,
    assembled: &AssembledCode,
    constant_pool: &ConstantPool,
    receiver_class: Option<ConstantPoolIndex>,
) -> Result<GeneratedStackMapFrames, StackMapFrameGenerationError> {
    if !flow.shares_sequence_scope(graph.sequence_scope())
        || !assembled
            .layout
            .shares_sequence_scope(graph.sequence_scope())
        || !flow.shares_sequence_revision(graph.sequence_revision())
        || !assembled
            .layout
            .shares_sequence_revision(graph.sequence_revision())
    {
        return Err(StackMapFrameGenerationError::DifferentCodeSequence);
    }
    if assembled.bytes.len() != assembled.layout.code_length() {
        return Err(StackMapFrameGenerationError::CodeLengthMismatch {
            bytes: assembled.bytes.len(),
            layout: assembled.layout.code_length(),
        });
    }
    if flow.block_entries().len() != graph.blocks().len() {
        return Err(StackMapFrameGenerationError::FlowBlockCountMismatch {
            cfg_blocks: graph.blocks().len(),
            flow_blocks: flow.block_entries().len(),
        });
    }

    let mut instruction_count = 0usize;
    for (block_index, block) in graph.blocks().iter().enumerate() {
        if block.id.index() != block_index
            || block.instruction_range.start != instruction_count
            || block.instruction_range.end < block.instruction_range.start
            || block.instructions.len()
                != block.instruction_range.end - block.instruction_range.start
        {
            return Err(StackMapFrameGenerationError::InvalidControlFlowBlock { block: block.id });
        }
        instruction_count = block.instruction_range.end;
        let has_state = flow.entry_state(block.id).is_some();
        if block.reachable != has_state {
            return Err(StackMapFrameGenerationError::ReachabilityMismatch {
                block: block.id,
                reachable: block.reachable,
                has_state,
            });
        }
        if let Some(state) = flow.entry_state(block.id)
            && state.locals.len() != usize::from(flow.max_locals)
        {
            return Err(StackMapFrameGenerationError::LocalCountMismatch {
                block: block.id,
                expected: usize::from(flow.max_locals),
                actual: state.locals.len(),
            });
        }
    }
    if instruction_count != assembled.layout.instruction_count() {
        return Err(StackMapFrameGenerationError::InstructionCountMismatch {
            cfg_instructions: instruction_count,
            layout_instructions: assembled.layout.instruction_count(),
        });
    }
    let entry = graph
        .block(graph.entry_block())
        .ok_or(StackMapFrameGenerationError::InvalidEntryOffset)?;
    if assembled
        .layout
        .instruction_offset(entry.instruction_range.start)
        != Some(0)
    {
        return Err(StackMapFrameGenerationError::InvalidEntryOffset);
    }

    let mut targets = BTreeMap::<usize, (BasicBlockId, FullFrame)>::new();
    for source in graph.blocks().iter().filter(|block| block.reachable) {
        for element in &source.instructions {
            let CodeElement::Branch(branch) = element else {
                continue;
            };
            let target = branch.target();
            let target_block_id = graph.block_for_label(target).ok_or_else(|| {
                StackMapFrameGenerationError::MissingTargetBlock {
                    label: target.clone(),
                }
            })?;
            let target_block = graph.block(target_block_id).ok_or_else(|| {
                StackMapFrameGenerationError::MissingTargetBlock {
                    label: target.clone(),
                }
            })?;
            let label_offset = assembled.layout.label_offset(target).ok_or_else(|| {
                StackMapFrameGenerationError::MissingTargetOffset {
                    label: target.clone(),
                }
            })?;
            let instruction_offset = assembled
                .layout
                .instruction_offset(target_block.instruction_range.start)
                .ok_or(StackMapFrameGenerationError::InvalidControlFlowBlock {
                    block: target_block_id,
                })?;
            if label_offset != instruction_offset {
                return Err(StackMapFrameGenerationError::TargetOffsetMismatch {
                    label: target.clone(),
                    block: target_block_id,
                    label_offset,
                    instruction_offset,
                });
            }
            let state = flow.entry_state(target_block_id).ok_or(
                StackMapFrameGenerationError::ReachabilityMismatch {
                    block: target_block_id,
                    reachable: target_block.reachable,
                    has_state: false,
                },
            )?;
            let mut frame =
                frame_from_state(target_block_id, state, flow.is_static(), receiver_class)?;
            frame.pc = u32::try_from(label_offset).map_err(|_| {
                StackMapFrameGenerationError::FrameOffsetOutOfRange {
                    offset: label_offset,
                }
            })?;

            if label_offset == 0 {
                if state != flow.initial_state() {
                    return Err(StackMapFrameGenerationError::ImplicitEntryStateMismatch);
                }
                continue;
            }

            match targets.entry(label_offset) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert((target_block_id, frame));
                }
                std::collections::btree_map::Entry::Occupied(entry)
                    if entry.get().0 == target_block_id => {}
                std::collections::btree_map::Entry::Occupied(entry) => {
                    return Err(StackMapFrameGenerationError::DuplicateFrameOffset {
                        pc: label_offset,
                        first: entry.get().0,
                        second: target_block_id,
                    });
                }
            }
        }
    }

    let frames: Vec<_> = targets.into_values().map(|(_, frame)| frame).collect();
    let table = StackMapTable {
        frames: frames.clone(),
    };
    table
        .encode_payload_with_layout(constant_pool, &assembled.bytes, &assembled.layout)
        .map_err(StackMapFrameGenerationError::InvalidStackMap)?;

    Ok(GeneratedStackMapFrames {
        code_analysis: CodeAnalysis {
            max_stack: flow.max_stack,
            max_locals: flow.max_locals,
        },
        frames,
    })
}

fn frame_from_state(
    block: BasicBlockId,
    state: &BlockEntryState,
    is_static: bool,
    receiver_class: Option<ConstantPoolIndex>,
) -> Result<FullFrame, StackMapFrameGenerationError> {
    if !is_static && state.locals.first() == Some(&FlowLocalState::Top) {
        return Err(StackMapFrameGenerationError::UnsupportedFlow {
            block,
            state: UnsupportedStackMapState::InstanceReceiver,
        });
    }
    let mut locals: Vec<_> = state
        .locals
        .iter()
        .map(|local| match local {
            FlowLocalState::Top => Ok(VerificationTypeInfo::Top),
            FlowLocalState::Integer => Ok(VerificationTypeInfo::Integer),
            FlowLocalState::Receiver => match receiver_class {
                Some(class) => Ok(VerificationTypeInfo::Object(class)),
                None => {
                    return Err(StackMapFrameGenerationError::UnsupportedFlow {
                        block,
                        state: UnsupportedStackMapState::InstanceReceiver,
                    });
                }
            },
        })
        .collect::<Result<_, _>>()?;
    while locals.last() == Some(&VerificationTypeInfo::Top) {
        locals.pop();
    }

    let stack = state
        .operand_stack
        .iter()
        .map(|value| match value {
            ValueKind::Int => Ok(VerificationTypeInfo::Integer),
            unsupported => Err(StackMapFrameGenerationError::UnsupportedFlow {
                block,
                state: UnsupportedStackMapState::OperandStackValue(*unsupported),
            }),
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(FullFrame {
        pc: 0,
        locals,
        stack,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::{
        BranchInstruction, CfgCodeAnalyzer, CodeSequence, ControlFlowGraph, Instruction,
        SmallIntConstant, assemble_labeled,
    };
    use crate::constant_pool::ConstantPool;
    use crate::constant_pool_builder::ConstantPoolBuilder;

    fn analyze_sequence(
        sequence: &CodeSequence,
        descriptor: &str,
    ) -> (ControlFlowGraph, FlowAnalysis, AssembledCode) {
        let pool = ConstantPool::default();
        let graph = ControlFlowGraph::build(sequence).unwrap();
        let flow = CfgCodeAnalyzer::analyze(&graph, descriptor, true, &pool).unwrap();
        let assembled = assemble_labeled(sequence, &pool).unwrap();
        (graph, flow, assembled)
    }

    fn table(result: GeneratedStackMapFrames) -> StackMapTable {
        StackMapTable {
            frames: result.frames,
        }
    }

    #[test]
    fn generates_diamond_branch_targets_and_join_frames_from_final_offsets() {
        let mut sequence = CodeSequence::new();
        let else_label = sequence.new_label().unwrap();
        let join = sequence.new_label().unwrap();
        sequence
            .instruction(Instruction::IConst(SmallIntConstant::Zero))
            .branch(BranchInstruction::IfEq(else_label.clone()))
            .instruction(Instruction::IConst(SmallIntConstant::One))
            .branch(BranchInstruction::Goto(join.clone()))
            .bind(else_label.clone())
            .unwrap()
            .instruction(Instruction::IConst(SmallIntConstant::Zero))
            .bind(join.clone())
            .unwrap()
            .instruction(Instruction::IReturn);

        let (graph, flow, assembled) = analyze_sequence(&sequence, "()I");
        let generated = generate_stack_map_frames(&flow, &graph, &assembled).unwrap();
        assert_eq!(generated.code_analysis.max_stack, flow.max_stack);
        assert_eq!(generated.code_analysis.max_locals, flow.max_locals);
        assert_eq!(
            generated
                .frames
                .iter()
                .map(|frame| frame.pc)
                .collect::<Vec<_>>(),
            [8, 9]
        );
        assert_eq!(
            table(generated)
                .encode_payload_with_layout(
                    &ConstantPool::default(),
                    &assembled.bytes,
                    &assembled.layout,
                )
                .unwrap(),
            [
                0x00, 0x02, 0xff, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0xff, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x01, 0x01,
            ]
        );
    }

    #[test]
    fn loop_back_edge_deduplicates_target_and_keeps_initial_frame_implicit() {
        let mut sequence = CodeSequence::new();
        let loop_head = sequence.new_label().unwrap();
        let exit = sequence.new_label().unwrap();
        sequence
            .instruction(Instruction::IConst(SmallIntConstant::Zero))
            .instruction(Instruction::IStore(crate::bytecode::LocalIndex::new(0)))
            .bind(loop_head.clone())
            .unwrap()
            .instruction(Instruction::ILoad(crate::bytecode::LocalIndex::new(0)))
            .branch(BranchInstruction::IfEq(exit.clone()))
            .branch(BranchInstruction::Goto(loop_head.clone()))
            .bind(exit.clone())
            .unwrap()
            .instruction(Instruction::Return);
        let (graph, flow, assembled) = analyze_sequence(&sequence, "()V");
        let generated = generate_stack_map_frames(&flow, &graph, &assembled).unwrap();
        assert_eq!(
            generated
                .frames
                .iter()
                .map(|frame| frame.pc)
                .collect::<Vec<_>>(),
            [2, 9]
        );
        assert_eq!(
            generated
                .frames
                .iter()
                .map(|frame| frame.locals.clone())
                .collect::<Vec<_>>(),
            [
                vec![VerificationTypeInfo::Integer],
                vec![VerificationTypeInfo::Integer]
            ]
        );
        assert!(generated.frames.iter().all(|frame| frame.pc != 0));
    }

    #[test]
    fn branch_back_to_pc_zero_keeps_matching_initial_frame_implicit() {
        let mut sequence = CodeSequence::new();
        let loop_head = sequence.new_label().unwrap();
        let exit = sequence.new_label().unwrap();
        sequence
            .bind(loop_head.clone())
            .unwrap()
            .instruction(Instruction::IConst(SmallIntConstant::Zero))
            .branch(BranchInstruction::IfEq(exit.clone()))
            .branch(BranchInstruction::Goto(loop_head))
            .bind(exit.clone())
            .unwrap()
            .instruction(Instruction::Return);

        let (graph, flow, assembled) = analyze_sequence(&sequence, "()V");
        let generated = generate_stack_map_frames(&flow, &graph, &assembled).unwrap();
        assert_eq!(
            generated
                .frames
                .iter()
                .map(|frame| frame.pc)
                .collect::<Vec<_>>(),
            [assembled.layout.label_offset(&exit).unwrap() as u32]
        );
    }

    #[test]
    fn emits_one_frame_for_duplicate_targets_and_none_for_unreachable_branches() {
        let mut sequence = CodeSequence::new();
        let shared = sequence.new_label().unwrap();
        let unreachable = sequence.new_label().unwrap();
        sequence
            .instruction(Instruction::IConst(SmallIntConstant::Zero))
            .branch(BranchInstruction::IfEq(shared.clone()))
            .branch(BranchInstruction::Goto(shared.clone()))
            .instruction(Instruction::Return)
            .bind(shared.clone())
            .unwrap()
            .instruction(Instruction::Return)
            .bind(unreachable.clone())
            .unwrap()
            .branch(BranchInstruction::Goto(shared.clone()));
        // The trailing block is unreachable and terminal because its only
        // instruction is an unconditional branch back to an existing target.
        let pool = ConstantPool::default();
        let graph = ControlFlowGraph::build(&sequence).unwrap();
        let flow = CfgCodeAnalyzer::analyze(&graph, "()V", true, &pool).unwrap();
        let assembled = assemble_labeled(&sequence, &pool).unwrap();
        let expected_pc = assembled.layout.label_offset(&shared).unwrap() as u32;
        let generated = generate_stack_map_frames(&flow, &graph, &assembled).unwrap();
        assert_eq!(generated.frames.len(), 1);
        assert_eq!(generated.frames[0].pc, expected_pc);
    }

    #[test]
    fn straight_line_code_produces_no_explicit_frames() {
        let mut sequence = CodeSequence::new();
        sequence
            .instruction(Instruction::IConst(SmallIntConstant::Zero))
            .instruction(Instruction::IReturn);
        let (graph, flow, assembled) = analyze_sequence(&sequence, "()I");
        let generated = generate_stack_map_frames(&flow, &graph, &assembled).unwrap();
        assert!(generated.frames.is_empty());
    }

    #[test]
    fn preserves_interior_top_holes_and_trims_only_trailing_top_locals() {
        let mut sequence = CodeSequence::new();
        let alternate = sequence.new_label().unwrap();
        let join = sequence.new_label().unwrap();
        sequence
            .instruction(Instruction::IConst(SmallIntConstant::One))
            .instruction(Instruction::IStore(crate::bytecode::LocalIndex::new(2)))
            .instruction(Instruction::ILoad(crate::bytecode::LocalIndex::new(0)))
            .branch(BranchInstruction::IfEq(alternate.clone()))
            .instruction(Instruction::IConst(SmallIntConstant::One))
            .instruction(Instruction::IStore(crate::bytecode::LocalIndex::new(1)))
            .branch(BranchInstruction::Goto(join.clone()))
            .bind(alternate)
            .unwrap()
            .bind(join)
            .unwrap()
            .instruction(Instruction::Return);
        let (graph, flow, assembled) = analyze_sequence(&sequence, "(I)V");
        let generated = generate_stack_map_frames(&flow, &graph, &assembled).unwrap();
        assert_eq!(generated.frames.len(), 1);
        assert_eq!(
            generated.frames[0].locals,
            [
                VerificationTypeInfo::Integer,
                VerificationTypeInfo::Top,
                VerificationTypeInfo::Integer,
            ]
        );
    }

    #[test]
    fn multiple_conditionals_and_early_returns_keep_target_frames_sorted() {
        let mut sequence = CodeSequence::new();
        let otherwise = sequence.new_label().unwrap();
        let done = sequence.new_label().unwrap();
        sequence
            .instruction(Instruction::ILoad(crate::bytecode::LocalIndex::new(0)))
            .branch(BranchInstruction::IfEq(otherwise.clone()))
            .instruction(Instruction::IConst(SmallIntConstant::Zero))
            .instruction(Instruction::IReturn)
            .bind(otherwise.clone())
            .unwrap()
            .instruction(Instruction::ILoad(crate::bytecode::LocalIndex::new(0)))
            .branch(BranchInstruction::IfNe(done.clone()))
            .instruction(Instruction::IConst(SmallIntConstant::One))
            .instruction(Instruction::IReturn)
            .bind(done.clone())
            .unwrap()
            .instruction(Instruction::IConst(SmallIntConstant::Zero))
            .instruction(Instruction::IReturn);
        let (graph, flow, assembled) = analyze_sequence(&sequence, "(I)I");
        let generated = generate_stack_map_frames(&flow, &graph, &assembled).unwrap();
        let expected = [
            assembled.layout.label_offset(&otherwise).unwrap() as u32,
            assembled.layout.label_offset(&done).unwrap() as u32,
        ];
        assert_eq!(
            generated
                .frames
                .iter()
                .map(|frame| frame.pc)
                .collect::<Vec<_>>(),
            expected
        );
    }

    #[test]
    fn uses_actual_wide_instruction_width_before_target_frames() {
        let mut sequence = CodeSequence::new();
        let target = sequence.new_label().unwrap();
        sequence
            .instruction(Instruction::IConst(SmallIntConstant::Zero))
            .instruction(Instruction::IStore(crate::bytecode::LocalIndex::new(256)))
            .instruction(Instruction::ILoad(crate::bytecode::LocalIndex::new(256)))
            .branch(BranchInstruction::IfEq(target.clone()))
            .instruction(Instruction::Return)
            .bind(target.clone())
            .unwrap()
            .instruction(Instruction::Return);
        let (graph, flow, assembled) = analyze_sequence(&sequence, "()V");
        let generated = generate_stack_map_frames(&flow, &graph, &assembled).unwrap();
        assert_eq!(assembled.layout.label_offset(&target), Some(13));
        assert_eq!(generated.frames[0].pc, 13);
    }

    #[test]
    fn rejects_results_from_a_different_sequence() {
        let mut first = CodeSequence::new();
        let first_target = first.new_label().unwrap();
        first
            .instruction(Instruction::IConst(SmallIntConstant::Zero))
            .branch(BranchInstruction::IfEq(first_target.clone()))
            .instruction(Instruction::Return)
            .bind(first_target)
            .unwrap()
            .instruction(Instruction::Return);
        let (first_graph, first_flow, _) = analyze_sequence(&first, "()V");

        let mut second = CodeSequence::new();
        let second_target = second.new_label().unwrap();
        second
            .instruction(Instruction::IConst(SmallIntConstant::Zero))
            .branch(BranchInstruction::IfEq(second_target.clone()))
            .instruction(Instruction::Return)
            .bind(second_target)
            .unwrap()
            .instruction(Instruction::Return);
        let second_assembled = assemble_labeled(&second, &ConstantPool::default()).unwrap();
        assert_eq!(
            generate_stack_map_frames(&first_flow, &first_graph, &second_assembled),
            Err(StackMapFrameGenerationError::DifferentCodeSequence)
        );
    }

    #[test]
    fn rejects_flow_analysis_from_an_earlier_revision_of_the_same_sequence() {
        let mut sequence = CodeSequence::new();
        let target = sequence.new_label().unwrap();
        sequence
            .instruction(Instruction::IConst(SmallIntConstant::Zero))
            .branch(BranchInstruction::IfEq(target.clone()))
            .instruction(Instruction::Return)
            .bind(target)
            .unwrap()
            .instruction(Instruction::Return)
            // This block is unreachable, but keeps the block count stable
            // when more instructions are appended to the same revision.
            .instruction(Instruction::IConst(SmallIntConstant::Zero));

        let pool = ConstantPool::default();
        let old_graph = ControlFlowGraph::build(&sequence).unwrap();
        let old_flow = CfgCodeAnalyzer::analyze(&old_graph, "()V", true, &pool).unwrap();

        sequence
            .instruction(Instruction::IStore(crate::bytecode::LocalIndex::new(256)))
            .instruction(Instruction::Return);
        let new_graph = ControlFlowGraph::build(&sequence).unwrap();
        let new_flow = CfgCodeAnalyzer::analyze(&new_graph, "()V", true, &pool).unwrap();
        let new_assembled = assemble_labeled(&sequence, &pool).unwrap();

        // Structural checks alone cannot distinguish these snapshots.
        assert_eq!(old_graph.blocks().len(), new_graph.blocks().len());
        assert_eq!(
            old_flow
                .block_entries()
                .iter()
                .map(Option::is_some)
                .collect::<Vec<_>>(),
            new_flow
                .block_entries()
                .iter()
                .map(Option::is_some)
                .collect::<Vec<_>>()
        );
        assert_eq!(old_flow.max_locals, 0);
        assert_eq!(new_flow.max_locals, 257);
        assert_eq!(
            generate_stack_map_frames(&old_flow, &new_graph, &new_assembled),
            Err(StackMapFrameGenerationError::DifferentCodeSequence)
        );
    }

    #[test]
    fn rejects_instance_receiver_placeholder_and_preserves_top_holes() {
        let mut instance = CodeSequence::new();
        let target = instance.new_label().unwrap();
        instance
            .instruction(Instruction::IConst(SmallIntConstant::Zero))
            .branch(BranchInstruction::IfEq(target.clone()))
            .instruction(Instruction::Return)
            .bind(target)
            .unwrap()
            .instruction(Instruction::Return);
        let pool = ConstantPool::default();
        let graph = ControlFlowGraph::build(&instance).unwrap();
        let flow = CfgCodeAnalyzer::analyze(&graph, "()V", false, &pool).unwrap();
        let assembled = assemble_labeled(&instance, &pool).unwrap();
        assert!(matches!(
            generate_stack_map_frames(&flow, &graph, &assembled),
            Err(StackMapFrameGenerationError::UnsupportedFlow {
                state: UnsupportedStackMapState::InstanceReceiver,
                ..
            })
        ));

        let mut static_code = CodeSequence::new();
        let target = static_code.new_label().unwrap();
        static_code
            .instruction(Instruction::ILoad(crate::bytecode::LocalIndex::new(0)))
            .branch(BranchInstruction::IfEq(target.clone()))
            .instruction(Instruction::Return)
            .bind(target)
            .unwrap()
            .instruction(Instruction::Return);
        let (graph, flow, assembled) = analyze_sequence(&static_code, "(I)V");
        let generated = generate_stack_map_frames(&flow, &graph, &assembled).unwrap();
        assert_eq!(generated.frames[0].locals, [VerificationTypeInfo::Integer]);
    }

    #[test]
    fn generates_instance_receiver_frame_with_current_class_type() {
        let mut sequence = CodeSequence::new();
        let target = sequence.new_label().unwrap();
        sequence
            .instruction(Instruction::IConst(SmallIntConstant::Zero))
            .branch(BranchInstruction::IfEq(target.clone()))
            .instruction(Instruction::Return)
            .bind(target)
            .unwrap()
            .instruction(Instruction::Return);

        let mut pool_builder = ConstantPoolBuilder::new();
        let this_class = pool_builder.class("example/Receiver").unwrap();
        let pool = pool_builder.finish().unwrap();
        let graph = ControlFlowGraph::build(&sequence).unwrap();
        let flow = CfgCodeAnalyzer::analyze(&graph, "()V", false, &pool).unwrap();
        let assembled = assemble_labeled(&sequence, &pool).unwrap();
        let generated =
            generate_stack_map_frames_with_receiver(&flow, &graph, &assembled, &pool, this_class)
                .unwrap();

        assert_eq!(
            generated.frames[0].locals,
            [VerificationTypeInfo::Object(this_class)]
        );
        table(generated)
            .encode_payload_with_layout(&pool, &assembled.bytes, &assembled.layout)
            .unwrap();
    }
}
