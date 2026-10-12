//! Symbolic builder for the supported straight-line JVM instruction subset.

use super::{
    ClassReference, ConstantPoolIndexOperand, FieldReference, Instruction, Ldc2Constant,
    LdcConstant, LocalIndex, SmallIntConstant, SmallLongConstant, SpecialMethodReference,
    StaticMethodReference, VirtualMethodReference,
};
use crate::constant_pool::ConstantPoolIndex;
use crate::descriptor::{DescriptorError, FieldType, MethodDescriptor};
use std::fmt;

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

/// A source of typed instructions and symbolic JVM references for one method.
///
/// Constant-pool indices are assigned by [`crate::class_file_builder::ClassFileBuilder`]
/// when the class is encoded. The builder intentionally exposes no raw opcode
/// or constant-pool-index operation.
#[derive(Debug, Clone, Default)]
pub struct CodeBuilder {
    pub(crate) instructions: Vec<SymbolicInstruction>,
}

impl CodeBuilder {
    /// Creates an empty instruction builder.
    pub fn new() -> Self {
        Self::default()
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
            self.instructions.push(SymbolicInstruction::Integer(value));
        }
        Ok(self)
    }

    /// Pushes a long, using `lconst` for zero/one and `ldc2_w` otherwise.
    pub fn lconst(&mut self, value: i64) -> Result<&mut Self, CodeBuilderError> {
        match value {
            0 => self.push(Instruction::LConst(SmallLongConstant::Zero)),
            1 => self.push(Instruction::LConst(SmallLongConstant::One)),
            _ => self.instructions.push(SymbolicInstruction::Long(value)),
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

    /// Duplicates the top category-one value.
    pub fn dup(&mut self) -> Result<&mut Self, CodeBuilderError> {
        self.push(Instruction::Dup);
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
        self.instructions.push(SymbolicInstruction::New(owner));
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
        self.instructions
            .push(SymbolicInstruction::Typed(instruction));
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
        self.instructions.push(SymbolicInstruction::Field {
            operation,
            owner,
            name,
            descriptor,
        });
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
                && (!matches!(
                    operation,
                    MethodOperation::Special | MethodOperation::SpecialInterface
                ) || parsed.return_type.is_some()))
        {
            return Err(CodeBuilderError::InvalidName);
        }
        self.instructions.push(SymbolicInstruction::Method {
            operation,
            owner,
            name,
            descriptor,
        });
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
