use crate::constant_pool::ConstantPoolIndex;
use std::fmt;

/// The 16-byte identifier stored in a Scala 3 class-level `TASTY` attribute.
///
/// The TASTy producer owns this value. This type only enforces its wire width;
/// it does not generate identifiers or interpret TASTy contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TastyLinkId([u8; Self::LENGTH]);

impl TastyLinkId {
    /// The exact number of bytes in the Scala 3 `TASTY` link payload.
    pub const LENGTH: usize = 16;

    /// Wraps an identifier already obtained from the TASTy producer.
    pub const fn from_bytes(bytes: [u8; Self::LENGTH]) -> Self {
        Self(bytes)
    }

    /// Parses a link payload, rejecting lengths other than the audited 16 bytes.
    pub fn try_from_bytes(bytes: &[u8]) -> Result<Self, TastyLinkError> {
        let bytes: [u8; Self::LENGTH] =
            bytes
                .try_into()
                .map_err(|_| TastyLinkError::InvalidPayloadLength {
                    length: bytes.len(),
                })?;
        Ok(Self(bytes))
    }

    /// Returns the exact bytes written to and read from the classfile attribute.
    pub const fn as_bytes(&self) -> &[u8; Self::LENGTH] {
        &self.0
    }
}

/// A malformed or misplaced Scala 3 class-level TASTy link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TastyLinkError {
    /// The raw attribute name index does not resolve to a constant-pool UTF-8 entry.
    InvalidNameIndex { name_index: ConstantPoolIndex },
    /// The attribute payload is not exactly 16 bytes.
    InvalidPayloadLength { length: usize },
    /// A class has more than one class-level `TASTY` attribute.
    DuplicateClassAttribute,
    /// A nested attribute tree exceeds the classfile decoder's traversal bound.
    TooDeeplyNestedAttribute { depth: usize },
    /// `TASTY` appears on a field, method, or nested attribute instead of the class.
    MisplacedAttribute { location: TastyLinkLocation },
}

/// The non-class location where a `TASTY` attribute was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TastyLinkLocation {
    /// A field's attribute list.
    Field,
    /// A method's attribute list.
    Method,
    /// A nested attribute list such as `Code` or a record component.
    Nested,
}

impl fmt::Display for TastyLinkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidNameIndex { name_index } => write!(
                formatter,
                "TASTY attribute name index {} is not a constant-pool UTF-8 entry",
                name_index.0
            ),
            Self::InvalidPayloadLength { length } => write!(
                formatter,
                "TASTY attribute payload has {length} bytes; expected {}",
                TastyLinkId::LENGTH
            ),
            Self::DuplicateClassAttribute => {
                formatter.write_str("class has more than one TASTY attribute")
            }
            Self::TooDeeplyNestedAttribute { depth } => write!(
                formatter,
                "TASTY attribute scan reached nesting depth {depth}, exceeding the configured limit"
            ),
            Self::MisplacedAttribute { location } => {
                write!(formatter, "TASTY attribute is misplaced in {location:?}")
            }
        }
    }
}

impl std::error::Error for TastyLinkError {}
