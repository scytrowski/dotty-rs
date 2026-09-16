use crate::ast::{
    IDENT_TAG, IDENTTPT_TAG, NAMEDARG_TAG, RECTHIS_TAG, RawNode, SELECT_TAG, SELECTTPT_TAG,
    SELFDEF_TAG, SHAREDTERM_TAG, SHAREDTYPE_TAG, TERMREFDIRECT_TAG, TYPEREFDIRECT_TAG,
};
use crate::name_table::NameRef;
use crate::reader::{ReadError, Reader};
use crate::writer::{WriteError, Writer};
use std::fmt;

pub const UNITCONST_TAG: u8 = 2;
pub const FALSECONST_TAG: u8 = 3;
pub const TRUECONST_TAG: u8 = 4;
pub const NULLCONST_TAG: u8 = 5;
pub const BYTECONST_TAG: u8 = 67;
pub const SHORTCONST_TAG: u8 = 68;
pub const CHARCONST_TAG: u8 = 69;
pub const INTCONST_TAG: u8 = 70;
pub const LONGCONST_TAG: u8 = 71;
pub const FLOATCONST_TAG: u8 = 72;
pub const DOUBLECONST_TAG: u8 = 73;
pub const STRINGCONST_TAG: u8 = 74;

pub const DEFAULT_MAX_TREE_DEPTH: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AstRefKind {
    SharedTerm,
    SharedType,
    TermRefDirect,
    TypeRefDirect,
    RecursiveThis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AstRef {
    pub kind: AstRefKind,
    pub address: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AstTreeNode {
    pub tag: u8,
    pub offset: usize,
}

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

/// A semantically decoded TASTy constant.
///
/// Floating-point values are kept as their wire bit patterns. This preserves
/// NaNs, signed zeroes, and other values that would be lost by converting to
/// Rust's floating-point types during a lossless decode/encode cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstantValue {
    Unit,
    Boolean(bool),
    Null,
    Byte(i8),
    Short(i16),
    Char(u16),
    Int(i32),
    Long(i64),
    FloatBits(u32),
    DoubleBits(u64),
    String(NameRef),
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
    InvalidConstant { tag: u8, value: i64, offset: usize },
    RecursionLimit { offset: usize, limit: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermEncodeError {
    Write(WriteError),
    InvalidTag { tag: u8 },
    InvalidValue { tag: u8 },
}

impl fmt::Display for TermEncodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Write(error) => error.fmt(formatter),
            Self::InvalidTag { tag } => write!(formatter, "invalid term tag {tag} for encoding"),
            Self::InvalidValue { tag } => write!(formatter, "invalid value for term tag {tag}"),
        }
    }
}

impl std::error::Error for TermEncodeError {}

