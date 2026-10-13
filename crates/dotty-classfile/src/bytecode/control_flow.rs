//! Deterministic control-flow graph construction for symbolic code sequences.
//!
//! Blocks preserve executable instruction order and source instruction
//! ranges. Labels resolve to blocks, and graph edges are independent of byte
//! offsets and assembler layout. Unreachable blocks are retained and marked;
//! only a reachable path that falls past the end of the sequence is rejected.

use super::{CodeElement, CodeSequence, CodeSequenceError, Instruction, LabelId};
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::ops::Range;

/// A stable block index assigned in source order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BasicBlockId(usize);

impl BasicBlockId {
    /// Returns this block's zero-based source-order index.
    pub const fn index(self) -> usize {
        self.0
    }
}

/// One source-ordered basic block and its graph relationships.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicBlock {
    /// Stable source-order identity.
    pub id: BasicBlockId,
    /// Half-open range of executable instruction indexes in the input
    /// sequence. Labels do not contribute to these indexes.
    pub instruction_range: Range<usize>,
    /// Original instructions and branches in this block, in source order.
    pub instructions: Vec<CodeElement>,
    /// Successor blocks in deterministic semantic order.
    pub successors: Vec<BasicBlockId>,
    /// Predecessor blocks in source order, without duplicate edges.
    pub predecessors: Vec<BasicBlockId>,
    /// Whether this block is reachable from the entry block.
    pub reachable: bool,
    /// Whether ordinary or conditional fallthrough would leave the sequence.
    /// This can be true only for an unreachable block in a successful graph.
    pub falls_through_end: bool,
}

/// A typed failure while building a ControlFlowGraph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlFlowGraphError {
    /// Symbolic labels or branch targets are invalid.
    InvalidCodeSequence(CodeSequenceError),
    /// A sequence without executable instructions cannot have an entry block.
    EmptyCodeSequence,
    /// A defined label sits after the final instruction and has no block.
    LabelAtCodeEnd { label: LabelId },
    /// A reachable block can fall through beyond the final instruction.
    FallthroughPastEnd {
        /// The block whose fallthrough leaves the sequence.
        block: BasicBlockId,
        /// The final executable instruction index in the block.
        instruction: usize,
    },
}

impl fmt::Display for ControlFlowGraphError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCodeSequence(error) => {
                write!(formatter, "invalid symbolic code sequence: {error}")
            }
            Self::EmptyCodeSequence => {
                formatter.write_str("cannot build a control-flow graph for empty code")
            }
            Self::LabelAtCodeEnd { label } => {
                write!(formatter, "label {label:?} has no instruction at code end")
            }
            Self::FallthroughPastEnd { block, instruction } => write!(
                formatter,
                "reachable block {} falls past the end at instruction {instruction}",
                block.index()
            ),
        }
    }
}

impl std::error::Error for ControlFlowGraphError {}

/// A deterministic control-flow graph for one symbolic code sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlFlowGraph {
    entry_block: BasicBlockId,
    blocks: Vec<BasicBlock>,
    label_blocks: HashMap<LabelId, BasicBlockId>,
}

