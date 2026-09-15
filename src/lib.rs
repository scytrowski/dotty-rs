pub mod ast;
pub mod header;
pub mod name_table;
pub mod reader;
pub mod section;
pub mod term;

pub use ast::{
    AstError, DEFDEF_TAG, DefinitionNode, NodeCategory, PACKAGE_TAG, PackageNode, RawNode,
    RawNodes, TERMREFPKG_TAG, TYPEDEF_TAG, VALDEF_TAG,
};
pub use header::{Header, HeaderError, TASTY_MAGIC};
pub use name_table::{NameRef, NameTable, NameTableError, ParamSig, RawName};
pub use reader::{ReadError, Reader};
pub use section::{Section, SectionError, SectionTable, StandardSection};
pub use term::{SimpleTerm, TermError, TermValue};

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
