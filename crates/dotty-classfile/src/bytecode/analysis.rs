//! Descriptor-aware analysis for straight-line code and integer-only CFGs.
//!
//! [`LinearCodeAnalyzer`] retains the typed straight-line analysis API.
//! [`CfgCodeAnalyzer`] propagates integer stack and local states across
//! symbolic branches. Neither is a full JVM verifier: reference identity,
//! object initialization, and reference joins are not modeled, and neither
//! analyzer produces stack-map frames.

use super::{
    BasicBlockId, BranchInstruction, ClassReference, CodeElement, ConstantPoolOperandKind,
    ControlFlowGraph, FieldReference, Instruction, Ldc2Constant, LdcConstant, LocalIndex,
    SpecialMethodReference, StaticMethodReference, VirtualMethodReference,
};
use crate::constant_pool::{ConstantPool, ConstantPoolEntry, ConstantPoolIndex, PoolRefError};
use crate::descriptor::{DescriptorError, FieldType, MethodDescriptor};
use crate::encode_error::EncodeError;
use std::fmt;
use std::sync::Arc;

const MAX_FLOW_STATE_SLOTS: usize = 8 * 1024 * 1024;

/// JVM value categories used by the bytecode analyzers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValueKind {
    /// An int-like JVM value (`boolean`, `byte`, `char`, `short`, or `int`).
    Int,
    /// A JVM `float` value.
    Float,
    /// A JVM `long` value.
    Long,
    /// A JVM `double` value.
    Double,
    /// An object or array reference.
    Reference,
}

impl ValueKind {
    const fn slots(self) -> usize {
        match self {
            Self::Long | Self::Double => 2,
            Self::Int | Self::Float | Self::Reference => 1,
        }
    }
}

impl fmt::Display for ValueKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Int => "int",
            Self::Float => "float",
            Self::Long => "long",
            Self::Double => "double",
            Self::Reference => "reference",
        })
    }
}

/// The computed bounds for a straight-line method body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeAnalysis {
    /// Maximum operand-stack depth in JVM slots.
    pub max_stack: u16,
    /// Required local-variable array size in JVM slots.
    pub max_locals: u16,
}

/// A typed failure while analyzing a method body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisError {
    /// The method descriptor is malformed.
    InvalidMethodDescriptor(DescriptorError),
    /// A method descriptor exceeds the JVM's 255 parameter-slot limit.
    TooManyMethodParameterSlots {
        /// Instruction that invokes the method, or `None` for the analyzed method.
        instruction: Option<usize>,
        /// Parameter slots, including the receiver for instance methods.
        slots: usize,
    },
    /// The constant pool has an invalid layout or nested reference.
    InvalidConstantPool(EncodeError),
    /// An instruction needs more stack values than are currently available.
    StackUnderflow {
        /// Zero-based instruction position.
        instruction: usize,
        /// Number of values required by the instruction.
        needed: usize,
        /// Number of values available before the instruction.
        available: usize,
    },
    /// An instruction consumes a value with the wrong JVM category.
    StackTypeMismatch {
        /// Zero-based instruction position.
        instruction: usize,
        /// Required value category.
        expected: ValueKind,
        /// Actual value category.
        actual: ValueKind,
    },
    /// `dup` cannot duplicate a category-two value.
    DupCategoryTwo {
        /// Zero-based instruction position.
        instruction: usize,
        /// The category-two value at the top of the stack.
        actual: ValueKind,
    },
    /// `pop` cannot remove a category-two value.
    PopCategoryTwo {
        /// Zero-based instruction position.
        instruction: usize,
        /// The category-two value at the top of the stack.
        actual: ValueKind,
    },
    /// A constant-pool operand is absent or has the wrong entry family.
    InvalidConstantPoolOperand {
        /// Zero-based instruction position.
        instruction: usize,
        /// Constant-pool entry used by the instruction.
        index: ConstantPoolIndex,
        /// Required constant-pool entry family.
        expected: ConstantPoolOperandKind,
    },
    /// A nested constant-pool reference needed by a member operand is invalid.
    InvalidConstantPoolReference {
        /// Zero-based instruction position.
        instruction: usize,
        /// The invalid nested reference.
        error: PoolRefError,
    },
    /// A field reference contains a malformed field descriptor.
    InvalidFieldDescriptor {
        /// Zero-based instruction position.
        instruction: usize,
        /// Constant-pool entry that owns the descriptor.
        index: ConstantPoolIndex,
        /// Descriptor parser failure.
        error: DescriptorError,
    },
    /// A method reference contains a malformed method descriptor.
    InvalidReferencedMethodDescriptor {
        /// Zero-based instruction position.
        instruction: usize,
        /// Constant-pool entry that owns the descriptor.
        index: ConstantPoolIndex,
        /// Descriptor parser failure.
        error: DescriptorError,
    },
    /// A local access extends past the maximum representable `max_locals`.
    LocalLimitExceeded {
        /// Zero-based instruction position.
        instruction: usize,
        /// Starting local slot.
        index: u16,
        /// Width of the access in local slots.
        width: u8,
    },
    /// A load reads a local slot that has no established value.
    UninitializedLocal {
        /// Zero-based instruction position.
        instruction: usize,
        /// Local slot being read.
        index: u16,
    },
    /// A local slot contains a different value category than the load expects.
    LocalTypeMismatch {
        /// Zero-based instruction position.
        instruction: usize,
        /// Local slot being read.
        index: u16,
        /// Required local value category.
        expected: ValueKind,
        /// Actual local value category.
        actual: ValueKind,
    },
    /// A local index points at the second slot of a category-two value.
    LocalSlotOverlap {
        /// Zero-based instruction position.
        instruction: usize,
        /// Local slot being accessed.
        index: u16,
    },
    /// The maximum stack depth cannot be represented by Code's `max_stack`.
    StackLimitExceeded {
        /// Zero-based instruction position that exceeded the limit.
        instruction: usize,
        /// Required stack depth in slots.
        slots: usize,
    },
    /// A terminal return opcode disagrees with the method descriptor.
    WrongReturnOpcode {
        /// Zero-based instruction position.
        instruction: usize,
        /// Return type required by the method descriptor, or `None` for void.
        expected: Option<ValueKind>,
        /// Return category represented by the actual opcode, or `None` for void.
        actual: Option<ValueKind>,
    },
    /// Values remain on the stack after a return value has been consumed.
    NonEmptyStackAtReturn {
        /// Zero-based instruction position.
        instruction: usize,
        /// Remaining stack depth in JVM slots.
        slots: usize,
    },
    /// The instruction sequence continues after a terminal return.
    InstructionAfterReturn {
        /// Zero-based position of the first instruction after return.
        instruction: usize,
    },
    /// The instruction sequence has no terminal return.
    MissingReturn,
    /// The CFG analyzer encountered an instruction outside its integer-only subset.
    UnsupportedFlowInstruction {
        /// Zero-based instruction position.
        instruction: usize,
        /// Unsupported value family or operation.
        kind: UnsupportedFlowInstructionKind,
    },
    /// The CFG analyzer encountered a non-int-like parameter.
    UnsupportedFlowParameter {
        /// Zero-based method parameter position.
        parameter: usize,
        /// Parameter's JVM category.
        actual: ValueKind,
    },
    /// Incoming operand stacks disagree at a block entry.
    IncompatibleStackMerge {
        /// Block receiving incompatible states.
        block: BasicBlockId,
        /// Stack value count already established for this block.
        expected_values: usize,
        /// Stack value count from the incoming predecessor.
        incoming_values: usize,
    },
    /// Automatic frame generation cannot emit a frame for an unreachable
    /// block because its verifier state is unknown.
    UnreachableBlock {
        /// Block without an inferred entry state.
        block: BasicBlockId,
    },
    /// A defensive fixed-point work limit was exceeded.
    FlowAnalysisLimitExceeded {
        /// Number of blocks processed before the limit was reached.
        steps: usize,
        /// Maximum block processing steps for this graph and local lattice.
        limit: usize,
    },
    /// Retained block-entry states would exceed the analyzer's memory budget.
    FlowStateSizeLimitExceeded {
        /// Total local and stack values across retained block-entry states.
        slots: usize,
        /// Maximum retained state values.
        limit: usize,
    },
    /// An internally inconsistent graph does not provide a block entry state.
    InvalidControlFlowGraph {
        /// Block whose graph state or identity is invalid.
        block: BasicBlockId,
    },
}

/// Why an instruction is outside the integer-only CFG analysis subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedFlowInstructionKind {
    /// The instruction reads, writes, or creates a reference value.
    Reference,
    /// The instruction reads or creates a category-two value.
    CategoryTwo,
    /// The instruction's transfer behavior is not implemented for CFG analysis.
    Operation,
}

