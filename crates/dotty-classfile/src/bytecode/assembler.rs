//! Encoding for the typed straight-line bytecode subset.
//!
//! `assemble` uses existing constant-pool indices and never mutates the pool.
//! It returns a complete code array or a typed error; partial bytes are never
//! exposed. Local forms are selected from the instruction's `LocalIndex`, and
//! `ldc` selects `ldc` or `ldc_w` from its actual pool index.

use super::{
    ClassReference, ConstantPoolIndexOperand, FieldReference, Instruction, Ldc2Constant,
    LdcConstant, LocalIndex, SpecialMethodReference, StaticMethodReference, VirtualMethodReference,
};
use crate::constant_pool::{ConstantPool, ConstantPoolEntry, ConstantPoolIndex, PoolRefError};
use crate::descriptor::{DescriptorError, FieldType};
use crate::encode_error::EncodeError;
use crate::writer::Writer;
use std::fmt;

/// The expected constant-pool tag family for an instruction operand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstantPoolOperandKind {
    /// A `CONSTANT_Class` entry.
    Class,
    /// A `CONSTANT_Fieldref` entry.
    FieldReference,
    /// A `CONSTANT_Methodref` entry.
    VirtualMethodReference,
    /// A `CONSTANT_Methodref` or `CONSTANT_InterfaceMethodref` entry.
    StaticOrSpecialMethodReference,
    /// A `CONSTANT_Integer` entry.
    IntegerConstant,
    /// A category-one reference-valued loadable constant.
    ReferenceConstant,
    /// A `CONSTANT_Long` entry.
    LongConstant,
}

impl fmt::Display for ConstantPoolOperandKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Class => "a Class entry",
            Self::FieldReference => "a Fieldref entry",
            Self::VirtualMethodReference => "a Methodref entry",
            Self::StaticOrSpecialMethodReference => "a Methodref or InterfaceMethodref entry",
            Self::IntegerConstant => "an Integer entry",
            Self::ReferenceConstant => "a category-one reference constant",
            Self::LongConstant => "a Long entry",
        })
    }
}

/// A typed failure while assembling a JVM instruction sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssembleError {
    /// The constant pool itself has an invalid layout or reference.
    InvalidConstantPool(EncodeError),
    /// An instruction operand is missing or has an incompatible constant-pool
    /// tag.
    InvalidConstantPoolOperand {
        /// The instruction's supplied constant-pool index.
        index: ConstantPoolIndex,
        /// The entry kind required by the instruction operand.
        expected: ConstantPoolOperandKind,
    },
    /// A nested constant-pool reference was malformed while checking a
    /// dynamic constant's field descriptor.
    InvalidDynamicConstantReference(PoolRefError),
    /// A dynamic constant does not contain a valid field descriptor.
    InvalidDynamicConstantDescriptor {
        /// The `CONSTANT_Dynamic` entry's index.
        index: ConstantPoolIndex,
        /// The descriptor parse failure.
        error: DescriptorError,
    },
    /// The assembled code array is empty or longer than the JVMS `code_length`
    /// limit of 65535 bytes.
    InvalidCodeLength {
        /// The final code length, or the first length that exceeds the limit.
        length: usize,
    },
}

impl fmt::Display for AssembleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConstantPool(error) => {
                write!(formatter, "invalid constant pool: {error}")
            }
            Self::InvalidConstantPoolOperand { index, expected } => write!(
                formatter,
                "constant-pool index {} does not resolve to {expected}",
                index.0
            ),
            Self::InvalidDynamicConstantReference(error) => {
                write!(
                    formatter,
                    "invalid dynamic-constant descriptor reference: {error}"
                )
            }
            Self::InvalidDynamicConstantDescriptor { index, error } => write!(
                formatter,
                "dynamic constant at index {} has an invalid descriptor: {error}",
                index.0
            ),
            Self::InvalidCodeLength { length } => write!(
                formatter,
                "assembled code length {length} is outside the JVM range 1..=65535"
            ),
        }
    }
}

impl std::error::Error for AssembleError {}

