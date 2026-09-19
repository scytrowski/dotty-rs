//! Constant values shared by AST literals and typed constant references.

use crate::ids::{NameId, TypeId};

/// A compile-time constant value.
///
/// `String` holds an interned [`NameId`] rather than an owned `String` so
/// that AST literals and (later) TASTy constant-pool entries share one
/// string table instead of allocating separately.
///
/// [`Constant::StringUtf16`] preserves the UTF-16 code units of a Scala
/// string containing an unpaired surrogate. Rust strings cannot represent
/// those values, so the parser keeps this explicit lossless representation
/// until a later layer chooses how to encode or diagnose it.
#[derive(Clone, Debug, PartialEq)]
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
    StringUtf16(Vec<u16>),
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
        assert_eq!(
            Constant::StringUtf16(vec![0xD800]),
            Constant::StringUtf16(vec![0xD800])
        );
        assert_eq!(Constant::Class(ty), Constant::Class(ty));
        assert_ne!(Constant::String(name), Constant::Class(ty));
    }
}
