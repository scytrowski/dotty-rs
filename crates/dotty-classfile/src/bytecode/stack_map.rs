//! Typed writer for the `full_frame` form of a JVM `StackMapTable` attribute.
//!
//! This module writes caller-supplied verification types and absolute bytecode
//! PCs. It does not infer verifier states, choose frame locations, compress
//! frames into `same`/`chop`/`append` forms, or parse existing attributes.
//! [`StackMapTable::encode_payload`] validates structure and constant-pool
//! references. Call [`StackMapTable::encode_payload_with_layout`] when code
//! bytes and an assembled instruction layout are available to additionally
//! validate frame boundaries and that `Uninitialized` entries point to `new`.
//! The resulting payload can be supplied to `Attribute::Other`; the existing
//! opaque attribute decoder and encoder remain unchanged.

use crate::bytecode::assembler::CodeLayout;
use crate::constant_pool::{ConstantPool, ConstantPoolEntry, ConstantPoolIndex};
use crate::encode_error::EncodeError;
use crate::writer::{Writer, checked_u16_len, checked_u32_len};

/// One JVM `verification_type_info` value (JVMS §4.7.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationTypeInfo {
    /// The `Top_variable_info` tag (`0`).
    Top,
    /// The `Integer_variable_info` tag (`1`).
    Integer,
    /// The `Float_variable_info` tag (`2`).
    Float,
    /// The `Double_variable_info` tag (`3`), occupying two local/stack slots.
    Double,
    /// The `Long_variable_info` tag (`4`), occupying two local/stack slots.
    Long,
    /// The `Null_variable_info` tag (`5`).
    Null,
    /// The `UninitializedThis_variable_info` tag (`6`).
    UninitializedThis,
    /// The `Object_variable_info` tag (`7`) and its `CONSTANT_Class` index.
    Object(ConstantPoolIndex),
    /// The `Uninitialized_variable_info` tag (`8`) and the `new` instruction PC.
    Uninitialized(u16),
}

impl VerificationTypeInfo {
    /// Returns the number of JVM slots represented by this single entry.
    ///
    /// This is distinct from the `number_of_locals` and
    /// `number_of_stack_items` fields, which count entries. `Long` and
    /// `Double` each serialize as one entry while occupying two slots.
    pub const fn slot_count(self) -> usize {
        match self {
            Self::Long | Self::Double => 2,
            _ => 1,
        }
    }
}

/// One `full_frame` with its absolute bytecode PC and verification entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullFrame {
    /// Absolute bytecode PC of this frame, before conversion to `offset_delta`.
    pub pc: u32,
    /// Local verification entries in JVM order. `Long`/`Double` occupy one
    /// entry each; include `Top` explicitly for real holes in locals.
    pub locals: Vec<VerificationTypeInfo>,
    /// Operand-stack verification entries in JVM order. `Long`/`Double`
    /// occupy one entry each.
    pub stack: Vec<VerificationTypeInfo>,
}

/// A typed collection of `full_frame` entries for a StackMapTable payload.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StackMapTable {
    /// Explicit frames in strictly increasing absolute-PC order.
    pub frames: Vec<FullFrame>,
}

impl StackMapTable {
    /// Encodes the StackMapTable payload, without the enclosing `attribute_info`
    /// header, using the supplied pool as the authority for `Object` entries.
    ///
    /// This validates absolute-PC ordering and representable offsets but has no
    /// bytecode metadata with which to check instruction boundaries or
    /// `Uninitialized` targets. Use [`Self::encode_payload_with_layout`] when
    /// those checks are required.
    pub fn encode_payload(&self, constant_pool: &ConstantPool) -> Result<Vec<u8>, EncodeError> {
        self.encode_payload_inner(constant_pool, None)
    }

    /// Encodes the payload and validates PCs against the provided assembled
    /// code. Every frame must begin at an instruction, and every
    /// `Uninitialized` entry must point to a `new` instruction.
    pub fn encode_payload_with_layout(
        &self,
        constant_pool: &ConstantPool,
        code: &[u8],
        layout: &CodeLayout,
    ) -> Result<Vec<u8>, EncodeError> {
        if code.len() != layout.code_length()
            || layout
                .instruction_offsets()
                .iter()
                .any(|offset| *offset >= code.len())
        {
            return Err(EncodeError::InvalidStackMapCodeLayout);
        }
        self.encode_payload_inner(constant_pool, Some((code, layout)))
    }