impl From<WriteError> for TermEncodeError {
    fn from(error: WriteError) -> Self {
        Self::Write(error)
    }
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
            Self::InvalidConstant { tag, value, offset } => write!(
                formatter,
                "constant tag {tag} at offset {offset} has invalid value {value}"
            ),
            Self::RecursionLimit { offset, limit } => write!(
                formatter,
                "term tree at offset {offset} exceeds the maximum depth of {limit}"
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
    /// Construct a validated category-1 or category-2 leaf for encoding.
    ///
    /// Newly constructed terms start at offset zero; offsets are metadata
    /// derived while decoding and are not needed when encoding a new tree.
    pub fn new(tag: u8, value: TermValue) -> Result<Self, TermEncodeError> {
        if !matches!(tag, 2..=5 | 60..=76) {
            return Err(TermEncodeError::InvalidTag { tag });
        }

        if !matches!(
            (&value, tag),
            (TermValue::Unit, 2)
                | (TermValue::Boolean(false), 3)
                | (TermValue::Boolean(true), 4)
                | (TermValue::Null, 5)
                | (TermValue::AstRef(_), 60..=63 | 66)
                | (TermValue::NameRef(_), 64..=65 | 74..=76)
                | (TermValue::Int(_), 67 | 68 | 70 | 72)
                | (TermValue::Nat(_), 69)
                | (TermValue::LongInt(_), 71 | 73)
        ) {
            return Err(TermEncodeError::InvalidValue { tag });
        }

        Ok(Self {
            tag,
            offset: 0,
            value,
        })
    }

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

    pub fn name_ref(&self) -> Option<NameRef> {
        match self.value {
            TermValue::NameRef(reference) => Some(reference),
            _ => None,
        }
    }

    pub fn ast_ref_kind(&self) -> Option<AstRefKind> {
        if !matches!(self.value, TermValue::AstRef(_)) {
            return None;
        }

        match self.tag {
            SHAREDTERM_TAG => Some(AstRefKind::SharedTerm),
            SHAREDTYPE_TAG => Some(AstRefKind::SharedType),
            TERMREFDIRECT_TAG => Some(AstRefKind::TermRefDirect),
            TYPEREFDIRECT_TAG => Some(AstRefKind::TypeRefDirect),
            RECTHIS_TAG => Some(AstRefKind::RecursiveThis),
            _ => None,
        }
    }

    pub fn ast_ref(&self) -> Option<AstRef> {
        let TermValue::AstRef(address) = &self.value else {
            return None;
        };

        Some(AstRef {
            kind: self.ast_ref_kind()?,
            address: *address,
        })
    }

    /// Interprets this category-1/2 leaf as a typed TASTy constant.
    ///
    /// `Ok(None)` means that the leaf is a name or AST reference rather than
    /// a constant. Narrow constants are range-checked here, and `CHARconst`
    /// is rejected when its Nat does not fit in Scala's 16-bit `Char`.
    pub fn constant_value(&self) -> Result<Option<ConstantValue>, TermError> {
        let value = match self.tag {
            UNITCONST_TAG => ConstantValue::Unit,
            FALSECONST_TAG => ConstantValue::Boolean(false),
            TRUECONST_TAG => ConstantValue::Boolean(true),
            NULLCONST_TAG => ConstantValue::Null,
            BYTECONST_TAG => ConstantValue::Byte(match self.value {
                TermValue::Int(value) => {
                    i8::try_from(value).map_err(|_| TermError::InvalidConstant {
                        tag: self.tag,
                        value: i64::from(value),
                        offset: self.offset,
                    })?
                }
                _ => return Ok(None),
            }),
            SHORTCONST_TAG => ConstantValue::Short(match self.value {
                TermValue::Int(value) => {
                    i16::try_from(value).map_err(|_| TermError::InvalidConstant {
                        tag: self.tag,
                        value: i64::from(value),
                        offset: self.offset,
                    })?
                }
                _ => return Ok(None),
            }),
            CHARCONST_TAG => ConstantValue::Char(match self.value {
                TermValue::Nat(value) => {
                    u16::try_from(value).map_err(|_| TermError::InvalidConstant {
                        tag: self.tag,
                        value: i64::from(value),
                        offset: self.offset,
                    })?
                }
                _ => return Ok(None),
            }),
            INTCONST_TAG => ConstantValue::Int(match self.value {
                TermValue::Int(value) => value,
                _ => return Ok(None),
            }),
            LONGCONST_TAG => ConstantValue::Long(match self.value {
                TermValue::LongInt(value) => value,
                _ => return Ok(None),
            }),
            FLOATCONST_TAG => ConstantValue::FloatBits(match self.value {
                TermValue::Int(value) => u32::from_ne_bytes(value.to_ne_bytes()),
                _ => return Ok(None),
            }),
            DOUBLECONST_TAG => ConstantValue::DoubleBits(match self.value {
                TermValue::LongInt(value) => u64::from_ne_bytes(value.to_ne_bytes()),
                _ => return Ok(None),
            }),
            STRINGCONST_TAG => ConstantValue::String(match self.value {
                TermValue::NameRef(value) => value,
                _ => return Ok(None),
            }),
            _ => return Ok(None),
        };

        Ok(Some(value))
    }

    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        writer.write_u8(self.tag);
        match (&self.value, self.tag) {
            (TermValue::Unit, 2)
            | (TermValue::Boolean(false), 3)
            | (TermValue::Boolean(true), 4)
            | (TermValue::Null, 5) => {}
            (TermValue::AstRef(value), 60..=63 | 66)
            | (TermValue::NameRef(value), 64..=65 | 74..=76)
            | (TermValue::Nat(value), 69) => writer.write_nat(*value),
            (TermValue::Int(value), 67 | 68 | 70 | 72) => writer.write_int(*value),
            (TermValue::LongInt(value), 71 | 73) => writer.write_long_int(*value),
            _ => return Err(TermEncodeError::InvalidValue { tag: self.tag }),
        }
        Ok(())
    }
}

impl ConstantValue {
    /// Encodes this typed constant using its canonical TASTy tag.
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        match self {
            Self::Unit => writer.write_u8(UNITCONST_TAG),
            Self::Boolean(false) => writer.write_u8(FALSECONST_TAG),
            Self::Boolean(true) => writer.write_u8(TRUECONST_TAG),
            Self::Null => writer.write_u8(NULLCONST_TAG),
            Self::Byte(value) => {
                writer.write_u8(BYTECONST_TAG);
                writer.write_int(i32::from(*value));
            }
            Self::Short(value) => {
                writer.write_u8(SHORTCONST_TAG);
                writer.write_int(i32::from(*value));
            }
            Self::Char(value) => {
                writer.write_u8(CHARCONST_TAG);
                writer.write_nat(u32::from(*value));
            }
            Self::Int(value) => {
                writer.write_u8(INTCONST_TAG);
                writer.write_int(*value);
            }
            Self::Long(value) => {
                writer.write_u8(LONGCONST_TAG);
                writer.write_long_int(*value);
            }
            Self::FloatBits(value) => {
                writer.write_u8(FLOATCONST_TAG);
                writer.write_int(i32::from_ne_bytes(value.to_ne_bytes()));
            }
            Self::DoubleBits(value) => {
                writer.write_u8(DOUBLECONST_TAG);
                writer.write_long_int(i64::from_ne_bytes(value.to_ne_bytes()));
            }
            Self::String(reference) => {
                writer.write_u8(STRINGCONST_TAG);
                writer.write_nat(*reference);
            }
        }
        Ok(())
    }
}

impl<'a> RawTree<'a> {
    /// Construct a validated category-1 or category-2 leaf for encoding.
    pub fn leaf(tag: u8, value: TermValue) -> Result<Self, TermEncodeError> {
        Ok(Self::Leaf(SimpleTerm::new(tag, value)?))
    }

    /// Construct a validated category-3 AST wrapper for encoding.
    pub fn ast(tag: u8, child: Self) -> Result<Self, TermEncodeError> {
        if !(90..=104).contains(&tag) {
            return Err(TermEncodeError::InvalidTag { tag });
        }
        Ok(Self::Ast {
            tag,
            offset: 0,
            child: Box::new(child),
        })
    }

