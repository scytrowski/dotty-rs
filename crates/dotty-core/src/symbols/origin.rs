//! Where a symbol came from.

use crate::ids::{ClassfileOriginId, SourceId, TastyOriginId};

/// A symbol's provenance.
///
/// The `Classfile`/`Tasty` variants carry an opaque, core-owned origin ID
/// rather than a bare tag — `dotty-core` does not know what a classfile or
/// `.tasty` file is; the adapter that constructs these assigns and resolves
/// the ID against its own classpath-entry or file table. See
/// `docs/dotty-core-design.md` §9, `[MINOR 1]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SymbolOrigin {
    Source(SourceId),
    Classfile(ClassfileOriginId),
    Tasty(TastyOriginId),
    Synthetic,
    Builtin,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_origin_carries_its_source_id() {
        let origin = SymbolOrigin::Source(SourceId::new(1));

        assert_eq!(origin, SymbolOrigin::Source(SourceId::new(1)));
    }

    #[test]
    fn classfile_origin_carries_an_opaque_id() {
        let origin = SymbolOrigin::Classfile(ClassfileOriginId::new(2));

        assert_ne!(origin, SymbolOrigin::Classfile(ClassfileOriginId::new(3)));
    }

    #[test]
    fn tasty_origin_carries_an_opaque_id() {
        let origin = SymbolOrigin::Tasty(TastyOriginId::new(4));

        assert_eq!(origin, SymbolOrigin::Tasty(TastyOriginId::new(4)));
    }

    #[test]
    fn synthetic_and_builtin_are_distinct_unit_variants() {
        assert_ne!(SymbolOrigin::Synthetic, SymbolOrigin::Builtin);
    }
}
