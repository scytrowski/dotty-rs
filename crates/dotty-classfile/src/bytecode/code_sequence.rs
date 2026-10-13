//! Symbolic labels and branches for a method's bytecode sequence.
//!
//! This module validates branch targets without assigning byte offsets or
//! encoding branch instructions. Labels identify positions between elements;
//! multiple labels may identify the same position. A validated sequence is
//! ready for [`super::assembler::assemble_labeled`], but does not perform
//! control-flow analysis or change the existing straight-line
//! [`super::Instruction`] APIs.

use super::Instruction;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// An opaque label owned by one [`CodeSequence`].
///
/// Labels from different sequences are distinct even when they have the same
/// local index. Create labels with [`CodeSequence::new_label`].
#[derive(Clone)]
pub struct LabelId {
    scope: Arc<()>,
    index: usize,
}

impl LabelId {
    fn belongs_to(&self, scope: &Arc<()>) -> bool {
        Arc::ptr_eq(&self.scope, scope)
    }
}

impl fmt::Debug for LabelId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "LabelId({})", self.index)
    }
}

impl PartialEq for LabelId {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index && Arc::ptr_eq(&self.scope, &other.scope)
    }
}

impl Eq for LabelId {}

impl Hash for LabelId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.scope).hash(state);
        self.index.hash(state);
    }
}

/// A symbolic JVM branch with a label target and no encoded offset.
///
/// `Goto` is the short `goto` form; [`Self::GotoWide`] is the explicit wide
/// form. Neither form is automatically widened. Conditional variants
/// correspond to the JVM's `ifeq`, `ifne`, and integer comparison branch
/// family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchInstruction {
    /// Unconditional short branch (`goto`), with a signed 16-bit offset.
    Goto(LabelId),
    /// Unconditional wide branch (`goto_w`), with a signed 32-bit offset.
    GotoWide(LabelId),
    /// Branch when the top integer value is zero (`ifeq`).
    IfEq(LabelId),
    /// Branch when the top integer value is nonzero (`ifne`).
    IfNe(LabelId),
    /// Branch when two integer values are equal (`if_icmpeq`).
    IfICmpEq(LabelId),
    /// Branch when two integer values are unequal (`if_icmpne`).
    IfICmpNe(LabelId),
    /// Branch when the first integer value is less than the second (`if_icmplt`).
    IfICmpLt(LabelId),
    /// Branch when the first integer value is greater than or equal to the second (`if_icmpge`).
    IfICmpGe(LabelId),
    /// Branch when the first integer value is greater than the second (`if_icmpgt`).
    IfICmpGt(LabelId),
    /// Branch when the first integer value is less than or equal to the second (`if_icmple`).
    IfICmpLe(LabelId),
}

impl BranchInstruction {
    /// Returns the symbolic branch target.
    pub fn target(&self) -> &LabelId {
        match self {
            Self::Goto(target)
            | Self::GotoWide(target)
            | Self::IfEq(target)
            | Self::IfNe(target)
            | Self::IfICmpEq(target)
            | Self::IfICmpNe(target)
            | Self::IfICmpLt(target)
            | Self::IfICmpGe(target)
            | Self::IfICmpGt(target)
            | Self::IfICmpLe(target) => target,
        }
    }

    /// Returns the JVM comparison represented by this branch.
    pub const fn condition(&self) -> BranchCondition {
        match self {
            Self::Goto(_) | Self::GotoWide(_) => BranchCondition::Always,
            Self::IfEq(_) => BranchCondition::EqualZero,
            Self::IfNe(_) => BranchCondition::NotEqualZero,
            Self::IfICmpEq(_) => BranchCondition::IntEqual,
            Self::IfICmpNe(_) => BranchCondition::IntNotEqual,
            Self::IfICmpLt(_) => BranchCondition::IntLessThan,
            Self::IfICmpGe(_) => BranchCondition::IntGreaterOrEqual,
            Self::IfICmpGt(_) => BranchCondition::IntGreaterThan,
            Self::IfICmpLe(_) => BranchCondition::IntLessOrEqual,
        }
    }

    /// Whether this branch has a comparison condition.
    pub const fn is_conditional(&self) -> bool {
        !matches!(self, Self::Goto(_) | Self::GotoWide(_))
    }
}

