//! A symbol's access boundary: who is allowed to refer to it.

use crate::ids::SymbolId;

/// Who can see a [`super::Symbol`].
///
/// This is deliberately a real access-boundary model rather than a second
/// pair of `SymbolFlags` bits: `no PRIVATE and no PROTECTED bit set` cannot
/// tell `public` apart from package-private, which loses real semantic
/// information for JVM members (see `Symbol::visibility`'s doc). `Package`
/// carries the owning package's `SymbolId` rather than a bare tag, so two
/// unrelated package-private symbols in different packages are not treated
/// as sharing a boundary just because both said "package-private".
///
/// Scala's qualified access modifiers get their own variants rather than
/// being widened or narrowed to a plain `Private`/`Protected`:
/// `private[Q]` is [`Visibility::PrivateWithin`] and `protected[Q]` is
/// [`Visibility::ProtectedWithin`], where `Q` is the qualifying package or
/// enclosing class. JVM classfiles never produce these, so the JVM lowering
/// that only yields `Public`/`Private`/`Protected`/`Package` is unaffected.
/// A plain `private`/`protected` stays the unqualified variant, and
/// `private[this]` is an unqualified `Private` (object-private is not a
/// boundary symbol).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Visibility {
    Public,
    Private,
    Protected,
    /// Package-private (JVM's "no access modifier"): visible only within
    /// the given package symbol.
    Package(SymbolId),
    /// `private[Q]`: visible within the qualifying package or class `Q`
    /// (and, for a class, its companion).
    PrivateWithin(SymbolId),
    /// `protected[Q]`: visible within `Q` and to subclasses.
    ProtectedWithin(SymbolId),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_private_and_protected_are_distinct_unit_variants() {
        assert_ne!(Visibility::Public, Visibility::Private);
        assert_ne!(Visibility::Public, Visibility::Protected);
        assert_ne!(Visibility::Private, Visibility::Protected);
    }

    #[test]
    fn package_private_carries_its_owning_package() {
        let a = Visibility::Package(SymbolId::new(1));
        let b = Visibility::Package(SymbolId::new(2));

        assert_ne!(a, b);
        assert_eq!(a, Visibility::Package(SymbolId::new(1)));
    }

    #[test]
    fn a_qualified_access_carries_its_qualifier() {
        let a = Visibility::PrivateWithin(SymbolId::new(1));
        let b = Visibility::PrivateWithin(SymbolId::new(2));
        assert_ne!(a, b);
        assert_eq!(a, Visibility::PrivateWithin(SymbolId::new(1)));
        let c = Visibility::ProtectedWithin(SymbolId::new(1));
        assert_ne!(c, Visibility::ProtectedWithin(SymbolId::new(2)));
    }

    #[test]
    fn qualified_access_is_distinct_from_every_unqualified_variant() {
        let qualifier = SymbolId::new(1);
        let private_within = Visibility::PrivateWithin(qualifier);
        let protected_within = Visibility::ProtectedWithin(qualifier);

        assert_ne!(private_within, protected_within);
        for unqualified in [
            Visibility::Public,
            Visibility::Private,
            Visibility::Protected,
            Visibility::Package(qualifier),
        ] {
            assert_ne!(private_within, unqualified);
            assert_ne!(protected_within, unqualified);
        }
    }

    #[test]
    fn package_private_is_distinct_from_public() {
        assert_ne!(Visibility::Public, Visibility::Package(SymbolId::new(1)));
    }
}
