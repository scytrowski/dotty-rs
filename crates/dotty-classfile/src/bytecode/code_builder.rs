//! Symbolic builder for typed JVM instructions and integer-only branches.

use super::{
    ClassReference, ConstantPoolIndexOperand, FieldReference, Instruction, Ldc2Constant,
    LdcConstant, LocalIndex, SmallIntConstant, SmallLongConstant, SpecialMethodReference,
    StaticMethodReference, VirtualMethodReference,
};
use crate::constant_pool::ConstantPoolIndex;
use crate::descriptor::{DescriptorError, FieldType, MethodDescriptor};
use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

/// An opaque label owned by one [`CodeBuilder`].
#[derive(Clone)]
pub struct CodeLabel {
    scope: Arc<()>,
    index: usize,
}

impl fmt::Debug for CodeLabel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "CodeLabel({})", self.index)
    }
}

/// A typed failure while adding an instruction to a [`CodeBuilder`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeBuilderError {
    /// A local index does not fit the JVM's `u16` instruction operand.
    LocalIndexOutOfRange { index: usize },
    /// An interned constant-pool index cannot be used as an instruction operand.
    InvalidConstantPoolIndex { index: u16 },
    /// A symbolic owner or member name is not a valid JVM name.
    InvalidName,
    /// A symbolic field or method descriptor is malformed.
    InvalidDescriptor(DescriptorError),
    /// An invoked method descriptor exceeds the JVM's 255 parameter-slot limit.
    TooManyParameterSlots,
    /// A branch or label operation used a label owned by another builder.
    ForeignLabel { label: usize },
    /// A branch targets a label that was allocated but never marked.
    UnboundLabel { label: usize },
    /// A label was marked more than once.
    DuplicateLabel { label: usize },
    /// A branch targets the position after the final executable instruction.
    BranchToTerminalEnd { label: usize },
    /// The builder exhausted its label identifiers.
    LabelIdExhausted,
}

impl fmt::Display for CodeBuilderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LocalIndexOutOfRange { index } => {
                write!(formatter, "local index {index} exceeds the JVM u16 range")
            }
            Self::InvalidConstantPoolIndex { index } => {
                write!(
                    formatter,
                    "constant-pool index {index} cannot identify an entry"
                )
            }
            Self::InvalidName => formatter.write_str("invalid symbolic JVM name"),
            Self::InvalidDescriptor(error) => {
                write!(formatter, "invalid symbolic JVM descriptor: {error}")
            }
            Self::TooManyParameterSlots => {
                formatter.write_str("method descriptor exceeds 255 parameter slots")
            }
            Self::ForeignLabel { label } => {
                write!(formatter, "label {label} belongs to another code builder")
            }
            Self::UnboundLabel { label } => {
                write!(formatter, "branch target label {label} is unbound")
            }
            Self::DuplicateLabel { label } => {
                write!(formatter, "label {label} was marked more than once")
            }
            Self::BranchToTerminalEnd { label } => {
                write!(formatter, "branch target label {label} is at code end")
            }
            Self::LabelIdExhausted => formatter.write_str("code builder exhausted label IDs"),
        }
    }
}

impl std::error::Error for CodeBuilderError {}