/// Assembles typed instructions into JVM code bytes.
///
/// The supplied pool is treated as authoritative and is validated before any
/// instructions are written. Symbolic references must already have been
/// interned; this function never changes constant-pool indices. No stack
/// analysis, descriptor-based frame computation, branch encoding, or general
/// bytecode verification is performed.
///
/// ```
/// use dotty_classfile::classfile::{
///     assemble, ConstantPoolBuilder, ConstantPoolIndexOperand, Instruction,
///     LdcConstant, SmallIntConstant,
/// };
///
/// let mut pool_builder = ConstantPoolBuilder::new();
/// let integer = pool_builder.integer(42)?;
/// let pool = pool_builder.finish()?;
/// let integer = ConstantPoolIndexOperand::try_from(integer)?;
/// let code = assemble(
///     &[Instruction::Ldc(LdcConstant::Integer(integer)), Instruction::IReturn],
///     &pool,
/// )?;
/// assert_eq!(code, [0x12, 1, 0xac]);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn assemble(
    instructions: &[Instruction],
    constant_pool: &ConstantPool,
) -> Result<Vec<u8>, AssembleError> {
    constant_pool
        .validate_for_encoding()
        .map_err(AssembleError::InvalidConstantPool)?;

    let mut writer = Writer::new();
    for instruction in instructions {
        write_instruction(&mut writer, *instruction, constant_pool)?;
        if writer.len() > usize::from(u16::MAX) {
            return Err(AssembleError::InvalidCodeLength {
                length: writer.len(),
            });
        }
    }
    if writer.is_empty() {
        return Err(AssembleError::InvalidCodeLength { length: 0 });
    }
    Ok(writer.into_bytes())
}

fn write_instruction(
    writer: &mut Writer,
    instruction: Instruction,
    constant_pool: &ConstantPool,
) -> Result<(), AssembleError> {
    match instruction {
        Instruction::AConstNull => writer.write_u1(0x01),
        Instruction::IConst(value) => writer.write_u1(match value {
            super::SmallIntConstant::MinusOne => 0x02,
            super::SmallIntConstant::Zero => 0x03,
            super::SmallIntConstant::One => 0x04,
            super::SmallIntConstant::Two => 0x05,
            super::SmallIntConstant::Three => 0x06,
            super::SmallIntConstant::Four => 0x07,
            super::SmallIntConstant::Five => 0x08,
        }),
        Instruction::Bipush(value) => {
            writer.write_u1(0x10);
            writer.write_u1(value.to_be_bytes()[0]);
        }
        Instruction::Sipush(value) => {
            writer.write_u1(0x11);
            writer.write_bytes(&value.to_be_bytes());
        }
        Instruction::LConst(value) => writer.write_u1(match value {
            super::SmallLongConstant::Zero => 0x09,
            super::SmallLongConstant::One => 0x0a,
        }),
        Instruction::Ldc(value) => write_ldc(writer, value, constant_pool)?,
        Instruction::Ldc2(value) => write_ldc2(writer, value, constant_pool)?,
        Instruction::ILoad(index) => write_local(writer, index, 0x15, 0x1a),
        Instruction::LLoad(index) => write_local(writer, index, 0x16, 0x1e),
        Instruction::ALoad(index) => write_local(writer, index, 0x19, 0x2a),
        Instruction::IStore(index) => write_local(writer, index, 0x36, 0x3b),
        Instruction::LStore(index) => write_local(writer, index, 0x37, 0x3f),
        Instruction::AStore(index) => write_local(writer, index, 0x3a, 0x4b),
        Instruction::IAdd => writer.write_u1(0x60),
        Instruction::Dup => writer.write_u1(0x59),
        Instruction::GetField(reference) => {
            write_reference(
                writer,
                reference,
                constant_pool,
                ConstantPoolOperandKind::FieldReference,
                is_field_ref,
                0xb4,
            )?;
        }
        Instruction::PutField(reference) => {
            write_reference(
                writer,
                reference,
                constant_pool,
                ConstantPoolOperandKind::FieldReference,
                is_field_ref,
                0xb5,
            )?;
        }
        Instruction::GetStatic(reference) => {
            write_reference(
                writer,
                reference,
                constant_pool,
                ConstantPoolOperandKind::FieldReference,
                is_field_ref,
                0xb2,
            )?;
        }
        Instruction::PutStatic(reference) => {
            write_reference(
                writer,
                reference,
                constant_pool,
                ConstantPoolOperandKind::FieldReference,
                is_field_ref,
                0xb3,
            )?;
        }
        Instruction::InvokeVirtual(reference) => {
            write_reference(
                writer,
                reference,
                constant_pool,
                ConstantPoolOperandKind::VirtualMethodReference,
                is_method_ref,
                0xb6,
            )?;
        }
        Instruction::InvokeSpecial(reference) => {
            write_reference(
                writer,
                reference,
                constant_pool,
                ConstantPoolOperandKind::StaticOrSpecialMethodReference,
                is_method_or_interface_method_ref,
                0xb7,
            )?;
        }
        Instruction::InvokeStatic(reference) => {
            write_reference(
                writer,
                reference,
                constant_pool,
                ConstantPoolOperandKind::StaticOrSpecialMethodReference,
                is_method_or_interface_method_ref,
                0xb8,
            )?;
        }
        Instruction::New(reference) => {
            write_reference(
                writer,
                reference,
                constant_pool,
                ConstantPoolOperandKind::Class,
                is_class,
                0xbb,
            )?;
        }
        Instruction::Return => writer.write_u1(0xb1),
        Instruction::IReturn => writer.write_u1(0xac),
        Instruction::AReturn => writer.write_u1(0xb0),
        Instruction::LReturn => writer.write_u1(0xad),
    }
    Ok(())
}

