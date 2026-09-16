use crate::name_table::NameRef;
use crate::reader::{ReadError, Reader};
use crate::term::{
    AstRef, AstRefKind, AstTreeNode, ConstantValue, RawTree, SimpleTerm, TermEncodeError, TermError,
};
use crate::writer::{WriteError, Writer};
use std::cell::{Cell, RefCell};
use std::fmt;

pub const DEFAULT_MAX_AST_INDEX_DEPTH: usize = 1024;

pub const TERMREFPKG_TAG: u8 = 64;
pub const SHAREDTERM_TAG: u8 = 60;
pub const SHAREDTYPE_TAG: u8 = 61;
pub const TERMREFDIRECT_TAG: u8 = 62;
pub const TYPEREFDIRECT_TAG: u8 = 63;
pub const RECTHIS_TAG: u8 = 66;
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
pub const HOLE_TAG: u8 = 255;
pub const ANNOTATEDTYPE_TAG: u8 = 153;
pub const ANNOTATEDTPT_TAG: u8 = 154;
pub const ANNOTATION_TAG: u8 = 173;
pub const PARAMTYPE_TAG: u8 = 172;
pub const FLEXIBLETYPE_TAG: u8 = 193;
pub const TYPEPARAM_TAG: u8 = 133;
pub const PARAM_TAG: u8 = 134;
pub const BIND_TAG: u8 = 150;
pub const ALTERNATIVE_TAG: u8 = 151;
pub const UNAPPLY_TAG: u8 = 152;
pub const CASEDEF_TAG: u8 = 155;
pub const REFINEDTYPE_TAG: u8 = 159;
pub const REFINEDTPT_TAG: u8 = 160;
pub const LAMBDATPT_TAG: u8 = 171;
pub const TERMREFIN_TAG: u8 = 174;
pub const TYPEREFIN_TAG: u8 = 175;
pub const SELECTIN_TAG: u8 = 176;
pub const QUOTE_TAG: u8 = 178;
pub const SPLICE_TAG: u8 = 179;
pub const APPLYSIGPOLY_TAG: u8 = 181;
pub const QUOTEPATTERN_TAG: u8 = 182;
pub const SPLICEPATTERN_TAG: u8 = 183;
pub const MATCHTYPE_TAG: u8 = 190;
pub const MATCHTPT_TAG: u8 = 191;
pub const APPLY_TAG: u8 = 136;
pub const TYPEAPPLY_TAG: u8 = 137;
pub const TYPED_TAG: u8 = 138;
pub const ASSIGN_TAG: u8 = 139;
pub const BLOCK_TAG: u8 = 140;
pub const IF_TAG: u8 = 141;
pub const LAMBDA_TAG: u8 = 142;
pub const MATCH_TAG: u8 = 143;
pub const RETURN_TAG: u8 = 144;
pub const WHILE_TAG: u8 = 145;
pub const TRY_TAG: u8 = 146;
pub const INLINED_TAG: u8 = 147;
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
pub const SUBMATCH_TAG: u8 = 48;
pub const PRIVATE_TAG: u8 = 6;
pub const PROTECTED_TAG: u8 = 8;
pub const ABSTRACT_TAG: u8 = 9;
pub const FINAL_TAG: u8 = 10;
pub const SEALED_TAG: u8 = 11;
pub const CASE_TAG: u8 = 12;
pub const IMPLICIT_TAG: u8 = 13;
pub const LAZY_TAG: u8 = 14;
pub const OVERRIDE_TAG: u8 = 15;
pub const INLINEPROXY_TAG: u8 = 16;
pub const INLINE_TAG: u8 = 17;
pub const STATIC_TAG: u8 = 18;
pub const OBJECT_TAG: u8 = 19;
pub const TRAIT_TAG: u8 = 20;
pub const ENUM_TAG: u8 = 21;
pub const LOCAL_TAG: u8 = 22;
pub const SYNTHETIC_TAG: u8 = 23;
pub const ARTIFACT_TAG: u8 = 24;
pub const MUTABLE_TAG: u8 = 25;
pub const FIELDACCESSOR_TAG: u8 = 26;
pub const CASEACCESSOR_TAG: u8 = 27;
pub const COVARIANT_TAG: u8 = 28;
pub const CONTRAVARIANT_TAG: u8 = 29;
pub const HASDEFAULT_TAG: u8 = 31;
pub const STABLE_TAG: u8 = 32;
pub const MACRO_TAG: u8 = 33;
pub const ERASED_TAG: u8 = 34;
pub const OPAQUE_TAG: u8 = 35;
pub const EXTENSION_TAG: u8 = 36;
pub const GIVEN_TAG: u8 = 37;
pub const PARAMSETTER_TAG: u8 = 38;
pub const EXPORTED_TAG: u8 = 39;
pub const OPEN_TAG: u8 = 40;
pub const PARAMALIAS_TAG: u8 = 41;
pub const TRANSPARENT_TAG: u8 = 42;
pub const INFIX_TAG: u8 = 43;
pub const INVISIBLE_TAG: u8 = 44;
pub const TRACKED_TAG: u8 = 47;
pub const INTO_TAG: u8 = 49;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AstReference {
    /// Address of the top-level AST node that owns this reference.
    pub owner_address: u32,
    pub reference: AstRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameReference {
    /// Address of the top-level AST node that owns this reference.
    pub owner_address: u32,
    pub reference: NameRef,
}

/// A structural parent-to-child edge in the global AST index.
///
/// The edge is derived from the enclosing TASTy tree grammar. Its `child`
/// order is the order in which child trees occur on the wire; it is not
/// necessarily sorted by absolute address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AstTreeEdge {
    pub parent: AstTreeNode,
    pub child: AstTreeNode,
}

