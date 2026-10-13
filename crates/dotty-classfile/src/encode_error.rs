use std::fmt;

/// The fixed-width integer used to store a count or byte length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LengthWidth {
    /// A JVM `u1` (one-byte) count or length.
    U8,
    /// A JVM `u2` (two-byte) count or length.
    U16,
    /// A JVM `u4` (four-byte) count or length.
    U32,
}

impl LengthWidth {
    fn max_value(self) -> u64 {
        match self {
            Self::U8 => u64::from(u8::MAX),
            Self::U16 => u64::from(u16::MAX),
            Self::U32 => u64::from(u32::MAX),
        }
    }
}

/// A structural failure while preparing or writing class-file bytes.
///
/// These errors describe invalid values or layouts supplied by a caller. They
/// are separate from [`crate::reader::ReadError`], which describes malformed
/// input bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeError {
    /// A count or length does not fit its on-wire integer width.
    CountOverflow {
        /// The count or length that was checked.
        value: usize,
        /// The width required by the relevant class-file field.
        width: LengthWidth,
    },
    /// An index is outside the available entries.
    InvalidIndex {
        /// The index supplied by the caller.
        index: usize,
        /// The number of available entries.
        count: usize,
    },
    /// A reference points to an invalid target.
    InvalidReference {
        /// The referenced index.
        index: usize,
    },
    /// A method handle's target method name is invalid for its reference kind.
    InvalidMethodHandleName {
        /// The index of the method or interface method referenced by the handle.
        index: usize,
    },
    /// A known attribute name is absent from the supplied constant pool.
    MissingAttributeName {
        /// The standard attribute name required by the model variant.
        name: &'static str,
    },
    /// The attribute model is recognized but its encoder is outside the current scope.
    UnsupportedAttribute,
    /// The provided class-file model violates a structural requirement.
    InvalidModel,
    /// The class-file version is outside the supported non-preview range.
    UnsupportedVersion {
        /// The class-file major version.
        major: u16,
        /// The class-file minor version.
        minor: u16,
    },
    /// A `Code` attribute has a code array length outside the JVMS range 1..=65535.
    InvalidCodeLength {
        /// The supplied bytecode length.
        length: usize,
    },
    /// An exception table entry has an invalid range in the raw-PC model.
    InvalidExceptionTableRange {
        /// Index of the invalid entry in the exception table.
        entry_index: usize,
    },
    /// An annotation or array element value exceeds the configured nesting limit.
    TooDeeplyNestedAnnotation,
    /// Nested `Code`/`Record` attribute lists exceed their configured depth limit.
    TooDeeplyNestedAttribute,
    /// The provided byte layout violates a structural requirement.
    InvalidLayout,
    /// A StackMapTable full_frame PC cannot be represented by a JVM `u2`.
    InvalidStackMapFrameOffset {
        /// Zero-based frame index.
        frame_index: usize,
        /// Absolute bytecode PC supplied by the caller.
        pc: u32,
    },
    /// StackMapTable frames must be ordered by strictly increasing absolute PC.
    InvalidStackMapFrameOrder {
        /// Zero-based frame index of the offending frame.
        frame_index: usize,
        /// PC of the preceding frame.
        previous_pc: u32,
        /// PC of the offending frame.
        pc: u32,
    },
    /// Supplied code bytes and instruction layout do not describe the same code array.
    InvalidStackMapCodeLayout,
    /// An uninitialized verification type does not point at a `new` instruction.
    InvalidStackMapUninitializedOffset {
        /// Zero-based frame index.
        frame_index: usize,
        /// Index of the verification type in its locals or operand stack list.
        verification_type_index: usize,
        /// Bytecode PC encoded by the verification type.
        pc: u16,
    },
    /// A UTF-8 byte length does not fit a JVM `u2` length field.
    Utf8LengthOverflow {
        /// The encoded UTF-8 byte length.
        byte_len: usize,
        /// The largest length accepted by the field.
        max: u16,
    },
}

impl fmt::Display for EncodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CountOverflow { value, width } => write!(
                formatter,
                "count or length {value} does not fit in a {}-bit field (maximum {})",
                match width {
                    LengthWidth::U8 => 8,
                    LengthWidth::U16 => 16,
                    LengthWidth::U32 => 32,
                },
                width.max_value()
            ),
            Self::InvalidIndex { index, count } => {
                write!(
                    formatter,
                    "index {index} is out of range for {count} entries"
                )
            }
            Self::InvalidReference { index } => {
                write!(formatter, "invalid reference to index {index}")
            }
            Self::InvalidMethodHandleName { index } => write!(
                formatter,
                "invalid method name for method handle target at index {index}"
            ),
            Self::MissingAttributeName { name } => {
                write!(
                    formatter,
                    "constant pool has no Utf8 entry for attribute {name}"
                )
            }
            Self::UnsupportedAttribute => {
                formatter.write_str("attribute variant is not supported by the encoder")
            }
            Self::InvalidModel => formatter.write_str("invalid class-file model"),
            Self::UnsupportedVersion { major, minor } => write!(
                formatter,
                "class-file version {major}.{minor} is outside the supported non-preview range"
            ),
            Self::InvalidCodeLength { length } => write!(
                formatter,
                "Code bytecode length {length} is outside the JVMS range 1..=65535"
            ),
            Self::InvalidExceptionTableRange { entry_index } => write!(
                formatter,
                "invalid Code exception table range at entry {entry_index}"
            ),
            Self::TooDeeplyNestedAnnotation => write!(
                formatter,
                "annotation element_value nested past depth {}",
                crate::attribute::DEFAULT_MAX_ANNOTATION_DEPTH
            ),
            Self::TooDeeplyNestedAttribute => write!(
                formatter,
                "Code/Record attribute nesting exceeds depth {}",
                crate::attribute::DEFAULT_MAX_ATTRIBUTE_DEPTH
            ),
            Self::InvalidLayout => formatter.write_str("invalid class-file layout"),
            Self::InvalidStackMapFrameOffset { frame_index, pc } => write!(
                formatter,
                "StackMapTable frame {frame_index} PC {pc} does not fit in a JVM u2"
            ),
            Self::InvalidStackMapFrameOrder {
                frame_index,
                previous_pc,
                pc,
            } => write!(
                formatter,
                "StackMapTable frame {frame_index} PC {pc} is not greater than preceding PC {previous_pc}"
            ),
            Self::InvalidStackMapCodeLayout => {
                formatter.write_str("StackMapTable code bytes and instruction layout do not match")
            }
            Self::InvalidStackMapUninitializedOffset {
                frame_index,
                verification_type_index,
                pc,
            } => write!(
                formatter,
                "StackMapTable frame {frame_index} verification type {verification_type_index} has uninitialized PC {pc} that does not point to new"
            ),
            Self::Utf8LengthOverflow { byte_len, max } => write!(
                formatter,
                "UTF-8 byte length {byte_len} exceeds the maximum {max}"
            ),
        }
    }
}

impl std::error::Error for EncodeError {}