#[derive(Debug, Clone, Copy)]
pub(crate) enum FieldOperation {
    Get,
    Put,
    GetStatic,
    PutStatic,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum MethodOperation {
    Static,
    StaticInterface,
    Virtual,
    Special,
    SpecialInterface,
}

#[derive(Debug, Clone)]
pub(crate) enum SymbolicInstruction {
    Typed(Instruction),
    Integer(i32),
    Long(i64),
    New(String),
    Field {
        operation: FieldOperation,
        owner: String,
        name: String,
        descriptor: String,
    },
    Method {
        operation: MethodOperation,
        owner: String,
        name: String,
        descriptor: String,
    },
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum SymbolicBranch {
    Goto,
    GotoWide,
    IfEq,
    IfNe,
    IfICmpEq,
    IfICmpNe,
    IfICmpLt,
    IfICmpGe,
    IfICmpGt,
    IfICmpLe,
}

#[derive(Debug, Clone)]
pub(crate) enum SymbolicCodeElement {
    Instruction(SymbolicInstruction),
    Label(CodeLabel),
    Branch {
        operation: SymbolicBranch,
        target: CodeLabel,
    },
}

/// A source of typed instructions and symbolic JVM references for one method.
///
/// Constant-pool indices are assigned by [`crate::class_file_builder::ClassFileBuilder`]
/// when the class is encoded. The builder intentionally exposes no raw opcode
/// or constant-pool-index operation.
#[derive(Debug)]
pub struct CodeBuilder {
    pub(crate) elements: Vec<SymbolicCodeElement>,
    scope: Arc<()>,
    next_label: usize,
    defined_labels: HashSet<usize>,
}

impl Clone for CodeBuilder {
    fn clone(&self) -> Self {
        let scope = Arc::new(());
        let elements = self
            .elements
            .iter()
            .map(|element| match element {
                SymbolicCodeElement::Instruction(instruction) => {
                    SymbolicCodeElement::Instruction(instruction.clone())
                }
                SymbolicCodeElement::Label(label) => SymbolicCodeElement::Label(CodeLabel {
                    scope: Arc::clone(&scope),
                    index: label.index,
                }),
                SymbolicCodeElement::Branch { operation, target } => SymbolicCodeElement::Branch {
                    operation: *operation,
                    target: CodeLabel {
                        scope: Arc::clone(&scope),
                        index: target.index,
                    },
                },
            })
            .collect();
        Self {
            elements,
            scope,
            next_label: self.next_label,
            defined_labels: self.defined_labels.clone(),
        }
    }
}

impl Default for CodeBuilder {
    fn default() -> Self {
        Self {
            elements: Vec::new(),
            scope: Arc::new(()),
            next_label: 0,
            defined_labels: HashSet::new(),
        }
    }
}

impl CodeBuilder {
    /// Creates an empty instruction builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocates a label for a branch target in this method.
    pub fn new_label(&mut self) -> Result<CodeLabel, CodeBuilderError> {
        let index = self.next_label;
        self.next_label = index
            .checked_add(1)
            .ok_or(CodeBuilderError::LabelIdExhausted)?;
        Ok(CodeLabel {
            scope: Arc::clone(&self.scope),
            index,
        })
    }

    /// Marks a label at the current bytecode position.
    pub fn mark(&mut self, label: CodeLabel) -> Result<&mut Self, CodeBuilderError> {
        self.check_label(&label)?;
        if !self.defined_labels.insert(label.index) {
            return Err(CodeBuilderError::DuplicateLabel { label: label.index });
        }
        self.elements.push(SymbolicCodeElement::Label(label));
        Ok(self)
    }

    /// Adds an unconditional short branch (`goto`).
    pub fn goto(&mut self, target: CodeLabel) -> Result<&mut Self, CodeBuilderError> {
        self.branch(SymbolicBranch::Goto, target)
    }

    /// Adds an unconditional wide branch (`goto_w`).
    pub fn goto_w(&mut self, target: CodeLabel) -> Result<&mut Self, CodeBuilderError> {
        self.branch(SymbolicBranch::GotoWide, target)
    }

    /// Branches when the top integer is zero (`ifeq`).
    pub fn ifeq(&mut self, target: CodeLabel) -> Result<&mut Self, CodeBuilderError> {
        self.branch(SymbolicBranch::IfEq, target)
    }

    /// Branches when the top integer is nonzero (`ifne`).
    pub fn ifne(&mut self, target: CodeLabel) -> Result<&mut Self, CodeBuilderError> {
        self.branch(SymbolicBranch::IfNe, target)
    }

    /// Branches when the first of two integers equals the second.
    pub fn if_icmpeq(&mut self, target: CodeLabel) -> Result<&mut Self, CodeBuilderError> {
        self.branch(SymbolicBranch::IfICmpEq, target)
    }

    /// Branches when the first of two integers differs from the second.
    pub fn if_icmpne(&mut self, target: CodeLabel) -> Result<&mut Self, CodeBuilderError> {
        self.branch(SymbolicBranch::IfICmpNe, target)
    }

    /// Branches when the first integer is less than the second.
    pub fn if_icmplt(&mut self, target: CodeLabel) -> Result<&mut Self, CodeBuilderError> {
        self.branch(SymbolicBranch::IfICmpLt, target)
    }