fn write_local(writer: &mut Writer, index: LocalIndex, opcode: u8, short_opcode: u8) {
    match index.get() {
        0 => writer.write_u1(short_opcode),
        1 => writer.write_u1(short_opcode + 1),
        2 => writer.write_u1(short_opcode + 2),
        3 => writer.write_u1(short_opcode + 3),
        value if value <= u16::from(u8::MAX) => {
            writer.write_u1(opcode);
            writer.write_u1(index.get().to_be_bytes()[1]);
        }
        value => {
            writer.write_u1(0xc4);
            writer.write_u1(opcode);
            writer.write_u2(value);
        }
    }
}

fn write_ldc(
    writer: &mut Writer,
    constant: LdcConstant,
    pool: &ConstantPool,
) -> Result<(), AssembleError> {
    let (operand, expected) = match constant {
        LdcConstant::Integer(index) => (index, ConstantPoolOperandKind::IntegerConstant),
        LdcConstant::Reference(index) => (index, ConstantPoolOperandKind::ReferenceConstant),
    };
    let index = operand.get();
    let entry = pool
        .get(index)
        .ok_or(AssembleError::InvalidConstantPoolOperand { index, expected })?;
    let valid = match constant {
        LdcConstant::Integer(_) => matches!(entry, ConstantPoolEntry::Integer(_)),
        LdcConstant::Reference(_) => match entry {
            ConstantPoolEntry::String { .. }
            | ConstantPoolEntry::Class { .. }
            | ConstantPoolEntry::MethodHandle { .. }
            | ConstantPoolEntry::MethodType { .. } => true,
            ConstantPoolEntry::Dynamic {
                name_and_type_index,
                ..
            } => {
                dynamic_value_kind(pool, index, *name_and_type_index)?
                    == Some(super::JvmValueKind::Reference)
            }
            _ => false,
        },
    };
    if !valid {
        return Err(AssembleError::InvalidConstantPoolOperand { index, expected });
    }
    write_ldc_index(writer, index);
    Ok(())
}

fn write_ldc2(
    writer: &mut Writer,
    constant: Ldc2Constant,
    pool: &ConstantPool,
) -> Result<(), AssembleError> {
    let operand = constant.index();
    let index = operand.get();
    let entry = pool
        .get(index)
        .ok_or(AssembleError::InvalidConstantPoolOperand {
            index,
            expected: ConstantPoolOperandKind::LongConstant,
        })?;
    let valid = match (constant, entry) {
        (Ldc2Constant::Long(_), ConstantPoolEntry::Long(_)) => true,
        (
            Ldc2Constant::Long(_),
            ConstantPoolEntry::Dynamic {
                name_and_type_index,
                ..
            },
        ) => {
            dynamic_value_kind(pool, index, *name_and_type_index)?
                == Some(super::JvmValueKind::Long)
        }
        _ => false,
    };
    if !valid {
        return Err(AssembleError::InvalidConstantPoolOperand {
            index,
            expected: ConstantPoolOperandKind::LongConstant,
        });
    }
    writer.write_u1(0x14);
    writer.write_u2(index.0);
    Ok(())
}