impl ControlFlowGraph {
    /// Builds a graph from a symbolic sequence without using byte offsets.
    ///
    /// Blocks are assigned IDs in source order. Labels at the same executable
    /// instruction resolve to the same block. Unreachable blocks are kept in
    /// the result and marked with reachable = false.
    pub fn build(sequence: &CodeSequence) -> Result<Self, ControlFlowGraphError> {
        let validated = sequence
            .validate()
            .map_err(ControlFlowGraphError::InvalidCodeSequence)?;
        let mut instructions = Vec::with_capacity(validated.instruction_count());
        let mut label_positions = HashMap::new();
        for element in validated.elements() {
            match element {
                CodeElement::Label(label) => {
                    let position = instructions.len();
                    if position == validated.instruction_count() {
                        return Err(ControlFlowGraphError::LabelAtCodeEnd {
                            label: label.clone(),
                        });
                    }
                    label_positions.insert(label.clone(), position);
                }
                CodeElement::Instruction(_) | CodeElement::Branch(_) => {
                    instructions.push(element.clone());
                }
            }
        }
        if instructions.is_empty() {
            return Err(ControlFlowGraphError::EmptyCodeSequence);
        }

        let instruction_count = instructions.len();
        let mut leaders = vec![false; instruction_count];
        leaders[0] = true;
        for (index, instruction) in instructions.iter().enumerate() {
            match instruction {
                CodeElement::Branch(branch) => {
                    let target =
                        label_positions
                            .get(branch.target())
                            .copied()
                            .ok_or_else(|| {
                                ControlFlowGraphError::InvalidCodeSequence(
                                    CodeSequenceError::UnboundLabel {
                                        label: branch.target().clone(),
                                    },
                                )
                            })?;
                    leaders[target] = true;
                    if index + 1 < instruction_count {
                        leaders[index + 1] = true;
                    }
                }
                CodeElement::Instruction(instruction) if is_return(*instruction) => {
                    if index + 1 < instruction_count {
                        leaders[index + 1] = true;
                    }
                }
                CodeElement::Instruction(_) => {}
                CodeElement::Label(_) => unreachable!("labels are filtered from executable code"),
            }
        }

        let starts: Vec<usize> = leaders
            .iter()
            .enumerate()
            .filter_map(|(index, &is_leader)| is_leader.then_some(index))
            .collect();
        let mut instruction_blocks = vec![BasicBlockId(0); instruction_count];
        let mut blocks = Vec::with_capacity(starts.len());
        for (block_index, &start) in starts.iter().enumerate() {
            let end = starts
                .get(block_index + 1)
                .copied()
                .unwrap_or(instruction_count);
            let id = BasicBlockId(block_index);
            instruction_blocks[start..end].fill(id);
            blocks.push(BasicBlock {
                id,
                instruction_range: start..end,
                instructions: instructions[start..end].to_vec(),
                successors: Vec::new(),
                predecessors: Vec::new(),
                reachable: false,
                falls_through_end: false,
            });
        }

        let mut label_blocks = HashMap::with_capacity(label_positions.len());
        for (label, position) in label_positions {
            label_blocks.insert(label, instruction_blocks[position]);
        }

        for block_index in 0..blocks.len() {
            let end = blocks[block_index].instruction_range.end;
            let last_instruction = &instructions[end - 1];
            match last_instruction {
                CodeElement::Branch(branch) => {
                    let target = label_blocks.get(branch.target()).copied().ok_or_else(|| {
                        ControlFlowGraphError::InvalidCodeSequence(
                            CodeSequenceError::UnboundLabel {
                                label: branch.target().clone(),
                            },
                        )
                    })?;
                    push_unique(&mut blocks[block_index].successors, target);
                    if branch.is_conditional() {
                        add_fallthrough(
                            &mut blocks,
                            block_index,
                            end,
                            instruction_count,
                            &instruction_blocks,
                        );
                    }
                }
                CodeElement::Instruction(instruction) if is_return(*instruction) => {}
                CodeElement::Instruction(_) => add_fallthrough(
                    &mut blocks,
                    block_index,
                    end,
                    instruction_count,
                    &instruction_blocks,
                ),
                CodeElement::Label(_) => unreachable!("labels are filtered from executable code"),
            }
        }

        for source_index in 0..blocks.len() {
            let source = blocks[source_index].id;
            let successors = blocks[source_index].successors.clone();
            for successor in successors {
                push_unique(&mut blocks[successor.index()].predecessors, source);
            }
        }

        let entry_block = BasicBlockId(0);
        mark_reachable(entry_block, &mut blocks);
        for block in &blocks {
            if block.reachable && block.falls_through_end {
                return Err(ControlFlowGraphError::FallthroughPastEnd {
                    block: block.id,
                    instruction: block.instruction_range.end - 1,
                });
            }
        }

        Ok(Self {
            entry_block,
            blocks,
            label_blocks,
        })
    }

    /// Returns the graph's entry block.
    pub const fn entry_block(&self) -> BasicBlockId {
        self.entry_block
    }

    /// Returns all blocks in deterministic source order.
    pub fn blocks(&self) -> &[BasicBlock] {
        &self.blocks
    }

    /// Returns a block by its stable ID.
    pub fn block(&self, id: BasicBlockId) -> Option<&BasicBlock> {
        self.blocks.get(id.index()).filter(|block| block.id == id)
    }

    /// Returns the block for a label defined in the input sequence.
    pub fn block_for_label(&self, label: &LabelId) -> Option<BasicBlockId> {
        self.label_blocks.get(label).copied()
    }
}

fn add_fallthrough(
    blocks: &mut [BasicBlock],
    block_index: usize,
    end: usize,
    instruction_count: usize,
    instruction_blocks: &[BasicBlockId],
) {
    if end < instruction_count {
        push_unique(&mut blocks[block_index].successors, instruction_blocks[end]);
    } else {
        blocks[block_index].falls_through_end = true;
    }
}