    /// Branches when the first integer is greater than or equal to the second.
    pub fn if_icmpge(&mut self, target: CodeLabel) -> Result<&mut Self, CodeBuilderError> {
        self.branch(SymbolicBranch::IfICmpGe, target)
    }

    /// Branches when the first integer is greater than the second.
    pub fn if_icmpgt(&mut self, target: CodeLabel) -> Result<&mut Self, CodeBuilderError> {
        self.branch(SymbolicBranch::IfICmpGt, target)
    }

    /// Branches when the first integer is less than or equal to the second.
    pub fn if_icmple(&mut self, target: CodeLabel) -> Result<&mut Self, CodeBuilderError> {
        self.branch(SymbolicBranch::IfICmpLe, target)
    }

    pub(crate) fn has_branches(&self) -> bool {
        self.elements
            .iter()
            .any(|element| matches!(element, SymbolicCodeElement::Branch { .. }))
    }

    pub(crate) fn label_count(&self) -> usize {
        self.next_label
    }

    pub(crate) fn validate(&self) -> Result<(), CodeBuilderError> {
        let mut label_positions = std::collections::HashMap::new();
        let mut instruction_count = 0usize;
        for element in &self.elements {
            match element {
                SymbolicCodeElement::Label(label) => {
                    label_positions.insert(label.index, instruction_count);
                }
                SymbolicCodeElement::Instruction(_) | SymbolicCodeElement::Branch { .. } => {
                    instruction_count += 1;
                }
            }
        }
        for element in &self.elements {
            if let SymbolicCodeElement::Branch { target, .. } = element {
                self.check_label(target)?;
                let Some(&target_position) = label_positions.get(&target.index) else {
                    return Err(CodeBuilderError::UnboundLabel {
                        label: target.index,
                    });
                };
                if target_position == instruction_count {
                    return Err(CodeBuilderError::BranchToTerminalEnd {
                        label: target.index,
                    });
                }
            }
        }
        Ok(())
    }

    fn check_label(&self, label: &CodeLabel) -> Result<(), CodeBuilderError> {
        if !Arc::ptr_eq(&self.scope, &label.scope) {
            return Err(CodeBuilderError::ForeignLabel { label: label.index });
        }
        Ok(())
    }

    pub(crate) const fn label_index(label: &CodeLabel) -> usize {
        label.index
    }

    fn branch(
        &mut self,
        operation: SymbolicBranch,
        target: CodeLabel,
    ) -> Result<&mut Self, CodeBuilderError> {
        self.check_label(&target)?;
        self.elements
            .push(SymbolicCodeElement::Branch { operation, target });
        Ok(self)
    }

    /// Pushes an integer, selecting `iconst`, `bipush`, `sipush`, or `ldc`.
    pub fn iconst(&mut self, value: i32) -> Result<&mut Self, CodeBuilderError> {
        let instruction = match value {
            -1 => Some(Instruction::IConst(SmallIntConstant::MinusOne)),
            0 => Some(Instruction::IConst(SmallIntConstant::Zero)),
            1 => Some(Instruction::IConst(SmallIntConstant::One)),
            2 => Some(Instruction::IConst(SmallIntConstant::Two)),
            3 => Some(Instruction::IConst(SmallIntConstant::Three)),
            4 => Some(Instruction::IConst(SmallIntConstant::Four)),
            5 => Some(Instruction::IConst(SmallIntConstant::Five)),
            _ => None,
        };
        if let Some(instruction) = instruction {
            self.push(instruction);
        } else if let Ok(value) = i8::try_from(value) {
            self.push(Instruction::Bipush(value));
        } else if let Ok(value) = i16::try_from(value) {
            self.push(Instruction::Sipush(value));
        } else {
            self.elements.push(SymbolicCodeElement::Instruction(
                SymbolicInstruction::Integer(value),
            ));
        }
        Ok(self)
    }

    /// Pushes a long, using `lconst` for zero/one and `ldc2_w` otherwise.
    pub fn lconst(&mut self, value: i64) -> Result<&mut Self, CodeBuilderError> {
        match value {
            0 => self.push(Instruction::LConst(SmallLongConstant::Zero)),
            1 => self.push(Instruction::LConst(SmallLongConstant::One)),
            _ => self
                .elements
                .push(SymbolicCodeElement::Instruction(SymbolicInstruction::Long(
                    value,
                ))),
        }
        Ok(self)
    }