impl<'a> RawNode<'a> {
    /// Construct a category-five AST node ready for encoding.
    ///
    /// Newly constructed nodes start at offset zero; [`RawNodes::encode`] and
    /// [`RawNodes::encode_with_addresses`] derive their emitted positions from
    /// the output stream.
    pub fn new(tag: u8, payload: &'a [u8]) -> Result<Self, AstError> {
        let Some(category) = NodeCategory::from_tag(tag) else {
            return Err(AstError::InvalidTag { tag, offset: 0 });
        };
        if category != NodeCategory::Category5 {
            return Err(AstError::UnsupportedCategory { tag, offset: 0 });
        }
        Ok(Self {
            tag,
            offset: 0,
            payload,
        })
    }

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
pub struct AstAddressIndex<'a> {
    nodes: Vec<RawNode<'a>>,
    all_nodes: Vec<AstTreeNode>,
    edges: Vec<AstTreeEdge>,
    edges_by_child: Vec<AstTreeEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedAstNodes {
    bytes: Vec<u8>,
    addresses: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructuredNode<'a> {
    Package(PackageNode<'a>),
    ValDef(DefinitionBody<'a>),
    DefDef(DefDefBody<'a>),
    TypeDef(DefinitionBody<'a>),
    ImportExport(ImportExportNode<'a>),
    Parameter(ParameterNode<'a>),
    Apply(ApplyNode<'a>),
    TypeApply(TypeApplyNode<'a>),
    Typed(TypedNode<'a>),
    Assign(AssignNode<'a>),
    Block(BlockNode<'a>),
    If(IfNode<'a>),
    Lambda(LambdaNode<'a>),
    Match(MatchNode<'a>),
    Return(ReturnNode<'a>),
    While(WhileNode<'a>),
    Try(TryNode<'a>),
    Inlined(InlinedNode<'a>),
    SelectOuter(SelectOuterNode<'a>),
    Repeated(RepeatedNode<'a>),
    Bind(BindNode<'a>),
    Alternative(AlternativeNode<'a>),
    Unapply(UnapplyNode<'a>),
    Annotated(AnnotatedNode<'a>),
    Annotation(AnnotationNode<'a>),
    CaseDef(CaseDefNode<'a>),
    Template(TemplateStructure<'a>),
    Super(SuperNode<'a>),
    BinaryType(BinaryTypeNode<'a>),
    RefinedType(RefinedTypeNode<'a>),
    RefinedTpt(RefinedTptNode<'a>),
    AppliedType(AppliedTypeNode<'a>),
    TypeBounds(TypeBoundsNode<'a>),
    FlexibleType(FlexibleTypeNode<'a>),
    LambdaTpt(LambdaTptNode<'a>),
    PolyType(PolyTypeNode<'a>),
    ParamType(ParamTypeNode),
    MethodType(MethodTypeNode<'a>),
    ApplySigPoly(ApplySigPolyNode<'a>),
    Quote(QuoteNode<'a>),
    QuotePattern(QuotePatternNode<'a>),
    SplicePattern(SplicePatternNode<'a>),
    MatchType(MatchTypeNode<'a>),
    MatchTpt(MatchTptNode<'a>),
    Hole(HoleNode<'a>),
    InReference(InReferenceNode<'a>),
    SelectIn(SelectInNode<'a>),
    Raw(RawNode<'a>),
}

/// Semantic dispatch for one complete raw tree.
///
/// Unlike [`StructuredNode`], which represents bounded category-5 AST
/// payloads, this type also covers category-1 through category-4 trees. The
/// original `RawTree` remains available for lossless encoding and for fields
/// whose grammar depends on an enclosing context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructuredTree<'a> {
    Constant(ConstantValue),
    Leaf(SimpleTerm),
    AstChild(AstChildNode<'a>),
    ClassConstant(ClassConstNode<'a>),
    Ident(IdentNode<'a>),
    Select(SelectNode<'a>),
    Reference(ReferenceNode<'a>),
    SelfDef(SelfDefNode<'a>),
    NamedArg(NamedArgNode<'a>),
    Length(StructuredNode<'a>),
}

impl<'a> StructuredTree<'a> {
    /// Encodes a semantically decoded tree back to its TASTy representation.
    ///
    /// The leaf and wrapper variants retain the wire tag in their typed
    /// payloads. Constants use their canonical TASTy tag, while bounded
    /// category-five trees delegate to [`StructuredNode::encode`].
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        match self {
            Self::Constant(value) => value.encode(writer),
            Self::Leaf(term) => term.encode(writer),
            Self::AstChild(node) => node.encode(writer),
            Self::ClassConstant(node) => node.encode(writer),
            Self::Ident(node) => node.encode(writer),
            Self::Select(node) => node.encode(writer),
            Self::Reference(node) => node.encode(writer),
            Self::SelfDef(node) => node.encode(writer),
            Self::NamedArg(node) => node.encode(writer),
            Self::Length(node) => node.encode(writer),
        }
    }
}

impl<'a> StructuredNode<'a> {
    /// Encodes a typed category-5 node back to its TASTy representation.
    ///
    /// Definition names are retained by `DefinitionBody` and `DefDefBody`,
    /// so nodes produced by `RawNode::decode_structured()` can be round-tripped
    /// without supplying external name metadata.
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        match self {
            Self::Package(node) => node.encode(writer),
            Self::ValDef(body) | Self::TypeDef(body) => body.encode_self(writer),
            Self::DefDef(body) => body.encode_self(writer),
            Self::ImportExport(node) => node.encode(writer),
            Self::Parameter(node) => node.encode(writer),
            Self::Apply(node) => node.encode(writer),
            Self::TypeApply(node) => node.encode(writer),
            Self::Typed(node) => node.encode(writer),
            Self::Assign(node) => node.encode(writer),
            Self::Block(node) => node.encode(writer),
            Self::If(node) => node.encode(writer),
            Self::Lambda(node) => node.encode(writer),
            Self::Match(node) => node.encode(writer),
            Self::Return(node) => node.encode(writer),
            Self::While(node) => node.encode(writer),
            Self::Try(node) => node.encode(writer),
            Self::Inlined(node) => node.encode(writer),
            Self::SelectOuter(node) => node.encode(writer),
            Self::Repeated(node) => node.encode(writer),
            Self::Bind(node) => node.encode(writer),
            Self::Alternative(node) => node.encode(writer),
            Self::Unapply(node) => node.encode(writer),
            Self::Annotated(node) => node.encode(writer),
            Self::Annotation(node) => node.encode(writer),
            Self::CaseDef(node) => node.encode(writer),
            Self::Template(node) => node.encode(writer),
            Self::Super(node) => node.encode(writer),
            Self::BinaryType(node) => node.encode(writer),
            Self::RefinedType(node) => node.encode(writer),
            Self::RefinedTpt(node) => node.encode(writer),
            Self::AppliedType(node) => node.encode(writer),
            Self::TypeBounds(node) => node.encode(writer),
            Self::FlexibleType(node) => node.encode(writer),
            Self::LambdaTpt(node) => node.encode(writer),
            Self::PolyType(node) => node.encode(writer),
            Self::ParamType(node) => node.encode(writer),
            Self::MethodType(node) => node.encode(writer),
            Self::ApplySigPoly(node) => node.encode(writer),
            Self::Quote(node) => node.encode(writer),
            Self::QuotePattern(node) => node.encode(writer),
            Self::SplicePattern(node) => node.encode(writer),
            Self::MatchType(node) => node.encode(writer),
            Self::MatchTpt(node) => node.encode(writer),
            Self::Hole(node) => node.encode(writer),
            Self::InReference(node) => node.encode(writer),
            Self::SelectIn(node) => node.encode(writer),
            Self::Raw(node) => node.encode(writer).map_err(TermEncodeError::from),
        }
    }
}

impl EncodedAstNodes {
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.bytes
    }

    pub fn addresses(&self) -> &[u32] {
        &self.addresses
    }

    pub fn address(&self, index: usize) -> Option<u32> {
        self.addresses.get(index).copied()
    }
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
        name: u32,
        type_tree: RawTree<'a>,
        rhs: Option<RawTree<'a>>,
        tail: Vec<DefinitionTail<'a>>,
    },
    TypeDef {
        name: u32,
        type_or_template: RawTree<'a>,
        tail: Vec<DefinitionTail<'a>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefDefHeaderItem<'a> {
    Parameter(ParameterNode<'a>),
    Clause(u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefDefBody<'a> {
    pub name: u32,
    pub parameters: Vec<ParameterNode<'a>>,
    pub clauses: Vec<u8>,
    /// Original wire order of parameters and clause markers.
    ///
    /// `parameters` and `clauses` remain available as convenient grouped
    /// views. This sequence is used by the encoder when the body came from a
    /// decoder, because TASTy permits clause markers between parameter nodes.
    pub header_items: Vec<DefDefHeaderItem<'a>>,
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
    pub target: AstRef,
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
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassConstNode<'a> {
    pub type_tree: RawTree<'a>,
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
    pub payload: &'a [u8],
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindBody<'a> {
    Pattern(RawTree<'a>),
    Type { modifiers: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindNode<'a> {
    pub name: u32,
    pub type_tree: RawTree<'a>,
    pub body: BindBody<'a>,
    /// Bytes after a pattern body whose meaning depends on the enclosing
    /// type or pattern context.
    pub remainder: &'a [u8],
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
pub struct RefinedTypeNode<'a> {
    pub tag: u8,
    pub name: u32,
    pub parent: RawTree<'a>,
    pub refinement: RawTree<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefinedTptNode<'a> {
    pub qualifier: RawTree<'a>,
    pub stats: RawNodes<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LambdaTptNode<'a> {
    pub type_params: Vec<ParameterNode<'a>>,
    pub body: RawTree<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlinedNode<'a> {
    pub expression: RawTree<'a>,
    pub call_site: Option<RawTree<'a>>,
    pub definitions: RawNodes<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchNode<'a> {
    pub modifiers: Vec<u8>,
    pub scrutinee: RawTree<'a>,
    pub cases: Vec<CaseDefNode<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TryNode<'a> {
    pub expression: RawTree<'a>,
    pub cases: Vec<CaseDefNode<'a>>,
    pub finalizer: Option<RawTree<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuoteNode<'a> {
    pub tag: u8,
    pub expression: RawTree<'a>,
    pub type_tree: RawTree<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplySigPolyNode<'a> {
    pub function: RawTree<'a>,
    pub type_tree: RawTree<'a>,
    pub arguments: Vec<RawTree<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotePatternNode<'a> {
    pub body: RawTree<'a>,
    pub quotes: RawTree<'a>,
    pub pattern_type: RawTree<'a>,
    pub bindings: Vec<RawTree<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplicePatternNode<'a> {
    pub pattern: RawTree<'a>,
    pub pattern_type: RawTree<'a>,
    /// The format does not encode a count separating type arguments from
    /// term arguments. Keep the complete ordered tail until typed decoding
    /// context is available to classify it.
    pub arguments: Vec<RawTree<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoleNode<'a> {
    pub index: u32,
    pub type_tree: RawTree<'a>,
    pub arguments: Vec<RawTree<'a>>,
}

impl<'a> SplicePatternNode<'a> {
    /// Splits the ordered argument tail once typed context supplies the
    /// number of type arguments. The wire format does not encode that count.
    pub fn split_arguments(
        &self,
        type_argument_count: usize,
    ) -> Option<(&[RawTree<'a>], &[RawTree<'a>])> {
        self.arguments
            .get(type_argument_count..)
            .map(|term_arguments| (&self.arguments[..type_argument_count], term_arguments))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchTypeNode<'a> {
    pub bound: RawTree<'a>,
    pub selector: RawTree<'a>,
    pub cases: Vec<RawTree<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchTptNode<'a> {
    /// The optional lower bound is omitted when the type-test has only a
    /// selector tree.
    pub bound: Option<RawTree<'a>>,
    pub selector: RawTree<'a>,
    pub cases: Vec<CaseDefNode<'a>>,
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
pub struct AnnotationNode<'a> {
    pub tycon: RawTree<'a>,
    pub full_annotation: RawTree<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamTypeNode {
    pub binder: AstRef,
    pub parameter_number: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeName {
    pub type_or_bounds: AstRef,
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
    TypeParam {
        name: u32,
        body: &'a [u8],
        offset: usize,
    },
    TermParam {
        name: u32,
        body: &'a [u8],
        offset: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterBody<'a> {
    pub type_tree: RawTree<'a>,
    pub tail: Vec<DefinitionTail<'a>>,
}

impl<'a> ParameterNode<'a> {
    pub fn tag(&self) -> u8 {
        match self {
            Self::TypeParam { .. } => TYPEPARAM_TAG,
            Self::TermParam { .. } => PARAM_TAG,
        }
    }

    pub fn offset(&self) -> usize {
        match self {
            Self::TypeParam { offset, .. } | Self::TermParam { offset, .. } => *offset,
        }
    }

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
    RecursionLimit {
        offset: usize,
        limit: usize,
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
            Self::RecursionLimit { offset, limit } => write!(
                formatter,
                "AST index traversal at offset {offset} exceeds the maximum depth of {limit}"
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
    /// Construct a top-level AST node list for encoding.
    ///
    /// Top-level entries in the `ASTs` section are length-delimited category-5
    /// nodes. Their `offset` fields are retained for inspection, but encoding
    /// computes fresh addresses from the emitted byte stream.
    pub fn from_entries(nodes: Vec<RawNode<'a>>) -> Result<Self, AstError> {
        for node in &nodes {
            let Some(category) = NodeCategory::from_tag(node.tag) else {
                return Err(AstError::InvalidTag {
                    tag: node.tag,
                    offset: node.offset,
                });
            };
            if category != NodeCategory::Category5 {
                return Err(AstError::UnsupportedCategory {
                    tag: node.tag,
                    offset: node.offset,
                });
            }
        }
        Ok(Self { nodes })
    }

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

    pub fn address_index(&self) -> AstAddressIndex<'a> {
        let mut nodes = self.nodes.clone();
        nodes.sort_unstable_by_key(|node| node.offset);
        let mut all_nodes = nodes
            .iter()
            .map(|node| AstTreeNode {
                tag: node.tag,
                offset: node.offset,
            })
            .collect::<Vec<_>>();
        all_nodes.sort_unstable_by_key(|node| node.offset);
        AstAddressIndex {
            nodes,
            all_nodes,
            edges: Vec::new(),
            edges_by_child: Vec::new(),
        }
    }

    pub(crate) fn deep_address_index_with_source_and_max_depth(
        &self,
        source: &[u8],
        max_depth: usize,
    ) -> Result<AstAddressIndex<'a>, AstError> {
        let mut nodes = Vec::new();
        let mut all_nodes = Vec::new();
        let context = AstIndexContext::new(max_depth);
        collect_raw_nodes_deep(self, source, 0, &context, &mut nodes, &mut all_nodes)?;
        nodes.sort_unstable_by_key(|node| node.offset);
        all_nodes.sort_unstable_by_key(|node| node.offset);
        let edges = context.into_edges();
        let mut edges_by_child = edges.clone();
        edges_by_child.sort_unstable_by_key(|edge| edge.child.offset);
        Ok(AstAddressIndex {
            nodes,
            all_nodes,
            edges,
            edges_by_child,
        })
    }

    pub fn encode(&self, writer: &mut Writer) -> Result<(), WriteError> {
        for node in &self.nodes {
            node.encode(writer)?;
        }
        Ok(())
    }

    pub fn encode_with_addresses(&self) -> Result<EncodedAstNodes, WriteError> {
        let mut writer = Writer::new();
        let mut addresses = Vec::with_capacity(self.nodes.len());

        for node in &self.nodes {
            addresses.push(u32::try_from(writer.position()).map_err(|_| {
                WriteError::NatOverflow {
                    value: writer.position() as u64,
                }
            })?);
            node.encode(&mut writer)?;
        }

        Ok(EncodedAstNodes {
            bytes: writer.into_inner(),
            addresses,
        })
    }
}

struct AstIndexContext {
    depth: Cell<usize>,
    max_depth: usize,
    parent: Cell<Option<AstTreeNode>>,
    edges: RefCell<Vec<AstTreeEdge>>,
}

impl AstIndexContext {
    fn new(max_depth: usize) -> Self {
        Self {
            depth: Cell::new(0),
            max_depth,
            parent: Cell::new(None),
            edges: RefCell::new(Vec::new()),
        }
    }

    fn record_node(&self, node: AstTreeNode) {
        if let Some(parent) = self.parent.get() {
            self.edges.borrow_mut().push(AstTreeEdge {
                parent,
                child: node,
            });
        }
    }

    fn with_parent<T>(
        &self,
        parent: Option<AstTreeNode>,
        action: impl FnOnce() -> Result<T, AstError>,
    ) -> Result<T, AstError> {
        let previous = self.parent.replace(parent);
        let result = action();
        self.parent.set(previous);
        result
    }

    fn into_edges(self) -> Vec<AstTreeEdge> {
        self.edges.into_inner()
    }

    fn enter(&self, offset: usize) -> Result<AstIndexDepth<'_>, AstError> {
        let depth = self.depth.get();
        if depth >= self.max_depth {
            return Err(AstError::RecursionLimit {
                offset,
                limit: self.max_depth,
            });
        }
        self.depth.set(depth + 1);
        Ok(AstIndexDepth { context: self })
    }
}

struct AstIndexDepth<'a> {
    context: &'a AstIndexContext,
}

impl Drop for AstIndexDepth<'_> {
    fn drop(&mut self) {
        self.context
            .depth
            .set(self.context.depth.get().saturating_sub(1));
    }
}

impl<'a> AstAddressIndex<'a> {
    /// Returns the category-five node with this AST address, if present.
    pub fn get(&self, address: u32) -> Option<&RawNode<'a>> {
        self.nodes
            .binary_search_by_key(&(address as usize), |node| node.offset)
            .ok()
            .map(|index| &self.nodes[index])
    }

    /// Resolves a reference when its target is a category-five node.
    pub fn resolve(&self, reference: AstRef) -> Option<&RawNode<'a>> {
        self.get(reference.address)
    }

    /// Iterates over indexed category-five nodes in absolute address order.
    pub fn iter(&self) -> impl Iterator<Item = &RawNode<'a>> {
        self.nodes.iter()
    }

    /// Returns any visible AST node with this address, including category-one
    /// through category-four tree nodes.
    pub fn get_node(&self, address: u32) -> Option<AstTreeNode> {
        self.all_nodes
            .binary_search_by_key(&(address as usize), |node| node.offset)
            .ok()
            .map(|index| self.all_nodes[index])
    }

    /// Resolves a reference to any visible AST node.
    pub fn resolve_node(&self, reference: AstRef) -> Option<AstTreeNode> {
        self.get_node(reference.address)
    }

    /// Iterates over all visible nodes in absolute address order.
    pub fn iter_nodes(&self) -> impl Iterator<Item = AstTreeNode> + '_ {
        self.all_nodes.iter().copied()
    }

    /// Iterates over visible nodes with `tag` in absolute address order.
    ///
    /// The query covers category-one through category-five nodes collected by
    /// the index. An unknown tag simply produces an empty iterator; callers
    /// that need to distinguish an unknown tag from a tag absent in this
    /// index can use [`NodeCategory::from_tag`] first.
    pub fn iter_nodes_with_tag(&self, tag: u8) -> impl Iterator<Item = AstTreeNode> + '_ {
        self.all_nodes
            .iter()
            .copied()
            .filter(move |node| node.tag == tag)
    }

    /// Iterates over visible nodes whose absolute addresses are in
    /// `[start, end)`, retaining address order.
    pub fn iter_nodes_in_address_range(
        &self,
        start: u32,
        end: u32,
    ) -> impl Iterator<Item = AstTreeNode> + '_ {
        self.all_nodes.iter().copied().filter(move |node| {
            let address = node.offset as u64;
            address >= u64::from(start) && address < u64::from(end)
        })
    }

    /// Returns the structural parent of a visible node.
    ///
    /// Top-level nodes and indexes created with [`RawNodes::address_index`]
    /// have no parent. The lookup is address-based and returns `None` both
    /// for an unknown address and for a root node.
    pub fn parent_of(&self, child_address: u32) -> Option<AstTreeNode> {
        self.edges_by_child
            .binary_search_by_key(&child_address, |edge| edge.child.offset as u32)
            .ok()
            .map(|index| self.edges_by_child[index].parent)
    }

    /// Iterates over the direct children of a visible node in wire order.
    ///
    /// The result is empty for an unknown address, a leaf node, or a shallow
    /// index created with [`RawNodes::address_index`].
    pub fn children_of(&self, parent_address: u32) -> impl Iterator<Item = AstTreeNode> + '_ {
        self.edges
            .iter()
            .filter(move |edge| edge.parent.offset as u64 == u64::from(parent_address))
            .map(|edge| edge.child)
    }

    /// Iterates over all parent-to-child edges in traversal order.
    pub fn iter_tree_edges(&self) -> impl Iterator<Item = AstTreeEdge> + '_ {
        self.edges.iter().copied()
    }

    pub fn node_addresses(&self) -> impl Iterator<Item = u32> + '_ {
        self.all_nodes.iter().map(|node| node.offset as u32)
    }

    pub fn addresses(&self) -> impl Iterator<Item = u32> + '_ {
        self.nodes.iter().map(|node| node.offset as u32)
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

fn raw_payload_base(source: &[u8], node: &RawNode<'_>, base: usize) -> usize {
    let fallback = base
        .saturating_add(node.offset)
        .saturating_add(1)
        .saturating_add(nat_width(node.payload.len()));

    let Some(relative) = subslice_offset(source, node.payload) else {
        return fallback;
    };
    base.saturating_add(relative)
}

fn nat_width(mut value: usize) -> usize {
    let mut width = 1;
    while value >= 128 {
        value >>= 7;
        width += 1;
    }
    width
}

fn subslice_offset(source: &[u8], subslice: &[u8]) -> Option<usize> {
    if subslice.is_empty() || subslice.len() > source.len() {
        return None;
    }

    (0..=source.len() - subslice.len())
        .find(|start| std::ptr::eq(source[*start..].as_ptr(), subslice.as_ptr()))
}

fn collect_raw_nodes_deep<'a>(
    nodes: &RawNodes<'a>,
    source: &[u8],
    base: usize,
    context: &AstIndexContext,
    output: &mut Vec<RawNode<'a>>,
    all_output: &mut Vec<AstTreeNode>,
) -> Result<(), AstError> {
    for node in &nodes.nodes {
        collect_raw_node_deep(node, source, base, context, output, all_output)?;
    }
    Ok(())
}

fn collect_raw_node_deep<'a>(
    node: &RawNode<'a>,
    source: &[u8],
    base: usize,
    context: &AstIndexContext,
    output: &mut Vec<RawNode<'a>>,
    all_output: &mut Vec<AstTreeNode>,
) -> Result<(), AstError> {
    let _depth = context.enter(base.saturating_add(node.offset))?;
    let absolute_offset = base.saturating_add(node.offset);
    let mut located = node.clone();
    located.offset = absolute_offset;
    output.push(located);
    let visible = AstTreeNode {
        tag: node.tag,
        offset: absolute_offset,
    };
    context.record_node(visible);
    all_output.push(visible);

    let payload_base = raw_payload_base(source, node, base);
    let Ok(structured) = node.decode_structured() else {
        return Ok(());
    };
    context.with_parent(Some(visible), || {
        collect_structured_nodes(
            &structured,
            node.payload,
            payload_base,
            context,
            output,
            all_output,
        )
    })
}

fn collect_tree_nodes<'a>(
    tree: &RawTree<'a>,
    source: &[u8],
    base: usize,
    context: &AstIndexContext,
    output: &mut Vec<RawNode<'a>>,
    all_output: &mut Vec<AstTreeNode>,
) -> Result<(), AstError> {
    let offset = match tree {
        RawTree::Leaf(term) => term.offset,
        RawTree::Ast { offset, .. } | RawTree::NatAst { offset, .. } => *offset,
        RawTree::LengthNode(node) => node.offset,
    };
    let _depth = context.enter(base.saturating_add(offset))?;
    match tree {
        RawTree::Leaf(term) => {
            let visible = AstTreeNode {
                tag: term.tag,
                offset: base.saturating_add(term.offset),
            };
            context.record_node(visible);
            all_output.push(visible);
        }
        RawTree::Ast { tag, offset, child }
        | RawTree::NatAst {
            tag, offset, child, ..
        } => {
            let visible = AstTreeNode {
                tag: *tag,
                offset: base.saturating_add(*offset),
            };
            context.record_node(visible);
            all_output.push(visible);
            context.with_parent(Some(visible), || {
                collect_tree_nodes(child, source, base, context, output, all_output)
            })?;
        }
        RawTree::LengthNode(node) => {
            let absolute_offset = base.saturating_add(node.offset);
            let mut located = node.clone();
            located.offset = absolute_offset;
            output.push(located);
            let visible = AstTreeNode {
                tag: node.tag,
                offset: absolute_offset,
            };
            context.record_node(visible);
            all_output.push(visible);

            let payload_base = raw_payload_base(source, node, base);
            let Ok(structured) = node.decode_structured() else {
                return Ok(());
            };
            context.with_parent(Some(visible), || {
                collect_structured_nodes(
                    &structured,
                    node.payload,
                    payload_base,
                    context,
                    output,
                    all_output,
                )
            })?;
        }
    }
    Ok(())
}

fn collect_trees_nodes<'a>(
    trees: &[RawTree<'a>],
    source: &[u8],
    base: usize,
    context: &AstIndexContext,
    output: &mut Vec<RawNode<'a>>,
    all_output: &mut Vec<AstTreeNode>,
) -> Result<(), AstError> {
    for tree in trees {
        collect_tree_nodes(tree, source, base, context, output, all_output)?;
    }
    Ok(())
}

fn collect_raw_node_list_nodes<'a>(
    nodes: &RawNodes<'a>,
    source: &[u8],
    base: usize,
    context: &AstIndexContext,
    output: &mut Vec<RawNode<'a>>,
    all_output: &mut Vec<AstTreeNode>,
) -> Result<(), AstError> {
    collect_raw_nodes_deep(nodes, source, base, context, output, all_output)
}

fn collect_definition_tail_nodes<'a>(
    tail: &[DefinitionTail<'a>],
    source: &[u8],
    base: usize,
    context: &AstIndexContext,
    output: &mut Vec<RawNode<'a>>,
    all_output: &mut Vec<AstTreeNode>,
) -> Result<(), AstError> {
    for entry in tail {
        if let DefinitionTail::Annotation(node) = entry {
            collect_raw_node_deep(node, source, base, context, output, all_output)?;
        }
    }
    Ok(())
}

fn collect_definition_body_nodes<'a>(
    body: &DefinitionBody<'a>,
    source: &[u8],
    base: usize,
    context: &AstIndexContext,
    output: &mut Vec<RawNode<'a>>,
    all_output: &mut Vec<AstTreeNode>,
) -> Result<(), AstError> {
    match body {
        DefinitionBody::ValDef {
            type_tree,
            rhs,
            tail,
            ..
        } => {
            collect_tree_nodes(type_tree, source, base, context, output, all_output)?;
            if let Some(rhs) = rhs {
                collect_tree_nodes(rhs, source, base, context, output, all_output)?;
            }
            collect_definition_tail_nodes(tail, source, base, context, output, all_output)?;
        }
        DefinitionBody::TypeDef {
            type_or_template,
            tail,
            ..
        } => {
            collect_tree_nodes(type_or_template, source, base, context, output, all_output)?;
            collect_definition_tail_nodes(tail, source, base, context, output, all_output)?;
        }
    }
    Ok(())
}

fn collect_parameter_body_nodes<'a>(
    parameter: &ParameterNode<'a>,
    source: &[u8],
    base: usize,
    context: &AstIndexContext,
    output: &mut Vec<RawNode<'a>>,
    all_output: &mut Vec<AstTreeNode>,
) -> Result<(), AstError> {
    let body = parameter.decode_body()?;
    let body_base = subslice_offset(source, parameter.body())
        .map(|relative| base.saturating_add(relative))
        .unwrap_or(base);
    collect_tree_nodes(
        &body.type_tree,
        parameter.body(),
        body_base,
        context,
        output,
        all_output,
    )?;
    collect_definition_tail_nodes(
        &body.tail,
        parameter.body(),
        body_base,
        context,
        output,
        all_output,
    )
}

fn collect_parameters_nodes<'a>(
    parameters: &[ParameterNode<'a>],
    source: &[u8],
    base: usize,
    context: &AstIndexContext,
    output: &mut Vec<RawNode<'a>>,
    all_output: &mut Vec<AstTreeNode>,
) -> Result<(), AstError> {
    for parameter in parameters {
        let absolute_offset = base.saturating_add(parameter.offset());
        let raw = RawNode {
            tag: parameter.tag(),
            offset: absolute_offset,
            payload: parameter.body(),
        };
        let tag = raw.tag;
        output.push(raw);
        let visible = AstTreeNode {
            tag,
            offset: absolute_offset,
        };
        context.record_node(visible);
        all_output.push(visible);
        context.with_parent(Some(visible), || {
            collect_parameter_body_nodes(parameter, source, base, context, output, all_output)
        })?;
    }
    Ok(())
}

fn collect_case_def_nodes<'a>(
    case_def: &CaseDefNode<'a>,
    source: &[u8],
    base: usize,
    context: &AstIndexContext,
    output: &mut Vec<RawNode<'a>>,
    all_output: &mut Vec<AstTreeNode>,
) -> Result<(), AstError> {
    collect_tree_nodes(&case_def.pattern, source, base, context, output, all_output)?;
    collect_tree_nodes(&case_def.body, source, base, context, output, all_output)?;
    if let Some(guard) = &case_def.guard {
        collect_tree_nodes(guard, source, base, context, output, all_output)?;
    }
    Ok(())
}

fn collect_embedded_case_def_node<'a>(
    case_def: &CaseDefNode<'a>,
    source: &[u8],
    base: usize,
    context: &AstIndexContext,
    output: &mut Vec<RawNode<'a>>,
    all_output: &mut Vec<AstTreeNode>,
) -> Result<(), AstError> {
    let absolute_offset = base.saturating_add(case_def.offset);
    let raw = RawNode {
        tag: CASEDEF_TAG,
        offset: case_def.offset,
        payload: case_def.payload,
    };
    let mut located = raw.clone();
    located.offset = absolute_offset;
    output.push(located);
    let visible = AstTreeNode {
        tag: CASEDEF_TAG,
        offset: absolute_offset,
    };
    context.record_node(visible);
    all_output.push(visible);
    let payload_base = raw_payload_base(source, &raw, base);
    context.with_parent(Some(visible), || {
        collect_case_def_nodes(
            case_def,
            case_def.payload,
            payload_base,
            context,
            output,
            all_output,
        )
    })
}

fn collect_case_defs_nodes<'a>(
    case_defs: &[CaseDefNode<'a>],
    source: &[u8],
    base: usize,
    context: &AstIndexContext,
    output: &mut Vec<RawNode<'a>>,
    all_output: &mut Vec<AstTreeNode>,
) -> Result<(), AstError> {
    for case_def in case_defs {
        collect_embedded_case_def_node(case_def, source, base, context, output, all_output)?;
    }
    Ok(())
}

fn collect_structured_nodes<'a>(
    node: &StructuredNode<'a>,
    source: &[u8],
    base: usize,
    context: &AstIndexContext,
    output: &mut Vec<RawNode<'a>>,
    all_output: &mut Vec<AstTreeNode>,
) -> Result<(), AstError> {
    macro_rules! tree {
        ($tree:expr) => {
            collect_tree_nodes($tree, source, base, context, output, all_output)?
        };
    }
    macro_rules! trees {
        ($trees:expr) => {
            collect_trees_nodes($trees, source, base, context, output, all_output)?
        };
    }
    macro_rules! raw_nodes {
        ($nodes:expr) => {
            collect_raw_node_list_nodes($nodes, source, base, context, output, all_output)?
        };
    }
    macro_rules! definition_body {
        ($body:expr) => {
            collect_definition_body_nodes($body, source, base, context, output, all_output)?
        };
    }
    macro_rules! definition_tail {
        ($tail:expr) => {
            collect_definition_tail_nodes($tail, source, base, context, output, all_output)?
        };
    }
    macro_rules! parameters {
        ($parameters:expr) => {
            collect_parameters_nodes($parameters, source, base, context, output, all_output)?
        };
    }
    macro_rules! case_defs {
        ($case_defs:expr) => {
            collect_case_defs_nodes($case_defs, source, base, context, output, all_output)?
        };
    }

    match node {
        StructuredNode::Package(package) => raw_nodes!(&package.stats),
        StructuredNode::ValDef(body) | StructuredNode::TypeDef(body) => definition_body!(body),
        StructuredNode::DefDef(body) => {
            parameters!(&body.parameters);
            tree!(&body.return_type);
            if let Some(rhs) = &body.rhs {
                tree!(rhs);
            }
            definition_tail!(&body.tail);
        }
        StructuredNode::ImportExport(import_export) => {
            tree!(&import_export.expr);
            for selector in &import_export.selectors {
                if let ImportSelector::Bounded { type_tree } = selector {
                    tree!(type_tree);
                }
            }
        }
        StructuredNode::Parameter(parameter) => {
            collect_parameter_body_nodes(parameter, source, base, context, output, all_output)?;
        }
        StructuredNode::Apply(apply) => {
            tree!(&apply.function);
            trees!(&apply.arguments);
        }
        StructuredNode::TypeApply(type_apply) => {
            tree!(&type_apply.function);
            trees!(&type_apply.type_arguments);
        }
        StructuredNode::Typed(typed) => {
            tree!(&typed.expression);
            tree!(&typed.type_tree);
        }
        StructuredNode::Assign(assign) => {
            tree!(&assign.left);
            tree!(&assign.right);
        }
        StructuredNode::Block(block) => {
            tree!(&block.expression);
            trees!(&block.stats);
        }
        StructuredNode::If(if_node) => {
            tree!(&if_node.condition);
            tree!(&if_node.then_branch);
            tree!(&if_node.else_branch);
        }
        StructuredNode::Lambda(lambda) => {
            tree!(&lambda.method);
            if let Some(target_type) = &lambda.target_type {
                tree!(target_type);
            }
        }
        StructuredNode::Match(match_node) => {
            tree!(&match_node.scrutinee);
            case_defs!(&match_node.cases);
        }
        StructuredNode::Return(return_node) => {
            if let Some(expression) = &return_node.expression {
                tree!(expression);
            }
        }
        StructuredNode::While(while_node) => {
            tree!(&while_node.condition);
            tree!(&while_node.body);
        }
        StructuredNode::Try(try_node) => {
            tree!(&try_node.expression);
            case_defs!(&try_node.cases);
            if let Some(finalizer) = &try_node.finalizer {
                tree!(finalizer);
            }
        }
        StructuredNode::Inlined(inlined) => {
            tree!(&inlined.expression);
            if let Some(call_site) = &inlined.call_site {
                tree!(call_site);
            }
            raw_nodes!(&inlined.definitions);
        }
        StructuredNode::SelectOuter(select_outer) => {
            tree!(&select_outer.qualifier);
            tree!(&select_outer.underlying_type);
        }
        StructuredNode::Repeated(repeated) => {
            tree!(&repeated.element_type);
            trees!(&repeated.elements);
        }
        StructuredNode::Bind(bind) => {
            tree!(&bind.type_tree);
            if let BindBody::Pattern(pattern) = &bind.body {
                tree!(pattern);
            }
        }
        StructuredNode::Alternative(alternative) => {
            trees!(&alternative.alternatives);
        }
        StructuredNode::Unapply(unapply) => {
            tree!(&unapply.function);
            for implicit_arg in &unapply.implicit_args {
                let absolute_offset = base.saturating_add(implicit_arg.offset);
                let visible = AstTreeNode {
                    tag: implicit_arg.tag,
                    offset: absolute_offset,
                };
                context.record_node(visible);
                all_output.push(visible);
                context.with_parent(Some(visible), || {
                    collect_tree_nodes(
                        &implicit_arg.child,
                        source,
                        base.saturating_add(implicit_arg.offset).saturating_add(1),
                        context,
                        output,
                        all_output,
                    )
                })?;
            }
            tree!(&unapply.type_tree);
            trees!(&unapply.patterns);
        }
        StructuredNode::Annotated(annotated) => {
            tree!(&annotated.underlying);
            tree!(&annotated.annotation);
        }
        StructuredNode::Annotation(annotation) => {
            tree!(&annotation.tycon);
            tree!(&annotation.full_annotation);
        }
        StructuredNode::CaseDef(case_def) => {
            collect_case_def_nodes(case_def, source, base, context, output, all_output)?;
        }
        StructuredNode::Template(template) => {
            parameters!(&template.type_params);
            parameters!(&template.term_params);
            trees!(&template.parents);
            if let Some(self_def) = &template.self_def {
                tree!(self_def);
            }
            raw_nodes!(&template.stats);
        }
        StructuredNode::Super(super_node) => {
            tree!(&super_node.this_term);
            if let Some(mixin_type) = &super_node.mixin_type {
                tree!(mixin_type);
            }
        }
        StructuredNode::BinaryType(binary) => {
            tree!(&binary.left);
            tree!(&binary.right);
        }
        StructuredNode::RefinedType(refined) => {
            tree!(&refined.parent);
            tree!(&refined.refinement);
        }
        StructuredNode::RefinedTpt(refined) => {
            tree!(&refined.qualifier);
            raw_nodes!(&refined.stats);
        }
        StructuredNode::AppliedType(applied) => {
            tree!(&applied.tycon);
            trees!(&applied.arguments);
        }
        StructuredNode::TypeBounds(bounds) => {
            tree!(&bounds.low_or_alias);
            if let Some(high) = &bounds.high {
                tree!(high);
            }
        }
        StructuredNode::FlexibleType(flexible) => {
            tree!(&flexible.underlying_type);
        }
        StructuredNode::LambdaTpt(lambda) => {
            parameters!(&lambda.type_params);
            tree!(&lambda.body);
        }
        StructuredNode::PolyType(poly) => {
            tree!(&poly.result_type);
        }
        StructuredNode::ParamType(_) => {}
        StructuredNode::MethodType(method) => {
            tree!(&method.result_type);
        }
        StructuredNode::ApplySigPoly(apply) => {
            tree!(&apply.function);
            tree!(&apply.type_tree);
            trees!(&apply.arguments);
        }
        StructuredNode::Quote(quote) => {
            tree!(&quote.expression);
            tree!(&quote.type_tree);
        }
        StructuredNode::QuotePattern(pattern) => {
            tree!(&pattern.body);
            tree!(&pattern.quotes);
            tree!(&pattern.pattern_type);
            trees!(&pattern.bindings);
        }
        StructuredNode::SplicePattern(pattern) => {
            tree!(&pattern.pattern);
            tree!(&pattern.pattern_type);
            trees!(&pattern.arguments);
        }
        StructuredNode::MatchType(match_type) => {
            tree!(&match_type.bound);
            tree!(&match_type.selector);
            trees!(&match_type.cases);
        }
        StructuredNode::MatchTpt(match_tpt) => {
            if let Some(bound) = &match_tpt.bound {
                tree!(bound);
            }
            tree!(&match_tpt.selector);
            case_defs!(&match_tpt.cases);
        }
        StructuredNode::Hole(hole) => {
            tree!(&hole.type_tree);
            trees!(&hole.arguments);
        }
        StructuredNode::InReference(reference) => {
            tree!(&reference.qualifier);
            tree!(&reference.underlying_type);
        }
        StructuredNode::SelectIn(select_in) => {
            tree!(&select_in.qualifier);
            tree!(&select_in.underlying_type);
        }
        StructuredNode::Raw(_) => {}
    }
    Ok(())
}

impl<'a> RawNode<'a> {
    pub fn decode_structured(&self) -> Result<StructuredNode<'a>, AstError> {
        Ok(match self.tag {
            PACKAGE_TAG => StructuredNode::Package(self.decode_package()?),
            VALDEF_TAG => StructuredNode::ValDef(self.decode_definition_body()?),
            DEFDEF_TAG => StructuredNode::DefDef(self.decode_defdef_body()?),
            TYPEDEF_TAG => StructuredNode::TypeDef(self.decode_definition_body()?),
            IMPORT_TAG | EXPORT_TAG => StructuredNode::ImportExport(self.decode_import_export()?),
            TYPEPARAM_TAG | PARAM_TAG => StructuredNode::Parameter(self.decode_parameter()?),
            APPLY_TAG => StructuredNode::Apply(self.decode_apply()?),
            TYPEAPPLY_TAG => StructuredNode::TypeApply(self.decode_type_apply()?),
            TYPED_TAG => StructuredNode::Typed(self.decode_typed()?),
            ASSIGN_TAG => StructuredNode::Assign(self.decode_assign()?),
            BLOCK_TAG => StructuredNode::Block(self.decode_block()?),
            IF_TAG => StructuredNode::If(self.decode_if()?),
            LAMBDA_TAG => StructuredNode::Lambda(self.decode_lambda()?),
            MATCH_TAG => StructuredNode::Match(self.decode_match()?),
            RETURN_TAG => StructuredNode::Return(self.decode_return()?),
            WHILE_TAG => StructuredNode::While(self.decode_while()?),
            TRY_TAG => StructuredNode::Try(self.decode_try()?),
            INLINED_TAG => StructuredNode::Inlined(self.decode_inlined()?),
            SELECTOUTER_TAG => StructuredNode::SelectOuter(self.decode_select_outer()?),
            REPEATED_TAG => StructuredNode::Repeated(self.decode_repeated()?),
            BIND_TAG => StructuredNode::Bind(self.decode_bind()?),
            ALTERNATIVE_TAG => StructuredNode::Alternative(self.decode_alternative()?),
            UNAPPLY_TAG => StructuredNode::Unapply(self.decode_unapply()?),
            ANNOTATEDTYPE_TAG | ANNOTATEDTPT_TAG => {
                StructuredNode::Annotated(self.decode_annotated()?)
            }
            ANNOTATION_TAG => StructuredNode::Annotation(self.decode_annotation()?),
            CASEDEF_TAG => StructuredNode::CaseDef(self.decode_case_def()?),
            TEMPLATE_TAG => StructuredNode::Template(self.decode_template_structure()?),
            SUPER_TAG => StructuredNode::Super(self.decode_super()?),
            SUPERTYPE_TAG | ANDTYPE_TAG | ORTYPE_TAG | MATCHCASETYPE_TAG => {
                StructuredNode::BinaryType(match self.tag {
                    SUPERTYPE_TAG => self.decode_super_type()?,
                    ANDTYPE_TAG => self.decode_and_type()?,
                    ORTYPE_TAG => self.decode_or_type()?,
                    MATCHCASETYPE_TAG => self.decode_matchcase_type()?,
                    _ => unreachable!(),
                })
            }
            REFINEDTYPE_TAG => StructuredNode::RefinedType(self.decode_refined_type()?),
            REFINEDTPT_TAG => StructuredNode::RefinedTpt(self.decode_refined_tpt()?),
            APPLIEDTYPE_TAG | APPLIEDTPT_TAG => {
                StructuredNode::AppliedType(self.decode_applied_type()?)
            }
            TYPEBOUNDS_TAG | TYPEBOUNDSTPT_TAG => {
                StructuredNode::TypeBounds(self.decode_type_bounds()?)
            }
            FLEXIBLETYPE_TAG => StructuredNode::FlexibleType(self.decode_flexible_type()?),
            LAMBDATPT_TAG => StructuredNode::LambdaTpt(self.decode_lambda_tpt()?),
            POLYTYPE_TAG | TYPELAMBDATYPE_TAG => StructuredNode::PolyType(self.decode_poly_type()?),
            PARAMTYPE_TAG => StructuredNode::ParamType(self.decode_param_type()?),
            METHODTYPE_TAG => StructuredNode::MethodType(self.decode_method_type()?),
            APPLYSIGPOLY_TAG => StructuredNode::ApplySigPoly(self.decode_apply_sigpoly()?),
            QUOTE_TAG | SPLICE_TAG => StructuredNode::Quote(match self.tag {
                QUOTE_TAG => self.decode_quote()?,
                SPLICE_TAG => self.decode_splice()?,
                _ => unreachable!(),
            }),
            QUOTEPATTERN_TAG => StructuredNode::QuotePattern(self.decode_quote_pattern()?),
            SPLICEPATTERN_TAG => StructuredNode::SplicePattern(self.decode_splice_pattern()?),
            MATCHTYPE_TAG => StructuredNode::MatchType(self.decode_match_type()?),
            MATCHTPT_TAG => StructuredNode::MatchTpt(self.decode_match_tpt()?),
            HOLE_TAG => StructuredNode::Hole(self.decode_hole()?),
            TERMREFIN_TAG | TYPEREFIN_TAG => {
                StructuredNode::InReference(self.decode_in_reference()?)
            }
            SELECTIN_TAG => StructuredNode::SelectIn(self.decode_select_in()?),
            _ => StructuredNode::Raw(self.clone()),
        })
    }

    /// Returns AST references contained in the supported structured payload
    /// of this node, in wire order.
    ///
    /// Unknown category-5 nodes and currently opaque definition tails are
    /// skipped. This method therefore never assigns a guessed grammar to a
    /// future tag; callers that need lossless handling can use
    /// [`RawNode::decode_structured`] and inspect the `Raw` variant.
    pub fn ast_refs(&self) -> Result<Vec<AstRef>, AstError> {
        let mut references = Vec::new();
        self.visit_ast_refs(&mut |reference| references.push(reference))?;
        Ok(references)
    }

    /// Visits AST references contained in the supported structured payload
    /// of this node without allocating a result vector.
    pub fn visit_ast_refs(&self, visitor: &mut impl FnMut(AstRef)) -> Result<(), AstError> {
        let structured = self.decode_structured()?;
        collect_structured_ast_refs(&structured, visitor)
    }

    /// Returns name-table references contained in the supported structured
    /// payload of this node, in wire order.
    ///
    /// Unknown category-five nodes remain opaque and produce no references.
    pub fn name_refs(&self) -> Result<Vec<NameRef>, AstError> {
        let mut references = Vec::new();
        self.visit_name_refs(&mut |reference| references.push(reference))?;
        Ok(references)
    }

    /// Visits name-table references contained in the supported structured
    /// payload of this node without allocating a result vector.
    pub fn visit_name_refs(&self, visitor: &mut impl FnMut(NameRef)) -> Result<(), AstError> {
        let structured = self.decode_structured()?;
        collect_structured_name_refs(&structured, visitor)
    }

    pub fn encode(&self, writer: &mut Writer) -> Result<(), WriteError> {
        if NodeCategory::from_tag(self.tag) != Some(NodeCategory::Category5) {
            return Err(WriteError::InvalidTag { tag: self.tag });
        }
        writer.write_u8(self.tag);
        writer.write_length_prefixed_bytes(self.payload)
    }

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
        let name = reader.read_nat()?;
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
                    name,
                    type_tree: first,
                    rhs,
                    tail,
                })
            }
            TYPEDEF_TAG => Ok(DefinitionBody::TypeDef {
                name,
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
        let name = reader.read_nat()?;
        let mut parameters = Vec::new();
        let mut clauses = Vec::new();
        let mut header_items = Vec::new();

        while !reader.is_at_end() {
            match reader.peek_u8()? {
                TYPEPARAM_TAG | PARAM_TAG => {
                    let parameter = match RawTree::decode(&mut reader)? {
                        RawTree::LengthNode(raw) => raw.decode_parameter()?,
                        _ => unreachable!("parameter tags are category-five tags"),
                    };
                    header_items.push(DefDefHeaderItem::Parameter(parameter.clone()));
                    parameters.push(parameter);
                }
                EMPTYCLAUSE_TAG | SPLITCLAUSE_TAG => {
                    let clause = reader.read_u8()?;
                    header_items.push(DefDefHeaderItem::Clause(clause));
                    clauses.push(clause);
                }
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
            name,
            parameters,
            clauses,
            header_items,
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
            payload: self.payload,
            offset: self.offset,
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
        let (body, remainder) = if reader.is_at_end() {
            (
                BindBody::Type {
                    modifiers: Vec::new(),
                },
                &self.payload[reader.position()..],
            )
        } else if is_modifier_tag(reader.peek_u8()?) {
            let mut modifiers = Vec::new();
            while !reader.is_at_end() {
                let offset = reader.position();
                let modifier = reader.read_u8()?;
                if !is_modifier_tag(modifier) {
                    return Err(AstError::UnexpectedTag {
                        expected: INLINE_TAG,
                        actual: modifier,
                        offset: self.offset + offset,
                    });
                }
                modifiers.push(modifier);
            }
            (
                BindBody::Type { modifiers },
                &self.payload[reader.position()..],
            )
        } else {
            let pattern = RawTree::decode(&mut reader)?;
            let remainder = reader.read_bytes(reader.remaining())?;
            (BindBody::Pattern(pattern), remainder)
        };

        Ok(BindNode {
            name,
            type_tree,
            body,
            remainder,
        })
    }

    pub fn decode_hole(&self) -> Result<HoleNode<'a>, AstError> {
        if self.tag != HOLE_TAG {
            return Err(AstError::UnexpectedTag {
                expected: HOLE_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let index = reader.read_nat()?;
        let type_tree = RawTree::decode(&mut reader)?;
        let mut arguments = Vec::new();
        while !reader.is_at_end() {
            arguments.push(RawTree::decode(&mut reader)?);
        }

        Ok(HoleNode {
            index,
            type_tree,
            arguments,
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

    pub fn decode_refined_type(&self) -> Result<RefinedTypeNode<'a>, AstError> {
        if self.tag != REFINEDTYPE_TAG {
            return Err(AstError::UnexpectedTag {
                expected: REFINEDTYPE_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let name = reader.read_nat()?;
        let parent = RawTree::decode(&mut reader)?;
        let refinement = RawTree::decode(&mut reader)?;
        if !reader.is_at_end() {
            return Err(AstError::UnsupportedCategory {
                tag: self.tag,
                offset: self.offset,
            });
        }

        Ok(RefinedTypeNode {
            tag: self.tag,
            name,
            parent,
            refinement,
        })
    }

    pub fn decode_refined_tpt(&self) -> Result<RefinedTptNode<'a>, AstError> {
        if self.tag != REFINEDTPT_TAG {
            return Err(AstError::UnexpectedTag {
                expected: REFINEDTPT_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let qualifier = RawTree::decode(&mut reader)?;
        let stats = RawNodes::decode(&mut reader)?;

        Ok(RefinedTptNode { qualifier, stats })
    }

    pub fn decode_lambda_tpt(&self) -> Result<LambdaTptNode<'a>, AstError> {
        if self.tag != LAMBDATPT_TAG {
            return Err(AstError::UnexpectedTag {
                expected: LAMBDATPT_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let mut type_params = Vec::new();
        while !reader.is_at_end() && reader.peek_u8()? == TYPEPARAM_TAG {
            let parameter = match RawTree::decode(&mut reader)? {
                RawTree::LengthNode(raw) => raw.decode_parameter()?,
                _ => unreachable!("type parameter tags are category-five tags"),
            };
            type_params.push(parameter);
        }
        let body = RawTree::decode(&mut reader)?;
        if !reader.is_at_end() {
            return Err(AstError::UnsupportedCategory {
                tag: self.tag,
                offset: self.offset,
            });
        }

        Ok(LambdaTptNode { type_params, body })
    }

    pub fn decode_inlined(&self) -> Result<InlinedNode<'a>, AstError> {
        if self.tag != INLINED_TAG {
            return Err(AstError::UnexpectedTag {
                expected: INLINED_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let expression = RawTree::decode(&mut reader)?;
        let call_site =
            if reader.is_at_end() || matches!(reader.peek_u8()?, VALDEF_TAG | DEFDEF_TAG) {
                None
            } else {
                Some(RawTree::decode(&mut reader)?)
            };
        let definitions = RawNodes::decode(&mut reader)?;

        Ok(InlinedNode {
            expression,
            call_site,
            definitions,
        })
    }

    pub fn decode_match(&self) -> Result<MatchNode<'a>, AstError> {
        if self.tag != MATCH_TAG {
            return Err(AstError::UnexpectedTag {
                expected: MATCH_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let mut modifiers = Vec::new();
        if !reader.is_at_end() {
            match reader.peek_u8()? {
                IMPLICIT_TAG | INLINE_TAG | SUBMATCH_TAG => modifiers.push(reader.read_u8()?),
                _ => {}
            }
        }
        let scrutinee = RawTree::decode(&mut reader)?;
        let cases = read_case_defs(&mut reader)?;
        if !reader.is_at_end() {
            return Err(AstError::UnsupportedCategory {
                tag: self.tag,
                offset: self.offset,
            });
        }

        Ok(MatchNode {
            modifiers,
            scrutinee,
            cases,
        })
    }

    pub fn decode_try(&self) -> Result<TryNode<'a>, AstError> {
        if self.tag != TRY_TAG {
            return Err(AstError::UnexpectedTag {
                expected: TRY_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let expression = RawTree::decode(&mut reader)?;
        let cases = read_case_defs(&mut reader)?;
        let finalizer = if reader.is_at_end() {
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

        Ok(TryNode {
            expression,
            cases,
            finalizer,
        })
    }

    pub fn decode_quote(&self) -> Result<QuoteNode<'a>, AstError> {
        self.decode_quote_like(QUOTE_TAG)
    }

    pub fn decode_splice(&self) -> Result<QuoteNode<'a>, AstError> {
        self.decode_quote_like(SPLICE_TAG)
    }

    fn decode_quote_like(&self, expected: u8) -> Result<QuoteNode<'a>, AstError> {
        if self.tag != expected {
            return Err(AstError::UnexpectedTag {
                expected,
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

        Ok(QuoteNode {
            tag: self.tag,
            expression,
            type_tree,
        })
    }

    pub fn decode_apply_sigpoly(&self) -> Result<ApplySigPolyNode<'a>, AstError> {
        if self.tag != APPLYSIGPOLY_TAG {
            return Err(AstError::UnexpectedTag {
                expected: APPLYSIGPOLY_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let function = RawTree::decode(&mut reader)?;
        let type_tree = RawTree::decode(&mut reader)?;
        let mut arguments = Vec::new();
        while !reader.is_at_end() {
            arguments.push(RawTree::decode(&mut reader)?);
        }

        Ok(ApplySigPolyNode {
            function,
            type_tree,
            arguments,
        })
    }

    pub fn decode_quote_pattern(&self) -> Result<QuotePatternNode<'a>, AstError> {
        if self.tag != QUOTEPATTERN_TAG {
            return Err(AstError::UnexpectedTag {
                expected: QUOTEPATTERN_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let body = RawTree::decode(&mut reader)?;
        let quotes = RawTree::decode(&mut reader)?;
        let pattern_type = RawTree::decode(&mut reader)?;
        let mut bindings = Vec::new();
        while !reader.is_at_end() {
            bindings.push(RawTree::decode(&mut reader)?);
        }

        Ok(QuotePatternNode {
            body,
            quotes,
            pattern_type,
            bindings,
        })
    }

    pub fn decode_splice_pattern(&self) -> Result<SplicePatternNode<'a>, AstError> {
        if self.tag != SPLICEPATTERN_TAG {
            return Err(AstError::UnexpectedTag {
                expected: SPLICEPATTERN_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let pattern = RawTree::decode(&mut reader)?;
        let pattern_type = RawTree::decode(&mut reader)?;
        let mut arguments = Vec::new();
        while !reader.is_at_end() {
            arguments.push(RawTree::decode(&mut reader)?);
        }

        Ok(SplicePatternNode {
            pattern,
            pattern_type,
            arguments,
        })
    }

    pub fn decode_match_type(&self) -> Result<MatchTypeNode<'a>, AstError> {
        if self.tag != MATCHTYPE_TAG {
            return Err(AstError::UnexpectedTag {
                expected: MATCHTYPE_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let bound = RawTree::decode(&mut reader)?;
        let selector = RawTree::decode(&mut reader)?;
        let mut cases = Vec::new();
        while !reader.is_at_end() {
            cases.push(RawTree::decode(&mut reader)?);
        }

        Ok(MatchTypeNode {
            bound,
            selector,
            cases,
        })
    }

    pub fn decode_match_tpt(&self) -> Result<MatchTptNode<'a>, AstError> {
        if self.tag != MATCHTPT_TAG {
            return Err(AstError::UnexpectedTag {
                expected: MATCHTPT_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let mut prefix = Vec::new();
        while !reader.is_at_end() && reader.peek_u8()? != CASEDEF_TAG {
            prefix.push(RawTree::decode(&mut reader)?);
        }
        let (bound, selector) = match prefix.len() {
            1 => (None, prefix.pop().expect("prefix length was checked")),
            2 => {
                let selector = prefix.pop().expect("prefix length was checked");
                let bound = prefix.pop().expect("prefix length was checked");
                (Some(bound), selector)
            }
            _ => {
                return Err(AstError::UnsupportedCategory {
                    tag: self.tag,
                    offset: self.offset,
                });
            }
        };
        let cases = read_case_defs(&mut reader)?;
        if !reader.is_at_end() {
            return Err(AstError::UnsupportedCategory {
                tag: self.tag,
                offset: self.offset,
            });
        }

        Ok(MatchTptNode {
            bound,
            selector,
            cases,
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

    pub fn decode_annotation(&self) -> Result<AnnotationNode<'a>, AstError> {
        if self.tag != ANNOTATION_TAG {
            return Err(AstError::UnexpectedTag {
                expected: ANNOTATION_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let tycon = RawTree::decode(&mut reader)?;
        let full_annotation = RawTree::decode(&mut reader)?;
        if !reader.is_at_end() {
            return Err(AstError::UnsupportedCategory {
                tag: self.tag,
                offset: self.offset,
            });
        }

        Ok(AnnotationNode {
            tycon,
            full_annotation,
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
            binder: AstRef {
                kind: AstRefKind::ParamTypeBinder,
                address: binder,
            },
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
                type_or_bounds: AstRef {
                    kind: AstRefKind::TypeNameBounds,
                    address: reader.read_nat()?,
                },
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
                type_or_bounds: AstRef {
                    kind: AstRefKind::TypeNameBounds,
                    address: reader.read_nat()?,
                },
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

    /// Decodes a method type when the surrounding typed context provides the
    /// number of `TypeName` entries. The wire format does not delimit those
    /// entries from the trailing modifiers, so this is the unambiguous form
    /// for type-name values whose first byte happens to be a modifier tag.
    pub fn decode_method_type_with_type_name_count(
        &self,
        type_name_count: usize,
    ) -> Result<MethodTypeNode<'a>, AstError> {
        if self.tag != METHODTYPE_TAG {
            return Err(AstError::UnexpectedTag {
                expected: METHODTYPE_TAG,
                actual: self.tag,
                offset: self.offset,
            });
        }

        let mut reader = self.reader();
        let result_type = RawTree::decode(&mut reader)?;
        let mut type_names = Vec::with_capacity(type_name_count);
        for _ in 0..type_name_count {
            type_names.push(TypeName {
                type_or_bounds: AstRef {
                    kind: AstRefKind::TypeNameBounds,
                    address: reader.read_nat()?,
                },
                name: reader.read_nat()?,
            });
        }

        let mut modifiers = Vec::new();
        while !reader.is_at_end() {
            let offset = reader.position();
            let modifier = reader.read_u8()?;
            if !is_modifier_tag(modifier) {
                return Err(AstError::UnexpectedTag {
                    expected: INLINE_TAG,
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

        Ok(ReturnNode {
            target: AstRef {
                kind: AstRefKind::ReturnTarget,
                address: target,
            },
            expression,
        })
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
            TYPEPARAM_TAG => Ok(ParameterNode::TypeParam {
                name,
                body,
                offset: self.offset,
            }),
            PARAM_TAG => Ok(ParameterNode::TermParam {
                name,
                body,
                offset: self.offset,
            }),
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
        let remainder_offset = subslice_offset(self.payload, template.remainder).unwrap_or(0);
        let mut reader = Reader::with_range(self.payload, remainder_offset, self.payload.len())?;
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

impl<'a> PackageNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(PACKAGE_TAG, writer, |payload| {
            payload.write_u8(TERMREFPKG_TAG);
            payload.write_nat(self.path_name);
            self.stats.encode(payload).map_err(TermEncodeError::from)
        })
    }
}

impl<'a> ImportExportNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        let tag = match self.kind {
            ImportExportKind::Import => IMPORT_TAG,
            ImportExportKind::Export => EXPORT_TAG,
        };
        encode_length_node(tag, writer, |payload| {
            self.expr.encode(payload)?;
            for selector in &self.selectors {
                match selector {
                    ImportSelector::Imported { name } => {
                        payload.write_u8(IMPORTED_TAG);
                        payload.write_nat(*name);
                    }
                    ImportSelector::Renamed { name } => {
                        payload.write_u8(RENAMED_TAG);
                        payload.write_nat(*name);
                    }
                    ImportSelector::Bounded { type_tree } => {
                        type_tree.encode_ast_child(BOUNDED_TAG, payload)?;
                    }
                }
            }
            Ok(())
        })
    }
}

impl<'a> DefinitionNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        let tag = match self {
            Self::ValDef { .. } => VALDEF_TAG,
            Self::DefDef { .. } => DEFDEF_TAG,
            Self::TypeDef { .. } => TYPEDEF_TAG,
        };
        encode_length_node(tag, writer, |payload| {
            payload.write_nat(self.name());
            payload.write_bytes(self.body());
            Ok(())
        })
    }
}

impl<'a> TemplateNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(TEMPLATE_TAG, writer, |payload| {
            for parameter in &self.type_params {
                parameter.encode(payload)?;
            }
            for parameter in &self.term_params {
                parameter.encode(payload)?;
            }
            payload.write_bytes(self.remainder);
            Ok(())
        })
    }
}

impl<'a> DefinitionTail<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        match self {
            Self::Modifier(tag) => {
                if !is_modifier_tag(*tag) {
                    return Err(TermEncodeError::InvalidValue { tag: *tag });
                }
                writer.write_u8(*tag);
            }
            Self::Annotation(annotation) => {
                annotation.encode(writer).map_err(TermEncodeError::from)?
            }
        }
        Ok(())
    }
}

impl<'a> DefinitionBody<'a> {
    pub fn name(&self) -> u32 {
        match self {
            Self::ValDef { name, .. } | Self::TypeDef { name, .. } => *name,
        }
    }

    pub fn encode_self(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        self.encode(self.name(), writer)
    }

    pub fn encode(&self, name: u32, writer: &mut Writer) -> Result<(), TermEncodeError> {
        let tag = match self {
            Self::ValDef { .. } => VALDEF_TAG,
            Self::TypeDef { .. } => TYPEDEF_TAG,
        };
        encode_length_node(tag, writer, |payload| {
            payload.write_nat(name);
            match self {
                Self::ValDef {
                    type_tree,
                    rhs,
                    tail,
                    ..
                } => {
                    type_tree.encode(payload)?;
                    if let Some(rhs) = rhs {
                        rhs.encode(payload)?;
                    }
                    for entry in tail {
                        entry.encode(payload)?;
                    }
                }
                Self::TypeDef {
                    type_or_template,
                    tail,
                    ..
                } => {
                    type_or_template.encode(payload)?;
                    for entry in tail {
                        entry.encode(payload)?;
                    }
                }
            }
            Ok(())
        })
    }
}

impl<'a> DefDefBody<'a> {
    pub fn name(&self) -> u32 {
        self.name
    }

    pub fn encode_self(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        self.encode(self.name, writer)
    }

    pub fn encode(&self, name: u32, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(DEFDEF_TAG, writer, |payload| {
            payload.write_nat(name);
            if self.header_items.is_empty() {
                for parameter in &self.parameters {
                    parameter.encode(payload)?;
                }
                for clause in &self.clauses {
                    encode_defdef_clause(*clause, payload)?;
                }
            } else {
                for item in &self.header_items {
                    match item {
                        DefDefHeaderItem::Parameter(parameter) => parameter.encode(payload)?,
                        DefDefHeaderItem::Clause(clause) => encode_defdef_clause(*clause, payload)?,
                    }
                }
            }
            self.return_type.encode(payload)?;
            if let Some(rhs) = &self.rhs {
                rhs.encode(payload)?;
            }
            for entry in &self.tail {
                entry.encode(payload)?;
            }
            Ok(())
        })
    }
}

fn encode_defdef_clause(clause: u8, writer: &mut Writer) -> Result<(), TermEncodeError> {
    if !matches!(clause, EMPTYCLAUSE_TAG | SPLITCLAUSE_TAG) {
        return Err(TermEncodeError::InvalidValue { tag: clause });
    }
    writer.write_u8(clause);
    Ok(())
}

impl<'a> TemplateStructure<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(TEMPLATE_TAG, writer, |payload| {
            for parameter in &self.type_params {
                parameter.encode(payload)?;
            }
            for parameter in &self.term_params {
                parameter.encode(payload)?;
            }
            encode_trees(&self.parents, payload)?;
            if let Some(self_def) = &self.self_def {
                self_def.encode(payload)?;
            }
            if self.split_clause {
                payload.write_u8(SPLITCLAUSE_TAG);
            }
            self.stats.encode(payload).map_err(TermEncodeError::from)
        })
    }
}

impl<'a> ApplyNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(APPLY_TAG, writer, |payload| {
            self.function.encode(payload)?;
            encode_trees(&self.arguments, payload)
        })
    }
}

impl<'a> TypeApplyNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(TYPEAPPLY_TAG, writer, |payload| {
            self.function.encode(payload)?;
            encode_trees(&self.type_arguments, payload)
        })
    }
}

impl<'a> TypedNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(TYPED_TAG, writer, |payload| {
            self.expression.encode(payload)?;
            self.type_tree.encode(payload)
        })
    }
}

impl<'a> AssignNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(ASSIGN_TAG, writer, |payload| {
            self.left.encode(payload)?;
            self.right.encode(payload)
        })
    }
}

impl<'a> BlockNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(BLOCK_TAG, writer, |payload| {
            self.expression.encode(payload)?;
            encode_trees(&self.stats, payload)
        })
    }
}

impl<'a> IfNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(IF_TAG, writer, |payload| {
            if self.inline {
                payload.write_u8(INLINE_TAG);
            }
            self.condition.encode(payload)?;
            self.then_branch.encode(payload)?;
            self.else_branch.encode(payload)
        })
    }
}

impl<'a> LambdaNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(LAMBDA_TAG, writer, |payload| {
            self.method.encode(payload)?;
            if let Some(target_type) = &self.target_type {
                target_type.encode(payload)?;
            }
            Ok(())
        })
    }
}

impl<'a> SuperNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(SUPER_TAG, writer, |payload| {
            self.this_term.encode(payload)?;
            if let Some(mixin_type) = &self.mixin_type {
                mixin_type.encode(payload)?;
            }
            Ok(())
        })
    }
}

impl<'a> RepeatedNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(REPEATED_TAG, writer, |payload| {
            self.element_type.encode(payload)?;
            encode_trees(&self.elements, payload)
        })
    }
}

impl<'a> SelectOuterNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(SELECTOUTER_TAG, writer, |payload| {
            payload.write_nat(self.levels);
            self.qualifier.encode(payload)?;
            self.underlying_type.encode(payload)
        })
    }
}

impl<'a> ReturnNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(RETURN_TAG, writer, |payload| {
            payload.write_nat(self.target.address);
            if let Some(expression) = &self.expression {
                expression.encode(payload)?;
            }
            Ok(())
        })
    }
}

impl<'a> WhileNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(WHILE_TAG, writer, |payload| {
            self.condition.encode(payload)?;
            self.body.encode(payload)
        })
    }
}

impl<'a> CaseDefNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(CASEDEF_TAG, writer, |payload| {
            self.pattern.encode(payload)?;
            self.body.encode(payload)?;
            if let Some(guard) = &self.guard {
                guard.encode(payload)?;
            }
            Ok(())
        })
    }
}

impl<'a> MatchNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(MATCH_TAG, writer, |payload| {
            for modifier in &self.modifiers {
                if !matches!(*modifier, IMPLICIT_TAG | INLINE_TAG | SUBMATCH_TAG) {
                    return Err(TermEncodeError::InvalidValue { tag: *modifier });
                }
                payload.write_u8(*modifier);
            }
            self.scrutinee.encode(payload)?;
            for case in &self.cases {
                case.encode(payload)?;
            }
            Ok(())
        })
    }
}

impl<'a> TryNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(TRY_TAG, writer, |payload| {
            self.expression.encode(payload)?;
            for case in &self.cases {
                case.encode(payload)?;
            }
            if let Some(finalizer) = &self.finalizer {
                finalizer.encode(payload)?;
            }
            Ok(())
        })
    }
}

