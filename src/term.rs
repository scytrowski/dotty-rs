use crate::reader::{ReadError, Reader};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermValue {
    AstRef(u32),
    NameRef(u32),
    Nat(u32),
    Int(i32),
    LongInt(i64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimpleTerm {
    pub tag: u8,
    pub offset: usize,
    pub value: TermValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermError {
    Read(ReadError),
    InvalidTag { tag: u8, offset: usize },
    UnsupportedCategory { tag: u8, offset: usize },
}

impl fmt::Display for TermError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => error.fmt(formatter),
            Self::InvalidTag { tag, offset } => {
                write!(formatter, "invalid term tag {tag} at offset {offset}")
            }
            Self::UnsupportedCategory { tag, offset } => write!(
                formatter,
                "term tag {tag} at offset {offset} is not a category-2 leaf"
            ),
        }
    }
}

impl std::error::Error for TermError {}

impl From<ReadError> for TermError {
    fn from(error: ReadError) -> Self {
        Self::Read(error)
    }
}

impl SimpleTerm {
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, TermError> {
        let offset = reader.position();
        let tag = reader.read_u8()?;
        let value = match tag {
            60..=63 | 66 => TermValue::AstRef(reader.read_nat()?),
            64..=65 | 74..=76 => TermValue::NameRef(reader.read_nat()?),
            67..=68 | 70 | 72 => TermValue::Int(reader.read_int()?),
            69 => TermValue::Nat(reader.read_nat()?),
            71 | 73 => TermValue::LongInt(reader.read_long_int()?),
            0 => return Err(TermError::InvalidTag { tag, offset }),
            _ => return Err(TermError::UnsupportedCategory { tag, offset }),
        };

        Ok(Self { tag, offset, value })
    }
}

#[cfg(test)]
mod tests {
    use super::{SimpleTerm, TermError, TermValue};
    use crate::reader::{ReadError, Reader};

    #[test]
    fn decodes_category_two_reference_and_literal_terms() {
        let mut reader = Reader::new(&[64, 0x85, 70, 0x82, 73, 0x81]);

        assert_eq!(
            SimpleTerm::decode(&mut reader).unwrap(),
            SimpleTerm {
                tag: 64,
                offset: 0,
                value: TermValue::NameRef(5),
            }
        );
        assert_eq!(
            SimpleTerm::decode(&mut reader).unwrap(),
            SimpleTerm {
                tag: 70,
                offset: 2,
                value: TermValue::Int(2),
            }
        );
        assert_eq!(
            SimpleTerm::decode(&mut reader).unwrap(),
            SimpleTerm {
                tag: 73,
                offset: 4,
                value: TermValue::LongInt(1),
            }
        );
    }

    #[test]
    fn rejects_tags_without_a_category_two_value() {
        let mut reader = Reader::new(&[90]);

        assert_eq!(
            SimpleTerm::decode(&mut reader),
            Err(TermError::UnsupportedCategory { tag: 90, offset: 0 })
        );
    }

    #[test]
    fn reports_a_truncated_term_value() {
        let mut reader = Reader::new(&[64]);

        assert_eq!(
            SimpleTerm::decode(&mut reader),
            Err(TermError::Read(ReadError::UnexpectedEof {
                offset: 1,
                needed: 1,
                remaining: 0,
            }))
        );
    }
}
