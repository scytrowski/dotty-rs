use crate::ast::RawNode;
use crate::reader::{ReadError, Reader};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermValue {
    Unit,
    Boolean(bool),
    Null,
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
pub enum RawTree<'a> {
    Leaf(SimpleTerm),
    Ast {
        tag: u8,
        offset: usize,
        child: Box<RawTree<'a>>,
    },
    NatAst {
        tag: u8,
        offset: usize,
        value: u32,
        child: Box<RawTree<'a>>,
    },
    LengthNode(RawNode<'a>),
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
        Self::decode_tagged(reader, tag, offset)
    }

    fn decode_tagged(reader: &mut Reader<'_>, tag: u8, offset: usize) -> Result<Self, TermError> {
        let value = match tag {
            2 => TermValue::Unit,
            3 => TermValue::Boolean(false),
            4 => TermValue::Boolean(true),
            5 => TermValue::Null,
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

impl<'a> RawTree<'a> {
    pub fn decode(reader: &mut Reader<'a>) -> Result<Self, TermError> {
        let offset = reader.position();
        let tag = reader.read_u8()?;
        let category = match tag {
            1..=59 => 1,
            60..=89 => 2,
            90..=109 => 3,
            110..=127 => 4,
            128..=255 => 5,
            0 => return Err(TermError::InvalidTag { tag, offset }),
        };

        match category {
            1 | 2 => Ok(Self::Leaf(SimpleTerm::decode_tagged(reader, tag, offset)?)),
            3 => Ok(Self::Ast {
                tag,
                offset,
                child: Box::new(Self::decode(reader)?),
            }),
            4 => {
                let value = reader.read_nat()?;
                Ok(Self::NatAst {
                    tag,
                    offset,
                    value,
                    child: Box::new(Self::decode(reader)?),
                })
            }
            5 => {
                let length = reader.read_nat()? as usize;
                let payload = reader.read_bytes(length)?;
                Ok(Self::LengthNode(RawNode {
                    tag,
                    offset,
                    payload,
                }))
            }
            _ => unreachable!("all AST tags are covered above"),
        }
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
    fn decodes_category_one_constant_terms() {
        let mut reader = Reader::new(&[2, 3, 4, 5]);

        assert_eq!(
            SimpleTerm::decode(&mut reader).unwrap().value,
            TermValue::Unit
        );
        assert_eq!(
            SimpleTerm::decode(&mut reader).unwrap().value,
            TermValue::Boolean(false)
        );
        assert_eq!(
            SimpleTerm::decode(&mut reader).unwrap().value,
            TermValue::Boolean(true)
        );
        assert_eq!(
            SimpleTerm::decode(&mut reader).unwrap().value,
            TermValue::Null
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

    #[test]
    fn decodes_nested_category_three_and_four_raw_trees() {
        let mut reader = Reader::new(&[112, 0x85, 64, 0x86]);
        let tree = super::RawTree::decode(&mut reader).unwrap();

        assert!(matches!(
            tree,
            super::RawTree::NatAst {
                tag: 112,
                value: 5,
                child: _,
                ..
            }
        ));
        assert!(reader.is_at_end());
    }

    #[test]
    fn preserves_a_category_five_node_as_a_bounded_raw_node() {
        let mut reader = Reader::new(&[128, 0x82, b'a', b'b']);
        let tree = super::RawTree::decode(&mut reader).unwrap();

        assert!(matches!(
            tree,
            super::RawTree::LengthNode(raw) if raw.tag == 128 && raw.payload == b"ab"
        ));
    }
}
