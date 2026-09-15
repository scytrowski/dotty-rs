pub mod ast;
pub mod header;
pub mod name_table;
pub mod reader;
pub mod section;
pub mod term;

pub use ast::{
    APPLY_TAG, ASSIGN_TAG, ApplyNode, AssignNode, AstChildNode, AstError, BLOCK_TAG, BOUNDED_TAG,
    DEFDEF_TAG, DefDefBody, DefinitionBody, DefinitionNode, DefinitionTail, ELIDED_TAG,
    EMPTYCLAUSE_TAG, EXPORT_TAG, IF_TAG, IMPORT_TAG, IMPORTED_TAG, INLINE_TAG, IfNode,
    ImportExportKind, ImportExportNode, ImportSelector, LAMBDA_TAG, LambdaNode, NEW_TAG,
    NodeCategory, PACKAGE_TAG, PARAM_TAG, PackageNode, ParameterBody, ParameterNode, RENAMED_TAG,
    RETURN_TAG, RawNode, RawNodes, ReturnNode, SELFDEF_TAG, SPLITCLAUSE_TAG, SelfDefNode,
    TEMPLATE_TAG, TERMREFPKG_TAG, THIS_TAG, THROW_TAG, TYPEAPPLY_TAG, TYPED_TAG, TYPEDEF_TAG,
    TYPEPARAM_TAG, TemplateNode, TemplateStructure, TypeApplyNode, TypedNode, VALDEF_TAG,
    WHILE_TAG, WhileNode,
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