impl fmt::Display for AnalysisError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMethodDescriptor(error) => {
                write!(formatter, "invalid method descriptor: {error}")
            }
            Self::TooManyMethodParameterSlots { instruction, slots } => {
                if let Some(instruction) = instruction {
                    write!(formatter, "instruction {instruction} ")?;
                }
                write!(
                    formatter,
                    "method descriptor uses {slots} parameter slots, exceeding the JVM limit of 255"
                )
            }
            Self::InvalidConstantPool(error) => write!(formatter, "invalid constant pool: {error}"),
            Self::StackUnderflow {
                instruction,
                needed,
                available,
            } => write!(
                formatter,
                "instruction {instruction} needs {needed} stack values, but only {available} are available"
            ),
            Self::StackTypeMismatch {
                instruction,
                expected,
                actual,
            } => write!(
                formatter,
                "instruction {instruction} requires {expected}, found {actual} on the stack"
            ),
            Self::DupCategoryTwo {
                instruction,
                actual,
            } => write!(
                formatter,
                "instruction {instruction} cannot dup category-two value {actual}"
            ),
            Self::PopCategoryTwo {
                instruction,
                actual,
            } => write!(
                formatter,
                "instruction {instruction} cannot pop category-two value {actual}"
            ),
            Self::InvalidConstantPoolOperand {
                instruction,
                index,
                expected,
            } => write!(
                formatter,
                "instruction {instruction} constant-pool index {} is not {expected}",
                index.0
            ),
            Self::InvalidConstantPoolReference { instruction, error } => {
                write!(formatter, "instruction {instruction}: {error}")
            }
            Self::InvalidFieldDescriptor {
                instruction,
                index,
                error,
            } => write!(
                formatter,
                "instruction {instruction} field at constant-pool index {} has an invalid descriptor: {error}",
                index.0
            ),
            Self::InvalidReferencedMethodDescriptor {
                instruction,
                index,
                error,
            } => write!(
                formatter,
                "instruction {instruction} method at constant-pool index {} has an invalid descriptor: {error}",
                index.0
            ),
            Self::LocalLimitExceeded {
                instruction,
                index,
                width,
            } => write!(
                formatter,
                "instruction {instruction} local slot {index} with width {width} exceeds max_locals limit"
            ),
            Self::UninitializedLocal { instruction, index } => write!(
                formatter,
                "instruction {instruction} reads uninitialized local slot {index}"
            ),
            Self::LocalTypeMismatch {
                instruction,
                index,
                expected,
                actual,
            } => write!(
                formatter,
                "instruction {instruction} local slot {index} contains {actual}, expected {expected}"
            ),
            Self::LocalSlotOverlap { instruction, index } => write!(
                formatter,
                "instruction {instruction} local slot {index} is the second half of a category-two value"
            ),
            Self::StackLimitExceeded { instruction, slots } => write!(
                formatter,
                "instruction {instruction} requires stack depth {slots}, exceeding u16 max_stack"
            ),
            Self::WrongReturnOpcode {
                instruction,
                expected,
                actual,
            } => write!(
                formatter,
                "instruction {instruction} returns {actual:?}, but the method descriptor requires {expected:?}"
            ),
            Self::NonEmptyStackAtReturn { instruction, slots } => write!(
                formatter,
                "instruction {instruction} returns with {slots} operand-stack slots remaining"
            ),
            Self::InstructionAfterReturn { instruction } => write!(
                formatter,
                "instruction {instruction} follows a terminal return"
            ),
            Self::MissingReturn => {
                formatter.write_str("instruction sequence has no terminal return")
            }
            Self::UnsupportedFlowInstruction { instruction, kind } => write!(
                formatter,
                "instruction {instruction} uses unsupported CFG analysis category {kind:?}"
            ),
            Self::UnsupportedFlowParameter { parameter, actual } => write!(
                formatter,
                "method parameter {parameter} has unsupported CFG analysis category {actual}"
            ),
            Self::IncompatibleStackMerge {
                block,
                expected_values,
                incoming_values,
            } => write!(
                formatter,
                "block {} merges operand stacks with {expected_values} and {incoming_values} values",
                block.index()
            ),
            Self::UnreachableBlock { block } => write!(
                formatter,
                "automatic stack-map generation does not support unreachable block {}",
                block.index()
            ),
            Self::FlowAnalysisLimitExceeded { steps, limit } => write!(
                formatter,
                "CFG analysis processed {steps} blocks, exceeding its fixed-point limit of {limit}"
            ),
            Self::FlowStateSizeLimitExceeded { slots, limit } => write!(
                formatter,
                "CFG analysis requires {slots} retained state values, exceeding its limit of {limit}"
            ),
            Self::InvalidControlFlowGraph { block } => {
                write!(
                    formatter,
                    "CFG block {} has inconsistent analysis state",
                    block.index()
                )
            }
        }
    }
}

impl std::error::Error for AnalysisError {}

/// Analyzes a supported instruction sequence against its method descriptor.
///
/// The calculation includes receiver and parameter slots in `max_locals`,
/// grows locals for explicit stores, and measures stack depth in JVM slots.
/// Field and call effects come from validated constant-pool entry families and
/// their descriptors. The function does not mutate the instruction sequence
/// or constant pool.
///
/// This is structural/category analysis, not full JVMS verification. It does
/// not prove initialized-`this` or object states, reference subtype
/// compatibility, stack-map-frame correctness, or control-flow joins.
///
/// ```
/// use dotty_classfile::classfile::{
///     analyze, ConstantPool, Instruction, LocalIndex,
/// };
///
/// let code = [
///     Instruction::ILoad(LocalIndex::new(0)),
///     Instruction::ILoad(LocalIndex::new(1)),
///     Instruction::IAdd,
///     Instruction::IReturn,
/// ];
/// let bounds = analyze("(II)I", true, &code, &ConstantPool::default())?;
/// assert_eq!(bounds.max_stack, 2);
/// assert_eq!(bounds.max_locals, 2);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn analyze(
    method_descriptor: &str,
    is_static: bool,
    instructions: &[Instruction],
    constant_pool: &ConstantPool,
) -> Result<CodeAnalysis, AnalysisError> {
    LinearCodeAnalyzer::analyze(method_descriptor, is_static, instructions, constant_pool)
}

/// Stateless entry point for straight-line code analysis.
pub struct LinearCodeAnalyzer;

impl LinearCodeAnalyzer {
    /// Analyzes a supported instruction sequence and computes `max_stack` and
    /// `max_locals` in JVM slots.
    pub fn analyze(
        method_descriptor: &str,
        is_static: bool,
        instructions: &[Instruction],
        constant_pool: &ConstantPool,
    ) -> Result<CodeAnalysis, AnalysisError> {
        constant_pool
            .validate_for_encoding()
            .map_err(AnalysisError::InvalidConstantPool)?;
        analyze_validated(method_descriptor, is_static, instructions, constant_pool)
    }
}

