//! Opaque, arena-relative identities shared by the semantic model.
//!
//! Every ID here is only meaningful relative to the arena that allocated it
//! (a `SymbolId` from one `SymbolTable` indexed into a different
//! `SymbolTable` is a logic bug, not a type error). Constructors are crate-
//! private; only the owning arena module may mint a new ID.

use std::hash::Hash;
use std::marker::PhantomData;

/// Converts an arena's current length into the raw index for its next
/// allocation.
///
/// Panics rather than silently wrapping if the arena has grown past
/// `u32::MAX` entries: a wrapped index would collide with an
/// already-allocated, unrelated entry and hand out a duplicate ID, which is
/// the same class of internal-invariant violation as an out-of-range `get`
/// (see `docs/dotty-core-design.md` §12's error-handling policy).
pub(crate) fn checked_index(len: usize) -> u32 {
    u32::try_from(len).unwrap_or_else(|_| panic!("arena exceeded u32::MAX entries ({len})"))
}

macro_rules! opaque_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub struct $name(u32);

        impl $name {
            // Unused outside tests until the owning arena (`SymbolTable`,
            // `TypeArena`, `ScopeArena`, `NameInterner`, `AnnotationArena`, or
            // an adapter's origin table) is implemented and starts minting
            // real IDs; see docs/dotty-core-design.md's implementation order.
            #[allow(dead_code)]
            pub(crate) const fn new(raw: u32) -> Self {
                Self(raw)
            }

            /// Returns the raw arena index backing this identity.
            pub const fn index(self) -> u32 {
                self.0
            }
        }
    };
}

opaque_id!(
    /// Identifies a `Symbol` allocated in a `SymbolTable`.
    SymbolId
);

opaque_id!(
    /// Identifies a `Type` allocated in a `TypeArena`.
    TypeId
);

opaque_id!(
    /// Identifies a `Scope` allocated in a `ScopeArena`.
    ScopeId
);

opaque_id!(
    /// Identifies a source file registered with the compilation session.
    SourceId
);

opaque_id!(
    /// Identifies an interned string owned by a `NameInterner`.
    NameId
);

opaque_id!(
    /// Identifies an `Annotation` allocated in an `AnnotationArena`.
    AnnotationId
);

opaque_id!(
    /// Identifies a deferred symbol completion. Opaque in the foundation PR —
    /// no completer engine exists yet; a future namer/typer mints these and
    /// resolves them to a `SymbolInfo::Complete`.
    CompletionId
);

opaque_id!(
    /// Opaque handle for "which classpath entry a classfile-derived symbol
    /// came from." `dotty-core` does not know what a classfile is; the
    /// classfile semantic adapter that constructs `SymbolOrigin::Classfile`
    /// assigns and interprets this ID.
    ClassfileOriginId
);

opaque_id!(
    /// Same role as [`ClassfileOriginId`], for symbols that originated from
    /// a `.tasty` file.
    TastyOriginId
);

/// Identifies a `Tree<P>` allocated in an `AstArena<P>`.
///
/// The phase marker `P` is carried only in the type, not the value — indexing
/// an `AstArena<Untyped>` with a `TreeId<Typed>` (or vice versa) is rejected
/// at compile time. `Clone`/`Copy`/`Debug`/`PartialEq`/`Eq`/`Hash` are
/// implemented by hand rather than derived: deriving on a type containing
/// `PhantomData<P>` would incorrectly require `P` itself to implement those
/// traits, even though the phase marker is never read at runtime.
///
/// `PartialEq` is only implemented for `TreeId<P> == TreeId<P>` (the same
/// `P`), so comparing IDs from different phases does not compile:
///
/// ```compile_fail
/// # use dotty_core::ids::TreeId;
/// enum PhaseA {}
/// enum PhaseB {}
///
/// let a: TreeId<PhaseA> = todo!();
/// let b: TreeId<PhaseB> = todo!();
/// let _ = a == b;
/// ```
pub struct TreeId<P> {
    raw: u32,
    phase: PhantomData<fn() -> P>,
}

impl<P> TreeId<P> {
    // Unused outside tests until `AstArena` is implemented and starts
    // minting real IDs; see docs/dotty-core-design.md's implementation order.
    #[allow(dead_code)]
    pub(crate) const fn new(raw: u32) -> Self {
        Self {
            raw,
            phase: PhantomData,
        }
    }

    /// Returns the raw arena index backing this identity.
    pub const fn index(self) -> u32 {
        self.raw
    }
}

impl<P> Clone for TreeId<P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P> Copy for TreeId<P> {}

impl<P> std::fmt::Debug for TreeId<P> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TreeId")
            .field("raw", &self.raw)
            .finish()
    }
}

impl<P> PartialEq for TreeId<P> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

impl<P> Eq for TreeId<P> {}

impl<P> Hash for TreeId<P> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.raw.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn opaque_ids_round_trip_their_raw_index() {
        let id = SymbolId::new(7);

        assert_eq!(id.index(), 7);
    }

    /// Every opaque ID type is exercised at least once, even before an arena
    /// exists to allocate it, so `new`/`index` are covered and not flagged as
    /// dead code while `symbols`/`types`/`store` are still unimplemented.
    #[test]
    fn every_opaque_id_type_round_trips_its_raw_index() {
        assert_eq!(ScopeId::new(4).index(), 4);
        assert_eq!(SourceId::new(5).index(), 5);
        assert_eq!(AnnotationId::new(6).index(), 6);
        assert_eq!(ClassfileOriginId::new(7).index(), 7);
        assert_eq!(TastyOriginId::new(8).index(), 8);
        assert_eq!(CompletionId::new(9).index(), 9);
    }

    #[test]
    fn opaque_ids_with_different_indexes_are_distinct() {
        assert_ne!(SymbolId::new(1), SymbolId::new(2));
        assert_ne!(NameId::new(1), NameId::new(2));
    }

    #[test]
    fn opaque_ids_are_usable_as_hash_keys() {
        let mut seen = HashSet::new();

        assert!(seen.insert(TypeId::new(0)));
        assert!(seen.insert(TypeId::new(1)));
        assert!(!seen.insert(TypeId::new(0)));
    }

    /// Two marker types standing in for `Untyped`/`Typed`, defined locally so
    /// this test does not depend on the `ast` module existing yet.
    enum PhaseA {}
    enum PhaseB {}

    #[test]
    fn tree_id_round_trips_its_raw_index_regardless_of_phase() {
        let a = TreeId::<PhaseA>::new(3);
        let b = TreeId::<PhaseB>::new(3);

        assert_eq!(a.index(), 3);
        assert_eq!(b.index(), 3);
    }

    #[test]
    fn checked_index_passes_through_a_representable_length() {
        assert_eq!(checked_index(42), 42);
        assert_eq!(checked_index(u32::MAX as usize), u32::MAX);
    }

    #[test]
    #[should_panic(expected = "arena exceeded u32::MAX entries")]
    fn checked_index_panics_instead_of_wrapping_a_length_that_overflows_u32() {
        checked_index(u32::MAX as usize + 1);
    }
}
