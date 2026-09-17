//! Constant values shared by AST literals and typed constant references.

use crate::ids::{NameId, TypeId};

/// A compile-time constant value.
///
/// `String` holds an interned [`NameId`] rather than an owned `String` so
/// that AST literals and (later) TASTy constant-pool entries share one
/// string table instead of allocating separately.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Constant {
    Unit,
    Null,
    Boolean(bool),
    Byte(i8),
    Short(i16),
    Char(char),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    String(NameId),
    Class(TypeId),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinguishes_all_numeric_variants_by_value() {
        assert_ne!(Constant::Byte(1), Constant::Byte(2));
        assert_ne!(Constant::Int(1), Constant::Long(1));
        assert_eq!(Constant::Boolean(true), Constant::Boolean(true));
    }

    #[test]
    fn string_and_class_constants_carry_their_id() {
        let name = NameId::new(3);
        let ty = TypeId::new(4);

        assert_eq!(Constant::String(name), Constant::String(name));
        assert_eq!(Constant::Class(ty), Constant::Class(ty));
        assert_ne!(Constant::String(name), Constant::Class(ty));
    }
}
