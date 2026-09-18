//! Where a symbol came from.

use crate::ids::{ClassfileOriginId, SourceId, TastyOriginId, checked_index};

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

/// Mints fresh [`ClassfileOriginId`]/[`TastyOriginId`] values.
///
/// `ClassfileOriginId`/`TastyOriginId`'s constructors are `pub(crate)` —
/// `dotty-core` does not expose raw-ID construction to external adapters,
/// since a handed-out ID with no allocation behind it could collide with a
/// legitimately-registered one. `OriginTable` is the controlled front door
/// instead: the classfile/TASTy loader calls `register_classfile`/
/// `register_tasty` once per classpath entry (or `.tasty` file) it loads
/// from, keeps the resulting ID as its own key into whatever table it uses
/// to resolve "which entry/file did this ID mean", and wraps it in
/// [`SymbolOrigin::Classfile`]/[`SymbolOrigin::Tasty`]. `dotty-core` never
/// interprets the ID itself, only allocates it.
///
/// One `OriginTable` per compilation session — see [`crate::SemanticStore`],
/// which owns one alongside its other arenas.
#[derive(Debug, Default)]
pub struct OriginTable {
    classfile_count: u32,
    tasty_count: u32,
}

impl OriginTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_classfile(&mut self) -> ClassfileOriginId {
        let id = ClassfileOriginId::new(checked_index(self.classfile_count as usize));
        self.classfile_count += 1;
        id
    }

    pub fn register_tasty(&mut self) -> TastyOriginId {
        let id = TastyOriginId::new(checked_index(self.tasty_count as usize));
        self.tasty_count += 1;
        id
    }
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

    #[test]
    fn origin_table_mints_distinct_classfile_ids() {
        let mut table = OriginTable::new();

        let first = table.register_classfile();
        let second = table.register_classfile();

        assert_ne!(first, second);
    }

    #[test]
    fn origin_table_mints_distinct_tasty_ids() {
        let mut table = OriginTable::new();

        let first = table.register_tasty();
        let second = table.register_tasty();

        assert_ne!(first, second);
    }

    #[test]
    fn classfile_and_tasty_counters_are_independent() {
        let mut table = OriginTable::new();

        let classfile = table.register_classfile();
        let tasty = table.register_tasty();

        // Both counters start at 0, so a classfile and a tasty origin
        // registered first each get raw index 0 — they are still distinct
        // because they are different ID types, not because the counters are
        // shared.
        assert_eq!(classfile.index(), 0);
        assert_eq!(tasty.index(), 0);
    }

    #[test]
    fn a_registered_id_can_be_wrapped_in_a_symbol_origin() {
        let mut table = OriginTable::new();

        let origin = SymbolOrigin::Classfile(table.register_classfile());

        assert_eq!(origin, SymbolOrigin::Classfile(ClassfileOriginId::new(0)));
    }
}