fn push_unique(blocks: &mut Vec<BasicBlockId>, id: BasicBlockId) {
    if !blocks.contains(&id) {
        blocks.push(id);
    }
}

fn mark_reachable(entry: BasicBlockId, blocks: &mut [BasicBlock]) {
    let mut pending = VecDeque::from([entry]);
    blocks[entry.index()].reachable = true;
    while let Some(id) = pending.pop_front() {
        let successors = blocks[id.index()].successors.clone();
        for successor in successors {
            if !blocks[successor.index()].reachable {
                blocks[successor.index()].reachable = true;
                pending.push_back(successor);
            }
        }
    }
}

fn is_return(instruction: Instruction) -> bool {
    matches!(
        instruction,
        Instruction::Return | Instruction::IReturn | Instruction::AReturn | Instruction::LReturn
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::{BranchInstruction, SmallIntConstant};

    fn instruction(sequence: &mut CodeSequence, instruction: Instruction) {
        sequence.instruction(instruction);
    }

    fn block_ids(ids: &[usize]) -> Vec<BasicBlockId> {
        ids.iter().copied().map(BasicBlockId).collect()
    }

    #[test]
    fn linear_code_is_one_source_ordered_block() {
        let mut sequence = CodeSequence::new();
        instruction(&mut sequence, Instruction::IConst(SmallIntConstant::Zero));
        instruction(&mut sequence, Instruction::IReturn);

        let graph = ControlFlowGraph::build(&sequence).unwrap();
        assert_eq!(graph.entry_block(), BasicBlockId(0));
        assert_eq!(graph.blocks().len(), 1);
        assert_eq!(graph.blocks()[0].instruction_range, 0..2);
        assert_eq!(graph.blocks()[0].instructions.len(), 2);
        assert_eq!(graph.blocks()[0].successors, []);
        assert_eq!(graph.blocks()[0].predecessors, []);
        assert!(graph.blocks()[0].reachable);
    }

    #[test]
    fn conditional_diamond_has_ordered_edges_and_join_predecessors() {
        let mut sequence = CodeSequence::new();
        let otherwise = sequence.new_label().unwrap();
        let join = sequence.new_label().unwrap();
        sequence.branch(BranchInstruction::IfEq(otherwise.clone()));
        instruction(&mut sequence, Instruction::IConst(SmallIntConstant::One));
        sequence.branch(BranchInstruction::Goto(join.clone()));
        sequence.bind(otherwise.clone()).unwrap();
        instruction(&mut sequence, Instruction::IConst(SmallIntConstant::Two));
        sequence.bind(join.clone()).unwrap();
        instruction(&mut sequence, Instruction::IReturn);

        let graph = ControlFlowGraph::build(&sequence).unwrap();
        assert_eq!(
            graph
                .blocks()
                .iter()
                .map(|block| block.instruction_range.clone())
                .collect::<Vec<_>>(),
            [0..1, 1..3, 3..4, 4..5]
        );
        assert_eq!(graph.blocks()[0].successors, block_ids(&[2, 1]));
        assert_eq!(graph.blocks()[1].successors, block_ids(&[3]));
        assert_eq!(graph.blocks()[2].successors, block_ids(&[3]));
        assert_eq!(graph.blocks()[3].predecessors, block_ids(&[1, 2]));
        assert_eq!(graph.block_for_label(&otherwise), Some(BasicBlockId(2)));
        assert_eq!(graph.block_for_label(&join), Some(BasicBlockId(3)));
        assert!(graph.blocks().iter().all(|block| block.reachable));
    }

    #[test]
    fn goto_and_returns_preserve_unreachable_blocks() {
        let mut sequence = CodeSequence::new();
        let target = sequence.new_label().unwrap();
        sequence.branch(BranchInstruction::Goto(target.clone()));
        instruction(&mut sequence, Instruction::Return);
        sequence.bind(target).unwrap();
        instruction(&mut sequence, Instruction::Return);
        instruction(&mut sequence, Instruction::IConst(SmallIntConstant::Zero));
        instruction(&mut sequence, Instruction::IReturn);

        let graph = ControlFlowGraph::build(&sequence).unwrap();
        assert_eq!(graph.blocks().len(), 4);
        assert_eq!(graph.blocks()[0].successors, block_ids(&[2]));
        assert_eq!(graph.blocks()[1].successors, []);
        assert_eq!(graph.blocks()[2].successors, []);
        assert_eq!(graph.blocks()[3].successors, []);
        assert_eq!(
            graph
                .blocks()
                .iter()
                .map(|block| block.reachable)
                .collect::<Vec<_>>(),
            [true, false, true, false]
        );
    }

    #[test]
    fn backward_and_self_branches_are_represented() {
        let mut sequence = CodeSequence::new();
        let start = sequence.new_label().unwrap();
        let exit = sequence.new_label().unwrap();
        sequence.bind(start.clone()).unwrap();
        sequence.branch(BranchInstruction::IfEq(exit.clone()));
        sequence.branch(BranchInstruction::Goto(start.clone()));
        sequence.bind(exit).unwrap();
        instruction(&mut sequence, Instruction::Return);

        let graph = ControlFlowGraph::build(&sequence).unwrap();
        assert_eq!(graph.blocks()[0].successors, block_ids(&[2, 1]));
        assert_eq!(graph.blocks()[1].successors, block_ids(&[0]));
        assert_eq!(graph.blocks()[0].predecessors, block_ids(&[1]));
        assert!(graph.blocks().iter().all(|block| block.reachable));

        let mut self_loop = CodeSequence::new();
        let again = self_loop.new_label().unwrap();
        self_loop.bind(again.clone()).unwrap();
        self_loop.branch(BranchInstruction::Goto(again));
        let graph = ControlFlowGraph::build(&self_loop).unwrap();
        assert_eq!(graph.blocks()[0].successors, block_ids(&[0]));
        assert_eq!(graph.blocks()[0].predecessors, block_ids(&[0]));
    }

    #[test]
    fn identical_branch_and_fallthrough_edges_are_deduplicated() {
        let mut sequence = CodeSequence::new();
        let target = sequence.new_label().unwrap();
        let alias = sequence.new_label().unwrap();
        sequence.branch(BranchInstruction::IfNe(target.clone()));
        sequence.bind(target.clone()).unwrap();
        sequence.bind(alias.clone()).unwrap();
        instruction(&mut sequence, Instruction::Return);

        let graph = ControlFlowGraph::build(&sequence).unwrap();
        assert_eq!(graph.blocks().len(), 2);
        assert_eq!(graph.blocks()[0].successors, block_ids(&[1]));
        assert_eq!(graph.blocks()[1].predecessors, block_ids(&[0]));
        assert_eq!(graph.block_for_label(&target), Some(BasicBlockId(1)));
        assert_eq!(graph.block_for_label(&alias), Some(BasicBlockId(1)));
    }

    #[test]
    fn reachable_fallthrough_past_end_is_rejected_but_unreachable_is_retained() {
        let mut reachable = CodeSequence::new();
        instruction(&mut reachable, Instruction::IConst(SmallIntConstant::Zero));
        assert!(matches!(
            ControlFlowGraph::build(&reachable),
            Err(ControlFlowGraphError::FallthroughPastEnd {
                block: BasicBlockId(0),
                instruction: 0
            })
        ));

        let mut unreachable = CodeSequence::new();
        let terminal = unreachable.new_label().unwrap();
        unreachable.branch(BranchInstruction::Goto(terminal.clone()));
        instruction(&mut unreachable, Instruction::Return);
        sequence_bind_return(&mut unreachable, terminal);
        instruction(
            &mut unreachable,
            Instruction::IConst(SmallIntConstant::Zero),
        );
        let graph = ControlFlowGraph::build(&unreachable).unwrap();
        assert!(!graph.blocks()[3].reachable);
        assert!(graph.blocks()[3].falls_through_end);
    }

    fn sequence_bind_return(sequence: &mut CodeSequence, label: LabelId) {
        sequence.bind(label).unwrap();
        sequence.instruction(Instruction::Return);
    }

    #[test]
    fn invalid_and_empty_sequences_return_typed_errors() {
        assert_eq!(
            ControlFlowGraph::build(&CodeSequence::new()).unwrap_err(),
            ControlFlowGraphError::EmptyCodeSequence
        );

        let mut sequence = CodeSequence::new();
        let missing = sequence.new_label().unwrap();
        sequence.branch(BranchInstruction::Goto(missing.clone()));
        assert!(matches!(
            ControlFlowGraph::build(&sequence),
            Err(ControlFlowGraphError::InvalidCodeSequence(
                CodeSequenceError::UnboundLabel { label }
            )) if label == missing
        ));

        let mut terminal_label = CodeSequence::new();
        let label = terminal_label.new_label().unwrap();
        terminal_label.instruction(Instruction::Return);
        terminal_label.bind(label.clone()).unwrap();
        assert_eq!(
            ControlFlowGraph::build(&terminal_label).unwrap_err(),
            ControlFlowGraphError::LabelAtCodeEnd { label }
        );
    }
}