fn write_ldc_index(writer: &mut Writer, index: ConstantPoolIndex) {
    if index.0 <= u8::MAX.into() {
        writer.write_u1(0x12);
        writer.write_u1(index.0.to_be_bytes()[1]);
    } else {
        writer.write_u1(0x13);
        writer.write_u2(index.0);
    }
}

fn dynamic_value_kind(
    pool: &ConstantPool,
    index: ConstantPoolIndex,
    name_and_type_index: ConstantPoolIndex,
) -> Result<Option<super::JvmValueKind>, AssembleError> {
    let (_, descriptor) = pool
        .name_and_type(name_and_type_index)
        .map_err(AssembleError::InvalidDynamicConstantReference)?;
    let field_type = FieldType::parse(descriptor)
        .map_err(|error| AssembleError::InvalidDynamicConstantDescriptor { index, error })?;
    Ok(match field_type {
        FieldType::Long => Some(super::JvmValueKind::Long),
        FieldType::Double | FieldType::Float => None,
        FieldType::Object(_) | FieldType::Array(_) => Some(super::JvmValueKind::Reference),
        FieldType::Boolean
        | FieldType::Byte
        | FieldType::Char
        | FieldType::Int
        | FieldType::Short => Some(super::JvmValueKind::Int),
    })
}

fn write_reference<T>(
    writer: &mut Writer,
    reference: T,
    pool: &ConstantPool,
    expected: ConstantPoolOperandKind,
    accepts: fn(&ConstantPoolEntry) -> bool,
    opcode: u8,
) -> Result<(), AssembleError>
where
    T: Into<ConstantPoolIndexOperand>,
{
    let index = reference.into().get();
    match pool.get(index) {
        Some(entry) if accepts(entry) => {
            writer.write_u1(opcode);
            writer.write_u2(index.0);
            Ok(())
        }
        _ => Err(AssembleError::InvalidConstantPoolOperand { index, expected }),
    }
}

impl From<ClassReference> for ConstantPoolIndexOperand {
    fn from(reference: ClassReference) -> Self {
        reference.index()
    }
}

impl From<FieldReference> for ConstantPoolIndexOperand {
    fn from(reference: FieldReference) -> Self {
        reference.index()
    }
}

impl From<StaticMethodReference> for ConstantPoolIndexOperand {
    fn from(reference: StaticMethodReference) -> Self {
        reference.index()
    }
}

impl From<VirtualMethodReference> for ConstantPoolIndexOperand {
    fn from(reference: VirtualMethodReference) -> Self {
        reference.index()
    }
}

impl From<SpecialMethodReference> for ConstantPoolIndexOperand {
    fn from(reference: SpecialMethodReference) -> Self {
        reference.index()
    }
}

fn is_class(entry: &ConstantPoolEntry) -> bool {
    matches!(entry, ConstantPoolEntry::Class { .. })
}

fn is_field_ref(entry: &ConstantPoolEntry) -> bool {
    matches!(entry, ConstantPoolEntry::Fieldref { .. })
}

fn is_method_ref(entry: &ConstantPoolEntry) -> bool {
    matches!(entry, ConstantPoolEntry::Methodref { .. })
}

