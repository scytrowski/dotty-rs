//! Descriptor-aware analysis for straight-line code and typed CFGs.
//!
//! [`LinearCodeAnalyzer`] retains the typed straight-line analysis API.
//! [`CfgCodeAnalyzer`] propagates integer and exact initialized-class
//! references across symbolic branches, including bounded field transfers,
//! constructor arguments, primitive virtual calls, and in-block object
//! initialization. Neither analyzer is a full JVM
//! verifier, and neither produces stack-map frames.

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

/// A validated JVM class identity used by CFG reference states.
///
/// Identity is the class-file internal name, not a constant-pool index. This
/// keeps equal names stable across pools while avoiding hierarchy guesses.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClassIdentity(Arc<str>);

impl ClassIdentity {
    /// Returns the validated JVM internal class name.
    pub fn internal_name(&self) -> &str {
        &self.0
    }
}

/// An explicitly supplied, validated JVM superclass proof graph.
///
/// Each class can have one direct superclass. Proof paths are bounded to
/// eight edges; interfaces and array covariance are not modeled.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClassHierarchy {
    superclasses: Arc<std::collections::BTreeMap<ClassIdentity, ClassIdentity>>,
}

/// A malformed or out-of-scope explicit class hierarchy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassHierarchyError {
    /// A class name is not a valid JVM internal name.
    InvalidClassName(String),
    /// The same direct edge was supplied more than once.
    DuplicateEdge {
        subclass: String,
        superclass: String,
    },
    /// One class was assigned two different direct superclasses.
    ConflictingSuperclass { subclass: String },
    /// The supplied graph contains a superclass cycle.
    Cycle { class: String },
    /// A proof path exceeds the supported depth.
    DepthExceeded { class: String, depth: usize },
}

impl fmt::Display for ClassHierarchyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidClassName(name) => write!(formatter, "invalid JVM class name {name:?}"),
            Self::DuplicateEdge {
                subclass,
                superclass,
            } => write!(
                formatter,
                "duplicate superclass edge {subclass} -> {superclass}"
            ),
            Self::ConflictingSuperclass { subclass } => {
                write!(formatter, "class {subclass} has conflicting superclasses")
            }
            Self::Cycle { class } => write!(formatter, "superclass cycle includes {class}"),
            Self::DepthExceeded { class, depth } => write!(
                formatter,
                "superclass proof from {class} exceeds supported depth ({depth})"
            ),
        }
    }
}

impl std::error::Error for ClassHierarchyError {}