    fn encode_payload_inner(
        &self,
        constant_pool: &ConstantPool,
        code_layout: Option<(&[u8], &CodeLayout)>,
    ) -> Result<Vec<u8>, EncodeError> {
        checked_u16_len(self.frames.len())?;

        let mut payload_length = 2usize;
        let mut previous_pc = None;
        for (frame_index, frame) in self.frames.iter().enumerate() {
            checked_u16_len(frame.locals.len())?;
            checked_u16_len(frame.stack.len())?;
            payload_length = add_payload_length(payload_length, 7)?;
            let pc =
                u16::try_from(frame.pc).map_err(|_| EncodeError::InvalidStackMapFrameOffset {
                    frame_index,
                    pc: frame.pc,
                })?;
            if let Some(previous_pc) = previous_pc
                && frame.pc <= previous_pc
            {
                return Err(EncodeError::InvalidStackMapFrameOrder {
                    frame_index,
                    previous_pc,
                    pc: frame.pc,
                });
            }
            previous_pc = Some(frame.pc);

            if let Some((_, layout)) = code_layout
                && layout
                    .instruction_offsets()
                    .binary_search(&usize::from(pc))
                    .is_err()
            {
                return Err(EncodeError::InvalidStackMapFrameOffset {
                    frame_index,
                    pc: frame.pc,
                });
            }

            for (verification_type_index, verification_type) in
                frame.locals.iter().chain(&frame.stack).enumerate()
            {
                payload_length = add_payload_length(
                    payload_length,
                    if matches!(verification_type, VerificationTypeInfo::Object(_)) {
                        3
                    } else {
                        1
                    },
                )?;
                match verification_type {
                    VerificationTypeInfo::Object(index) => match constant_pool.get(*index) {
                        Some(ConstantPoolEntry::Class { .. }) => {}
                        Some(_) => {
                            return Err(EncodeError::InvalidReference {
                                index: usize::from(index.0),
                            });
                        }
                        None => {
                            return Err(EncodeError::InvalidIndex {
                                index: usize::from(index.0),
                                count: constant_pool.entry_count(),
                            });
                        }
                    },
                    VerificationTypeInfo::Uninitialized(uninitialized_pc) => {
                        if let Some((code, layout)) = code_layout {
                            let offset = usize::from(*uninitialized_pc);
                            if layout.instruction_offsets().binary_search(&offset).is_err()
                                || code.get(offset) != Some(&0xbb)
                            {
                                return Err(EncodeError::InvalidStackMapUninitializedOffset {
                                    frame_index,
                                    verification_type_index,
                                    pc: *uninitialized_pc,
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        checked_u32_len(payload_length)?;

        let mut writer = Writer::new();
        writer.write_u2_length(self.frames.len())?;
        let mut previous_pc = None;
        for (frame_index, frame) in self.frames.iter().enumerate() {
            let pc =
                u16::try_from(frame.pc).map_err(|_| EncodeError::InvalidStackMapFrameOffset {
                    frame_index,
                    pc: frame.pc,
                })?;
            let offset_delta = previous_pc.map_or(pc, |previous: u16| pc - previous - 1);
            writer.write_u1(255);
            writer.write_u2(offset_delta);
            writer.write_u2_length(frame.locals.len())?;
            for verification_type in &frame.locals {
                write_verification_type(&mut writer, *verification_type);
            }
            writer.write_u2_length(frame.stack.len())?;
            for verification_type in &frame.stack {
                write_verification_type(&mut writer, *verification_type);
            }
            previous_pc = Some(pc);
        }
        Ok(writer.into_bytes())
    }
}

fn add_payload_length(length: usize, additional: usize) -> Result<usize, EncodeError> {
    length
        .checked_add(additional)
        .ok_or(EncodeError::CountOverflow {
            value: usize::MAX,
            width: crate::encode_error::LengthWidth::U32,
        })
}

fn write_verification_type(writer: &mut Writer, verification_type: VerificationTypeInfo) {
    match verification_type {
        VerificationTypeInfo::Top => writer.write_u1(0),
        VerificationTypeInfo::Integer => writer.write_u1(1),
        VerificationTypeInfo::Float => writer.write_u1(2),
        VerificationTypeInfo::Double => writer.write_u1(3),
        VerificationTypeInfo::Long => writer.write_u1(4),
        VerificationTypeInfo::Null => writer.write_u1(5),
        VerificationTypeInfo::UninitializedThis => writer.write_u1(6),
        VerificationTypeInfo::Object(index) => {
            writer.write_u1(7);
            writer.write_u2(index.0);
        }
        VerificationTypeInfo::Uninitialized(pc) => {
            writer.write_u1(8);
            writer.write_u2(pc);
        }
    }
}