    /// Pushes a null reference.
    pub fn aconst_null(&mut self) -> Result<&mut Self, CodeBuilderError> {
        self.push(Instruction::AConstNull);
        Ok(self)
    }

    /// Loads an integer local.
    pub fn iload(&mut self, index: usize) -> Result<&mut Self, CodeBuilderError> {
        self.local(index, Instruction::ILoad)
    }

    /// Loads a long local.
    pub fn lload(&mut self, index: usize) -> Result<&mut Self, CodeBuilderError> {
        self.local(index, Instruction::LLoad)
    }

    /// Loads a reference local.
    pub fn aload(&mut self, index: usize) -> Result<&mut Self, CodeBuilderError> {
        self.local(index, Instruction::ALoad)
    }

    /// Stores an integer in a local.
    pub fn istore(&mut self, index: usize) -> Result<&mut Self, CodeBuilderError> {
        self.local(index, Instruction::IStore)
    }

    /// Stores a long in a local.
    pub fn lstore(&mut self, index: usize) -> Result<&mut Self, CodeBuilderError> {
        self.local(index, Instruction::LStore)
    }

    /// Stores a reference in a local.
    pub fn astore(&mut self, index: usize) -> Result<&mut Self, CodeBuilderError> {
        self.local(index, Instruction::AStore)
    }

    /// Adds two integers.
    pub fn iadd(&mut self) -> Result<&mut Self, CodeBuilderError> {
        self.push(Instruction::IAdd);
        Ok(self)
    }

    /// Subtracts the top integer from the next integer on the stack.
    pub fn isub(&mut self) -> Result<&mut Self, CodeBuilderError> {
        self.push(Instruction::ISub);
        Ok(self)
    }

    /// Multiplies the top two integers on the stack.
    pub fn imul(&mut self) -> Result<&mut Self, CodeBuilderError> {
        self.push(Instruction::IMul);
        Ok(self)
    }

    /// Divides the next integer on the stack by the top integer, truncating toward zero.
    pub fn idiv(&mut self) -> Result<&mut Self, CodeBuilderError> {
        self.push(Instruction::IDiv);
        Ok(self)
    }

    /// Computes the signed remainder of the next integer divided by the top integer.
    pub fn irem(&mut self) -> Result<&mut Self, CodeBuilderError> {
        self.push(Instruction::IRem);
        Ok(self)
    }

    /// Computes the 32-bit bitwise AND of the top two integers.
    pub fn iand(&mut self) -> Result<&mut Self, CodeBuilderError> {
        self.push(Instruction::IAnd);
        Ok(self)
    }

    /// Computes the 32-bit bitwise OR of the top two integers.
    pub fn ior(&mut self) -> Result<&mut Self, CodeBuilderError> {
        self.push(Instruction::IOr);
        Ok(self)
    }

    /// Computes the 32-bit bitwise XOR of the top two integers.
    pub fn ixor(&mut self) -> Result<&mut Self, CodeBuilderError> {
        self.push(Instruction::IXor);
        Ok(self)
    }

    /// Duplicates the top category-one value.
    pub fn dup(&mut self) -> Result<&mut Self, CodeBuilderError> {
        self.push(Instruction::Dup);
        Ok(self)
    }

    /// Removes the top category-one stack value.
    pub fn pop(&mut self) -> Result<&mut Self, CodeBuilderError> {
        self.push(Instruction::Pop);
        Ok(self)
    }

    /// Reads an instance field by symbolic owner, name, and descriptor.
    pub fn getfield(
        &mut self,
        owner: impl Into<String>,
        name: impl Into<String>,
        descriptor: impl Into<String>,
    ) -> Result<&mut Self, CodeBuilderError> {
        self.field(FieldOperation::Get, owner, name, descriptor)
    }

    /// Writes an instance field by symbolic owner, name, and descriptor.
    pub fn putfield(
        &mut self,
        owner: impl Into<String>,
        name: impl Into<String>,
        descriptor: impl Into<String>,
    ) -> Result<&mut Self, CodeBuilderError> {
        self.field(FieldOperation::Put, owner, name, descriptor)
    }