impl<'a> InlinedNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(INLINED_TAG, writer, |payload| {
            self.expression.encode(payload)?;
            if let Some(call_site) = &self.call_site {
                call_site.encode(payload)?;
            }
            self.definitions
                .encode(payload)
                .map_err(TermEncodeError::from)
        })
    }
}

impl<'a> QuoteNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        if !matches!(self.tag, QUOTE_TAG | SPLICE_TAG) {
            return Err(TermEncodeError::InvalidValue { tag: self.tag });
        }
        encode_length_node(self.tag, writer, |payload| {
            self.expression.encode(payload)?;
            self.type_tree.encode(payload)
        })
    }
}

impl<'a> ApplySigPolyNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(APPLYSIGPOLY_TAG, writer, |payload| {
            self.function.encode(payload)?;
            self.type_tree.encode(payload)?;
            encode_trees(&self.arguments, payload)
        })
    }
}

impl<'a> BinaryTypeNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        if !matches!(
            self.tag,
            ANDTYPE_TAG | ORTYPE_TAG | SUPERTYPE_TAG | MATCHCASETYPE_TAG
        ) {
            return Err(TermEncodeError::InvalidValue { tag: self.tag });
        }
        encode_length_node(self.tag, writer, |payload| {
            self.left.encode(payload)?;
            self.right.encode(payload)
        })
    }
}