/// The comparison semantics of a [`BranchInstruction`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BranchCondition {
    /// Always take the branch.
    Always,
    /// Take the branch when an integer value equals zero.
    EqualZero,
    /// Take the branch when an integer value is nonzero.
    NotEqualZero,
    /// Take the branch when two integer values are equal.
    IntEqual,
    /// Take the branch when two integer values are unequal.
    IntNotEqual,
    /// Take the branch when the first integer is less than the second.
    IntLessThan,
    /// Take the branch when the first integer is greater than or equal to the second.
    IntGreaterOrEqual,
    /// Take the branch when the first integer is greater than the second.
    IntGreaterThan,
    /// Take the branch when the first integer is less than or equal to the second.
    IntLessOrEqual,
}

/// One item in a symbolic method code sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeElement {
    /// A label at the current position, between executable instructions.
    Label(LabelId),
    /// An existing typed straight-line instruction.
    Instruction(Instruction),
    /// A branch with a symbolic target.
    Branch(BranchInstruction),
}

/// A typed error while constructing or validating a [`CodeSequence`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeSequenceError {
    /// A label belongs to another sequence.
    ForeignLabel { label: LabelId },
    /// A label was bound more than once.
    DuplicateLabelDefinition { label: LabelId },
    /// A branch refers to a label that was never bound.
    UnboundLabel { label: LabelId },
    /// A branch target is the position after the final instruction.
    BranchToTerminalEnd { label: LabelId },
    /// The sequence has exhausted its available local label indexes.
    LabelIdExhausted,
}

/// The category of a [`CodeSequenceError`], without its sequence-scoped label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodeSequenceErrorKind {
    /// The label belongs to another sequence.
    ForeignLabel,
    /// The label has more than one definition.
    DuplicateLabelDefinition,
    /// A branch refers to an undefined label.
    UnboundLabel,
    /// A branch targets the position after the last instruction.
    BranchToTerminalEnd,
    /// The sequence exhausted its local label indexes.
    LabelIdExhausted,
}

impl CodeSequenceError {
    /// Returns this error's category without its sequence-scoped label.
    pub const fn kind(&self) -> CodeSequenceErrorKind {
        match self {
            Self::ForeignLabel { .. } => CodeSequenceErrorKind::ForeignLabel,
            Self::DuplicateLabelDefinition { .. } => {
                CodeSequenceErrorKind::DuplicateLabelDefinition
            }
            Self::UnboundLabel { .. } => CodeSequenceErrorKind::UnboundLabel,
            Self::BranchToTerminalEnd { .. } => CodeSequenceErrorKind::BranchToTerminalEnd,
            Self::LabelIdExhausted => CodeSequenceErrorKind::LabelIdExhausted,
        }
    }
}

impl fmt::Display for CodeSequenceErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ForeignLabel => "foreign label",
            Self::DuplicateLabelDefinition => "duplicate label definition",
            Self::UnboundLabel => "unbound branch target",
            Self::BranchToTerminalEnd => "branch target is at code end",
            Self::LabelIdExhausted => "label IDs exhausted",
        })
    }
}

impl fmt::Display for CodeSequenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ForeignLabel { label } => {
                write!(
                    formatter,
                    "label {label:?} belongs to another code sequence"
                )
            }
            Self::DuplicateLabelDefinition { label } => {
                write!(formatter, "label {label:?} is defined more than once")
            }
            Self::UnboundLabel { label } => write!(formatter, "branch target {label:?} is unbound"),
            Self::BranchToTerminalEnd { label } => write!(
                formatter,
                "branch target {label:?} points after the final instruction"
            ),
            Self::LabelIdExhausted => formatter.write_str("code sequence exhausted label IDs"),
        }
    }
}

impl std::error::Error for CodeSequenceError {}

/// A method's instructions and symbolic branches before target validation.
#[derive(Debug)]
pub struct CodeSequence {
    scope: Arc<()>,
    next_label: usize,
    elements: Vec<CodeElement>,
    definitions: HashSet<LabelId>,
}

impl Default for CodeSequence {
    fn default() -> Self {
        Self::new()
    }
}