    /// Reads a static field by symbolic owner, name, and descriptor.
    pub fn getstatic(
        &mut self,
        owner: impl Into<String>,
        name: impl Into<String>,
        descriptor: impl Into<String>,
    ) -> Result<&mut Self, CodeBuilderError> {
        self.field(FieldOperation::GetStatic, owner, name, descriptor)
    }

    /// Writes a static field by symbolic owner, name, and descriptor.
    pub fn putstatic(
        &mut self,
        owner: impl Into<String>,
        name: impl Into<String>,
        descriptor: impl Into<String>,
    ) -> Result<&mut Self, CodeBuilderError> {
        self.field(FieldOperation::PutStatic, owner, name, descriptor)
    }

    /// Invokes a static method by symbolic owner, name, and descriptor.
    pub fn invokestatic(
        &mut self,
        owner: impl Into<String>,
        name: impl Into<String>,
        descriptor: impl Into<String>,
    ) -> Result<&mut Self, CodeBuilderError> {
        self.method(MethodOperation::Static, owner, name, descriptor)
    }

    /// Invokes a static method declared by an interface.
    pub fn invokestatic_interface(
        &mut self,
        owner: impl Into<String>,
        name: impl Into<String>,
        descriptor: impl Into<String>,
    ) -> Result<&mut Self, CodeBuilderError> {
        self.method(MethodOperation::StaticInterface, owner, name, descriptor)
    }

    /// Invokes a virtual method by symbolic owner, name, and descriptor.
    pub fn invokevirtual(
        &mut self,
        owner: impl Into<String>,
        name: impl Into<String>,
        descriptor: impl Into<String>,
    ) -> Result<&mut Self, CodeBuilderError> {
        self.method(MethodOperation::Virtual, owner, name, descriptor)
    }

    /// Invokes a special method, commonly a constructor or superclass method.
    pub fn invokespecial(
        &mut self,
        owner: impl Into<String>,
        name: impl Into<String>,
        descriptor: impl Into<String>,
    ) -> Result<&mut Self, CodeBuilderError> {
        self.method(MethodOperation::Special, owner, name, descriptor)
    }

    /// Invokes a special method declared by an interface.
    pub fn invokespecial_interface(
        &mut self,
        owner: impl Into<String>,
        name: impl Into<String>,
        descriptor: impl Into<String>,
    ) -> Result<&mut Self, CodeBuilderError> {
        self.method(MethodOperation::SpecialInterface, owner, name, descriptor)
    }

    /// Allocates an instance of a symbolic class.
    pub fn new_object(&mut self, owner: impl Into<String>) -> Result<&mut Self, CodeBuilderError> {
        let owner = owner.into();
        validate_internal_name(&owner)?;
        self.elements
            .push(SymbolicCodeElement::Instruction(SymbolicInstruction::New(
                owner,
            )));
        Ok(self)
    }

    /// Returns from a void method.
    pub fn return_void(&mut self) -> Result<&mut Self, CodeBuilderError> {
        self.push(Instruction::Return);
        Ok(self)
    }

    /// Returns an integer from a method.
    pub fn ireturn(&mut self) -> Result<&mut Self, CodeBuilderError> {
        self.push(Instruction::IReturn);
        Ok(self)
    }

    /// Returns a reference from a method.
    pub fn areturn(&mut self) -> Result<&mut Self, CodeBuilderError> {
        self.push(Instruction::AReturn);
        Ok(self)
    }

    /// Returns a long from a method.
    pub fn lreturn(&mut self) -> Result<&mut Self, CodeBuilderError> {
        self.push(Instruction::LReturn);
        Ok(self)
    }

    fn push(&mut self, instruction: Instruction) {
        self.elements.push(SymbolicCodeElement::Instruction(
            SymbolicInstruction::Typed(instruction),
        ));
    }

    fn local(
        &mut self,
        index: usize,
        instruction: fn(LocalIndex) -> Instruction,
    ) -> Result<&mut Self, CodeBuilderError> {
        let index = u16::try_from(index)
            .map(LocalIndex::new)
            .map_err(|_| CodeBuilderError::LocalIndexOutOfRange { index })?;
        self.push(instruction(index));
        Ok(self)
    }