impl<'a> AppliedTypeNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        if !matches!(self.tag, APPLIEDTYPE_TAG | APPLIEDTPT_TAG) {
            return Err(TermEncodeError::InvalidValue { tag: self.tag });
        }
        encode_length_node(self.tag, writer, |payload| {
            self.tycon.encode(payload)?;
            encode_trees(&self.arguments, payload)
        })
    }
}

impl<'a> FlexibleTypeNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(FLEXIBLETYPE_TAG, writer, |payload| {
            self.underlying_type.encode(payload)
        })
    }
}

impl<'a> TypeBoundsNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        if !matches!(self.tag, TYPEBOUNDS_TAG | TYPEBOUNDSTPT_TAG) {
            return Err(TermEncodeError::InvalidValue { tag: self.tag });
        }
        encode_length_node(self.tag, writer, |payload| {
            self.low_or_alias.encode(payload)?;
            if let Some(high) = &self.high {
                high.encode(payload)?;
            }
            for variance in &self.variances {
                if !matches!(*variance, 28 | 29) {
                    return Err(TermEncodeError::InvalidValue { tag: *variance });
                }
                payload.write_u8(*variance);
            }
            Ok(())
        })
    }
}

impl<'a> AnnotatedNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        if !matches!(self.tag, ANNOTATEDTYPE_TAG | ANNOTATEDTPT_TAG) {
            return Err(TermEncodeError::InvalidValue { tag: self.tag });
        }
        encode_length_node(self.tag, writer, |payload| {
            self.underlying.encode(payload)?;
            self.annotation.encode(payload)
        })
    }
}

impl<'a> AnnotationNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(ANNOTATION_TAG, writer, |payload| {
            self.tycon.encode(payload)?;
            self.full_annotation.encode(payload)
        })
    }
}

impl ParamTypeNode {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(PARAMTYPE_TAG, writer, |payload| {
            payload.write_nat(self.binder.address);
            payload.write_nat(self.parameter_number);
            Ok(())
        })
    }
}

impl<'a> PolyTypeNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        if !matches!(self.tag, POLYTYPE_TAG | TYPELAMBDATYPE_TAG) {
            return Err(TermEncodeError::InvalidValue { tag: self.tag });
        }
        encode_length_node(self.tag, writer, |payload| {
            self.result_type.encode(payload)?;
            for type_name in &self.type_names {
                payload.write_nat(type_name.type_or_bounds.address);
                payload.write_nat(type_name.name);
            }
            Ok(())
        })
    }
}

impl<'a> MethodTypeNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(METHODTYPE_TAG, writer, |payload| {
            self.result_type.encode(payload)?;
            for type_name in &self.type_names {
                payload.write_nat(type_name.type_or_bounds.address);
                payload.write_nat(type_name.name);
            }
            for modifier in &self.modifiers {
                if !is_modifier_tag(*modifier) {
                    return Err(TermEncodeError::InvalidValue { tag: *modifier });
                }
                payload.write_u8(*modifier);
            }
            Ok(())
        })
    }
}

impl<'a> RefinedTypeNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        if self.tag != REFINEDTYPE_TAG {
            return Err(TermEncodeError::InvalidValue { tag: self.tag });
        }
        encode_length_node(self.tag, writer, |payload| {
            payload.write_nat(self.name);
            self.parent.encode(payload)?;
            self.refinement.encode(payload)
        })
    }
}

impl<'a> RefinedTptNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(REFINEDTPT_TAG, writer, |payload| {
            self.qualifier.encode(payload)?;
            self.stats.encode(payload).map_err(TermEncodeError::from)
        })
    }
}

impl<'a> ParameterNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        let (tag, name) = match self {
            Self::TypeParam { name, .. } => (TYPEPARAM_TAG, *name),
            Self::TermParam { name, .. } => (PARAM_TAG, *name),
        };
        encode_length_node(tag, writer, |payload| {
            payload.write_nat(name);
            payload.write_bytes(self.body());
            Ok(())
        })
    }
}

impl<'a> LambdaTptNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(LAMBDATPT_TAG, writer, |payload| {
            for parameter in &self.type_params {
                parameter.encode(payload)?;
            }
            self.body.encode(payload)
        })
    }
}

impl<'a> BindNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(BIND_TAG, writer, |payload| {
            payload.write_nat(self.name);
            self.type_tree.encode(payload)?;
            match &self.body {
                BindBody::Pattern(pattern) => pattern.encode(payload)?,
                BindBody::Type { modifiers } => {
                    for modifier in modifiers {
                        payload.write_u8(*modifier);
                    }
                }
            }
            payload.write_bytes(self.remainder);
            Ok(())
        })
    }
}

impl<'a> HoleNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(HOLE_TAG, writer, |payload| {
            payload.write_nat(self.index);
            self.type_tree.encode(payload)?;
            encode_trees(&self.arguments, payload)
        })
    }
}

impl<'a> AlternativeNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(ALTERNATIVE_TAG, writer, |payload| {
            encode_trees(&self.alternatives, payload)
        })
    }
}

impl<'a> UnapplyNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(UNAPPLY_TAG, writer, |payload| {
            self.function.encode(payload)?;
            for implicit_arg in &self.implicit_args {
                implicit_arg.encode(payload)?;
            }
            self.type_tree.encode(payload)?;
            encode_trees(&self.patterns, payload)
        })
    }
}

impl<'a> QuotePatternNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(QUOTEPATTERN_TAG, writer, |payload| {
            self.body.encode(payload)?;
            self.quotes.encode(payload)?;
            self.pattern_type.encode(payload)?;
            encode_trees(&self.bindings, payload)
        })
    }
}

impl<'a> SplicePatternNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(SPLICEPATTERN_TAG, writer, |payload| {
            self.pattern.encode(payload)?;
            self.pattern_type.encode(payload)?;
            encode_trees(&self.arguments, payload)
        })
    }
}

impl<'a> MatchTypeNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(MATCHTYPE_TAG, writer, |payload| {
            self.bound.encode(payload)?;
            self.selector.encode(payload)?;
            encode_trees(&self.cases, payload)
        })
    }
}

impl<'a> MatchTptNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        encode_length_node(MATCHTPT_TAG, writer, |payload| {
            if let Some(bound) = &self.bound {
                bound.encode(payload)?;
            }
            self.selector.encode(payload)?;
            for case in &self.cases {
                case.encode(payload)?;
            }
            Ok(())
        })
    }
}

impl<'a> InReferenceNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        if !matches!(self.tag, TERMREFIN_TAG | TYPEREFIN_TAG) {
            return Err(TermEncodeError::InvalidValue { tag: self.tag });
        }
        encode_length_node(self.tag, writer, |payload| {
            payload.write_nat(self.name);
            self.qualifier.encode(payload)?;
            self.underlying_type.encode(payload)
        })
    }
}

impl<'a> SelectInNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        if self.tag != SELECTIN_TAG {
            return Err(TermEncodeError::InvalidValue { tag: self.tag });
        }
        encode_length_node(self.tag, writer, |payload| {
            payload.write_nat(self.name);
            self.qualifier.encode(payload)?;
            self.underlying_type.encode(payload)
        })
    }
}

fn encode_length_node<F>(
    tag: u8,
    writer: &mut Writer,
    encode_payload: F,
) -> Result<(), TermEncodeError>
where
    F: FnOnce(&mut Writer) -> Result<(), TermEncodeError>,
{
    let mut payload = Writer::new();
    encode_payload(&mut payload)?;
    writer.write_u8(tag);
    writer.write_length_prefixed_bytes(payload.as_slice())?;
    Ok(())
}

fn encode_trees<'a>(trees: &[RawTree<'a>], writer: &mut Writer) -> Result<(), TermEncodeError> {
    for tree in trees {
        tree.encode(writer)?;
    }
    Ok(())
}

impl<'a> RawTree<'a> {
    /// Dispatches this raw tree to its typed representation.
    ///
    /// Category-1 leaves that are not constants remain available as
    /// `StructuredTree::Leaf`, because they can be references or modifiers
    /// whose meaning is supplied by the enclosing grammar. Unknown or
    /// context-dependent details inside a category-5 payload are preserved
    /// by the existing `StructuredNode::Raw` variant.
    pub fn decode_structured(&self) -> Result<StructuredTree<'a>, AstError> {
        match self {
            Self::Leaf(term) => match term.constant_value()? {
                Some(value) => Ok(StructuredTree::Constant(value)),
                None => Ok(StructuredTree::Leaf(term.clone())),
            },
            Self::Ast { tag, .. } => {
                if *tag == CLASSCONST_TAG {
                    Ok(StructuredTree::ClassConstant(self.decode_class_constant()?))
                } else {
                    Ok(StructuredTree::AstChild(self.decode_ast_child(*tag)?))
                }
            }
            Self::NatAst { tag, .. } => match *tag {
                IDENT_TAG | IDENTTPT_TAG => Ok(StructuredTree::Ident(self.decode_ident()?)),
                SELECT_TAG | SELECTTPT_TAG => Ok(StructuredTree::Select(self.decode_select()?)),
                TERMREFSYMBOL_TAG | TERMREF_TAG | TYPEREFSYMBOL_TAG | TYPEREF_TAG => {
                    Ok(StructuredTree::Reference(self.decode_reference()?))
                }
                SELFDEF_TAG => Ok(StructuredTree::SelfDef(self.decode_self_def()?)),
                NAMEDARG_TAG => Ok(StructuredTree::NamedArg(self.decode_named_arg()?)),
                _ => Err(AstError::InvalidTag {
                    tag: *tag,
                    offset: raw_tree_tag_offset(self).1,
                }),
            },
            Self::LengthNode(node) => Ok(StructuredTree::Length(node.decode_structured()?)),
        }
    }

    pub fn encode_ast_child(&self, tag: u8, writer: &mut Writer) -> Result<(), TermEncodeError> {
        if !matches!(tag, 90..=104) {
            return Err(TermEncodeError::InvalidValue { tag });
        }
        writer.write_u8(tag);
        self.encode(writer)
    }

    pub fn decode_ast_child(&self, expected: u8) -> Result<AstChildNode<'a>, AstError> {
        match self {
            RawTree::Ast { tag, offset, child } if *tag == expected => Ok(AstChildNode {
                tag: *tag,
                child: (**child).clone(),
                offset: *offset,
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

    pub fn decode_class_constant(&self) -> Result<ClassConstNode<'a>, AstError> {
        Ok(ClassConstNode {
            type_tree: self.decode_class_const()?.child,
        })
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

impl<'a> AstChildNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        self.child.encode_ast_child(self.tag, writer)
    }
}

impl<'a> ClassConstNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        self.type_tree.encode_ast_child(CLASSCONST_TAG, writer)
    }
}

impl<'a> NamedArgNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        writer.write_u8(NAMEDARG_TAG);
        writer.write_nat(self.name);
        self.argument.encode(writer)
    }
}

impl<'a> IdentNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        if !matches!(self.tag, IDENT_TAG | IDENTTPT_TAG) {
            return Err(TermEncodeError::InvalidValue { tag: self.tag });
        }
        writer.write_u8(self.tag);
        writer.write_nat(self.name);
        self.type_tree.encode(writer)
    }
}

impl<'a> SelectNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        if !matches!(self.tag, SELECT_TAG | SELECTTPT_TAG) {
            return Err(TermEncodeError::InvalidValue { tag: self.tag });
        }
        writer.write_u8(self.tag);
        writer.write_nat(self.name);
        self.qualifier.encode(writer)
    }
}

impl<'a> ReferenceNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        if !matches!(
            self.tag,
            TERMREFSYMBOL_TAG | TERMREF_TAG | TYPEREFSYMBOL_TAG | TYPEREF_TAG
        ) {
            return Err(TermEncodeError::InvalidValue { tag: self.tag });
        }
        writer.write_u8(self.tag);
        writer.write_nat(self.reference);
        self.qualifier.encode(writer)
    }
}