impl ClassHierarchy {
    /// Builds a bounded proof graph from direct `(subclass, superclass)` edges.
    pub fn from_edges(
        edges: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, ClassHierarchyError> {
        let mut superclasses = std::collections::BTreeMap::new();
        for (subclass, superclass) in edges {
            if !is_valid_internal_name(&subclass) {
                return Err(ClassHierarchyError::InvalidClassName(subclass));
            }
            if !is_valid_internal_name(&superclass) {
                return Err(ClassHierarchyError::InvalidClassName(superclass));
            }
            if subclass == superclass {
                return Err(ClassHierarchyError::Cycle { class: subclass });
            }
            let sub = ClassIdentity(Arc::from(subclass.as_str()));
            let sup = ClassIdentity(Arc::from(superclass.as_str()));
            if let Some(previous) = superclasses.get(&sub) {
                if previous == &sup {
                    return Err(ClassHierarchyError::DuplicateEdge {
                        subclass,
                        superclass,
                    });
                }
                return Err(ClassHierarchyError::ConflictingSuperclass { subclass });
            }
            superclasses.insert(sub, sup);
        }
        let hierarchy = Self {
            superclasses: Arc::new(superclasses),
        };
        hierarchy.validate()?;
        Ok(hierarchy)
    }

    /// Returns whether `actual` is exactly `expected` or has a bounded proof
    /// path to it in this graph.
    pub fn is_assignable(&self, actual: &str, expected: &str) -> bool {
        if !is_valid_internal_name(actual) || !is_valid_internal_name(expected) {
            return false;
        }
        if actual == expected {
            return true;
        }
        let mut current = ClassIdentity(Arc::from(actual));
        for _ in 0..8 {
            let Some(parent) = self.superclasses.get(&current) else {
                return false;
            };
            if parent.internal_name() == expected {
                return true;
            }
            current = parent.clone();
        }
        false
    }

    pub(crate) fn with_edge(
        &self,
        subclass: &str,
        superclass: &str,
    ) -> Result<Self, ClassHierarchyError> {
        if let Some(existing) = self.superclasses.get(&ClassIdentity(Arc::from(subclass)))
            && existing.internal_name() == superclass
        {
            return Ok(self.clone());
        }
        let mut edges = self
            .superclasses
            .iter()
            .map(|(sub, sup)| {
                (
                    sub.internal_name().to_owned(),
                    sup.internal_name().to_owned(),
                )
            })
            .collect::<Vec<_>>();
        edges.push((subclass.to_owned(), superclass.to_owned()));
        Self::from_edges(edges)
    }

    fn validate(&self) -> Result<(), ClassHierarchyError> {
        for start in self.superclasses.keys() {
            let mut current = start;
            let mut seen = std::collections::BTreeSet::new();
            let mut depth = 0;
            while let Some(parent) = self.superclasses.get(current) {
                if !seen.insert(current.clone()) {
                    return Err(ClassHierarchyError::Cycle {
                        class: current.internal_name().to_owned(),
                    });
                }
                depth += 1;
                if depth > 8 {
                    return Err(ClassHierarchyError::DepthExceeded {
                        class: start.internal_name().to_owned(),
                        depth,
                    });
                }
                current = parent;
            }
        }
        Ok(())
    }
}

/// A verifier value retained on a CFG operand stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowValue {
    /// An int-like JVM value.
    Integer,
    /// An initialized reference to this exact class.
    Object(ClassIdentity),
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalysisError {
    /// The explicit hierarchy proof graph is malformed or exceeds its bound.
    InvalidClassHierarchy(ClassHierarchyError),
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
    /// A field access has a valid descriptor outside the supported int-like and object subset.
    UnsupportedFlowFieldType {
        /// Zero-based instruction position.
        instruction: usize,
        /// Constant-pool entry that owns the descriptor.
        index: ConstantPoolIndex,
    },
    /// A field access needs an initialized reference with an exact class identity.
    MissingExactReference {
        /// Zero-based instruction position.
        instruction: usize,
    },
    /// The exact receiver class differs from the owner named by the field reference.
    FieldOwnerMismatch {
        /// Zero-based instruction position.
        instruction: usize,
        /// Owner named by the field reference.
        expected: ClassIdentity,
        /// Exact receiver class on the operand stack.
        actual: ClassIdentity,
    },
    /// A reference field value differs from the exact class in its descriptor.
    WrongExactFieldValue {
        /// Zero-based instruction position.
        instruction: usize,
        /// Exact class required by the field descriptor.
        expected: ClassIdentity,
        /// Exact class found on the operand stack.
        actual: ClassIdentity,
    },
    /// A reference field value has no exact initialized class identity.
    MissingExactFieldValue {
        /// Zero-based instruction position.
        instruction: usize,
    },
    /// An object type in a field descriptor has an invalid internal name.
    InvalidReferencedFieldClassName {
        /// Zero-based instruction position.
        instruction: usize,
        /// Constant-pool entry that owns the field descriptor.
        index: ConstantPoolIndex,
        /// Invalid class name from the descriptor.
        name: String,
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
    /// A method reference has an invalid JVM method name or names a special method.
    InvalidReferencedMethodName {
        /// Zero-based instruction position.
        instruction: usize,
        /// Constant-pool entry that owns the name.
        index: ConstantPoolIndex,
    },
    /// An invocation parameter type is outside the supported `I`/`Z` subset.
    UnsupportedFlowMethodParameter {
        /// Zero-based instruction position.
        instruction: usize,
        /// Zero-based parameter position in the method descriptor.
        parameter: usize,
        /// Unsupported parameter type.
        actual: FieldType,
    },
    /// An invocation result type is outside the supported `I`/`Z`/`V` subset.
    UnsupportedFlowMethodReturn {
        /// Zero-based instruction position.
        instruction: usize,
        /// Unsupported result type.
        actual: FieldType,
    },
    /// A reference argument does not match the exact class declared by the method reference.
    WrongExactMethodArgument {
        /// Zero-based instruction position.
        instruction: usize,
        /// Zero-based parameter position in the method descriptor.
        parameter: usize,
        /// Exact class required by the method descriptor.
        expected: ClassIdentity,
        /// Exact class found on the operand stack.
        actual: ClassIdentity,
    },
    /// A reference argument has no exact initialized class identity.
    MissingExactMethodArgument {
        /// Zero-based instruction position.
        instruction: usize,
        /// Zero-based parameter position in the method descriptor.
        parameter: usize,
    },
    /// An object type in a referenced method descriptor has an invalid internal name.
    InvalidReferencedMethodClassName {
        /// Zero-based instruction position.
        instruction: usize,
        /// Constant-pool entry that owns the descriptor.
        index: ConstantPoolIndex,
        /// Zero-based parameter position, or `None` for the result.
        parameter: Option<usize>,
        /// Invalid class name from the descriptor.
        name: String,
    },
    /// The exact receiver class differs from the owner named by the method reference.
    MethodOwnerMismatch {
        /// Zero-based instruction position.
        instruction: usize,
        /// Owner named by the method reference.
        expected: ClassIdentity,
        /// Exact receiver class on the operand stack.
        actual: ClassIdentity,
    },
    /// An uninitialized allocation was used as an ordinary value.
    UninitializedReferenceUse {
        /// Zero-based instruction position.
        instruction: usize,
        /// Allocation instruction that created the reference.
        allocation_site: usize,
    },
    /// A constructor call did not use an uninitialized allocation receiver.
    ConstructorReceiverNotUninitialized {
        /// Zero-based instruction position.
        instruction: usize,
    },
    /// A constructor owner does not match the class created at the allocation site.
    ConstructorOwnerMismatch {
        /// Zero-based instruction position.
        instruction: usize,
        /// Class created by the `new` instruction.
        allocated: ClassIdentity,
        /// Owner named by the constructor method reference.
        actual: ClassIdentity,
    },
    /// A branchless constructor does not start with the validated direct-super call.
    InvalidConstructorSuperCall {
        /// Zero-based instruction position of the invalid prologue instruction.
        instruction: usize,
    },
    /// The direct-super constructor call receives a different number of arguments.
    ConstructorSuperArgumentCountMismatch {
        /// Zero-based instruction position of the `invokespecial` instruction.
        instruction: usize,
        /// Number of arguments required by the direct-super constructor.
        expected: usize,
        /// Number of values supplied before the receiver.
        actual: usize,
    },
    /// A direct-super constructor argument has the wrong JVM value category.
    ConstructorSuperArgumentTypeMismatch {
        /// Zero-based instruction position of the `invokespecial` instruction.
        instruction: usize,
        /// Zero-based parameter position in the constructor descriptor.
        parameter: usize,
        /// Required value category.
        expected: ValueKind,
        /// Actual value category.
        actual: ValueKind,
    },
    /// A direct-super reference argument is not assignable to the exact descriptor class.
    WrongExactConstructorSuperArgument {
        /// Zero-based instruction position of the `invokespecial` instruction.
        instruction: usize,
        /// Zero-based parameter position in the constructor descriptor.
        parameter: usize,
        /// Required class from the constructor descriptor.
        expected: ClassIdentity,
        /// Exact class loaded from the child constructor parameter.
        actual: ClassIdentity,
    },
    /// A constructor argument uses a type outside the supported pre-super subset.
    UnsupportedConstructorSuperArgumentType {
        /// Zero-based instruction position of the `invokespecial` instruction.
        instruction: usize,
        /// Zero-based parameter position in the constructor descriptor.
        parameter: usize,
    },
    /// The bounded direct-super argument sequence is longer than the verifier permits.
    ConstructorSuperPrologueTooLong {
        /// Zero-based instruction position where the bound was exceeded.
        instruction: usize,
        /// Maximum allowed instructions before the `invokespecial` instruction.
        maximum: usize,
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
    /// An `areturn` value does not match the exact class in the method result descriptor.
    WrongExactReferenceReturn {
        /// Zero-based instruction position.
        instruction: usize,
        /// Exact class required by the method descriptor.
        expected: ClassIdentity,
        /// Exact class found on the operand stack.
        actual: ClassIdentity,
    },
    /// An `areturn` value has no exact initialized class identity.
    MissingExactReferenceReturn {
        /// Zero-based instruction position.
        instruction: usize,
        /// Exact class required by the method descriptor.
        expected: ClassIdentity,
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
    /// The CFG analyzer encountered an instruction outside its supported subset.
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
    /// A bounded exact-reference method descriptor contains an unsupported type.
    UnsupportedMethodDescriptorType {
        /// Zero-based parameter position, or `None` for the result.
        parameter: Option<usize>,
        /// Unsupported descriptor type.
        actual: FieldType,
    },
    /// An object type in a method descriptor has an invalid JVM internal name.
    InvalidDescriptorClassName {
        /// Zero-based parameter position, or `None` for the result.
        parameter: Option<usize>,
        /// Invalid class name from the descriptor.
        name: String,
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
    /// Corresponding stack values have incompatible exact verifier types.
    IncompatibleStackValue {
        /// Block receiving incompatible states.
        block: BasicBlockId,
        /// Zero-based value position from the bottom of the stack.
        value: usize,
        /// Type already established for this position.
        expected: FlowValue,
        /// Type from the incoming predecessor.
        incoming: FlowValue,
    },
    /// The initialization context does not match the method kind.
    InvalidInitializationContext {
        /// Whether the analyzed method is static.
        is_static: bool,
        /// Whether the analyzed method is a JVM constructor.
        is_constructor: bool,
    },
    /// A constant-pool class name is not a valid JVM internal name.
    InvalidClassName {
        /// Constant-pool Class entry that contains the invalid name.
        index: ConstantPoolIndex,
    },
    /// A non-integer stack value lacks an exact reference identity.
    UnsupportedFlowStackValue {
        /// The category of the unrepresentable stack value.
        actual: ValueKind,
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

/// Why an instruction is outside the supported CFG analysis subset.
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
            Self::InvalidClassHierarchy(error) => {
                write!(formatter, "invalid class hierarchy: {error}")
            }
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
            Self::UnsupportedFlowFieldType { instruction, index } => write!(
                formatter,
                "instruction {instruction} field at constant-pool index {} has a type outside the supported int/boolean subset",
                index.0
            ),
            Self::MissingExactReference { instruction } => write!(
                formatter,
                "instruction {instruction} requires an initialized reference with an exact class identity"
            ),
            Self::FieldOwnerMismatch {
                instruction,
                expected,
                actual,
            } => write!(
                formatter,
                "instruction {instruction} field owner {} does not match receiver class {}",
                expected.internal_name(),
                actual.internal_name()
            ),
            Self::WrongExactFieldValue {
                instruction,
                expected,
                actual,
            } => write!(
                formatter,
                "instruction {instruction} stores {} in a field requiring {}",
                actual.internal_name(),
                expected.internal_name()
            ),
            Self::MissingExactFieldValue { instruction } => write!(
                formatter,
                "instruction {instruction} stores a reference without an exact class identity"
            ),
            Self::InvalidReferencedFieldClassName {
                instruction,
                index,
                name,
            } => write!(
                formatter,
                "instruction {instruction} field at constant-pool index {} has invalid JVM class name {name:?}",
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
            Self::InvalidReferencedMethodName { instruction, index } => write!(
                formatter,
                "instruction {instruction} method at constant-pool index {} has an invalid or special name",
                index.0
            ),
            Self::UnsupportedFlowMethodParameter {
                instruction,
                parameter,
                actual,
            } => write!(
                formatter,
                "instruction {instruction} method parameter {parameter} has unsupported CFG type {actual:?}"
            ),
            Self::UnsupportedFlowMethodReturn {
                instruction,
                actual,
            } => write!(
                formatter,
                "instruction {instruction} method result has unsupported CFG type {actual:?}"
            ),
            Self::WrongExactMethodArgument {
                instruction,
                parameter,
                expected,
                actual,
            } => write!(
                formatter,
                "instruction {instruction} passes {} as method parameter {parameter}, but the descriptor requires {}",
                actual.internal_name(),
                expected.internal_name()
            ),
            Self::MissingExactMethodArgument {
                instruction,
                parameter,
            } => write!(
                formatter,
                "instruction {instruction} passes a reference without an exact class identity for method parameter {parameter}"
            ),
            Self::InvalidReferencedMethodClassName {
                instruction,
                index,
                parameter,
                name,
            } => {
                if let Some(parameter) = parameter {
                    write!(formatter, "method parameter {parameter}")?;
                } else {
                    formatter.write_str("method result")?;
                }
                write!(
                    formatter,
                    " at instruction {instruction} has invalid JVM class name {name:?} in constant-pool entry {}",
                    index.0
                )
            }
            Self::MethodOwnerMismatch {
                instruction,
                expected,
                actual,
            } => write!(
                formatter,
                "instruction {instruction} method owner {} does not match receiver class {}",
                expected.internal_name(),
                actual.internal_name()
            ),
            Self::UninitializedReferenceUse {
                instruction,
                allocation_site,
            } => write!(
                formatter,
                "instruction {instruction} uses uninitialized allocation from instruction {allocation_site}"
            ),
            Self::ConstructorReceiverNotUninitialized { instruction } => write!(
                formatter,
                "instruction {instruction} constructor receiver is not an uninitialized allocation"
            ),
            Self::ConstructorOwnerMismatch {
                instruction,
                allocated,
                actual,
            } => write!(
                formatter,
                "instruction {instruction} constructor owner {} does not match allocated class {}",
                actual.internal_name(),
                allocated.internal_name()
            ),
            Self::InvalidConstructorSuperCall { instruction } => write!(
                formatter,
                "instruction {instruction} is not the required direct-super constructor call"
            ),
            Self::ConstructorSuperArgumentCountMismatch {
                instruction,
                expected,
                actual,
            } => write!(
                formatter,
                "instruction {instruction} supplies {actual} direct-super constructor arguments, expected {expected}"
            ),
            Self::ConstructorSuperArgumentTypeMismatch {
                instruction,
                parameter,
                expected,
                actual,
            } => write!(
                formatter,
                "instruction {instruction} supplies {actual} for direct-super constructor parameter {parameter}, expected {expected}"
            ),
            Self::WrongExactConstructorSuperArgument {
                instruction,
                parameter,
                expected,
                actual,
            } => write!(
                formatter,
                "instruction {instruction} supplies reference {} for direct-super constructor parameter {parameter}, expected {}",
                actual.internal_name(),
                expected.internal_name()
            ),
            Self::UnsupportedConstructorSuperArgumentType {
                instruction,
                parameter,
            } => write!(
                formatter,
                "instruction {instruction} uses unsupported type for direct-super constructor parameter {parameter}"
            ),
            Self::ConstructorSuperPrologueTooLong {
                instruction,
                maximum,
            } => write!(
                formatter,
                "instruction {instruction} exceeds the direct-super constructor prologue limit of {maximum} instructions"
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
            Self::WrongExactReferenceReturn {
                instruction,
                expected,
                actual,
            } => write!(
                formatter,
                "instruction {instruction} returns {}, but the method descriptor requires {}",
                actual.internal_name(),
                expected.internal_name()
            ),
            Self::MissingExactReferenceReturn {
                instruction,
                expected,
            } => write!(
                formatter,
                "instruction {instruction} returns a reference without an exact class identity required for {}",
                expected.internal_name()
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
            Self::UnsupportedMethodDescriptorType { parameter, actual } => {
                if let Some(parameter) = parameter {
                    write!(formatter, "method parameter {parameter}")?;
                } else {
                    formatter.write_str("method result")?;
                }
                write!(
                    formatter,
                    " has unsupported exact-reference descriptor type {actual:?}"
                )
            }
            Self::InvalidDescriptorClassName { parameter, name } => {
                if let Some(parameter) = parameter {
                    write!(formatter, "method parameter {parameter}")?;
                } else {
                    formatter.write_str("method result")?;
                }
                write!(formatter, " has invalid JVM class name {name:?}")
            }
            Self::IncompatibleStackMerge {
                block,
                expected_values,
                incoming_values,
            } => write!(
                formatter,
                "block {} merges operand stacks with {expected_values} and {incoming_values} values",
                block.index()
            ),
            Self::IncompatibleStackValue {
                block,
                value,
                expected,
                incoming,
            } => write!(
                formatter,
                "block {} merges incompatible stack value {value}: {expected:?} and {incoming:?}",
                block.index()
            ),
            Self::InvalidInitializationContext {
                is_static,
                is_constructor,
            } => write!(
                formatter,
                "{}{} method has an incompatible current-class initialization context",
                if *is_static { "static " } else { "instance " },
                if *is_constructor {
                    "constructor"
                } else {
                    "ordinary"
                }
            ),
            Self::InvalidClassName { index } => write!(
                formatter,
                "constant-pool class at index {} has an invalid internal name",
                index.0
            ),
            Self::UnsupportedFlowStackValue { actual } => write!(
                formatter,
                "CFG operand stack contains {actual} without a supported exact verifier state"
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
    analyze_validated_with_current_class(
        "",
        method_descriptor,
        is_static,
        instructions,
        constant_pool,
        None,
    )
}

pub(crate) fn analyze_validated_with_current_class(
    method_name: &str,
    method_descriptor: &str,
    is_static: bool,
    instructions: &[Instruction],
    constant_pool: &ConstantPool,
    current_class_index: Option<ConstantPoolIndex>,
) -> Result<CodeAnalysis, AnalysisError> {
    analyze_validated_with_current_class_and_hierarchy(
        method_name,
        method_descriptor,
        is_static,
        instructions,
        constant_pool,
        current_class_index,
        &ClassHierarchy::default(),
    )
}

pub(crate) fn analyze_validated_with_current_class_and_hierarchy(
    method_name: &str,
    method_descriptor: &str,
    is_static: bool,
    instructions: &[Instruction],
    constant_pool: &ConstantPool,
    current_class_index: Option<ConstantPoolIndex>,
    hierarchy: &ClassHierarchy,
) -> Result<CodeAnalysis, AnalysisError> {
    analyze_validated_with_current_class_inner(
        method_name,
        method_descriptor,
        is_static,
        instructions,
        constant_pool,
        current_class_index,
        AnalysisContext::method(hierarchy),
    )
}

/// Analyzes a branchless constructor after validating its bounded direct-super
/// prologue. Before the call, only integer constants and loads from initialized
/// constructor parameter locals are accepted.
pub(crate) fn analyze_validated_constructor_body_with_hierarchy(
    method_descriptor: &str,
    instructions: &[Instruction],
    constant_pool: &ConstantPool,
    current_class_index: ConstantPoolIndex,
    super_class_index: ConstantPoolIndex,
    hierarchy: &ClassHierarchy,
) -> Result<CodeAnalysis, AnalysisError> {
    if !matches!(instructions.first(), Some(Instruction::ALoad(index)) if index.get() == 0) {
        return Err(AnalysisError::InvalidConstructorSuperCall { instruction: 0 });
    }
    const MAX_CONSTRUCTOR_PROLOGUE_INSTRUCTIONS: usize = 255;

    let child_descriptor = MethodDescriptor::parse(method_descriptor)
        .map_err(AnalysisError::InvalidMethodDescriptor)?;
    check_parameter_slots(&child_descriptor, true, None)?;
    if child_descriptor.return_type.is_some() {
        return Err(AnalysisError::InvalidConstructorSuperCall { instruction: 0 });
    }
    validate_descriptor_class_names(&child_descriptor)?;
    validate_bounded_reference_descriptor(&child_descriptor)?;

    let mut parameter_locals = vec![ConstructorParameterLocal::UninitializedThis];
    for parameter in &child_descriptor.parameters {
        let local = match parameter {
            FieldType::Boolean
            | FieldType::Byte
            | FieldType::Char
            | FieldType::Short
            | FieldType::Int => ConstructorParameterLocal::Int,
            FieldType::Object(name) => {
                ConstructorParameterLocal::Reference(ClassIdentity(Arc::from(name.as_str())))
            }
            FieldType::Array(_) => ConstructorParameterLocal::Unsupported,
            FieldType::Float | FieldType::Long | FieldType::Double => {
                ConstructorParameterLocal::Unsupported
            }
        };
        parameter_locals.push(local);
        if value_kind(parameter).slots() == 2 {
            parameter_locals.push(ConstructorParameterLocal::Unsupported);
        }
    }

    let empty_descriptor =
        MethodDescriptor::parse("()V").map_err(AnalysisError::InvalidMethodDescriptor)?;
    let resolver = Frame::new(&empty_descriptor, true, None, hierarchy.clone())?;
    let mut prologue_stack = vec![ConstructorPrologueValue::UninitializedThis];
    let mut prologue_max_stack = 1usize;
    let mut super_call = None;
    for (instruction_index, instruction) in instructions.iter().copied().enumerate().skip(1) {
        if let Instruction::InvokeSpecial(reference) = instruction {
            super_call = Some((instruction_index, reference));
            break;
        }
        if instruction_index > MAX_CONSTRUCTOR_PROLOGUE_INSTRUCTIONS {
            return Err(AnalysisError::ConstructorSuperPrologueTooLong {
                instruction: instruction_index,
                maximum: MAX_CONSTRUCTOR_PROLOGUE_INSTRUCTIONS,
            });
        }
        let value = match instruction {
            Instruction::IConst(_) | Instruction::Bipush(_) | Instruction::Sipush(_) => {
                ConstructorPrologueValue::Int
            }
            Instruction::Ldc(constant @ LdcConstant::Integer(_)) => {
                resolver.ldc_kind(instruction_index, constant, constant_pool)?;
                ConstructorPrologueValue::Int
            }
            Instruction::ILoad(index) => match parameter_locals.get(usize::from(index.get())) {
                Some(ConstructorParameterLocal::Int) => ConstructorPrologueValue::Int,
                _ => {
                    return Err(AnalysisError::InvalidConstructorSuperCall {
                        instruction: instruction_index,
                    });
                }
            },
            Instruction::ALoad(index) => match parameter_locals.get(usize::from(index.get())) {
                Some(ConstructorParameterLocal::Reference(class)) => {
                    ConstructorPrologueValue::Reference(class.clone())
                }
                _ => {
                    return Err(AnalysisError::InvalidConstructorSuperCall {
                        instruction: instruction_index,
                    });
                }
            },
            _ => {
                return Err(AnalysisError::InvalidConstructorSuperCall {
                    instruction: instruction_index,
                });
            }
        };
        prologue_stack.push(value);
        prologue_max_stack = prologue_max_stack.max(prologue_stack.len());
    }

    let Some((instruction_index, reference)) = super_call else {
        return Err(AnalysisError::InvalidConstructorSuperCall {
            instruction: instructions.len(),
        });
    };
    let (actual_owner, name, descriptor) =
        resolver.special_method(instruction_index, reference, constant_pool)?;
    let expected_owner =
        class_identity_from_pool(constant_pool, super_class_index, instruction_index)?;
    check_parameter_slots(&descriptor, true, Some(instruction_index))?;
    validate_descriptor_class_names(&descriptor)?;
    validate_bounded_reference_descriptor(&descriptor)?;
    if name != "<init>" || descriptor.return_type.is_some() || actual_owner != expected_owner {
        return Err(AnalysisError::InvalidConstructorSuperCall {
            instruction: instruction_index,
        });
    }

    let supplied_arguments = prologue_stack.len().saturating_sub(1);
    if supplied_arguments != descriptor.parameters.len() {
        return Err(AnalysisError::ConstructorSuperArgumentCountMismatch {
            instruction: instruction_index,
            expected: descriptor.parameters.len(),
            actual: supplied_arguments,
        });
    }
    for (parameter_index, (expected, actual)) in descriptor
        .parameters
        .iter()
        .zip(prologue_stack.iter().skip(1))
        .enumerate()
    {
        match (expected, actual) {
            (
                FieldType::Boolean
                | FieldType::Byte
                | FieldType::Char
                | FieldType::Short
                | FieldType::Int,
                ConstructorPrologueValue::Int,
            ) => {}
            (
                FieldType::Object(expected_name),
                ConstructorPrologueValue::Reference(actual_class),
            ) => {
                let expected_class = ClassIdentity(Arc::from(expected_name.as_str()));
                if !hierarchy.is_assignable(actual_class.internal_name(), expected_name) {
                    return Err(AnalysisError::WrongExactConstructorSuperArgument {
                        instruction: instruction_index,
                        parameter: parameter_index,
                        expected: expected_class,
                        actual: actual_class.clone(),
                    });
                }
            }
            (FieldType::Array(_) | FieldType::Float | FieldType::Long | FieldType::Double, _) => {
                return Err(AnalysisError::UnsupportedConstructorSuperArgumentType {
                    instruction: instruction_index,
                    parameter: parameter_index,
                });
            }
            (expected, actual) => {
                return Err(AnalysisError::ConstructorSuperArgumentTypeMismatch {
                    instruction: instruction_index,
                    parameter: parameter_index,
                    expected: match expected {
                        FieldType::Object(_) | FieldType::Array(_) => ValueKind::Reference,
                        _ => ValueKind::Int,
                    },
                    actual: match actual {
                        ConstructorPrologueValue::Int => ValueKind::Int,
                        ConstructorPrologueValue::Reference(_)
                        | ConstructorPrologueValue::UninitializedThis => ValueKind::Reference,
                    },
                });
            }
        }
    }

    let mut analysis = analyze_validated_with_current_class_inner(
        "<init>",
        method_descriptor,
        false,
        &instructions[instruction_index + 1..],
        constant_pool,
        Some(current_class_index),
        AnalysisContext {
            after_constructor_super_call: true,
            instruction_offset: instruction_index + 1,
            hierarchy,
        },
    )?;
    // The initialized receiver and ordered arguments are all on the stack at
    // the invokespecial site. Account for this full prologue in the method max.
    let prologue_max_stack =
        u16::try_from(prologue_max_stack).map_err(|_| AnalysisError::StackLimitExceeded {
            instruction: instruction_index,
            slots: prologue_max_stack,
        })?;
    analysis.max_stack = analysis.max_stack.max(prologue_max_stack);
    Ok(analysis)
}

#[derive(Clone)]
enum ConstructorParameterLocal {
    UninitializedThis,
    Int,
    Reference(ClassIdentity),
    Unsupported,
}

#[derive(Clone)]
enum ConstructorPrologueValue {
    UninitializedThis,
    Int,
    Reference(ClassIdentity),
}

#[derive(Clone, Copy)]
struct AnalysisContext<'a> {
    after_constructor_super_call: bool,
    instruction_offset: usize,
    hierarchy: &'a ClassHierarchy,
}

impl<'a> AnalysisContext<'a> {
    fn method(hierarchy: &'a ClassHierarchy) -> Self {
        Self {
            after_constructor_super_call: false,
            instruction_offset: 0,
            hierarchy,
        }
    }
}

fn analyze_validated_with_current_class_inner(
    method_name: &str,
    method_descriptor: &str,
    is_static: bool,
    instructions: &[Instruction],
    constant_pool: &ConstantPool,
    current_class_index: Option<ConstantPoolIndex>,
    context: AnalysisContext<'_>,
) -> Result<CodeAnalysis, AnalysisError> {
    let descriptor = MethodDescriptor::parse(method_descriptor)
        .map_err(AnalysisError::InvalidMethodDescriptor)?;
    check_parameter_slots(&descriptor, !is_static, None)?;
    if current_class_index.is_some() {
        validate_descriptor_class_names(&descriptor)?;
        validate_bounded_reference_descriptor(&descriptor)?;
    }
    let current_class = current_class_index
        .map(|index| class_identity_from_pool(constant_pool, index, 0))
        .transpose()?;
    if current_class_index.is_some()
        && (method_name == "<clinit>"
            || (method_name == "<init>" && !context.after_constructor_super_call))
    {
        return Err(AnalysisError::InvalidInitializationContext {
            is_static,
            is_constructor: method_name == "<init>" || method_name == "<clinit>",
        });
    }
    let mut frame = Frame::new(
        &descriptor,
        is_static,
        current_class.as_ref(),
        context.hierarchy.clone(),
    )?;
    let mut returned = false;

    for (offset, instruction) in instructions.iter().copied().enumerate() {
        let instruction_index = context.instruction_offset.saturating_add(offset);
        if returned {
            return Err(AnalysisError::InstructionAfterReturn {
                instruction: instruction_index,
            });
        }
        frame.apply(
            instruction_index,
            instruction,
            &descriptor,
            constant_pool,
            current_class.as_ref(),
        )?;
        returned = is_return(instruction);
    }

    if !returned {
        return Err(AnalysisError::MissingReturn);
    }
    Ok(CodeAnalysis {
        max_stack: u16::try_from(frame.max_stack).map_err(|_| {
            AnalysisError::StackLimitExceeded {
                instruction: context
                    .instruction_offset
                    .saturating_add(instructions.len().saturating_sub(1)),
                slots: frame.max_stack,
            }
        })?,
        max_locals: u16::try_from(frame.locals.len()).map_err(|_| {
            AnalysisError::LocalLimitExceeded {
                instruction: context
                    .instruction_offset
                    .saturating_add(instructions.len().saturating_sub(1)),
                index: u16::MAX,
                width: 1,
            }
        })?,
    })
}

fn validate_descriptor_class_names(descriptor: &MethodDescriptor) -> Result<(), AnalysisError> {
    for (parameter, ty) in descriptor.parameters.iter().enumerate() {
        validate_descriptor_type_name(ty, Some(parameter))?;
    }
    if let Some(result) = &descriptor.return_type {
        validate_descriptor_type_name(result, None)?;
    }
    Ok(())
}

fn validate_descriptor_type_name(
    ty: &FieldType,
    parameter: Option<usize>,
) -> Result<(), AnalysisError> {
    match ty {
        FieldType::Object(name) if !is_valid_internal_name(name) => {
            Err(AnalysisError::InvalidDescriptorClassName {
                parameter,
                name: name.clone(),
            })
        }
        FieldType::Array(element) => validate_descriptor_type_name(element, parameter),
        _ => Ok(()),
    }
}

fn validate_bounded_reference_descriptor(
    descriptor: &MethodDescriptor,
) -> Result<(), AnalysisError> {
    let has_reference = descriptor
        .parameters
        .iter()
        .chain(descriptor.return_type.iter())
        .any(|ty| matches!(ty, FieldType::Object(_) | FieldType::Array(_)));
    if !has_reference {
        return Ok(());
    }

    for (parameter, ty) in descriptor.parameters.iter().enumerate() {
        if !matches!(ty, FieldType::Object(_)) && value_kind(ty) != ValueKind::Int {
            return Err(AnalysisError::UnsupportedMethodDescriptorType {
                parameter: Some(parameter),
                actual: ty.clone(),
            });
        }
    }
    if let Some(result) = &descriptor.return_type
        && !matches!(result, FieldType::Object(_))
        && value_kind(result) != ValueKind::Int
    {
        return Err(AnalysisError::UnsupportedMethodDescriptorType {
            parameter: None,
            actual: result.clone(),
        });
    }
    Ok(())
}

fn class_identity_from_pool(
    pool: &ConstantPool,
    index: ConstantPoolIndex,
    instruction: usize,
) -> Result<ClassIdentity, AnalysisError> {
    let name = pool
        .class_name(index)
        .map_err(|error| AnalysisError::InvalidConstantPoolReference { instruction, error })?;
    if !is_valid_internal_name(name) {
        return Err(AnalysisError::InvalidClassName { index });
    }
    Ok(ClassIdentity(Arc::from(name)))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LocalSlot {
    Value(ValueKind),
    Receiver,
    Object(ClassIdentity),
    CategoryTwoTail,
}

struct Frame {
    stack: Vec<ValueKind>,
    stack_classes: Vec<Option<ClassIdentity>>,
    stack_allocation_sites: Vec<Option<usize>>,
    stack_slots: usize,
    pending_allocation: Option<UninitializedAllocation>,
    locals: Vec<Option<LocalSlot>>,
    max_stack: usize,
    hierarchy: ClassHierarchy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UninitializedAllocation {
    site: usize,
    class: ClassIdentity,
}

impl Frame {
    fn new(
        descriptor: &MethodDescriptor,
        is_static: bool,
        current_class: Option<&ClassIdentity>,
        hierarchy: ClassHierarchy,
    ) -> Result<Self, AnalysisError> {
        let mut locals = Vec::new();
        if !is_static {
            locals.push(Some(
                current_class
                    .cloned()
                    .map(LocalSlot::Object)
                    .unwrap_or(LocalSlot::Receiver),
            ));
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
            locals.push(Some(match (current_class, parameter) {
                (Some(_), FieldType::Object(name)) => {
                    LocalSlot::Object(ClassIdentity(Arc::from(name.as_str())))
                }
                _ => LocalSlot::Value(kind),
            }));
            if width == 2 {
                locals.push(Some(LocalSlot::CategoryTwoTail));
            }
        }
        Ok(Self {
            stack: Vec::new(),
            stack_classes: Vec::new(),
            stack_allocation_sites: Vec::new(),
            stack_slots: 0,
            pending_allocation: None,
            locals,
            max_stack: 0,
            hierarchy,
        })
    }

    fn apply(
        &mut self,
        instruction_index: usize,
        instruction: Instruction,
        method_descriptor: &MethodDescriptor,
        pool: &ConstantPool,
        flow_current_class: Option<&ClassIdentity>,
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
            Instruction::IAdd
            | Instruction::ISub
            | Instruction::IMul
            | Instruction::IDiv
            | Instruction::IRem
            | Instruction::IAnd
            | Instruction::IOr
            | Instruction::IXor => {
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
                let class = self.stack_classes.last().cloned().flatten();
                let allocation_site = self.stack_allocation_sites.last().copied().flatten();
                self.push_typed(instruction_index, top, class)?;
                *self
                    .stack_allocation_sites
                    .last_mut()
                    .expect("push_typed adds an allocation-site slot") = allocation_site;
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
                self.stack_classes.pop();
                self.stack_allocation_sites.pop();
                self.stack_slots -= 1;
            }
            Instruction::GetField(reference) => {
                if flow_current_class.is_some() {
                    self.ensure_no_pending_allocation(instruction_index)?;
                    let (owner, ty) = self.flow_field_type(instruction_index, reference, pool)?;
                    let field_class =
                        validate_flow_field_type(instruction_index, reference.index().get(), &ty)?;
                    self.pop_field_receiver(instruction_index, &owner)?;
                    match field_class {
                        Some(class) => self.push_object(instruction_index, class)?,
                        None => self.push(instruction_index, ValueKind::Int)?,
                    }
                } else {
                    let ty = self.field_type(instruction_index, reference, pool)?;
                    self.pop_expected(instruction_index, ValueKind::Reference)?;
                    self.push(instruction_index, value_kind(&ty))?;
                }
            }
            Instruction::PutField(reference) => {
                if flow_current_class.is_some() {
                    self.ensure_no_pending_allocation(instruction_index)?;
                    let (owner, ty) = self.flow_field_type(instruction_index, reference, pool)?;
                    let field_class =
                        validate_flow_field_type(instruction_index, reference.index().get(), &ty)?;
                    if let Some(expected) = field_class {
                        let (_, actual) =
                            self.pop_typed(instruction_index, ValueKind::Reference)?;
                        let actual = actual.ok_or(AnalysisError::MissingExactFieldValue {
                            instruction: instruction_index,
                        })?;
                        if !self
                            .hierarchy
                            .is_assignable(actual.internal_name(), expected.internal_name())
                        {
                            return Err(AnalysisError::WrongExactFieldValue {
                                instruction: instruction_index,
                                expected,
                                actual,
                            });
                        }
                    } else {
                        self.pop_expected(instruction_index, ValueKind::Int)?;
                    }
                    self.pop_field_receiver(instruction_index, &owner)?;
                } else {
                    let ty = self.field_type(instruction_index, reference, pool)?;
                    self.pop_expected(instruction_index, value_kind(&ty))?;
                    self.pop_expected(instruction_index, ValueKind::Reference)?;
                }
            }
            Instruction::GetStatic(reference) => {
                let ty = self.field_type(instruction_index, reference, pool)?;
                if flow_current_class.is_some()
                    && validate_flow_field_type(instruction_index, reference.index().get(), &ty)?
                        .is_some()
                {
                    return Err(AnalysisError::UnsupportedFlowFieldType {
                        instruction: instruction_index,
                        index: reference.index().get(),
                    });
                }
                self.push(instruction_index, value_kind(&ty))?;
            }
            Instruction::PutStatic(reference) => {
                let ty = self.field_type(instruction_index, reference, pool)?;
                if flow_current_class.is_some()
                    && validate_flow_field_type(instruction_index, reference.index().get(), &ty)?
                        .is_some()
                {
                    return Err(AnalysisError::UnsupportedFlowFieldType {
                        instruction: instruction_index,
                        index: reference.index().get(),
                    });
                }
                self.pop_expected(instruction_index, value_kind(&ty))?;
            }
            Instruction::InvokeStatic(reference) => {
                let descriptor = self.method_descriptor(instruction_index, reference, pool)?;
                if flow_current_class.is_some() {
                    validate_flow_static_signature(
                        instruction_index,
                        reference.index().get(),
                        &descriptor,
                    )?;
                }
                self.invoke(instruction_index, &descriptor, false)?;
            }
            Instruction::InvokeVirtual(reference) => {
                if flow_current_class.is_some() {
                    self.ensure_no_pending_allocation(instruction_index)?;
                    let (owner, name, descriptor) =
                        self.virtual_method(instruction_index, reference, pool)?;
                    validate_flow_virtual_signature(
                        instruction_index,
                        reference.index().get(),
                        name,
                        &descriptor,
                    )?;
                    self.invoke_virtual_flow(instruction_index, &descriptor, &owner)?;
                } else {
                    let descriptor = self.method_descriptor(instruction_index, reference, pool)?;
                    self.invoke(instruction_index, &descriptor, true)?;
                }
            }
            Instruction::InvokeSpecial(reference) => {
                if flow_current_class.is_some() {
                    let (owner, name, descriptor) =
                        self.special_method(instruction_index, reference, pool)?;
                    self.invoke_constructor_flow(
                        instruction_index,
                        reference.index().get(),
                        &owner,
                        name,
                        &descriptor,
                    )?;
                } else {
                    let descriptor = self.method_descriptor(instruction_index, reference, pool)?;
                    self.invoke(instruction_index, &descriptor, true)?;
                }
            }
            Instruction::New(reference) => {
                if flow_current_class.is_some() {
                    if let Some(allocation) = &self.pending_allocation {
                        return Err(AnalysisError::UninitializedReferenceUse {
                            instruction: instruction_index,
                            allocation_site: allocation.site,
                        });
                    }
                    let class = self.allocation_class(instruction_index, reference, pool)?;
                    self.pending_allocation = Some(UninitializedAllocation {
                        site: instruction_index,
                        class: class.clone(),
                    });
                    self.push_typed(instruction_index, ValueKind::Reference, Some(class))?;
                    *self
                        .stack_allocation_sites
                        .last_mut()
                        .expect("push_typed adds an allocation-site slot") =
                        Some(instruction_index);
                } else {
                    self.validate_class(instruction_index, reference, pool)?;
                    self.push(instruction_index, ValueKind::Reference)?;
                }
            }
            Instruction::Return => {
                self.ensure_no_pending_allocation(instruction_index)?;
                self.check_return(instruction_index, None, method_descriptor)?;
                self.ensure_empty_stack(instruction_index)?;
            }
            Instruction::IReturn => {
                self.ensure_no_pending_allocation(instruction_index)?;
                self.check_return(instruction_index, Some(ValueKind::Int), method_descriptor)?;
                self.pop_expected(instruction_index, ValueKind::Int)?;
                self.ensure_empty_stack(instruction_index)?;
            }
            Instruction::AReturn => {
                self.ensure_no_pending_allocation(instruction_index)?;
                self.check_return(
                    instruction_index,
                    Some(ValueKind::Reference),
                    method_descriptor,
                )?;
                if flow_current_class.is_some() {
                    self.check_exact_reference_return(instruction_index, method_descriptor)?;
                }
                self.pop_expected(instruction_index, ValueKind::Reference)?;
                self.ensure_empty_stack(instruction_index)?;
            }
            Instruction::LReturn => {
                self.ensure_no_pending_allocation(instruction_index)?;
                self.check_return(instruction_index, Some(ValueKind::Long), method_descriptor)?;
                self.pop_expected(instruction_index, ValueKind::Long)?;
                self.ensure_empty_stack(instruction_index)?;
            }
        }
        Ok(())
    }

    fn push(&mut self, instruction: usize, value: ValueKind) -> Result<(), AnalysisError> {
        self.push_typed(instruction, value, None)
    }

    fn push_object(
        &mut self,
        instruction: usize,
        class: ClassIdentity,
    ) -> Result<(), AnalysisError> {
        self.push_typed(instruction, ValueKind::Reference, Some(class))
    }

    fn push_typed(
        &mut self,
        instruction: usize,
        value: ValueKind,
        class: Option<ClassIdentity>,
    ) -> Result<(), AnalysisError> {
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
        self.stack_classes.push(class);
        self.stack_allocation_sites.push(None);
        self.stack_slots = slots;
        self.max_stack = self.max_stack.max(slots);
        Ok(())
    }

    fn pop_expected(
        &mut self,
        instruction: usize,
        expected: ValueKind,
    ) -> Result<ValueKind, AnalysisError> {
        self.pop_typed(instruction, expected).map(|value| value.0)
    }

    fn pop_typed(
        &mut self,
        instruction: usize,
        expected: ValueKind,
    ) -> Result<(ValueKind, Option<ClassIdentity>), AnalysisError> {
        if let Some(site) = self.stack_allocation_sites.last().copied().flatten() {
            return Err(AnalysisError::UninitializedReferenceUse {
                instruction,
                allocation_site: site,
            });
        }
        let available = self.stack.len();
        let actual = self.stack.pop().ok_or(AnalysisError::StackUnderflow {
            instruction,
            needed: 1,
            available,
        })?;
        let class = self.stack_classes.pop().flatten();
        self.stack_allocation_sites.pop();
        self.stack_slots -= actual.slots();
        if actual != expected {
            return Err(AnalysisError::StackTypeMismatch {
                instruction,
                expected,
                actual,
            });
        }
        Ok((actual, class))
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
        let slot = self.locals.get(usize::from(index.get())).cloned().flatten();
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
            Some(LocalSlot::Object(class)) if expected == ValueKind::Reference => {
                self.push_object(instruction, class)
            }
            Some(LocalSlot::Object(_)) => Err(AnalysisError::LocalTypeMismatch {
                instruction,
                index: index.get(),
                expected,
                actual: ValueKind::Reference,
            }),
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
        let actual_class = self.pop_typed(instruction, expected)?.1;
        let start = usize::from(index.get());
        let width = expected.slots();
        for slot in start..start + width {
            self.clear_local_slot(slot);
        }
        self.locals[start] = Some(match actual_class {
            Some(class) => LocalSlot::Object(class),
            None => LocalSlot::Value(expected),
        });
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

    fn flow_field_type(
        &self,
        instruction: usize,
        reference: FieldReference,
        pool: &ConstantPool,
    ) -> Result<(ClassIdentity, FieldType), AnalysisError> {
        let (owner, _, ty) = self.field_reference(instruction, reference, pool)?;
        Ok((owner, ty))
    }

    fn field_reference<'a>(
        &self,
        instruction: usize,
        reference: FieldReference,
        pool: &'a ConstantPool,
    ) -> Result<(ClassIdentity, &'a str, FieldType), AnalysisError> {
        let index = reference.index().get();
        let entry = pool
            .get(index)
            .ok_or(AnalysisError::InvalidConstantPoolOperand {
                instruction,
                index,
                expected: ConstantPoolOperandKind::FieldReference,
            })?;
        let (class_index, name_and_type_index) = match entry {
            ConstantPoolEntry::Fieldref {
                class_index,
                name_and_type_index,
            } => (*class_index, *name_and_type_index),
            _ => {
                return Err(AnalysisError::InvalidConstantPoolOperand {
                    instruction,
                    index,
                    expected: ConstantPoolOperandKind::FieldReference,
                });
            }
        };
        let owner = pool
            .class_name(class_index)
            .map_err(|error| AnalysisError::InvalidConstantPoolReference { instruction, error })?;
        if !is_valid_internal_name(owner) {
            return Err(AnalysisError::InvalidClassName { index: class_index });
        }
        let (name, descriptor) = pool
            .name_and_type(name_and_type_index)
            .map_err(|error| AnalysisError::InvalidConstantPoolReference { instruction, error })?;
        let ty = FieldType::parse(descriptor).map_err(|error| {
            AnalysisError::InvalidFieldDescriptor {
                instruction,
                index,
                error,
            }
        })?;
        Ok((ClassIdentity(Arc::from(owner)), name, ty))
    }

    fn pop_field_receiver(
        &mut self,
        instruction: usize,
        expected_owner: &ClassIdentity,
    ) -> Result<(), AnalysisError> {
        let (_, actual) = self.pop_typed(instruction, ValueKind::Reference)?;
        let actual = actual.ok_or(AnalysisError::MissingExactReference { instruction })?;
        if !self
            .hierarchy
            .is_assignable(actual.internal_name(), expected_owner.internal_name())
        {
            return Err(AnalysisError::FieldOwnerMismatch {
                instruction,
                expected: expected_owner.clone(),
                actual,
            });
        }
        Ok(())
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

    fn virtual_method<'a>(
        &self,
        instruction: usize,
        reference: VirtualMethodReference,
        pool: &'a ConstantPool,
    ) -> Result<(ClassIdentity, &'a str, MethodDescriptor), AnalysisError> {
        let index = reference.index().get();
        let entry = pool
            .get(index)
            .ok_or(AnalysisError::InvalidConstantPoolOperand {
                instruction,
                index,
                expected: ConstantPoolOperandKind::VirtualMethodReference,
            })?;
        let (class_index, name_and_type_index) = match entry {
            ConstantPoolEntry::Methodref {
                class_index,
                name_and_type_index,
            } => (*class_index, *name_and_type_index),
            _ => {
                return Err(AnalysisError::InvalidConstantPoolOperand {
                    instruction,
                    index,
                    expected: ConstantPoolOperandKind::VirtualMethodReference,
                });
            }
        };
        let owner = pool
            .class_name(class_index)
            .map_err(|error| AnalysisError::InvalidConstantPoolReference { instruction, error })?;
        if !is_valid_internal_name(owner) {
            return Err(AnalysisError::InvalidClassName { index: class_index });
        }
        let (name, raw_descriptor) = pool
            .name_and_type(name_and_type_index)
            .map_err(|error| AnalysisError::InvalidConstantPoolReference { instruction, error })?;
        let descriptor = MethodDescriptor::parse(raw_descriptor).map_err(|error| {
            AnalysisError::InvalidReferencedMethodDescriptor {
                instruction,
                index,
                error,
            }
        })?;
        Ok((ClassIdentity(Arc::from(owner)), name, descriptor))
    }

    fn special_method<'a>(
        &self,
        instruction: usize,
        reference: SpecialMethodReference,
        pool: &'a ConstantPool,
    ) -> Result<(ClassIdentity, &'a str, MethodDescriptor), AnalysisError> {
        let index = reference.index().get();
        let entry = pool
            .get(index)
            .ok_or(AnalysisError::InvalidConstantPoolOperand {
                instruction,
                index,
                expected: ConstantPoolOperandKind::StaticOrSpecialMethodReference,
            })?;
        let (class_index, name_and_type_index) = match entry {
            ConstantPoolEntry::Methodref {
                class_index,
                name_and_type_index,
            } => (*class_index, *name_and_type_index),
            _ => {
                return Err(AnalysisError::InvalidConstantPoolOperand {
                    instruction,
                    index,
                    expected: ConstantPoolOperandKind::StaticOrSpecialMethodReference,
                });
            }
        };
        let owner = pool
            .class_name(class_index)
            .map_err(|error| AnalysisError::InvalidConstantPoolReference { instruction, error })?;
        if !is_valid_internal_name(owner) {
            return Err(AnalysisError::InvalidClassName { index: class_index });
        }
        let (name, raw_descriptor) = pool
            .name_and_type(name_and_type_index)
            .map_err(|error| AnalysisError::InvalidConstantPoolReference { instruction, error })?;
        let descriptor = MethodDescriptor::parse(raw_descriptor).map_err(|error| {
            AnalysisError::InvalidReferencedMethodDescriptor {
                instruction,
                index,
                error,
            }
        })?;
        Ok((ClassIdentity(Arc::from(owner)), name, descriptor))
    }

    fn allocation_class(
        &self,
        instruction: usize,
        reference: ClassReference,
        pool: &ConstantPool,
    ) -> Result<ClassIdentity, AnalysisError> {
        let index = reference.index().get();
        let name =
            pool.class_name(index)
                .map_err(|_| AnalysisError::InvalidConstantPoolOperand {
                    instruction,
                    index,
                    expected: ConstantPoolOperandKind::Class,
                })?;
        if !is_valid_internal_name(name) {
            return Err(AnalysisError::InvalidClassName { index });
        }
        Ok(ClassIdentity(Arc::from(name)))
    }

    fn invoke_constructor_flow(
        &mut self,
        instruction: usize,
        index: ConstantPoolIndex,
        owner: &ClassIdentity,
        name: &str,
        descriptor: &MethodDescriptor,
    ) -> Result<(), AnalysisError> {
        if name != "<init>" {
            return Err(AnalysisError::InvalidReferencedMethodName { instruction, index });
        }
        if let Some(actual) = descriptor.return_type.as_ref() {
            return Err(AnalysisError::UnsupportedFlowMethodReturn {
                instruction,
                actual: actual.clone(),
            });
        }
        check_parameter_slots(descriptor, true, Some(instruction))?;
        for (parameter, ty) in descriptor.parameters.iter().enumerate().rev() {
            match ty {
                FieldType::Int | FieldType::Boolean => {
                    self.pop_expected(instruction, ValueKind::Int)?;
                }
                FieldType::Object(name) if is_valid_internal_name(name) => {
                    let (_, actual) = self.pop_typed(instruction, ValueKind::Reference)?;
                    let expected = ClassIdentity(Arc::from(name.as_str()));
                    let actual = actual.ok_or(AnalysisError::MissingExactMethodArgument {
                        instruction,
                        parameter,
                    })?;
                    if !self
                        .hierarchy
                        .is_assignable(actual.internal_name(), expected.internal_name())
                    {
                        return Err(AnalysisError::WrongExactMethodArgument {
                            instruction,
                            parameter,
                            expected,
                            actual,
                        });
                    }
                }
                FieldType::Object(name) => {
                    return Err(AnalysisError::InvalidReferencedMethodClassName {
                        instruction,
                        index,
                        parameter: Some(parameter),
                        name: name.clone(),
                    });
                }
                _ => {
                    return Err(AnalysisError::UnsupportedFlowMethodParameter {
                        instruction,
                        parameter,
                        actual: ty.clone(),
                    });
                }
            }
        }

        let Some(actual_kind) = self.stack.last().copied() else {
            return Err(AnalysisError::StackUnderflow {
                instruction,
                needed: 1,
                available: 0,
            });
        };
        if actual_kind != ValueKind::Reference {
            return Err(AnalysisError::StackTypeMismatch {
                instruction,
                expected: ValueKind::Reference,
                actual: actual_kind,
            });
        }
        let Some(site) = self.stack_allocation_sites.last().copied().flatten() else {
            return Err(AnalysisError::ConstructorReceiverNotUninitialized { instruction });
        };
        let Some(allocated) = self.stack_classes.last().cloned().flatten() else {
            return Err(AnalysisError::ConstructorReceiverNotUninitialized { instruction });
        };
        if allocated != *owner {
            return Err(AnalysisError::ConstructorOwnerMismatch {
                instruction,
                allocated,
                actual: owner.clone(),
            });
        }
        let matches_pending = self
            .pending_allocation
            .as_ref()
            .is_some_and(|allocation| allocation.site == site && allocation.class == allocated);
        if !matches_pending {
            return Err(AnalysisError::ConstructorReceiverNotUninitialized { instruction });
        }

        self.stack.pop();
        self.stack_classes.pop();
        self.stack_allocation_sites.pop();
        self.stack_slots -= 1;
        self.pending_allocation = None;
        for (class, allocation_site) in self
            .stack_classes
            .iter_mut()
            .zip(&mut self.stack_allocation_sites)
        {
            if *allocation_site == Some(site) {
                *allocation_site = None;
                *class = Some(allocated.clone());
            }
        }
        Ok(())
    }

    fn ensure_no_pending_allocation(&self, instruction: usize) -> Result<(), AnalysisError> {
        if let Some(allocation) = &self.pending_allocation {
            Err(AnalysisError::UninitializedReferenceUse {
                instruction,
                allocation_site: allocation.site,
            })
        } else {
            Ok(())
        }
    }

    fn invoke_virtual_flow(
        &mut self,
        instruction: usize,
        descriptor: &MethodDescriptor,
        expected_owner: &ClassIdentity,
    ) -> Result<(), AnalysisError> {
        check_parameter_slots(descriptor, true, Some(instruction))?;
        for (parameter, ty) in descriptor.parameters.iter().enumerate().rev() {
            match ty {
                FieldType::Int | FieldType::Boolean => {
                    self.pop_expected(instruction, ValueKind::Int)?;
                }
                FieldType::Object(name) => {
                    let (_, actual) = self.pop_typed(instruction, ValueKind::Reference)?;
                    let expected = ClassIdentity(Arc::from(name.as_str()));
                    let actual = actual.ok_or(AnalysisError::MissingExactMethodArgument {
                        instruction,
                        parameter,
                    })?;
                    if !self
                        .hierarchy
                        .is_assignable(actual.internal_name(), expected.internal_name())
                    {
                        return Err(AnalysisError::WrongExactMethodArgument {
                            instruction,
                            parameter,
                            expected,
                            actual,
                        });
                    }
                }
                _ => {
                    return Err(AnalysisError::UnsupportedFlowMethodParameter {
                        instruction,
                        parameter,
                        actual: ty.clone(),
                    });
                }
            }
        }
        self.pop_method_receiver(instruction, expected_owner)?;
        match descriptor.return_type.as_ref() {
            None => {}
            Some(FieldType::Int | FieldType::Boolean) => {
                self.push(instruction, ValueKind::Int)?;
            }
            Some(FieldType::Object(name)) => {
                self.push_object(instruction, ClassIdentity(Arc::from(name.as_str())))?;
            }
            Some(actual) => {
                return Err(AnalysisError::UnsupportedFlowMethodReturn {
                    instruction,
                    actual: actual.clone(),
                });
            }
        }
        Ok(())
    }

    fn pop_method_receiver(
        &mut self,
        instruction: usize,
        expected_owner: &ClassIdentity,
    ) -> Result<(), AnalysisError> {
        let (_, actual) = self.pop_typed(instruction, ValueKind::Reference)?;
        let actual = actual.ok_or(AnalysisError::MissingExactReference { instruction })?;
        if !self
            .hierarchy
            .is_assignable(actual.internal_name(), expected_owner.internal_name())
        {
            return Err(AnalysisError::MethodOwnerMismatch {
                instruction,
                expected: expected_owner.clone(),
                actual,
            });
        }
        Ok(())
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

    fn check_exact_reference_return(
        &self,
        instruction: usize,
        descriptor: &MethodDescriptor,
    ) -> Result<(), AnalysisError> {
        let Some(FieldType::Object(expected_name)) = descriptor.return_type.as_ref() else {
            return Ok(());
        };
        let expected = ClassIdentity(Arc::from(expected_name.as_str()));
        let Some(ValueKind::Reference) = self.stack.last() else {
            // Preserve the ordinary return-category or stack-underflow error.
            return Ok(());
        };
        if let Some(allocation_site) = self.stack_allocation_sites.last().copied().flatten() {
            return Err(AnalysisError::UninitializedReferenceUse {
                instruction,
                allocation_site,
            });
        }
        let Some(actual) = self.stack_classes.last().cloned().flatten() else {
            return Err(AnalysisError::MissingExactReferenceReturn {
                instruction,
                expected,
            });
        };
        if self
            .hierarchy
            .is_assignable(actual.internal_name(), expected.internal_name())
        {
            Ok(())
        } else {
            Err(AnalysisError::WrongExactReferenceReturn {
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

/// The local-variable lattice used by CFG analysis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowLocalState {
    /// The slot is uninitialized or has incompatible predecessor values.
    Top,
    /// The slot contains an int-like JVM value.
    Integer,
    /// The slot is the current instance receiver.
    Receiver,
    /// The slot contains an initialized reference to this exact class.
    Object(ClassIdentity),
}

/// The incoming verifier state for one reachable basic block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockEntryState {
    /// Local slots, including receiver/argument slots and slots grown by stores.
    pub locals: Vec<FlowLocalState>,
    /// Exact int or initialized-class values in bottom-to-top order.
    pub operand_stack: Vec<FlowValue>,
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

/// Fixed-point stack and local analysis for CFGs with exact initialized
/// references, bounded field transfers and constructor arguments, primitive
/// virtual calls, and in-block allocation-site initialization.
pub struct CfgCodeAnalyzer;

impl CfgCodeAnalyzer {
    pub(crate) fn validate_all_instructions_with_references(
        graph: &ControlFlowGraph,
    ) -> Result<(), AnalysisError> {
        for block in graph.blocks() {
            for (offset, element) in block.instructions.iter().enumerate() {
                let instruction_index = block.instruction_range.start + offset;
                match element {
                    CodeElement::Instruction(instruction) => {
                        validate_flow_instruction_with_context(
                            instruction_index,
                            *instruction,
                            true,
                        )?;
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
    /// instance method is retained as an opaque compatibility placeholder.
    /// `if*` branches consume their comparison operands, `goto` preserves the
    /// stack, and predecessor states are merged until stable. Locals merge to
    /// `Top`; operand-stack heights and exact value types must agree.
    /// Unreachable blocks are retained by the CFG but have no entry state.
    /// Each local can only move from `Integer` to `Top`, so the worklist has a
    /// finite bound based on the graph size and local-state width. An
    /// incompatible stack state fails explicitly instead of growing a cycle.
    ///
    /// This compatibility entry point supports integer constants, integer
    /// local loads/stores, `dup`, integer arithmetic/comparisons, gotos, and
    /// `return`/`ireturn`. Reference, category-two, floating-point, exception,
    /// and switch flows are not supported. Use
    /// [`Self::analyze_with_current_class`] to enable exact initialized
    /// receiver moves, primitive field accesses, and exact `invokevirtual`
    /// calls with supported object/int-like descriptors. No byte offsets or
    /// stack-map frames are produced.
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

    /// Analyzes using a validated `CONSTANT_Class` as the exact initialized
    /// receiver type for a non-static, non-constructor method.
    ///
    /// The existing [`Self::analyze`] entry point retains its receiver
    /// placeholder behavior. This context enables exact initialized-reference
    /// moves, `dup`/`pop`, `getfield`/`putfield` for `int` and `boolean` fields,
    /// and `invokevirtual` methods with exact object or int-like parameters and
    /// exact object, int-like, or void results. Reference arguments and
    /// receivers must match their declared class names exactly; no hierarchy
    /// conversions are inferred by this entry point. Use
    /// [`Self::analyze_with_hierarchy`] to provide explicit superclass proofs.
    /// It also supports in-block `new; dup;
    /// primitive arguments; invokespecial <init>` with an exact allocation
    /// class and owner match. Uninitialized values may not escape to locals,
    /// ordinary operations, branches, or block boundaries. Field owners must
    /// match the exact receiver. Other invocation kinds, reference fields,
    /// arrays, and category-two method types remain unsupported.
    pub fn analyze_with_current_class(
        graph: &ControlFlowGraph,
        method_name: &str,
        method_descriptor: &str,
        is_static: bool,
        constant_pool: &ConstantPool,
        current_class: ConstantPoolIndex,
    ) -> Result<FlowAnalysis, AnalysisError> {
        constant_pool
            .validate_for_encoding()
            .map_err(AnalysisError::InvalidConstantPool)?;
        if is_static || method_name == "<init>" || method_name == "<clinit>" {
            return Err(AnalysisError::InvalidInitializationContext {
                is_static,
                is_constructor: method_name == "<init>" || method_name == "<clinit>",
            });
        }
        Self::analyze_with_hierarchy(
            graph,
            method_name,
            method_descriptor,
            is_static,
            constant_pool,
            current_class,
            &ClassHierarchy::default(),
        )
    }

    /// Analyzes reference transfers using an explicit, bounded superclass
    /// proof graph in addition to the exact initialized current-class type.
    pub fn analyze_with_hierarchy(
        graph: &ControlFlowGraph,
        method_name: &str,
        method_descriptor: &str,
        is_static: bool,
        constant_pool: &ConstantPool,
        current_class: ConstantPoolIndex,
        hierarchy: &ClassHierarchy,
    ) -> Result<FlowAnalysis, AnalysisError> {
        constant_pool
            .validate_for_encoding()
            .map_err(AnalysisError::InvalidConstantPool)?;
        if is_static || method_name == "<init>" || method_name == "<clinit>" {
            return Err(AnalysisError::InvalidInitializationContext {
                is_static,
                is_constructor: method_name == "<init>" || method_name == "<clinit>",
            });
        }
        Self::analyze_validated_with_current_class_and_hierarchy(
            graph,
            method_name,
            method_descriptor,
            is_static,
            constant_pool,
            Some(current_class),
            hierarchy,
        )
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
        Self::analyze_validated_with_current_class(
            graph,
            "",
            method_descriptor,
            is_static,
            constant_pool,
            None,
        )
    }

    pub(crate) fn analyze_validated_with_current_class(
        graph: &ControlFlowGraph,
        method_name: &str,
        method_descriptor: &str,
        is_static: bool,
        constant_pool: &ConstantPool,
        current_class_index: Option<ConstantPoolIndex>,
    ) -> Result<FlowAnalysis, AnalysisError> {
        Self::analyze_validated_with_current_class_and_hierarchy(
            graph,
            method_name,
            method_descriptor,
            is_static,
            constant_pool,
            current_class_index,
            &ClassHierarchy::default(),
        )
    }

    pub(crate) fn analyze_validated_with_current_class_and_hierarchy(
        graph: &ControlFlowGraph,
        method_name: &str,
        method_descriptor: &str,
        is_static: bool,
        constant_pool: &ConstantPool,
        current_class_index: Option<ConstantPoolIndex>,
        hierarchy: &ClassHierarchy,
    ) -> Result<FlowAnalysis, AnalysisError> {
        let is_constructor = method_name == "<init>";
        if current_class_index.is_some() && is_constructor {
            return Err(AnalysisError::InvalidInitializationContext {
                is_static,
                is_constructor,
            });
        }
        let current_class = current_class_index
            .map(|index| class_identity_from_pool(constant_pool, index, 0))
            .transpose()?;
        let descriptor = MethodDescriptor::parse(method_descriptor)
            .map_err(AnalysisError::InvalidMethodDescriptor)?;
        validate_descriptor_class_names(&descriptor)?;
        validate_bounded_reference_descriptor(&descriptor)?;
        check_parameter_slots(&descriptor, !is_static, None)?;
        for (parameter, ty) in descriptor.parameters.iter().enumerate() {
            if value_kind(ty) != ValueKind::Int
                && !(current_class.is_some() && matches!(ty, FieldType::Object(_)))
            {
                return Err(AnalysisError::UnsupportedFlowParameter {
                    parameter,
                    actual: value_kind(ty),
                });
            }
        }

        let mut initial = Frame::new(
            &descriptor,
            is_static,
            current_class.as_ref(),
            hierarchy.clone(),
        )?;
        let mut local_count = initial.locals.len();
        for block in graph.blocks() {
            for (offset, element) in block.instructions.iter().enumerate() {
                let instruction_index = block.instruction_range.start + offset;
                match element {
                    CodeElement::Instruction(instruction) => {
                        if block.reachable {
                            validate_flow_instruction_with_context(
                                instruction_index,
                                *instruction,
                                current_class.is_some(),
                            )?;
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
        let initial_state = state_from_frame(&initial)?;

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
            frame.hierarchy = hierarchy.clone();

            for (offset, element) in block.instructions.iter().enumerate() {
                let instruction_index = block.instruction_range.start + offset;
                match element {
                    CodeElement::Instruction(instruction) => frame.apply(
                        instruction_index,
                        *instruction,
                        &descriptor,
                        constant_pool,
                        current_class.as_ref(),
                    )?,
                    CodeElement::Branch(branch) => {
                        apply_branch(&mut frame, instruction_index, branch)?;
                        frame.ensure_no_pending_allocation(instruction_index)?;
                    }
                    CodeElement::Label(_) => {
                        return Err(AnalysisError::UnsupportedFlowInstruction {
                            instruction: instruction_index,
                            kind: UnsupportedFlowInstructionKind::Operation,
                        });
                    }
                }
            }
            frame.ensure_no_pending_allocation(block.instruction_range.end.saturating_sub(1))?;
            max_stack = max_stack.max(frame.max_stack);
            let outgoing = state_from_frame(&frame)?;

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

#[cfg(test)]
fn validate_flow_instruction(
    instruction_index: usize,
    instruction: Instruction,
) -> Result<(), AnalysisError> {
    validate_flow_instruction_with_context(instruction_index, instruction, false)
}

fn validate_flow_instruction_with_context(
    instruction_index: usize,
    instruction: Instruction,
    exact_references: bool,
) -> Result<(), AnalysisError> {
    let supported = matches!(
        instruction,
        Instruction::IConst(_)
            | Instruction::Bipush(_)
            | Instruction::Sipush(_)
            | Instruction::ILoad(_)
            | Instruction::IStore(_)
            | Instruction::IAdd
            | Instruction::ISub
            | Instruction::IMul
            | Instruction::IDiv
            | Instruction::IRem
            | Instruction::IAnd
            | Instruction::IOr
            | Instruction::IXor
            | Instruction::Dup
            | Instruction::Pop
            | Instruction::Return
            | Instruction::IReturn
            | Instruction::Ldc(LdcConstant::Integer(_))
    ) || (exact_references
        && matches!(
            instruction,
            Instruction::AConstNull
                | Instruction::ALoad(_)
                | Instruction::AStore(_)
                | Instruction::AReturn
                | Instruction::GetField(_)
                | Instruction::PutField(_)
                | Instruction::InvokeVirtual(_)
                | Instruction::InvokeSpecial(_)
                | Instruction::New(_)
        ));
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

fn validate_flow_field_type(
    instruction: usize,
    index: ConstantPoolIndex,
    ty: &FieldType,
) -> Result<Option<ClassIdentity>, AnalysisError> {
    match ty {
        FieldType::Int | FieldType::Boolean => Ok(None),
        FieldType::Object(name) if is_valid_internal_name(name) => {
            Ok(Some(ClassIdentity(Arc::from(name.as_str()))))
        }
        FieldType::Object(name) => Err(AnalysisError::InvalidReferencedFieldClassName {
            instruction,
            index,
            name: name.clone(),
        }),
        _ => Err(AnalysisError::UnsupportedFlowFieldType { instruction, index }),
    }
}

fn validate_flow_virtual_signature(
    instruction: usize,
    index: ConstantPoolIndex,
    name: &str,
    descriptor: &MethodDescriptor,
) -> Result<(), AnalysisError> {
    if name.is_empty()
        || name
            .chars()
            .any(|character| matches!(character, '.' | ';' | '[' | '/' | '<' | '>'))
    {
        return Err(AnalysisError::InvalidReferencedMethodName { instruction, index });
    }
    for (parameter, ty) in descriptor.parameters.iter().enumerate() {
        match ty {
            FieldType::Int | FieldType::Boolean => {}
            FieldType::Object(name) if is_valid_internal_name(name) => {}
            FieldType::Object(name) => {
                return Err(AnalysisError::InvalidReferencedMethodClassName {
                    instruction,
                    index,
                    parameter: Some(parameter),
                    name: name.clone(),
                });
            }
            _ => {
                return Err(AnalysisError::UnsupportedFlowMethodParameter {
                    instruction,
                    parameter,
                    actual: ty.clone(),
                });
            }
        }
    }
    match descriptor.return_type.as_ref() {
        None | Some(FieldType::Int | FieldType::Boolean) => {}
        Some(FieldType::Object(name)) if is_valid_internal_name(name) => {}
        Some(FieldType::Object(name)) => {
            return Err(AnalysisError::InvalidReferencedMethodClassName {
                instruction,
                index,
                parameter: None,
                name: name.clone(),
            });
        }
        Some(actual) => {
            return Err(AnalysisError::UnsupportedFlowMethodReturn {
                instruction,
                actual: actual.clone(),
            });
        }
    }
    Ok(())
}

fn validate_flow_static_signature(
    instruction: usize,
    index: ConstantPoolIndex,
    descriptor: &MethodDescriptor,
) -> Result<(), AnalysisError> {
    for (parameter, ty) in descriptor.parameters.iter().enumerate() {
        match ty {
            FieldType::Int | FieldType::Boolean => {}
            FieldType::Object(name) if !is_valid_internal_name(name) => {
                return Err(AnalysisError::InvalidReferencedMethodClassName {
                    instruction,
                    index,
                    parameter: Some(parameter),
                    name: name.clone(),
                });
            }
            other => {
                return Err(AnalysisError::UnsupportedFlowMethodParameter {
                    instruction,
                    parameter,
                    actual: other.clone(),
                });
            }
        }
    }
    match descriptor.return_type.as_ref() {
        None | Some(FieldType::Int | FieldType::Boolean) => Ok(()),
        Some(FieldType::Object(name)) if !is_valid_internal_name(name) => {
            Err(AnalysisError::InvalidReferencedMethodClassName {
                instruction,
                index,
                parameter: None,
                name: name.clone(),
            })
        }
        Some(actual) => Err(AnalysisError::UnsupportedFlowMethodReturn {
            instruction,
            actual: actual.clone(),
        }),
    }
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

fn is_valid_internal_name(name: &str) -> bool {
    !name.is_empty()
        && !name
            .split('/')
            .any(|part| part.is_empty() || part.chars().any(|c| matches!(c, '.' | ';' | '[')))
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

fn state_from_frame(frame: &Frame) -> Result<BlockEntryState, AnalysisError> {
    let locals = frame
        .locals
        .iter()
        .map(|slot| match slot {
            Some(LocalSlot::Value(ValueKind::Int)) => FlowLocalState::Integer,
            Some(LocalSlot::Receiver) => FlowLocalState::Receiver,
            Some(LocalSlot::Object(class)) => FlowLocalState::Object(class.clone()),
            _ => FlowLocalState::Top,
        })
        .collect();
    let operand_stack = frame
        .stack
        .iter()
        .zip(&frame.stack_classes)
        .map(|(value, class)| match (value, class) {
            (ValueKind::Int, _) => Ok(FlowValue::Integer),
            (ValueKind::Reference, Some(class)) => Ok(FlowValue::Object(class.clone())),
            (value, _) => Err(AnalysisError::UnsupportedFlowStackValue { actual: *value }),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(BlockEntryState {
        locals,
        operand_stack,
    })
}

fn frame_from_state(state: &BlockEntryState) -> Frame {
    let locals = state
        .locals
        .iter()
        .map(|local| match local {
            FlowLocalState::Top => None,
            FlowLocalState::Integer => Some(LocalSlot::Value(ValueKind::Int)),
            FlowLocalState::Receiver => Some(LocalSlot::Receiver),
            FlowLocalState::Object(class) => Some(LocalSlot::Object(class.clone())),
        })
        .collect();
    let stack = state
        .operand_stack
        .iter()
        .map(|value| match value {
            FlowValue::Integer => ValueKind::Int,
            FlowValue::Object(_) => ValueKind::Reference,
        })
        .collect::<Vec<_>>();
    let stack_classes = state
        .operand_stack
        .iter()
        .map(|value| match value {
            FlowValue::Integer => None,
            FlowValue::Object(class) => Some(class.clone()),
        })
        .collect();
    let stack_slots = stack.len();
    Frame {
        stack,
        stack_classes,
        stack_allocation_sites: vec![None; stack_slots],
        stack_slots,
        pending_allocation: None,
        locals,
        max_stack: stack_slots,
        hierarchy: ClassHierarchy::default(),
    }
}

fn merge_flow_states(
    existing: &mut BlockEntryState,
    incoming: &BlockEntryState,
    block: BasicBlockId,
) -> Result<bool, AnalysisError> {
    if existing.operand_stack.len() != incoming.operand_stack.len() {
        return Err(AnalysisError::IncompatibleStackMerge {
            block,
            expected_values: existing.operand_stack.len(),
            incoming_values: incoming.operand_stack.len(),
        });
    }
    for (value, (expected, incoming)) in existing
        .operand_stack
        .iter()
        .zip(&incoming.operand_stack)
        .enumerate()
    {
        if expected != incoming {
            return Err(AnalysisError::IncompatibleStackValue {
                block,
                value,
                expected: expected.clone(),
                incoming: incoming.clone(),
            });
        }
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
        BranchInstruction, CodeSequence, ConstantPoolIndexOperand, ControlFlowGraph,
        FieldReference, SmallIntConstant, SmallLongConstant,
    };
    use crate::constant_pool::ConstantPool;
    use crate::constant_pool_builder::ConstantPoolBuilder;

    fn operand(index: ConstantPoolIndex) -> ConstantPoolIndexOperand {
        ConstantPoolIndexOperand::try_from(index).unwrap()
    }

    fn method_descriptor(parameters: &str) -> String {
        format!("({parameters})V")
    }

    fn test_block_id() -> BasicBlockId {
        let mut sequence = CodeSequence::new();
        sequence.instruction(Instruction::Return);
        ControlFlowGraph::build(&sequence).unwrap().blocks()[0].id
    }

    fn class_identity(name: &str) -> ClassIdentity {
        ClassIdentity(Arc::from(name))
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
    fn characterizes_reference_cfg_operations_as_unsupported_without_class_context() {
        let mut builder = ConstantPoolBuilder::new();
        let class = builder.class("example/Counter").unwrap();
        let field = builder.field_ref("example/Counter", "value", "I").unwrap();
        let static_method = builder
            .method_ref("example/Counter", "make", "()V")
            .unwrap();
        let virtual_method = builder.method_ref("example/Counter", "get", "()I").unwrap();
        let constructor = builder
            .method_ref("example/Counter", "<init>", "()V")
            .unwrap();
        let _pool = builder.finish().unwrap();
        let cp_index = |index| operand(index);

        // The context-free compatibility entry point does not infer object
        // identities and continues to reject reference operations before type
        // flow. `analyze_with_current_class` enables only its documented subset.
        let reference_operations = [
            Instruction::AConstNull,
            Instruction::ALoad(LocalIndex::new(0)),
            Instruction::AStore(LocalIndex::new(0)),
            Instruction::GetField(FieldReference::new(cp_index(field))),
            Instruction::PutField(FieldReference::new(cp_index(field))),
            Instruction::GetStatic(FieldReference::new(cp_index(field))),
            Instruction::PutStatic(FieldReference::new(cp_index(field))),
            Instruction::InvokeStatic(StaticMethodReference::new(cp_index(static_method))),
            Instruction::InvokeVirtual(VirtualMethodReference::new(cp_index(virtual_method))),
            Instruction::InvokeSpecial(SpecialMethodReference::new(cp_index(constructor))),
            Instruction::New(ClassReference::new(cp_index(class))),
            Instruction::AReturn,
        ];

        for instruction in reference_operations {
            assert_eq!(
                validate_flow_instruction(7, instruction),
                Err(AnalysisError::UnsupportedFlowInstruction {
                    instruction: 7,
                    kind: UnsupportedFlowInstructionKind::Reference,
                }),
                "context-free CFG rejection for {instruction:?}"
            );
        }
    }

    #[test]
    fn preserves_exact_current_class_on_diamond_stack_and_local_joins() {
        let mut builder = ConstantPoolBuilder::new();
        let class_index = builder.class("example/Receiver").unwrap();
        let pool = builder.finish().unwrap();
        let mut sequence = CodeSequence::new();
        let right = sequence.new_label().unwrap();
        let join = sequence.new_label().unwrap();
        sequence
            .instruction(Instruction::IConst(SmallIntConstant::Zero))
            .branch(BranchInstruction::IfEq(right.clone()))
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::AStore(LocalIndex::new(1)))
            .instruction(Instruction::ALoad(LocalIndex::new(1)))
            .branch(BranchInstruction::Goto(join.clone()))
            .bind(right)
            .unwrap()
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::AStore(LocalIndex::new(1)))
            .instruction(Instruction::ALoad(LocalIndex::new(1)))
            .bind(join)
            .unwrap()
            .instruction(Instruction::Pop)
            .instruction(Instruction::ALoad(LocalIndex::new(1)))
            .instruction(Instruction::Pop)
            .instruction(Instruction::Return);
        let graph = ControlFlowGraph::build(&sequence).unwrap();
        let flow = CfgCodeAnalyzer::analyze_with_current_class(
            &graph,
            "ordinary",
            "()V",
            false,
            &pool,
            class_index,
        )
        .unwrap();
        let join_state = flow.entry_state(graph.blocks().last().unwrap().id).unwrap();

        assert_eq!(
            join_state.locals[1],
            FlowLocalState::Object(class_identity("example/Receiver"))
        );
        assert_eq!(
            join_state.operand_stack,
            [FlowValue::Object(class_identity("example/Receiver"))]
        );
    }

    #[test]
    fn preserves_exact_current_class_on_loop_back_edge() {
        let mut builder = ConstantPoolBuilder::new();
        let class_index = builder.class("example/LoopReceiver").unwrap();
        let pool = builder.finish().unwrap();
        let mut sequence = CodeSequence::new();
        let loop_head = sequence.new_label().unwrap();
        let exit = sequence.new_label().unwrap();
        sequence
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::AStore(LocalIndex::new(1)))
            .bind(loop_head.clone())
            .unwrap()
            .instruction(Instruction::ALoad(LocalIndex::new(1)))
            .instruction(Instruction::Pop)
            .instruction(Instruction::IConst(SmallIntConstant::Zero))
            .branch(BranchInstruction::IfEq(exit.clone()))
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::AStore(LocalIndex::new(1)))
            .branch(BranchInstruction::Goto(loop_head))
            .bind(exit)
            .unwrap()
            .instruction(Instruction::Return);
        let graph = ControlFlowGraph::build(&sequence).unwrap();
        let flow = CfgCodeAnalyzer::analyze_with_current_class(
            &graph,
            "ordinary",
            "()V",
            false,
            &pool,
            class_index,
        )
        .unwrap();
        let loop_state = flow.entry_state(graph.blocks()[1].id).unwrap();

        assert_eq!(
            loop_state.locals[1],
            FlowLocalState::Object(class_identity("example/LoopReceiver"))
        );
        assert!(loop_state.operand_stack.is_empty());
    }

    #[test]
    fn invokes_virtual_method_after_reference_local_branch_join() {
        let mut builder = ConstantPoolBuilder::new();
        let class_index = builder.class("example/Counter").unwrap();
        let get = builder.method_ref("example/Counter", "get", "()I").unwrap();
        let enabled = builder
            .method_ref("example/Counter", "isEnabled", "()Z")
            .unwrap();
        let pool = builder.finish().unwrap();
        let mut sequence = CodeSequence::new();
        let otherwise = sequence.new_label().unwrap();
        let join = sequence.new_label().unwrap();
        sequence
            .instruction(Instruction::ILoad(LocalIndex::new(1)))
            .branch(BranchInstruction::IfEq(otherwise.clone()))
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::AStore(LocalIndex::new(2)))
            .branch(BranchInstruction::Goto(join.clone()))
            .bind(otherwise)
            .unwrap()
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::AStore(LocalIndex::new(2)))
            .bind(join.clone())
            .unwrap()
            .instruction(Instruction::ALoad(LocalIndex::new(2)))
            .instruction(Instruction::InvokeVirtual(VirtualMethodReference::new(
                operand(get),
            )))
            .instruction(Instruction::IReturn);
        let graph = ControlFlowGraph::build(&sequence).unwrap();

        let flow = CfgCodeAnalyzer::analyze_with_current_class(
            &graph,
            "readAfterBranch",
            "(Z)I",
            false,
            &pool,
            class_index,
        )
        .unwrap();

        let join_state = flow
            .entry_state(graph.block_for_label(&join).unwrap())
            .unwrap();
        assert_eq!(
            join_state.locals[2],
            FlowLocalState::Object(class_identity("example/Counter"))
        );
        assert!(join_state.operand_stack.is_empty());
        assert_eq!(flow.max_stack, 1);
        assert_eq!(flow.max_locals, 3);

        let mut boolean_sequence = CodeSequence::new();
        boolean_sequence
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::InvokeVirtual(VirtualMethodReference::new(
                operand(enabled),
            )))
            .instruction(Instruction::IReturn);
        let boolean_graph = ControlFlowGraph::build(&boolean_sequence).unwrap();
        assert!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &boolean_graph,
                "enabled",
                "()Z",
                false,
                &pool,
                class_index,
            )
            .is_ok()
        );
    }

    #[test]
    fn invokes_virtual_method_on_loop_back_edge_and_void_call_leaves_empty_stack() {
        let mut builder = ConstantPoolBuilder::new();
        let class_index = builder.class("example/Counter").unwrap();
        let increment = builder
            .method_ref("example/Counter", "increment", "()I")
            .unwrap();
        let set = builder
            .method_ref("example/Counter", "set", "(I)V")
            .unwrap();
        let set_flag = builder
            .method_ref("example/Counter", "setFlag", "(Z)V")
            .unwrap();
        let pool = builder.finish().unwrap();

        let mut loop_sequence = CodeSequence::new();
        let loop_head = loop_sequence.new_label().unwrap();
        let exit = loop_sequence.new_label().unwrap();
        loop_sequence
            .bind(loop_head.clone())
            .unwrap()
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::InvokeVirtual(VirtualMethodReference::new(
                operand(increment),
            )))
            .instruction(Instruction::IStore(LocalIndex::new(1)))
            .instruction(Instruction::ILoad(LocalIndex::new(1)))
            .branch(BranchInstruction::IfEq(exit.clone()))
            .branch(BranchInstruction::Goto(loop_head.clone()))
            .bind(exit)
            .unwrap()
            .instruction(Instruction::Return);
        let loop_graph = ControlFlowGraph::build(&loop_sequence).unwrap();
        let loop_flow = CfgCodeAnalyzer::analyze_with_current_class(
            &loop_graph,
            "incrementUntilZero",
            "()V",
            false,
            &pool,
            class_index,
        )
        .unwrap();
        assert_eq!(loop_flow.max_stack, 1);
        assert_eq!(loop_flow.max_locals, 2);
        assert_eq!(
            loop_flow
                .entry_state(loop_graph.block_for_label(&loop_head).unwrap())
                .unwrap()
                .locals[0],
            FlowLocalState::Object(class_identity("example/Counter"))
        );

        let mut void_sequence = CodeSequence::new();
        let after_call = void_sequence.new_label().unwrap();
        void_sequence
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::ILoad(LocalIndex::new(1)))
            .instruction(Instruction::InvokeVirtual(VirtualMethodReference::new(
                operand(set),
            )))
            .branch(BranchInstruction::Goto(after_call.clone()))
            .bind(after_call.clone())
            .unwrap()
            .instruction(Instruction::Return);
        let void_graph = ControlFlowGraph::build(&void_sequence).unwrap();
        let void_flow = CfgCodeAnalyzer::analyze_with_current_class(
            &void_graph,
            "setValue",
            "(I)V",
            false,
            &pool,
            class_index,
        )
        .unwrap();
        assert!(
            void_flow
                .entry_state(void_graph.block_for_label(&after_call).unwrap())
                .unwrap()
                .operand_stack
                .is_empty()
        );
        assert_eq!(void_flow.max_stack, 2);

        let mut boolean_parameter_sequence = CodeSequence::new();
        boolean_parameter_sequence
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::ILoad(LocalIndex::new(1)))
            .instruction(Instruction::InvokeVirtual(VirtualMethodReference::new(
                operand(set_flag),
            )))
            .instruction(Instruction::Return);
        let boolean_parameter_graph = ControlFlowGraph::build(&boolean_parameter_sequence).unwrap();
        assert!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &boolean_parameter_graph,
                "setFlag",
                "(Z)V",
                false,
                &pool,
                class_index,
            )
            .is_ok()
        );
    }

    #[test]
    fn invokes_virtual_with_exact_mixed_reference_arguments_and_results() {
        let mut builder = ConstantPoolBuilder::new();
        let class_index = builder.class("example/Counter").unwrap();
        let mix = builder
            .method_ref(
                "example/Counter",
                "mix",
                "(ILexample/Arg;Z)Lexample/Result;",
            )
            .unwrap();
        let get = builder.method_ref("example/Result", "get", "()I").unwrap();
        let pool = builder.finish().unwrap();

        let mut chained = CodeSequence::new();
        chained
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::ILoad(LocalIndex::new(1)))
            .instruction(Instruction::ALoad(LocalIndex::new(2)))
            .instruction(Instruction::ILoad(LocalIndex::new(3)))
            .instruction(Instruction::InvokeVirtual(VirtualMethodReference::new(
                operand(mix),
            )))
            .instruction(Instruction::InvokeVirtual(VirtualMethodReference::new(
                operand(get),
            )))
            .instruction(Instruction::IReturn);
        let chained_graph = ControlFlowGraph::build(&chained).unwrap();
        let chained_flow = CfgCodeAnalyzer::analyze_with_current_class(
            &chained_graph,
            "mixAndChain",
            "(ILexample/Arg;Z)I",
            false,
            &pool,
            class_index,
        )
        .unwrap();
        assert_eq!(chained_flow.max_stack, 4);

        let mut loop_sequence = CodeSequence::new();
        let head = loop_sequence.new_label().unwrap();
        let done = loop_sequence.new_label().unwrap();
        loop_sequence
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::ILoad(LocalIndex::new(1)))
            .instruction(Instruction::ALoad(LocalIndex::new(2)))
            .instruction(Instruction::ILoad(LocalIndex::new(3)))
            .instruction(Instruction::InvokeVirtual(VirtualMethodReference::new(
                operand(mix),
            )))
            .instruction(Instruction::ILoad(LocalIndex::new(3)))
            .instruction(Instruction::IStore(LocalIndex::new(4)))
            .bind(head.clone())
            .unwrap()
            .instruction(Instruction::ILoad(LocalIndex::new(4)))
            .branch(BranchInstruction::IfEq(done.clone()))
            .instruction(Instruction::IConst(SmallIntConstant::Zero))
            .instruction(Instruction::IStore(LocalIndex::new(4)))
            .branch(BranchInstruction::Goto(head.clone()))
            .bind(done)
            .unwrap()
            .instruction(Instruction::AReturn);
        let loop_graph = ControlFlowGraph::build(&loop_sequence).unwrap();
        let loop_flow = CfgCodeAnalyzer::analyze_with_current_class(
            &loop_graph,
            "mixAcrossLoop",
            "(ILexample/Arg;Z)Lexample/Result;",
            false,
            &pool,
            class_index,
        )
        .unwrap();
        assert_eq!(
            loop_flow
                .entry_state(loop_graph.block_for_label(&head).unwrap())
                .unwrap()
                .operand_stack,
            [FlowValue::Object(class_identity("example/Result"))]
        );
        assert_eq!(
            loop_flow
                .entry_state(loop_graph.block_for_label(&head).unwrap())
                .unwrap()
                .locals[2],
            FlowLocalState::Object(class_identity("example/Arg"))
        );
    }

    #[test]
    fn rejects_wrong_uninitialized_and_swapped_virtual_reference_arguments() {
        let mut builder = ConstantPoolBuilder::new();
        let class_index = builder.class("example/Counter").unwrap();
        let arg_class = builder.class("example/Arg").unwrap();
        let arg_method = builder
            .method_ref("example/Counter", "take", "(Lexample/Arg;)V")
            .unwrap();
        let two_arg_method = builder
            .method_ref(
                "example/Counter",
                "takeTwo",
                "(Lexample/Arg;Lexample/Other;)V",
            )
            .unwrap();
        let pool = builder.finish().unwrap();

        let mut wrong_code = CodeSequence::new();
        wrong_code
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::ALoad(LocalIndex::new(1)))
            .instruction(Instruction::InvokeVirtual(VirtualMethodReference::new(
                operand(arg_method),
            )))
            .instruction(Instruction::Return);
        let wrong_graph = ControlFlowGraph::build(&wrong_code).unwrap();
        assert_eq!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &wrong_graph,
                "wrongArgument",
                "(Lexample/Other;)V",
                false,
                &pool,
                class_index,
            ),
            Err(AnalysisError::WrongExactMethodArgument {
                instruction: 2,
                parameter: 0,
                expected: class_identity("example/Arg"),
                actual: class_identity("example/Other"),
            })
        );

        let mut swapped_code = CodeSequence::new();
        swapped_code
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::ALoad(LocalIndex::new(2)))
            .instruction(Instruction::ALoad(LocalIndex::new(1)))
            .instruction(Instruction::InvokeVirtual(VirtualMethodReference::new(
                operand(two_arg_method),
            )))
            .instruction(Instruction::Return);
        let swapped_graph = ControlFlowGraph::build(&swapped_code).unwrap();
        assert_eq!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &swapped_graph,
                "swappedArguments",
                "(Lexample/Arg;Lexample/Other;)V",
                false,
                &pool,
                class_index,
            ),
            Err(AnalysisError::WrongExactMethodArgument {
                instruction: 3,
                parameter: 1,
                expected: class_identity("example/Other"),
                actual: class_identity("example/Arg"),
            })
        );

        let mut uninitialized_code = CodeSequence::new();
        uninitialized_code
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::New(ClassReference::new(operand(arg_class))))
            .instruction(Instruction::Dup)
            .instruction(Instruction::InvokeVirtual(VirtualMethodReference::new(
                operand(arg_method),
            )))
            .instruction(Instruction::Return);
        let uninitialized_graph = ControlFlowGraph::build(&uninitialized_code).unwrap();
        assert_eq!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &uninitialized_graph,
                "uninitializedArgument",
                "()V",
                false,
                &pool,
                class_index,
            ),
            Err(AnalysisError::UninitializedReferenceUse {
                instruction: 3,
                allocation_site: 1,
            })
        );

        let null_sequence = [
            Instruction::ALoad(LocalIndex::new(0)),
            Instruction::AConstNull,
            Instruction::InvokeVirtual(VirtualMethodReference::new(operand(arg_method))),
            Instruction::Return,
        ];
        assert_eq!(
            analyze_validated_with_current_class(
                "nullArgument",
                "()V",
                false,
                &null_sequence,
                &pool,
                Some(class_index),
            ),
            Err(AnalysisError::MissingExactMethodArgument {
                instruction: 2,
                parameter: 0,
            })
        );
    }

    #[test]
    fn rejects_virtual_method_owner_category_signature_and_reference_kinds() {
        let mut builder = ConstantPoolBuilder::new();
        let class_index = builder.class("example/Counter").unwrap();
        let wrong_owner = builder.method_ref("example/Other", "get", "()I").unwrap();
        let get = builder.method_ref("example/Counter", "get", "()I").unwrap();
        let take_int = builder
            .method_ref("example/Counter", "take", "(I)V")
            .unwrap();
        let too_many_parameters = builder
            .method_ref(
                "example/Counter",
                "tooWide",
                &format!("({})V", "I".repeat(255)),
            )
            .unwrap();
        let array_parameter = builder
            .method_ref("example/Counter", "acceptArray", "([I)V")
            .unwrap();
        let malformed_parameter_class = builder
            .method_ref("example/Counter", "acceptMalformed", "(Lbad.name;)V")
            .unwrap();
        let byte_parameter = builder
            .method_ref("example/Counter", "acceptByte", "(B)V")
            .unwrap();
        let long_result = builder
            .method_ref("example/Counter", "wide", "()J")
            .unwrap();
        let array_result = builder
            .method_ref("example/Counter", "array", "()[I")
            .unwrap();
        let constructor = builder
            .method_ref("example/Counter", "<init>", "()V")
            .unwrap();
        let malformed = builder
            .method_ref("example/Counter", "broken", "not-a-method")
            .unwrap();
        let interface_method = builder
            .interface_method_ref("example/Counter", "get", "()I")
            .unwrap();
        let invalid_owner_index = builder.class("bad.name").unwrap();
        let invalid_owner = builder.method_ref("bad.name", "get", "()I").unwrap();
        let pool = builder.finish().unwrap();

        let mut wrong_owner_code = CodeSequence::new();
        wrong_owner_code
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::InvokeVirtual(VirtualMethodReference::new(
                operand(wrong_owner),
            )))
            .instruction(Instruction::IReturn);
        let wrong_owner_graph = ControlFlowGraph::build(&wrong_owner_code).unwrap();
        assert_eq!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &wrong_owner_graph,
                "wrongOwner",
                "()I",
                false,
                &pool,
                class_index,
            ),
            Err(AnalysisError::MethodOwnerMismatch {
                instruction: 1,
                expected: class_identity("example/Other"),
                actual: class_identity("example/Counter"),
            })
        );

        let mut missing_receiver_code = CodeSequence::new();
        missing_receiver_code
            .instruction(Instruction::InvokeVirtual(VirtualMethodReference::new(
                operand(get),
            )))
            .instruction(Instruction::IReturn);
        let missing_receiver_graph = ControlFlowGraph::build(&missing_receiver_code).unwrap();
        assert_eq!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &missing_receiver_graph,
                "missingReceiver",
                "()I",
                false,
                &pool,
                class_index,
            ),
            Err(AnalysisError::StackUnderflow {
                instruction: 0,
                needed: 1,
                available: 0,
            })
        );

        let mut too_many_parameters_code = CodeSequence::new();
        too_many_parameters_code
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::InvokeVirtual(VirtualMethodReference::new(
                operand(too_many_parameters),
            )))
            .instruction(Instruction::Return);
        let too_many_parameters_graph = ControlFlowGraph::build(&too_many_parameters_code).unwrap();
        assert_eq!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &too_many_parameters_graph,
                "tooManyParameters",
                "()V",
                false,
                &pool,
                class_index,
            ),
            Err(AnalysisError::TooManyMethodParameterSlots {
                instruction: Some(1),
                slots: 256,
            })
        );

