use crate::reader::{ReadError, Reader};
use crate::term::{RawTree, TermError};
use std::fmt;

pub const TERMREFPKG_TAG: u8 = 64;
pub const PACKAGE_TAG: u8 = 128;
pub const VALDEF_TAG: u8 = 129;
pub const DEFDEF_TAG: u8 = 130;
pub const TYPEDEF_TAG: u8 = 131;
pub const TYPEPARAM_TAG: u8 = 133;
pub const PARAM_TAG: u8 = 134;
pub const TEMPLATE_TAG: u8 = 156;
pub const IMPORT_TAG: u8 = 132;
pub const EXPORT_TAG: u8 = 177;
pub const SELFDEF_TAG: u8 = 118;
pub const EMPTYCLAUSE_TAG: u8 = 45;
pub const SPLITCLAUSE_TAG: u8 = 46;

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
pub struct PackageNode<'a> {
    pub path_name: u32,
    pub stats: RawNodes<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefinitionNode<'a> {
    ValDef { name: u32, body: &'a [u8] },
    DefDef { name: u32, body: &'a [u8] },
    TypeDef { name: u32, body: &'a [u8] },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefinitionBody<'a> {
    ValDef {
        type_tree: RawTree<'a>,
        rhs: Option<RawTree<'a>>,
        tail: Vec<DefinitionTail<'a>>,
    },
    TypeDef {
        type_or_template: RawTree<'a>,
        tail: Vec<DefinitionTail<'a>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefDefBody<'a> {
    pub parameters: Vec<ParameterNode<'a>>,
    pub clauses: Vec<u8>,
    pub return_type: RawTree<'a>,
    pub rhs: Option<RawTree<'a>>,
    pub tail: Vec<DefinitionTail<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefinitionTail<'a> {
    Modifier(u8),
    Annotation(RawNode<'a>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParameterNode<'a> {
    TypeParam { name: u32, body: &'a [u8] },
    TermParam { name: u32, body: &'a [u8] },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterBody<'a> {
    pub type_tree: RawTree<'a>,
    pub tail: Vec<DefinitionTail<'a>>,
}

impl<'a> ParameterNode<'a> {
    pub fn name(&self) -> u32 {
        match self {
            Self::TypeParam { name, .. } | Self::TermParam { name, .. } => *name,
        }
    }

    pub fn body(&self) -> &'a [u8] {
        match self {
            Self::TypeParam { body, .. } | Self::TermParam { body, .. } => body,
        }
    }

    pub fn decode_body(&self) -> Result<ParameterBody<'a>, AstError> {
        let mut reader = Reader::new(self.body());
        let type_tree = RawTree::decode(&mut reader)?;
        let tail = read_definition_tail(&mut reader)?;
        Ok(ParameterBody { type_tree, tail })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateNode<'a> {
    pub type_params: Vec<ParameterNode<'a>>,
    pub term_params: Vec<ParameterNode<'a>>,
    pub remainder: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateStructure<'a> {
    pub type_params: Vec<ParameterNode<'a>>,
    pub term_params: Vec<ParameterNode<'a>>,
    pub parents: Vec<RawTree<'a>>,
    pub self_def: Option<RawTree<'a>>,
    pub split_clause: bool,
    pub stats: RawNodes<'a>,
}

impl<'a> DefinitionNode<'a> {
    pub fn name(&self) -> u32 {
        match self {
            Self::ValDef { name, .. } | Self::DefDef { name, .. } | Self::TypeDef { name, .. } => {
                *name
            }
        }
    }

    pub fn body(&self) -> &'a [u8] {
        match self {
            Self::ValDef { body, .. } | Self::DefDef { body, .. } | Self::TypeDef { body, .. } => {
                body
            }
        }
    }

    pub fn body_reader(&self) -> Reader<'a> {
        Reader::new(self.body())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AstError {
    Read(ReadError),
    Term(TermError),
    InvalidTag {
        tag: u8,
        offset: usize,
    },
    UnsupportedCategory {
        tag: u8,
        offset: usize,
    },
    UnexpectedTag {
        expected: u8,
        actual: u8,
        offset: usize,
    },
}

impl fmt::Display for AstError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => error.fmt(formatter),
            Self::Term(error) => error.fmt(formatter),
            Self::InvalidTag { tag, offset } => {
                write!(formatter, "invalid AST tag {tag} at offset {offset}")
            }
            Self::UnsupportedCategory { tag, offset } => write!(
                formatter,
                "cannot determine a raw AST node boundary for category of tag {tag} at offset {offset}"
            ),
            Self::UnexpectedTag {
                expected,
                actual,
                offset,
            } => write!(
                formatter,
                "expected AST tag {expected} at offset {offset}, found {actual}"
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

impl From<TermError> for AstError {
    fn from(error: TermError) -> Self {
        Self::Term(error)
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

impl<'a> RawNode<'a> {
    pub fn decode_package(&self) -> Result<PackageNode<'a>, AstError> {
        let mut reader = self.reader();
        if self.tag != PACKAGE_TAG {
            return Err(AstError::UnexpectedTag {
                expected: PACKAGE_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let path_offset = reader.position();
        let path_tag = reader.read_u8()?;
        if path_tag != TERMREFPKG_TAG {
            return Err(AstError::UnexpectedTag {
                expected: TERMREFPKG_TAG,
                actual: path_tag,
                offset: self.offset + path_offset,
            });
        }
        let path_name = reader.read_nat()?;
        let stats = RawNodes::decode(&mut reader)?;

        Ok(PackageNode { path_name, stats })
    }

    pub fn decode_definition(&self) -> Result<DefinitionNode<'a>, AstError> {
        let mut reader = self.reader();
        let name = match self.tag {
            VALDEF_TAG | DEFDEF_TAG | TYPEDEF_TAG => reader.read_nat()?,
            _ => {
                return Err(AstError::UnexpectedTag {
                    expected: VALDEF_TAG,
                    actual: self.tag,
                    offset: self.offset,
                });
            }
        };
        let body = reader.read_bytes(reader.remaining())?;

        Ok(match self.tag {
            VALDEF_TAG => DefinitionNode::ValDef { name, body },
            DEFDEF_TAG => DefinitionNode::DefDef { name, body },
            TYPEDEF_TAG => DefinitionNode::TypeDef { name, body },
            _ => unreachable!("definition tag was checked above"),
        })
    }

    pub fn decode_definition_body(&self) -> Result<DefinitionBody<'a>, AstError> {
        let mut reader = self.reader();
        let _name = reader.read_nat()?;
        let first = RawTree::decode(&mut reader)?;

        match self.tag {
            VALDEF_TAG => {
                let rhs = if reader.is_at_end() || is_tail_tag(reader.peek_u8()?) {
                    None
                } else {
                    Some(RawTree::decode(&mut reader)?)
                };
                let tail = read_definition_tail(&mut reader)?;
                Ok(DefinitionBody::ValDef {
                    type_tree: first,
                    rhs,
                    tail,
                })
            }
            TYPEDEF_TAG => Ok(DefinitionBody::TypeDef {
                type_or_template: first,
                tail: read_definition_tail(&mut reader)?,
            }),
            _ => Err(AstError::UnexpectedTag {
                expected: VALDEF_TAG,
                actual: self.tag,
                offset: self.offset,
            }),
        }
    }

    pub fn decode_defdef_body(&self) -> Result<DefDefBody<'a>, AstError> {
        if self.tag != DEFDEF_TAG {
            return Err(AstError::UnexpectedTag {
                expected: DEFDEF_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let _name = reader.read_nat()?;
        let mut parameters = Vec::new();
        let mut clauses = Vec::new();

        while !reader.is_at_end() {
            match reader.peek_u8()? {
                TYPEPARAM_TAG | PARAM_TAG => {
                    let parameter = match RawTree::decode(&mut reader)? {
                        RawTree::LengthNode(raw) => raw.decode_parameter()?,
                        _ => unreachable!("parameter tags are category-five tags"),
                    };
                    parameters.push(parameter);
                }
                EMPTYCLAUSE_TAG | SPLITCLAUSE_TAG => clauses.push(reader.read_u8()?),
                _ => break,
            }
        }

        let return_type = RawTree::decode(&mut reader)?;
        let rhs = if reader.is_at_end() || is_tail_tag(reader.peek_u8()?) {
            None
        } else {
            Some(RawTree::decode(&mut reader)?)
        };
        let tail = read_definition_tail(&mut reader)?;

        Ok(DefDefBody {
            parameters,
            clauses,
            return_type,
            rhs,
            tail,
        })
    }

    pub fn decode_parameter(&self) -> Result<ParameterNode<'a>, AstError> {
        let mut reader = self.reader();
        let name = reader.read_nat()?;
        let body = reader.read_bytes(reader.remaining())?;

        match self.tag {
            TYPEPARAM_TAG => Ok(ParameterNode::TypeParam { name, body }),
            PARAM_TAG => Ok(ParameterNode::TermParam { name, body }),
            _ => Err(AstError::UnexpectedTag {
                expected: TYPEPARAM_TAG,
                actual: self.tag,
                offset: self.offset,
            }),
        }
    }

    pub fn decode_template(&self) -> Result<TemplateNode<'a>, AstError> {
        if self.tag != TEMPLATE_TAG {
            return Err(AstError::UnexpectedTag {
                expected: TEMPLATE_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let mut type_params = Vec::new();
        let mut term_params = Vec::new();

        while !reader.is_at_end() && matches!(reader.peek_u8()?, TYPEPARAM_TAG | PARAM_TAG) {
            let parameter = match RawTree::decode(&mut reader)? {
                RawTree::LengthNode(raw) => raw.decode_parameter()?,
                _ => unreachable!("parameter tags are category-five tags"),
            };
            match parameter {
                ParameterNode::TypeParam { .. } => type_params.push(parameter),
                ParameterNode::TermParam { .. } => term_params.push(parameter),
            }
        }

        let remainder = reader.read_bytes(reader.remaining())?;
        Ok(TemplateNode {
            type_params,
            term_params,
            remainder,
        })
    }

    pub fn decode_template_structure(&self) -> Result<TemplateStructure<'a>, AstError> {
        let template = self.decode_template()?;
        let mut reader = Reader::new(template.remainder);
        let mut parents = Vec::new();
        let mut self_def = None;
        let mut split_clause = false;
        let mut stats = Vec::new();
        let mut in_stats = false;

        while !reader.is_at_end() {
            let tag = reader.peek_u8()?;
            if !in_stats && tag == SPLITCLAUSE_TAG {
                reader.read_u8()?;
                split_clause = true;
                in_stats = true;
                continue;
            }

            if !in_stats && tag == SELFDEF_TAG {
                self_def = Some(RawTree::decode(&mut reader)?);
                continue;
            }

            if !in_stats && is_template_stat_tag(tag) {
                in_stats = true;
            }

            if in_stats {
                let offset = reader.position();
                match RawTree::decode(&mut reader)? {
                    RawTree::LengthNode(raw) => stats.push(raw),
                    _ => {
                        return Err(AstError::UnexpectedTag {
                            expected: PACKAGE_TAG,
                            actual: tag,
                            offset,
                        });
                    }
                }
            } else {
                parents.push(RawTree::decode(&mut reader)?);
            }
        }

        Ok(TemplateStructure {
            type_params: template.type_params,
            term_params: template.term_params,
            parents,
            self_def,
            split_clause,
            stats: RawNodes { nodes: stats },
        })
    }
}

fn is_template_stat_tag(tag: u8) -> bool {
    matches!(
        tag,
        PACKAGE_TAG
            | VALDEF_TAG
            | DEFDEF_TAG
            | TYPEDEF_TAG
            | IMPORT_TAG
            | TYPEPARAM_TAG
            | PARAM_TAG
            | EXPORT_TAG
    )
}

fn is_modifier_tag(tag: u8) -> bool {
    (6..=49).contains(&tag)
}

fn is_tail_tag(tag: u8) -> bool {
    is_modifier_tag(tag) || tag == 173
}

fn read_definition_tail<'a>(reader: &mut Reader<'a>) -> Result<Vec<DefinitionTail<'a>>, AstError> {
    let mut tail = Vec::new();
    while !reader.is_at_end() {
        let offset = reader.position();
        let tag = reader.read_u8()?;
        if is_modifier_tag(tag) {
            tail.push(DefinitionTail::Modifier(tag));
        } else if tag == 173 {
            let length = reader.read_nat()? as usize;
            let payload = reader.read_bytes(length)?;
            tail.push(DefinitionTail::Annotation(RawNode {
                tag,
                offset,
                payload,
            }));
        } else {
            return Err(AstError::UnexpectedTag {
                expected: 6,
                actual: tag,
                offset,
            });
        }
    }
    Ok(tail)
}

#[cfg(test)]
mod tests {
    use super::{
        AstError, DEFDEF_TAG, DefDefBody, DefinitionBody, DefinitionNode, DefinitionTail,
        NodeCategory, PACKAGE_TAG, PARAM_TAG, RawNodes, RawTree, SELFDEF_TAG, TEMPLATE_TAG,
        TERMREFPKG_TAG, TYPEDEF_TAG, TYPEPARAM_TAG, VALDEF_TAG,
    };
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

    #[test]
    fn decodes_a_package_path_and_its_raw_stats() {
        let bytes = [PACKAGE_TAG, 0x84, TERMREFPKG_TAG, 0x85, 0x81, 0x80];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let package = nodes.get(0).unwrap().decode_package().unwrap();

        assert_eq!(package.path_name, 5);
        assert_eq!(package.stats.len(), 1);
        assert_eq!(package.stats.get(0).unwrap().tag, 129);
    }

    #[test]
    fn decodes_a_definition_name_and_preserves_its_remaining_body() {
        let bytes = [VALDEF_TAG, 0x83, 0x81, b'a', b'b'];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let definition = nodes.get(0).unwrap().decode_definition().unwrap();

        assert_eq!(
            definition,
            DefinitionNode::ValDef {
                name: 1,
                body: b"ab"
            }
        );
        assert_eq!(definition.body_reader().remaining(), 2);
    }

    #[test]
    fn decodes_a_typedef_using_the_same_name_header() {
        let bytes = [TYPEDEF_TAG, 0x81, 0x81];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();

        assert_eq!(
            nodes.get(0).unwrap().decode_definition(),
            Ok(DefinitionNode::TypeDef { name: 1, body: b"" })
        );
    }

    #[test]
    fn decodes_a_valdef_body_with_a_type_rhs_and_modifier() {
        let bytes = [VALDEF_TAG, 0x86, 0x81, 64, 0x82, 70, 0x82, 17];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let body = nodes.get(0).unwrap().decode_definition_body().unwrap();

        assert!(matches!(
            body,
            DefinitionBody::ValDef {
                type_tree: _,
                rhs: Some(_),
                ref tail,
            } if tail == &[DefinitionTail::Modifier(17)]
        ));
    }

    #[test]
    fn decodes_a_typedef_body_using_the_same_first_tree_rule() {
        let bytes = [TYPEDEF_TAG, 0x83, 0x81, 2, 17];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();

        assert!(matches!(
            nodes.get(0).unwrap().decode_definition_body(),
            Ok(DefinitionBody::TypeDef {
                type_or_template: _,
                tail,
            }) if tail == vec![DefinitionTail::Modifier(17)]
        ));
    }

    #[test]
    fn decodes_template_parameter_prefix_and_preserves_the_remainder() {
        let bytes = [
            TEMPLATE_TAG,
            0x8c,
            TYPEPARAM_TAG,
            0x83,
            0x81,
            2,
            17,
            PARAM_TAG,
            0x82,
            0x82,
            2,
            46,
            VALDEF_TAG,
            0x80,
        ];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let template = nodes.get(0).unwrap().decode_template().unwrap();

        assert_eq!(template.type_params.len(), 1);
        assert_eq!(template.term_params.len(), 1);
        assert_eq!(template.type_params[0].name(), 1);
        assert_eq!(template.term_params[0].name(), 2);
        assert_eq!(template.remainder, &[46, VALDEF_TAG, 0x80]);
    }

    #[test]
    fn decodes_template_parents_self_and_stats() {
        let bytes = [
            TEMPLATE_TAG,
            0x87,
            136,
            0x80,
            SELFDEF_TAG,
            0x81,
            2,
            VALDEF_TAG,
            0x80,
        ];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let structure = nodes.get(0).unwrap().decode_template_structure().unwrap();

        assert_eq!(structure.parents.len(), 1);
        assert!(structure.self_def.is_some());
        assert!(!structure.split_clause);
        assert_eq!(structure.stats.len(), 1);
    }

    #[test]
    fn decodes_a_parameter_body_type_and_modifier() {
        let bytes = [PARAM_TAG, 0x83, 0x81, 2, 17];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let parameter = nodes.get(0).unwrap().decode_parameter().unwrap();
        let body = parameter.decode_body().unwrap();

        assert!(matches!(body.type_tree, RawTree::Leaf(_)));
        assert_eq!(body.tail, vec![DefinitionTail::Modifier(17)]);
    }

    #[test]
    fn decodes_a_defdef_parameters_clauses_return_type_and_tail() {
        let bytes = [
            DEFDEF_TAG, 0x89, 0x81, PARAM_TAG, 0x83, 0x82, 2, 17, 45, 2, 17,
        ];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let body = nodes.get(0).unwrap().decode_defdef_body().unwrap();

        assert_eq!(body.parameters.len(), 1);
        assert_eq!(body.clauses, vec![45]);
        assert!(matches!(body.return_type, RawTree::Leaf(_)));
        assert!(body.rhs.is_none());
        assert_eq!(body.tail, vec![DefinitionTail::Modifier(17)]);
        assert!(matches!(body, DefDefBody { .. }));
    }
}