impl CodeSequence {
    /// Creates an empty symbolic code sequence.
    pub fn new() -> Self {
        Self {
            scope: Arc::new(()),
            next_label: 0,
            elements: Vec::new(),
            definitions: HashSet::new(),
        }
    }

    /// Allocates a label that can be bound or used as a branch target in this
    /// sequence.
    pub fn new_label(&mut self) -> Result<LabelId, CodeSequenceError> {
        let index = self.next_label;
        self.next_label = index
            .checked_add(1)
            .ok_or(CodeSequenceError::LabelIdExhausted)?;
        Ok(LabelId {
            scope: Arc::clone(&self.scope),
            index,
        })
    }

    /// Binds a label at the current position. Several labels may be bound at
    /// the same position.
    pub fn bind(&mut self, label: LabelId) -> Result<&mut Self, CodeSequenceError> {
        if !label.belongs_to(&self.scope) {
            return Err(CodeSequenceError::ForeignLabel { label });
        }
        if !self.definitions.insert(label.clone()) {
            return Err(CodeSequenceError::DuplicateLabelDefinition { label });
        }
        self.elements.push(CodeElement::Label(label));
        Ok(self)
    }

    /// Appends an existing typed instruction without changing its API or
    /// representation.
    pub fn instruction(&mut self, instruction: Instruction) -> &mut Self {
        self.elements.push(CodeElement::Instruction(instruction));
        self
    }

    /// Appends a symbolic branch. Its target is checked by [`Self::validate`].
    pub fn branch(&mut self, branch: BranchInstruction) -> &mut Self {
        self.elements.push(CodeElement::Branch(branch));
        self
    }

    /// Validates branch ownership, definitions, and executable target
    /// positions, producing a sequence ready for
    /// [`super::assembler::assemble_labeled`].
    ///
    /// This does not assign byte offsets, encode branch bytes, or perform
    /// control-flow or stack analysis.
    pub fn validate(&self) -> Result<ValidatedCodeSequence, CodeSequenceError> {
        let mut positions = HashMap::with_capacity(self.definitions.len());
        let mut instruction_index = 0usize;
        for element in &self.elements {
            match element {
                CodeElement::Label(label) => {
                    positions.insert(label.clone(), instruction_index);
                }
                CodeElement::Instruction(_) | CodeElement::Branch(_) => {
                    instruction_index += 1;
                }
            }
        }

        for element in &self.elements {
            let CodeElement::Branch(branch) = element else {
                continue;
            };
            let label = branch.target();
            if !label.belongs_to(&self.scope) {
                return Err(CodeSequenceError::ForeignLabel {
                    label: label.clone(),
                });
            }
            let Some(&target_position) = positions.get(label) else {
                return Err(CodeSequenceError::UnboundLabel {
                    label: label.clone(),
                });
            };
            if target_position == instruction_index {
                return Err(CodeSequenceError::BranchToTerminalEnd {
                    label: label.clone(),
                });
            }
        }

        Ok(ValidatedCodeSequence {
            scope: Arc::clone(&self.scope),
            elements: self.elements.clone(),
            positions,
            instruction_count: instruction_index,
        })
    }
}

/// A symbolic code sequence whose branch labels all resolve to executable
/// instruction positions.
#[derive(Debug)]
pub struct ValidatedCodeSequence {
    scope: Arc<()>,
    elements: Vec<CodeElement>,
    positions: HashMap<LabelId, usize>,
    instruction_count: usize,
}

impl ValidatedCodeSequence {
    pub(crate) fn sequence_scope(&self) -> &Arc<()> {
        &self.scope
    }

    /// Returns the sequence in source order, including zero-width labels.
    pub fn elements(&self) -> &[CodeElement] {
        &self.elements
    }

    /// Returns the zero-based executable instruction position for a label
    /// owned by this sequence. Labels do not contribute to this index.
    pub fn instruction_index(&self, label: &LabelId) -> Option<usize> {
        label
            .belongs_to(&self.scope)
            .then(|| self.positions.get(label).copied())
            .flatten()
    }

    /// Returns the number of executable instructions and branches.
    pub const fn instruction_count(&self) -> usize {
        self.instruction_count
    }
}