    fn field(
        &mut self,
        operation: FieldOperation,
        owner: impl Into<String>,
        name: impl Into<String>,
        descriptor: impl Into<String>,
    ) -> Result<&mut Self, CodeBuilderError> {
        let owner = owner.into();
        let name = name.into();
        let descriptor = descriptor.into();
        validate_internal_name(&owner)?;
        validate_unqualified_name(&name, false)?;
        let ty = FieldType::parse(&descriptor).map_err(CodeBuilderError::InvalidDescriptor)?;
        validate_field_type_names(&ty)?;
        self.elements.push(SymbolicCodeElement::Instruction(
            SymbolicInstruction::Field {
                operation,
                owner,
                name,
                descriptor,
            },
        ));
        Ok(self)
    }

    fn method(
        &mut self,
        operation: MethodOperation,
        owner: impl Into<String>,
        name: impl Into<String>,
        descriptor: impl Into<String>,
    ) -> Result<&mut Self, CodeBuilderError> {
        let owner = owner.into();
        let name = name.into();
        let descriptor = descriptor.into();
        validate_internal_name(&owner)?;
        validate_unqualified_name(&name, true)?;
        let parsed =
            MethodDescriptor::parse(&descriptor).map_err(CodeBuilderError::InvalidDescriptor)?;
        for parameter in &parsed.parameters {
            validate_field_type_names(parameter)?;
        }
        if let Some(result) = &parsed.return_type {
            validate_field_type_names(result)?;
        }
        let has_receiver = !matches!(
            operation,
            MethodOperation::Static | MethodOperation::StaticInterface
        );
        let slots = parsed
            .parameters
            .iter()
            .fold(usize::from(has_receiver), |slots, ty| {
                slots
                    + if matches!(ty, FieldType::Long | FieldType::Double) {
                        2
                    } else {
                        1
                    }
            });
        if slots > 255 {
            return Err(CodeBuilderError::TooManyParameterSlots);
        }
        if name == "<clinit>"
            || (name == "<init>"
                && (!matches!(operation, MethodOperation::Special) || parsed.return_type.is_some()))
        {
            return Err(CodeBuilderError::InvalidName);
        }
        self.elements.push(SymbolicCodeElement::Instruction(
            SymbolicInstruction::Method {
                operation,
                owner,
                name,
                descriptor,
            },
        ));
        Ok(self)
    }
}

fn validate_internal_name(name: &str) -> Result<(), CodeBuilderError> {
    if name.is_empty()
        || name
            .split('/')
            .any(|part| part.is_empty() || part.chars().any(|c| matches!(c, '.' | ';' | '[')))
    {
        Err(CodeBuilderError::InvalidName)
    } else {
        Ok(())
    }
}

fn validate_unqualified_name(name: &str, method: bool) -> Result<(), CodeBuilderError> {
    if name.is_empty()
        || name.chars().any(|c| matches!(c, '.' | ';' | '[' | '/'))
        || (method
            && name.chars().any(|c| matches!(c, '<' | '>'))
            && !matches!(name, "<init>" | "<clinit>"))
    {
        Err(CodeBuilderError::InvalidName)
    } else {
        Ok(())
    }
}

fn validate_field_type_names(ty: &FieldType) -> Result<(), CodeBuilderError> {
    match ty {
        FieldType::Object(name) => validate_internal_name(name),
        FieldType::Array(component) => validate_field_type_names(component),
        FieldType::Byte
        | FieldType::Char
        | FieldType::Double
        | FieldType::Float
        | FieldType::Int
        | FieldType::Long
        | FieldType::Short
        | FieldType::Boolean => Ok(()),
    }
}

pub(crate) fn operand(
    index: ConstantPoolIndex,
) -> Result<ConstantPoolIndexOperand, CodeBuilderError> {
    ConstantPoolIndexOperand::new(index)
        .map_err(|_| CodeBuilderError::InvalidConstantPoolIndex { index: index.0 })
}

pub(crate) fn typed_field_reference(
    index: ConstantPoolIndex,
) -> Result<FieldReference, CodeBuilderError> {
    Ok(FieldReference::new(operand(index)?))
}

pub(crate) fn typed_class_reference(
    index: ConstantPoolIndex,
) -> Result<ClassReference, CodeBuilderError> {
    Ok(ClassReference::new(operand(index)?))
}

pub(crate) fn typed_method_reference(
    operation: MethodOperation,
    index: ConstantPoolIndex,
) -> Result<Instruction, CodeBuilderError> {
    let operand = operand(index)?;
    Ok(match operation {
        MethodOperation::Static | MethodOperation::StaticInterface => {
            Instruction::InvokeStatic(StaticMethodReference::new(operand))
        }
        MethodOperation::Virtual => {
            Instruction::InvokeVirtual(VirtualMethodReference::new(operand))
        }
        MethodOperation::Special | MethodOperation::SpecialInterface => {
            Instruction::InvokeSpecial(SpecialMethodReference::new(operand))
        }
    })
}

pub(crate) fn integer_load(index: ConstantPoolIndex) -> Result<Instruction, CodeBuilderError> {
    Ok(Instruction::Ldc(LdcConstant::Integer(operand(index)?)))
}

pub(crate) fn long_load(index: ConstantPoolIndex) -> Result<Instruction, CodeBuilderError> {
    Ok(Instruction::Ldc2(Ldc2Constant::Long(operand(index)?)))
}

#[cfg(test)]
mod tests {
    use super::{CodeBuilder, SymbolicCodeElement, SymbolicInstruction};
    use crate::bytecode::{Instruction, SmallIntConstant};

