use std::fmt;

/// The fixed-width integer used to store a count or byte length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LengthWidth {
    /// A JVM `u2` (two-byte) count or length.
    U16,
    /// A JVM `u4` (four-byte) count or length.
    U32,
}

impl LengthWidth {
    fn max_value(self) -> u64 {
        match self {
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
    /// The provided class-file model violates a structural requirement.
    InvalidModel,
    /// The provided byte layout violates a structural requirement.
    InvalidLayout,
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
            Self::InvalidModel => formatter.write_str("invalid class-file model"),
            Self::InvalidLayout => formatter.write_str("invalid class-file layout"),
            Self::Utf8LengthOverflow { byte_len, max } => write!(
                formatter,
                "UTF-8 byte length {byte_len} exceeds the maximum {max}"
            ),
        }
    }
}

impl std::error::Error for EncodeError {}
