use crate::reader::{ReadError, Reader};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeCategory {
    Category1,
    Category2,
    Category3,
    Category4,
    Category5,
}

impl NodeCategory {
    pub fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1..=59 => Some(Self::Category1),
            60..=89 => Some(Self::Category2),
            90..=109 => Some(Self::Category3),
            110..=127 => Some(Self::Category4),
            128..=255 => Some(Self::Category5),
            0 => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawNode<'a> {
    pub tag: u8,
    pub offset: usize,
    pub payload: &'a [u8],
}

impl<'a> RawNode<'a> {
    pub fn category(&self) -> NodeCategory {
        NodeCategory::from_tag(self.tag).expect("RawNode tags are validated during decoding")
    }

    pub fn reader(&self) -> Reader<'a> {
        Reader::new(self.payload)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawNodes<'a> {
    nodes: Vec<RawNode<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AstError {
    Read(ReadError),
    InvalidTag { tag: u8, offset: usize },
    UnsupportedCategory { tag: u8, offset: usize },
}

impl fmt::Display for AstError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => error.fmt(formatter),
            Self::InvalidTag { tag, offset } => {
                write!(formatter, "invalid AST tag {tag} at offset {offset}")
            }
            Self::UnsupportedCategory { tag, offset } => write!(
                formatter,
                "cannot determine a raw AST node boundary for category of tag {tag} at offset {offset}"
            ),
        }
    }
}

impl std::error::Error for AstError {}

impl From<ReadError> for AstError {
    fn from(error: ReadError) -> Self {
        Self::Read(error)
    }
}

impl<'a> RawNodes<'a> {
    /// Decode the length-delimited top-level nodes in an `ASTs` section.
    ///
    /// Category-5 tags carry their own byte length and can therefore be
    /// preserved without understanding the node grammar. Categories 1-4
    /// require tag-specific parsing and are intentionally deferred to the
    /// structural decoder.
    pub fn decode(reader: &mut Reader<'a>) -> Result<Self, AstError> {
        let mut nodes = Vec::new();

        while !reader.is_at_end() {
            let offset = reader.position();
            let tag = reader.read_u8()?;
            let category =
                NodeCategory::from_tag(tag).ok_or(AstError::InvalidTag { tag, offset })?;

            if category != NodeCategory::Category5 {
                return Err(AstError::UnsupportedCategory { tag, offset });
            }

            let length = reader.read_nat()? as usize;
            let payload = reader.read_bytes(length)?;
            nodes.push(RawNode {
                tag,
                offset,
                payload,
            });
        }

        Ok(Self { nodes })
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&RawNode<'a>> {
        self.nodes.get(index)
    }

    pub fn iter(&self) -> impl Iterator<Item = &RawNode<'a>> {
        self.nodes.iter()
    }

    pub fn entries(&self) -> &[RawNode<'a>] {
        &self.nodes
    }
}

#[cfg(test)]
mod tests {
    use super::{AstError, NodeCategory, RawNodes};
    use crate::reader::{ReadError, Reader};

    #[test]
    fn classifies_tags_by_the_tasty_categories() {
        assert_eq!(NodeCategory::from_tag(1), Some(NodeCategory::Category1));
        assert_eq!(NodeCategory::from_tag(60), Some(NodeCategory::Category2));
        assert_eq!(NodeCategory::from_tag(90), Some(NodeCategory::Category3));
        assert_eq!(NodeCategory::from_tag(110), Some(NodeCategory::Category4));
        assert_eq!(NodeCategory::from_tag(128), Some(NodeCategory::Category5));
        assert_eq!(NodeCategory::from_tag(0), None);
    }

    #[test]
    fn decodes_and_borrows_length_delimited_nodes() {
        let bytes = [0x80, 0x82, b'a', b'b', 0x81, 0x80];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();

        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes.get(0).unwrap().tag, 0x80);
        assert_eq!(nodes.get(0).unwrap().offset, 0);
        assert_eq!(nodes.get(0).unwrap().payload, b"ab".as_slice());
        assert_eq!(nodes.get(1).unwrap().tag, 0x81);
        assert!(nodes.get(1).unwrap().payload.is_empty());
        assert!(reader.is_at_end());
    }

    #[test]
    fn rejects_non_length_delimited_nodes_until_structural_decoding_exists() {
        let mut reader = Reader::new(&[0x3c]);

        assert_eq!(
            RawNodes::decode(&mut reader),
            Err(AstError::UnsupportedCategory {
                tag: 0x3c,
                offset: 0
            })
        );
    }

    #[test]
    fn reports_a_truncated_length_delimited_node() {
        let mut reader = Reader::new(&[0x80, 0x83, b'a']);

        assert_eq!(
            RawNodes::decode(&mut reader),
            Err(AstError::Read(ReadError::UnexpectedEof {
                offset: 2,
                needed: 3,
                remaining: 1,
            }))
        );
    }
}