    /// Construct a validated category-4 `Nat AST` wrapper for encoding.
    pub fn nat_ast(tag: u8, value: u32, child: Self) -> Result<Self, TermEncodeError> {
        if !(110..=119).contains(&tag) {
            return Err(TermEncodeError::InvalidTag { tag });
        }
        Ok(Self::NatAst {
            tag,
            offset: 0,
            value,
            child: Box::new(child),
        })
    }

    /// Returns the nodes visible in this raw tree in wire order.
    ///
    /// The returned metadata includes category-3 and category-4 wrappers,
    /// category-1/2 leaves, and category-5 boundary nodes. The contents of a
    /// category-5 payload are not traversed by this generic term operation.
    pub fn nodes(&self) -> Vec<AstTreeNode> {
        let mut nodes = Vec::new();
        self.visit_nodes(&mut |node| nodes.push(node));
        nodes
    }

    /// Visits the nodes visible in this raw tree in wire order without
    /// allocating a result vector.
    pub fn visit_nodes(&self, visitor: &mut impl FnMut(AstTreeNode)) {
        match self {
            Self::Leaf(term) => visitor(AstTreeNode {
                tag: term.tag,
                offset: term.offset,
            }),
            Self::Ast { tag, offset, child }
            | Self::NatAst {
                tag, offset, child, ..
            } => {
                visitor(AstTreeNode {
                    tag: *tag,
                    offset: *offset,
                });
                child.visit_nodes(visitor);
            }
            Self::LengthNode(node) => visitor(AstTreeNode {
                tag: node.tag,
                offset: node.offset,
            }),
        }
    }

    /// Returns all AST references visible in this raw tree.
    ///
    /// Category-3 and category-4 wrappers are traversed in source order. A
    /// category-5 length-delimited node is an opaque boundary here because
    /// its payload has a tag-specific grammar; callers can inspect that node
    /// with the corresponding AST decoder when they need to continue deeper.
    pub fn ast_refs(&self) -> Vec<AstRef> {
        let mut references = Vec::new();
        self.visit_ast_refs(&mut |reference| references.push(reference));
        references
    }

    /// Visits all AST references visible in this raw tree in source order.
    ///
    /// This non-allocating form is useful for consumers that want to build an
    /// index or stream references directly. Category-5 payloads remain
    /// opaque; their tag-specific child trees are exposed by the structured
    /// AST decoders instead.
    pub fn visit_ast_refs(&self, visitor: &mut impl FnMut(AstRef)) {
        match self {
            Self::Leaf(term) => {
                if let Some(reference) = term.ast_ref() {
                    visitor(reference);
                }
            }
            Self::Ast { child, .. } | Self::NatAst { child, .. } => child.visit_ast_refs(visitor),
            Self::LengthNode(_) => {}
        }
    }

    /// Returns all name-table references visible in this raw tree.
    ///
    /// Category-3 and category-4 wrappers are traversed in source order.
    /// Category-5 length-delimited nodes remain opaque because their payload
    /// grammar is tag-specific; structured AST decoders can inspect those
    /// payloads when needed.
    pub fn name_refs(&self) -> Vec<NameRef> {
        let mut references = Vec::new();
        self.visit_name_refs(&mut |reference| references.push(reference));
        references
    }

    /// Visits all name-table references visible in this raw tree in source
    /// order without allocating a result vector.
    pub fn visit_name_refs(&self, visitor: &mut impl FnMut(NameRef)) {
        match self {
            Self::Leaf(term) => {
                if let Some(reference) = term.name_ref() {
                    visitor(reference);
                }
            }
            Self::Ast { child, .. } => child.visit_name_refs(visitor),
            Self::NatAst {
                tag, value, child, ..
            } => {
                if matches!(
                    *tag,
                    IDENT_TAG
                        | IDENTTPT_TAG
                        | SELECT_TAG
                        | SELECTTPT_TAG
                        | SELFDEF_TAG
                        | NAMEDARG_TAG
                ) {
                    visitor(*value);
                }
                child.visit_name_refs(visitor);
            }
            Self::LengthNode(_) => {}
        }
    }

    pub fn ast_ref(&self) -> Option<AstRef> {
        match self {
            Self::Leaf(term) => term.ast_ref(),
            Self::Ast { .. } | Self::NatAst { .. } | Self::LengthNode(_) => None,
        }
    }

    pub fn decode(reader: &mut Reader<'a>) -> Result<Self, TermError> {
        Self::decode_with_max_depth(reader, DEFAULT_MAX_TREE_DEPTH)
    }

    /// Decode a tree while reporting offsets relative to an enclosing byte
    /// buffer.
    ///
    /// This is the offset-aware counterpart of [`RawTree::decode`]. It is
    /// useful when a bounded reader covers a nested AST payload but AST
    /// references must still be compared with addresses in the complete
    /// `ASTs` section.
    pub fn decode_with_base_offset(
        reader: &mut Reader<'a>,
        base_offset: usize,
    ) -> Result<Self, TermError> {
        Self::decode_with_max_depth_and_base_offset(reader, DEFAULT_MAX_TREE_DEPTH, base_offset)
    }

