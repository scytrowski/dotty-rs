//! Orthogonal symbol properties, as a hand-rolled bitset.
//!
//! No `bitflags` dependency: the workspace stays dependency-light by choice
//! (see `docs/dotty-core-design.md` §13), so this is a plain `u64` newtype
//! with associated-constant bits and manual `BitOr`/`BitAnd` impls instead.

use std::ops::{BitAnd, BitOr};

use crate::ast::Modifier;

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
    /// Declaration has no complete concrete implementation.
    ABSTRACT = 0;
    /// Declaration cannot be extended or overridden.
    FINAL = 1;
    /// Direct subclasses are restricted by the source declaration.
    SEALED = 2;
    /// Declaration participates in case-class or case-member semantics.
    CASE = 3;
    /// Declaration carries the legacy implicit marker.
    IMPLICIT = 4;
    /// Declaration supplies a contextual given value.
    GIVEN = 5;
    /// Value initialization is deferred until use.
    LAZY = 6;
    /// Storage can be reassigned after initialization.
    MUTABLE = 7;
    /// Definition carries inline semantics.
    INLINE = 8;
    /// Inline definition retains transparent result typing.
    TRANSPARENT = 9;
    /// Type alias has opaque visibility semantics.
    OPAQUE = 10;
    /// Method belongs to an extension declaration.
    EXTENSION = 11;
    /// Member is static in its binary representation.
    STATIC = 12;
    /// Declaration was introduced by compiler synthesis.
    SYNTHETIC = 13;
    /// Declaration originates from Java metadata.
    JAVA_DEFINED = 14;
    /// Declaration is erased from runtime execution.
    ERASED = 15;
    /// Declaration explicitly overrides an inherited member.
    OVERRIDE = 16;
    /// Declaration represents an enum or enum case identity.
    ENUM = 17;
    /// Member was introduced through an export.
    EXPORTED = 18;
    /// The case-class companion's generated `apply` was deliberately omitted
    /// because this source subset cannot represent its constructor shape.
    CASE_CLASS_APPLY_DEFERRED = 19;
    /// Source primary constructor with no type parameters and a single clause
    /// consisting entirely of required, plain class parameters (no fields).
    PLAIN_PRIMARY_CLAUSE = 20;
}

impl SymbolFlags {
    /// Empty flag set with no declaration properties enabled.
    pub const EMPTY: Self = Self(0);

    /// Whether every bit set in `other` is also set in `self`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Returns the set containing every flag present in either input.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Returns the set containing only flags present in both inputs.
    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// Returns the flags present in this set and absent from the other set.
    pub const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// Returns whether no symbol flags are set.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// Returns the one-to-one symbol flag represented by a source modifier.
///
/// This covers modifiers whose semantic symbol representation is identical
/// across source declaration kinds. Definition-kind markers such as `Enum`
/// and `EnumCase` are intentionally left to the owning frontend because they
/// map to combinations of flags.
pub const fn source_modifier_flag(modifier: Modifier) -> SymbolFlags {
    match modifier {
        Modifier::Abstract => SymbolFlags::ABSTRACT,
        Modifier::Final => SymbolFlags::FINAL,
        Modifier::Sealed => SymbolFlags::SEALED,
        Modifier::Case => SymbolFlags::CASE,
        Modifier::Implicit => SymbolFlags::IMPLICIT,
        Modifier::Given => SymbolFlags::GIVEN,
        Modifier::Lazy => SymbolFlags::LAZY,
        Modifier::Var => SymbolFlags::MUTABLE,
        Modifier::Override => SymbolFlags::OVERRIDE,
        Modifier::Inline => SymbolFlags::INLINE,
        Modifier::Transparent => SymbolFlags::TRANSPARENT,
        Modifier::Opaque => SymbolFlags::OPAQUE,
        Modifier::Extension => SymbolFlags::EXTENSION,
        Modifier::Erased => SymbolFlags::ERASED,
        Modifier::Trait
        | Modifier::Enum
        | Modifier::EnumCase
        | Modifier::PackageObject
        | Modifier::ParamAccessor
        | Modifier::Param
        | Modifier::PrivateLocal
        | Modifier::Update
        | Modifier::Impure
        | Modifier::Open
        | Modifier::Infix
        | Modifier::Tracked
        | Modifier::Into => SymbolFlags::EMPTY,
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
    fn source_modifier_flags_cover_one_to_one_symbol_properties() {
        let cases = [
            (Modifier::Abstract, SymbolFlags::ABSTRACT),
            (Modifier::Final, SymbolFlags::FINAL),
            (Modifier::Sealed, SymbolFlags::SEALED),
            (Modifier::Case, SymbolFlags::CASE),
            (Modifier::Implicit, SymbolFlags::IMPLICIT),
            (Modifier::Given, SymbolFlags::GIVEN),
            (Modifier::Lazy, SymbolFlags::LAZY),
            (Modifier::Var, SymbolFlags::MUTABLE),
            (Modifier::Override, SymbolFlags::OVERRIDE),
            (Modifier::Inline, SymbolFlags::INLINE),
            (Modifier::Transparent, SymbolFlags::TRANSPARENT),
            (Modifier::Opaque, SymbolFlags::OPAQUE),
            (Modifier::Extension, SymbolFlags::EXTENSION),
            (Modifier::Erased, SymbolFlags::ERASED),
        ];

        for (modifier, expected) in cases {
            assert_eq!(source_modifier_flag(modifier), expected, "{modifier:?}");
        }
    }

    #[test]
    fn source_only_markers_do_not_invent_symbol_flags() {
        let markers = [
            Modifier::Trait,
            Modifier::PackageObject,
            Modifier::ParamAccessor,
            Modifier::PrivateLocal,
            Modifier::Update,
            Modifier::Impure,
            Modifier::Open,
            Modifier::Infix,
            Modifier::Tracked,
            Modifier::Into,
        ];

        for modifier in markers {
            assert_eq!(
                source_modifier_flag(modifier),
                SymbolFlags::EMPTY,
                "{modifier:?}"
            );
        }
    }

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

    #[test]
    fn enum_flag_is_distinct_and_composes_with_other_properties() {
        assert_ne!(SymbolFlags::ENUM, SymbolFlags::CASE);
        assert_ne!(SymbolFlags::ENUM, SymbolFlags::EXPORTED);

        let flags = SymbolFlags::ENUM | SymbolFlags::CASE;
        assert!(flags.contains(SymbolFlags::ENUM));
        assert!(flags.contains(SymbolFlags::CASE));
    }

    #[test]
    fn exported_flag_is_distinct_and_composes_with_other_properties() {
        assert_ne!(SymbolFlags::EXPORTED, SymbolFlags::SYNTHETIC);
        assert_ne!(SymbolFlags::EXPORTED, SymbolFlags::ENUM);

        let flags = SymbolFlags::EXPORTED | SymbolFlags::SYNTHETIC;
        assert!(flags.contains(SymbolFlags::EXPORTED));
        assert!(flags.contains(SymbolFlags::SYNTHETIC));
    }
}
