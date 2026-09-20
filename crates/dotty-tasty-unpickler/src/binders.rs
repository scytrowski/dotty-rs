//! Binder identity: the state that lets a `PARAMtype` refer to a binder that
//! is still being decoded.
//!
//! A binder is the `TypeId` its `Type::TypeLambda` (later `Method`/`Poly`) is
//! stored under, and a `ParamRef` names that id. The id must therefore exist
//! before the binder's children are decoded, since they may contain the
//! `ParamRef`s. The sequence for a `TYPELAMBDAtype` at address `B` is:
//!
//! ```text
//! validate the envelope and parameter names
//! reserve a TypeId B'                 (TypeArena::reserve)
//! record  B -> B' in the index        (publish the address)
//! push    PendingBinder { B, B', .. } (decoding state, below)
//! decode parameter bounds and result  (a PARAMtype to B now resolves to B')
//! fill    B' with Type::TypeLambda    (TypeArena::fill)
//! pop     the pending binder
//! ```
//!
//! Between "record" and "fill" the address entry refers to an *unfilled*
//! arena slot, and reading such a slot panics. That window exists only inside
//! one active decode transaction ([`unpickle_type`](crate::tasty_unpickler::TastyUnpickler::unpickle_type)):
//! on failure the store and the index roll back together, discarding the
//! reservation. Code that runs during the window must not read a binder's
//! slot; it asks [`TastyUnpickler::is_pending`] first, and a `PARAMtype`
//! validates an in-progress binder from its [`PendingBinder`] (kind and arity)
//! rather than from the arena.
//!
//! The pending state is decoding state only. It is not part of the semantic
//! result, is not stored in `dotty-core`, and is empty whenever
//! `unpickle_type` returns.

// The decoders that use this state arrive in the next commits.
#![allow(dead_code)]

use dotty_core::ids::TypeId;

use crate::unpickler::TastyUnpickler;

/// What a pending binder will be once filled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BinderKind {
    TypeLambda,
}

/// A binder whose `TypeId` is reserved and published but not yet filled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingBinder {
    /// The binder's AST address, the key a `PARAMtype` refers to.
    pub(crate) address: u32,
    /// The reserved id: the binder identity a `ParamRef` carries.
    pub(crate) id: TypeId,
    pub(crate) kind: BinderKind,
    /// The number of parameters, for checking a `PARAMtype` index.
    pub(crate) arity: usize,
}

impl TastyUnpickler<'_, '_, '_> {
    /// The pending binder at AST address `address`, if one is being decoded.
    pub(crate) fn pending_at(&self, address: u32) -> Option<PendingBinder> {
        self.pending_binders
            .iter()
            .rev()
            .find(|binder| binder.address == address)
            .copied()
    }

    /// Whether `id` is a reserved binder slot that has not been filled yet.
    /// Such a slot must not be read from the arena.
    pub(crate) fn is_pending(&self, id: TypeId) -> bool {
        self.pending_binders.iter().any(|binder| binder.id == id)
    }

    /// Runs `decode` with `binder` registered as pending, and unregisters it
    /// however `decode` ends, so an error in a child cannot leave the binder
    /// (or a nested one) behind.
    pub(crate) fn with_pending_binder<T>(
        &mut self,
        binder: PendingBinder,
        decode: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let depth = self.pending_binders.len();
        self.pending_binders.push(binder);
        let result = decode(self);
        self.pending_binders.truncate(depth);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::Definitions;
    use dotty_core::store::SemanticStore;
    use dotty_core::types::Type;
    use dotty_tasty::tasty::TastyFile;

    const FILE: &[u8] = include_bytes!("../tests/fixtures/semantic/Plain.tasty");

    fn binder(address: u32, id: TypeId) -> PendingBinder {
        PendingBinder {
            address,
            id,
            kind: BinderKind::TypeLambda,
            arity: 1,
        }
    }

    /// Runs `check` with an unpickler and two type ids to use as binders.
    fn with_unpickler(check: impl FnOnce(&mut TastyUnpickler<'_, '_, '_>, TypeId, TypeId)) {
        let file = TastyFile::parse_scala_3_9(FILE).unwrap();
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let first = store.types.alloc(Type::NoType);
        let second = store.types.alloc(Type::NoType);
        let mut unpickler = TastyUnpickler::new(&file, &mut store, definitions);
        check(&mut unpickler, first, second);
    }

    #[test]
    fn a_pending_binder_is_visible_inside_and_gone_after() {
        with_unpickler(|unpickler, first, second| {
            assert_eq!(unpickler.pending_at(10), None);

            unpickler.with_pending_binder(binder(10, first), |inner| {
                assert_eq!(inner.pending_at(10), Some(binder(10, first)));
                assert!(inner.is_pending(first));
                assert!(!inner.is_pending(second));
            });

            assert_eq!(unpickler.pending_at(10), None);
            assert!(!unpickler.is_pending(first));
        });
    }

    #[test]
    fn an_error_inside_still_unregisters_the_binder() {
        with_unpickler(|unpickler, first, _| {
            let result: Result<(), &str> =
                unpickler.with_pending_binder(binder(10, first), |_| Err("child failed"));

            assert_eq!(result, Err("child failed"));
            assert!(unpickler.pending_binders.is_empty());
        });
    }

    #[test]
    fn a_failed_nested_binder_restores_the_enclosing_stack() {
        with_unpickler(|unpickler, first, second| {
            unpickler.with_pending_binder(binder(10, first), |outer| {
                let failed: Result<(), &str> =
                    outer.with_pending_binder(binder(20, second), |_| Err("inner failed"));

                assert!(failed.is_err());
                // The outer binder is still pending, the inner one is not.
                assert_eq!(outer.pending_at(10), Some(binder(10, first)));
                assert_eq!(outer.pending_at(20), None);
                assert_eq!(outer.pending_binders.len(), 1);
            });
            assert!(unpickler.pending_binders.is_empty());
        });
    }

    #[test]
    fn lookup_by_address_finds_the_binder_at_that_address_not_the_latest() {
        with_unpickler(|unpickler, first, second| {
            unpickler.with_pending_binder(binder(10, first), |outer| {
                outer.with_pending_binder(binder(20, second), |inner| {
                    assert_eq!(inner.pending_at(10).map(|b| b.id), Some(first));
                    assert_eq!(inner.pending_at(20).map(|b| b.id), Some(second));
                });
            });
        });
    }
}