    pub fn decode_with_max_depth(
        reader: &mut Reader<'a>,
        max_depth: usize,
    ) -> Result<Self, TermError> {
        Self::decode_with_max_depth_and_base_offset(reader, max_depth, 0)
    }

    /// Decode a tree with both a recursion limit and an enclosing-buffer base
    /// offset.
    pub fn decode_with_max_depth_and_base_offset(
        reader: &mut Reader<'a>,
        max_depth: usize,
        base_offset: usize,
    ) -> Result<Self, TermError> {
        Self::decode_at_depth(reader, 0, max_depth, base_offset)
    }

    fn decode_at_depth(
        reader: &mut Reader<'a>,
        depth: usize,
        max_depth: usize,
        base_offset: usize,
    ) -> Result<Self, TermError> {
        let offset = base_offset.saturating_add(reader.position());
        if depth >= max_depth {
            return Err(TermError::RecursionLimit {
                offset,
                limit: max_depth,
            });
        }
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
            3 => {
                if !(90..=104).contains(&tag) {
                    return Err(TermError::InvalidTag { tag, offset });
                }
                Ok(Self::Ast {
                    tag,
                    offset,
                    child: Box::new(Self::decode_at_depth(
                        reader,
                        depth + 1,
                        max_depth,
                        base_offset,
                    )?),
                })
            }
            4 => {
                if !(110..=119).contains(&tag) {
                    return Err(TermError::InvalidTag { tag, offset });
                }
                let value = reader.read_nat()?;
                Ok(Self::NatAst {
                    tag,
                    offset,
                    value,
                    child: Box::new(Self::decode_at_depth(
                        reader,
                        depth + 1,
                        max_depth,
                        base_offset,
                    )?),
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

    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        match self {
            Self::Leaf(term) => term.encode(writer),
            Self::Ast { tag, child, .. } => {
                if !(90..=104).contains(tag) {
                    return Err(TermEncodeError::InvalidTag { tag: *tag });
                }
                writer.write_u8(*tag);
                child.encode(writer)
            }
            Self::NatAst {
                tag, value, child, ..
            } => {
                if !(110..=119).contains(tag) {
                    return Err(TermEncodeError::InvalidTag { tag: *tag });
                }
                writer.write_u8(*tag);
                writer.write_nat(*value);
                child.encode(writer)
            }
            Self::LengthNode(node) => node.encode(writer).map_err(TermEncodeError::from),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AstRef, AstRefKind, ConstantValue, RawTree, SimpleTerm, TermEncodeError, TermError,
        TermValue,
    };
    use crate::ast::{
        IDENT_TAG, IDENTTPT_TAG, NAMEDARG_TAG, RECTHIS_TAG, SELECT_TAG, SELECTTPT_TAG, SELFDEF_TAG,
        SHAREDTERM_TAG, SHAREDTYPE_TAG, TERMREF_TAG, TERMREFDIRECT_TAG, THIS_TAG,
        TYPEREFDIRECT_TAG,
    };
    use crate::reader::{ReadError, Reader};
    use crate::writer::Writer;

    #[test]
    fn decodes_a_name_reference_term() {
        let mut reader = Reader::new(&[64, 0x85]);

        assert_eq!(
            SimpleTerm::decode(&mut reader).unwrap(),
            SimpleTerm {
                tag: 64,
                offset: 0,
                value: TermValue::NameRef(5),
            }
        );
    }

    #[test]
    fn decodes_an_int_term() {
        let mut reader = Reader::new(&[70, 0x82]);

        assert_eq!(
            SimpleTerm::decode(&mut reader).unwrap(),
            SimpleTerm {
                tag: 70,
                offset: 0,
                value: TermValue::Int(2),
            }
        );
    }

    #[test]
    fn decodes_a_long_int_term() {
        let mut reader = Reader::new(&[73, 0x81]);

        assert_eq!(
            SimpleTerm::decode(&mut reader).unwrap(),
            SimpleTerm {
                tag: 73,
                offset: 0,
                value: TermValue::LongInt(1),
            }
        );
    }

    #[test]
    fn constructs_a_validated_integer_leaf() {
        let term = SimpleTerm::new(70, TermValue::Int(-42)).unwrap();

        assert_eq!(
            term,
            SimpleTerm {
                tag: 70,
                offset: 0,
                value: TermValue::Int(-42),
            }
        );
    }

    #[test]
    fn constructs_a_validated_name_reference_leaf() {
        let term = SimpleTerm::new(64, TermValue::NameRef(7)).unwrap();

        assert_eq!(term.offset, 0);
        assert_eq!(term.name_ref(), Some(7));
    }

    #[test]
    fn rejects_a_non_leaf_tag_when_constructing_a_simple_term() {
        assert_eq!(
            SimpleTerm::new(90, TermValue::AstRef(1)),
            Err(TermEncodeError::InvalidTag { tag: 90 })
        );
    }

    #[test]
    fn rejects_a_mismatched_value_when_constructing_a_simple_term() {
        assert_eq!(
            SimpleTerm::new(70, TermValue::LongInt(1)),
            Err(TermEncodeError::InvalidValue { tag: 70 })
        );
    }

    #[test]
    fn constructs_and_encodes_a_category_three_tree() {
        let tree = RawTree::ast(THIS_TAG, RawTree::leaf(70, TermValue::Int(42)).unwrap()).unwrap();
        let mut writer = Writer::new();

        tree.encode(&mut writer).unwrap();

        assert_eq!(writer.as_slice(), &[THIS_TAG, 70, 0xAA]);
    }

    #[test]
    fn constructs_and_encodes_a_category_four_tree() {
        let tree =
            RawTree::nat_ast(IDENT_TAG, 7, RawTree::leaf(2, TermValue::Unit).unwrap()).unwrap();
        let mut writer = Writer::new();

        tree.encode(&mut writer).unwrap();

        assert_eq!(writer.as_slice(), &[IDENT_TAG, 0x87, 2]);
    }

    #[test]
    fn rejects_an_invalid_category_three_wrapper_tag() {
        assert_eq!(
            RawTree::ast(105, RawTree::leaf(2, TermValue::Unit).unwrap()),
            Err(TermEncodeError::InvalidTag { tag: 105 })
        );
    }

    #[test]
    fn rejects_an_invalid_category_four_wrapper_tag() {
        assert_eq!(
            RawTree::nat_ast(120, 1, RawTree::leaf(2, TermValue::Unit).unwrap()),
            Err(TermEncodeError::InvalidTag { tag: 120 })
        );
    }

    macro_rules! decodes_constant_term {
        ($name:ident, $tag:expr, $value:expr) => {
            #[test]
            fn $name() {
                let mut reader = Reader::new(&[$tag]);
                assert_eq!(SimpleTerm::decode(&mut reader).unwrap().value, $value);
                assert!(reader.is_at_end());
            }
        };
    }

    decodes_constant_term!(decodes_unit_term, 2, TermValue::Unit);
    decodes_constant_term!(decodes_false_boolean_term, 3, TermValue::Boolean(false));
    decodes_constant_term!(decodes_true_boolean_term, 4, TermValue::Boolean(true));
    decodes_constant_term!(decodes_null_term, 5, TermValue::Null);

    fn assert_constant_round_trip(value: ConstantValue) {
        let mut writer = Writer::new();
        value.encode(&mut writer).unwrap();
        let bytes = writer.into_inner();
        let mut reader = Reader::new(&bytes);
        let term = SimpleTerm::decode(&mut reader).unwrap();

        assert_eq!(term.constant_value().unwrap(), Some(value));
        assert!(reader.is_at_end());
    }

    #[test]
    fn decodes_and_encodes_a_byte_constant() {
        assert_constant_round_trip(ConstantValue::Byte(-42));
    }

    #[test]
    fn decodes_and_encodes_a_short_constant() {
        assert_constant_round_trip(ConstantValue::Short(-1234));
    }

    #[test]
    fn decodes_and_encodes_a_char_constant() {
        assert_constant_round_trip(ConstantValue::Char(0x017c));
    }

    #[test]
    fn decodes_and_encodes_an_int_constant() {
        assert_constant_round_trip(ConstantValue::Int(-123456));
    }

    #[test]
    fn decodes_and_encodes_a_long_constant() {
        assert_constant_round_trip(ConstantValue::Long(-1_234_567_890_123));
    }

    #[test]
    fn decodes_and_encodes_a_float_constant_by_bits() {
        assert_constant_round_trip(ConstantValue::FloatBits(0x7fc0_0001));
    }

    #[test]
    fn decodes_and_encodes_a_double_constant_by_bits() {
        assert_constant_round_trip(ConstantValue::DoubleBits(0x7ff8_0000_0000_0001));
    }

    #[test]
    fn decodes_and_encodes_a_string_constant_reference() {
        assert_constant_round_trip(ConstantValue::String(42));
    }

    #[test]
    fn decodes_and_encodes_a_unit_constant() {
        assert_constant_round_trip(ConstantValue::Unit);
    }

    #[test]
    fn decodes_and_encodes_a_false_constant() {
        assert_constant_round_trip(ConstantValue::Boolean(false));
    }

    #[test]
    fn decodes_and_encodes_a_true_constant() {
        assert_constant_round_trip(ConstantValue::Boolean(true));
    }

    #[test]
    fn decodes_and_encodes_a_null_constant() {
        assert_constant_round_trip(ConstantValue::Null);
    }

    #[test]
    fn returns_none_for_a_non_constant_leaf() {
        let mut reader = Reader::new(&[64, 0x81]);
        let term = SimpleTerm::decode(&mut reader).unwrap();

        assert_eq!(term.constant_value(), Ok(None));
    }

    #[test]
    fn rejects_a_byte_constant_outside_its_range() {
        let mut reader = Reader::new(&[67, 0x01, 0x80]);
        let term = SimpleTerm::decode(&mut reader).unwrap();

        assert_eq!(
            term.constant_value(),
            Err(TermError::InvalidConstant {
                tag: 67,
                value: 128,
                offset: 0,
            })
        );
    }

    #[test]
    fn rejects_a_short_constant_outside_its_range() {
        let mut reader = Reader::new(&[68, 0x04, 0x00, 0x80]);
        let term = SimpleTerm::decode(&mut reader).unwrap();

        assert!(matches!(
            term.constant_value(),
            Err(TermError::InvalidConstant {
                tag: 68,
                value: 65_536,
                offset: 0,
            })
        ));
    }

    #[test]
    fn rejects_a_char_constant_outside_the_16_bit_range() {
        let mut writer = Writer::new();
        writer.write_u8(69);
        writer.write_nat(0x1_0000);
        let bytes = writer.into_inner();
        let mut reader = Reader::new(&bytes);
        let term = SimpleTerm::decode(&mut reader).unwrap();

        assert_eq!(
            term.constant_value(),
            Err(TermError::InvalidConstant {
                tag: 69,
                value: 65_536,
                offset: 0,
            })
        );
    }

    #[test]
    fn decodes_every_supported_category_two_tag() {
        let cases = [
            (60, TermValue::AstRef(1)),
            (61, TermValue::AstRef(1)),
            (62, TermValue::AstRef(1)),
            (63, TermValue::AstRef(1)),
            (64, TermValue::NameRef(1)),
            (65, TermValue::NameRef(1)),
            (66, TermValue::AstRef(1)),
            (67, TermValue::Int(1)),
            (68, TermValue::Int(1)),
            (69, TermValue::Nat(1)),
            (70, TermValue::Int(1)),
            (71, TermValue::LongInt(1)),
            (72, TermValue::Int(1)),
            (73, TermValue::LongInt(1)),
            (74, TermValue::NameRef(1)),
            (75, TermValue::NameRef(1)),
            (76, TermValue::NameRef(1)),
        ];

        for (tag, expected) in cases {
            let bytes = [tag, 0x81];
            let mut reader = Reader::new(&bytes);
            assert_eq!(SimpleTerm::decode(&mut reader).unwrap().value, expected);
            assert!(reader.is_at_end());
        }
    }

    #[test]
    fn classifies_ast_reference_kinds_from_the_scala_3_9_tag_matrix() {
        let cases = [
            (SHAREDTERM_TAG, AstRefKind::SharedTerm),
            (SHAREDTYPE_TAG, AstRefKind::SharedType),
            (TERMREFDIRECT_TAG, AstRefKind::TermRefDirect),
            (TYPEREFDIRECT_TAG, AstRefKind::TypeRefDirect),
            (RECTHIS_TAG, AstRefKind::RecursiveThis),
        ];

        for (tag, expected_kind) in cases {
            let bytes = [tag, 0x81];
            let mut reader = Reader::new(&bytes);
            let term = SimpleTerm::decode(&mut reader).unwrap();

            assert_eq!(term.value, TermValue::AstRef(1));
            assert_eq!(term.ast_ref_kind(), Some(expected_kind));
            assert_eq!(
                term.ast_ref(),
                Some(AstRef {
                    kind: expected_kind,
                    address: 1,
                })
            );
            assert!(reader.is_at_end());
        }
    }

    #[test]
    fn returns_no_ast_reference_kind_for_non_reference_terms() {
        let mut reader = Reader::new(&[70, 0x81]);
        let term = SimpleTerm::decode(&mut reader).unwrap();

        assert_eq!(term.ast_ref_kind(), None);
    }

    #[test]
    fn returns_no_ast_reference_kind_for_an_inconsistent_raw_term() {
        let term = SimpleTerm {
            tag: 60,
            offset: 0,
            value: TermValue::Int(1),
        };

        assert_eq!(term.ast_ref_kind(), None);
        assert_eq!(term.ast_ref(), None);
    }

    #[test]
    fn exposes_ast_references_from_raw_tree_leaves() {
        let mut reader = Reader::new(&[60, 0x85]);
        let tree = RawTree::decode(&mut reader).unwrap();

        assert_eq!(
            tree.ast_ref(),
            Some(AstRef {
                kind: AstRefKind::SharedTerm,
                address: 5,
            })
        );
    }

    #[test]
    fn exposes_name_references_from_raw_tree_leaves_and_wrappers() {
        let mut reader = Reader::new(&[64, 0x85, 90, 64, 0x86, 110, 0x87, 64, 0x88, 128, 0x80]);
        let first = RawTree::decode(&mut reader).unwrap();
        let second = RawTree::decode(&mut reader).unwrap();
        let third = RawTree::decode(&mut reader).unwrap();
        let opaque = RawTree::decode(&mut reader).unwrap();

        assert_eq!(first.name_refs(), vec![5]);
        assert_eq!(second.name_refs(), vec![6]);
        assert_eq!(third.name_refs(), vec![7, 8]);
        assert!(opaque.name_refs().is_empty());

        let mut visited = Vec::new();
        third.visit_name_refs(&mut |reference| visited.push(reference));
        assert_eq!(visited, vec![7, 8]);
    }

    #[test]
    fn exposes_an_identifier_name_reference_from_a_category_four_tree() {
        let mut reader = Reader::new(&[IDENT_TAG, 0x85, 2]);
        let tree = RawTree::decode(&mut reader).unwrap();

        assert_eq!(tree.name_refs(), vec![5]);
    }

    #[test]
    fn exposes_an_identifier_tpt_name_reference_from_a_category_four_tree() {
        let mut reader = Reader::new(&[IDENTTPT_TAG, 0x85, 2]);
        let tree = RawTree::decode(&mut reader).unwrap();

        assert_eq!(tree.name_refs(), vec![5]);
    }

    #[test]
    fn exposes_a_select_name_reference_from_a_category_four_tree() {
        let mut reader = Reader::new(&[SELECT_TAG, 0x85, 2]);
        let tree = RawTree::decode(&mut reader).unwrap();

        assert_eq!(tree.name_refs(), vec![5]);
    }

    #[test]
    fn exposes_a_select_tpt_name_reference_from_a_category_four_tree() {
        let mut reader = Reader::new(&[SELECTTPT_TAG, 0x85, 2]);
        let tree = RawTree::decode(&mut reader).unwrap();

        assert_eq!(tree.name_refs(), vec![5]);
    }

    #[test]
    fn exposes_a_self_definition_name_reference_from_a_category_four_tree() {
        let mut reader = Reader::new(&[SELFDEF_TAG, 0x85, 2]);
        let tree = RawTree::decode(&mut reader).unwrap();

        assert_eq!(tree.name_refs(), vec![5]);
    }

    #[test]
    fn exposes_a_named_argument_name_reference_from_a_category_four_tree() {
        let mut reader = Reader::new(&[NAMEDARG_TAG, 0x85, 2]);
        let tree = RawTree::decode(&mut reader).unwrap();

        assert_eq!(tree.name_refs(), vec![5]);
    }

    #[test]
    fn does_not_treat_a_category_four_ast_reference_as_a_name_reference() {
        let mut reader = Reader::new(&[TERMREF_TAG, 0x85, 2]);
        let tree = RawTree::decode(&mut reader).unwrap();

        assert!(tree.name_refs().is_empty());
    }

    #[test]
    fn collects_references_through_ast_wrappers_in_source_order() {
        let mut reader = Reader::new(&[90, 110, 0x81, 60, 0x85]);
        let tree = RawTree::decode(&mut reader).unwrap();

        assert_eq!(
            tree.ast_refs(),
            vec![AstRef {
                kind: AstRefKind::SharedTerm,
                address: 5,
            }]
        );

        let mut visited = Vec::new();
        tree.visit_ast_refs(&mut |reference| visited.push(reference));
        assert_eq!(visited, tree.ast_refs());
    }

    #[test]
    fn treats_length_delimited_nodes_as_opaque_reference_boundaries() {
        let mut reader = Reader::new(&[128, 0x82, 60, 0x85]);
        let tree = RawTree::decode(&mut reader).unwrap();

        assert!(tree.ast_refs().is_empty());
        assert!(reader.is_at_end());
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
                ..
            }
        ));
        assert!(reader.is_at_end());
    }

    #[test]
    fn reports_tree_offsets_relative_to_an_enclosing_ast_section() {
        let mut reader = Reader::new(&[90, 110, 0x82, 60, 0x85]);
        let tree = super::RawTree::decode_with_base_offset(&mut reader, 100).unwrap();

        let super::RawTree::Ast {
            offset: outer_offset,
            child: outer_child,
            ..
        } = &tree
        else {
            panic!("expected a category-three wrapper")
        };
        assert_eq!(*outer_offset, 100);

        let super::RawTree::NatAst {
            offset: nat_offset,
            child: nat_child,
            ..
        } = outer_child.as_ref()
        else {
            panic!("expected a category-four wrapper")
        };
        assert_eq!(*nat_offset, 101);

        let super::RawTree::Leaf(term) = nat_child.as_ref() else {
            panic!("expected a leaf")
        };
        assert_eq!(term.offset, 103);
        assert_eq!(
            tree.nodes(),
            vec![
                super::AstTreeNode {
                    tag: 90,
                    offset: 100,
                },
                super::AstTreeNode {
                    tag: 110,
                    offset: 101,
                },
                super::AstTreeNode {
                    tag: 60,
                    offset: 103,
                },
            ]
        );
        assert!(reader.is_at_end());
    }

    #[test]
    fn visits_visible_tree_nodes_and_stops_at_a_length_boundary() {
        let mut reader = Reader::new(&[90, 110, 0x82, 128, 0x80]);
        let tree = RawTree::decode(&mut reader).unwrap();

        let mut visited = Vec::new();
        tree.visit_nodes(&mut |node| visited.push(node));

        assert_eq!(
            visited,
            vec![
                super::AstTreeNode { tag: 90, offset: 0 },
                super::AstTreeNode {
                    tag: 110,
                    offset: 1
                },
                super::AstTreeNode {
                    tag: 128,
                    offset: 3
                },
            ]
        );
        assert_eq!(tree.nodes(), visited);
        assert!(reader.is_at_end());
    }

    #[test]
    fn decodes_every_category_three_and_four_tag_as_a_raw_tree() {
        for tag in 90..=104 {
            let bytes = [tag, 64, 0x81];
            let mut reader = Reader::new(&bytes);
            let tree = super::RawTree::decode(&mut reader).unwrap();
            assert!(matches!(tree, super::RawTree::Ast { tag: actual, .. } if actual == tag));
            assert!(reader.is_at_end());
        }

        for tag in 110..=119 {
            let bytes = [tag, 0x81, 64, 0x81];
            let mut reader = Reader::new(&bytes);
            let tree = super::RawTree::decode(&mut reader).unwrap();
            assert!(matches!(tree, super::RawTree::NatAst { tag: actual, .. } if actual == tag));
            assert!(reader.is_at_end());
        }
    }

    #[test]
    fn rejects_unassigned_category_three_and_four_tags() {
        for tag in [
            105, 106, 107, 108, 109, 120, 121, 122, 123, 124, 125, 126, 127,
        ] {
            let bytes = if tag < 110 {
                vec![tag, 64, 0x81]
            } else {
                vec![tag, 0x81, 64, 0x81]
            };
            let mut reader = Reader::new(&bytes);

            assert_eq!(
                super::RawTree::decode(&mut reader),
                Err(TermError::InvalidTag { tag, offset: 0 })
            );
        }
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

    #[test]
    fn decodes_every_assigned_category_five_tag_as_a_bounded_raw_node() {
        let assigned = (128..=134)
            .chain(136..=165)
            .chain([167])
            .chain(169..=183)
            .chain(190..=193)
            .chain([255]);

        for tag in assigned {
            let bytes = [tag, 0x80];
            let mut reader = Reader::new(&bytes);
            let tree = super::RawTree::decode(&mut reader).unwrap();
            assert!(matches!(tree, super::RawTree::LengthNode(raw) if raw.tag == tag));
            assert!(reader.is_at_end());
        }
    }

    #[test]
    fn preserves_unassigned_category_five_tags() {
        for tag in [135, 166, 168, 184, 185, 186, 187, 188, 189, 194, 254] {
            let bytes = [tag, 0x80];
            let mut reader = Reader::new(&bytes);
            assert!(matches!(
                super::RawTree::decode(&mut reader),
                Ok(super::RawTree::LengthNode(raw)) if raw.tag == tag && raw.payload.is_empty()
            ));
            assert!(reader.is_at_end());
        }
    }

    macro_rules! round_trips_term_value {
        ($name:ident, $term:expr) => {
            #[test]
            fn $name() {
                let term = $term;
                let mut writer = Writer::new();
                term.encode(&mut writer).unwrap();
                let mut reader = Reader::new(writer.as_slice());

                assert_eq!(SimpleTerm::decode(&mut reader).unwrap().value, term.value);
                assert!(reader.is_at_end());
            }
        };
    }

    round_trips_term_value!(
        round_trips_unit_encoding,
        SimpleTerm {
            tag: 2,
            offset: 0,
            value: TermValue::Unit,
        }
    );
    round_trips_term_value!(
        round_trips_false_boolean_encoding,
        SimpleTerm {
            tag: 3,
            offset: 0,
            value: TermValue::Boolean(false),
        }
    );
    round_trips_term_value!(
        round_trips_true_boolean_encoding,
        SimpleTerm {
            tag: 4,
            offset: 0,
            value: TermValue::Boolean(true),
        }
    );
    round_trips_term_value!(
        round_trips_null_encoding,
        SimpleTerm {
            tag: 5,
            offset: 0,
            value: TermValue::Null,
        }
    );
    round_trips_term_value!(
        round_trips_name_reference_encoding,
        SimpleTerm {
            tag: 64,
            offset: 0,
            value: TermValue::NameRef(5),
        }
    );
    round_trips_term_value!(
        round_trips_int_encoding,
        SimpleTerm {
            tag: 70,
            offset: 0,
            value: TermValue::Int(-5),
        }
    );
    round_trips_term_value!(
        round_trips_long_int_encoding,
        SimpleTerm {
            tag: 73,
            offset: 0,
            value: TermValue::LongInt(5),
        }
    );

    #[test]
    fn rejects_a_simple_term_with_a_mismatched_tag_and_value() {
        let term = SimpleTerm {
            tag: 2,
            offset: 0,
            value: TermValue::Null,
        };
        let mut writer = Writer::new();

        assert_eq!(
            term.encode(&mut writer),
            Err(TermEncodeError::InvalidValue { tag: 2 })
        );
    }

    #[test]
    fn round_trips_nested_raw_trees() {
        let mut reader = Reader::new(&[112, 0x85, 64, 0x86]);
        let tree = RawTree::decode(&mut reader).unwrap();
        let mut writer = Writer::new();

        tree.encode(&mut writer).unwrap();

        assert_eq!(writer.as_slice(), &[112, 0x85, 64, 0x86]);
    }

    #[test]
    fn rejects_invalid_category_three_and_four_tags_when_encoding() {
        let leaf = || {
            RawTree::Leaf(SimpleTerm {
                tag: 2,
                offset: 0,
                value: TermValue::Unit,
            })
        };

        for tree in [
            RawTree::Ast {
                tag: 105,
                offset: 0,
                child: Box::new(leaf()),
            },
            RawTree::NatAst {
                tag: 120,
                offset: 0,
                value: 0,
                child: Box::new(leaf()),
            },
        ] {
            let mut writer = Writer::new();
            assert_eq!(
                tree.encode(&mut writer),
                Err(TermEncodeError::InvalidTag {
                    tag: match tree {
                        RawTree::Ast { tag, .. } | RawTree::NatAst { tag, .. } => tag,
                        _ => unreachable!(),
                    }
                })
            );
            assert!(writer.as_slice().is_empty());
        }
    }

    #[test]
    fn rejects_a_tree_that_exceeds_the_configured_recursion_limit() {
        let bytes = [90, 90, 2];
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            RawTree::decode_with_max_depth(&mut reader, 2),
            Err(TermError::RecursionLimit {
                offset: 2,
                limit: 2,
            })
        );
    }

    #[test]
    fn decodes_a_tree_within_the_configured_recursion_limit() {
        let bytes = [90, 90, 2];
        let mut reader = Reader::new(&bytes);

        RawTree::decode_with_max_depth(&mut reader, 3).unwrap();
        assert!(reader.is_at_end());
    }

    #[test]
    fn applies_the_base_offset_to_recursion_limit_errors() {
        let mut reader = Reader::new(&[90, 2]);

        assert_eq!(
            RawTree::decode_with_max_depth_and_base_offset(&mut reader, 1, 40),
            Err(TermError::RecursionLimit {
                offset: 41,
                limit: 1,
            })
        );
    }
}
