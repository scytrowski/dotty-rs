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
/// This does not yet cover Scala's qualified `private[pkg]`/`protected[pkg]`
/// — that needs an access boundary distinct from a plain unqualified
/// `Private`/`Protected`, which JVM classfiles never produce. The variants
/// here are additive, so adding e.g. `PrivateWithin(SymbolId)` /
/// `ProtectedWithin(SymbolId)` later does not require revisiting the JVM
/// lowering that only ever produces `Public`/`Private`/`Protected`/`Package`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Visibility {
    Public,
    Private,
    Protected,
    /// Package-private (JVM's "no access modifier"): visible only within
    /// the given package symbol.
    Package(SymbolId),
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
    fn package_private_is_distinct_from_public() {
        assert_ne!(Visibility::Public, Visibility::Package(SymbolId::new(1)));
    }
}
