use crate::reader::{ReadError, Reader};
use crate::term::{RawTree, TermError};
use std::fmt;

pub const TERMREFPKG_TAG: u8 = 64;
pub const THIS_TAG: u8 = 90;
pub const NEW_TAG: u8 = 95;
pub const THROW_TAG: u8 = 96;
pub const ELIDED_TAG: u8 = 104;
pub const PACKAGE_TAG: u8 = 128;
pub const VALDEF_TAG: u8 = 129;
pub const DEFDEF_TAG: u8 = 130;
pub const TYPEDEF_TAG: u8 = 131;
pub const TYPEPARAM_TAG: u8 = 133;
pub const PARAM_TAG: u8 = 134;
pub const APPLY_TAG: u8 = 136;
pub const TYPEAPPLY_TAG: u8 = 137;
pub const TYPED_TAG: u8 = 138;
pub const ASSIGN_TAG: u8 = 139;
pub const BLOCK_TAG: u8 = 140;
pub const RETURN_TAG: u8 = 144;
pub const WHILE_TAG: u8 = 145;
pub const TEMPLATE_TAG: u8 = 156;
pub const IMPORT_TAG: u8 = 132;
pub const EXPORT_TAG: u8 = 177;
pub const IMPORTED_TAG: u8 = 75;
pub const RENAMED_TAG: u8 = 76;
pub const BOUNDED_TAG: u8 = 102;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportExportKind {
    Import,
    Export,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportSelector<'a> {
    Imported { name: u32 },
    Renamed { name: u32 },
    Bounded { type_tree: RawTree<'a> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportExportNode<'a> {
    pub kind: ImportExportKind,
    pub expr: RawTree<'a>,
    pub selectors: Vec<ImportSelector<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfDefNode<'a> {
    pub name: u32,
    pub type_tree: RawTree<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyNode<'a> {
    pub function: RawTree<'a>,
    pub arguments: Vec<RawTree<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockNode<'a> {
    pub expression: RawTree<'a>,
    pub stats: Vec<RawTree<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeApplyNode<'a> {
    pub function: RawTree<'a>,
    pub type_arguments: Vec<RawTree<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedNode<'a> {
    pub expression: RawTree<'a>,
    pub type_tree: RawTree<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignNode<'a> {
    pub left: RawTree<'a>,
    pub right: RawTree<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReturnNode<'a> {
    pub target: u32,
    pub expression: Option<RawTree<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhileNode<'a> {
    pub condition: RawTree<'a>,
    pub body: RawTree<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AstChildNode<'a> {
    pub tag: u8,
    pub child: RawTree<'a>,
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

    pub fn decode_import_export(&self) -> Result<ImportExportNode<'a>, AstError> {
        let kind = match self.tag {
            IMPORT_TAG => ImportExportKind::Import,
            EXPORT_TAG => ImportExportKind::Export,
            _ => {
                return Err(AstError::UnexpectedTag {
                    expected: IMPORT_TAG,
                    actual: self.tag,
                    offset: self.offset,
                });
            }
        };

        let mut reader = self.reader();
        let expr = RawTree::decode(&mut reader)?;
        let mut selectors = Vec::new();

        while !reader.is_at_end() {
            let offset = reader.position();
            match reader.peek_u8()? {
                IMPORTED_TAG => {
                    reader.read_u8()?;
                    selectors.push(ImportSelector::Imported {
                        name: reader.read_nat()?,
                    });
                }
                RENAMED_TAG => {
                    reader.read_u8()?;
                    selectors.push(ImportSelector::Renamed {
                        name: reader.read_nat()?,
                    });
                }
                BOUNDED_TAG => {
                    let tree = RawTree::decode(&mut reader)?;
                    let RawTree::Ast { child, .. } = tree else {
                        return Err(AstError::UnexpectedTag {
                            expected: BOUNDED_TAG,
                            actual: BOUNDED_TAG,
                            offset,
                        });
                    };
                    selectors.push(ImportSelector::Bounded { type_tree: *child });
                }
                actual => {
                    return Err(AstError::UnexpectedTag {
                        expected: IMPORTED_TAG,
                        actual,
                        offset,
                    });
                }
            }
        }

        Ok(ImportExportNode {
            kind,
            expr,
            selectors,
        })
    }

    pub fn decode_apply(&self) -> Result<ApplyNode<'a>, AstError> {
        if self.tag != APPLY_TAG {
            return Err(AstError::UnexpectedTag {
                expected: APPLY_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let function = RawTree::decode(&mut reader)?;
        let mut arguments = Vec::new();
        while !reader.is_at_end() {
            arguments.push(RawTree::decode(&mut reader)?);
        }

        Ok(ApplyNode {
            function,
            arguments,
        })
    }

    pub fn decode_block(&self) -> Result<BlockNode<'a>, AstError> {
        if self.tag != BLOCK_TAG {
            return Err(AstError::UnexpectedTag {
                expected: BLOCK_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let expression = RawTree::decode(&mut reader)?;
        let mut stats = Vec::new();
        while !reader.is_at_end() {
            stats.push(RawTree::decode(&mut reader)?);
        }

        Ok(BlockNode { expression, stats })
    }

    pub fn decode_type_apply(&self) -> Result<TypeApplyNode<'a>, AstError> {
        if self.tag != TYPEAPPLY_TAG {
            return Err(AstError::UnexpectedTag {
                expected: TYPEAPPLY_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let function = RawTree::decode(&mut reader)?;
        let mut type_arguments = Vec::new();
        while !reader.is_at_end() {
            type_arguments.push(RawTree::decode(&mut reader)?);
        }

        Ok(TypeApplyNode {
            function,
            type_arguments,
        })
    }

    pub fn decode_typed(&self) -> Result<TypedNode<'a>, AstError> {
        if self.tag != TYPED_TAG {
            return Err(AstError::UnexpectedTag {
                expected: TYPED_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let expression = RawTree::decode(&mut reader)?;
        let type_tree = RawTree::decode(&mut reader)?;
        if !reader.is_at_end() {
            return Err(AstError::UnsupportedCategory {
                tag: self.tag,
                offset: self.offset,
            });
        }

        Ok(TypedNode {
            expression,
            type_tree,
        })
    }

    pub fn decode_assign(&self) -> Result<AssignNode<'a>, AstError> {
        if self.tag != ASSIGN_TAG {
            return Err(AstError::UnexpectedTag {
                expected: ASSIGN_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let left = RawTree::decode(&mut reader)?;
        let right = RawTree::decode(&mut reader)?;
        if !reader.is_at_end() {
            return Err(AstError::UnsupportedCategory {
                tag: self.tag,
                offset: self.offset,
            });
        }

        Ok(AssignNode { left, right })
    }

    pub fn decode_return(&self) -> Result<ReturnNode<'a>, AstError> {
        if self.tag != RETURN_TAG {
            return Err(AstError::UnexpectedTag {
                expected: RETURN_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let target = reader.read_nat()?;
        let expression = if reader.is_at_end() {
            None
        } else {
            Some(RawTree::decode(&mut reader)?)
        };

        Ok(ReturnNode { target, expression })
    }

    pub fn decode_while(&self) -> Result<WhileNode<'a>, AstError> {
        if self.tag != WHILE_TAG {
            return Err(AstError::UnexpectedTag {
                expected: WHILE_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let condition = RawTree::decode(&mut reader)?;
        let body = RawTree::decode(&mut reader)?;
        if !reader.is_at_end() {
            return Err(AstError::UnsupportedCategory {
                tag: self.tag,
                offset: self.offset,
            });
        }

        Ok(WhileNode { condition, body })
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

impl<'a> RawTree<'a> {
    pub fn decode_ast_child(&self, expected: u8) -> Result<AstChildNode<'a>, AstError> {
        match self {
            RawTree::Ast { tag, child, .. } if *tag == expected => Ok(AstChildNode {
                tag: *tag,
                child: (**child).clone(),
            }),
            tree => {
                let (actual, offset) = raw_tree_tag_offset(tree);
                Err(AstError::UnexpectedTag {
                    expected,
                    actual,
                    offset,
                })
            }
        }
    }

    pub fn decode_this(&self) -> Result<AstChildNode<'a>, AstError> {
        self.decode_ast_child(THIS_TAG)
    }

    pub fn decode_new(&self) -> Result<AstChildNode<'a>, AstError> {
        self.decode_ast_child(NEW_TAG)
    }

    pub fn decode_throw(&self) -> Result<AstChildNode<'a>, AstError> {
        self.decode_ast_child(THROW_TAG)
    }

    pub fn decode_elided(&self) -> Result<AstChildNode<'a>, AstError> {
        self.decode_ast_child(ELIDED_TAG)
    }

    pub fn decode_self_def(&self) -> Result<SelfDefNode<'a>, AstError> {
        match self {
            RawTree::NatAst {
                tag: SELFDEF_TAG,
                value: name,
                child,
                ..
            } => Ok(SelfDefNode {
                name: *name,
                type_tree: (**child).clone(),
            }),
            tree => {
                let (actual, offset) = raw_tree_tag_offset(tree);
                Err(AstError::UnexpectedTag {
                    expected: SELFDEF_TAG,
                    actual,
                    offset,
                })
            }
        }
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

fn raw_tree_tag_offset(tree: &RawTree<'_>) -> (u8, usize) {
    match tree {
        RawTree::Leaf(term) => (term.tag, term.offset),
        RawTree::Ast { tag, offset, .. } | RawTree::NatAst { tag, offset, .. } => (*tag, *offset),
        RawTree::LengthNode(RawNode { tag, offset, .. }) => (*tag, *offset),
    }
}

fn is_modifier_tag(tag: u8) -> bool {
    matches!(tag, 6 | 8..=29 | 31..=49)
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
        APPLY_TAG, ASSIGN_TAG, AstChildNode, AstError, BLOCK_TAG, BOUNDED_TAG, DEFDEF_TAG,
        DefDefBody, DefinitionBody, DefinitionNode, DefinitionTail, ELIDED_TAG, EXPORT_TAG,
        IMPORT_TAG, IMPORTED_TAG, ImportExportKind, ImportSelector, NEW_TAG, NodeCategory,
        PACKAGE_TAG, PARAM_TAG, ParameterNode, RENAMED_TAG, RETURN_TAG, RawNode, RawNodes, RawTree,
        SELFDEF_TAG, SPLITCLAUSE_TAG, TEMPLATE_TAG, TERMREFPKG_TAG, THIS_TAG, THROW_TAG,
        TYPEAPPLY_TAG, TYPED_TAG, TYPEDEF_TAG, TYPEPARAM_TAG, TypeApplyNode, TypedNode, VALDEF_TAG,
        WHILE_TAG,
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
    fn rejects_a_package_with_an_invalid_path_tree() {
        let node = RawNode {
            tag: PACKAGE_TAG,
            offset: 4,
            payload: &[2],
        };

        assert_eq!(
            node.decode_package(),
            Err(AstError::UnexpectedTag {
                expected: TERMREFPKG_TAG,
                actual: 2,
                offset: 4,
            })
        );
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
    fn decodes_a_valdef_body_without_rhs_or_definition_tail() {
        let bytes = [VALDEF_TAG, 0x82, 0x81, 2];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let body = nodes.get(0).unwrap().decode_definition_body().unwrap();

        assert!(matches!(
            body,
            DefinitionBody::ValDef {
                rhs: None,
                ref tail,
                ..
            } if tail.is_empty()
        ));
    }

    #[test]
    fn reports_a_truncated_definition_body() {
        let node = RawNode {
            tag: VALDEF_TAG,
            offset: 0,
            payload: &[1],
        };

        assert!(node.decode_definition_body().is_err());
    }

    #[test]
    fn recognizes_every_assigned_category_one_modifier_and_rejects_holes() {
        let assigned = [
            6, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28,
            29, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49,
        ];

        for tag in assigned {
            assert!(
                super::is_modifier_tag(tag),
                "modifier tag {tag} was rejected"
            );
        }
        for tag in [0, 1, 2, 5, 7, 30, 50, 255] {
            assert!(
                !super::is_modifier_tag(tag),
                "non-modifier tag {tag} was accepted"
            );
        }
    }

    #[test]
    fn decodes_each_assigned_category_one_modifier_as_a_definition_tail() {
        let assigned = [
            6, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28,
            29, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49,
        ];

        for modifier in assigned {
            let bytes = [VALDEF_TAG, 0x83, 0x81, 2, modifier];
            let mut reader = Reader::new(&bytes);
            let nodes = RawNodes::decode(&mut reader).unwrap();
            let body = nodes.get(0).unwrap().decode_definition_body().unwrap();

            assert_eq!(
                body,
                DefinitionBody::ValDef {
                    type_tree: RawTree::Leaf(crate::term::SimpleTerm {
                        tag: 2,
                        offset: 1,
                        value: crate::term::TermValue::Unit,
                    }),
                    rhs: None,
                    tail: vec![DefinitionTail::Modifier(modifier)],
                },
                "modifier tag {modifier} was not decoded as a definition tail"
            );
        }
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
    fn decodes_an_empty_template_structure() {
        let bytes = [TEMPLATE_TAG, 0x80];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let structure = nodes.get(0).unwrap().decode_template_structure().unwrap();

        assert!(structure.type_params.is_empty());
        assert!(structure.term_params.is_empty());
        assert!(structure.parents.is_empty());
        assert!(structure.self_def.is_none());
        assert!(!structure.split_clause);
        assert!(structure.stats.is_empty());
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
        let self_def = structure
            .self_def
            .as_ref()
            .unwrap()
            .decode_self_def()
            .unwrap();
        assert_eq!(self_def.name, 1);
        assert!(matches!(self_def.type_tree, RawTree::Leaf(_)));
        assert!(!structure.split_clause);
        assert_eq!(structure.stats.len(), 1);
    }

    #[test]
    fn rejects_a_tree_that_is_not_a_self_definition() {
        let mut reader = Reader::new(&[TERMREFPKG_TAG, 0x81]);
        let tree = RawTree::decode(&mut reader).unwrap();

        assert_eq!(
            tree.decode_self_def(),
            Err(AstError::UnexpectedTag {
                expected: SELFDEF_TAG,
                actual: TERMREFPKG_TAG,
                offset: 0,
            })
        );
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
    fn decodes_a_parameter_body_without_a_definition_tail() {
        let bytes = [PARAM_TAG, 0x82, 0x81, 2];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let body = nodes
            .get(0)
            .unwrap()
            .decode_parameter()
            .unwrap()
            .decode_body()
            .unwrap();

        assert!(matches!(body.type_tree, RawTree::Leaf(_)));
        assert!(body.tail.is_empty());
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

    #[test]
    fn decodes_defdef_type_and_term_parameters_rhs_split_and_annotation() {
        let bytes = [
            DEFDEF_TAG,
            0x91,
            0x81,
            TYPEPARAM_TAG,
            0x83,
            0x82,
            TERMREFPKG_TAG,
            0x83,
            PARAM_TAG,
            0x83,
            0x84,
            TERMREFPKG_TAG,
            0x85,
            SPLITCLAUSE_TAG,
            TERMREFPKG_TAG,
            0x86,
            2,
            173,
            0x80,
        ];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let body = nodes.get(0).unwrap().decode_defdef_body().unwrap();

        assert_eq!(body.parameters.len(), 2);
        assert!(matches!(
            body.parameters[0],
            ParameterNode::TypeParam { .. }
        ));
        assert!(matches!(
            body.parameters[1],
            ParameterNode::TermParam { .. }
        ));
        assert_eq!(body.clauses, vec![SPLITCLAUSE_TAG]);
        assert!(matches!(body.return_type, RawTree::Leaf(_)));
        assert!(matches!(body.rhs, Some(RawTree::Leaf(_))));
        assert!(
            matches!(body.tail.as_slice(), [DefinitionTail::Annotation(annotation)] if annotation.tag == 173)
        );
    }

    #[test]
    fn reports_a_defdef_without_a_return_type() {
        let node = RawNode {
            tag: DEFDEF_TAG,
            offset: 0,
            payload: &[1],
        };

        assert!(node.decode_defdef_body().is_err());
    }

    #[test]
    fn decodes_import_selectors_and_bounded_types() {
        let bytes = [
            IMPORT_TAG,
            0x89,
            TERMREFPKG_TAG,
            0x81,
            IMPORTED_TAG,
            0x85,
            RENAMED_TAG,
            0x86,
            BOUNDED_TAG,
            TERMREFPKG_TAG,
            0x87,
        ];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let import = nodes.get(0).unwrap().decode_import_export().unwrap();

        assert_eq!(import.kind, ImportExportKind::Import);
        assert!(matches!(import.expr, RawTree::Leaf(_)));
        assert!(matches!(
            import.selectors.as_slice(),
            [
                ImportSelector::Imported { name: 5 },
                ImportSelector::Renamed { name: 6 },
                ImportSelector::Bounded {
                    type_tree: RawTree::Leaf(_)
                }
            ]
        ));
    }

    #[test]
    fn decodes_export_nodes_with_the_same_selector_grammar() {
        let bytes = [EXPORT_TAG, 0x84, TERMREFPKG_TAG, 0x81, IMPORTED_TAG, 0x85];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let export = nodes.get(0).unwrap().decode_import_export().unwrap();

        assert_eq!(export.kind, ImportExportKind::Export);
        assert_eq!(export.selectors, vec![ImportSelector::Imported { name: 5 }]);
    }

    #[test]
    fn decodes_import_and_export_without_selectors() {
        for tag in [IMPORT_TAG, EXPORT_TAG] {
            let bytes = [tag, 0x81, 2];
            let mut reader = Reader::new(&bytes);
            let nodes = RawNodes::decode(&mut reader).unwrap();
            let node = nodes.get(0).unwrap().decode_import_export().unwrap();

            assert!(node.selectors.is_empty());
        }
    }

    #[test]
    fn rejects_an_import_selector_with_an_unknown_tag() {
        let node = RawNode {
            tag: IMPORT_TAG,
            offset: 0,
            payload: &[2, 200],
        };

        assert_eq!(
            node.decode_import_export(),
            Err(AstError::UnexpectedTag {
                expected: IMPORTED_TAG,
                actual: 200,
                offset: 1,
            })
        );
    }

    #[test]
    fn decodes_apply_function_and_arguments() {
        let bytes = [APPLY_TAG, 0x85, TERMREFPKG_TAG, 0x81, 70, 0x82, 4];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let apply = nodes.get(0).unwrap().decode_apply().unwrap();

        assert!(matches!(apply.function, RawTree::Leaf(_)));
        assert_eq!(apply.arguments.len(), 2);
    }

    #[test]
    fn decodes_apply_without_arguments() {
        let bytes = [APPLY_TAG, 0x81, 2];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();

        assert!(
            nodes
                .get(0)
                .unwrap()
                .decode_apply()
                .unwrap()
                .arguments
                .is_empty()
        );
    }

    #[test]
    fn decodes_block_expression_and_stats() {
        let bytes = [BLOCK_TAG, 0x83, 2, VALDEF_TAG, 0x80];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let block = nodes.get(0).unwrap().decode_block().unwrap();

        assert!(matches!(block.expression, RawTree::Leaf(_)));
        assert_eq!(block.stats.len(), 1);
        assert!(matches!(block.stats[0], RawTree::LengthNode(_)));
    }

    #[test]
    fn decodes_block_without_stats() {
        let bytes = [BLOCK_TAG, 0x81, 2];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();

        assert!(
            nodes
                .get(0)
                .unwrap()
                .decode_block()
                .unwrap()
                .stats
                .is_empty()
        );
    }

    #[test]
    fn decodes_type_apply_function_and_type_arguments() {
        let bytes = [
            TYPEAPPLY_TAG,
            0x86,
            TERMREFPKG_TAG,
            0x81,
            TERMREFPKG_TAG,
            0x82,
            TERMREFPKG_TAG,
            0x83,
        ];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let type_apply = nodes.get(0).unwrap().decode_type_apply().unwrap();

        assert!(matches!(type_apply.function, RawTree::Leaf(_)));
        assert_eq!(type_apply.type_arguments.len(), 2);
        assert!(matches!(type_apply, TypeApplyNode { .. }));
    }

    #[test]
    fn decodes_type_apply_without_type_arguments() {
        let bytes = [TYPEAPPLY_TAG, 0x81, 2];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();

        assert!(
            nodes
                .get(0)
                .unwrap()
                .decode_type_apply()
                .unwrap()
                .type_arguments
                .is_empty()
        );
    }

    #[test]
    fn decodes_typed_expression_and_type() {
        let bytes = [TYPED_TAG, 0x84, TERMREFPKG_TAG, 0x81, TERMREFPKG_TAG, 0x82];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let typed = nodes.get(0).unwrap().decode_typed().unwrap();

        assert!(matches!(typed.expression, RawTree::Leaf(_)));
        assert!(matches!(typed.type_tree, RawTree::Leaf(_)));
        assert!(matches!(typed, TypedNode { .. }));
    }

    #[test]
    fn rejects_a_typed_node_with_an_extra_tree() {
        let node = RawNode {
            tag: TYPED_TAG,
            offset: 0,
            payload: &[2, 2, 2],
        };

        assert_eq!(
            node.decode_typed(),
            Err(AstError::UnsupportedCategory {
                tag: TYPED_TAG,
                offset: 0,
            })
        );
    }

    #[test]
    fn decodes_assign_left_and_right_operands() {
        let bytes = [ASSIGN_TAG, 0x84, TERMREFPKG_TAG, 0x81, TERMREFPKG_TAG, 0x82];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let assign = nodes.get(0).unwrap().decode_assign().unwrap();

        assert!(matches!(assign.left, RawTree::Leaf(_)));
        assert!(matches!(assign.right, RawTree::Leaf(_)));
    }

    #[test]
    fn reports_an_assign_node_without_a_right_operand() {
        let node = RawNode {
            tag: ASSIGN_TAG,
            offset: 0,
            payload: &[2],
        };

        assert!(matches!(node.decode_assign(), Err(AstError::Term(_))));
    }

    #[test]
    fn decodes_return_target_with_optional_expression() {
        let bytes = [RETURN_TAG, 0x83, 0x85, TERMREFPKG_TAG, 0x81];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let return_node = nodes.get(0).unwrap().decode_return().unwrap();

        assert_eq!(return_node.target, 5);
        assert!(matches!(return_node.expression, Some(RawTree::Leaf(_))));
    }

    #[test]
    fn decodes_return_without_an_expression() {
        let bytes = [RETURN_TAG, 0x81, 0x85];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let return_node = nodes.get(0).unwrap().decode_return().unwrap();

        assert_eq!(return_node.target, 5);
        assert!(return_node.expression.is_none());
    }

    #[test]
    fn reports_a_return_node_without_a_target() {
        let node = RawNode {
            tag: RETURN_TAG,
            offset: 0,
            payload: &[],
        };

        assert!(matches!(node.decode_return(), Err(AstError::Read(_))));
    }

    #[test]
    fn decodes_while_condition_and_body() {
        let bytes = [WHILE_TAG, 0x83, 3, TERMREFPKG_TAG, 0x81];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let while_node = nodes.get(0).unwrap().decode_while().unwrap();

        assert!(matches!(while_node.condition, RawTree::Leaf(_)));
        assert!(matches!(while_node.body, RawTree::Leaf(_)));
    }

    #[test]
    fn reports_a_while_node_without_a_body() {
        let node = RawNode {
            tag: WHILE_TAG,
            offset: 0,
            payload: &[2],
        };

        assert!(matches!(node.decode_while(), Err(AstError::Term(_))));
    }

    #[test]
    fn decodes_category_three_ast_children() {
        let mut reader = Reader::new(&[
            THIS_TAG,
            TERMREFPKG_TAG,
            0x81,
            NEW_TAG,
            TERMREFPKG_TAG,
            0x82,
            THROW_TAG,
            TERMREFPKG_TAG,
            0x83,
            ELIDED_TAG,
            TERMREFPKG_TAG,
            0x84,
        ]);

        let this = RawTree::decode(&mut reader).unwrap().decode_this().unwrap();
        let new = RawTree::decode(&mut reader).unwrap().decode_new().unwrap();
        let throw = RawTree::decode(&mut reader)
            .unwrap()
            .decode_throw()
            .unwrap();
        let elided = RawTree::decode(&mut reader)
            .unwrap()
            .decode_elided()
            .unwrap();

        assert_eq!(this.tag, THIS_TAG);
        assert_eq!(new.tag, NEW_TAG);
        assert!(matches!(throw.child, RawTree::Leaf(_)));
        assert!(matches!(elided.child, RawTree::Leaf(_)));
        assert!(matches!(this, AstChildNode { .. }));
    }
}
