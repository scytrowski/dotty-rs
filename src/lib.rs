pub mod header;
pub mod name_table;
pub mod reader;

pub use header::{Header, HeaderError, TASTY_MAGIC};
pub use name_table::{NameRef, NameTable, NameTableError, ParamSig, RawName};
pub use reader::{ReadError, Reader};

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