impl<'a> SelfDefNode<'a> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), TermEncodeError> {
        writer.write_u8(SELFDEF_TAG);
        writer.write_nat(self.name);
        self.type_tree.encode(writer)
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

fn collect_tree_ast_refs(tree: &RawTree<'_>, visitor: &mut impl FnMut(AstRef)) {
    tree.visit_ast_refs(visitor);
}

fn collect_trees_ast_refs(trees: &[RawTree<'_>], visitor: &mut impl FnMut(AstRef)) {
    for tree in trees {
        collect_tree_ast_refs(tree, visitor);
    }
}

fn collect_raw_nodes_ast_refs(
    nodes: &RawNodes<'_>,
    visitor: &mut impl FnMut(AstRef),
) -> Result<(), AstError> {
    for node in &nodes.nodes {
        node.visit_ast_refs(visitor)?;
    }
    Ok(())
}

fn collect_definition_tail_ast_refs(
    tail: &[DefinitionTail<'_>],
    visitor: &mut impl FnMut(AstRef),
) -> Result<(), AstError> {
    for entry in tail {
        if let DefinitionTail::Annotation(node) = entry {
            node.visit_ast_refs(visitor)?;
        }
    }
    Ok(())
}

fn collect_definition_body_ast_refs(
    body: &DefinitionBody<'_>,
    visitor: &mut impl FnMut(AstRef),
) -> Result<(), AstError> {
    match body {
        DefinitionBody::ValDef {
            type_tree,
            rhs,
            tail,
            ..
        } => {
            collect_tree_ast_refs(type_tree, visitor);
            if let Some(rhs) = rhs {
                collect_tree_ast_refs(rhs, visitor);
            }
            collect_definition_tail_ast_refs(tail, visitor)?;
        }
        DefinitionBody::TypeDef {
            type_or_template,
            tail,
            ..
        } => {
            collect_tree_ast_refs(type_or_template, visitor);
            collect_definition_tail_ast_refs(tail, visitor)?;
        }
    }
    Ok(())
}

fn collect_parameter_ast_refs(
    parameter: &ParameterNode<'_>,
    visitor: &mut impl FnMut(AstRef),
) -> Result<(), AstError> {
    let body = parameter.decode_body()?;
    collect_tree_ast_refs(&body.type_tree, visitor);
    collect_definition_tail_ast_refs(&body.tail, visitor)
}

fn collect_parameters_ast_refs(
    parameters: &[ParameterNode<'_>],
    visitor: &mut impl FnMut(AstRef),
) -> Result<(), AstError> {
    for parameter in parameters {
        collect_parameter_ast_refs(parameter, visitor)?;
    }
    Ok(())
}

fn collect_case_def_ast_refs(case_def: &CaseDefNode<'_>, visitor: &mut impl FnMut(AstRef)) {
    collect_tree_ast_refs(&case_def.pattern, visitor);
    collect_tree_ast_refs(&case_def.body, visitor);
    if let Some(guard) = &case_def.guard {
        collect_tree_ast_refs(guard, visitor);
    }
}

fn collect_case_defs_ast_refs(case_defs: &[CaseDefNode<'_>], visitor: &mut impl FnMut(AstRef)) {
    for case_def in case_defs {
        collect_case_def_ast_refs(case_def, visitor);
    }
}

fn collect_structured_ast_refs(
    node: &StructuredNode<'_>,
    visitor: &mut impl FnMut(AstRef),
) -> Result<(), AstError> {
    match node {
        StructuredNode::Package(package) => collect_raw_nodes_ast_refs(&package.stats, visitor)?,
        StructuredNode::ValDef(body) | StructuredNode::TypeDef(body) => {
            collect_definition_body_ast_refs(body, visitor)?;
        }
        StructuredNode::DefDef(body) => {
            collect_parameters_ast_refs(&body.parameters, visitor)?;
            collect_tree_ast_refs(&body.return_type, visitor);
            if let Some(rhs) = &body.rhs {
                collect_tree_ast_refs(rhs, visitor);
            }
            collect_definition_tail_ast_refs(&body.tail, visitor)?;
        }
        StructuredNode::ImportExport(import_export) => {
            collect_tree_ast_refs(&import_export.expr, visitor);
            for selector in &import_export.selectors {
                if let ImportSelector::Bounded { type_tree } = selector {
                    collect_tree_ast_refs(type_tree, visitor);
                }
            }
        }
        StructuredNode::Parameter(parameter) => collect_parameter_ast_refs(parameter, visitor)?,
        StructuredNode::Apply(apply) => {
            collect_tree_ast_refs(&apply.function, visitor);
            collect_trees_ast_refs(&apply.arguments, visitor);
        }
        StructuredNode::TypeApply(type_apply) => {
            collect_tree_ast_refs(&type_apply.function, visitor);
            collect_trees_ast_refs(&type_apply.type_arguments, visitor);
        }
        StructuredNode::Typed(typed) => {
            collect_tree_ast_refs(&typed.expression, visitor);
            collect_tree_ast_refs(&typed.type_tree, visitor);
        }
        StructuredNode::Assign(assign) => {
            collect_tree_ast_refs(&assign.left, visitor);
            collect_tree_ast_refs(&assign.right, visitor);
        }
        StructuredNode::Block(block) => {
            collect_tree_ast_refs(&block.expression, visitor);
            collect_trees_ast_refs(&block.stats, visitor);
        }
        StructuredNode::If(if_node) => {
            collect_tree_ast_refs(&if_node.condition, visitor);
            collect_tree_ast_refs(&if_node.then_branch, visitor);
            collect_tree_ast_refs(&if_node.else_branch, visitor);
        }
        StructuredNode::Lambda(lambda) => {
            collect_tree_ast_refs(&lambda.method, visitor);
            if let Some(target_type) = &lambda.target_type {
                collect_tree_ast_refs(target_type, visitor);
            }
        }
        StructuredNode::Match(match_node) => {
            collect_tree_ast_refs(&match_node.scrutinee, visitor);
            collect_case_defs_ast_refs(&match_node.cases, visitor);
        }
        StructuredNode::Return(return_node) => {
            visitor(return_node.target);
            if let Some(expression) = &return_node.expression {
                collect_tree_ast_refs(expression, visitor);
            }
        }
        StructuredNode::While(while_node) => {
            collect_tree_ast_refs(&while_node.condition, visitor);
            collect_tree_ast_refs(&while_node.body, visitor);
        }
        StructuredNode::Try(try_node) => {
            collect_tree_ast_refs(&try_node.expression, visitor);
            collect_case_defs_ast_refs(&try_node.cases, visitor);
            if let Some(finalizer) = &try_node.finalizer {
                collect_tree_ast_refs(finalizer, visitor);
            }
        }
        StructuredNode::Inlined(inlined) => {
            collect_tree_ast_refs(&inlined.expression, visitor);
            if let Some(call_site) = &inlined.call_site {
                collect_tree_ast_refs(call_site, visitor);
            }
            collect_raw_nodes_ast_refs(&inlined.definitions, visitor)?;
        }
        StructuredNode::SelectOuter(select_outer) => {
            collect_tree_ast_refs(&select_outer.qualifier, visitor);
            collect_tree_ast_refs(&select_outer.underlying_type, visitor);
        }
        StructuredNode::Repeated(repeated) => {
            collect_tree_ast_refs(&repeated.element_type, visitor);
            collect_trees_ast_refs(&repeated.elements, visitor);
        }
        StructuredNode::Bind(bind) => {
            collect_tree_ast_refs(&bind.type_tree, visitor);
            if let BindBody::Pattern(pattern) = &bind.body {
                collect_tree_ast_refs(pattern, visitor);
            }
        }
        StructuredNode::Alternative(alternative) => {
            collect_trees_ast_refs(&alternative.alternatives, visitor);
        }
        StructuredNode::Unapply(unapply) => {
            collect_tree_ast_refs(&unapply.function, visitor);
            for implicit_arg in &unapply.implicit_args {
                collect_tree_ast_refs(&implicit_arg.child, visitor);
            }
            collect_tree_ast_refs(&unapply.type_tree, visitor);
            collect_trees_ast_refs(&unapply.patterns, visitor);
        }
        StructuredNode::Annotated(annotated) => {
            collect_tree_ast_refs(&annotated.underlying, visitor);
            collect_tree_ast_refs(&annotated.annotation, visitor);
        }
        StructuredNode::Annotation(annotation) => {
            collect_tree_ast_refs(&annotation.tycon, visitor);
            collect_tree_ast_refs(&annotation.full_annotation, visitor);
        }
        StructuredNode::CaseDef(case_def) => collect_case_def_ast_refs(case_def, visitor),
        StructuredNode::Template(template) => {
            collect_parameters_ast_refs(&template.type_params, visitor)?;
            collect_parameters_ast_refs(&template.term_params, visitor)?;
            collect_trees_ast_refs(&template.parents, visitor);
            if let Some(self_def) = &template.self_def {
                collect_tree_ast_refs(self_def, visitor);
            }
            collect_raw_nodes_ast_refs(&template.stats, visitor)?;
        }
        StructuredNode::Super(super_node) => {
            collect_tree_ast_refs(&super_node.this_term, visitor);
            if let Some(mixin_type) = &super_node.mixin_type {
                collect_tree_ast_refs(mixin_type, visitor);
            }
        }
        StructuredNode::BinaryType(binary) => {
            collect_tree_ast_refs(&binary.left, visitor);
            collect_tree_ast_refs(&binary.right, visitor);
        }
        StructuredNode::RefinedType(refined) => {
            collect_tree_ast_refs(&refined.parent, visitor);
            collect_tree_ast_refs(&refined.refinement, visitor);
        }
        StructuredNode::RefinedTpt(refined) => {
            collect_tree_ast_refs(&refined.qualifier, visitor);
            collect_raw_nodes_ast_refs(&refined.stats, visitor)?;
        }
        StructuredNode::AppliedType(applied) => {
            collect_tree_ast_refs(&applied.tycon, visitor);
            collect_trees_ast_refs(&applied.arguments, visitor);
        }
        StructuredNode::TypeBounds(bounds) => {
            collect_tree_ast_refs(&bounds.low_or_alias, visitor);
            if let Some(high) = &bounds.high {
                collect_tree_ast_refs(high, visitor);
            }
        }
        StructuredNode::FlexibleType(flexible) => {
            collect_tree_ast_refs(&flexible.underlying_type, visitor);
        }
        StructuredNode::LambdaTpt(lambda) => {
            collect_parameters_ast_refs(&lambda.type_params, visitor)?;
            collect_tree_ast_refs(&lambda.body, visitor);
        }
        StructuredNode::PolyType(poly) => {
            collect_tree_ast_refs(&poly.result_type, visitor);
            for type_name in &poly.type_names {
                visitor(type_name.type_or_bounds);
            }
        }
        StructuredNode::ParamType(param_type) => visitor(param_type.binder),
        StructuredNode::MethodType(method) => {
            collect_tree_ast_refs(&method.result_type, visitor);
            for type_name in &method.type_names {
                visitor(type_name.type_or_bounds);
            }
        }
        StructuredNode::ApplySigPoly(apply) => {
            collect_tree_ast_refs(&apply.function, visitor);
            collect_tree_ast_refs(&apply.type_tree, visitor);
            collect_trees_ast_refs(&apply.arguments, visitor);
        }
        StructuredNode::Quote(quote) => {
            collect_tree_ast_refs(&quote.expression, visitor);
            collect_tree_ast_refs(&quote.type_tree, visitor);
        }
        StructuredNode::QuotePattern(pattern) => {
            collect_tree_ast_refs(&pattern.body, visitor);
            collect_tree_ast_refs(&pattern.quotes, visitor);
            collect_tree_ast_refs(&pattern.pattern_type, visitor);
            collect_trees_ast_refs(&pattern.bindings, visitor);
        }
        StructuredNode::SplicePattern(pattern) => {
            collect_tree_ast_refs(&pattern.pattern, visitor);
            collect_tree_ast_refs(&pattern.pattern_type, visitor);
            collect_trees_ast_refs(&pattern.arguments, visitor);
        }
        StructuredNode::MatchType(match_type) => {
            collect_tree_ast_refs(&match_type.bound, visitor);
            collect_tree_ast_refs(&match_type.selector, visitor);
            collect_trees_ast_refs(&match_type.cases, visitor);
        }
        StructuredNode::MatchTpt(match_tpt) => {
            if let Some(bound) = &match_tpt.bound {
                collect_tree_ast_refs(bound, visitor);
            }
            collect_tree_ast_refs(&match_tpt.selector, visitor);
            collect_case_defs_ast_refs(&match_tpt.cases, visitor);
        }
        StructuredNode::Hole(hole) => {
            collect_tree_ast_refs(&hole.type_tree, visitor);
            collect_trees_ast_refs(&hole.arguments, visitor);
        }
        StructuredNode::InReference(reference) => {
            collect_tree_ast_refs(&reference.qualifier, visitor);
            collect_tree_ast_refs(&reference.underlying_type, visitor);
        }
        StructuredNode::SelectIn(select_in) => {
            collect_tree_ast_refs(&select_in.qualifier, visitor);
            collect_tree_ast_refs(&select_in.underlying_type, visitor);
        }
        StructuredNode::Raw(_) => {}
    }
    Ok(())
}

fn collect_raw_node_name_refs(
    node: &RawNode<'_>,
    visitor: &mut impl FnMut(NameRef),
) -> Result<(), AstError> {
    let structured = node.decode_structured()?;
    collect_structured_name_refs(&structured, visitor)
}

fn collect_raw_nodes_name_refs(
    nodes: &RawNodes<'_>,
    visitor: &mut impl FnMut(NameRef),
) -> Result<(), AstError> {
    for node in nodes.iter() {
        collect_raw_node_name_refs(node, visitor)?;
    }
    Ok(())
}

fn collect_definition_tail_name_refs(
    tail: &[DefinitionTail<'_>],
    visitor: &mut impl FnMut(NameRef),
) -> Result<(), AstError> {
    for entry in tail {
        if let DefinitionTail::Annotation(node) = entry {
            collect_raw_node_name_refs(node, visitor)?;
        }
    }
    Ok(())
}

fn collect_definition_body_name_refs(
    body: &DefinitionBody<'_>,
    visitor: &mut impl FnMut(NameRef),
) -> Result<(), AstError> {
    visitor(body.name());
    match body {
        DefinitionBody::ValDef {
            type_tree,
            rhs,
            tail,
            ..
        } => {
            type_tree.visit_name_refs(visitor);
            if let Some(rhs) = rhs {
                rhs.visit_name_refs(visitor);
            }
            collect_definition_tail_name_refs(tail, visitor)?;
        }
        DefinitionBody::TypeDef {
            type_or_template,
            tail,
            ..
        } => {
            type_or_template.visit_name_refs(visitor);
            collect_definition_tail_name_refs(tail, visitor)?;
        }
    }
    Ok(())
}

fn collect_parameter_name_refs(
    parameter: &ParameterNode<'_>,
    visitor: &mut impl FnMut(NameRef),
) -> Result<(), AstError> {
    visitor(parameter.name());
    let body = parameter.decode_body()?;
    body.type_tree.visit_name_refs(visitor);
    collect_definition_tail_name_refs(&body.tail, visitor)
}

fn collect_parameters_name_refs(
    parameters: &[ParameterNode<'_>],
    visitor: &mut impl FnMut(NameRef),
) -> Result<(), AstError> {
    for parameter in parameters {
        collect_parameter_name_refs(parameter, visitor)?;
    }
    Ok(())
}

fn collect_case_def_name_refs(case_def: &CaseDefNode<'_>, visitor: &mut impl FnMut(NameRef)) {
    case_def.pattern.visit_name_refs(visitor);
    case_def.body.visit_name_refs(visitor);
    if let Some(guard) = &case_def.guard {
        guard.visit_name_refs(visitor);
    }
}

fn collect_case_defs_name_refs(case_defs: &[CaseDefNode<'_>], visitor: &mut impl FnMut(NameRef)) {
    for case_def in case_defs {
        collect_case_def_name_refs(case_def, visitor);
    }
}

fn collect_structured_name_refs(
    node: &StructuredNode<'_>,
    visitor: &mut impl FnMut(NameRef),
) -> Result<(), AstError> {
    macro_rules! tree {
        ($tree:expr) => {
            $tree.visit_name_refs(visitor)
        };
    }
    macro_rules! trees {
        ($trees:expr) => {
            for tree in $trees {
                tree.visit_name_refs(visitor);
            }
        };
    }
    macro_rules! raw_nodes {
        ($nodes:expr) => {
            collect_raw_nodes_name_refs($nodes, visitor)?
        };
    }
    macro_rules! definition_body {
        ($body:expr) => {
            collect_definition_body_name_refs($body, visitor)?
        };
    }
    macro_rules! definition_tail {
        ($tail:expr) => {
            collect_definition_tail_name_refs($tail, visitor)?
        };
    }
    macro_rules! parameters {
        ($parameters:expr) => {
            collect_parameters_name_refs($parameters, visitor)?
        };
    }
    macro_rules! case_defs {
        ($case_defs:expr) => {
            collect_case_defs_name_refs($case_defs, visitor)
        };
    }

    match node {
        StructuredNode::Package(package) => {
            visitor(package.path_name);
            raw_nodes!(&package.stats);
        }
        StructuredNode::ValDef(body) | StructuredNode::TypeDef(body) => definition_body!(body),
        StructuredNode::DefDef(body) => {
            visitor(body.name());
            parameters!(&body.parameters);
            tree!(&body.return_type);
            if let Some(rhs) = &body.rhs {
                tree!(rhs);
            }
            definition_tail!(&body.tail);
        }
        StructuredNode::ImportExport(import_export) => {
            tree!(&import_export.expr);
            for selector in &import_export.selectors {
                match selector {
                    ImportSelector::Imported { name } | ImportSelector::Renamed { name } => {
                        visitor(*name)
                    }
                    ImportSelector::Bounded { type_tree } => tree!(type_tree),
                }
            }
        }
        StructuredNode::Parameter(parameter) => collect_parameter_name_refs(parameter, visitor)?,
        StructuredNode::Apply(apply) => {
            tree!(&apply.function);
            trees!(&apply.arguments);
        }
        StructuredNode::TypeApply(type_apply) => {
            tree!(&type_apply.function);
            trees!(&type_apply.type_arguments);
        }
        StructuredNode::Typed(typed) => {
            tree!(&typed.expression);
            tree!(&typed.type_tree);
        }
        StructuredNode::Assign(assign) => {
            tree!(&assign.left);
            tree!(&assign.right);
        }
        StructuredNode::Block(block) => {
            tree!(&block.expression);
            trees!(&block.stats);
        }
        StructuredNode::If(if_node) => {
            tree!(&if_node.condition);
            tree!(&if_node.then_branch);
            tree!(&if_node.else_branch);
        }
        StructuredNode::Lambda(lambda) => {
            tree!(&lambda.method);
            if let Some(target_type) = &lambda.target_type {
                tree!(target_type);
            }
        }
        StructuredNode::Match(match_node) => {
            tree!(&match_node.scrutinee);
            case_defs!(&match_node.cases);
        }
        StructuredNode::Return(return_node) => {
            if let Some(expression) = &return_node.expression {
                tree!(expression);
            }
        }
        StructuredNode::While(while_node) => {
            tree!(&while_node.condition);
            tree!(&while_node.body);
        }
        StructuredNode::Try(try_node) => {
            tree!(&try_node.expression);
            case_defs!(&try_node.cases);
            if let Some(finalizer) = &try_node.finalizer {
                tree!(finalizer);
            }
        }
        StructuredNode::Inlined(inlined) => {
            tree!(&inlined.expression);
            if let Some(call_site) = &inlined.call_site {
                tree!(call_site);
            }
            raw_nodes!(&inlined.definitions);
        }
        StructuredNode::SelectOuter(select_outer) => {
            tree!(&select_outer.qualifier);
            tree!(&select_outer.underlying_type);
        }
        StructuredNode::Repeated(repeated) => {
            tree!(&repeated.element_type);
            trees!(&repeated.elements);
        }
        StructuredNode::Bind(bind) => {
            visitor(bind.name);
            tree!(&bind.type_tree);
            if let BindBody::Pattern(pattern) = &bind.body {
                tree!(pattern);
            }
        }
        StructuredNode::Alternative(alternative) => trees!(&alternative.alternatives),
        StructuredNode::Unapply(unapply) => {
            tree!(&unapply.function);
            for implicit_arg in &unapply.implicit_args {
                tree!(&implicit_arg.child);
            }
            tree!(&unapply.type_tree);
            trees!(&unapply.patterns);
        }
        StructuredNode::Annotated(annotated) => {
            tree!(&annotated.underlying);
            tree!(&annotated.annotation);
        }
        StructuredNode::Annotation(annotation) => {
            tree!(&annotation.tycon);
            tree!(&annotation.full_annotation);
        }
        StructuredNode::CaseDef(case_def) => collect_case_def_name_refs(case_def, visitor),
        StructuredNode::Template(template) => {
            parameters!(&template.type_params);
            parameters!(&template.term_params);
            trees!(&template.parents);
            if let Some(self_def) = &template.self_def {
                tree!(self_def);
            }
            raw_nodes!(&template.stats);
        }
        StructuredNode::Super(super_node) => {
            tree!(&super_node.this_term);
            if let Some(mixin_type) = &super_node.mixin_type {
                tree!(mixin_type);
            }
        }
        StructuredNode::BinaryType(binary) => {
            tree!(&binary.left);
            tree!(&binary.right);
        }
        StructuredNode::RefinedType(refined) => {
            visitor(refined.name);
            tree!(&refined.parent);
            tree!(&refined.refinement);
        }
        StructuredNode::RefinedTpt(refined) => {
            tree!(&refined.qualifier);
            raw_nodes!(&refined.stats);
        }
        StructuredNode::AppliedType(applied) => {
            tree!(&applied.tycon);
            trees!(&applied.arguments);
        }
        StructuredNode::TypeBounds(bounds) => {
            tree!(&bounds.low_or_alias);
            if let Some(high) = &bounds.high {
                tree!(high);
            }
        }
        StructuredNode::FlexibleType(flexible) => tree!(&flexible.underlying_type),
        StructuredNode::LambdaTpt(lambda) => {
            parameters!(&lambda.type_params);
            tree!(&lambda.body);
        }
        StructuredNode::PolyType(poly) => {
            tree!(&poly.result_type);
            for type_name in &poly.type_names {
                visitor(type_name.name);
            }
        }
        StructuredNode::ParamType(_) => {}
        StructuredNode::MethodType(method) => {
            tree!(&method.result_type);
            for type_name in &method.type_names {
                visitor(type_name.name);
            }
        }
        StructuredNode::ApplySigPoly(apply) => {
            tree!(&apply.function);
            tree!(&apply.type_tree);
            trees!(&apply.arguments);
        }
        StructuredNode::Quote(quote) => {
            tree!(&quote.expression);
            tree!(&quote.type_tree);
        }
        StructuredNode::QuotePattern(pattern) => {
            tree!(&pattern.body);
            tree!(&pattern.quotes);
            tree!(&pattern.pattern_type);
            trees!(&pattern.bindings);
        }
        StructuredNode::SplicePattern(pattern) => {
            tree!(&pattern.pattern);
            tree!(&pattern.pattern_type);
            trees!(&pattern.arguments);
        }
        StructuredNode::MatchType(match_type) => {
            tree!(&match_type.bound);
            tree!(&match_type.selector);
            trees!(&match_type.cases);
        }
        StructuredNode::MatchTpt(match_tpt) => {
            if let Some(bound) = &match_tpt.bound {
                tree!(bound);
            }
            tree!(&match_tpt.selector);
            case_defs!(&match_tpt.cases);
        }
        StructuredNode::Hole(hole) => {
            tree!(&hole.type_tree);
            trees!(&hole.arguments);
        }
        StructuredNode::InReference(reference) => {
            visitor(reference.name);
            tree!(&reference.qualifier);
            tree!(&reference.underlying_type);
        }
        StructuredNode::SelectIn(select_in) => {
            visitor(select_in.name);
            tree!(&select_in.qualifier);
            tree!(&select_in.underlying_type);
        }
        StructuredNode::Raw(_) => {}
    }
    Ok(())
}

fn is_modifier_tag(tag: u8) -> bool {
    matches!(
        tag,
        PRIVATE_TAG
            | PROTECTED_TAG
            | ABSTRACT_TAG
            | FINAL_TAG
            | SEALED_TAG
            | CASE_TAG
            | IMPLICIT_TAG
            | LAZY_TAG
            | OVERRIDE_TAG
            | INLINEPROXY_TAG
            | INLINE_TAG
            | STATIC_TAG
            | OBJECT_TAG
            | TRAIT_TAG
            | ENUM_TAG
            | LOCAL_TAG
            | SYNTHETIC_TAG
            | ARTIFACT_TAG
            | MUTABLE_TAG
            | FIELDACCESSOR_TAG
            | CASEACCESSOR_TAG
            | COVARIANT_TAG
            | CONTRAVARIANT_TAG
            | HASDEFAULT_TAG
            | STABLE_TAG
            | MACRO_TAG
            | ERASED_TAG
            | OPAQUE_TAG
            | EXTENSION_TAG
            | GIVEN_TAG
            | PARAMSETTER_TAG
            | EXPORTED_TAG
            | OPEN_TAG
            | PARAMALIAS_TAG
            | TRANSPARENT_TAG
            | INFIX_TAG
            | INVISIBLE_TAG
            | TRACKED_TAG
            | INTO_TAG
    )
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

fn read_case_defs<'a>(reader: &mut Reader<'a>) -> Result<Vec<CaseDefNode<'a>>, AstError> {
    let mut cases = Vec::new();
    while !reader.is_at_end() && reader.peek_u8()? == CASEDEF_TAG {
        let tree = RawTree::decode(reader)?;
        let RawTree::LengthNode(raw) = tree else {
            unreachable!("case definition tags are category-five tags");
        };
        cases.push(raw.decode_case_def()?);
    }
    Ok(cases)
}

#[cfg(test)]
mod tests {
    use super::{
        ALTERNATIVE_TAG, ANDTYPE_TAG, ANNOTATEDTPT_TAG, ANNOTATEDTYPE_TAG, ANNOTATION_TAG,
        APPLIEDTPT_TAG, APPLIEDTYPE_TAG, APPLY_TAG, APPLYSIGPOLY_TAG, ASSIGN_TAG, AstError,
        AstTreeEdge, BIND_TAG, BLOCK_TAG, BOUNDED_TAG, BYNAMETPT_TAG, BYNAMETYPE_TAG, CASEDEF_TAG,
        CLASSCONST_TAG, DEFAULT_MAX_AST_INDEX_DEPTH, DEFDEF_TAG, DefDefBody, DefDefHeaderItem,
        DefinitionBody, DefinitionNode, DefinitionTail, ELIDED_TAG, EMPTYCLAUSE_TAG,
        EXPLICITTPT_TAG, EXPORT_TAG, FLEXIBLETYPE_TAG, HOLE_TAG, IDENT_TAG, IDENTTPT_TAG, IF_TAG,
        IMPLICIT_TAG, IMPLICITARG_TAG, IMPORT_TAG, IMPORTED_TAG, INLINE_TAG, INLINED_TAG,
        ImportExportKind, ImportSelector, LAMBDA_TAG, LAMBDATPT_TAG, MATCH_TAG, MATCHCASETYPE_TAG,
        MATCHTPT_TAG, MATCHTYPE_TAG, METHODTYPE_TAG, NAMEDARG_TAG, NEW_TAG, NodeCategory,
        ORTYPE_TAG, PACKAGE_TAG, PARAM_TAG, PARAMTYPE_TAG, POLYTYPE_TAG, PRIVATEQUALIFIED_TAG,
        PROTECTEDQUALIFIED_TAG, ParameterNode, QUALTHIS_TAG, QUOTE_TAG, QUOTEPATTERN_TAG,
        RECTYPE_TAG, REFINEDTPT_TAG, REFINEDTYPE_TAG, RENAMED_TAG, REPEATED_TAG, RETURN_TAG,
        RawNode, RawNodes, RawTree, SELECT_TAG, SELECTIN_TAG, SELECTOUTER_TAG, SELECTTPT_TAG,
        SELFDEF_TAG, SINGLETONTPT_TAG, SPLICE_TAG, SPLICEPATTERN_TAG, SPLITCLAUSE_TAG,
        SUBMATCH_TAG, SUPER_TAG, SUPERTYPE_TAG, StructuredNode, StructuredTree, TEMPLATE_TAG,
        TERMREF_TAG, TERMREFIN_TAG, TERMREFPKG_TAG, TERMREFSYMBOL_TAG, THIS_TAG, THROW_TAG,
        TRY_TAG, TYPEAPPLY_TAG, TYPEBOUNDS_TAG, TYPEBOUNDSTPT_TAG, TYPED_TAG, TYPEDEF_TAG,
        TYPELAMBDATYPE_TAG, TYPEPARAM_TAG, TYPEREF_TAG, TYPEREFIN_TAG, TYPEREFSYMBOL_TAG,
        TypeApplyNode, TypeName, TypedNode, UNAPPLY_TAG, VALDEF_TAG, WHILE_TAG,
    };
    use crate::reader::{ReadError, Reader};
    use crate::term::{AstTreeNode, TermEncodeError};
    use crate::writer::{WriteError, Writer};

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
    fn exposes_the_complete_category_one_tag_matrix() {
        use super::{
            ABSTRACT_TAG, ARTIFACT_TAG, CASE_TAG, CASEACCESSOR_TAG, CONTRAVARIANT_TAG,
            COVARIANT_TAG, EMPTYCLAUSE_TAG, ENUM_TAG, ERASED_TAG, EXPORTED_TAG, EXTENSION_TAG,
            FIELDACCESSOR_TAG, FINAL_TAG, GIVEN_TAG, HASDEFAULT_TAG, IMPLICIT_TAG, INFIX_TAG,
            INLINE_TAG, INLINEPROXY_TAG, INTO_TAG, INVISIBLE_TAG, LAZY_TAG, LOCAL_TAG, MACRO_TAG,
            MUTABLE_TAG, OBJECT_TAG, OPAQUE_TAG, OPEN_TAG, OVERRIDE_TAG, PARAMALIAS_TAG,
            PARAMSETTER_TAG, PRIVATE_TAG, PROTECTED_TAG, SEALED_TAG, SPLITCLAUSE_TAG, STABLE_TAG,
            STATIC_TAG, SUBMATCH_TAG, SYNTHETIC_TAG, TRACKED_TAG, TRAIT_TAG, TRANSPARENT_TAG,
        };

        assert_eq!(
            [
                PRIVATE_TAG,
                PROTECTED_TAG,
                ABSTRACT_TAG,
                FINAL_TAG,
                SEALED_TAG,
                CASE_TAG,
                IMPLICIT_TAG,
                LAZY_TAG,
                OVERRIDE_TAG,
                INLINEPROXY_TAG,
                INLINE_TAG,
                STATIC_TAG,
                OBJECT_TAG,
                TRAIT_TAG,
                ENUM_TAG,
                LOCAL_TAG,
                SYNTHETIC_TAG,
                ARTIFACT_TAG,
                MUTABLE_TAG,
                FIELDACCESSOR_TAG,
                CASEACCESSOR_TAG,
                COVARIANT_TAG,
                CONTRAVARIANT_TAG,
                HASDEFAULT_TAG,
                STABLE_TAG,
                MACRO_TAG,
                ERASED_TAG,
                OPAQUE_TAG,
                EXTENSION_TAG,
                GIVEN_TAG,
                PARAMSETTER_TAG,
                EXPORTED_TAG,
                OPEN_TAG,
                PARAMALIAS_TAG,
                TRANSPARENT_TAG,
                INFIX_TAG,
                INVISIBLE_TAG,
                EMPTYCLAUSE_TAG,
                SPLITCLAUSE_TAG,
                TRACKED_TAG,
                SUBMATCH_TAG,
                INTO_TAG,
            ],
            [
                6, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27,
                28, 29, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49,
            ]
        );
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
    fn constructs_raw_nodes_for_programmatic_encoding() {
        let nodes = RawNodes::from_entries(vec![RawNode {
            tag: VALDEF_TAG,
            offset: 99,
            payload: &[1, 2],
        }])
        .unwrap();
        let mut writer = Writer::new();

        nodes.encode(&mut writer).unwrap();

        assert_eq!(writer.as_slice(), &[VALDEF_TAG, 0x82, 1, 2]);
        assert_eq!(nodes.encode_with_addresses().unwrap().addresses(), &[0]);
    }

    #[test]
    fn constructs_a_raw_node_without_an_explicit_offset() {
        let node = RawNode::new(VALDEF_TAG, &[1, 2]).unwrap();
        let mut writer = Writer::new();

        node.encode(&mut writer).unwrap();

        assert_eq!(node.offset, 0);
        assert_eq!(writer.as_slice(), &[VALDEF_TAG, 0x82, 1, 2]);
    }

    #[test]
    fn rejects_a_non_category_five_tag_when_constructing_a_raw_node() {
        assert_eq!(
            RawNode::new(SELECT_TAG, &[]),
            Err(AstError::UnsupportedCategory {
                tag: SELECT_TAG,
                offset: 0,
            })
        );
    }

    #[test]
    fn rejects_an_invalid_tag_when_constructing_a_raw_node() {
        assert_eq!(
            RawNode::new(0, &[]),
            Err(AstError::InvalidTag { tag: 0, offset: 0 })
        );
    }

    #[test]
    fn rejects_non_category_five_entries_when_constructing_raw_nodes() {
        assert_eq!(
            RawNodes::from_entries(vec![RawNode {
                tag: SELECT_TAG,
                offset: 7,
                payload: &[],
            }]),
            Err(AstError::UnsupportedCategory {
                tag: SELECT_TAG,
                offset: 7,
            })
        );
    }

    #[test]
    fn rejects_an_invalid_tag_when_constructing_raw_nodes() {
        assert_eq!(
            RawNodes::from_entries(vec![RawNode {
                tag: 0,
                offset: 3,
                payload: &[],
            }]),
            Err(AstError::InvalidTag { tag: 0, offset: 3 })
        );
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
    fn preserves_an_unassigned_category_five_top_level_tag() {
        let mut reader = Reader::new(&[135, 0x80]);

        let nodes = RawNodes::decode(&mut reader).unwrap();
        assert_eq!(nodes.get(0).unwrap().tag, 135);
        assert!(nodes.get(0).unwrap().payload.is_empty());
        assert!(reader.is_at_end());
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

        assert_eq!(body.name(), 1);
        assert!(matches!(
            body,
            DefinitionBody::ValDef {
                type_tree: _,
                rhs: Some(_),
                ref tail,
                ..
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
            29, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 47, 49,
        ];

        for tag in assigned {
            assert!(
                super::is_modifier_tag(tag),
                "modifier tag {tag} was rejected"
            );
        }
        for tag in [0, 1, 2, 5, 7, 30, 45, 46, 48, 50, 255] {
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
            29, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 47, 49,
        ];

        for modifier in assigned {
            let bytes = [VALDEF_TAG, 0x83, 0x81, 2, modifier];
            let mut reader = Reader::new(&bytes);
            let nodes = RawNodes::decode(&mut reader).unwrap();
            let body = nodes.get(0).unwrap().decode_definition_body().unwrap();

            assert_eq!(
                body,
                DefinitionBody::ValDef {
                    name: 1,
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
    fn does_not_decode_clause_markers_as_definition_modifiers() {
        for marker in [EMPTYCLAUSE_TAG, SPLITCLAUSE_TAG, SUBMATCH_TAG] {
            let bytes = [VALDEF_TAG, 0x83, 0x81, 2, marker];
            let mut reader = Reader::new(&bytes);
            let nodes = RawNodes::decode(&mut reader).unwrap();

            assert!(
                nodes.get(0).unwrap().decode_definition_body().is_err(),
                "category-one marker {marker} was accepted as a definition modifier"
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
                ..
            }) if tail == vec![DefinitionTail::Modifier(17)]
        ));
        assert_eq!(
            nodes
                .get(0)
                .unwrap()
                .decode_definition_body()
                .unwrap()
                .name(),
            1
        );
    }

    #[test]
    fn round_trips_a_valdef_body_with_its_decoded_name() {
        let bytes = [VALDEF_TAG, 0x82, 0x81, 2];
        let mut reader = Reader::new(&bytes);
        let body = RawNodes::decode(&mut reader)
            .unwrap()
            .get(0)
            .unwrap()
            .decode_definition_body()
            .unwrap();
        let mut writer = Writer::new();

        body.encode_self(&mut writer).unwrap();
        assert_eq!(writer.as_slice(), bytes);
    }

    #[test]
    fn round_trips_a_defdef_body_with_its_decoded_name() {
        let bytes = [DEFDEF_TAG, 0x82, 0x81, 2];
        let mut reader = Reader::new(&bytes);
        let body = RawNodes::decode(&mut reader)
            .unwrap()
            .get(0)
            .unwrap()
            .decode_defdef_body()
            .unwrap();
        let mut writer = Writer::new();

        body.encode_self(&mut writer).unwrap();
        assert_eq!(writer.as_slice(), bytes);
    }

    #[test]
    fn round_trips_a_typedef_body_with_its_decoded_name() {
        let bytes = [TYPEDEF_TAG, 0x82, 0x81, 2];
        let mut reader = Reader::new(&bytes);
        let body = RawNodes::decode(&mut reader)
            .unwrap()
            .get(0)
            .unwrap()
            .decode_definition_body()
            .unwrap();
        let mut writer = Writer::new();

        body.encode_self(&mut writer).unwrap();
        assert_eq!(writer.as_slice(), bytes);
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

    fn decode_structured_tree(bytes: &[u8]) -> StructuredTree<'_> {
        let mut reader = Reader::new(bytes);
        let tree = RawTree::decode(&mut reader).unwrap();
        let structured = tree.decode_structured().unwrap();
        assert!(reader.is_at_end());
        structured
    }

    #[test]
    fn dispatches_a_typed_constant_tree() {
        assert!(matches!(
            decode_structured_tree(&[70, 0xaa]),
            StructuredTree::Constant(crate::term::ConstantValue::Int(42))
        ));
    }

    #[test]
    fn dispatches_a_non_constant_leaf_tree() {
        assert!(matches!(
            decode_structured_tree(&[TERMREFPKG_TAG, 0x81]),
            StructuredTree::Leaf(term)
                if term.tag == TERMREFPKG_TAG
                    && term.value == crate::term::TermValue::NameRef(1)
        ));
    }

    #[test]
    fn dispatches_a_category_three_tree() {
        assert!(matches!(
            decode_structured_tree(&[THIS_TAG, TERMREFPKG_TAG, 0x81]),
            StructuredTree::AstChild(node) if node.tag == THIS_TAG
        ));
    }

    #[test]
    fn dispatches_a_class_constant_tree() {
        assert!(matches!(
            decode_structured_tree(&[CLASSCONST_TAG, TERMREFPKG_TAG, 0x81]),
            StructuredTree::ClassConstant(node)
                if matches!(node.type_tree, RawTree::Leaf(_))
        ));
    }

    #[test]
    fn dispatches_an_identifier_tree() {
        assert!(matches!(
            decode_structured_tree(&[IDENT_TAG, 0x81, 2]),
            StructuredTree::Ident(node) if node.name == 1
        ));
    }

    #[test]
    fn dispatches_a_select_tree() {
        assert!(matches!(
            decode_structured_tree(&[SELECT_TAG, 0x81, 2]),
            StructuredTree::Select(node) if node.name == 1
        ));
    }

    #[test]
    fn dispatches_a_reference_tree() {
        assert!(matches!(
            decode_structured_tree(&[TERMREF_TAG, 0x81, 2]),
            StructuredTree::Reference(node) if node.reference == 1
        ));
    }

    #[test]
    fn dispatches_a_self_definition_tree() {
        assert!(matches!(
            decode_structured_tree(&[SELFDEF_TAG, 0x81, 2]),
            StructuredTree::SelfDef(node) if node.name == 1
        ));
    }

    #[test]
    fn dispatches_a_named_argument_tree() {
        assert!(matches!(
            decode_structured_tree(&[NAMEDARG_TAG, 0x81, 2]),
            StructuredTree::NamedArg(node) if node.name == 1
        ));
    }

    #[test]
    fn dispatches_a_bounded_structured_tree() {
        assert!(matches!(
            decode_structured_tree(&[APPLY_TAG, 0x81, 2]),
            StructuredTree::Length(StructuredNode::Apply(_))
        ));
    }

    #[test]
    fn encodes_a_typed_constant_structured_tree() {
        assert_structured_tree_round_trip(&[70, 0xaa]);
    }

    #[test]
    fn encodes_a_non_constant_leaf_structured_tree() {
        assert_structured_tree_round_trip(&[TERMREFPKG_TAG, 0x85]);
    }

    #[test]
    fn encodes_a_category_three_structured_tree() {
        assert_structured_tree_round_trip(&[THIS_TAG, TERMREFPKG_TAG, 0x81]);
    }

    #[test]
    fn encodes_a_class_constant_structured_tree() {
        assert_structured_tree_round_trip(&[CLASSCONST_TAG, TERMREFPKG_TAG, 0x81]);
    }

    #[test]
    fn encodes_an_identifier_structured_tree() {
        assert_structured_tree_round_trip(&[IDENT_TAG, 0x81, 2]);
    }

    #[test]
    fn encodes_a_select_structured_tree() {
        assert_structured_tree_round_trip(&[SELECT_TAG, 0x81, 2]);
    }

    #[test]
    fn encodes_a_reference_structured_tree() {
        assert_structured_tree_round_trip(&[TERMREF_TAG, 0x81, 2]);
    }

    #[test]
    fn encodes_a_self_definition_structured_tree() {
        assert_structured_tree_round_trip(&[SELFDEF_TAG, 0x81, 2]);
    }

    #[test]
    fn encodes_a_named_argument_structured_tree() {
        assert_structured_tree_round_trip(&[NAMEDARG_TAG, 0x81, 2]);
    }

    #[test]
    fn encodes_a_bounded_structured_tree() {
        assert_structured_tree_round_trip(&[APPLY_TAG, 0x83, 2, 3, 4]);
    }

    fn assert_structured_tree_round_trip(bytes: &[u8]) {
        let structured = decode_structured_tree(bytes);
        let mut writer = Writer::new();
        structured.encode(&mut writer).unwrap();
        assert_eq!(writer.as_slice(), bytes);
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

        assert_eq!(body.name(), 1);
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
        assert!(matches!(
            body.header_items.as_slice(),
            [
                DefDefHeaderItem::Parameter(ParameterNode::TypeParam { .. }),
                DefDefHeaderItem::Parameter(ParameterNode::TermParam { .. }),
                DefDefHeaderItem::Clause(SPLITCLAUSE_TAG),
            ]
        ));
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

        assert_eq!(node.binder.address, 5);
        assert_eq!(node.binder.kind, crate::term::AstRefKind::ParamTypeBinder);
        assert_eq!(node.parameter_number, 3);
    }

    #[test]
    fn collects_the_param_type_binder_as_an_ast_reference() {
        let node = RawNode {
            tag: PARAMTYPE_TAG,
            offset: 0,
            payload: &[0x85, 0x83],
        };

        assert_eq!(
            node.ast_refs().unwrap(),
            vec![crate::term::AstRef {
                kind: crate::term::AstRefKind::ParamTypeBinder,
                address: 5,
            }]
        );
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
                        type_or_bounds: crate::term::AstRef {
                            kind: crate::term::AstRefKind::TypeNameBounds,
                            address: 5,
                        },
                        name: 6,
                    },
                    super::TypeName {
                        type_or_bounds: crate::term::AstRef {
                            kind: crate::term::AstRefKind::TypeNameBounds,
                            address: 7,
                        },
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
    fn collects_poly_type_name_bounds_as_ast_references() {
        let node = RawNode {
            tag: POLYTYPE_TAG,
            offset: 0,
            payload: &[2, 0x85, 0x86, 0x87, 0x88],
        };

        assert_eq!(
            node.ast_refs().unwrap(),
            vec![
                crate::term::AstRef {
                    kind: crate::term::AstRefKind::TypeNameBounds,
                    address: 5,
                },
                crate::term::AstRef {
                    kind: crate::term::AstRefKind::TypeNameBounds,
                    address: 7,
                },
            ]
        );
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
    fn collects_method_type_name_bounds_as_ast_references() {
        let node = RawNode {
            tag: METHODTYPE_TAG,
            offset: 0,
            payload: &[2, 0x85, 0x86],
        };

        assert_eq!(
            node.ast_refs().unwrap(),
            vec![crate::term::AstRef {
                kind: crate::term::AstRefKind::TypeNameBounds,
                address: 5,
            }]
        );
    }

    #[test]
    fn decodes_a_method_type_with_an_ambiguous_type_name_using_context() {
        let node = RawNode {
            tag: METHODTYPE_TAG,
            offset: 0,
            payload: &[2, 0x91, 0x85, 37],
        };

        let node = node.decode_method_type_with_type_name_count(1).unwrap();

        assert_eq!(
            node.type_names,
            vec![TypeName {
                type_or_bounds: crate::term::AstRef {
                    kind: crate::term::AstRefKind::TypeNameBounds,
                    address: INLINE_TAG as u32,
                },
                name: 5,
            }]
        );
        assert_eq!(node.modifiers, vec![37]);
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

        assert_eq!(return_node.target.address, 5);
        assert_eq!(
            return_node.target.kind,
            crate::term::AstRefKind::ReturnTarget
        );
        assert!(matches!(return_node.expression, Some(RawTree::Leaf(_))));
    }

    #[test]
    fn decodes_return_without_an_expression() {
        let bytes = [RETURN_TAG, 0x81, 0x85];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let return_node = nodes.get(0).unwrap().decode_return().unwrap();

        assert_eq!(return_node.target.address, 5);
        assert_eq!(
            return_node.target.kind,
            crate::term::AstRefKind::ReturnTarget
        );
        assert!(return_node.expression.is_none());
    }

    #[test]
    fn collects_the_return_target_as_an_ast_reference() {
        let node = RawNode {
            tag: RETURN_TAG,
            offset: 0,
            payload: &[0x85],
        };

        assert_eq!(
            node.ast_refs().unwrap(),
            vec![crate::term::AstRef {
                kind: crate::term::AstRefKind::ReturnTarget,
                address: 5,
            }]
        );
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
        assert_eq!(case_def.offset, 0);
        assert_eq!(case_def.payload, &bytes[2..]);
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
        assert!(matches!(
            bind.body,
            super::BindBody::Pattern(RawTree::Leaf(_))
        ));
        assert!(bind.remainder.is_empty());
        assert_eq!(alternative.alternatives.len(), 2);
    }

    #[test]
    fn decodes_bind_type_form_with_modifiers() {
        let bytes = [
            BIND_TAG,
            0x85,
            0x85,
            TERMREFPKG_TAG,
            0x81,
            INLINE_TAG,
            IMPLICIT_TAG,
        ];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let bind = nodes.get(0).unwrap().decode_bind().unwrap();

        assert!(matches!(
            bind.body,
            super::BindBody::Type { modifiers } if modifiers == vec![INLINE_TAG, IMPLICIT_TAG]
        ));
        assert!(bind.remainder.is_empty());
    }

    #[test]
    fn preserves_an_ambiguous_bind_remainder_for_round_trip() {
        let bytes = [BIND_TAG, 0x85, 0x85, 2, 3, super::SHAREDTYPE_TAG, 0x81];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let bind = nodes.get(0).unwrap().decode_bind().unwrap();

        assert!(matches!(
            bind.body,
            super::BindBody::Pattern(RawTree::Leaf(_))
        ));
        assert_eq!(bind.remainder, &[super::SHAREDTYPE_TAG, 0x81]);

        let mut writer = Writer::new();
        bind.encode(&mut writer).unwrap();
        assert_eq!(writer.as_slice(), bytes);
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
        assert_eq!(unapply.implicit_args[0].offset, 2);
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
    fn decodes_a_refined_type_with_name_parent_and_refinement() {
        let bytes = [
            REFINEDTYPE_TAG,
            0x85,
            0x85,
            TERMREFPKG_TAG,
            0x81,
            TERMREFPKG_TAG,
            0x82,
        ];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let refined = nodes.get(0).unwrap().decode_refined_type().unwrap();

        assert_eq!(refined.tag, REFINEDTYPE_TAG);
        assert_eq!(refined.name, 5);
        assert!(matches!(refined.parent, RawTree::Leaf(_)));
        assert!(matches!(refined.refinement, RawTree::Leaf(_)));
    }

    #[test]
    fn decodes_a_refined_tpt_with_qualifier_and_stats() {
        let bytes = [
            REFINEDTPT_TAG,
            0x84,
            TERMREFPKG_TAG,
            0x81,
            PACKAGE_TAG,
            0x80,
        ];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let refined = nodes.get(0).unwrap().decode_refined_tpt().unwrap();

        assert!(matches!(refined.qualifier, RawTree::Leaf(_)));
        assert_eq!(refined.stats.len(), 1);
    }

    #[test]
    fn decodes_a_lambda_tpt_with_type_parameters_and_body() {
        let bytes = [
            LAMBDATPT_TAG,
            0x87,
            TYPEPARAM_TAG,
            0x83,
            0x85,
            TERMREFPKG_TAG,
            0x81,
            TERMREFPKG_TAG,
            0x82,
        ];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let lambda = nodes.get(0).unwrap().decode_lambda_tpt().unwrap();

        assert_eq!(lambda.type_params.len(), 1);
        assert!(matches!(
            lambda.type_params[0],
            ParameterNode::TypeParam { name: 5, .. }
        ));
        assert!(matches!(lambda.body, RawTree::Leaf(_)));
    }

    #[test]
    fn decodes_an_inlined_expression_with_an_optional_call_site_and_definitions() {
        let bytes = [
            INLINED_TAG,
            0x89,
            TERMREFPKG_TAG,
            0x81,
            TERMREFPKG_TAG,
            0x82,
            VALDEF_TAG,
            0x83,
            0x85,
            TERMREFPKG_TAG,
            0x83,
        ];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let inlined = nodes.get(0).unwrap().decode_inlined().unwrap();

        assert!(matches!(inlined.expression, RawTree::Leaf(_)));
        assert!(matches!(inlined.call_site, Some(RawTree::Leaf(_))));
        assert_eq!(inlined.definitions.len(), 1);
        assert_eq!(inlined.definitions.get(0).unwrap().tag, VALDEF_TAG);
    }

    #[test]
    fn decodes_match_with_a_modifier_and_case_definitions() {
        for modifier in [IMPLICIT_TAG, INLINE_TAG, SUBMATCH_TAG] {
            let bytes = [
                MATCH_TAG,
                0x89,
                modifier,
                TERMREFPKG_TAG,
                0x81,
                CASEDEF_TAG,
                0x84,
                TERMREFPKG_TAG,
                0x82,
                TERMREFPKG_TAG,
                0x83,
            ];
            let mut reader = Reader::new(&bytes);
            let nodes = RawNodes::decode(&mut reader).unwrap();
            let matched = nodes.get(0).unwrap().decode_match().unwrap();

            assert_eq!(matched.modifiers, vec![modifier]);
            assert!(matches!(matched.scrutinee, RawTree::Leaf(_)));
            assert_eq!(matched.cases.len(), 1);
            assert!(matched.cases[0].guard.is_none());
        }
    }

    #[test]
    fn decodes_try_with_case_definitions_and_an_optional_finalizer() {
        let bytes = [
            TRY_TAG,
            0x8a,
            TERMREFPKG_TAG,
            0x81,
            CASEDEF_TAG,
            0x84,
            TERMREFPKG_TAG,
            0x82,
            TERMREFPKG_TAG,
            0x83,
            TERMREFPKG_TAG,
            0x84,
        ];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let tried = nodes.get(0).unwrap().decode_try().unwrap();

        assert!(matches!(tried.expression, RawTree::Leaf(_)));
        assert_eq!(tried.cases.len(), 1);
        assert!(matches!(tried.finalizer, Some(RawTree::Leaf(_))));
    }

    #[test]
    fn decodes_quote_and_splice_with_expression_and_type() {
        for tag in [QUOTE_TAG, SPLICE_TAG] {
            let bytes = [tag, 0x84, TERMREFPKG_TAG, 0x81, TERMREFPKG_TAG, 0x82];
            let mut reader = Reader::new(&bytes);
            let nodes = RawNodes::decode(&mut reader).unwrap();
            let raw = nodes.get(0).unwrap();
            let quoted = if tag == QUOTE_TAG {
                raw.decode_quote().unwrap()
            } else {
                raw.decode_splice().unwrap()
            };

            assert_eq!(quoted.tag, tag);
            assert!(matches!(quoted.expression, RawTree::Leaf(_)));
            assert!(matches!(quoted.type_tree, RawTree::Leaf(_)));
        }
    }

    #[test]
    fn decodes_apply_sigpoly_with_type_and_arguments() {
        let bytes = [
            APPLYSIGPOLY_TAG,
            0x88,
            TERMREFPKG_TAG,
            0x81,
            TERMREFPKG_TAG,
            0x82,
            TERMREFPKG_TAG,
            0x83,
            TERMREFPKG_TAG,
            0x84,
        ];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let apply = nodes.get(0).unwrap().decode_apply_sigpoly().unwrap();

        assert!(matches!(apply.function, RawTree::Leaf(_)));
        assert!(matches!(apply.type_tree, RawTree::Leaf(_)));
        assert_eq!(apply.arguments.len(), 2);
    }

    #[test]
    fn decodes_quote_pattern_with_body_quotes_type_and_bindings() {
        let bytes = [
            QUOTEPATTERN_TAG,
            0x88,
            TERMREFPKG_TAG,
            0x81,
            TERMREFPKG_TAG,
            0x82,
            TERMREFPKG_TAG,
            0x83,
            TERMREFPKG_TAG,
            0x84,
        ];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let quote = nodes.get(0).unwrap().decode_quote_pattern().unwrap();

        assert!(matches!(quote.body, RawTree::Leaf(_)));
        assert!(matches!(quote.quotes, RawTree::Leaf(_)));
        assert!(matches!(quote.pattern_type, RawTree::Leaf(_)));
        assert_eq!(quote.bindings.len(), 1);
    }

    #[test]
    fn decodes_splice_pattern_and_preserves_its_ordered_argument_tail() {
        let bytes = [
            SPLICEPATTERN_TAG,
            0x8a,
            TERMREFPKG_TAG,
            0x81,
            TERMREFPKG_TAG,
            0x82,
            TERMREFPKG_TAG,
            0x83,
            TERMREFPKG_TAG,
            0x84,
            TERMREFPKG_TAG,
            0x85,
        ];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let splice = nodes.get(0).unwrap().decode_splice_pattern().unwrap();

        assert!(matches!(splice.pattern, RawTree::Leaf(_)));
        assert!(matches!(splice.pattern_type, RawTree::Leaf(_)));
        assert_eq!(splice.arguments.len(), 3);

        let (type_arguments, term_arguments) = splice.split_arguments(1).unwrap();
        assert_eq!(type_arguments.len(), 1);
        assert_eq!(term_arguments.len(), 2);
        assert!(splice.split_arguments(4).is_none());
    }

    #[test]
    fn decodes_a_hole_with_index_type_and_arguments() {
        let bytes = [HOLE_TAG, 0x84, 0x85, 2, 3, 4];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let hole = nodes.get(0).unwrap().decode_hole().unwrap();

        assert_eq!(hole.index, 5);
        assert!(matches!(hole.type_tree, RawTree::Leaf(_)));
        assert_eq!(hole.arguments.len(), 2);
    }

    #[test]
    fn rejects_a_hole_without_a_type_tree() {
        let node = RawNode {
            tag: HOLE_TAG,
            offset: 0,
            payload: &[0x85],
        };

        assert!(node.decode_hole().is_err());
    }

    #[test]
    fn decodes_match_type_with_raw_case_type_trees() {
        let bytes = [
            MATCHTYPE_TAG,
            0x8c,
            TERMREFPKG_TAG,
            0x81,
            TERMREFPKG_TAG,
            0x82,
            MATCHCASETYPE_TAG,
            0x82,
            TERMREFPKG_TAG,
            0x83,
            MATCHCASETYPE_TAG,
            0x82,
            TERMREFPKG_TAG,
            0x84,
        ];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let matched = nodes.get(0).unwrap().decode_match_type().unwrap();

        assert!(matches!(matched.bound, RawTree::Leaf(_)));
        assert!(matches!(matched.selector, RawTree::Leaf(_)));
        assert_eq!(matched.cases.len(), 2);
        assert!(matches!(matched.cases[0], RawTree::LengthNode(_)));
    }

    #[test]
    fn decodes_match_tpt_with_optional_bound() {
        for (payload, has_bound) in [
            (
                vec![
                    TERMREFPKG_TAG,
                    0x81,
                    CASEDEF_TAG,
                    0x84,
                    TERMREFPKG_TAG,
                    0x82,
                    TERMREFPKG_TAG,
                    0x83,
                ],
                false,
            ),
            (
                vec![
                    TERMREFPKG_TAG,
                    0x81,
                    TERMREFPKG_TAG,
                    0x82,
                    CASEDEF_TAG,
                    0x84,
                    TERMREFPKG_TAG,
                    0x83,
                    TERMREFPKG_TAG,
                    0x84,
                ],
                true,
            ),
        ] {
            let mut bytes = vec![MATCHTPT_TAG, 0x80 | payload.len() as u8];
            bytes.extend(payload);
            let mut reader = Reader::new(&bytes);
            let nodes = RawNodes::decode(&mut reader).unwrap();
            let matched = nodes.get(0).unwrap().decode_match_tpt().unwrap();

            assert_eq!(matched.bound.is_some(), has_bound);
            assert!(matches!(matched.selector, RawTree::Leaf(_)));
            assert_eq!(matched.cases.len(), 1);
            assert!(matched.cases[0].guard.is_none());
        }
    }

    #[test]
    fn rejects_match_tpt_without_a_selector() {
        let node = RawNode {
            tag: MATCHTPT_TAG,
            offset: 0,
            payload: &[CASEDEF_TAG, 0x80],
        };

        assert!(node.decode_match_tpt().is_err());
    }

    #[test]
    fn rejects_match_tpt_with_more_than_one_optional_bound() {
        let node = RawNode {
            tag: MATCHTPT_TAG,
            offset: 0,
            payload: &[2, 3, 4, CASEDEF_TAG, 0x80],
        };

        assert_eq!(
            node.decode_match_tpt(),
            Err(AstError::UnsupportedCategory {
                tag: MATCHTPT_TAG,
                offset: 0,
            })
        );
    }

    #[test]
    fn encodes_structured_category_three_and_four_nodes() {
        let category_three = [THIS_TAG, TERMREFPKG_TAG, 0x81];
        let mut reader = Reader::new(&category_three);
        let node = RawTree::decode(&mut reader).unwrap().decode_this().unwrap();
        let mut writer = Writer::new();
        node.encode(&mut writer).unwrap();
        assert_eq!(writer.as_slice(), category_three);

        let category_four = [SELECT_TAG, 0x85, TERMREFPKG_TAG, 0x81];
        let mut reader = Reader::new(&category_four);
        let node = RawTree::decode(&mut reader)
            .unwrap()
            .decode_select()
            .unwrap();
        let mut writer = Writer::new();
        node.encode(&mut writer).unwrap();
        assert_eq!(writer.as_slice(), category_four);
    }

    #[test]
    fn round_trips_apply_encoding() {
        assert_structured_round_trip(&[APPLY_TAG, 0x83, 2, 3, 4], |raw, writer| {
            raw.decode_apply().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_type_apply_encoding() {
        assert_structured_round_trip(&[TYPEAPPLY_TAG, 0x83, 2, 3, 4], |raw, writer| {
            raw.decode_type_apply().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_typed_encoding() {
        assert_structured_round_trip(&[TYPED_TAG, 0x82, 2, 3], |raw, writer| {
            raw.decode_typed().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_assign_encoding() {
        assert_structured_round_trip(&[ASSIGN_TAG, 0x82, 2, 3], |raw, writer| {
            raw.decode_assign().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_block_encoding() {
        assert_structured_round_trip(&[BLOCK_TAG, 0x83, 2, VALDEF_TAG, 0x80], |raw, writer| {
            raw.decode_block().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_if_encoding() {
        assert_structured_round_trip(&[IF_TAG, 0x84, INLINE_TAG, 2, 3, 4], |raw, writer| {
            raw.decode_if().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_lambda_encoding() {
        assert_structured_round_trip(&[LAMBDA_TAG, 0x82, 2, 3], |raw, writer| {
            raw.decode_lambda().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_match_encoding() {
        assert_structured_round_trip(
            &[
                MATCH_TAG,
                0x88,
                SUBMATCH_TAG,
                2,
                CASEDEF_TAG,
                0x84,
                TERMREFPKG_TAG,
                0x81,
                TERMREFPKG_TAG,
                0x82,
            ],
            |raw, writer| raw.decode_match().unwrap().encode(writer),
        );
    }

    #[test]
    fn round_trips_return_encoding() {
        assert_structured_round_trip(&[RETURN_TAG, 0x82, 0x85, 2], |raw, writer| {
            raw.decode_return().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_while_encoding() {
        assert_structured_round_trip(&[WHILE_TAG, 0x82, 2, 3], |raw, writer| {
            raw.decode_while().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_try_encoding() {
        assert_structured_round_trip(
            &[
                TRY_TAG,
                0x88,
                2,
                CASEDEF_TAG,
                0x84,
                TERMREFPKG_TAG,
                0x81,
                TERMREFPKG_TAG,
                0x82,
                4,
            ],
            |raw, writer| raw.decode_try().unwrap().encode(writer),
        );
    }

    #[test]
    fn round_trips_inlined_encoding() {
        assert_structured_round_trip(
            &[INLINED_TAG, 0x84, 2, 3, VALDEF_TAG, 0x80],
            |raw, writer| raw.decode_inlined().unwrap().encode(writer),
        );
    }

    #[test]
    fn round_trips_select_outer_encoding() {
        assert_structured_round_trip(&[SELECTOUTER_TAG, 0x83, 0x85, 2, 3], |raw, writer| {
            raw.decode_select_outer().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_repeated_encoding() {
        assert_structured_round_trip(&[REPEATED_TAG, 0x83, 2, 3, 4], |raw, writer| {
            raw.decode_repeated().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_super_encoding() {
        assert_structured_round_trip(&[SUPER_TAG, 0x82, 2, 3], |raw, writer| {
            raw.decode_super().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_quote_encoding() {
        assert_structured_round_trip(&[QUOTE_TAG, 0x82, 2, 3], |raw, writer| {
            raw.decode_quote().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_apply_sigpoly_encoding() {
        assert_structured_round_trip(&[APPLYSIGPOLY_TAG, 0x83, 2, 3, 4], |raw, writer| {
            raw.decode_apply_sigpoly().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_and_type_encoding() {
        assert_structured_round_trip(&[ANDTYPE_TAG, 0x82, 2, 3], |raw, writer| {
            raw.decode_and_type().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_applied_type_encoding() {
        assert_structured_round_trip(&[APPLIEDTYPE_TAG, 0x83, 2, 3, 4], |raw, writer| {
            raw.decode_applied_type().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_flexible_type_encoding() {
        assert_structured_round_trip(&[FLEXIBLETYPE_TAG, 0x81, 2], |raw, writer| {
            raw.decode_flexible_type().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_type_bounds_encoding() {
        assert_structured_round_trip(&[TYPEBOUNDS_TAG, 0x83, 2, 3, 28], |raw, writer| {
            raw.decode_type_bounds().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_annotated_type_encoding() {
        assert_structured_round_trip(&[ANNOTATEDTYPE_TAG, 0x82, 2, 3], |raw, writer| {
            raw.decode_annotated().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_annotation_encoding() {
        assert_structured_round_trip(&[ANNOTATION_TAG, 0x82, 2, 3], |raw, writer| {
            raw.decode_annotation().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_param_type_encoding() {
        assert_structured_round_trip(&[PARAMTYPE_TAG, 0x82, 0x85, 0x83], |raw, writer| {
            raw.decode_param_type().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_poly_type_encoding() {
        assert_structured_round_trip(&[POLYTYPE_TAG, 0x83, 2, 0x85, 0x86], |raw, writer| {
            raw.decode_poly_type().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_method_type_encoding() {
        assert_structured_round_trip(&[METHODTYPE_TAG, 0x83, 2, 0x85, 0x86], |raw, writer| {
            raw.decode_method_type().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_contextual_method_type_encoding() {
        assert_structured_round_trip(&[METHODTYPE_TAG, 0x84, 2, 0x91, 0x85, 37], |raw, writer| {
            raw.decode_method_type_with_type_name_count(1)
                .unwrap()
                .encode(writer)
        });
    }

    #[test]
    fn round_trips_refined_type_encoding() {
        assert_structured_round_trip(
            &[
                REFINEDTYPE_TAG,
                0x85,
                0x85,
                TERMREFPKG_TAG,
                0x81,
                TERMREFPKG_TAG,
                0x82,
            ],
            |raw, writer| raw.decode_refined_type().unwrap().encode(writer),
        );
    }

    #[test]
    fn round_trips_refined_type_tree_encoding() {
        assert_structured_round_trip(
            &[
                REFINEDTPT_TAG,
                0x84,
                TERMREFPKG_TAG,
                0x81,
                PACKAGE_TAG,
                0x80,
            ],
            |raw, writer| raw.decode_refined_tpt().unwrap().encode(writer),
        );
    }

    #[test]
    fn round_trips_bind_encoding() {
        assert_structured_round_trip(&[BIND_TAG, 0x83, 0x85, 2, 3], |raw, writer| {
            raw.decode_bind().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_alternative_encoding() {
        assert_structured_round_trip(&[ALTERNATIVE_TAG, 0x82, 2, 3], |raw, writer| {
            raw.decode_alternative().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_unapply_encoding() {
        assert_structured_round_trip(
            &[
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
            ],
            |raw, writer| raw.decode_unapply().unwrap().encode(writer),
        );
    }

    #[test]
    fn round_trips_quote_pattern_encoding() {
        assert_structured_round_trip(&[QUOTEPATTERN_TAG, 0x84, 2, 3, 4, 5], |raw, writer| {
            raw.decode_quote_pattern().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_splice_pattern_encoding() {
        assert_structured_round_trip(&[SPLICEPATTERN_TAG, 0x84, 2, 3, 4, 5], |raw, writer| {
            raw.decode_splice_pattern().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_hole_encoding() {
        assert_structured_round_trip(&[HOLE_TAG, 0x84, 0x85, 2, 3, 4], |raw, writer| {
            raw.decode_hole().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_match_type_encoding() {
        assert_structured_round_trip(
            &[MATCHTYPE_TAG, 0x86, 2, 3, MATCHCASETYPE_TAG, 0x82, 4, 5],
            |raw, writer| raw.decode_match_type().unwrap().encode(writer),
        );
    }

    #[test]
    fn round_trips_match_type_tree_without_bound_encoding() {
        assert_structured_round_trip(
            &[MATCHTPT_TAG, 0x85, 2, CASEDEF_TAG, 0x82, 3, 4],
            |raw, writer| raw.decode_match_tpt().unwrap().encode(writer),
        );
    }

    #[test]
    fn round_trips_match_type_tree_with_bound_encoding() {
        assert_structured_round_trip(
            &[MATCHTPT_TAG, 0x86, 2, 3, CASEDEF_TAG, 0x82, 4, 5],
            |raw, writer| raw.decode_match_tpt().unwrap().encode(writer),
        );
    }

    #[test]
    fn round_trips_term_refinement_encoding() {
        assert_structured_round_trip(&[TERMREFIN_TAG, 0x83, 0x85, 2, 3], |raw, writer| {
            raw.decode_in_reference().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_type_refinement_encoding() {
        assert_structured_round_trip(&[TYPEREFIN_TAG, 0x83, 0x85, 2, 3], |raw, writer| {
            raw.decode_in_reference().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_select_in_encoding() {
        assert_structured_round_trip(&[SELECTIN_TAG, 0x83, 0x85, 2, 3], |raw, writer| {
            raw.decode_select_in().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_package_encoding() {
        assert_structured_round_trip(
            &[PACKAGE_TAG, 0x84, TERMREFPKG_TAG, 0x85, VALDEF_TAG, 0x80],
            |raw, writer| raw.decode_package().unwrap().encode(writer),
        );
    }

    #[test]
    fn round_trips_import_encoding() {
        assert_structured_round_trip(
            &[
                IMPORT_TAG,
                0x89,
                TERMREFPKG_TAG,
                0x81,
                IMPORTED_TAG,
                0x82,
                RENAMED_TAG,
                0x83,
                BOUNDED_TAG,
                TERMREFPKG_TAG,
                0x84,
            ],
            |raw, writer| raw.decode_import_export().unwrap().encode(writer),
        );
    }

    #[test]
    fn round_trips_definition_header_encoding() {
        assert_structured_round_trip(
            &[VALDEF_TAG, 0x83, 0x85, TERMREFPKG_TAG, 0x81],
            |raw, writer| raw.decode_definition().unwrap().encode(writer),
        );
    }

    #[test]
    fn round_trips_empty_template_encoding() {
        assert_structured_round_trip(&[TEMPLATE_TAG, 0x80], |raw, writer| {
            raw.decode_template().unwrap().encode(writer)
        });
    }

    #[test]
    fn round_trips_template_with_parameters_encoding() {
        assert_structured_round_trip(
            &[TEMPLATE_TAG, 0x85, TYPEPARAM_TAG, 0x83, 0x81, 2, 17],
            |raw, writer| raw.decode_template().unwrap().encode(writer),
        );
    }

    #[test]
    fn round_trips_valdef_body_encoding() {
        assert_structured_round_trip(&[VALDEF_TAG, 0x84, 0x85, 2, 3, 17], |raw, writer| {
            raw.decode_definition_body().unwrap().encode(5, writer)
        });
    }

    #[test]
    fn round_trips_defdef_body_encoding() {
        assert_structured_round_trip(&[DEFDEF_TAG, 0x84, 0x85, 2, 3, 17], |raw, writer| {
            raw.decode_defdef_body().unwrap().encode(5, writer)
        });
    }

    #[test]
    fn round_trips_typedef_body_encoding() {
        assert_structured_round_trip(
            &[TYPEDEF_TAG, 0x84, 0x85, TEMPLATE_TAG, 0x80, 17],
            |raw, writer| raw.decode_definition_body().unwrap().encode(5, writer),
        );
    }

    #[test]
    fn round_trips_template_structure_encoding() {
        assert_structured_round_trip(
            &[
                TEMPLATE_TAG,
                0x87,
                2,
                SELFDEF_TAG,
                0x85,
                3,
                SPLITCLAUSE_TAG,
                VALDEF_TAG,
                0x80,
            ],
            |raw, writer| raw.decode_template_structure().unwrap().encode(writer),
        );
    }

    fn assert_structured_round_trip<F>(bytes: &[u8], encode: F)
    where
        F: FnOnce(&RawNode<'_>, &mut Writer) -> Result<(), TermEncodeError>,
    {
        let mut reader = Reader::new(bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let mut writer = Writer::new();

        encode(nodes.get(0).unwrap(), &mut writer).unwrap();

        assert_eq!(writer.as_slice(), bytes);
    }

    macro_rules! decodes_category_three_child {
        ($name:ident, $tag:expr, $decoder:ident) => {
            #[test]
            fn $name() {
                let mut reader = Reader::new(&[$tag, TERMREFPKG_TAG, 0x81]);
                let node = RawTree::decode(&mut reader).unwrap().$decoder().unwrap();

                assert_eq!(node.tag, $tag);
                assert!(reader.is_at_end());
            }
        };
    }

    decodes_category_three_child!(decodes_this_node, THIS_TAG, decode_this);
    decodes_category_three_child!(decodes_new_node, NEW_TAG, decode_new);
    decodes_category_three_child!(decodes_throw_node, THROW_TAG, decode_throw);
    decodes_category_three_child!(decodes_elided_node, ELIDED_TAG, decode_elided);
    decodes_category_three_child!(decodes_qual_this_node, QUALTHIS_TAG, decode_qual_this);
    decodes_category_three_child!(decodes_class_const_node, CLASSCONST_TAG, decode_class_const);

    #[test]
    fn decodes_and_encodes_a_typed_class_constant() {
        let bytes = [CLASSCONST_TAG, TERMREFPKG_TAG, 0x81];
        let mut reader = Reader::new(&bytes);
        let node = RawTree::decode(&mut reader)
            .unwrap()
            .decode_class_constant()
            .unwrap();

        assert!(matches!(
            &node.type_tree,
            RawTree::Leaf(term) if term.tag == TERMREFPKG_TAG && term.value == crate::term::TermValue::NameRef(1)
        ));

        let mut writer = Writer::new();
        node.encode(&mut writer).unwrap();
        assert_eq!(writer.as_slice(), bytes);
        assert!(reader.is_at_end());
    }

    decodes_category_three_child!(
        decodes_by_name_type_node,
        BYNAMETYPE_TAG,
        decode_by_name_type
    );
    decodes_category_three_child!(decodes_by_name_tpt_node, BYNAMETPT_TAG, decode_by_name_tpt);
    decodes_category_three_child!(
        decodes_implicit_arg_node,
        IMPLICITARG_TAG,
        decode_implicit_arg
    );
    decodes_category_three_child!(
        decodes_private_qualified_node,
        PRIVATEQUALIFIED_TAG,
        decode_private_qualified
    );
    decodes_category_three_child!(
        decodes_protected_qualified_node,
        PROTECTEDQUALIFIED_TAG,
        decode_protected_qualified
    );
    decodes_category_three_child!(decodes_rec_type_node, RECTYPE_TAG, decode_rec_type);
    decodes_category_three_child!(
        decodes_singleton_tpt_node,
        SINGLETONTPT_TAG,
        decode_singleton_tpt
    );
    decodes_category_three_child!(decodes_bounded_node, BOUNDED_TAG, decode_bounded);
    decodes_category_three_child!(
        decodes_explicit_tpt_node,
        EXPLICITTPT_TAG,
        decode_explicit_tpt
    );

    #[test]
    fn allocates_addresses_for_top_level_ast_nodes_while_encoding() {
        let bytes = [VALDEF_TAG, 0x82, b'a', b'b', DEFDEF_TAG, 0x81, b'c'];
        let mut reader = Reader::new(&bytes);
        let nodes = super::RawNodes::decode(&mut reader).unwrap();
        let encoded = nodes.encode_with_addresses().unwrap();

        assert_eq!(encoded.as_slice(), bytes);
        assert_eq!(encoded.addresses(), &[0, 4]);
        assert_eq!(encoded.address(1), Some(4));
        assert_eq!(encoded.address(2), None);
    }

    #[test]
    fn rejects_non_category_five_raw_nodes_when_encoding() {
        let node = RawNode {
            tag: 127,
            offset: 0,
            payload: &[],
        };
        let mut writer = Writer::new();

        assert_eq!(
            node.encode(&mut writer),
            Err(WriteError::InvalidTag { tag: 127 })
        );
        assert!(writer.as_slice().is_empty());
    }

    #[test]
    fn dispatches_a_package_node_to_its_structured_variant() {
        let node = RawNode {
            tag: PACKAGE_TAG,
            offset: 0,
            payload: &[TERMREFPKG_TAG, 0x81, VALDEF_TAG, 0x80],
        };

        assert!(matches!(
            node.decode_structured().unwrap(),
            super::StructuredNode::Package(package)
                if package.path_name == 1 && package.stats.len() == 1
        ));
    }

    #[test]
    fn dispatches_a_flexible_type_node_to_its_structured_variant() {
        let node = RawNode {
            tag: FLEXIBLETYPE_TAG,
            offset: 0,
            payload: &[2],
        };

        assert!(matches!(
            node.decode_structured().unwrap(),
            super::StructuredNode::FlexibleType(_)
        ));
    }

    #[test]
    fn collects_ast_references_from_a_structured_node_in_wire_order() {
        let node = RawNode {
            tag: super::APPLY_TAG,
            offset: 0,
            payload: &[super::TERMREFDIRECT_TAG, 0x85, super::SHAREDTYPE_TAG, 0x83],
        };

        assert_eq!(
            node.ast_refs().unwrap(),
            vec![
                crate::AstRef {
                    kind: crate::AstRefKind::TermRefDirect,
                    address: 5,
                },
                crate::AstRef {
                    kind: crate::AstRefKind::SharedType,
                    address: 3,
                },
            ]
        );

        let mut visited = Vec::new();
        node.visit_ast_refs(&mut |reference| visited.push(reference))
            .unwrap();
        assert_eq!(visited, node.ast_refs().unwrap());
    }

    #[test]
    fn collects_name_references_from_a_structured_node_in_wire_order() {
        let node = RawNode {
            tag: super::APPLY_TAG,
            offset: 0,
            payload: &[
                super::IDENT_TAG,
                0x85,
                2,
                super::SELECT_TAG,
                0x86,
                super::TERMREFPKG_TAG,
                0x87,
            ],
        };

        assert_eq!(node.name_refs().unwrap(), vec![5, 6, 7]);

        let mut visited = Vec::new();
        node.visit_name_refs(&mut |reference| visited.push(reference))
            .unwrap();
        assert_eq!(visited, node.name_refs().unwrap());
    }

    #[test]
    fn collects_a_definition_name_before_its_nested_name_references() {
        let node = RawNode {
            tag: super::VALDEF_TAG,
            offset: 0,
            payload: &[0x85, super::TERMREFPKG_TAG, 0x86],
        };

        assert_eq!(node.name_refs().unwrap(), vec![5, 6]);
    }

    #[test]
    fn collects_a_package_path_and_nested_definition_name() {
        let node = RawNode {
            tag: super::PACKAGE_TAG,
            offset: 0,
            payload: &[
                super::TERMREFPKG_TAG,
                0x85,
                super::VALDEF_TAG,
                0x82,
                0x86,
                2,
            ],
        };

        assert_eq!(node.name_refs().unwrap(), vec![5, 6]);
    }

    #[test]
    fn collects_names_from_poly_type_name_suffixes() {
        let node = RawNode {
            tag: super::POLYTYPE_TAG,
            offset: 0,
            payload: &[2, 0x81, 0x85],
        };

        assert_eq!(node.name_refs().unwrap(), vec![5]);
    }

    #[test]
    fn dispatches_and_decodes_an_annotation_node() {
        let node = RawNode {
            tag: ANNOTATION_TAG,
            offset: 0,
            payload: &[2, 3],
        };

        assert!(matches!(
            node.decode_structured().unwrap(),
            super::StructuredNode::Annotation(annotation)
                if matches!(annotation.tycon, RawTree::Leaf(ref term) if term.tag == 2)
                    && matches!(annotation.full_annotation, RawTree::Leaf(ref term) if term.tag == 3)
        ));
    }

    #[test]
    fn encodes_a_structured_valdef_with_its_decoded_name() {
        let node = RawNode {
            tag: VALDEF_TAG,
            offset: 0,
            payload: &[0x81, 2],
        };
        let structured = node.decode_structured().unwrap();

        assert!(matches!(
            &structured,
            StructuredNode::ValDef(body) if body.name() == 1
        ));

        let mut writer = Writer::new();
        structured.encode(&mut writer).unwrap();
        let mut expected = Writer::new();
        node.encode(&mut expected).unwrap();
        assert_eq!(writer.as_slice(), expected.as_slice());
    }

    #[test]
    fn encodes_a_structured_defdef_with_its_decoded_name() {
        let node = RawNode {
            tag: DEFDEF_TAG,
            offset: 0,
            payload: &[0x81, 2],
        };
        let structured = node.decode_structured().unwrap();

        assert!(matches!(
            &structured,
            StructuredNode::DefDef(body) if body.name() == 1
        ));

        let mut writer = Writer::new();
        structured.encode(&mut writer).unwrap();
        let mut expected = Writer::new();
        node.encode(&mut expected).unwrap();
        assert_eq!(writer.as_slice(), expected.as_slice());
    }

    #[test]
    fn encodes_an_unknown_structured_node_without_reinterpreting_its_payload() {
        let node = RawNode {
            tag: 200,
            offset: 0,
            payload: b"future",
        };
        let structured = node.decode_structured().unwrap();

        assert!(matches!(&structured, StructuredNode::Raw(raw) if raw.tag == 200));

        let mut writer = Writer::new();
        structured.encode(&mut writer).unwrap();
        let mut expected = Writer::new();
        node.encode(&mut expected).unwrap();
        assert_eq!(writer.as_slice(), expected.as_slice());
    }

    macro_rules! dispatches_to_structured_variant {
        ($name:ident, $tag:expr, $payload:expr, $pattern:pat) => {
            #[test]
            fn $name() {
                let node = RawNode {
                    tag: $tag,
                    offset: 0,
                    payload: $payload,
                };
                let structured = node
                    .decode_structured()
                    .unwrap_or_else(|error| panic!("tag {} failed to dispatch: {error}", $tag));
                assert!(
                    matches!(structured.clone(), $pattern),
                    "tag {} dispatched to an unexpected variant",
                    $tag
                );

                let mut structured_bytes = Writer::new();
                structured.encode(&mut structured_bytes).unwrap();
                let mut raw_bytes = Writer::new();
                node.encode(&mut raw_bytes).unwrap();
                assert_eq!(
                    structured_bytes.as_slice(),
                    raw_bytes.as_slice(),
                    "tag {} changed during structured round-trip",
                    $tag
                );
            }
        };
    }

    dispatches_to_structured_variant!(
        dispatches_package,
        PACKAGE_TAG,
        &[TERMREFPKG_TAG, 0x81, VALDEF_TAG, 0x80],
        StructuredNode::Package(_)
    );
    dispatches_to_structured_variant!(
        dispatches_valdef,
        VALDEF_TAG,
        &[0x81, 2],
        StructuredNode::ValDef(_)
    );
    dispatches_to_structured_variant!(
        dispatches_defdef,
        DEFDEF_TAG,
        &[0x81, 2],
        StructuredNode::DefDef(_)
    );
    dispatches_to_structured_variant!(
        dispatches_typedef,
        TYPEDEF_TAG,
        &[0x81, 2],
        StructuredNode::TypeDef(_)
    );
    dispatches_to_structured_variant!(
        dispatches_import,
        IMPORT_TAG,
        &[2],
        StructuredNode::ImportExport(_)
    );
    dispatches_to_structured_variant!(
        dispatches_export,
        EXPORT_TAG,
        &[2],
        StructuredNode::ImportExport(_)
    );
    dispatches_to_structured_variant!(
        dispatches_typeparam,
        TYPEPARAM_TAG,
        &[0x81, 2],
        StructuredNode::Parameter(_)
    );
    dispatches_to_structured_variant!(
        dispatches_param,
        PARAM_TAG,
        &[0x81, 2],
        StructuredNode::Parameter(_)
    );
    dispatches_to_structured_variant!(dispatches_apply, APPLY_TAG, &[2], StructuredNode::Apply(_));
    dispatches_to_structured_variant!(
        dispatches_type_apply,
        TYPEAPPLY_TAG,
        &[2],
        StructuredNode::TypeApply(_)
    );
    dispatches_to_structured_variant!(
        dispatches_typed,
        TYPED_TAG,
        &[2, 3],
        StructuredNode::Typed(_)
    );
    dispatches_to_structured_variant!(
        dispatches_assign,
        ASSIGN_TAG,
        &[2, 3],
        StructuredNode::Assign(_)
    );
    dispatches_to_structured_variant!(dispatches_block, BLOCK_TAG, &[2], StructuredNode::Block(_));
    dispatches_to_structured_variant!(dispatches_if, IF_TAG, &[2, 3, 4], StructuredNode::If(_));
    dispatches_to_structured_variant!(
        dispatches_lambda,
        LAMBDA_TAG,
        &[2],
        StructuredNode::Lambda(_)
    );
    dispatches_to_structured_variant!(dispatches_match, MATCH_TAG, &[2], StructuredNode::Match(_));
    dispatches_to_structured_variant!(
        dispatches_return,
        RETURN_TAG,
        &[0x81],
        StructuredNode::Return(_)
    );
    dispatches_to_structured_variant!(
        dispatches_while,
        WHILE_TAG,
        &[2, 3],
        StructuredNode::While(_)
    );
    dispatches_to_structured_variant!(dispatches_try, TRY_TAG, &[2], StructuredNode::Try(_));
    dispatches_to_structured_variant!(
        dispatches_inlined,
        INLINED_TAG,
        &[2],
        StructuredNode::Inlined(_)
    );
    dispatches_to_structured_variant!(
        dispatches_select_outer,
        SELECTOUTER_TAG,
        &[0x81, 2, 3],
        StructuredNode::SelectOuter(_)
    );
    dispatches_to_structured_variant!(
        dispatches_repeated,
        REPEATED_TAG,
        &[2],
        StructuredNode::Repeated(_)
    );
    dispatches_to_structured_variant!(
        dispatches_bind,
        BIND_TAG,
        &[0x81, 2, 3],
        StructuredNode::Bind(_)
    );
    dispatches_to_structured_variant!(
        dispatches_alternative,
        ALTERNATIVE_TAG,
        &[],
        StructuredNode::Alternative(_)
    );
    dispatches_to_structured_variant!(
        dispatches_unapply,
        UNAPPLY_TAG,
        &[2, 2],
        StructuredNode::Unapply(_)
    );
    dispatches_to_structured_variant!(
        dispatches_annotated_type,
        ANNOTATEDTYPE_TAG,
        &[2, 3],
        StructuredNode::Annotated(_)
    );
    dispatches_to_structured_variant!(
        dispatches_annotated_tpt,
        ANNOTATEDTPT_TAG,
        &[2, 3],
        StructuredNode::Annotated(_)
    );
    dispatches_to_structured_variant!(
        dispatches_annotation,
        ANNOTATION_TAG,
        &[2, 3],
        StructuredNode::Annotation(_)
    );
    dispatches_to_structured_variant!(
        dispatches_case_def,
        CASEDEF_TAG,
        &[2, 3],
        StructuredNode::CaseDef(_)
    );
    dispatches_to_structured_variant!(
        dispatches_template,
        TEMPLATE_TAG,
        &[],
        StructuredNode::Template(_)
    );
    dispatches_to_structured_variant!(dispatches_super, SUPER_TAG, &[2], StructuredNode::Super(_));
    dispatches_to_structured_variant!(
        dispatches_super_type,
        SUPERTYPE_TAG,
        &[2, 3],
        StructuredNode::BinaryType(_)
    );
    dispatches_to_structured_variant!(
        dispatches_refined_type,
        REFINEDTYPE_TAG,
        &[0x81, 2, 3],
        StructuredNode::RefinedType(_)
    );
    dispatches_to_structured_variant!(
        dispatches_refined_tpt,
        REFINEDTPT_TAG,
        &[2],
        StructuredNode::RefinedTpt(_)
    );
    dispatches_to_structured_variant!(
        dispatches_applied_type,
        APPLIEDTYPE_TAG,
        &[2],
        StructuredNode::AppliedType(_)
    );
    dispatches_to_structured_variant!(
        dispatches_applied_tpt,
        APPLIEDTPT_TAG,
        &[2],
        StructuredNode::AppliedType(_)
    );
    dispatches_to_structured_variant!(
        dispatches_type_bounds,
        TYPEBOUNDS_TAG,
        &[2],
        StructuredNode::TypeBounds(_)
    );
    dispatches_to_structured_variant!(
        dispatches_type_bounds_tpt,
        TYPEBOUNDSTPT_TAG,
        &[2],
        StructuredNode::TypeBounds(_)
    );
    dispatches_to_structured_variant!(
        dispatches_and_type,
        ANDTYPE_TAG,
        &[2, 3],
        StructuredNode::BinaryType(_)
    );
    dispatches_to_structured_variant!(
        dispatches_or_type,
        ORTYPE_TAG,
        &[2, 3],
        StructuredNode::BinaryType(_)
    );
    dispatches_to_structured_variant!(
        dispatches_poly_type,
        POLYTYPE_TAG,
        &[2],
        StructuredNode::PolyType(_)
    );
    dispatches_to_structured_variant!(
        dispatches_type_lambda_type,
        TYPELAMBDATYPE_TAG,
        &[2],
        StructuredNode::PolyType(_)
    );
    dispatches_to_structured_variant!(
        dispatches_lambda_tpt,
        LAMBDATPT_TAG,
        &[2],
        StructuredNode::LambdaTpt(_)
    );
    dispatches_to_structured_variant!(
        dispatches_param_type,
        PARAMTYPE_TAG,
        &[0x81, 0x82],
        StructuredNode::ParamType(_)
    );
    dispatches_to_structured_variant!(
        dispatches_method_type,
        METHODTYPE_TAG,
        &[2],
        StructuredNode::MethodType(_)
    );
    dispatches_to_structured_variant!(
        dispatches_apply_sigpoly,
        APPLYSIGPOLY_TAG,
        &[2, 3],
        StructuredNode::ApplySigPoly(_)
    );
    dispatches_to_structured_variant!(
        dispatches_quote,
        QUOTE_TAG,
        &[2, 3],
        StructuredNode::Quote(_)
    );
    dispatches_to_structured_variant!(
        dispatches_splice,
        SPLICE_TAG,
        &[2, 3],
        StructuredNode::Quote(_)
    );
    dispatches_to_structured_variant!(
        dispatches_quote_pattern,
        QUOTEPATTERN_TAG,
        &[2, 3, 4],
        StructuredNode::QuotePattern(_)
    );
    dispatches_to_structured_variant!(
        dispatches_splice_pattern,
        SPLICEPATTERN_TAG,
        &[2, 3],
        StructuredNode::SplicePattern(_)
    );
    dispatches_to_structured_variant!(
        dispatches_hole,
        HOLE_TAG,
        &[0x81, 2],
        StructuredNode::Hole(_)
    );
    dispatches_to_structured_variant!(
        dispatches_match_type,
        MATCHTYPE_TAG,
        &[2, 3],
        StructuredNode::MatchType(_)
    );
    dispatches_to_structured_variant!(
        dispatches_match_tpt,
        MATCHTPT_TAG,
        &[2, 3],
        StructuredNode::MatchTpt(_)
    );
    dispatches_to_structured_variant!(
        dispatches_term_refinement,
        TERMREFIN_TAG,
        &[0x81, 2, 3],
        StructuredNode::InReference(_)
    );
    dispatches_to_structured_variant!(
        dispatches_type_refinement,
        TYPEREFIN_TAG,
        &[0x81, 2, 3],
        StructuredNode::InReference(_)
    );
    dispatches_to_structured_variant!(
        dispatches_select_in,
        SELECTIN_TAG,
        &[0x81, 2, 3],
        StructuredNode::SelectIn(_)
    );
    dispatches_to_structured_variant!(
        dispatches_match_case_type,
        MATCHCASETYPE_TAG,
        &[2, 3],
        StructuredNode::BinaryType(_)
    );
    dispatches_to_structured_variant!(
        dispatches_flexible_type,
        FLEXIBLETYPE_TAG,
        &[2],
        StructuredNode::FlexibleType(_)
    );
    dispatches_to_structured_variant!(
        dispatches_unknown_category_five,
        135,
        &[],
        StructuredNode::Raw(_)
    );

    #[test]
    fn rejects_a_truncated_or_extended_annotation_payload() {
        let truncated = RawNode {
            tag: ANNOTATION_TAG,
            offset: 0,
            payload: &[2],
        };
        assert!(truncated.decode_annotation().is_err());

        let extended = RawNode {
            tag: ANNOTATION_TAG,
            offset: 0,
            payload: &[2, 3, 4],
        };
        assert_eq!(
            extended.decode_annotation(),
            Err(AstError::UnsupportedCategory {
                tag: ANNOTATION_TAG,
                offset: 0,
            })
        );
    }

    #[test]
    fn preserves_an_unknown_category_five_node_through_structured_dispatch() {
        let node = RawNode {
            tag: 135,
            offset: 4,
            payload: b"future",
        };

        assert_eq!(
            node.decode_structured().unwrap(),
            super::StructuredNode::Raw(node)
        );
    }

    #[test]
    fn indexes_decoded_nodes_by_their_ast_addresses() {
        let bytes = [VALDEF_TAG, 0x82, b'a', b'b', DEFDEF_TAG, 0x81, b'c'];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let index = nodes.address_index();

        assert_eq!(index.len(), 2);
        assert_eq!(index.addresses().collect::<Vec<_>>(), vec![0, 4]);
        assert_eq!(index.get(4).unwrap().tag, DEFDEF_TAG);
        assert!(index.get(1).is_none());
    }

    #[test]
    fn sorts_programmatic_ast_addresses_before_index_lookup() {
        let nodes = RawNodes::from_entries(vec![
            RawNode {
                tag: DEFDEF_TAG,
                offset: 9,
                payload: &[],
            },
            RawNode {
                tag: VALDEF_TAG,
                offset: 2,
                payload: &[],
            },
        ])
        .unwrap();
        let index = nodes.address_index();

        assert_eq!(index.addresses().collect::<Vec<_>>(), vec![2, 9]);
        assert_eq!(index.get(2).map(|node| node.tag), Some(VALDEF_TAG));
        assert_eq!(index.get(9).map(|node| node.tag), Some(DEFDEF_TAG));
    }

    #[test]
    fn iterates_indexed_category_five_nodes_in_address_order() {
        let nodes = RawNodes::from_entries(vec![
            RawNode {
                tag: DEFDEF_TAG,
                offset: 9,
                payload: &[],
            },
            RawNode {
                tag: VALDEF_TAG,
                offset: 2,
                payload: &[],
            },
        ])
        .unwrap();
        let index = nodes.address_index();

        assert_eq!(
            index.iter().map(|node| node.offset).collect::<Vec<_>>(),
            vec![2, 9]
        );
    }

    #[test]
    fn iterates_all_visible_ast_nodes_in_address_order() {
        let bytes = [
            APPLY_TAG, 0x87, BLOCK_TAG, 0x85, 2, VALDEF_TAG, 0x82, 0x81, 2,
        ];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let index = nodes
            .deep_address_index_with_source_and_max_depth(&bytes, DEFAULT_MAX_AST_INDEX_DEPTH)
            .unwrap();

        assert_eq!(
            index
                .iter_nodes()
                .map(|node| node.offset)
                .collect::<Vec<_>>(),
            vec![0, 2, 4, 5, 8]
        );
    }

    #[test]
    fn filters_visible_ast_nodes_by_tag_in_address_order() {
        let bytes = [
            APPLY_TAG, 0x87, BLOCK_TAG, 0x85, 2, VALDEF_TAG, 0x82, 0x81, 2,
        ];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let index = nodes
            .deep_address_index_with_source_and_max_depth(&bytes, DEFAULT_MAX_AST_INDEX_DEPTH)
            .unwrap();

        assert_eq!(
            index
                .iter_nodes_with_tag(VALDEF_TAG)
                .map(|node| node.offset)
                .collect::<Vec<_>>(),
            vec![5]
        );
        assert_eq!(
            index
                .iter_nodes_with_tag(APPLY_TAG)
                .map(|node| node.offset)
                .collect::<Vec<_>>(),
            vec![0]
        );
    }

    #[test]
    fn returns_no_visible_ast_nodes_for_an_absent_tag() {
        let nodes = RawNodes::from_entries(vec![RawNode {
            tag: VALDEF_TAG,
            offset: 0,
            payload: &[],
        }])
        .unwrap();
        let index = nodes.address_index();

        assert_eq!(
            index.iter_nodes_with_tag(DEFDEF_TAG).collect::<Vec<_>>(),
            Vec::new()
        );
    }

    #[test]
    fn filters_visible_ast_nodes_by_an_absolute_address_range() {
        let bytes = [
            APPLY_TAG, 0x87, BLOCK_TAG, 0x85, 2, VALDEF_TAG, 0x82, 0x81, 2,
        ];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let index = nodes
            .deep_address_index_with_source_and_max_depth(&bytes, DEFAULT_MAX_AST_INDEX_DEPTH)
            .unwrap();

        assert_eq!(
            index
                .iter_nodes_in_address_range(2, 8)
                .map(|node| node.offset)
                .collect::<Vec<_>>(),
            vec![2, 4, 5]
        );
    }

    #[test]
    fn records_direct_ast_parent_child_edges_in_wire_order() {
        let bytes = [
            APPLY_TAG, 0x87, BLOCK_TAG, 0x85, 2, VALDEF_TAG, 0x82, 0x81, 2,
        ];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let index = nodes
            .deep_address_index_with_source_and_max_depth(&bytes, DEFAULT_MAX_AST_INDEX_DEPTH)
            .unwrap();

        assert_eq!(
            index.iter_tree_edges().collect::<Vec<_>>(),
            vec![
                AstTreeEdge {
                    parent: AstTreeNode {
                        tag: APPLY_TAG,
                        offset: 0,
                    },
                    child: AstTreeNode {
                        tag: BLOCK_TAG,
                        offset: 2,
                    },
                },
                AstTreeEdge {
                    parent: AstTreeNode {
                        tag: BLOCK_TAG,
                        offset: 2,
                    },
                    child: AstTreeNode { tag: 2, offset: 4 },
                },
                AstTreeEdge {
                    parent: AstTreeNode {
                        tag: BLOCK_TAG,
                        offset: 2,
                    },
                    child: AstTreeNode {
                        tag: VALDEF_TAG,
                        offset: 5,
                    },
                },
                AstTreeEdge {
                    parent: AstTreeNode {
                        tag: VALDEF_TAG,
                        offset: 5,
                    },
                    child: AstTreeNode { tag: 2, offset: 8 },
                },
            ]
        );
    }

    #[test]
    fn looks_up_ast_parents_and_children_without_losing_child_order() {
        let bytes = [
            APPLY_TAG, 0x87, BLOCK_TAG, 0x85, 2, VALDEF_TAG, 0x82, 0x81, 2,
        ];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let index = nodes
            .deep_address_index_with_source_and_max_depth(&bytes, DEFAULT_MAX_AST_INDEX_DEPTH)
            .unwrap();

        assert_eq!(index.parent_of(0), None);
        assert_eq!(
            index.parent_of(8),
            Some(AstTreeNode {
                tag: VALDEF_TAG,
                offset: 5,
            })
        );
        assert_eq!(
            index.children_of(2).collect::<Vec<_>>(),
            vec![
                AstTreeNode { tag: 2, offset: 4 },
                AstTreeNode {
                    tag: VALDEF_TAG,
                    offset: 5,
                },
            ]
        );
        assert!(index.children_of(4).next().is_none());
        assert!(index.parent_of(u32::MAX).is_none());
    }

    #[test]
    fn shallow_ast_indexes_have_no_structural_edges() {
        let nodes = RawNodes::from_entries(vec![RawNode {
            tag: VALDEF_TAG,
            offset: 0,
            payload: &[],
        }])
        .unwrap();

        let index = nodes.address_index();

        assert!(index.iter_tree_edges().next().is_none());
        assert!(index.children_of(0).next().is_none());
        assert!(index.parent_of(0).is_none());
    }

    #[test]
    fn returns_no_visible_ast_nodes_for_an_empty_address_range() {
        let nodes = RawNodes::from_entries(vec![RawNode {
            tag: VALDEF_TAG,
            offset: 4,
            payload: &[],
        }])
        .unwrap();
        let index = nodes.address_index();

        assert!(index.iter_nodes_in_address_range(4, 4).next().is_none());
    }

    #[test]
    fn indexes_nested_category_five_nodes_with_absolute_ast_addresses() {
        let bytes = [
            APPLY_TAG, 0x87, BLOCK_TAG, 0x85, 2, VALDEF_TAG, 0x82, 0x81, 2,
        ];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();
        let index = nodes
            .deep_address_index_with_source_and_max_depth(&bytes, DEFAULT_MAX_AST_INDEX_DEPTH)
            .unwrap();

        assert_eq!(index.addresses().collect::<Vec<_>>(), vec![0, 2, 5]);
        assert_eq!(
            index.node_addresses().collect::<Vec<_>>(),
            vec![0, 2, 4, 5, 8]
        );
        assert_eq!(index.get(0).map(|node| node.tag), Some(APPLY_TAG));
        assert_eq!(index.get(2).map(|node| node.tag), Some(BLOCK_TAG));
        assert_eq!(
            index.get_node(4),
            Some(crate::AstTreeNode { tag: 2, offset: 4 })
        );
        assert_eq!(
            index
                .resolve(crate::AstRef {
                    kind: crate::AstRefKind::TermRefDirect,
                    address: 5,
                })
                .map(|node| node.tag),
            Some(VALDEF_TAG)
        );
        assert_eq!(
            index
                .resolve_node(crate::AstRef {
                    kind: crate::AstRefKind::SharedTerm,
                    address: 4,
                })
                .map(|node| node.tag),
            Some(2)
        );
        assert!(
            index
                .resolve(crate::AstRef {
                    kind: crate::AstRefKind::SharedTerm,
                    address: 1,
                })
                .is_none()
        );
    }

    #[test]
    fn rejects_ast_index_traversal_that_exceeds_the_configured_depth() {
        let bytes = [
            APPLY_TAG, 0x87, BLOCK_TAG, 0x85, 2, VALDEF_TAG, 0x82, 0x81, 2,
        ];
        let mut reader = Reader::new(&bytes);
        let nodes = RawNodes::decode(&mut reader).unwrap();

        assert_eq!(
            nodes.deep_address_index_with_source_and_max_depth(&bytes, 1),
            Err(AstError::RecursionLimit {
                offset: 2,
                limit: 1,
            })
        );
    }
}