    #[test]
    fn pop_appends_one_deterministic_typed_instruction() {
        let mut first = CodeBuilder::new();
        first
            .iconst(1)
            .unwrap()
            .pop()
            .unwrap()
            .return_void()
            .unwrap();

        assert!(matches!(
            first.elements.as_slice(),
            [
                SymbolicCodeElement::Instruction(SymbolicInstruction::Typed(Instruction::IConst(
                    SmallIntConstant::One
                ))),
                SymbolicCodeElement::Instruction(SymbolicInstruction::Typed(Instruction::Pop)),
                SymbolicCodeElement::Instruction(SymbolicInstruction::Typed(Instruction::Return)),
            ]
        ));

        let mut second = CodeBuilder::new();
        second
            .iconst(1)
            .unwrap()
            .pop()
            .unwrap()
            .return_void()
            .unwrap();
        assert!(matches!(
            second.elements.as_slice(),
            [
                SymbolicCodeElement::Instruction(SymbolicInstruction::Typed(Instruction::IConst(
                    SmallIntConstant::One
                ))),
                SymbolicCodeElement::Instruction(SymbolicInstruction::Typed(Instruction::Pop)),
                SymbolicCodeElement::Instruction(SymbolicInstruction::Typed(Instruction::Return)),
            ]
        ));
    }

    #[test]
    fn integer_arithmetic_methods_append_exact_typed_instructions() {
        let mut builder = CodeBuilder::new();
        builder
            .isub()
            .unwrap()
            .imul()
            .unwrap()
            .idiv()
            .unwrap()
            .irem()
            .unwrap();

        assert!(matches!(
            builder.elements.as_slice(),
            [
                SymbolicCodeElement::Instruction(SymbolicInstruction::Typed(Instruction::ISub)),
                SymbolicCodeElement::Instruction(SymbolicInstruction::Typed(Instruction::IMul)),
                SymbolicCodeElement::Instruction(SymbolicInstruction::Typed(Instruction::IDiv)),
                SymbolicCodeElement::Instruction(SymbolicInstruction::Typed(Instruction::IRem)),
            ]
        ));
    }

    #[test]
    fn int_bitwise_methods_append_exact_typed_instructions() {
        let mut builder = CodeBuilder::new();
        builder.iand().unwrap().ior().unwrap().ixor().unwrap();
        assert!(matches!(
            builder.elements.as_slice(),
            [
                SymbolicCodeElement::Instruction(SymbolicInstruction::Typed(Instruction::IAnd)),
                SymbolicCodeElement::Instruction(SymbolicInstruction::Typed(Instruction::IOr)),
                SymbolicCodeElement::Instruction(SymbolicInstruction::Typed(Instruction::IXor)),
            ]
        ));
    }
}