fn is_method_or_interface_method_ref(entry: &ConstantPoolEntry) -> bool {
    matches!(
        entry,
        ConstantPoolEntry::Methodref { .. } | ConstantPoolEntry::InterfaceMethodref { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::{
        ClassReference, ConstantPoolIndexOperand, FieldReference, Ldc2Constant, LdcConstant,
        SmallIntConstant, SmallLongConstant, SpecialMethodReference, StaticMethodReference,
        VirtualMethodReference,
    };
    use crate::constant_pool_builder::ConstantPoolBuilder;

    fn operand(index: ConstantPoolIndex) -> ConstantPoolIndexOperand {
        ConstantPoolIndexOperand::try_from(index).unwrap()
    }

    #[test]
    fn encodes_constants_with_exact_opcodes_and_signed_boundaries() {
        let mut pool_builder = ConstantPoolBuilder::new();
        let integer = pool_builder.integer(123456).unwrap();
        let long = pool_builder.long(i64::MIN).unwrap();
        let pool = pool_builder.finish().unwrap();
        let instructions = [
            Instruction::AConstNull,
            Instruction::IConst(SmallIntConstant::MinusOne),
            Instruction::IConst(SmallIntConstant::Five),
            Instruction::Bipush(i8::MIN),
            Instruction::Bipush(i8::MAX),
            Instruction::Sipush(i16::MIN),
            Instruction::Sipush(i16::MAX),
            Instruction::LConst(SmallLongConstant::Zero),
            Instruction::LConst(SmallLongConstant::One),
            Instruction::Ldc(LdcConstant::Integer(operand(integer))),
            Instruction::Ldc2(Ldc2Constant::Long(operand(long))),
        ];
        assert_eq!(
            assemble(&instructions, &pool).unwrap(),
            [
                0x01,
                0x02,
                0x08,
                0x10,
                0x80,
                0x10,
                0x7f,
                0x11,
                0x80,
                0x00,
                0x11,
                0x7f,
                0xff,
                0x09,
                0x0a,
                0x12,
                integer.0.to_be_bytes()[1],
                0x14,
                long.0.to_be_bytes()[0],
                long.0.to_be_bytes()[1],
            ]
        );
    }

    #[test]
    fn encodes_ldc_using_u1_then_u2_indices_and_long_with_ldc2_w() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Integer(0)); 255]);
        assert_eq!(
            assemble(
                &[Instruction::Ldc(LdcConstant::Integer(operand(
                    ConstantPoolIndex(255)
                )))],
                &pool,
            )
            .unwrap(),
            [0x12, 0xff]
        );
        let entries = vec![Some(ConstantPoolEntry::Integer(0)); 256];
        let pool = ConstantPool::from_entries(entries);
        assert_eq!(
            assemble(
                &[Instruction::Ldc(LdcConstant::Integer(operand(
                    ConstantPoolIndex(256)
                )))],
                &pool,
            )
            .unwrap(),
            [0x13, 0x01, 0x00]
        );
    }

    #[test]
    fn encodes_local_forms_at_compact_narrow_and_wide_boundaries() {
        let pool = ConstantPool::default();
        let instructions = [
            Instruction::ILoad(LocalIndex::new(0)),
            Instruction::ILoad(LocalIndex::new(3)),
            Instruction::ILoad(LocalIndex::new(4)),
            Instruction::ILoad(LocalIndex::new(255)),
            Instruction::ILoad(LocalIndex::new(256)),
            Instruction::LLoad(LocalIndex::new(65535)),
            Instruction::ALoad(LocalIndex::new(2)),
            Instruction::IStore(LocalIndex::new(1)),
            Instruction::LStore(LocalIndex::new(256)),
            Instruction::AStore(LocalIndex::new(255)),
        ];
        assert_eq!(
            assemble(&instructions, &pool).unwrap(),
            [
                0x1a, 0x1d, 0x15, 0x04, 0x15, 0xff, 0xc4, 0x15, 0x01, 0x00, 0xc4, 0x16, 0xff, 0xff,
                0x2c, 0x3c, 0xc4, 0x37, 0x01, 0x00, 0x3a, 0xff,
            ]
        );
    }

    #[test]
    fn encodes_fields_calls_allocation_and_return_opcodes() {
        let mut pool_builder = ConstantPoolBuilder::new();
        let class = pool_builder.class("example/C").unwrap();
        let field = pool_builder.field_ref("example/C", "value", "I").unwrap();
        let method = pool_builder.method_ref("example/C", "run", "()V").unwrap();
        let interface_method = pool_builder
            .interface_method_ref("example/Api", "run", "()V")
            .unwrap();
        let pool = pool_builder.finish().unwrap();
        let instructions = [
            Instruction::IAdd,
            Instruction::Dup,
            Instruction::GetStatic(FieldReference::new(operand(field))),
            Instruction::PutStatic(FieldReference::new(operand(field))),
            Instruction::GetField(FieldReference::new(operand(field))),
            Instruction::PutField(FieldReference::new(operand(field))),
            Instruction::InvokeVirtual(VirtualMethodReference::new(operand(method))),
            Instruction::InvokeSpecial(SpecialMethodReference::new(operand(interface_method))),
            Instruction::InvokeStatic(StaticMethodReference::new(operand(method))),
            Instruction::New(ClassReference::new(operand(class))),
            Instruction::Return,
            Instruction::IReturn,
            Instruction::AReturn,
            Instruction::LReturn,
        ];
        let actual = assemble(&instructions, &pool).unwrap();
        let mut expected = vec![0x60, 0x59];
        for (opcode, index) in [
            (0xb2, field),
            (0xb3, field),
            (0xb4, field),
            (0xb5, field),
            (0xb6, method),
            (0xb7, interface_method),
            (0xb8, method),
            (0xbb, class),
        ] {
            expected.push(opcode);
            expected.extend_from_slice(&index.0.to_be_bytes());
        }
        expected.extend_from_slice(&[0xb1, 0xac, 0xb0, 0xad]);
        assert_eq!(actual, expected);
    }

    #[test]
    fn rejects_missing_and_wrong_kind_constant_pool_operands() {
        let mut pool_builder = ConstantPoolBuilder::new();
        let integer = pool_builder.integer(1).unwrap();
        let pool = pool_builder.finish().unwrap();

        assert_eq!(
            assemble(
                &[Instruction::New(ClassReference::new(operand(integer)))],
                &pool,
            ),
            Err(AssembleError::InvalidConstantPoolOperand {
                index: integer,
                expected: ConstantPoolOperandKind::Class,
            })
        );
        assert_eq!(
            assemble(
                &[Instruction::GetField(FieldReference::new(operand(
                    ConstantPoolIndex(2)
                )))],
                &pool,
            ),
            Err(AssembleError::InvalidConstantPoolOperand {
                index: ConstantPoolIndex(2),
                expected: ConstantPoolOperandKind::FieldReference,
            })
        );
        assert_eq!(
            assemble(
                &[Instruction::Ldc2(Ldc2Constant::Long(operand(integer)))],
                &pool,
            ),
            Err(AssembleError::InvalidConstantPoolOperand {
                index: integer,
                expected: ConstantPoolOperandKind::LongConstant,
            })
        );
    }

    #[test]
    fn rejects_ldc_constants_with_incompatible_category() {
        let mut pool_builder = ConstantPoolBuilder::new();
        let class = pool_builder.class("example/C").unwrap();
        let float = pool_builder.float(1.5).unwrap();
        let double = pool_builder.double(2.5).unwrap();
        let pool = pool_builder.finish().unwrap();
        assert_eq!(
            assemble(
                &[Instruction::Ldc(LdcConstant::Integer(operand(class)))],
                &pool,
            ),
            Err(AssembleError::InvalidConstantPoolOperand {
                index: class,
                expected: ConstantPoolOperandKind::IntegerConstant,
            })
        );
        assert_eq!(
            assemble(
                &[Instruction::Ldc(LdcConstant::Reference(operand(float)))],
                &pool,
            ),
            Err(AssembleError::InvalidConstantPoolOperand {
                index: float,
                expected: ConstantPoolOperandKind::ReferenceConstant,
            })
        );
        assert_eq!(
            assemble(
                &[Instruction::Ldc2(Ldc2Constant::Long(operand(double)))],
                &pool,
            ),
            Err(AssembleError::InvalidConstantPoolOperand {
                index: double,
                expected: ConstantPoolOperandKind::LongConstant,
            })
        );
    }

    #[test]
    fn enforces_code_length_and_returns_stable_complete_output() {
        let pool = ConstantPool::default();
        assert_eq!(
            assemble(&[], &pool),
            Err(AssembleError::InvalidCodeLength { length: 0 })
        );
        let maximum = vec![Instruction::AConstNull; usize::from(u16::MAX)];
        let bytes = assemble(&maximum, &pool).unwrap();
        assert_eq!(bytes.len(), usize::from(u16::MAX));
        assert!(bytes.iter().all(|byte| *byte == 0x01));
        let overflow = vec![Instruction::AConstNull; usize::from(u16::MAX) + 1];
        assert_eq!(
            assemble(&overflow, &pool),
            Err(AssembleError::InvalidCodeLength { length: 65536 })
        );
        assert_eq!(
            assemble(&[Instruction::Bipush(42), Instruction::IReturn], &pool).unwrap(),
            [0x10, 42, 0xac]
        );
        assert_eq!(
            assemble(&[Instruction::Bipush(42), Instruction::IReturn], &pool).unwrap(),
            assemble(&[Instruction::Bipush(42), Instruction::IReturn], &pool).unwrap()
        );
    }
}
