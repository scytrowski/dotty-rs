//! Borrowed and owned codecs for the Scala 3.9.0 TASTy format.
//!
//! The crate is organized into four layers:
//!
//! - [`header`] and [`reader`] / [`writer`] expose the binary primitives;
//! - [`name_table`] and [`section`] decode the file-level tables;
//! - [`term`] and [`ast`] expose raw and structured tree representations;
//! - [`mod@file`] composes those pieces into the borrowed [`TastyFile`] and the
//!   owned [`TastyFileBuilder`].
//!
//! Parsed files borrow their input bytes. Use [`TastyFile::parse`] or one of
//! its validating variants for read-only inspection. Use [`TastyFileBuilder`]
//! and the owned encoded section types when constructing a new file. Raw
//! nodes and sections remain available when a semantic interpretation is not
//! implemented. The codec targets TASTy 3.9.0 (format 28.9.0); compatibility
//! with another format must be requested explicitly through the compatible
//! APIs.

/// Raw and structured AST nodes, tags, references, and AST indexes.
pub mod ast;
/// File-level parsing, validation, encoding, and AST queries.
pub mod file;
/// TASTy header parsing and version validation.
pub mod header;
/// One-based TASTy names, compound names, and name-table builders.
pub mod name_table;
/// Bounded zero-copy readers for TASTy binary values.
pub mod reader;
/// Standard and raw TASTy sections and their encoded counterparts.
pub mod section;
/// Raw terms, constants, and length-delimited tree nodes.
pub mod term;
/// Binary writers for TASTy values and bounded payloads.
pub mod writer;

/// Scala 3 TASTy encoding and structural APIs.
pub mod tasty {
    pub use super::ast;
    pub use super::ast::*;
    pub use super::file;
    pub use super::file::*;
    pub use super::header;
    pub use super::header::*;
    pub use super::name_table;
    pub use super::name_table::*;
    pub use super::reader;
    pub use super::reader::*;
    pub use super::section;
    pub use super::section::*;
    pub use super::term;
    pub use super::term::*;
    pub use super::writer;
    pub use super::writer::*;
}

