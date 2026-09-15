pub mod ast;
pub mod file;
pub mod header;
pub mod name_table;
pub mod reader;
pub mod section;
pub mod term;
pub mod writer;

pub use ast::{
    ALTERNATIVE_TAG, ANDTYPE_TAG, ANNOTATEDTPT_TAG, ANNOTATEDTYPE_TAG, APPLIEDTPT_TAG,
    APPLIEDTYPE_TAG, APPLY_TAG, APPLYSIGPOLY_TAG, ASSIGN_TAG, AlternativeNode, AnnotatedNode,
    AppliedTypeNode, ApplyNode, ApplySigPolyNode, AssignNode, AstChildNode, AstError, BIND_TAG,
    BLOCK_TAG, BOUNDED_TAG, BYNAMETPT_TAG, BYNAMETYPE_TAG, BinaryTypeNode, BindNode, CASEDEF_TAG,
    CLASSCONST_TAG, CaseDefNode, DEFDEF_TAG, DefDefBody, DefinitionBody, DefinitionNode,
    DefinitionTail, ELIDED_TAG, EMPTYCLAUSE_TAG, EXPLICITTPT_TAG, EXPORT_TAG, EncodedAstNodes,
    FLEXIBLETYPE_TAG, FlexibleTypeNode, IDENT_TAG, IDENTTPT_TAG, IF_TAG, IMPLICIT_TAG,
    IMPLICITARG_TAG, IMPORT_TAG, IMPORTED_TAG, INLINE_TAG, INLINED_TAG, IdentNode, IfNode,
    ImportExportKind, ImportExportNode, ImportSelector, InReferenceNode, LAMBDA_TAG, LAMBDATPT_TAG,
    LambdaNode, LambdaTptNode, MATCH_TAG, MATCHCASETYPE_TAG, MATCHTPT_TAG, MATCHTYPE_TAG,
    METHODTYPE_TAG, MatchTptNode, MatchTypeNode, NAMEDARG_TAG, NEW_TAG, NamedArgNode, NodeCategory,
    ORTYPE_TAG, PACKAGE_TAG, PARAMTYPE_TAG, POLYTYPE_TAG, PRIVATEQUALIFIED_TAG,
    PROTECTEDQUALIFIED_TAG, PackageNode, ParamTypeNode, ParameterBody, ParameterNode, PolyTypeNode,
    QUALTHIS_TAG, QUOTE_TAG, QUOTEPATTERN_TAG, QuoteNode, QuotePatternNode, RECTHIS_TAG,
    RECTYPE_TAG, REFINEDTPT_TAG, REFINEDTYPE_TAG, RENAMED_TAG, REPEATED_TAG, RETURN_TAG, RawNode,
    RawNodes, ReferenceNode, RefinedTptNode, RefinedTypeNode, RepeatedNode, ReturnNode, SELECT_TAG,
    SELECTIN_TAG, SELECTOUTER_TAG, SELECTTPT_TAG, SELFDEF_TAG, SHAREDTERM_TAG, SHAREDTYPE_TAG,
    SINGLETONTPT_TAG, SPLICE_TAG, SPLICEPATTERN_TAG, SPLITCLAUSE_TAG, SUBMATCH_TAG, SUPER_TAG,
    SUPERTYPE_TAG, SelectInNode, SelectNode, SelfDefNode, SplicePatternNode, StructuredNode,
    SuperNode, TEMPLATE_TAG, TERMREF_TAG, TERMREFDIRECT_TAG, TERMREFIN_TAG, TERMREFPKG_TAG,
    TERMREFSYMBOL_TAG, THIS_TAG, THROW_TAG, TRY_TAG, TYPEAPPLY_TAG, TYPEBOUNDS_TAG,
    TYPEBOUNDSTPT_TAG, TYPED_TAG, TYPEDEF_TAG, TYPELAMBDATYPE_TAG, TYPEPARAM_TAG, TYPEREF_TAG,
    TYPEREFDIRECT_TAG, TYPEREFIN_TAG, TYPEREFSYMBOL_TAG, TemplateNode, TemplateStructure,
    TypeApplyNode, TypeBoundsNode, TypedNode, UNAPPLY_TAG, UnapplyNode, VALDEF_TAG, WhileNode,
};
pub use file::{EncodedTastyFile, TastyFile, TastyFileError};
pub use header::{
    Header, HeaderError, SCALA_3_9_EXPERIMENTAL_VERSION, SCALA_3_9_MAJOR_VERSION,
    SCALA_3_9_MINOR_VERSION, TASTY_MAGIC,
};
pub use name_table::{NameRef, NameTable, NameTableBuilder, NameTableError, ParamSig, RawName};
pub use reader::{ReadError, Reader};
pub use section::{
    Attribute, CAPTURECHECKED_ATTR, Comment, EXPLICITNULLS_ATTR, JAVA_ATTR, OUTLINE_ATTR,
    PositionEntry, PositionSection, SCALA2STANDARDLIBRARY_ATTR, SOURCEFILE_ATTR, Section,
    SectionError, SectionTable, StandardSection, WITHPUREFUNS_ATTR,
};
pub use term::{
    AstRef, AstRefKind, DEFAULT_MAX_TREE_DEPTH, RawTree, SimpleTerm, TermEncodeError, TermError,
    TermValue,
};
pub use writer::{WriteError, Writer};

pub fn add(left: u64, right: u64) -> u64 {
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let result = add(2, 2);
        assert_eq!(result, 4);
    }
}