        let mut integer_receiver_code = CodeSequence::new();
        integer_receiver_code
            .instruction(Instruction::IConst(SmallIntConstant::Zero))
            .instruction(Instruction::InvokeVirtual(VirtualMethodReference::new(
                operand(get),
            )))
            .instruction(Instruction::IReturn);
        let integer_receiver_graph = ControlFlowGraph::build(&integer_receiver_code).unwrap();
        assert!(matches!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &integer_receiver_graph,
                "integerReceiver",
                "()I",
                false,
                &pool,
                class_index,
            ),
            Err(AnalysisError::StackTypeMismatch {
                instruction: 1,
                expected: ValueKind::Reference,
                actual: ValueKind::Int,
            })
        ));

        let mut wrong_argument_code = CodeSequence::new();
        wrong_argument_code
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::InvokeVirtual(VirtualMethodReference::new(
                operand(take_int),
            )))
            .instruction(Instruction::Return);
        let wrong_argument_graph = ControlFlowGraph::build(&wrong_argument_code).unwrap();
        assert!(matches!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &wrong_argument_graph,
                "wrongArgument",
                "()V",
                false,
                &pool,
                class_index,
            ),
            Err(AnalysisError::StackTypeMismatch {
                instruction: 2,
                expected: ValueKind::Int,
                actual: ValueKind::Reference,
            })
        ));

        let signature_cases = [
            (
                array_parameter,
                AnalysisError::UnsupportedFlowMethodParameter {
                    instruction: 1,
                    parameter: 0,
                    actual: FieldType::Array(Box::new(FieldType::Int)),
                },
            ),
            (
                long_result,
                AnalysisError::UnsupportedFlowMethodReturn {
                    instruction: 1,
                    actual: FieldType::Long,
                },
            ),
            (
                array_result,
                AnalysisError::UnsupportedFlowMethodReturn {
                    instruction: 1,
                    actual: FieldType::Array(Box::new(FieldType::Int)),
                },
            ),
            (
                byte_parameter,
                AnalysisError::UnsupportedFlowMethodParameter {
                    instruction: 1,
                    parameter: 0,
                    actual: FieldType::Byte,
                },
            ),
            (
                constructor,
                AnalysisError::InvalidReferencedMethodName {
                    instruction: 1,
                    index: constructor,
                },
            ),
        ];
        for (method, expected) in signature_cases {
            let mut sequence = CodeSequence::new();
            sequence
                .instruction(Instruction::ALoad(LocalIndex::new(0)))
                .instruction(Instruction::InvokeVirtual(VirtualMethodReference::new(
                    operand(method),
                )))
                .instruction(Instruction::Return);
            let graph = ControlFlowGraph::build(&sequence).unwrap();
            assert_eq!(
                CfgCodeAnalyzer::analyze_with_current_class(
                    &graph,
                    "invalidSignature",
                    "()V",
                    false,
                    &pool,
                    class_index,
                ),
                Err(expected)
            );
        }

        let mut malformed_parameter_code = CodeSequence::new();
        malformed_parameter_code
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::InvokeVirtual(VirtualMethodReference::new(
                operand(malformed_parameter_class),
            )))
            .instruction(Instruction::Return);
        let malformed_parameter_graph = ControlFlowGraph::build(&malformed_parameter_code).unwrap();
        assert_eq!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &malformed_parameter_graph,
                "malformedParameterClass",
                "()V",
                false,
                &pool,
                class_index,
            ),
            Err(AnalysisError::InvalidReferencedMethodClassName {
                instruction: 1,
                index: malformed_parameter_class,
                parameter: Some(0),
                name: "bad.name".to_owned(),
            })
        );

        let mut malformed_code = CodeSequence::new();
        malformed_code
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::InvokeVirtual(VirtualMethodReference::new(
                operand(malformed),
            )))
            .instruction(Instruction::Return);
        let malformed_graph = ControlFlowGraph::build(&malformed_code).unwrap();
        assert!(matches!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &malformed_graph,
                "malformedDescriptor",
                "()V",
                false,
                &pool,
                class_index,
            ),
            Err(AnalysisError::InvalidReferencedMethodDescriptor { index, .. }) if index == malformed
        ));

        let mut interface_code = CodeSequence::new();
        interface_code
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::InvokeVirtual(VirtualMethodReference::new(
                operand(interface_method),
            )))
            .instruction(Instruction::IReturn);
        let interface_graph = ControlFlowGraph::build(&interface_code).unwrap();
        assert!(matches!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &interface_graph,
                "interfaceReference",
                "()I",
                false,
                &pool,
                class_index,
            ),
            Err(AnalysisError::InvalidConstantPoolOperand {
                instruction: 1,
                expected: ConstantPoolOperandKind::VirtualMethodReference,
                ..
            })
        ));

        let mut invalid_owner_code = CodeSequence::new();
        invalid_owner_code
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::InvokeVirtual(VirtualMethodReference::new(
                operand(invalid_owner),
            )))
            .instruction(Instruction::IReturn);
        let invalid_owner_graph = ControlFlowGraph::build(&invalid_owner_code).unwrap();
        assert_eq!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &invalid_owner_graph,
                "invalidOwner",
                "()I",
                false,
                &pool,
                class_index,
            ),
            Err(AnalysisError::InvalidClassName {
                index: invalid_owner_index,
            })
        );
    }

    #[test]
    fn rejects_uninitialized_and_top_reference_locals_as_virtual_receivers() {
        let mut builder = ConstantPoolBuilder::new();
        let class_index = builder.class("example/Counter").unwrap();
        let get = builder.method_ref("example/Counter", "get", "()I").unwrap();
        let pool = builder.finish().unwrap();

        let mut uninitialized_code = CodeSequence::new();
        uninitialized_code
            .instruction(Instruction::ALoad(LocalIndex::new(1)))
            .instruction(Instruction::InvokeVirtual(VirtualMethodReference::new(
                operand(get),
            )))
            .instruction(Instruction::IReturn);
        let uninitialized_graph = ControlFlowGraph::build(&uninitialized_code).unwrap();
        assert_eq!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &uninitialized_graph,
                "uninitializedReceiver",
                "()I",
                false,
                &pool,
                class_index,
            ),
            Err(AnalysisError::UninitializedLocal {
                instruction: 0,
                index: 1,
            })
        );

        let mut top_code = CodeSequence::new();
        let other = top_code.new_label().unwrap();
        let join = top_code.new_label().unwrap();
        top_code
            .instruction(Instruction::ILoad(LocalIndex::new(1)))
            .branch(BranchInstruction::IfEq(other.clone()))
            .instruction(Instruction::IConst(SmallIntConstant::Zero))
            .instruction(Instruction::IStore(LocalIndex::new(2)))
            .branch(BranchInstruction::Goto(join.clone()))
            .bind(other)
            .unwrap()
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::AStore(LocalIndex::new(2)))
            .bind(join)
            .unwrap()
            .instruction(Instruction::ALoad(LocalIndex::new(2)))
            .instruction(Instruction::InvokeVirtual(VirtualMethodReference::new(
                operand(get),
            )))
            .instruction(Instruction::IReturn);
        let top_graph = ControlFlowGraph::build(&top_code).unwrap();
        assert!(matches!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &top_graph,
                "topReceiver",
                "(Z)I",
                false,
                &pool,
                class_index,
            ),
            Err(AnalysisError::UninitializedLocal {
                instruction: 7,
                index: 2,
            })
        ));
    }

    #[test]
    fn analyzes_int_field_reads_and_writes_through_reference_local_loop() {
        let mut builder = ConstantPoolBuilder::new();
        let class_index = builder.class("example/Counter").unwrap();
        let field = builder.field_ref("example/Counter", "count", "I").unwrap();
        let pool = builder.finish().unwrap();
        let mut sequence = CodeSequence::new();
        let loop_head = sequence.new_label().unwrap();
        let exit = sequence.new_label().unwrap();
        sequence
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::AStore(LocalIndex::new(1)))
            .instruction(Instruction::ALoad(LocalIndex::new(1)))
            .instruction(Instruction::Dup)
            .instruction(Instruction::GetField(FieldReference::new(operand(field))))
            .instruction(Instruction::Pop)
            .instruction(Instruction::GetField(FieldReference::new(operand(field))))
            .instruction(Instruction::Pop)
            .bind(loop_head.clone())
            .unwrap()
            .instruction(Instruction::ALoad(LocalIndex::new(1)))
            .instruction(Instruction::GetField(FieldReference::new(operand(field))))
            .branch(BranchInstruction::IfEq(exit.clone()))
            .instruction(Instruction::ALoad(LocalIndex::new(1)))
            .instruction(Instruction::ALoad(LocalIndex::new(1)))
            .instruction(Instruction::GetField(FieldReference::new(operand(field))))
            .instruction(Instruction::IConst(SmallIntConstant::One))
            .instruction(Instruction::IAdd)
            .instruction(Instruction::PutField(FieldReference::new(operand(field))))
            .branch(BranchInstruction::Goto(loop_head.clone()))
            .bind(exit)
            .unwrap()
            .instruction(Instruction::Return);
        let graph = ControlFlowGraph::build(&sequence).unwrap();

        let flow = CfgCodeAnalyzer::analyze_with_current_class(
            &graph,
            "increment",
            "()V",
            false,
            &pool,
            class_index,
        )
        .unwrap();

        assert_eq!(flow.max_stack, 3);
        assert_eq!(flow.max_locals, 2);
        assert_eq!(
            flow.entry_state(graph.block_for_label(&loop_head).unwrap())
                .unwrap()
                .locals[1],
            FlowLocalState::Object(class_identity("example/Counter"))
        );
    }

    #[test]
    fn rejects_field_owner_type_and_receiver_mismatches_in_cfg() {
        let mut builder = ConstantPoolBuilder::new();
        let class_index = builder.class("example/Counter").unwrap();
        let int_field = builder.field_ref("example/Counter", "count", "I").unwrap();
        let boolean_field = builder
            .field_ref("example/Counter", "enabled", "Z")
            .unwrap();
        let wrong_owner = builder.field_ref("example/Other", "count", "I").unwrap();
        let long_field = builder.field_ref("example/Counter", "wide", "J").unwrap();
        let pool = builder.finish().unwrap();

        let mut wrong_owner_code = CodeSequence::new();
        wrong_owner_code
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::GetField(FieldReference::new(operand(
                wrong_owner,
            ))))
            .instruction(Instruction::Pop)
            .instruction(Instruction::Return);
        let wrong_owner_graph = ControlFlowGraph::build(&wrong_owner_code).unwrap();
        assert_eq!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &wrong_owner_graph,
                "read",
                "()V",
                false,
                &pool,
                class_index,
            ),
            Err(AnalysisError::FieldOwnerMismatch {
                instruction: 1,
                expected: class_identity("example/Other"),
                actual: class_identity("example/Counter"),
            })
        );

        let mut boolean_code = CodeSequence::new();
        boolean_code
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::GetField(FieldReference::new(operand(
                boolean_field,
            ))))
            .instruction(Instruction::Pop)
            .instruction(Instruction::Return);
        let boolean_graph = ControlFlowGraph::build(&boolean_code).unwrap();
        assert!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &boolean_graph,
                "readBoolean",
                "()V",
                false,
                &pool,
                class_index,
            )
            .is_ok()
        );

        let mut unsupported_type_code = CodeSequence::new();
        unsupported_type_code
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::GetField(FieldReference::new(operand(
                long_field,
            ))))
            .instruction(Instruction::Pop)
            .instruction(Instruction::Return);
        let unsupported_type_graph = ControlFlowGraph::build(&unsupported_type_code).unwrap();
        assert_eq!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &unsupported_type_graph,
                "read",
                "()V",
                false,
                &pool,
                class_index,
            ),
            Err(AnalysisError::UnsupportedFlowFieldType {
                instruction: 1,
                index: long_field,
            })
        );

        let mut wrong_value_code = CodeSequence::new();
        wrong_value_code
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::PutField(FieldReference::new(operand(
                int_field,
            ))))
            .instruction(Instruction::Return);
        let wrong_value_graph = ControlFlowGraph::build(&wrong_value_code).unwrap();
        assert!(matches!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &wrong_value_graph,
                "write",
                "()V",
                false,
                &pool,
                class_index,
            ),
            Err(AnalysisError::StackTypeMismatch {
                instruction: 2,
                expected: ValueKind::Int,
                actual: ValueKind::Reference,
            })
        ));
    }

    #[test]
    fn analyzes_exact_object_field_reads_and_writes_in_cfg() {
        let mut builder = ConstantPoolBuilder::new();
        let holder_class = builder.class("example/Holder").unwrap();
        let counter_field = builder
            .field_ref("example/Holder", "ref", "Lexample/Counter;")
            .unwrap();
        let other_field = builder
            .field_ref("example/Holder", "other", "[Lexample/Other;")
            .unwrap();
        let pool = builder.finish().unwrap();

        let mut read = CodeSequence::new();
        read.instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::GetField(FieldReference::new(operand(
                counter_field,
            ))))
            .instruction(Instruction::AReturn);
        let read_graph = ControlFlowGraph::build(&read).unwrap();
        let read_flow = CfgCodeAnalyzer::analyze_with_current_class(
            &read_graph,
            "get",
            "()Lexample/Counter;",
            false,
            &pool,
            holder_class,
        )
        .unwrap();
        assert_eq!(read_flow.max_stack, 1);

        let mut write = CodeSequence::new();
        let alternate = write.new_label().unwrap();
        let join = write.new_label().unwrap();
        write
            .instruction(Instruction::ILoad(LocalIndex::new(2)))
            .branch(BranchInstruction::IfEq(alternate.clone()))
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::ALoad(LocalIndex::new(1)))
            .instruction(Instruction::PutField(FieldReference::new(operand(
                counter_field,
            ))))
            .branch(BranchInstruction::Goto(join.clone()))
            .bind(alternate)
            .unwrap()
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::ALoad(LocalIndex::new(1)))
            .instruction(Instruction::PutField(FieldReference::new(operand(
                counter_field,
            ))))
            .bind(join)
            .unwrap()
            .instruction(Instruction::Return);
        let write_graph = ControlFlowGraph::build(&write).unwrap();
        assert!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &write_graph,
                "setIf",
                "(Lexample/Counter;Z)V",
                false,
                &pool,
                holder_class,
            )
            .is_ok()
        );

        let mut wrong_value = CodeSequence::new();
        wrong_value
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::ALoad(LocalIndex::new(1)))
            .instruction(Instruction::PutField(FieldReference::new(operand(
                counter_field,
            ))))
            .instruction(Instruction::Return);
        let wrong_value_graph = ControlFlowGraph::build(&wrong_value).unwrap();
        assert_eq!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &wrong_value_graph,
                "setWrong",
                "(Lexample/Other;)V",
                false,
                &pool,
                holder_class,
            ),
            Err(AnalysisError::WrongExactFieldValue {
                instruction: 2,
                expected: class_identity("example/Counter"),
                actual: class_identity("example/Other"),
            })
        );

        let mut null_value = CodeSequence::new();
        null_value
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::AConstNull)
            .instruction(Instruction::PutField(FieldReference::new(operand(
                counter_field,
            ))))
            .instruction(Instruction::Return);
        let null_value_graph = ControlFlowGraph::build(&null_value).unwrap();
        assert_eq!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &null_value_graph,
                "setNull",
                "()V",
                false,
                &pool,
                holder_class,
            ),
            Err(AnalysisError::MissingExactFieldValue { instruction: 2 })
        );

        let mut unsupported_array = CodeSequence::new();
        unsupported_array
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::GetField(FieldReference::new(operand(
                other_field,
            ))))
            .instruction(Instruction::Pop)
            .instruction(Instruction::Return);
        let unsupported_array_graph = ControlFlowGraph::build(&unsupported_array).unwrap();
        assert_eq!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &unsupported_array_graph,
                "readArray",
                "()V",
                false,
                &pool,
                holder_class,
            ),
            Err(AnalysisError::UnsupportedFlowFieldType {
                instruction: 1,
                index: other_field,
            })
        );
    }

    #[test]
    fn invokespecial_constructor_arguments_use_only_verified_assignability() {
        let mut builder = ConstantPoolBuilder::new();
        let client_class = builder.class("example/Client").unwrap();
        let holder_class = builder.class("example/Holder").unwrap();
        let constructor = builder
            .method_ref(
                "example/Holder",
                "<init>",
                "(Lexample/Counter;Lexample/Other;)V",
            )
            .unwrap();
        let wrong_constructor = builder
            .method_ref("example/Holder", "<init>", "(Lexample/Counter;)V")
            .unwrap();
        let pool = builder.finish().unwrap();

        let mut construct = CodeSequence::new();
        construct
            .instruction(Instruction::New(ClassReference::new(operand(holder_class))))
            .instruction(Instruction::Dup)
            .instruction(Instruction::ALoad(LocalIndex::new(1)))
            .instruction(Instruction::ALoad(LocalIndex::new(2)))
            .instruction(Instruction::InvokeSpecial(SpecialMethodReference::new(
                operand(constructor),
            )))
            .instruction(Instruction::AReturn);
        let construct_graph = ControlFlowGraph::build(&construct).unwrap();
        assert!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &construct_graph,
                "make",
                "(Lexample/Counter;Lexample/Other;)Lexample/Holder;",
                false,
                &pool,
                client_class,
            )
            .is_ok()
        );

        let mut swapped_arguments = CodeSequence::new();
        swapped_arguments
            .instruction(Instruction::New(ClassReference::new(operand(holder_class))))
            .instruction(Instruction::Dup)
            .instruction(Instruction::ALoad(LocalIndex::new(2)))
            .instruction(Instruction::ALoad(LocalIndex::new(1)))
            .instruction(Instruction::InvokeSpecial(SpecialMethodReference::new(
                operand(constructor),
            )))
            .instruction(Instruction::AReturn);
        let swapped_arguments_graph = ControlFlowGraph::build(&swapped_arguments).unwrap();
        assert_eq!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &swapped_arguments_graph,
                "makeSwapped",
                "(Lexample/Counter;Lexample/Other;)Lexample/Holder;",
                false,
                &pool,
                client_class,
            ),
            Err(AnalysisError::WrongExactMethodArgument {
                instruction: 4,
                parameter: 1,
                expected: class_identity("example/Other"),
                actual: class_identity("example/Counter"),
            })
        );

        let mut wrong_argument = CodeSequence::new();
        wrong_argument
            .instruction(Instruction::New(ClassReference::new(operand(holder_class))))
            .instruction(Instruction::Dup)
            .instruction(Instruction::ALoad(LocalIndex::new(2)))
            .instruction(Instruction::InvokeSpecial(SpecialMethodReference::new(
                operand(wrong_constructor),
            )))
            .instruction(Instruction::AReturn);
        let wrong_argument_graph = ControlFlowGraph::build(&wrong_argument).unwrap();
        assert_eq!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &wrong_argument_graph,
                "makeWrong",
                "(Lexample/Counter;Lexample/Other;)Lexample/Holder;",
                false,
                &pool,
                client_class,
            ),
            Err(AnalysisError::WrongExactMethodArgument {
                instruction: 3,
                parameter: 0,
                expected: class_identity("example/Counter"),
                actual: class_identity("example/Other"),
            })
        );
    }

    #[test]
    fn invokespecial_accepts_a_constructor_argument_with_verified_parent_type() {
        let mut builder = ConstantPoolBuilder::new();
        let client_class = builder.class("example/Client").unwrap();
        let holder_class = builder.class("example/Holder").unwrap();
        let constructor = builder
            .method_ref("example/Holder", "<init>", "(Lexample/Base;)V")
            .unwrap();
        let pool = builder.finish().unwrap();
        let mut construct = CodeSequence::new();
        construct
            .instruction(Instruction::New(ClassReference::new(operand(holder_class))))
            .instruction(Instruction::Dup)
            .instruction(Instruction::ALoad(LocalIndex::new(1)))
            .instruction(Instruction::InvokeSpecial(SpecialMethodReference::new(
                operand(constructor),
            )))
            .instruction(Instruction::AReturn);
        let graph = ControlFlowGraph::build(&construct).unwrap();
        let hierarchy =
            ClassHierarchy::from_edges([("example/Derived".to_owned(), "example/Base".to_owned())])
                .unwrap();

        assert!(
            CfgCodeAnalyzer::analyze_with_hierarchy(
                &graph,
                "make",
                "(Lexample/Derived;)Lexample/Holder;",
                false,
                &pool,
                client_class,
                &hierarchy,
            )
            .is_ok()
        );
    }

    #[test]
    fn field_access_rejects_uninitialized_reference_local() {
        let mut builder = ConstantPoolBuilder::new();
        let class_index = builder.class("example/Counter").unwrap();
        let field = builder.field_ref("example/Counter", "count", "I").unwrap();
        let pool = builder.finish().unwrap();
        let mut sequence = CodeSequence::new();
        sequence
            .instruction(Instruction::ALoad(LocalIndex::new(1)))
            .instruction(Instruction::GetField(FieldReference::new(operand(field))))
            .instruction(Instruction::Pop)
            .instruction(Instruction::Return);
        let graph = ControlFlowGraph::build(&sequence).unwrap();

        assert_eq!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &graph,
                "read",
                "()V",
                false,
                &pool,
                class_index,
            ),
            Err(AnalysisError::UninitializedLocal {
                instruction: 0,
                index: 1,
            })
        );
    }

    #[test]
    fn rejects_distinct_reference_and_integer_stack_joins() {
        let block = test_block_id();
        let mut existing = BlockEntryState {
            locals: vec![],
            operand_stack: vec![FlowValue::Object(class_identity("example/A"))],
        };
        let incoming_reference = BlockEntryState {
            locals: vec![],
            operand_stack: vec![FlowValue::Object(class_identity("example/B"))],
        };
        assert!(matches!(
            merge_flow_states(&mut existing, &incoming_reference, block),
            Err(AnalysisError::IncompatibleStackValue { block: actual, value: 0, .. }) if actual == block
        ));

        let incoming_integer = BlockEntryState {
            locals: vec![],
            operand_stack: vec![FlowValue::Integer],
        };
        assert!(matches!(
            merge_flow_states(&mut existing, &incoming_integer, block),
            Err(AnalysisError::IncompatibleStackValue { value: 0, .. })
        ));
    }

    #[test]
    fn conflicting_reference_locals_degrade_to_top_and_cannot_be_loaded() {
        let mut existing = BlockEntryState {
            locals: vec![FlowLocalState::Object(class_identity("example/A"))],
            operand_stack: vec![],
        };
        let incoming = BlockEntryState {
            locals: vec![FlowLocalState::Object(class_identity("example/B"))],
            operand_stack: vec![],
        };
        assert!(merge_flow_states(&mut existing, &incoming, test_block_id()).unwrap());
        assert_eq!(existing.locals, [FlowLocalState::Top]);

        let mut frame = frame_from_state(&existing);
        assert_eq!(
            frame.load_local(4, LocalIndex::new(0), ValueKind::Reference),
            Err(AnalysisError::UninitializedLocal {
                instruction: 4,
                index: 0,
            })
        );
    }

    #[test]
    fn validates_current_class_context_and_constant_pool_class_name() {
        let mut sequence = CodeSequence::new();
        sequence.instruction(Instruction::Return);
        let graph = ControlFlowGraph::build(&sequence).unwrap();
        let mut builder = ConstantPoolBuilder::new();
        let class_index = builder.class("example/Valid").unwrap();
        let pool = builder.finish().unwrap();
        assert_eq!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &graph,
                "ordinary",
                "()V",
                true,
                &pool,
                class_index,
            ),
            Err(AnalysisError::InvalidInitializationContext {
                is_static: true,
                is_constructor: false,
            })
        );
        assert_eq!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &graph,
                "<init>",
                "()V",
                false,
                &pool,
                class_index,
            ),
            Err(AnalysisError::InvalidInitializationContext {
                is_static: false,
                is_constructor: true,
            })
        );
        assert!(matches!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &graph,
                "ordinary",
                "()V",
                false,
                &pool,
                ConstantPoolIndex(u16::MAX),
            ),
            Err(AnalysisError::InvalidConstantPoolReference { .. })
        ));

        let malformed = ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Utf8("bad.name".to_owned())),
            Some(ConstantPoolEntry::Class {
                name_index: ConstantPoolIndex(1),
            }),
        ]);
        assert_eq!(
            CfgCodeAnalyzer::analyze_with_current_class(
                &graph,
                "ordinary",
                "()V",
                false,
                &malformed,
                ConstantPoolIndex(2),
            ),
            Err(AnalysisError::InvalidClassName {
                index: ConstantPoolIndex(2),
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
    fn analyzes_each_integer_arithmetic_operation_with_two_ints() {
        let pool = ConstantPool::default();
        for operation in [
            Instruction::ISub,
            Instruction::IMul,
            Instruction::IDiv,
            Instruction::IRem,
        ] {
            let analysis = analyze(
                "(II)I",
                true,
                &[
                    Instruction::ILoad(LocalIndex::new(0)),
                    Instruction::ILoad(LocalIndex::new(1)),
                    operation,
                    Instruction::IReturn,
                ],
                &pool,
            )
            .unwrap();
            assert_eq!(analysis.max_stack, 2, "{operation:?}");
            assert_eq!(analysis.max_locals, 2, "{operation:?}");
        }
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
    fn cfg_initializes_all_aliases_for_zero_one_and_multiple_primitive_arguments() {
        let mut builder = ConstantPoolBuilder::new();
        let class = builder.class("example/Counter").unwrap();
        let no_args = builder
            .method_ref("example/Counter", "<init>", "()V")
            .unwrap();
        let one_arg = builder
            .method_ref("example/Counter", "<init>", "(I)V")
            .unwrap();
        let many_args = builder
            .method_ref("example/Counter", "<init>", "(IZI)V")
            .unwrap();
        let pool = builder.finish().unwrap();
        let class_identity = class_identity("example/Counter");

        for (constructor, arguments, expected_max_stack) in
            [(no_args, 0, 2), (one_arg, 1, 3), (many_args, 3, 5)]
        {
            let mut sequence = CodeSequence::new();
            let after = sequence.new_label().unwrap();
            sequence
                .instruction(Instruction::New(ClassReference::new(operand(class))))
                .instruction(Instruction::Dup);
            for _ in 0..arguments {
                sequence.instruction(Instruction::IConst(SmallIntConstant::One));
            }
            sequence
                .instruction(Instruction::InvokeSpecial(SpecialMethodReference::new(
                    operand(constructor),
                )))
                .instruction(Instruction::AStore(LocalIndex::new(2)))
                .branch(BranchInstruction::Goto(after.clone()))
                .bind(after.clone())
                .unwrap()
                .instruction(Instruction::Return);
            let graph = ControlFlowGraph::build(&sequence).unwrap();
            let flow = CfgCodeAnalyzer::analyze_with_current_class(
                &graph,
                "createCounter",
                "()V",
                false,
                &pool,
                class,
            )
            .unwrap();

            let initialized = flow
                .entry_state(graph.block_for_label(&after).unwrap())
                .unwrap();
            assert_eq!(
                initialized.locals[2],
                FlowLocalState::Object(class_identity.clone())
            );
            assert!(initialized.operand_stack.is_empty());
            assert_eq!(flow.max_stack, expected_max_stack);
            assert_eq!(flow.max_locals, 3);
        }
    }

    #[test]
    fn cfg_rewrites_every_surviving_duplicate_after_constructor_call() {
        let mut builder = ConstantPoolBuilder::new();
        let class = builder.class("example/Counter").unwrap();
        let constructor = builder
            .method_ref("example/Counter", "<init>", "()V")
            .unwrap();
        let pool = builder.finish().unwrap();
        let mut sequence = CodeSequence::new();
        let after = sequence.new_label().unwrap();
        sequence
            .instruction(Instruction::New(ClassReference::new(operand(class))))
            .instruction(Instruction::Dup)
            .instruction(Instruction::Dup)
            .instruction(Instruction::InvokeSpecial(SpecialMethodReference::new(
                operand(constructor),
            )))
            .branch(BranchInstruction::Goto(after.clone()))
            .bind(after.clone())
            .unwrap()
            .instruction(Instruction::Pop)
            .instruction(Instruction::Pop)
            .instruction(Instruction::Return);
        let graph = ControlFlowGraph::build(&sequence).unwrap();
        let flow = CfgCodeAnalyzer::analyze_with_current_class(
            &graph,
            "createCounter",
            "()V",
            false,
            &pool,
            class,
        )
        .unwrap();

        assert_eq!(
            flow.entry_state(graph.block_for_label(&after).unwrap())
                .unwrap()
                .operand_stack,
            [
                FlowValue::Object(class_identity("example/Counter")),
                FlowValue::Object(class_identity("example/Counter")),
            ]
        );
        assert_eq!(flow.max_stack, 3);
    }

    #[test]
    fn cfg_allows_distinct_initialized_allocations_to_join_and_reinitialize_in_loops() {
        let mut builder = ConstantPoolBuilder::new();
        let class = builder.class("example/Counter").unwrap();
        let constructor = builder
            .method_ref("example/Counter", "<init>", "(I)V")
            .unwrap();
        let pool = builder.finish().unwrap();

        let mut branches = CodeSequence::new();
        let otherwise = branches.new_label().unwrap();
        let join = branches.new_label().unwrap();
        branches
            .instruction(Instruction::ILoad(LocalIndex::new(1)))
            .branch(BranchInstruction::IfEq(otherwise.clone()))
            .instruction(Instruction::New(ClassReference::new(operand(class))))
            .instruction(Instruction::Dup)
            .instruction(Instruction::Bipush(10))
            .instruction(Instruction::InvokeSpecial(SpecialMethodReference::new(
                operand(constructor),
            )))
            .instruction(Instruction::AStore(LocalIndex::new(2)))
            .branch(BranchInstruction::Goto(join.clone()))
            .bind(otherwise)
            .unwrap()
            .instruction(Instruction::New(ClassReference::new(operand(class))))
            .instruction(Instruction::Dup)
            .instruction(Instruction::Bipush(20))
            .instruction(Instruction::InvokeSpecial(SpecialMethodReference::new(
                operand(constructor),
            )))
            .instruction(Instruction::AStore(LocalIndex::new(2)))
            .bind(join.clone())
            .unwrap()
            .instruction(Instruction::ALoad(LocalIndex::new(2)))
            .instruction(Instruction::Pop)
            .instruction(Instruction::Return);
        let graph = ControlFlowGraph::build(&branches).unwrap();
        let flow = CfgCodeAnalyzer::analyze_with_current_class(
            &graph,
            "createOnEitherPath",
            "(Z)V",
            false,
            &pool,
            class,
        )
        .unwrap();
        assert_eq!(
            flow.entry_state(graph.block_for_label(&join).unwrap())
                .unwrap()
                .locals[2],
            FlowLocalState::Object(class_identity("example/Counter"))
        );
        assert_eq!(flow.max_stack, 3);

        let mut loop_code = CodeSequence::new();
        let loop_head = loop_code.new_label().unwrap();
        let exit = loop_code.new_label().unwrap();
        loop_code
            .bind(loop_head.clone())
            .unwrap()
            .instruction(Instruction::New(ClassReference::new(operand(class))))
            .instruction(Instruction::Dup)
            .instruction(Instruction::ILoad(LocalIndex::new(1)))
            .instruction(Instruction::InvokeSpecial(SpecialMethodReference::new(
                operand(constructor),
            )))
            .instruction(Instruction::AStore(LocalIndex::new(2)))
            .instruction(Instruction::ILoad(LocalIndex::new(1)))
            .branch(BranchInstruction::IfEq(exit.clone()))
            .branch(BranchInstruction::Goto(loop_head.clone()))
            .bind(exit)
            .unwrap()
            .instruction(Instruction::Return);
        let loop_graph = ControlFlowGraph::build(&loop_code).unwrap();
        let loop_flow = CfgCodeAnalyzer::analyze_with_current_class(
            &loop_graph,
            "createEachIteration",
            "(I)V",
            false,
            &pool,
            class,
        )
        .unwrap();
        assert_eq!(loop_flow.max_stack, 3);
        assert_eq!(
            loop_flow
                .entry_state(loop_graph.block_for_label(&loop_head).unwrap())
                .unwrap()
                .locals[2],
            FlowLocalState::Top
        );
    }

    #[test]
    fn cfg_rejects_uninitialized_reference_escapes_and_invalid_constructor_calls() {
        let mut builder = ConstantPoolBuilder::new();
        let class = builder.class("example/Counter").unwrap();
        let other_constructor = builder
            .method_ref("example/Other", "<init>", "()V")
            .unwrap();
        let constructor = builder
            .method_ref("example/Counter", "<init>", "()V")
            .unwrap();
        let bad_name = builder
            .method_ref("example/Counter", "<clinit>", "()V")
            .unwrap();
        let field = builder.field_ref("example/Counter", "value", "I").unwrap();
        let pool = builder.finish().unwrap();

        let analyze_sequence = |sequence: &CodeSequence| {
            let graph = ControlFlowGraph::build(sequence).unwrap();
            CfgCodeAnalyzer::analyze_with_current_class(
                &graph,
                "createCounter",
                "()V",
                false,
                &pool,
                class,
            )
        };

        let mut store = CodeSequence::new();
        store
            .instruction(Instruction::New(ClassReference::new(operand(class))))
            .instruction(Instruction::AStore(LocalIndex::new(2)))
            .instruction(Instruction::Return);
        assert_eq!(
            analyze_sequence(&store),
            Err(AnalysisError::UninitializedReferenceUse {
                instruction: 1,
                allocation_site: 0,
            })
        );

        let mut branch = CodeSequence::new();
        let target = branch.new_label().unwrap();
        branch
            .instruction(Instruction::New(ClassReference::new(operand(class))))
            .instruction(Instruction::Dup)
            .branch(BranchInstruction::Goto(target.clone()))
            .bind(target)
            .unwrap()
            .instruction(Instruction::InvokeSpecial(SpecialMethodReference::new(
                operand(constructor),
            )))
            .instruction(Instruction::Pop)
            .instruction(Instruction::Return);
        assert_eq!(
            analyze_sequence(&branch),
            Err(AnalysisError::UninitializedReferenceUse {
                instruction: 2,
                allocation_site: 0,
            })
        );

        let mut wrong_owner = CodeSequence::new();
        wrong_owner
            .instruction(Instruction::New(ClassReference::new(operand(class))))
            .instruction(Instruction::Dup)
            .instruction(Instruction::InvokeSpecial(SpecialMethodReference::new(
                operand(other_constructor),
            )))
            .instruction(Instruction::Pop)
            .instruction(Instruction::Return);
        assert_eq!(
            analyze_sequence(&wrong_owner),
            Err(AnalysisError::ConstructorOwnerMismatch {
                instruction: 2,
                allocated: class_identity("example/Counter"),
                actual: class_identity("example/Other"),
            })
        );

        let mut field_read = CodeSequence::new();
        field_read
            .instruction(Instruction::New(ClassReference::new(operand(class))))
            .instruction(Instruction::Dup)
            .instruction(Instruction::GetField(FieldReference::new(operand(field))))
            .instruction(Instruction::Pop)
            .instruction(Instruction::Pop)
            .instruction(Instruction::Return);
        assert_eq!(
            analyze_sequence(&field_read),
            Err(AnalysisError::UninitializedReferenceUse {
                instruction: 2,
                allocation_site: 0,
            })
        );

        let mut double_init = CodeSequence::new();
        double_init
            .instruction(Instruction::New(ClassReference::new(operand(class))))
            .instruction(Instruction::Dup)
            .instruction(Instruction::InvokeSpecial(SpecialMethodReference::new(
                operand(constructor),
            )))
            .instruction(Instruction::InvokeSpecial(SpecialMethodReference::new(
                operand(constructor),
            )))
            .instruction(Instruction::Pop)
            .instruction(Instruction::Return);
        assert_eq!(
            analyze_sequence(&double_init),
            Err(AnalysisError::ConstructorReceiverNotUninitialized { instruction: 3 })
        );

        let mut bad_name_code = CodeSequence::new();
        bad_name_code
            .instruction(Instruction::New(ClassReference::new(operand(class))))
            .instruction(Instruction::Dup)
            .instruction(Instruction::InvokeSpecial(SpecialMethodReference::new(
                operand(bad_name),
            )))
            .instruction(Instruction::Pop)
            .instruction(Instruction::Return);
        assert!(matches!(
            analyze_sequence(&bad_name_code),
            Err(AnalysisError::InvalidReferencedMethodName { instruction: 2, .. })
        ));

        let mut missing_init = CodeSequence::new();
        missing_init
            .instruction(Instruction::New(ClassReference::new(operand(class))))
            .instruction(Instruction::Dup)
            .instruction(Instruction::Pop)
            .instruction(Instruction::Return);
        assert_eq!(
            analyze_sequence(&missing_init),
            Err(AnalysisError::UninitializedReferenceUse {
                instruction: 3,
                allocation_site: 0,
            })
        );
    }

    #[test]
    fn cfg_validates_constructor_descriptor_and_rejects_nested_allocations() {
        let mut builder = ConstantPoolBuilder::new();
        let class = builder.class("example/Counter").unwrap();
        let int_constructor = builder
            .method_ref("example/Counter", "<init>", "(I)V")
            .unwrap();
        let void_constructor = builder
            .method_ref("example/Counter", "<init>", "()V")
            .unwrap();
        let reference_constructor = builder
            .method_ref("example/Counter", "<init>", "(Ljava/lang/Object;)V")
            .unwrap();
        let returning_constructor = builder
            .method_ref("example/Counter", "<init>", "()I")
            .unwrap();
        let malformed_constructor = builder
            .method_ref("example/Counter", "<init>", "not-a-method")
            .unwrap();
        let interface_constructor = builder
            .interface_method_ref("example/Counter", "<init>", "()V")
            .unwrap();
        let pool = builder.finish().unwrap();

        let analyze_sequence = |sequence: &CodeSequence| {
            let graph = ControlFlowGraph::build(sequence).unwrap();
            CfgCodeAnalyzer::analyze_with_current_class(
                &graph,
                "createCounter",
                "()V",
                false,
                &pool,
                class,
            )
        };

        let mut wrong_arity = CodeSequence::new();
        wrong_arity
            .instruction(Instruction::New(ClassReference::new(operand(class))))
            .instruction(Instruction::Dup)
            .instruction(Instruction::InvokeSpecial(SpecialMethodReference::new(
                operand(int_constructor),
            )))
            .instruction(Instruction::Pop)
            .instruction(Instruction::Return);
        assert_eq!(
            analyze_sequence(&wrong_arity),
            Err(AnalysisError::UninitializedReferenceUse {
                instruction: 2,
                allocation_site: 0,
            })
        );

        let mut wrong_category = CodeSequence::new();
        wrong_category
            .instruction(Instruction::New(ClassReference::new(operand(class))))
            .instruction(Instruction::Dup)
            .instruction(Instruction::IConst(SmallIntConstant::One))
            .instruction(Instruction::InvokeSpecial(SpecialMethodReference::new(
                operand(void_constructor),
            )))
            .instruction(Instruction::Pop)
            .instruction(Instruction::Return);
        assert_eq!(
            analyze_sequence(&wrong_category),
            Err(AnalysisError::StackTypeMismatch {
                instruction: 3,
                expected: ValueKind::Reference,
                actual: ValueKind::Int,
            })
        );

        for (constructor, expected) in [
            (
                reference_constructor,
                AnalysisError::UninitializedReferenceUse {
                    instruction: 2,
                    allocation_site: 0,
                },
            ),
            (
                returning_constructor,
                AnalysisError::UnsupportedFlowMethodReturn {
                    instruction: 2,
                    actual: FieldType::Int,
                },
            ),
        ] {
            let mut sequence = CodeSequence::new();
            sequence
                .instruction(Instruction::New(ClassReference::new(operand(class))))
                .instruction(Instruction::Dup)
                .instruction(Instruction::InvokeSpecial(SpecialMethodReference::new(
                    operand(constructor),
                )))
                .instruction(Instruction::Pop)
                .instruction(Instruction::Return);
            assert_eq!(analyze_sequence(&sequence), Err(expected));
        }

        let mut malformed = CodeSequence::new();
        malformed
            .instruction(Instruction::New(ClassReference::new(operand(class))))
            .instruction(Instruction::Dup)
            .instruction(Instruction::InvokeSpecial(SpecialMethodReference::new(
                operand(malformed_constructor),
            )))
            .instruction(Instruction::Pop)
            .instruction(Instruction::Return);
        assert!(matches!(
            analyze_sequence(&malformed),
            Err(AnalysisError::InvalidReferencedMethodDescriptor { instruction: 2, .. })
        ));

        let mut interface_reference = CodeSequence::new();
        interface_reference
            .instruction(Instruction::New(ClassReference::new(operand(class))))
            .instruction(Instruction::Dup)
            .instruction(Instruction::InvokeSpecial(SpecialMethodReference::new(
                operand(interface_constructor),
            )))
            .instruction(Instruction::Pop)
            .instruction(Instruction::Return);
        assert!(matches!(
            analyze_sequence(&interface_reference),
            Err(AnalysisError::InvalidConstantPoolOperand { instruction: 2, .. })
        ));

        let mut nested = CodeSequence::new();
        nested
            .instruction(Instruction::New(ClassReference::new(operand(class))))
            .instruction(Instruction::Dup)
            .instruction(Instruction::New(ClassReference::new(operand(class))))
            .instruction(Instruction::Pop)
            .instruction(Instruction::Return);
        assert_eq!(
            analyze_sequence(&nested),
            Err(AnalysisError::UninitializedReferenceUse {
                instruction: 2,
                allocation_site: 0,
            })
        );
    }

    #[test]
    fn validates_bounded_parameterized_direct_super_constructor_prologues() {
        let mut builder = ConstantPoolBuilder::new();
        let child = builder.class("example/Derived").unwrap();
        let base = builder.class("example/Base").unwrap();
        let no_arg = builder.method_ref("example/Base", "<init>", "()V").unwrap();
        let int_arg = builder
            .method_ref("example/Base", "<init>", "(I)V")
            .unwrap();
        let mixed_args = builder
            .method_ref("example/Base", "<init>", "(IZ)V")
            .unwrap();
        let reference_arg = builder
            .method_ref("example/Base", "<init>", "(Lexample/Dep;)V")
            .unwrap();
        let object_arg = builder
            .method_ref("example/Base", "<init>", "(Ljava/lang/Object;)V")
            .unwrap();
        let integer_constants = [32_767, 32_768, -32_768, -32_769, i32::MIN, i32::MAX]
            .map(|value| (value, builder.integer(value).unwrap()));
        let wrong_owner = builder.method_ref("example/Dep", "<init>", "(I)V").unwrap();
        let returning_constructor = builder
            .method_ref("example/Base", "<init>", "(I)I")
            .unwrap();
        let pool = builder.finish().unwrap();
        let hierarchy = ClassHierarchy::from_edges([
            ("example/Derived".to_owned(), "example/Base".to_owned()),
            ("example/Dep".to_owned(), "java/lang/Object".to_owned()),
        ])
        .unwrap();
        let analyze_constructor = |descriptor: &str, instructions: &[Instruction]| {
            analyze_validated_constructor_body_with_hierarchy(
                descriptor,
                instructions,
                &pool,
                child,
                base,
                &hierarchy,
            )
        };
        let invoke =
            |index| Instruction::InvokeSpecial(SpecialMethodReference::new(operand(index)));

        let int_prologue = [
            Instruction::ALoad(LocalIndex::new(0)),
            Instruction::ILoad(LocalIndex::new(1)),
            invoke(int_arg),
            Instruction::Return,
        ];
        assert_eq!(
            analyze_constructor("(I)V", &int_prologue),
            Ok(CodeAnalysis {
                max_stack: 2,
                max_locals: 2,
            })
        );

        let mixed_prologue = [
            Instruction::ALoad(LocalIndex::new(0)),
            Instruction::ILoad(LocalIndex::new(1)),
            Instruction::ILoad(LocalIndex::new(2)),
            invoke(mixed_args),
            Instruction::Return,
        ];
        assert_eq!(
            analyze_constructor("(IZ)V", &mixed_prologue),
            Ok(CodeAnalysis {
                max_stack: 3,
                max_locals: 3,
            })
        );

        let reference_prologue = [
            Instruction::ALoad(LocalIndex::new(0)),
            Instruction::ALoad(LocalIndex::new(1)),
            invoke(reference_arg),
            Instruction::Return,
        ];
        assert_eq!(
            analyze_constructor("(Lexample/Dep;)V", &reference_prologue),
            Ok(CodeAnalysis {
                max_stack: 2,
                max_locals: 2,
            })
        );

        let constant_prologue = [
            Instruction::ALoad(LocalIndex::new(0)),
            Instruction::IConst(SmallIntConstant::One),
            invoke(int_arg),
            Instruction::Return,
        ];
        assert_eq!(
            analyze_constructor("()V", &constant_prologue),
            Ok(CodeAnalysis {
                max_stack: 2,
                max_locals: 1,
            })
        );

        for (_, integer_constant) in integer_constants {
            let ldc_prologue = [
                Instruction::ALoad(LocalIndex::new(0)),
                Instruction::Ldc(LdcConstant::Integer(operand(integer_constant))),
                invoke(int_arg),
                Instruction::Return,
            ];
            assert_eq!(
                analyze_constructor("()V", &ldc_prologue),
                Ok(CodeAnalysis {
                    max_stack: 2,
                    max_locals: 1,
                })
            );
        }

        let wrong_ldc_operand = [
            Instruction::ALoad(LocalIndex::new(0)),
            Instruction::Ldc(LdcConstant::Integer(operand(child))),
            invoke(int_arg),
            Instruction::Return,
        ];
        assert!(matches!(
            analyze_constructor("()V", &wrong_ldc_operand),
            Err(AnalysisError::InvalidConstantPoolOperand {
                instruction: 1,
                expected: ConstantPoolOperandKind::IntegerConstant,
                ..
            })
        ));

        let no_arguments = [Instruction::ALoad(LocalIndex::new(0)), invoke(int_arg)];
        assert_eq!(
            analyze_constructor("()V", &no_arguments),
            Err(AnalysisError::ConstructorSuperArgumentCountMismatch {
                instruction: 1,
                expected: 1,
                actual: 0,
            })
        );

        let wrong_reference_prologue = [
            Instruction::ALoad(LocalIndex::new(0)),
            Instruction::ALoad(LocalIndex::new(1)),
            invoke(reference_arg),
            Instruction::Return,
        ];
        assert!(matches!(
            analyze_constructor("(Ljava/lang/String;)V", &wrong_reference_prologue),
            Err(AnalysisError::WrongExactConstructorSuperArgument {
                instruction: 2,
                parameter: 0,
                ref expected,
                ref actual,
            }) if expected == &class_identity("example/Dep")
                && actual == &class_identity("java/lang/String")
        ));

        let uninitialized_this_argument = [
            Instruction::ALoad(LocalIndex::new(0)),
            Instruction::ALoad(LocalIndex::new(0)),
            invoke(object_arg),
            Instruction::Return,
        ];
        assert_eq!(
            analyze_constructor("()V", &uninitialized_this_argument),
            Err(AnalysisError::InvalidConstructorSuperCall { instruction: 1 })
        );

        let wrong_owner_prologue = [
            Instruction::ALoad(LocalIndex::new(0)),
            Instruction::IConst(SmallIntConstant::One),
            invoke(wrong_owner),
            Instruction::Return,
        ];
        assert_eq!(
            analyze_constructor("()V", &wrong_owner_prologue),
            Err(AnalysisError::InvalidConstructorSuperCall { instruction: 2 })
        );

        let wrong_return_prologue = [
            Instruction::ALoad(LocalIndex::new(0)),
            Instruction::IConst(SmallIntConstant::One),
            invoke(returning_constructor),
            Instruction::Return,
        ];
        assert_eq!(
            analyze_constructor("()V", &wrong_return_prologue),
            Err(AnalysisError::InvalidConstructorSuperCall { instruction: 2 })
        );

        let missing_call = [Instruction::ALoad(LocalIndex::new(0)), Instruction::Return];
        assert_eq!(
            analyze_constructor("()V", &missing_call),
            Err(AnalysisError::InvalidConstructorSuperCall { instruction: 1 })
        );

        let duplicate_call = [
            Instruction::ALoad(LocalIndex::new(0)),
            invoke(no_arg),
            Instruction::ALoad(LocalIndex::new(0)),
            invoke(no_arg),
            Instruction::Return,
        ];
        assert_eq!(
            analyze_constructor("()V", &duplicate_call),
            Err(AnalysisError::ConstructorReceiverNotUninitialized { instruction: 3 })
        );
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
    fn integer_arithmetic_rejects_underflow_and_non_integer_operands() {
        let pool = ConstantPool::default();
        let mut pool_builder = ConstantPoolBuilder::new();
        let double_method = pool_builder
            .method_ref("example/C", "double", "()D")
            .unwrap();
        let pool_with_double = pool_builder.finish().unwrap();
        for operation in [
            Instruction::ISub,
            Instruction::IMul,
            Instruction::IDiv,
            Instruction::IRem,
        ] {
            assert_eq!(
                analyze("()I", true, &[operation, Instruction::IReturn], &pool),
                Err(AnalysisError::StackUnderflow {
                    instruction: 0,
                    needed: 1,
                    available: 0,
                }),
                "{operation:?}"
            );
            assert_eq!(
                analyze(
                    "()I",
                    true,
                    &[
                        Instruction::AConstNull,
                        Instruction::IConst(SmallIntConstant::One),
                        operation,
                        Instruction::IReturn,
                    ],
                    &pool,
                ),
                Err(AnalysisError::StackTypeMismatch {
                    instruction: 2,
                    expected: ValueKind::Int,
                    actual: ValueKind::Reference,
                }),
                "{operation:?}"
            );
            assert_eq!(
                analyze(
                    "()I",
                    true,
                    &[
                        Instruction::LConst(SmallLongConstant::Zero),
                        operation,
                        Instruction::IReturn,
                    ],
                    &pool,
                ),
                Err(AnalysisError::StackTypeMismatch {
                    instruction: 1,
                    expected: ValueKind::Int,
                    actual: ValueKind::Long,
                }),
                "{operation:?}"
            );
            assert_eq!(
                analyze(
                    "()I",
                    true,
                    &[
                        Instruction::InvokeStatic(StaticMethodReference::new(operand(
                            double_method
                        ))),
                        operation,
                        Instruction::IReturn,
                    ],
                    &pool_with_double,
                ),
                Err(AnalysisError::StackTypeMismatch {
                    instruction: 1,
                    expected: ValueKind::Int,
                    actual: ValueKind::Double,
                }),
                "{operation:?}"
            );
        }
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
