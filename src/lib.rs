pub mod ast;
pub mod header;
pub mod name_table;
pub mod reader;
pub mod section;
pub mod term;

pub use ast::{
    AstError, BOUNDED_TAG, DEFDEF_TAG, DefDefBody, DefinitionBody, DefinitionNode, DefinitionTail,
    EMPTYCLAUSE_TAG, EXPORT_TAG, IMPORT_TAG, IMPORTED_TAG, ImportExportKind, ImportExportNode,
    ImportSelector, NodeCategory, PACKAGE_TAG, PARAM_TAG, PackageNode, ParameterBody,
    ParameterNode, RENAMED_TAG, RawNode, RawNodes, SELFDEF_TAG, SPLITCLAUSE_TAG, SelfDefNode,
    TEMPLATE_TAG, TERMREFPKG_TAG, TYPEDEF_TAG, TYPEPARAM_TAG, TemplateNode, TemplateStructure,
    VALDEF_TAG,
};
pub use header::{Header, HeaderError, TASTY_MAGIC};
pub use name_table::{NameRef, NameTable, NameTableError, ParamSig, RawName};
pub use reader::{ReadError, Reader};
pub use section::{
    Attribute, CAPTURECHECKED_ATTR, EXPLICITNULLS_ATTR, JAVA_ATTR, OUTLINE_ATTR,
    SCALA2STANDARDLIBRARY_ATTR, SOURCEFILE_ATTR, Section, SectionError, SectionTable,
    StandardSection, WITHPUREFUNS_ATTR,
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