/// Analyzes code against a pool whose complete layout was already validated.
/// Method descriptors and instruction operands are still checked.
pub(crate) fn analyze_validated(
    method_descriptor: &str,
    is_static: bool,
    instructions: &[Instruction],
    constant_pool: &ConstantPool,
) -> Result<CodeAnalysis, AnalysisError> {
    let descriptor = MethodDescriptor::parse(method_descriptor)
        .map_err(AnalysisError::InvalidMethodDescriptor)?;
    check_parameter_slots(&descriptor, !is_static, None)?;
    let mut frame = Frame::new(&descriptor, is_static)?;
    let mut returned = false;

    for (instruction_index, instruction) in instructions.iter().copied().enumerate() {
        if returned {
            return Err(AnalysisError::InstructionAfterReturn {
                instruction: instruction_index,
            });
        }
        frame.apply(instruction_index, instruction, &descriptor, constant_pool)?;
        returned = is_return(instruction);
    }

    if !returned {
        return Err(AnalysisError::MissingReturn);
    }
    Ok(CodeAnalysis {
        max_stack: u16::try_from(frame.max_stack).map_err(|_| {
            AnalysisError::StackLimitExceeded {
                instruction: instructions.len().saturating_sub(1),
                slots: frame.max_stack,
            }
        })?,
        max_locals: u16::try_from(frame.locals.len()).map_err(|_| {
            AnalysisError::LocalLimitExceeded {
                instruction: instructions.len().saturating_sub(1),
                index: u16::MAX,
                width: 1,
            }
        })?,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalSlot {
    Value(ValueKind),
    Receiver,
    CategoryTwoTail,
}

struct Frame {
    stack: Vec<ValueKind>,
    stack_slots: usize,
    locals: Vec<Option<LocalSlot>>,
    max_stack: usize,
}

impl Frame {
    fn new(descriptor: &MethodDescriptor, is_static: bool) -> Result<Self, AnalysisError> {
        let mut locals = Vec::new();
        if !is_static {
            locals.push(Some(LocalSlot::Receiver));
        }
        for parameter in &descriptor.parameters {
            let kind = value_kind(parameter);
            let width = kind.slots();
            if locals
                .len()
                .checked_add(width)
                .is_none_or(|size| size > usize::from(u16::MAX))
            {
                return Err(AnalysisError::LocalLimitExceeded {
                    instruction: 0,
                    index: u16::MAX,
                    width: u8::try_from(width).unwrap_or(2),
                });
            }
            locals.push(Some(LocalSlot::Value(kind)));
            if width == 2 {
                locals.push(Some(LocalSlot::CategoryTwoTail));
            }
        }
        Ok(Self {
            stack: Vec::new(),
            stack_slots: 0,
            locals,
            max_stack: 0,
        })
    }

    fn apply(
        &mut self,
        instruction_index: usize,
        instruction: Instruction,
        method_descriptor: &MethodDescriptor,
        pool: &ConstantPool,
    ) -> Result<(), AnalysisError> {
        match instruction {
            Instruction::AConstNull => self.push(instruction_index, ValueKind::Reference)?,
            Instruction::IConst(_) | Instruction::Bipush(_) | Instruction::Sipush(_) => {
                self.push(instruction_index, ValueKind::Int)?
            }
            Instruction::LConst(_) => self.push(instruction_index, ValueKind::Long)?,
            Instruction::Ldc(value) => {
                let kind = self.ldc_kind(instruction_index, value, pool)?;
                self.push(instruction_index, kind)?;
            }
            Instruction::Ldc2(value) => {
                self.validate_ldc2(instruction_index, value, pool)?;
                self.push(instruction_index, ValueKind::Long)?;
            }
            Instruction::ILoad(index) => {
                self.load_local(instruction_index, index, ValueKind::Int)?
            }
            Instruction::LLoad(index) => {
                self.load_local(instruction_index, index, ValueKind::Long)?
            }
            Instruction::ALoad(index) => {
                self.load_local(instruction_index, index, ValueKind::Reference)?
            }
            Instruction::IStore(index) => {
                self.store_local(instruction_index, index, ValueKind::Int)?
            }
            Instruction::LStore(index) => {
                self.store_local(instruction_index, index, ValueKind::Long)?
            }
            Instruction::AStore(index) => {
                self.store_local(instruction_index, index, ValueKind::Reference)?
            }
            Instruction::IAdd => {
                self.pop_expected(instruction_index, ValueKind::Int)?;
                self.pop_expected(instruction_index, ValueKind::Int)?;
                self.push(instruction_index, ValueKind::Int)?;
            }
            Instruction::Dup => {
                let top = *self.stack.last().ok_or(AnalysisError::StackUnderflow {
                    instruction: instruction_index,
                    needed: 1,
                    available: 0,
                })?;
                if top.slots() != 1 {
                    return Err(AnalysisError::DupCategoryTwo {
                        instruction: instruction_index,
                        actual: top,
                    });
                }
                self.push(instruction_index, top)?;
            }
            Instruction::Pop => {
                let available = self.stack.len();
                let actual = self
                    .stack
                    .last()
                    .copied()
                    .ok_or(AnalysisError::StackUnderflow {
                        instruction: instruction_index,
                        needed: 1,
                        available,
                    })?;
                if actual.slots() != 1 {
                    return Err(AnalysisError::PopCategoryTwo {
                        instruction: instruction_index,
                        actual,
                    });
                }
                self.stack.pop();
                self.stack_slots -= 1;
            }
            Instruction::GetField(reference) => {
                let ty = self.field_type(instruction_index, reference, pool)?;
                self.pop_expected(instruction_index, ValueKind::Reference)?;
                self.push(instruction_index, value_kind(&ty))?;
            }
            Instruction::PutField(reference) => {
                let ty = self.field_type(instruction_index, reference, pool)?;
                self.pop_expected(instruction_index, value_kind(&ty))?;
                self.pop_expected(instruction_index, ValueKind::Reference)?;
            }
            Instruction::GetStatic(reference) => {
                let ty = self.field_type(instruction_index, reference, pool)?;
                self.push(instruction_index, value_kind(&ty))?;
            }
            Instruction::PutStatic(reference) => {
                let ty = self.field_type(instruction_index, reference, pool)?;
                self.pop_expected(instruction_index, value_kind(&ty))?;
            }
            Instruction::InvokeStatic(reference) => {
                let descriptor = self.method_descriptor(instruction_index, reference, pool)?;
                self.invoke(instruction_index, &descriptor, false)?;
            }
            Instruction::InvokeVirtual(reference) => {
                let descriptor = self.method_descriptor(instruction_index, reference, pool)?;
                self.invoke(instruction_index, &descriptor, true)?;
            }
            Instruction::InvokeSpecial(reference) => {
                let descriptor = self.method_descriptor(instruction_index, reference, pool)?;
                self.invoke(instruction_index, &descriptor, true)?;
            }
            Instruction::New(reference) => {
                self.validate_class(instruction_index, reference, pool)?;
                self.push(instruction_index, ValueKind::Reference)?;
            }
            Instruction::Return => {
                self.check_return(instruction_index, None, method_descriptor)?;
                self.ensure_empty_stack(instruction_index)?;
            }
            Instruction::IReturn => {
                self.check_return(instruction_index, Some(ValueKind::Int), method_descriptor)?;
                self.pop_expected(instruction_index, ValueKind::Int)?;
                self.ensure_empty_stack(instruction_index)?;
            }
            Instruction::AReturn => {
                self.check_return(
                    instruction_index,
                    Some(ValueKind::Reference),
                    method_descriptor,
                )?;
                self.pop_expected(instruction_index, ValueKind::Reference)?;
                self.ensure_empty_stack(instruction_index)?;
            }
            Instruction::LReturn => {
                self.check_return(instruction_index, Some(ValueKind::Long), method_descriptor)?;
                self.pop_expected(instruction_index, ValueKind::Long)?;
                self.ensure_empty_stack(instruction_index)?;
            }
        }
        Ok(())
    }

    fn push(&mut self, instruction: usize, value: ValueKind) -> Result<(), AnalysisError> {
        let slots = self.stack_slots.checked_add(value.slots()).ok_or(
            AnalysisError::StackLimitExceeded {
                instruction,
                slots: usize::MAX,
            },
        )?;
        if slots > usize::from(u16::MAX) {
            return Err(AnalysisError::StackLimitExceeded { instruction, slots });
        }
        self.stack.push(value);
        self.stack_slots = slots;
        self.max_stack = self.max_stack.max(slots);
        Ok(())
    }

    fn pop_expected(
        &mut self,
        instruction: usize,
        expected: ValueKind,
    ) -> Result<ValueKind, AnalysisError> {
        let available = self.stack.len();
        let actual = self.stack.pop().ok_or(AnalysisError::StackUnderflow {
            instruction,
            needed: 1,
            available,
        })?;
        self.stack_slots -= actual.slots();
        if actual != expected {
            return Err(AnalysisError::StackTypeMismatch {
                instruction,
                expected,
                actual,
            });
        }
        Ok(actual)
    }

    fn ensure_empty_stack(&self, instruction: usize) -> Result<(), AnalysisError> {
        let slots = self.stack_slots;
        if slots == 0 {
            Ok(())
        } else {
            Err(AnalysisError::NonEmptyStackAtReturn { instruction, slots })
        }
    }

    fn load_local(
        &mut self,
        instruction: usize,
        index: LocalIndex,
        expected: ValueKind,
    ) -> Result<(), AnalysisError> {
        self.ensure_local_range(instruction, index, expected)?;
        let slot = self.locals.get(usize::from(index.get())).copied().flatten();
        match slot {
            None => Err(AnalysisError::UninitializedLocal {
                instruction,
                index: index.get(),
            }),
            Some(LocalSlot::CategoryTwoTail) => Err(AnalysisError::LocalSlotOverlap {
                instruction,
                index: index.get(),
            }),
            Some(LocalSlot::Value(actual)) if actual != expected => {
                Err(AnalysisError::LocalTypeMismatch {
                    instruction,
                    index: index.get(),
                    expected,
                    actual,
                })
            }
            Some(LocalSlot::Value(actual)) => self.push(instruction, actual),
            Some(LocalSlot::Receiver) if expected == ValueKind::Reference => {
                self.push(instruction, ValueKind::Reference)
            }
            Some(LocalSlot::Receiver) => Err(AnalysisError::LocalTypeMismatch {
                instruction,
                index: index.get(),
                expected,
                actual: ValueKind::Reference,
            }),
        }
    }

    fn store_local(
        &mut self,
        instruction: usize,
        index: LocalIndex,
        expected: ValueKind,
    ) -> Result<(), AnalysisError> {
        self.ensure_local_range(instruction, index, expected)?;
        self.pop_expected(instruction, expected)?;
        let start = usize::from(index.get());
        let width = expected.slots();
        for slot in start..start + width {
            self.clear_local_slot(slot);
        }
        self.locals[start] = Some(LocalSlot::Value(expected));
        if width == 2 {
            self.locals[start + 1] = Some(LocalSlot::CategoryTwoTail);
        }
        Ok(())
    }

    fn ensure_local_range(
        &mut self,
        instruction: usize,
        index: LocalIndex,
        value: ValueKind,
    ) -> Result<(), AnalysisError> {
        let start = usize::from(index.get());
        let width = value.slots();
        let end = start
            .checked_add(width)
            .ok_or(AnalysisError::LocalLimitExceeded {
                instruction,
                index: index.get(),
                width: u8::try_from(width).unwrap_or(2),
            })?;
        if end > usize::from(u16::MAX) {
            return Err(AnalysisError::LocalLimitExceeded {
                instruction,
                index: index.get(),
                width: u8::try_from(width).unwrap_or(2),
            });
        }
        if self.locals.len() < end {
            self.locals.resize(end, None);
        }
        Ok(())
    }

    fn clear_local_slot(&mut self, slot: usize) {
        if matches!(
            self.locals.get(slot),
            Some(Some(LocalSlot::CategoryTwoTail))
        ) && slot > 0
        {
            self.locals[slot - 1] = None;
        }
        if matches!(self.locals.get(slot), Some(Some(LocalSlot::Value(kind))) if kind.slots() == 2)
            && slot + 1 < self.locals.len()
        {
            self.locals[slot + 1] = None;
        }
        self.locals[slot] = None;
    }

    fn field_type(
        &self,
        instruction: usize,
        reference: FieldReference,
        pool: &ConstantPool,
    ) -> Result<FieldType, AnalysisError> {
        let index = reference.index().get();
        let entry = pool
            .get(index)
            .ok_or(AnalysisError::InvalidConstantPoolOperand {
                instruction,
                index,
                expected: ConstantPoolOperandKind::FieldReference,
            })?;
        let name_and_type_index = match entry {
            ConstantPoolEntry::Fieldref {
                name_and_type_index,
                ..
            } => *name_and_type_index,
            _ => {
                return Err(AnalysisError::InvalidConstantPoolOperand {
                    instruction,
                    index,
                    expected: ConstantPoolOperandKind::FieldReference,
                });
            }
        };
        let (_, descriptor) = pool
            .name_and_type(name_and_type_index)
            .map_err(|error| AnalysisError::InvalidConstantPoolReference { instruction, error })?;
        FieldType::parse(descriptor).map_err(|error| AnalysisError::InvalidFieldDescriptor {
            instruction,
            index,
            error,
        })
    }

    fn ldc_kind(
        &self,
        instruction: usize,
        constant: LdcConstant,
        pool: &ConstantPool,
    ) -> Result<ValueKind, AnalysisError> {
        let index = constant.index().get();
        let expected = match constant {
            LdcConstant::Integer(_) => ConstantPoolOperandKind::IntegerConstant,
            LdcConstant::Reference(_) => ConstantPoolOperandKind::ReferenceConstant,
        };
        let entry = pool
            .get(index)
            .ok_or(AnalysisError::InvalidConstantPoolOperand {
                instruction,
                index,
                expected,
            })?;
        let kind = match (constant, entry) {
            (LdcConstant::Integer(_), ConstantPoolEntry::Integer(_)) => ValueKind::Int,
            (
                LdcConstant::Reference(_),
                ConstantPoolEntry::String { .. }
                | ConstantPoolEntry::Class { .. }
                | ConstantPoolEntry::MethodHandle { .. }
                | ConstantPoolEntry::MethodType { .. },
            ) => ValueKind::Reference,
            (
                LdcConstant::Reference(_),
                ConstantPoolEntry::Dynamic {
                    name_and_type_index,
                    ..
                },
            ) => {
                let field_type =
                    self.dynamic_field_type(instruction, index, *name_and_type_index, pool)?;
                if matches!(field_type, FieldType::Object(_) | FieldType::Array(_)) {
                    ValueKind::Reference
                } else {
                    return Err(AnalysisError::InvalidConstantPoolOperand {
                        instruction,
                        index,
                        expected,
                    });
                }
            }
            _ => {
                return Err(AnalysisError::InvalidConstantPoolOperand {
                    instruction,
                    index,
                    expected,
                });
            }
        };
        Ok(kind)
    }

    fn validate_ldc2(
        &self,
        instruction: usize,
        constant: Ldc2Constant,
        pool: &ConstantPool,
    ) -> Result<(), AnalysisError> {
        let index = constant.index().get();
        match pool.get(index) {
            Some(ConstantPoolEntry::Long(_)) => Ok(()),
            Some(ConstantPoolEntry::Dynamic {
                name_and_type_index,
                ..
            }) => {
                let field_type =
                    self.dynamic_field_type(instruction, index, *name_and_type_index, pool)?;
                if field_type == FieldType::Long {
                    Ok(())
                } else {
                    Err(AnalysisError::InvalidConstantPoolOperand {
                        instruction,
                        index,
                        expected: ConstantPoolOperandKind::LongConstant,
                    })
                }
            }
            _ => Err(AnalysisError::InvalidConstantPoolOperand {
                instruction,
                index,
                expected: ConstantPoolOperandKind::LongConstant,
            }),
        }
    }

    fn dynamic_field_type(
        &self,
        instruction: usize,
        index: ConstantPoolIndex,
        name_and_type_index: ConstantPoolIndex,
        pool: &ConstantPool,
    ) -> Result<FieldType, AnalysisError> {
        let (_, descriptor) = pool
            .name_and_type(name_and_type_index)
            .map_err(|error| AnalysisError::InvalidConstantPoolReference { instruction, error })?;
        FieldType::parse(descriptor).map_err(|error| AnalysisError::InvalidFieldDescriptor {
            instruction,
            index,
            error,
        })
    }

    fn method_descriptor<T: Into<MethodOperand>>(
        &self,
        instruction: usize,
        reference: T,
        pool: &ConstantPool,
    ) -> Result<MethodDescriptor, AnalysisError> {
        let reference = reference.into();
        let index = reference.index;
        let entry = pool
            .get(index)
            .ok_or(AnalysisError::InvalidConstantPoolOperand {
                instruction,
                index,
                expected: reference.expected,
            })?;
        let name_and_type_index = match (reference.accept_interface, entry) {
            (
                true,
                ConstantPoolEntry::Methodref {
                    name_and_type_index,
                    ..
                },
            )
            | (
                true,
                ConstantPoolEntry::InterfaceMethodref {
                    name_and_type_index,
                    ..
                },
            )
            | (
                false,
                ConstantPoolEntry::Methodref {
                    name_and_type_index,
                    ..
                },
            ) => *name_and_type_index,
            _ => {
                return Err(AnalysisError::InvalidConstantPoolOperand {
                    instruction,
                    index,
                    expected: reference.expected,
                });
            }
        };
        let (_, descriptor) = pool
            .name_and_type(name_and_type_index)
            .map_err(|error| AnalysisError::InvalidConstantPoolReference { instruction, error })?;
        MethodDescriptor::parse(descriptor).map_err(|error| {
            AnalysisError::InvalidReferencedMethodDescriptor {
                instruction,
                index,
                error,
            }
        })
    }

    fn invoke(
        &mut self,
        instruction: usize,
        descriptor: &MethodDescriptor,
        has_receiver: bool,
    ) -> Result<(), AnalysisError> {
        check_parameter_slots(descriptor, has_receiver, Some(instruction))?;
        for parameter in descriptor.parameters.iter().rev() {
            self.pop_expected(instruction, value_kind(parameter))?;
        }
        if has_receiver {
            self.pop_expected(instruction, ValueKind::Reference)?;
        }
        if let Some(result) = &descriptor.return_type {
            self.push(instruction, value_kind(result))?;
        }
        Ok(())
    }

    fn validate_class(
        &self,
        instruction: usize,
        reference: ClassReference,
        pool: &ConstantPool,
    ) -> Result<(), AnalysisError> {
        let index = reference.index().get();
        if matches!(pool.get(index), Some(ConstantPoolEntry::Class { .. })) {
            Ok(())
        } else {
            Err(AnalysisError::InvalidConstantPoolOperand {
                instruction,
                index,
                expected: ConstantPoolOperandKind::Class,
            })
        }
    }

    fn check_return(
        &self,
        instruction: usize,
        actual: Option<ValueKind>,
        descriptor: &MethodDescriptor,
    ) -> Result<(), AnalysisError> {
        let expected = descriptor.return_type.as_ref().map(value_kind);
        let compatible = match (expected, actual) {
            (None, None) => true,
            (Some(ValueKind::Int), Some(ValueKind::Int)) => true,
            (Some(ValueKind::Reference), Some(ValueKind::Reference)) => true,
            (Some(expected), Some(actual)) => expected == actual,
            _ => false,
        };
        if compatible {
            Ok(())
        } else {
            Err(AnalysisError::WrongReturnOpcode {
                instruction,
                expected,
                actual,
            })
        }
    }
}

fn check_parameter_slots(
    descriptor: &MethodDescriptor,
    has_receiver: bool,
    instruction: Option<usize>,
) -> Result<(), AnalysisError> {
    let slots = descriptor
        .parameters
        .iter()
        .map(|parameter| value_kind(parameter).slots())
        .sum::<usize>()
        + usize::from(has_receiver);
    if slots > 255 {
        Err(AnalysisError::TooManyMethodParameterSlots { instruction, slots })
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct MethodOperand {
    index: ConstantPoolIndex,
    expected: ConstantPoolOperandKind,
    accept_interface: bool,
}

impl From<StaticMethodReference> for MethodOperand {
    fn from(reference: StaticMethodReference) -> Self {
        Self {
            index: reference.index().get(),
            expected: ConstantPoolOperandKind::StaticOrSpecialMethodReference,
            accept_interface: true,
        }
    }
}

impl From<SpecialMethodReference> for MethodOperand {
    fn from(reference: SpecialMethodReference) -> Self {
        Self {
            index: reference.index().get(),
            expected: ConstantPoolOperandKind::StaticOrSpecialMethodReference,
            accept_interface: true,
        }
    }
}

impl From<VirtualMethodReference> for MethodOperand {
    fn from(reference: VirtualMethodReference) -> Self {
        Self {
            index: reference.index().get(),
            expected: ConstantPoolOperandKind::VirtualMethodReference,
            accept_interface: false,
        }
    }
}

fn value_kind(field_type: &FieldType) -> ValueKind {
    match field_type {
        FieldType::Boolean
        | FieldType::Byte
        | FieldType::Char
        | FieldType::Int
        | FieldType::Short => ValueKind::Int,
        FieldType::Float => ValueKind::Float,
        FieldType::Long => ValueKind::Long,
        FieldType::Double => ValueKind::Double,
        FieldType::Object(_) | FieldType::Array(_) => ValueKind::Reference,
    }
}

fn is_return(instruction: Instruction) -> bool {
    matches!(
        instruction,
        Instruction::Return | Instruction::IReturn | Instruction::AReturn | Instruction::LReturn
    )
}

/// The local-variable lattice used by integer-only CFG analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowLocalState {
    /// The slot is uninitialized or has incompatible predecessor values.
    Top,
    /// The slot contains an int-like JVM value.
    Integer,
    /// The slot is the current instance receiver.
    Receiver,
}

/// The incoming verifier state for one reachable basic block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockEntryState {
    /// Local slots, including receiver/argument slots and slots grown by stores.
    pub locals: Vec<FlowLocalState>,
    /// Operand stack values in bottom-to-top order. Every value is `Int`.
    pub operand_stack: Vec<ValueKind>,
}

/// Maxima and incoming states produced by fixed-point CFG analysis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowAnalysis {
    /// Maximum operand-stack depth in JVM slots over reachable instructions.
    pub max_stack: u16,
    /// Required local-variable array size in JVM slots.
    pub max_locals: u16,
    block_entries: Vec<Option<BlockEntryState>>,
    initial_state: BlockEntryState,
    sequence_scope: Arc<()>,
    sequence_revision: Arc<()>,
    is_static: bool,
}

impl FlowAnalysis {
    /// Returns an incoming state for a reachable block, or `None` for an
    /// unreachable block or an ID outside this result.
    pub fn entry_state(&self, block: BasicBlockId) -> Option<&BlockEntryState> {
        self.block_entries.get(block.index())?.as_ref()
    }

    /// Returns states indexed by source-ordered block ID. Unreachable blocks
    /// have no state.
    pub fn block_entries(&self) -> &[Option<BlockEntryState>] {
        &self.block_entries
    }

    pub(crate) fn initial_state(&self) -> &BlockEntryState {
        &self.initial_state
    }

    pub(crate) fn shares_sequence_scope(&self, scope: &Arc<()>) -> bool {
        Arc::ptr_eq(&self.sequence_scope, scope)
    }

    pub(crate) fn shares_sequence_revision(&self, revision: &Arc<()>) -> bool {
        Arc::ptr_eq(&self.sequence_revision, revision)
    }

    pub(crate) const fn is_static(&self) -> bool {
        self.is_static
    }
}

/// Fixed-point stack and local analysis for integer-only CFGs.
pub struct CfgCodeAnalyzer;

impl CfgCodeAnalyzer {
    pub(crate) fn validate_all_instructions(graph: &ControlFlowGraph) -> Result<(), AnalysisError> {
        for block in graph.blocks() {
            for (offset, element) in block.instructions.iter().enumerate() {
                let instruction_index = block.instruction_range.start + offset;
                match element {
                    CodeElement::Instruction(instruction) => {
                        validate_flow_instruction(instruction_index, *instruction)?;
                    }
                    CodeElement::Branch(_) => {}
                    CodeElement::Label(_) => {
                        return Err(AnalysisError::UnsupportedFlowInstruction {
                            instruction: instruction_index,
                            kind: UnsupportedFlowInstructionKind::Operation,
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// Analyzes reachable instructions in a control-flow graph.
    ///
    /// Int-like parameters initialize integer locals. A receiver in an
    /// instance method is retained as `Top`, because reference operations are
    /// outside this analyzer's scope. `if*` branches consume their comparison
    /// operands, `goto` preserves the stack, and predecessor states are merged
    /// until stable. Locals merge to `Top`; operand-stack heights must agree.
    /// Unreachable blocks are retained by the CFG but have no entry state.
    /// Each local can only move from `Integer` to `Top`, so the worklist has a
    /// finite bound based on the graph size and local-state width. An
    /// incompatible stack state fails explicitly instead of growing a cycle.
    ///
    /// The analyzer supports integer constants, integer local loads/stores,
    /// `iadd`, `dup`, integer comparisons, gotos, and `return`/`ireturn`.
    /// Reference, category-two, floating-point, exception, and switch flows
    /// are not supported. No byte offsets or stack-map frames are produced.
    pub fn analyze(
        graph: &ControlFlowGraph,
        method_descriptor: &str,
        is_static: bool,
        constant_pool: &ConstantPool,
    ) -> Result<FlowAnalysis, AnalysisError> {
        constant_pool
            .validate_for_encoding()
            .map_err(AnalysisError::InvalidConstantPool)?;
        Self::analyze_validated(graph, method_descriptor, is_static, constant_pool)
    }

    /// Analyzes against a constant pool whose complete layout was already
    /// validated. Method descriptors and instruction operands are still
    /// checked.
    pub(crate) fn analyze_validated(
        graph: &ControlFlowGraph,
        method_descriptor: &str,
        is_static: bool,
        constant_pool: &ConstantPool,
    ) -> Result<FlowAnalysis, AnalysisError> {
        let descriptor = MethodDescriptor::parse(method_descriptor)
            .map_err(AnalysisError::InvalidMethodDescriptor)?;
        check_parameter_slots(&descriptor, !is_static, None)?;
        for (parameter, ty) in descriptor.parameters.iter().enumerate() {
            if value_kind(ty) != ValueKind::Int {
                return Err(AnalysisError::UnsupportedFlowParameter {
                    parameter,
                    actual: value_kind(ty),
                });
            }
        }

        let mut initial = Frame::new(&descriptor, is_static)?;
        let mut local_count = initial.locals.len();
        for block in graph.blocks() {
            for (offset, element) in block.instructions.iter().enumerate() {
                let instruction_index = block.instruction_range.start + offset;
                match element {
                    CodeElement::Instruction(instruction) => {
                        if block.reachable {
                            validate_flow_instruction(instruction_index, *instruction)?;
                        }
                        if let Some((index, width)) = local_access(*instruction) {
                            let end = usize::from(index.get()) + usize::from(width);
                            if end > usize::from(u16::MAX) {
                                return Err(AnalysisError::LocalLimitExceeded {
                                    instruction: instruction_index,
                                    index: index.get(),
                                    width,
                                });
                            }
                            local_count = local_count.max(end);
                        }
                    }
                    CodeElement::Branch(_) => {}
                    CodeElement::Label(_) => {
                        return Err(AnalysisError::UnsupportedFlowInstruction {
                            instruction: instruction_index,
                            kind: UnsupportedFlowInstructionKind::Operation,
                        });
                    }
                }
            }
        }
        initial.locals.resize(local_count, None);
        let initial_state = state_from_frame(&initial);

        let block_count = graph.blocks().len();
        let initial_state_slots = initial_state
            .locals
            .len()
            .saturating_add(initial_state.operand_stack.len());
        if block_count > MAX_FLOW_STATE_SLOTS || initial_state_slots > MAX_FLOW_STATE_SLOTS {
            return Err(AnalysisError::FlowStateSizeLimitExceeded {
                slots: initial_state_slots.max(block_count),
                limit: MAX_FLOW_STATE_SLOTS,
            });
        }
        let max_steps = block_count
            .saturating_mul(local_count.saturating_add(1))
            .max(1);
        let mut block_entries = vec![None; block_count];
        let entry = graph.entry_block();
        let Some(entry_state) = block_entries.get_mut(entry.index()) else {
            return Err(AnalysisError::InvalidControlFlowGraph { block: entry });
        };
        *entry_state = Some(initial_state.clone());
        let mut retained_state_slots = initial_state_slots;

        let mut pending = std::collections::VecDeque::from([entry]);
        let mut queued = vec![false; block_count];
        queued[entry.index()] = true;
        let mut steps = 0usize;
        let mut max_stack = 0usize;

        while let Some(block_id) = pending.pop_front() {
            queued[block_id.index()] = false;
            steps = steps.saturating_add(1);
            if steps > max_steps {
                return Err(AnalysisError::FlowAnalysisLimitExceeded {
                    steps,
                    limit: max_steps,
                });
            }
            let block = graph
                .block(block_id)
                .ok_or(AnalysisError::InvalidControlFlowGraph { block: block_id })?;
            let state = block_entries
                .get(block_id.index())
                .and_then(Option::as_ref)
                .ok_or(AnalysisError::InvalidControlFlowGraph { block: block_id })?
                .clone();
            let mut frame = frame_from_state(&state);

            for (offset, element) in block.instructions.iter().enumerate() {
                let instruction_index = block.instruction_range.start + offset;
                match element {
                    CodeElement::Instruction(instruction) => {
                        frame.apply(instruction_index, *instruction, &descriptor, constant_pool)?
                    }
                    CodeElement::Branch(branch) => {
                        apply_branch(&mut frame, instruction_index, branch)?;
                    }
                    CodeElement::Label(_) => {
                        return Err(AnalysisError::UnsupportedFlowInstruction {
                            instruction: instruction_index,
                            kind: UnsupportedFlowInstructionKind::Operation,
                        });
                    }
                }
            }
            max_stack = max_stack.max(frame.max_stack);
            let outgoing = state_from_frame(&frame);

            for successor in &block.successors {
                let Some(slot) = block_entries.get_mut(successor.index()) else {
                    return Err(AnalysisError::InvalidControlFlowGraph { block: *successor });
                };
                let changed = match slot {
                    Some(existing) => merge_flow_states(existing, &outgoing, *successor)?,
                    None => {
                        let additional_slots = outgoing
                            .locals
                            .len()
                            .saturating_add(outgoing.operand_stack.len());
                        let required_slots = retained_state_slots.saturating_add(additional_slots);
                        if required_slots > MAX_FLOW_STATE_SLOTS {
                            return Err(AnalysisError::FlowStateSizeLimitExceeded {
                                slots: required_slots,
                                limit: MAX_FLOW_STATE_SLOTS,
                            });
                        }
                        retained_state_slots = required_slots;
                        *slot = Some(outgoing.clone());
                        true
                    }
                };
                if changed && !queued[successor.index()] {
                    pending.push_back(*successor);
                    queued[successor.index()] = true;
                }
            }
        }

        let max_stack =
            u16::try_from(max_stack).map_err(|_| AnalysisError::StackLimitExceeded {
                instruction: 0,
                slots: max_stack,
            })?;
        let max_locals =
            u16::try_from(local_count).map_err(|_| AnalysisError::LocalLimitExceeded {
                instruction: 0,
                index: u16::MAX,
                width: 1,
            })?;
        Ok(FlowAnalysis {
            max_stack,
            max_locals,
            block_entries,
            initial_state,
            sequence_scope: Arc::clone(graph.sequence_scope()),
            sequence_revision: Arc::clone(graph.sequence_revision()),
            is_static,
        })
    }
}

fn validate_flow_instruction(
    instruction_index: usize,
    instruction: Instruction,
) -> Result<(), AnalysisError> {
    let supported = matches!(
        instruction,
        Instruction::IConst(_)
            | Instruction::Bipush(_)
            | Instruction::Sipush(_)
            | Instruction::ILoad(_)
            | Instruction::IStore(_)
            | Instruction::IAdd
            | Instruction::Dup
            | Instruction::Pop
            | Instruction::Return
            | Instruction::IReturn
            | Instruction::Ldc(LdcConstant::Integer(_))
    );
    if supported {
        return Ok(());
    }
    let kind = match instruction {
        Instruction::AConstNull
        | Instruction::ALoad(_)
        | Instruction::AStore(_)
        | Instruction::GetField(_)
        | Instruction::PutField(_)
        | Instruction::GetStatic(_)
        | Instruction::PutStatic(_)
        | Instruction::InvokeStatic(_)
        | Instruction::InvokeVirtual(_)
        | Instruction::InvokeSpecial(_)
        | Instruction::New(_)
        | Instruction::Ldc(LdcConstant::Reference(_))
        | Instruction::AReturn => UnsupportedFlowInstructionKind::Reference,
        Instruction::LConst(_)
        | Instruction::Ldc2(_)
        | Instruction::LLoad(_)
        | Instruction::LStore(_)
        | Instruction::LReturn => UnsupportedFlowInstructionKind::CategoryTwo,
        _ => UnsupportedFlowInstructionKind::Operation,
    };
    Err(AnalysisError::UnsupportedFlowInstruction {
        instruction: instruction_index,
        kind,
    })
}

fn local_access(instruction: Instruction) -> Option<(LocalIndex, u8)> {
    match instruction {
        Instruction::ILoad(index)
        | Instruction::IStore(index)
        | Instruction::ALoad(index)
        | Instruction::AStore(index) => Some((index, 1)),
        Instruction::LLoad(index) | Instruction::LStore(index) => Some((index, 2)),
        _ => None,
    }
}

fn apply_branch(
    frame: &mut Frame,
    instruction: usize,
    branch: &BranchInstruction,
) -> Result<(), AnalysisError> {
    match branch {
        BranchInstruction::Goto(_) | BranchInstruction::GotoWide(_) => Ok(()),
        BranchInstruction::IfEq(_) | BranchInstruction::IfNe(_) => {
            frame.pop_expected(instruction, ValueKind::Int).map(|_| ())
        }
        BranchInstruction::IfICmpEq(_)
        | BranchInstruction::IfICmpNe(_)
        | BranchInstruction::IfICmpLt(_)
        | BranchInstruction::IfICmpGe(_)
        | BranchInstruction::IfICmpGt(_)
        | BranchInstruction::IfICmpLe(_) => {
            frame.pop_expected(instruction, ValueKind::Int)?;
            frame.pop_expected(instruction, ValueKind::Int)?;
            Ok(())
        }
    }
}

fn state_from_frame(frame: &Frame) -> BlockEntryState {
    let locals = frame
        .locals
        .iter()
        .map(|slot| match slot {
            Some(LocalSlot::Value(ValueKind::Int)) => FlowLocalState::Integer,
            Some(LocalSlot::Receiver) => FlowLocalState::Receiver,
            _ => FlowLocalState::Top,
        })
        .collect();
    BlockEntryState {
        locals,
        operand_stack: frame.stack.clone(),
    }
}

fn frame_from_state(state: &BlockEntryState) -> Frame {
    let locals = state
        .locals
        .iter()
        .map(|local| match local {
            FlowLocalState::Top => None,
            FlowLocalState::Integer => Some(LocalSlot::Value(ValueKind::Int)),
            FlowLocalState::Receiver => Some(LocalSlot::Receiver),
        })
        .collect();
    let stack_slots = state.operand_stack.len();
    Frame {
        stack: state.operand_stack.clone(),
        stack_slots,
        locals,
        max_stack: stack_slots,
    }
}

fn merge_flow_states(
    existing: &mut BlockEntryState,
    incoming: &BlockEntryState,
    block: BasicBlockId,
) -> Result<bool, AnalysisError> {
    if existing.operand_stack != incoming.operand_stack {
        return Err(AnalysisError::IncompatibleStackMerge {
            block,
            expected_values: existing.operand_stack.len(),
            incoming_values: incoming.operand_stack.len(),
        });
    }
    let mut changed = false;
    for (existing, incoming) in existing.locals.iter_mut().zip(&incoming.locals) {
        if *existing != *incoming && *existing != FlowLocalState::Top {
            *existing = FlowLocalState::Top;
            changed = true;
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::{
        ConstantPoolIndexOperand, FieldReference, SmallIntConstant, SmallLongConstant,
    };
    use crate::constant_pool::ConstantPool;
    use crate::constant_pool_builder::ConstantPoolBuilder;

    fn operand(index: ConstantPoolIndex) -> ConstantPoolIndexOperand {
        ConstantPoolIndexOperand::try_from(index).unwrap()
    }

    fn method_descriptor(parameters: &str) -> String {
        format!("({parameters})V")
    }

    #[test]
    fn enforces_the_jvm_method_parameter_slot_limit() {
        let pool = ConstantPool::default();
        let void_return = [Instruction::Return];

        let static_boundary = method_descriptor(&format!("{}I", "J".repeat(127)));
        assert!(analyze(&static_boundary, true, &void_return, &pool).is_ok());

        let instance_boundary = method_descriptor(&"J".repeat(127));
        assert!(analyze(&instance_boundary, false, &void_return, &pool).is_ok());

        let static_overflow = method_descriptor(&"I".repeat(256));
        assert_eq!(
            analyze(&static_overflow, true, &void_return, &pool),
            Err(AnalysisError::TooManyMethodParameterSlots {
                instruction: None,
                slots: 256,
            })
        );

        let receiver_overflow = method_descriptor(&"I".repeat(255));
        assert_eq!(
            analyze(&receiver_overflow, false, &void_return, &pool),
            Err(AnalysisError::TooManyMethodParameterSlots {
                instruction: None,
                slots: 256,
            })
        );

        let category_two_overflow = method_descriptor(&"J".repeat(128));
        assert_eq!(
            analyze(&category_two_overflow, true, &void_return, &pool),
            Err(AnalysisError::TooManyMethodParameterSlots {
                instruction: None,
                slots: 256,
            })
        );
    }

    #[test]
    fn enforces_parameter_slot_limit_on_invoked_methods() {
        let mut builder = ConstantPoolBuilder::new();
        let static_method = builder
            .method_ref("example/C", "wide", &method_descriptor(&"I".repeat(256)))
            .unwrap();
        let instance_method = builder
            .method_ref(
                "example/C",
                "wideInstance",
                &method_descriptor(&"I".repeat(255)),
            )
            .unwrap();
        let pool = builder.finish().unwrap();

        assert_eq!(
            analyze(
                "()V",
                true,
                &[
                    Instruction::InvokeStatic(StaticMethodReference::new(operand(static_method))),
                    Instruction::Return,
                ],
                &pool,
            ),
            Err(AnalysisError::TooManyMethodParameterSlots {
                instruction: Some(0),
                slots: 256,
            })
        );
        assert_eq!(
            analyze(
                "()V",
                true,
                &[
                    Instruction::InvokeVirtual(VirtualMethodReference::new(operand(
                        instance_method
                    ))),
                    Instruction::Return,
                ],
                &pool,
            ),
            Err(AnalysisError::TooManyMethodParameterSlots {
                instruction: Some(0),
                slots: 256,
            })
        );
    }

    #[test]
    fn analyzes_static_parameters_integer_arithmetic_and_return() {
        let pool = ConstantPool::default();
        let instructions = [
            Instruction::ILoad(LocalIndex::new(0)),
            Instruction::ILoad(LocalIndex::new(1)),
            Instruction::IAdd,
            Instruction::IReturn,
        ];
        let analysis = analyze("(II)I", true, &instructions, &pool).unwrap();
        assert_eq!(
            analysis,
            CodeAnalysis {
                max_stack: 2,
                max_locals: 2,
            }
        );
        assert_eq!(analyze("(II)I", true, &instructions, &pool), Ok(analysis));
    }

    #[test]
    fn counts_receiver_and_mixed_width_parameters_in_local_slots() {
        let pool = ConstantPool::default();
        let constructor = analyze(
            "(I)V",
            false,
            &[
                Instruction::ILoad(LocalIndex::new(1)),
                Instruction::IStore(LocalIndex::new(1)),
                Instruction::Return,
            ],
            &pool,
        )
        .unwrap();
        assert_eq!(constructor.max_stack, 1);
        assert_eq!(constructor.max_locals, 2);

        let instance = analyze(
            "()Ljava/lang/Object;",
            false,
            &[Instruction::ALoad(LocalIndex::new(0)), Instruction::AReturn],
            &pool,
        )
        .unwrap();
        assert_eq!(instance.max_stack, 1);
        assert_eq!(instance.max_locals, 1);

        let mixed = analyze(
            "(JDI)J",
            true,
            &[Instruction::LLoad(LocalIndex::new(0)), Instruction::LReturn],
            &pool,
        )
        .unwrap();
        assert_eq!(mixed.max_stack, 2);
        assert_eq!(mixed.max_locals, 5);
    }

    #[test]
    fn validates_constant_categories_and_reference_local_stores() {
        let mut builder = ConstantPoolBuilder::new();
        let integer = builder.integer(9).unwrap();
        let string = builder.string("value").unwrap();
        let long = builder.long(12).unwrap();
        let pool = builder.finish().unwrap();

        assert_eq!(
            analyze(
                "()I",
                true,
                &[
                    Instruction::Ldc(LdcConstant::Integer(operand(integer))),
                    Instruction::IReturn,
                ],
                &pool,
            )
            .unwrap()
            .max_stack,
            1
        );
        assert_eq!(
            analyze(
                "()Ljava/lang/String;",
                true,
                &[
                    Instruction::Ldc(LdcConstant::Reference(operand(string))),
                    Instruction::AReturn,
                ],
                &pool,
            )
            .unwrap()
            .max_stack,
            1
        );
        assert_eq!(
            analyze(
                "()J",
                true,
                &[
                    Instruction::Ldc2(Ldc2Constant::Long(operand(long))),
                    Instruction::LReturn,
                ],
                &pool,
            )
            .unwrap()
            .max_stack,
            2
        );

        let reference_local = analyze(
            "()Ljava/lang/Object;",
            true,
            &[
                Instruction::AConstNull,
                Instruction::AStore(LocalIndex::new(2)),
                Instruction::ALoad(LocalIndex::new(2)),
                Instruction::AReturn,
            ],
            &pool,
        )
        .unwrap();
        assert_eq!(reference_local.max_stack, 1);
        assert_eq!(reference_local.max_locals, 3);
        assert_eq!(
            analyze(
                "()I",
                true,
                &[
                    Instruction::Ldc(LdcConstant::Integer(operand(string))),
                    Instruction::IReturn,
                ],
                &pool,
            ),
            Err(AnalysisError::InvalidConstantPoolOperand {
                instruction: 0,
                index: string,
                expected: ConstantPoolOperandKind::IntegerConstant,
            })
        );
    }

    #[test]
    fn field_and_invocation_effects_use_constant_pool_descriptors() {
        let mut builder = ConstantPoolBuilder::new();
        let double_field = builder.field_ref("example/C", "ratio", "D").unwrap();
        let long_field = builder.field_ref("example/C", "count", "J").unwrap();
        let static_method = builder.method_ref("example/C", "combine", "(ID)J").unwrap();
        let virtual_method = builder
            .method_ref(
                "example/C",
                "name",
                "(JLjava/lang/String;)Ljava/lang/Object;",
            )
            .unwrap();
        let interface_method = builder
            .interface_method_ref("example/Api", "size", "()I")
            .unwrap();
        let pool = builder.finish().unwrap();

        let static_call = analyze(
            "()J",
            true,
            &[
                Instruction::IConst(SmallIntConstant::One),
                Instruction::GetStatic(FieldReference::new(operand(double_field))),
                Instruction::InvokeStatic(StaticMethodReference::new(operand(static_method))),
                Instruction::LReturn,
            ],
            &pool,
        )
        .unwrap();
        assert_eq!(static_call.max_stack, 3);
        assert_eq!(static_call.max_locals, 0);

        let instance_call = analyze(
            "()Ljava/lang/Object;",
            true,
            &[
                Instruction::AConstNull,
                Instruction::LConst(SmallLongConstant::One),
                Instruction::AConstNull,
                Instruction::InvokeVirtual(VirtualMethodReference::new(operand(virtual_method))),
                Instruction::AReturn,
            ],
            &pool,
        )
        .unwrap();
        assert_eq!(instance_call.max_stack, 4);

        let field_write = analyze(
            "()V",
            true,
            &[
                Instruction::AConstNull,
                Instruction::LConst(SmallLongConstant::Zero),
                Instruction::PutField(FieldReference::new(operand(long_field))),
                Instruction::Return,
            ],
            &pool,
        )
        .unwrap();
        assert_eq!(field_write.max_stack, 3);

        let static_field_write = analyze(
            "()V",
            true,
            &[
                Instruction::LConst(SmallLongConstant::One),
                Instruction::PutStatic(FieldReference::new(operand(long_field))),
                Instruction::Return,
            ],
            &pool,
        )
        .unwrap();
        assert_eq!(static_field_write.max_stack, 2);

        let interface_call = analyze(
            "()I",
            true,
            &[
                Instruction::AConstNull,
                Instruction::InvokeSpecial(SpecialMethodReference::new(operand(interface_method))),
                Instruction::IReturn,
            ],
            &pool,
        )
        .unwrap();
        assert_eq!(interface_call.max_stack, 1);
    }

    #[test]
    fn analyzes_new_dup_constructor_field_and_reference_return() {
        let mut builder = ConstantPoolBuilder::new();
        let class = builder.class("example/C").unwrap();
        let constructor = builder.method_ref("example/C", "<init>", "()V").unwrap();
        let object_field = builder
            .field_ref("example/C", "child", "[Ljava/lang/Object;")
            .unwrap();
        let pool = builder.finish().unwrap();

        let analysis = analyze(
            "()Ljava/lang/Object;",
            true,
            &[
                Instruction::New(ClassReference::new(operand(class))),
                Instruction::Dup,
                Instruction::InvokeSpecial(SpecialMethodReference::new(operand(constructor))),
                Instruction::GetField(FieldReference::new(operand(object_field))),
                Instruction::AReturn,
            ],
            &pool,
        )
        .unwrap();
        assert_eq!(analysis.max_stack, 2);
        assert_eq!(analysis.max_locals, 0);
    }

    #[test]
    fn rejects_stack_underflow_type_mismatch_and_category_two_dup() {
        let pool = ConstantPool::default();
        assert_eq!(
            analyze(
                "()I",
                true,
                &[Instruction::IAdd, Instruction::IReturn],
                &pool
            ),
            Err(AnalysisError::StackUnderflow {
                instruction: 0,
                needed: 1,
                available: 0,
            })
        );
        assert_eq!(
            analyze(
                "()I",
                true,
                &[
                    Instruction::LConst(SmallLongConstant::Zero),
                    Instruction::IAdd,
                    Instruction::IReturn,
                ],
                &pool,
            ),
            Err(AnalysisError::StackTypeMismatch {
                instruction: 1,
                expected: ValueKind::Int,
                actual: ValueKind::Long,
            })
        );
        assert_eq!(
            analyze(
                "()J",
                true,
                &[
                    Instruction::LConst(SmallLongConstant::Zero),
                    Instruction::Dup,
                    Instruction::LReturn,
                ],
                &pool,
            ),
            Err(AnalysisError::DupCategoryTwo {
                instruction: 1,
                actual: ValueKind::Long,
            })
        );
        assert_eq!(
            analyze("()V", true, &[Instruction::Pop, Instruction::Return], &pool),
            Err(AnalysisError::StackUnderflow {
                instruction: 0,
                needed: 1,
                available: 0,
            })
        );
        assert_eq!(
            analyze(
                "()J",
                true,
                &[
                    Instruction::LConst(SmallLongConstant::Zero),
                    Instruction::Pop,
                    Instruction::LReturn,
                ],
                &pool,
            ),
            Err(AnalysisError::PopCategoryTwo {
                instruction: 1,
                actual: ValueKind::Long,
            })
        );
        assert_eq!(
            analyze(
                "()I",
                true,
                &[
                    Instruction::IConst(SmallIntConstant::One),
                    Instruction::Pop,
                    Instruction::IReturn,
                ],
                &pool,
            ),
            Err(AnalysisError::StackUnderflow {
                instruction: 2,
                needed: 1,
                available: 0,
            })
        );
    }

    #[test]
    fn pop_discards_category_one_ints_and_references_without_changing_max_stack() {
        let pool = ConstantPool::default();
        assert_eq!(
            analyze(
                "()V",
                true,
                &[
                    Instruction::IConst(SmallIntConstant::One),
                    Instruction::Pop,
                    Instruction::AConstNull,
                    Instruction::Pop,
                    Instruction::Return,
                ],
                &pool,
            ),
            Ok(CodeAnalysis {
                max_stack: 1,
                max_locals: 0,
            })
        );
    }

    #[test]
    fn tracks_stack_slots_for_mixed_category_values() {
        let pool = ConstantPool::default();
        let analysis = analyze(
            "()J",
            true,
            &[
                Instruction::LConst(SmallLongConstant::Zero),
                Instruction::IConst(SmallIntConstant::One),
                Instruction::IStore(LocalIndex::new(0)),
                Instruction::LReturn,
            ],
            &pool,
        )
        .unwrap();
        assert_eq!(analysis.max_stack, 3);
        assert_eq!(analysis.max_locals, 1);
    }

    #[test]
    fn analyzes_a_near_maximum_length_code_sequence_with_a_deep_stack() {
        let values = 32_767;
        let mut instructions = Vec::with_capacity(values * 2);
        instructions.extend((0..values).map(|index| {
            if index % 2 == 0 {
                Instruction::IConst(SmallIntConstant::Zero)
            } else {
                Instruction::IConst(SmallIntConstant::One)
            }
        }));
        instructions.extend((1..values).map(|_| Instruction::IAdd));
        instructions.push(Instruction::IReturn);

        assert_eq!(instructions.len(), 65_534);
        let analysis = analyze("()I", true, &instructions, &ConstantPool::default()).unwrap();
        assert_eq!(analysis.max_stack, values as u16);
        assert_eq!(analysis.max_locals, 0);
    }

    #[test]
    fn checks_return_opcode_terminal_position_and_empty_stack() {
        let pool = ConstantPool::default();
        assert_eq!(
            analyze("()J", true, &[Instruction::IReturn], &pool),
            Err(AnalysisError::WrongReturnOpcode {
                instruction: 0,
                expected: Some(ValueKind::Long),
                actual: Some(ValueKind::Int),
            })
        );
        assert_eq!(
            analyze(
                "()V",
                true,
                &[
                    Instruction::IConst(SmallIntConstant::One),
                    Instruction::Return
                ],
                &pool
            ),
            Err(AnalysisError::NonEmptyStackAtReturn {
                instruction: 1,
                slots: 1,
            })
        );
        assert_eq!(
            analyze(
                "()V",
                true,
                &[Instruction::Return, Instruction::Return],
                &pool
            ),
            Err(AnalysisError::InstructionAfterReturn { instruction: 1 })
        );
        assert_eq!(
            analyze("()V", true, &[], &pool),
            Err(AnalysisError::MissingReturn)
        );
    }

    #[test]
    fn validates_local_initialization_types_widths_and_u16_limits() {
        let pool = ConstantPool::default();
        assert_eq!(
            analyze(
                "()I",
                true,
                &[Instruction::ILoad(LocalIndex::new(0)), Instruction::IReturn],
                &pool
            ),
            Err(AnalysisError::UninitializedLocal {
                instruction: 0,
                index: 0,
            })
        );
        assert_eq!(
            analyze(
                "()V",
                true,
                &[
                    Instruction::IConst(SmallIntConstant::One),
                    Instruction::IStore(LocalIndex::new(0)),
                    Instruction::LLoad(LocalIndex::new(0)),
                    Instruction::Return,
                ],
                &pool,
            ),
            Err(AnalysisError::LocalTypeMismatch {
                instruction: 2,
                index: 0,
                expected: ValueKind::Long,
                actual: ValueKind::Int,
            })
        );
        assert_eq!(
            analyze(
                "()V",
                true,
                &[
                    Instruction::LConst(SmallLongConstant::Zero),
                    Instruction::LStore(LocalIndex::new(0)),
                    Instruction::ILoad(LocalIndex::new(1)),
                    Instruction::Return,
                ],
                &pool,
            ),
            Err(AnalysisError::LocalSlotOverlap {
                instruction: 2,
                index: 1,
            })
        );

        let cat1_max = analyze(
            "()I",
            true,
            &[
                Instruction::IConst(SmallIntConstant::One),
                Instruction::IStore(LocalIndex::new(65_534)),
                Instruction::ILoad(LocalIndex::new(65_534)),
                Instruction::IReturn,
            ],
            &pool,
        )
        .unwrap();
        assert_eq!(cat1_max.max_locals, u16::MAX);

        assert_eq!(
            analyze(
                "()I",
                true,
                &[
                    Instruction::IConst(SmallIntConstant::One),
                    Instruction::IStore(LocalIndex::new(u16::MAX)),
                    Instruction::ILoad(LocalIndex::new(u16::MAX)),
                    Instruction::IReturn,
                ],
                &pool,
            ),
            Err(AnalysisError::LocalLimitExceeded {
                instruction: 1,
                index: u16::MAX,
                width: 1,
            })
        );

        assert_eq!(
            analyze(
                "()J",
                true,
                &[
                    Instruction::LConst(SmallLongConstant::One),
                    Instruction::LStore(LocalIndex::new(65_534)),
                    Instruction::LLoad(LocalIndex::new(65_534)),
                    Instruction::LReturn,
                ],
                &pool,
            ),
            Err(AnalysisError::LocalLimitExceeded {
                instruction: 1,
                index: 65_534,
                width: 2,
            })
        );
    }

    #[test]
    fn rejects_invalid_member_tags_and_descriptors() {
        let mut builder = ConstantPoolBuilder::new();
        let integer = builder.integer(5).unwrap();
        let bad_field = builder
            .field_ref("example/C", "bad", "not-a-field")
            .unwrap();
        let bad_method = builder.method_ref("example/C", "bad", "(I").unwrap();
        let interface_method = builder
            .interface_method_ref("example/Api", "run", "()V")
            .unwrap();
        let pool = builder.finish().unwrap();
        let before = pool.clone();
        assert_eq!(
            analyze(
                "()I",
                true,
                &[
                    Instruction::New(ClassReference::new(operand(integer))),
                    Instruction::IReturn,
                ],
                &pool,
            ),
            Err(AnalysisError::InvalidConstantPoolOperand {
                instruction: 0,
                index: integer,
                expected: ConstantPoolOperandKind::Class,
            })
        );
        assert!(matches!(
            analyze(
                "()I",
                true,
                &[
                    Instruction::GetStatic(FieldReference::new(operand(bad_field))),
                    Instruction::IReturn,
                ],
                &pool,
            ),
            Err(AnalysisError::InvalidFieldDescriptor { index, .. }) if index == bad_field
        ));
        assert!(matches!(
            analyze(
                "()V",
                true,
                &[
                    Instruction::InvokeStatic(StaticMethodReference::new(operand(bad_method))),
                    Instruction::Return,
                ],
                &pool,
            ),
            Err(AnalysisError::InvalidReferencedMethodDescriptor { index, .. }) if index == bad_method
        ));
        assert_eq!(
            analyze(
                "()V",
                true,
                &[
                    Instruction::AConstNull,
                    Instruction::InvokeVirtual(VirtualMethodReference::new(operand(
                        interface_method
                    ))),
                    Instruction::Return,
                ],
                &pool,
            ),
            Err(AnalysisError::InvalidConstantPoolOperand {
                instruction: 1,
                index: interface_method,
                expected: ConstantPoolOperandKind::VirtualMethodReference,
            })
        );
        assert_eq!(pool, before);
    }

    #[test]
    fn rejects_malformed_method_descriptor_and_stack_overflow() {
        let pool = ConstantPool::default();
        assert!(matches!(
            analyze("(I", true, &[Instruction::Return], &pool),
            Err(AnalysisError::InvalidMethodDescriptor(_))
        ));
        let mut instructions = vec![Instruction::IConst(SmallIntConstant::Zero); 65_536];
        instructions.push(Instruction::IReturn);
        assert!(matches!(
            analyze("()I", true, &instructions, &pool),
            Err(AnalysisError::StackLimitExceeded {
                instruction: 65_535,
                slots: 65_536,
            })
        ));
    }
}