pub use ast::{
    ABSTRACT_TAG, ALTERNATIVE_TAG, ANDTYPE_TAG, ANNOTATEDTPT_TAG, ANNOTATEDTYPE_TAG,
    ANNOTATION_TAG, APPLIEDTPT_TAG, APPLIEDTYPE_TAG, APPLY_TAG, APPLYSIGPOLY_TAG, ARTIFACT_TAG,
    ASSIGN_TAG, AlternativeNode, AnnotatedNode, AnnotationNode, AppliedTypeNode, ApplyNode,
    ApplySigPolyNode, AssignNode, AstAddressIndex, AstChildNode, AstError, AstReference,
    AstTreeEdge, BIND_TAG, BLOCK_TAG, BOUNDED_TAG, BYNAMETPT_TAG, BYNAMETYPE_TAG, BinaryTypeNode,
    BindBody, BindNode, CASE_TAG, CASEACCESSOR_TAG, CASEDEF_TAG, CLASSCONST_TAG, CONTRAVARIANT_TAG,
    COVARIANT_TAG, CaseDefNode, ClassConstNode, DEFAULT_MAX_AST_INDEX_DEPTH, DEFDEF_TAG,
    DefDefBody, DefDefHeaderItem, DefinitionBody, DefinitionNode, DefinitionTail, ELIDED_TAG,
    EMPTYCLAUSE_TAG, ENUM_TAG, ERASED_TAG, EXPLICITTPT_TAG, EXPORT_TAG, EXPORTED_TAG,
    EXTENSION_TAG, EncodedAstNodes, FIELDACCESSOR_TAG, FINAL_TAG, FLEXIBLETYPE_TAG,
    FlexibleTypeNode, GIVEN_TAG, HASDEFAULT_TAG, HOLE_TAG, HoleNode, IDENT_TAG, IDENTTPT_TAG,
    IF_TAG, IMPLICIT_TAG, IMPLICITARG_TAG, IMPORT_TAG, IMPORTED_TAG, INFIX_TAG, INLINE_TAG,
    INLINED_TAG, INLINEPROXY_TAG, INTO_TAG, INVISIBLE_TAG, IdentNode, IfNode, ImportExportKind,
    ImportExportNode, ImportSelector, InReferenceNode, LAMBDA_TAG, LAMBDATPT_TAG, LAZY_TAG,
    LOCAL_TAG, LambdaNode, LambdaTptNode, MACRO_TAG, MATCH_TAG, MATCHCASETYPE_TAG, MATCHTPT_TAG,
    MATCHTYPE_TAG, METHODTYPE_TAG, MUTABLE_TAG, MatchTptNode, MatchTypeNode, NAMEDARG_TAG, NEW_TAG,
    NameReference, NamedArgNode, NodeCategory, OBJECT_TAG, OPAQUE_TAG, OPEN_TAG, ORTYPE_TAG,
    OVERRIDE_TAG, PACKAGE_TAG, PARAM_TAG, PARAMALIAS_TAG, PARAMSETTER_TAG, PARAMTYPE_TAG,
    POLYTYPE_TAG, PRIVATE_TAG, PRIVATEQUALIFIED_TAG, PROTECTED_TAG, PROTECTEDQUALIFIED_TAG,
    PackageNode, ParamTypeNode, ParameterBody, ParameterNode, PolyTypeNode, QUALTHIS_TAG,
    QUOTE_TAG, QUOTEPATTERN_TAG, QuoteNode, QuotePatternNode, RECTHIS_TAG, RECTYPE_TAG,
    REFINEDTPT_TAG, REFINEDTYPE_TAG, RENAMED_TAG, REPEATED_TAG, RETURN_TAG, RawNode, RawNodes,
    ReferenceNode, RefinedTptNode, RefinedTypeNode, RepeatedNode, ReturnNode, SEALED_TAG,
    SELECT_TAG, SELECTIN_TAG, SELECTOUTER_TAG, SELECTTPT_TAG, SELFDEF_TAG, SHAREDTERM_TAG,
    SHAREDTYPE_TAG, SINGLETONTPT_TAG, SPLICE_TAG, SPLICEPATTERN_TAG, SPLITCLAUSE_TAG, STABLE_TAG,
    STATIC_TAG, SUBMATCH_TAG, SUPER_TAG, SUPERTYPE_TAG, SYNTHETIC_TAG, SelectInNode, SelectNode,
    SelfDefNode, SplicePatternNode, StructuredNode, StructuredTree, SuperNode, TEMPLATE_TAG,
    TERMREF_TAG, TERMREFDIRECT_TAG, TERMREFIN_TAG, TERMREFPKG_TAG, TERMREFSYMBOL_TAG, THIS_TAG,
    THROW_TAG, TRACKED_TAG, TRAIT_TAG, TRANSPARENT_TAG, TRY_TAG, TYPEAPPLY_TAG, TYPEBOUNDS_TAG,
    TYPEBOUNDSTPT_TAG, TYPED_TAG, TYPEDEF_TAG, TYPELAMBDATYPE_TAG, TYPEPARAM_TAG, TYPEREF_TAG,
    TYPEREFDIRECT_TAG, TYPEREFIN_TAG, TYPEREFPKG_TAG, TYPEREFSYMBOL_TAG, TemplateNode,
    TemplateStructure, TypeApplyNode, TypeBoundsNode, TypedNode, UNAPPLY_TAG, UnapplyNode,
    VALDEF_TAG, WHILE_TAG, WhileNode,
};
pub use file::{AstTreePosition, EncodedTastyFile, TastyFile, TastyFileBuilder, TastyFileError};
pub use header::{
    Header, HeaderError, SCALA_3_9_EXPERIMENTAL_VERSION, SCALA_3_9_MAJOR_VERSION,
    SCALA_3_9_MINOR_VERSION, TASTY_MAGIC,
};
pub use name_table::{
    NameRef, NameRenderError, NameSignature, NameTable, NameTableBuilder, NameTableError, ParamSig,
    ParamSigValue, RawName, RawNameKind, RenderedNameSignature, RenderedParamSig,
    RenderedSignedName, SignedName, interpret_param_sig,
};
pub use reader::{ReadError, Reader};
pub use section::{
    Attribute, CAPTURECHECKED_ATTR, Comment, EXPLICITNULLS_ATTR, EncodedAstSection, EncodedSection,
    JAVA_ATTR, OUTLINE_ATTR, PositionCoordinate, PositionEntry, PositionSection, ResolvedPosition,
    ResolvedPositionEntry, SCALA2STANDARDLIBRARY_ATTR, SOURCEFILE_ATTR, Section, SectionError,
    SectionTable, StandardSection, WITHPUREFUNS_ATTR,
};
pub use term::{
    AstRef, AstRefKind, AstTreeNode, BYTECONST_TAG, CHARCONST_TAG, ConstantValue,
    DEFAULT_MAX_TREE_DEPTH, DOUBLECONST_TAG, FALSECONST_TAG, FLOATCONST_TAG, INTCONST_TAG,
    LONGCONST_TAG, NULLCONST_TAG, RawTree, SHORTCONST_TAG, STRINGCONST_TAG, SimpleTerm,
    TRUECONST_TAG, TermEncodeError, TermError, TermValue, UNITCONST_TAG,
};
pub use writer::{WriteError, Writer};
