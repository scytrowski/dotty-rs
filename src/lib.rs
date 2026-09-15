pub mod ast;
pub mod header;
pub mod name_table;
pub mod reader;
pub mod section;
pub mod term;

pub use ast::{
    ANDTYPE_TAG, ANNOTATEDTPT_TAG, ANNOTATEDTYPE_TAG, APPLIEDTPT_TAG, APPLIEDTYPE_TAG, APPLY_TAG,
    ASSIGN_TAG, AnnotatedNode, AppliedTypeNode, ApplyNode, AssignNode, AstChildNode, AstError,
    BLOCK_TAG, BOUNDED_TAG, BinaryTypeNode, DEFDEF_TAG, DefDefBody, DefinitionBody, DefinitionNode,
    DefinitionTail, ELIDED_TAG, EMPTYCLAUSE_TAG, EXPORT_TAG, FLEXIBLETYPE_TAG, FlexibleTypeNode,
    IDENT_TAG, IDENTTPT_TAG, IF_TAG, IMPORT_TAG, IMPORTED_TAG, INLINE_TAG, IdentNode, IfNode,
    ImportExportKind, ImportExportNode, ImportSelector, LAMBDA_TAG, LambdaNode, MATCHCASETYPE_TAG,
    NAMEDARG_TAG, NEW_TAG, NamedArgNode, NodeCategory, ORTYPE_TAG, PACKAGE_TAG, PARAM_TAG,
    PARAMTYPE_TAG, POLYTYPE_TAG, PackageNode, ParamTypeNode, ParameterBody, ParameterNode,
    PolyTypeNode, RENAMED_TAG, REPEATED_TAG, RETURN_TAG, RawNode, RawNodes, RepeatedNode,
    ReturnNode, SELECTOUTER_TAG, SELFDEF_TAG, SPLITCLAUSE_TAG, SUPER_TAG, SUPERTYPE_TAG,
    SelectOuterNode, SelfDefNode, SuperNode, TEMPLATE_TAG, TERMREFPKG_TAG, THIS_TAG, THROW_TAG,
    TYPEAPPLY_TAG, TYPEBOUNDS_TAG, TYPEBOUNDSTPT_TAG, TYPED_TAG, TYPEDEF_TAG, TYPELAMBDATYPE_TAG,
    TYPEPARAM_TAG, TemplateNode, TemplateStructure, TypeApplyNode, TypeBoundsNode, TypedNode,
    VALDEF_TAG, WHILE_TAG, WhileNode,
};
pub use header::{Header, HeaderError, TASTY_MAGIC};
pub use name_table::{NameRef, NameTable, NameTableError, ParamSig, RawName};
pub use reader::{ReadError, Reader};
pub use section::{
    Attribute, CAPTURECHECKED_ATTR, Comment, EXPLICITNULLS_ATTR, JAVA_ATTR, OUTLINE_ATTR,
    PositionEntry, PositionSection, SCALA2STANDARDLIBRARY_ATTR, SOURCEFILE_ATTR, Section,
    SectionError, SectionTable, StandardSection, WITHPUREFUNS_ATTR,
};
pub use term::{RawTree, SimpleTerm, TermError, TermValue};

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
