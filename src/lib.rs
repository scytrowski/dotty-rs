pub mod ast;
pub mod header;
pub mod name_table;
pub mod reader;
pub mod section;
pub mod term;

pub use ast::{
    AstError, DEFDEF_TAG, DefinitionBody, DefinitionNode, DefinitionTail, EXPORT_TAG, IMPORT_TAG,
    NodeCategory, PACKAGE_TAG, PARAM_TAG, PackageNode, ParameterBody, ParameterNode, RawNode,
    RawNodes, SELFDEF_TAG, SPLITCLAUSE_TAG, TEMPLATE_TAG, TERMREFPKG_TAG, TYPEDEF_TAG,
    TYPEPARAM_TAG, TemplateNode, TemplateStructure, VALDEF_TAG,
};
pub use header::{Header, HeaderError, TASTY_MAGIC};
pub use name_table::{NameRef, NameTable, NameTableError, ParamSig, RawName};
pub use reader::{ReadError, Reader};
pub use section::{Section, SectionError, SectionTable, StandardSection};
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
