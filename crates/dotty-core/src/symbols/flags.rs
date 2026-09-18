//! Orthogonal symbol properties, as a hand-rolled bitset.
//!
//! No `bitflags` dependency: the workspace stays dependency-light by choice
//! (see `docs/dotty-core-design.md` §13), so this is a plain `u64` newtype
//! with associated-constant bits and manual `BitOr`/`BitAnd` impls instead.

use std::ops::{BitAnd, BitOr};

/// A set of orthogonal boolean properties a [`super::Symbol`] can have.
///
/// `kind` ([`super::SymbolKind`]) gives a symbol's stable category; flags
/// give its additional, independently-combinable properties.
///
/// Visibility (`public`/`private`/`protected`/package-private) is **not**
/// one of these flags — "no `PRIVATE` and no `PROTECTED` bit set" cannot
/// distinguish `public` from package-private, which is real semantic
/// information loss for JVM members. `Symbol::visibility` (a
/// [`super::Visibility`]) is the sole source of truth for that instead.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub struct SymbolFlags(u64);

macro_rules! flag_bits {
    ($($(#[$doc:meta])* $name:ident = $bit:expr;)*) => {
        impl SymbolFlags {
            $(
                $(#[$doc])*
                pub const $name: Self = Self(1 << $bit);
            )*
        }
    };
}

flag_bits! {
    ABSTRACT = 0;
    FINAL = 1;
    SEALED = 2;
    CASE = 3;
    IMPLICIT = 4;
    GIVEN = 5;
    LAZY = 6;
    MUTABLE = 7;
    INLINE = 8;
    TRANSPARENT = 9;
    OPAQUE = 10;
    EXTENSION = 11;
    STATIC = 12;
    SYNTHETIC = 13;
    JAVA_DEFINED = 14;
    ERASED = 15;
    OVERRIDE = 16;
}

impl SymbolFlags {
    pub const EMPTY: Self = Self(0);

    /// Whether every bit set in `other` is also set in `self`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    pub const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl BitOr for SymbolFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

impl BitAnd for SymbolFlags {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self {
        self.intersection(rhs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_flags_contain_nothing_but_themselves() {
        assert!(SymbolFlags::EMPTY.is_empty());
        assert!(SymbolFlags::EMPTY.contains(SymbolFlags::EMPTY));
        assert!(!SymbolFlags::EMPTY.contains(SymbolFlags::FINAL));
    }

    #[test]
    fn union_combines_independent_flags() {
        let combined = SymbolFlags::FINAL | SymbolFlags::ABSTRACT;

        assert!(combined.contains(SymbolFlags::FINAL));
        assert!(combined.contains(SymbolFlags::ABSTRACT));
        assert!(!combined.contains(SymbolFlags::SEALED));
    }

    #[test]
    fn intersection_keeps_only_shared_flags() {
        let a = SymbolFlags::FINAL | SymbolFlags::ABSTRACT;
        let b = SymbolFlags::FINAL | SymbolFlags::SEALED;

        assert_eq!(a & b, SymbolFlags::FINAL);
    }

    #[test]
    fn difference_removes_the_given_flags() {
        let combined = SymbolFlags::FINAL | SymbolFlags::ABSTRACT;

        assert_eq!(
            combined.difference(SymbolFlags::ABSTRACT),
            SymbolFlags::FINAL
        );
    }
}
