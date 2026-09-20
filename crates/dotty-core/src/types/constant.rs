//! Constant values shared by AST literals and typed constant references.

use crate::ids::{NameId, TypeId};

/// A compile-time constant value.
///
/// `String` holds an interned [`NameId`] rather than an owned `String` so
/// that AST literals and (later) TASTy constant-pool entries share one
/// string table instead of allocating separately.
///
/// Every constant TASTy can write is representable exactly. `Char` holds one
/// UTF-16 code unit (a Scala `Char` may be an unpaired surrogate, which is not
/// a Rust `char`), and `FloatBits` / `DoubleBits` hold the IEEE-754 bit
/// pattern, so NaN payloads and the sign of zero survive. Equality is
/// therefore bitwise: `0.0` differs from `-0.0`, and a NaN equals the same
/// NaN. Build floating-point constants with [`Constant::float`] and
/// [`Constant::double`], and read them back with [`Constant::as_float`] and
/// [`Constant::as_double`].
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
    Char(u16),
    Int(i32),
    Long(i64),
    FloatBits(u32),
    DoubleBits(u64),
    String(NameId),
    StringUtf16(Vec<u16>),
    Class(TypeId),
}

impl Constant {
    /// A `Float` constant holding exactly the bits of `value`.
    pub fn float(value: f32) -> Self {
        Self::FloatBits(value.to_bits())
    }

    /// A `Double` constant holding exactly the bits of `value`.
    pub fn double(value: f64) -> Self {
        Self::DoubleBits(value.to_bits())
    }

    /// The value of a `Float` constant, or `None` for any other constant.
    pub fn as_float(&self) -> Option<f32> {
        match self {
            Self::FloatBits(bits) => Some(f32::from_bits(*bits)),
            _ => None,
        }
    }

    /// The value of a `Double` constant, or `None` for any other constant.
    pub fn as_double(&self) -> Option<f64> {
        match self {
            Self::DoubleBits(bits) => Some(f64::from_bits(*bits)),
            _ => None,
        }
    }

    /// A `Char` constant for a Rust scalar value that fits in one UTF-16 code
    /// unit, or `None` for a character outside the Basic Multilingual Plane.
    pub fn char(value: char) -> Option<Self> {
        u16::try_from(u32::from(value)).ok().map(Self::Char)
    }

    /// The scalar value of a `Char` constant, or `None` for another constant
    /// or a lone surrogate code unit, which is not a Rust `char`.
    pub fn as_char(&self) -> Option<char> {
        match self {
            Self::Char(unit) => char::from_u32(u32::from(*unit)),
            _ => None,
        }
    }
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

    #[test]
    fn floating_point_constants_keep_their_exact_bits() {
        assert_eq!(Constant::float(1.5).as_float(), Some(1.5));
        assert_eq!(Constant::double(-2.25).as_double(), Some(-2.25));
        assert_ne!(Constant::float(0.0), Constant::float(-0.0));
        assert_ne!(Constant::double(0.0), Constant::double(-0.0));
        assert!(
            Constant::double(-0.0)
                .as_double()
                .unwrap()
                .is_sign_negative()
        );

        // A NaN with a payload is not the canonical NaN, and equals itself.
        let payload = Constant::DoubleBits(0x7ff8_0000_0000_1234);
        assert_eq!(payload, payload.clone());
        assert_ne!(payload, Constant::double(f64::NAN));
        assert_eq!(
            payload.as_double().unwrap().to_bits(),
            0x7ff8_0000_0000_1234
        );
        let float_payload = Constant::FloatBits(0x7fc0_1234);
        assert_eq!(float_payload.as_float().unwrap().to_bits(), 0x7fc0_1234);
    }

    #[test]
    fn a_char_constant_is_one_utf16_code_unit() {
        assert_eq!(Constant::char('a'), Some(Constant::Char(0x61)));
        assert_eq!(Constant::char('\u{1F600}'), None);
        assert_eq!(Constant::Char(0xD800).as_char(), None);
        assert_eq!(Constant::Char(0xFFFF).as_char(), Some('\u{FFFF}'));
        assert_eq!(Constant::Int(1).as_char(), None);
        assert_eq!(Constant::Int(1).as_float(), None);
        assert_eq!(Constant::Int(1).as_double(), None);
    }
}
