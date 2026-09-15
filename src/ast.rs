use crate::reader::{ReadError, Reader};
use crate::term::{RawTree, TermError, is_known_category5_tag};
use std::fmt;

pub const TERMREFPKG_TAG: u8 = 64;
pub const THIS_TAG: u8 = 90;
pub const QUALTHIS_TAG: u8 = 91;
pub const CLASSCONST_TAG: u8 = 92;
pub const BYNAMETYPE_TAG: u8 = 93;
pub const BYNAMETPT_TAG: u8 = 94;
pub const NEW_TAG: u8 = 95;
pub const THROW_TAG: u8 = 96;
pub const IMPLICITARG_TAG: u8 = 97;
pub const PRIVATEQUALIFIED_TAG: u8 = 98;
pub const PROTECTEDQUALIFIED_TAG: u8 = 99;
pub const RECTYPE_TAG: u8 = 100;
pub const SINGLETONTPT_TAG: u8 = 101;
pub const EXPLICITTPT_TAG: u8 = 103;
pub const ELIDED_TAG: u8 = 104;
pub const PACKAGE_TAG: u8 = 128;
pub const VALDEF_TAG: u8 = 129;
pub const DEFDEF_TAG: u8 = 130;
pub const TYPEDEF_TAG: u8 = 131;
pub const ANDTYPE_TAG: u8 = 165;
pub const ORTYPE_TAG: u8 = 167;
pub const APPLIEDTYPE_TAG: u8 = 161;
pub const APPLIEDTPT_TAG: u8 = 162;
pub const TYPEBOUNDS_TAG: u8 = 163;
pub const TYPEBOUNDSTPT_TAG: u8 = 164;
pub const SUPERTYPE_TAG: u8 = 158;
pub const MATCHCASETYPE_TAG: u8 = 192;
pub const POLYTYPE_TAG: u8 = 169;
pub const TYPELAMBDATYPE_TAG: u8 = 170;
pub const METHODTYPE_TAG: u8 = 180;
pub const ANNOTATEDTYPE_TAG: u8 = 153;
pub const ANNOTATEDTPT_TAG: u8 = 154;
pub const PARAMTYPE_TAG: u8 = 172;
pub const FLEXIBLETYPE_TAG: u8 = 193;
pub const TYPEPARAM_TAG: u8 = 133;
pub const PARAM_TAG: u8 = 134;
pub const BIND_TAG: u8 = 150;
pub const ALTERNATIVE_TAG: u8 = 151;
pub const UNAPPLY_TAG: u8 = 152;
pub const CASEDEF_TAG: u8 = 155;
pub const TERMREFIN_TAG: u8 = 174;
pub const TYPEREFIN_TAG: u8 = 175;
pub const SELECTIN_TAG: u8 = 176;
pub const APPLY_TAG: u8 = 136;
pub const TYPEAPPLY_TAG: u8 = 137;
pub const TYPED_TAG: u8 = 138;
pub const ASSIGN_TAG: u8 = 139;
pub const BLOCK_TAG: u8 = 140;
pub const IF_TAG: u8 = 141;
pub const LAMBDA_TAG: u8 = 142;
pub const RETURN_TAG: u8 = 144;
pub const WHILE_TAG: u8 = 145;
pub const SELECTOUTER_TAG: u8 = 148;
pub const REPEATED_TAG: u8 = 149;
pub const TEMPLATE_TAG: u8 = 156;
pub const SUPER_TAG: u8 = 157;
pub const IMPORT_TAG: u8 = 132;
pub const EXPORT_TAG: u8 = 177;
pub const IMPORTED_TAG: u8 = 75;
pub const RENAMED_TAG: u8 = 76;
pub const BOUNDED_TAG: u8 = 102;
pub const IDENT_TAG: u8 = 110;
pub const IDENTTPT_TAG: u8 = 111;
pub const SELECT_TAG: u8 = 112;
pub const SELECTTPT_TAG: u8 = 113;
pub const TERMREFSYMBOL_TAG: u8 = 114;
pub const TERMREF_TAG: u8 = 115;
pub const TYPEREFSYMBOL_TAG: u8 = 116;
pub const TYPEREF_TAG: u8 = 117;
pub const SELFDEF_TAG: u8 = 118;
pub const NAMEDARG_TAG: u8 = 119;
pub const EMPTYCLAUSE_TAG: u8 = 45;
pub const SPLITCLAUSE_TAG: u8 = 46;
pub const INLINE_TAG: u8 = 17;

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
pub struct IfNode<'a> {
    pub inline: bool,
    pub condition: RawTree<'a>,
    pub then_branch: RawTree<'a>,
    pub else_branch: RawTree<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LambdaNode<'a> {
    pub method: RawTree<'a>,
    pub target_type: Option<RawTree<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuperNode<'a> {
    pub this_term: RawTree<'a>,
    pub mixin_type: Option<RawTree<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepeatedNode<'a> {
    pub element_type: RawTree<'a>,
    pub elements: Vec<RawTree<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectOuterNode<'a> {
    pub levels: u32,
    pub qualifier: RawTree<'a>,
    pub underlying_type: RawTree<'a>,
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
pub struct NamedArgNode<'a> {
    pub name: u32,
    pub argument: RawTree<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentNode<'a> {
    pub tag: u8,
    pub name: u32,
    pub type_tree: RawTree<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceNode<'a> {
    pub tag: u8,
    pub reference: u32,
    pub qualifier: RawTree<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectNode<'a> {
    pub tag: u8,
    pub name: u32,
    pub qualifier: RawTree<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InReferenceNode<'a> {
    pub tag: u8,
    pub name: u32,
    pub qualifier: RawTree<'a>,
    pub underlying_type: RawTree<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectInNode<'a> {
    pub tag: u8,
    pub name: u32,
    pub qualifier: RawTree<'a>,
    pub underlying_type: RawTree<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseDefNode<'a> {
    pub pattern: RawTree<'a>,
    pub body: RawTree<'a>,
    pub guard: Option<RawTree<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindNode<'a> {
    pub name: u32,
    pub type_tree: RawTree<'a>,
    pub pattern: RawTree<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlternativeNode<'a> {
    pub alternatives: Vec<RawTree<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnapplyNode<'a> {
    pub function: RawTree<'a>,
    pub implicit_args: Vec<AstChildNode<'a>>,
    pub type_tree: RawTree<'a>,
    pub patterns: Vec<RawTree<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryTypeNode<'a> {
    pub tag: u8,
    pub left: RawTree<'a>,
    pub right: RawTree<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedTypeNode<'a> {
    pub tag: u8,
    pub tycon: RawTree<'a>,
    pub arguments: Vec<RawTree<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlexibleTypeNode<'a> {
    pub underlying_type: RawTree<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeBoundsNode<'a> {
    pub tag: u8,
    pub low_or_alias: RawTree<'a>,
    pub high: Option<RawTree<'a>>,
    pub variances: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnotatedNode<'a> {
    pub tag: u8,
    pub underlying: RawTree<'a>,
    pub annotation: RawTree<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamTypeNode {
    pub binder: u32,
    pub parameter_number: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeName {
    pub type_or_bounds: u32,
    pub name: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolyTypeNode<'a> {
    pub tag: u8,
    pub result_type: RawTree<'a>,
    pub type_names: Vec<TypeName>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodTypeNode<'a> {
    pub result_type: RawTree<'a>,
    pub type_names: Vec<TypeName>,
    pub modifiers: Vec<u8>,
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
            if !is_known_category5_tag(tag) {
                return Err(AstError::InvalidTag { tag, offset });
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

    pub fn decode_in_reference(&self) -> Result<InReferenceNode<'a>, AstError> {
        if !matches!(self.tag, TERMREFIN_TAG | TYPEREFIN_TAG) {
            return Err(AstError::UnexpectedTag {
                expected: TERMREFIN_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let name = reader.read_nat()?;
        let qualifier = RawTree::decode(&mut reader)?;
        let underlying_type = RawTree::decode(&mut reader)?;
        if !reader.is_at_end() {
            return Err(AstError::UnsupportedCategory {
                tag: self.tag,
                offset: self.offset,
            });
        }

        Ok(InReferenceNode {
            tag: self.tag,
            name,
            qualifier,
            underlying_type,
        })
    }

    pub fn decode_select_in(&self) -> Result<SelectInNode<'a>, AstError> {
        if self.tag != SELECTIN_TAG {
            return Err(AstError::UnexpectedTag {
                expected: SELECTIN_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let name = reader.read_nat()?;
        let qualifier = RawTree::decode(&mut reader)?;
        let underlying_type = RawTree::decode(&mut reader)?;
        if !reader.is_at_end() {
            return Err(AstError::UnsupportedCategory {
                tag: self.tag,
                offset: self.offset,
            });
        }

        Ok(SelectInNode {
            tag: self.tag,
            name,
            qualifier,
            underlying_type,
        })
    }

    pub fn decode_case_def(&self) -> Result<CaseDefNode<'a>, AstError> {
        if self.tag != CASEDEF_TAG {
            return Err(AstError::UnexpectedTag {
                expected: CASEDEF_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let pattern = RawTree::decode(&mut reader)?;
        let body = RawTree::decode(&mut reader)?;
        let guard = if reader.is_at_end() {
            None
        } else {
            Some(RawTree::decode(&mut reader)?)
        };
        if !reader.is_at_end() {
            return Err(AstError::UnsupportedCategory {
                tag: self.tag,
                offset: self.offset,
            });
        }

        Ok(CaseDefNode {
            pattern,
            body,
            guard,
        })
    }

    pub fn decode_bind(&self) -> Result<BindNode<'a>, AstError> {
        if self.tag != BIND_TAG {
            return Err(AstError::UnexpectedTag {
                expected: BIND_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let name = reader.read_nat()?;
        let type_tree = RawTree::decode(&mut reader)?;
        let pattern = RawTree::decode(&mut reader)?;
        if !reader.is_at_end() {
            return Err(AstError::UnsupportedCategory {
                tag: self.tag,
                offset: self.offset,
            });
        }

        Ok(BindNode {
            name,
            type_tree,
            pattern,
        })
    }

    pub fn decode_alternative(&self) -> Result<AlternativeNode<'a>, AstError> {
        if self.tag != ALTERNATIVE_TAG {
            return Err(AstError::UnexpectedTag {
                expected: ALTERNATIVE_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let mut alternatives = Vec::new();
        while !reader.is_at_end() {
            alternatives.push(RawTree::decode(&mut reader)?);
        }

        Ok(AlternativeNode { alternatives })
    }

    pub fn decode_unapply(&self) -> Result<UnapplyNode<'a>, AstError> {
        if self.tag != UNAPPLY_TAG {
            return Err(AstError::UnexpectedTag {
                expected: UNAPPLY_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let function = RawTree::decode(&mut reader)?;
        let mut implicit_args = Vec::new();
        while !reader.is_at_end() && reader.peek_u8()? == IMPLICITARG_TAG {
            implicit_args.push(RawTree::decode(&mut reader)?.decode_implicit_arg()?);
        }
        let type_tree = RawTree::decode(&mut reader)?;
        let mut patterns = Vec::new();
        while !reader.is_at_end() {
            patterns.push(RawTree::decode(&mut reader)?);
        }

        Ok(UnapplyNode {
            function,
            implicit_args,
            type_tree,
            patterns,
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

    pub fn decode_if(&self) -> Result<IfNode<'a>, AstError> {
        if self.tag != IF_TAG {
            return Err(AstError::UnexpectedTag {
                expected: IF_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let inline = if !reader.is_at_end() && reader.peek_u8()? == INLINE_TAG {
            reader.read_u8()?;
            true
        } else {
            false
        };
        let condition = RawTree::decode(&mut reader)?;
        let then_branch = RawTree::decode(&mut reader)?;
        let else_branch = RawTree::decode(&mut reader)?;
        if !reader.is_at_end() {
            return Err(AstError::UnsupportedCategory {
                tag: self.tag,
                offset: self.offset,
            });
        }

        Ok(IfNode {
            inline,
            condition,
            then_branch,
            else_branch,
        })
    }

    pub fn decode_lambda(&self) -> Result<LambdaNode<'a>, AstError> {
        if self.tag != LAMBDA_TAG {
            return Err(AstError::UnexpectedTag {
                expected: LAMBDA_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let method = RawTree::decode(&mut reader)?;
        let target_type = if reader.is_at_end() {
            None
        } else {
            Some(RawTree::decode(&mut reader)?)
        };
        if !reader.is_at_end() {
            return Err(AstError::UnsupportedCategory {
                tag: self.tag,
                offset: self.offset,
            });
        }

        Ok(LambdaNode {
            method,
            target_type,
        })
    }

    pub fn decode_super(&self) -> Result<SuperNode<'a>, AstError> {
        if self.tag != SUPER_TAG {
            return Err(AstError::UnexpectedTag {
                expected: SUPER_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let this_term = RawTree::decode(&mut reader)?;
        let mixin_type = if reader.is_at_end() {
            None
        } else {
            Some(RawTree::decode(&mut reader)?)
        };
        if !reader.is_at_end() {
            return Err(AstError::UnsupportedCategory {
                tag: self.tag,
                offset: self.offset,
            });
        }

        Ok(SuperNode {
            this_term,
            mixin_type,
        })
    }

    pub fn decode_repeated(&self) -> Result<RepeatedNode<'a>, AstError> {
        if self.tag != REPEATED_TAG {
            return Err(AstError::UnexpectedTag {
                expected: REPEATED_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let element_type = RawTree::decode(&mut reader)?;
        let mut elements = Vec::new();
        while !reader.is_at_end() {
            elements.push(RawTree::decode(&mut reader)?);
        }

        Ok(RepeatedNode {
            element_type,
            elements,
        })
    }

    pub fn decode_select_outer(&self) -> Result<SelectOuterNode<'a>, AstError> {
        if self.tag != SELECTOUTER_TAG {
            return Err(AstError::UnexpectedTag {
                expected: SELECTOUTER_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let levels = reader.read_nat()?;
        let qualifier = RawTree::decode(&mut reader)?;
        let underlying_type = RawTree::decode(&mut reader)?;
        if !reader.is_at_end() {
            return Err(AstError::UnsupportedCategory {
                tag: self.tag,
                offset: self.offset,
            });
        }

        Ok(SelectOuterNode {
            levels,
            qualifier,
            underlying_type,
        })
    }

    pub fn decode_and_type(&self) -> Result<BinaryTypeNode<'a>, AstError> {
        self.decode_binary_type(ANDTYPE_TAG)
    }

    pub fn decode_or_type(&self) -> Result<BinaryTypeNode<'a>, AstError> {
        self.decode_binary_type(ORTYPE_TAG)
    }

    pub fn decode_super_type(&self) -> Result<BinaryTypeNode<'a>, AstError> {
        self.decode_binary_type(SUPERTYPE_TAG)
    }

    pub fn decode_matchcase_type(&self) -> Result<BinaryTypeNode<'a>, AstError> {
        self.decode_binary_type(MATCHCASETYPE_TAG)
    }

    fn decode_binary_type(&self, expected: u8) -> Result<BinaryTypeNode<'a>, AstError> {
        if self.tag != expected {
            return Err(AstError::UnexpectedTag {
                expected,
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

        Ok(BinaryTypeNode {
            tag: self.tag,
            left,
            right,
        })
    }

    pub fn decode_applied_type(&self) -> Result<AppliedTypeNode<'a>, AstError> {
        if self.tag != APPLIEDTYPE_TAG && self.tag != APPLIEDTPT_TAG {
            return Err(AstError::UnexpectedTag {
                expected: APPLIEDTYPE_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let tycon = RawTree::decode(&mut reader)?;
        let mut arguments = Vec::new();
        while !reader.is_at_end() {
            arguments.push(RawTree::decode(&mut reader)?);
        }

        Ok(AppliedTypeNode {
            tag: self.tag,
            tycon,
            arguments,
        })
    }

    pub fn decode_flexible_type(&self) -> Result<FlexibleTypeNode<'a>, AstError> {
        if self.tag != FLEXIBLETYPE_TAG {
            return Err(AstError::UnexpectedTag {
                expected: FLEXIBLETYPE_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let underlying_type = RawTree::decode(&mut reader)?;
        if !reader.is_at_end() {
            return Err(AstError::UnsupportedCategory {
                tag: self.tag,
                offset: self.offset,
            });
        }

        Ok(FlexibleTypeNode { underlying_type })
    }

    pub fn decode_type_bounds(&self) -> Result<TypeBoundsNode<'a>, AstError> {
        if self.tag != TYPEBOUNDS_TAG && self.tag != TYPEBOUNDSTPT_TAG {
            return Err(AstError::UnexpectedTag {
                expected: TYPEBOUNDS_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let low_or_alias = RawTree::decode(&mut reader)?;
        let high = if reader.is_at_end() || matches!(reader.peek_u8()?, 28 | 29) {
            None
        } else {
            Some(RawTree::decode(&mut reader)?)
        };
        let mut variances = Vec::new();
        while !reader.is_at_end() {
            let variance = reader.read_u8()?;
            if !matches!(variance, 28 | 29) {
                return Err(AstError::UnexpectedTag {
                    expected: 28,
                    actual: variance,
                    offset: self.offset + reader.position() - 1,
                });
            }
            variances.push(variance);
        }

        Ok(TypeBoundsNode {
            tag: self.tag,
            low_or_alias,
            high,
            variances,
        })
    }

    pub fn decode_annotated(&self) -> Result<AnnotatedNode<'a>, AstError> {
        if self.tag != ANNOTATEDTYPE_TAG && self.tag != ANNOTATEDTPT_TAG {
            return Err(AstError::UnexpectedTag {
                expected: ANNOTATEDTYPE_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let underlying = RawTree::decode(&mut reader)?;
        let annotation = RawTree::decode(&mut reader)?;
        if !reader.is_at_end() {
            return Err(AstError::UnsupportedCategory {
                tag: self.tag,
                offset: self.offset,
            });
        }

        Ok(AnnotatedNode {
            tag: self.tag,
            underlying,
            annotation,
        })
    }

    pub fn decode_param_type(&self) -> Result<ParamTypeNode, AstError> {
        if self.tag != PARAMTYPE_TAG {
            return Err(AstError::UnexpectedTag {
                expected: PARAMTYPE_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let binder = reader.read_nat()?;
        let parameter_number = reader.read_nat()?;
        if !reader.is_at_end() {
            return Err(AstError::UnsupportedCategory {
                tag: self.tag,
                offset: self.offset,
            });
        }

        Ok(ParamTypeNode {
            binder,
            parameter_number,
        })
    }

    pub fn decode_poly_type(&self) -> Result<PolyTypeNode<'a>, AstError> {
        if self.tag != POLYTYPE_TAG && self.tag != TYPELAMBDATYPE_TAG {
            return Err(AstError::UnexpectedTag {
                expected: POLYTYPE_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let result_type = RawTree::decode(&mut reader)?;
        let mut type_names = Vec::new();
        while !reader.is_at_end() {
            type_names.push(TypeName {
                type_or_bounds: reader.read_nat()?,
                name: reader.read_nat()?,
            });
        }

        Ok(PolyTypeNode {
            tag: self.tag,
            result_type,
            type_names,
        })
    }

    pub fn decode_method_type(&self) -> Result<MethodTypeNode<'a>, AstError> {
        if self.tag != METHODTYPE_TAG {
            return Err(AstError::UnexpectedTag {
                expected: METHODTYPE_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let result_type = RawTree::decode(&mut reader)?;
        let mut type_names = Vec::new();
        while !reader.is_at_end() && !is_modifier_tag(reader.peek_u8()?) {
            type_names.push(TypeName {
                type_or_bounds: reader.read_nat()?,
                name: reader.read_nat()?,
            });
        }

        let mut modifiers = Vec::new();
        while !reader.is_at_end() {
            let offset = reader.position();
            let modifier = reader.read_u8()?;
            if !is_modifier_tag(modifier) {
                return Err(AstError::UnexpectedTag {
                    expected: 6,
                    actual: modifier,
                    offset: self.offset + offset,
                });
            }
            modifiers.push(modifier);
        }

        Ok(MethodTypeNode {
            result_type,
            type_names,
            modifiers,
        })
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

    pub fn decode_qual_this(&self) -> Result<AstChildNode<'a>, AstError> {
        self.decode_ast_child(QUALTHIS_TAG)
    }

    pub fn decode_class_const(&self) -> Result<AstChildNode<'a>, AstError> {
        self.decode_ast_child(CLASSCONST_TAG)
    }

    pub fn decode_by_name_type(&self) -> Result<AstChildNode<'a>, AstError> {
        self.decode_ast_child(BYNAMETYPE_TAG)
    }

    pub fn decode_by_name_tpt(&self) -> Result<AstChildNode<'a>, AstError> {
        self.decode_ast_child(BYNAMETPT_TAG)
    }

    pub fn decode_new(&self) -> Result<AstChildNode<'a>, AstError> {
        self.decode_ast_child(NEW_TAG)
    }

    pub fn decode_throw(&self) -> Result<AstChildNode<'a>, AstError> {
        self.decode_ast_child(THROW_TAG)
    }

    pub fn decode_implicit_arg(&self) -> Result<AstChildNode<'a>, AstError> {
        self.decode_ast_child(IMPLICITARG_TAG)
    }

    pub fn decode_private_qualified(&self) -> Result<AstChildNode<'a>, AstError> {
        self.decode_ast_child(PRIVATEQUALIFIED_TAG)
    }

    pub fn decode_protected_qualified(&self) -> Result<AstChildNode<'a>, AstError> {
        self.decode_ast_child(PROTECTEDQUALIFIED_TAG)
    }

    pub fn decode_rec_type(&self) -> Result<AstChildNode<'a>, AstError> {
        self.decode_ast_child(RECTYPE_TAG)
    }

    pub fn decode_singleton_tpt(&self) -> Result<AstChildNode<'a>, AstError> {
        self.decode_ast_child(SINGLETONTPT_TAG)
    }

    pub fn decode_bounded(&self) -> Result<AstChildNode<'a>, AstError> {
        self.decode_ast_child(BOUNDED_TAG)
    }

    pub fn decode_explicit_tpt(&self) -> Result<AstChildNode<'a>, AstError> {
        self.decode_ast_child(EXPLICITTPT_TAG)
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

    pub fn decode_named_arg(&self) -> Result<NamedArgNode<'a>, AstError> {
        match self {
            RawTree::NatAst {
                tag: NAMEDARG_TAG,
                value: name,
                child,
                ..
            } => Ok(NamedArgNode {
                name: *name,
                argument: (**child).clone(),
            }),
            tree => {
                let (actual, offset) = raw_tree_tag_offset(tree);
                Err(AstError::UnexpectedTag {
                    expected: NAMEDARG_TAG,
                    actual,
                    offset,
                })
            }
        }
    }

    pub fn decode_ident(&self) -> Result<IdentNode<'a>, AstError> {
        match self {
            RawTree::NatAst {
                tag: tag @ (IDENT_TAG | IDENTTPT_TAG),
                value: name,
                child,
                ..
            } => Ok(IdentNode {
                tag: *tag,
                name: *name,
                type_tree: (**child).clone(),
            }),
            tree => {
                let (actual, offset) = raw_tree_tag_offset(tree);
                Err(AstError::UnexpectedTag {
                    expected: IDENT_TAG,
                    actual,
                    offset,
                })
            }
        }
    }

    pub fn decode_select(&self) -> Result<SelectNode<'a>, AstError> {
        match self {
            RawTree::NatAst {
                tag: tag @ (SELECT_TAG | SELECTTPT_TAG),
                value: name,
                child,
                ..
            } => Ok(SelectNode {
                tag: *tag,
                name: *name,
                qualifier: (**child).clone(),
            }),
            tree => {
                let (actual, offset) = raw_tree_tag_offset(tree);
                Err(AstError::UnexpectedTag {
                    expected: SELECT_TAG,
                    actual,
                    offset,
                })
            }
        }
    }

    pub fn decode_reference(&self) -> Result<ReferenceNode<'a>, AstError> {
        match self {
            RawTree::NatAst {
                tag: tag @ (TERMREFSYMBOL_TAG | TERMREF_TAG | TYPEREFSYMBOL_TAG | TYPEREF_TAG),
                value: reference,
                child,
                ..
            } => Ok(ReferenceNode {
                tag: *tag,
                reference: *reference,
                qualifier: (**child).clone(),
            }),
            tree => {
                let (actual, offset) = raw_tree_tag_offset(tree);
                Err(AstError::UnexpectedTag {
                    expected: TERMREFSYMBOL_TAG,
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
        ALTERNATIVE_TAG, ANDTYPE_TAG, ANNOTATEDTPT_TAG, ANNOTATEDTYPE_TAG, APPLIEDTPT_TAG,
        APPLIEDTYPE_TAG, APPLY_TAG, ASSIGN_TAG, AstChildNode, AstError, BIND_TAG, BLOCK_TAG,
        BOUNDED_TAG, BYNAMETPT_TAG, BYNAMETYPE_TAG, CASEDEF_TAG, CLASSCONST_TAG, DEFDEF_TAG,
        DefDefBody, DefinitionBody, DefinitionNode, DefinitionTail, ELIDED_TAG, EXPLICITTPT_TAG,
        EXPORT_TAG, FLEXIBLETYPE_TAG, IDENT_TAG, IDENTTPT_TAG, IF_TAG, IMPLICITARG_TAG, IMPORT_TAG,
        IMPORTED_TAG, INLINE_TAG, ImportExportKind, ImportSelector, LAMBDA_TAG, MATCHCASETYPE_TAG,
        METHODTYPE_TAG, NAMEDARG_TAG, NEW_TAG, NodeCategory, ORTYPE_TAG, PACKAGE_TAG, PARAM_TAG,
        PARAMTYPE_TAG, POLYTYPE_TAG, PRIVATEQUALIFIED_TAG, PROTECTEDQUALIFIED_TAG, ParameterNode,
        QUALTHIS_TAG, RECTYPE_TAG, RENAMED_TAG, REPEATED_TAG, RETURN_TAG, RawNode, RawNodes,
        RawTree, SELECT_TAG, SELECTIN_TAG, SELECTOUTER_TAG, SELECTTPT_TAG, SELFDEF_TAG,
        SINGLETONTPT_TAG, SPLITCLAUSE_TAG, SUPER_TAG, SUPERTYPE_TAG, TEMPLATE_TAG, TERMREF_TAG,
        TERMREFIN_TAG, TERMREFPKG_TAG, TERMREFSYMBOL_TAG, THIS_TAG, THROW_TAG, TYPEAPPLY_TAG,
        TYPEBOUNDS_TAG, TYPEBOUNDSTPT_TAG, TYPED_TAG, TYPEDEF_TAG, TYPELAMBDATYPE_TAG,
        TYPEPARAM_TAG, TYPEREF_TAG, TYPEREFIN_TAG, TYPEREFSYMBOL_TAG, TypeApplyNode, TypedNode,
        UNAPPLY_TAG, VALDEF_TAG, WHILE_TAG,
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
    fn rejects_an_unassigned_category_five_top_level_tag() {
        let mut reader = Reader::new(&[135, 0x80]);

        assert_eq!(
            RawNodes::decode(&mut reader),
            Err(AstError::InvalidTag {
                tag: 135,
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
    fn decodes_a_named_argument_name_and_value() {
        let mut reader = Reader::new(&[NAMEDARG_TAG, 0x85, 2]);
        let tree = RawTree::decode(&mut reader).unwrap();
        let node = tree.decode_named_arg().unwrap();

        assert_eq!(node.name, 5);
        assert!(matches!(node.argument, RawTree::Leaf(_)));
        assert!(reader.is_at_end());
    }

    #[test]
    fn rejects_a_tree_with_the_wrong_named_argument_tag() {
        let mut reader = Reader::new(&[TERMREFPKG_TAG, 0x81]);
        let tree = RawTree::decode(&mut reader).unwrap();

        assert_eq!(
            tree.decode_named_arg(),
            Err(AstError::UnexpectedTag {
                expected: NAMEDARG_TAG,
                actual: TERMREFPKG_TAG,
                offset: 0,
            })
        );
    }

    #[test]
    fn decodes_ident_and_identtpt_with_the_shared_structure() {
        for tag in [IDENT_TAG, IDENTTPT_TAG] {
            let bytes = [tag, 0x85, 2];
            let mut reader = Reader::new(&bytes);
            let tree = RawTree::decode(&mut reader).unwrap();
            let node = tree.decode_ident().unwrap();

            assert_eq!(node.tag, tag);
            assert_eq!(node.name, 5);
            assert!(matches!(node.type_tree, RawTree::Leaf(_)));
            assert!(reader.is_at_end());
        }
    }

    #[test]
    fn rejects_a_tree_with_the_wrong_ident_tag() {
        let mut reader = Reader::new(&[NAMEDARG_TAG, 0x85, 2]);
        let tree = RawTree::decode(&mut reader).unwrap();

        assert_eq!(
            tree.decode_ident(),
            Err(AstError::UnexpectedTag {
                expected: IDENT_TAG,
                actual: NAMEDARG_TAG,
                offset: 0,
            })
        );
    }

    #[test]
    fn decodes_all_category_four_reference_nodes() {
        for tag in [
            TERMREFSYMBOL_TAG,
            TERMREF_TAG,
            TYPEREFSYMBOL_TAG,
            TYPEREF_TAG,
        ] {
            let bytes = [tag, 0x85, TERMREFPKG_TAG, 0x81];
            let mut reader = Reader::new(&bytes);
            let tree = RawTree::decode(&mut reader).unwrap();
            let node = tree.decode_reference().unwrap();

            assert_eq!(node.tag, tag);
            assert_eq!(node.reference, 5);
            assert!(matches!(node.qualifier, RawTree::Leaf(_)));
            assert!(reader.is_at_end());
        }
    }

    #[test]
    fn rejects_a_tree_with_the_wrong_reference_tag() {
        let mut reader = Reader::new(&[IDENT_TAG, 0x85, 2]);
        let tree = RawTree::decode(&mut reader).unwrap();

        assert_eq!(
            tree.decode_reference(),
            Err(AstError::UnexpectedTag {
                expected: TERMREFSYMBOL_TAG,
                actual: IDENT_TAG,
                offset: 0,
            })
        );
    }

    #[test]
    fn rejects_a_reference_without_a_qualifier() {
        let mut reader = Reader::new(&[TERMREF_TAG, 0x85]);

        assert!(RawTree::decode(&mut reader).is_err());
    }

    #[test]
    fn decodes_select_and_selecttpt_with_the_shared_structure() {
        for tag in [SELECT_TAG, SELECTTPT_TAG] {
            let bytes = [tag, 0x85, TERMREFPKG_TAG, 0x81];
            let mut reader = Reader::new(&bytes);
            let tree = RawTree::decode(&mut reader).unwrap();
            let node = tree.decode_select().unwrap();

            assert_eq!(node.tag, tag);
            assert_eq!(node.name, 5);
            assert!(matches!(node.qualifier, RawTree::Leaf(_)));
            assert!(reader.is_at_end());
        }
    }

    #[test]
    fn decodes_termrefin_and_typerefin_with_two_trees() {
        for tag in [TERMREFIN_TAG, TYPEREFIN_TAG] {
            let bytes = [tag, 0x85, 0x85, TERMREFPKG_TAG, 0x81, TERMREFPKG_TAG, 0x82];
            let mut reader = Reader::new(&bytes);
            let tree = RawTree::decode(&mut reader).unwrap();
            let RawTree::LengthNode(raw) = tree else {
                panic!("expected a length-delimited node");
            };
            let node = raw.decode_in_reference().unwrap();

            assert_eq!(node.tag, tag);
            assert_eq!(node.name, 5);
            assert!(matches!(node.qualifier, RawTree::Leaf(_)));
            assert!(matches!(node.underlying_type, RawTree::Leaf(_)));
            assert!(reader.is_at_end());
        }
    }

    #[test]
    fn decodes_selectin_with_a_name_qualifier_and_type() {
        let bytes = [SELECTIN_TAG, 0x85, 0x85, TERMREFPKG_TAG, 0x81, 70, 0x82];
        let mut reader = Reader::new(&bytes);
        let tree = RawTree::decode(&mut reader).unwrap();
        let RawTree::LengthNode(raw) = tree else {
            panic!("expected a length-delimited node");
        };
        let node = raw.decode_select_in().unwrap();

        assert_eq!(node.tag, SELECTIN_TAG);
        assert_eq!(node.name, 5);
        assert!(matches!(node.qualifier, RawTree::Leaf(_)));
        assert!(matches!(node.underlying_type, RawTree::Leaf(_)));
        assert!(reader.is_at_end());
    }

    #[test]
    fn rejects_an_in_reference_with_the_wrong_tag() {
        let raw = RawNode {
            tag: SELECTIN_TAG,
            offset: 0,
            payload: &[],
        };

        assert_eq!(
            raw.decode_in_reference(),
            Err(AstError::UnexpectedTag {
                expected: TERMREFIN_TAG,
                actual: SELECTIN_TAG,
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
    fn decodes_if_without_the_inline_modifier() {
        let bytes = [IF_TAG, 0x83, 2, 3, 4];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let node = nodes.get(0).unwrap().decode_if().unwrap();

        assert!(!node.inline);
        assert!(matches!(node.condition, RawTree::Leaf(_)));
        assert!(matches!(node.then_branch, RawTree::Leaf(_)));
        assert!(matches!(node.else_branch, RawTree::Leaf(_)));
    }

    #[test]
    fn decodes_if_with_the_inline_modifier() {
        let bytes = [IF_TAG, 0x84, INLINE_TAG, 2, 3, 4];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();

        assert!(nodes.get(0).unwrap().decode_if().unwrap().inline);
    }

    #[test]
    fn rejects_if_with_a_missing_branch() {
        let node = RawNode {
            tag: IF_TAG,
            offset: 0,
            payload: &[2, 3],
        };

        assert!(node.decode_if().is_err());
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
    fn decodes_lambda_without_a_target_type() {
        let bytes = [LAMBDA_TAG, 0x81, 2];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let node = nodes.get(0).unwrap().decode_lambda().unwrap();

        assert!(matches!(node.method, RawTree::Leaf(_)));
        assert!(node.target_type.is_none());
    }

    #[test]
    fn decodes_lambda_with_a_target_type() {
        let bytes = [LAMBDA_TAG, 0x82, 2, 3];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let node = nodes.get(0).unwrap().decode_lambda().unwrap();

        assert!(matches!(node.method, RawTree::Leaf(_)));
        assert!(matches!(node.target_type, Some(RawTree::Leaf(_))));
    }

    #[test]
    fn rejects_lambda_with_more_than_one_target_type() {
        let node = RawNode {
            tag: LAMBDA_TAG,
            offset: 0,
            payload: &[2, 3, 4],
        };

        assert!(node.decode_lambda().is_err());
    }

    #[test]
    fn decodes_super_without_a_mixin_type() {
        let bytes = [SUPER_TAG, 0x81, 2];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let node = nodes.get(0).unwrap().decode_super().unwrap();

        assert!(matches!(node.this_term, RawTree::Leaf(_)));
        assert!(node.mixin_type.is_none());
    }

    #[test]
    fn decodes_super_with_a_mixin_type() {
        let bytes = [SUPER_TAG, 0x82, 2, 3];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let node = nodes.get(0).unwrap().decode_super().unwrap();

        assert!(matches!(node.this_term, RawTree::Leaf(_)));
        assert!(matches!(node.mixin_type, Some(RawTree::Leaf(_))));
    }

    #[test]
    fn rejects_super_with_more_than_one_mixin_type() {
        let node = RawNode {
            tag: SUPER_TAG,
            offset: 0,
            payload: &[2, 3, 4],
        };

        assert!(node.decode_super().is_err());
    }

    #[test]
    fn decodes_repeated_without_elements() {
        let bytes = [REPEATED_TAG, 0x81, 2];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let node = nodes.get(0).unwrap().decode_repeated().unwrap();

        assert!(matches!(node.element_type, RawTree::Leaf(_)));
        assert!(node.elements.is_empty());
    }

    #[test]
    fn decodes_repeated_with_elements() {
        let bytes = [REPEATED_TAG, 0x83, 2, 3, 4];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let node = nodes.get(0).unwrap().decode_repeated().unwrap();

        assert!(matches!(node.element_type, RawTree::Leaf(_)));
        assert_eq!(node.elements.len(), 2);
    }

    #[test]
    fn decodes_select_outer_levels_and_trees() {
        let bytes = [SELECTOUTER_TAG, 0x83, 0x85, 2, 3];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let node = nodes.get(0).unwrap().decode_select_outer().unwrap();

        assert_eq!(node.levels, 5);
        assert!(matches!(node.qualifier, RawTree::Leaf(_)));
        assert!(matches!(node.underlying_type, RawTree::Leaf(_)));
    }

    #[test]
    fn rejects_select_outer_with_an_extra_tree() {
        let node = RawNode {
            tag: SELECTOUTER_TAG,
            offset: 0,
            payload: &[0, 2, 3, 4],
        };

        assert!(node.decode_select_outer().is_err());
    }

    #[test]
    fn decodes_and_and_or_types_with_the_shared_structure() {
        for tag in [ANDTYPE_TAG, ORTYPE_TAG, SUPERTYPE_TAG, MATCHCASETYPE_TAG] {
            let bytes = [tag, 0x82, 2, 5];
            let mut reader = Reader::new(&bytes);
            let nodes = RawNodes::decode(&mut reader).unwrap();
            let node = match tag {
                ANDTYPE_TAG => nodes.get(0).unwrap().decode_and_type().unwrap(),
                ORTYPE_TAG => nodes.get(0).unwrap().decode_or_type().unwrap(),
                SUPERTYPE_TAG => nodes.get(0).unwrap().decode_super_type().unwrap(),
                MATCHCASETYPE_TAG => nodes.get(0).unwrap().decode_matchcase_type().unwrap(),
                _ => unreachable!("all binary type tags are covered above"),
            };

            assert_eq!(node.tag, tag);
            assert!(matches!(node.left, RawTree::Leaf(_)));
            assert!(matches!(node.right, RawTree::Leaf(_)));
        }
    }

    #[test]
    fn rejects_a_binary_type_with_a_missing_operand() {
        let node = RawNode {
            tag: ANDTYPE_TAG,
            offset: 0,
            payload: &[2],
        };

        assert!(node.decode_and_type().is_err());
    }

    #[test]
    fn rejects_a_binary_type_with_an_extra_tree() {
        let node = RawNode {
            tag: ORTYPE_TAG,
            offset: 0,
            payload: &[2, 5, 3],
        };

        assert!(node.decode_or_type().is_err());
    }

    #[test]
    fn decodes_applied_types_with_zero_or_more_arguments() {
        for (tag, payload, expected_arguments) in [
            (APPLIEDTYPE_TAG, vec![2], 0),
            (APPLIEDTPT_TAG, vec![2, 3, 4], 2),
        ] {
            let mut bytes = vec![tag, 0x80 | payload.len() as u8];
            bytes.extend(payload);
            let mut reader = Reader::new(&bytes);
            let nodes = RawNodes::decode(&mut reader).unwrap();
            let node = nodes.get(0).unwrap().decode_applied_type().unwrap();

            assert_eq!(node.tag, tag);
            assert!(matches!(node.tycon, RawTree::Leaf(_)));
            assert_eq!(node.arguments.len(), expected_arguments);
        }
    }

    #[test]
    fn rejects_a_tree_with_an_invalid_applied_type_tag() {
        let node = RawNode {
            tag: ORTYPE_TAG,
            offset: 0,
            payload: &[2, 3],
        };

        assert!(node.decode_applied_type().is_err());
    }

    #[test]
    fn decodes_a_flexible_type_with_one_underlying_tree() {
        let bytes = [FLEXIBLETYPE_TAG, 0x81, 2];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let node = nodes.get(0).unwrap().decode_flexible_type().unwrap();

        assert!(matches!(node.underlying_type, RawTree::Leaf(_)));
    }

    #[test]
    fn rejects_a_flexible_type_with_an_extra_tree() {
        let node = RawNode {
            tag: FLEXIBLETYPE_TAG,
            offset: 0,
            payload: &[2, 3],
        };

        assert!(node.decode_flexible_type().is_err());
    }

    #[test]
    fn decodes_type_bounds_with_optional_high_type_and_variances() {
        for (tag, payload, has_high, expected_variances) in [
            (TYPEBOUNDS_TAG, vec![2], false, vec![]),
            (TYPEBOUNDSTPT_TAG, vec![2, 28, 29], false, vec![28, 29]),
            (TYPEBOUNDS_TAG, vec![2, 5, 28], true, vec![28]),
        ] {
            let mut bytes = vec![tag, 0x80 | payload.len() as u8];
            bytes.extend(payload);
            let mut reader = Reader::new(&bytes);
            let nodes = RawNodes::decode(&mut reader).unwrap();
            let node = nodes.get(0).unwrap().decode_type_bounds().unwrap();

            assert_eq!(node.tag, tag);
            assert!(matches!(node.low_or_alias, RawTree::Leaf(_)));
            assert_eq!(node.high.is_some(), has_high);
            assert_eq!(node.variances, expected_variances);
        }
    }

    #[test]
    fn rejects_type_bounds_with_an_invalid_variance_tag() {
        let node = RawNode {
            tag: TYPEBOUNDS_TAG,
            offset: 0,
            payload: &[2, 17],
        };

        assert!(node.decode_type_bounds().is_err());
    }

    #[test]
    fn decodes_annotated_types_with_the_shared_structure() {
        for tag in [ANNOTATEDTYPE_TAG, ANNOTATEDTPT_TAG] {
            let bytes = [tag, 0x82, 2, 5];
            let mut reader = Reader::new(&bytes);
            let nodes = RawNodes::decode(&mut reader).unwrap();
            let node = nodes.get(0).unwrap().decode_annotated().unwrap();

            assert_eq!(node.tag, tag);
            assert!(matches!(node.underlying, RawTree::Leaf(_)));
            assert!(matches!(node.annotation, RawTree::Leaf(_)));
        }
    }

    #[test]
    fn rejects_an_annotated_type_with_a_missing_annotation() {
        let node = RawNode {
            tag: ANNOTATEDTYPE_TAG,
            offset: 0,
            payload: &[2],
        };

        assert!(node.decode_annotated().is_err());
    }

    #[test]
    fn decodes_param_type_binder_and_parameter_number() {
        let bytes = [PARAMTYPE_TAG, 0x82, 0x85, 0x83];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let node = nodes.get(0).unwrap().decode_param_type().unwrap();

        assert_eq!(node.binder, 5);
        assert_eq!(node.parameter_number, 3);
    }

    #[test]
    fn rejects_param_type_with_an_extra_value() {
        let node = RawNode {
            tag: PARAMTYPE_TAG,
            offset: 0,
            payload: &[5, 3, 1],
        };

        assert!(node.decode_param_type().is_err());
    }

    #[test]
    fn decodes_poly_and_type_lambda_types_with_type_names() {
        for tag in [POLYTYPE_TAG, TYPELAMBDATYPE_TAG] {
            let bytes = [tag, 0x85, 2, 0x85, 0x86, 0x87, 0x88];
            let mut reader = Reader::new(&bytes);
            let nodes = RawNodes::decode(&mut reader).unwrap();
            let node = nodes.get(0).unwrap().decode_poly_type().unwrap();

            assert_eq!(node.tag, tag);
            assert!(matches!(node.result_type, RawTree::Leaf(_)));
            assert_eq!(
                node.type_names,
                vec![
                    super::TypeName {
                        type_or_bounds: 5,
                        name: 6,
                    },
                    super::TypeName {
                        type_or_bounds: 7,
                        name: 8,
                    },
                ]
            );
        }
    }

    #[test]
    fn decodes_a_poly_type_without_type_names() {
        let node = RawNode {
            tag: POLYTYPE_TAG,
            offset: 0,
            payload: &[2],
        };

        assert!(node.decode_poly_type().unwrap().type_names.is_empty());
    }

    #[test]
    fn rejects_a_poly_type_with_an_incomplete_type_name() {
        let node = RawNode {
            tag: TYPELAMBDATYPE_TAG,
            offset: 0,
            payload: &[2, 5],
        };

        assert!(node.decode_poly_type().is_err());
    }

    #[test]
    fn decodes_a_method_type_with_type_names_and_modifiers() {
        let bytes = [METHODTYPE_TAG, 0x87, 2, 0x85, 0x86, 0x87, 0x88, 17, 37];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let node = nodes.get(0).unwrap().decode_method_type().unwrap();

        assert!(matches!(node.result_type, RawTree::Leaf(_)));
        assert_eq!(node.type_names.len(), 2);
        assert_eq!(node.modifiers, vec![17, 37]);
    }

    #[test]
    fn decodes_a_method_type_without_type_names_or_modifiers() {
        let node = RawNode {
            tag: METHODTYPE_TAG,
            offset: 0,
            payload: &[2],
        };

        let node = node.decode_method_type().unwrap();
        assert!(node.type_names.is_empty());
        assert!(node.modifiers.is_empty());
    }

    #[test]
    fn rejects_a_method_type_with_an_invalid_modifier() {
        let node = RawNode {
            tag: METHODTYPE_TAG,
            offset: 0,
            payload: &[2, 50],
        };

        assert!(node.decode_method_type().is_err());
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
    fn decodes_case_definition_with_an_optional_guard() {
        let bytes = [
            CASEDEF_TAG,
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
        let case_def = nodes.get(0).unwrap().decode_case_def().unwrap();

        assert!(matches!(case_def.pattern, RawTree::Leaf(_)));
        assert!(matches!(case_def.body, RawTree::Leaf(_)));
        assert!(matches!(case_def.guard, Some(RawTree::Leaf(_))));
    }

    #[test]
    fn decodes_bind_and_alternative_pattern_nodes() {
        let bytes = [
            BIND_TAG,
            0x85,
            0x85,
            TERMREFPKG_TAG,
            0x81,
            TERMREFPKG_TAG,
            0x82,
            ALTERNATIVE_TAG,
            0x84,
            TERMREFPKG_TAG,
            0x83,
            TERMREFPKG_TAG,
            0x84,
        ];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let bind = nodes.get(0).unwrap().decode_bind().unwrap();
        let alternative = nodes.get(1).unwrap().decode_alternative().unwrap();

        assert_eq!(bind.name, 5);
        assert!(matches!(bind.type_tree, RawTree::Leaf(_)));
        assert!(matches!(bind.pattern, RawTree::Leaf(_)));
        assert_eq!(alternative.alternatives.len(), 2);
    }

    #[test]
    fn decodes_unapply_with_implicit_arguments_type_and_patterns() {
        let bytes = [
            UNAPPLY_TAG,
            0x89,
            TERMREFPKG_TAG,
            0x81,
            IMPLICITARG_TAG,
            TERMREFPKG_TAG,
            0x82,
            TERMREFPKG_TAG,
            0x83,
            TERMREFPKG_TAG,
            0x84,
        ];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let unapply = nodes.get(0).unwrap().decode_unapply().unwrap();

        assert!(matches!(unapply.function, RawTree::Leaf(_)));
        assert_eq!(unapply.implicit_args.len(), 1);
        assert_eq!(unapply.implicit_args[0].tag, IMPLICITARG_TAG);
        assert!(matches!(unapply.type_tree, RawTree::Leaf(_)));
        assert_eq!(unapply.patterns.len(), 1);
    }

    #[test]
    fn rejects_an_unapply_without_a_type_tree() {
        let node = RawNode {
            tag: UNAPPLY_TAG,
            offset: 0,
            payload: &[TERMREFPKG_TAG, 0x81],
        };

        assert!(node.decode_unapply().is_err());
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

    #[test]
    fn decodes_the_remaining_category_three_ast_children() {
        let mut reader = Reader::new(&[
            QUALTHIS_TAG,
            TERMREFPKG_TAG,
            0x81,
            CLASSCONST_TAG,
            TERMREFPKG_TAG,
            0x82,
            BYNAMETYPE_TAG,
            TERMREFPKG_TAG,
            0x83,
            BYNAMETPT_TAG,
            TERMREFPKG_TAG,
            0x84,
            IMPLICITARG_TAG,
            TERMREFPKG_TAG,
            0x85,
            PRIVATEQUALIFIED_TAG,
            TERMREFPKG_TAG,
            0x86,
            PROTECTEDQUALIFIED_TAG,
            TERMREFPKG_TAG,
            0x87,
            RECTYPE_TAG,
            TERMREFPKG_TAG,
            0x88,
            SINGLETONTPT_TAG,
            TERMREFPKG_TAG,
            0x89,
            BOUNDED_TAG,
            TERMREFPKG_TAG,
            0x8a,
            EXPLICITTPT_TAG,
            TERMREFPKG_TAG,
            0x8b,
        ]);

        let qual_this = RawTree::decode(&mut reader)
            .unwrap()
            .decode_qual_this()
            .unwrap();
        let class_const = RawTree::decode(&mut reader)
            .unwrap()
            .decode_class_const()
            .unwrap();
        let by_name_type = RawTree::decode(&mut reader)
            .unwrap()
            .decode_by_name_type()
            .unwrap();
        let by_name_tpt = RawTree::decode(&mut reader)
            .unwrap()
            .decode_by_name_tpt()
            .unwrap();
        let implicit_arg = RawTree::decode(&mut reader)
            .unwrap()
            .decode_implicit_arg()
            .unwrap();
        let private_qualified = RawTree::decode(&mut reader)
            .unwrap()
            .decode_private_qualified()
            .unwrap();
        let protected_qualified = RawTree::decode(&mut reader)
            .unwrap()
            .decode_protected_qualified()
            .unwrap();
        let rec_type = RawTree::decode(&mut reader)
            .unwrap()
            .decode_rec_type()
            .unwrap();
        let singleton_tpt = RawTree::decode(&mut reader)
            .unwrap()
            .decode_singleton_tpt()
            .unwrap();
        let bounded = RawTree::decode(&mut reader)
            .unwrap()
            .decode_bounded()
            .unwrap();
        let explicit_tpt = RawTree::decode(&mut reader)
            .unwrap()
            .decode_explicit_tpt()
            .unwrap();

        assert_eq!(qual_this.tag, QUALTHIS_TAG);
        assert_eq!(class_const.tag, CLASSCONST_TAG);
        assert_eq!(by_name_type.tag, BYNAMETYPE_TAG);
        assert_eq!(by_name_tpt.tag, BYNAMETPT_TAG);
        assert_eq!(implicit_arg.tag, IMPLICITARG_TAG);
        assert_eq!(private_qualified.tag, PRIVATEQUALIFIED_TAG);
        assert_eq!(protected_qualified.tag, PROTECTEDQUALIFIED_TAG);
        assert_eq!(rec_type.tag, RECTYPE_TAG);
        assert_eq!(singleton_tpt.tag, SINGLETONTPT_TAG);
        assert_eq!(bounded.tag, BOUNDED_TAG);
        assert_eq!(explicit_tpt.tag, EXPLICITTPT_TAG);
        assert!(reader.is_at_end());
    }
}
