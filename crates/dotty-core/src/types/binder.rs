//! Binder identity for dependent type references.
//!
//! `[BLOCKER 1]` (see `docs/dotty-core-design.md` §8): a binder is not a
//! separate `Binder`/`BinderId` value — it is the [`TypeId`] a
//! `Type::Method`/`Type::Poly`/`Type::TypeLambda`/`Type::Recursive` value is
//! allocated under. `Type::ParamRef { binder, .. }` and `Type::RecThis
//! { binder }` reference that `TypeId` directly.
//!
//! Because a binder's own `TypeId` does not exist yet while its nested
//! `ParamRef`/`RecThis` values are being constructed, `TypeArena` supports a
//! two-phase allocation: [`TypeArena::reserve`] hands out a `TypeId` backed
//! by a placeholder before the real value is known, and
//! [`TypeArena::fill`] overwrites that placeholder once construction
//! completes. [`ReservedTypeId`] is the type-level proof that a `TypeId` came
//! from `reserve` and has not necessarily been filled yet.

use crate::ids::TypeId;

/// A [`TypeId`] reserved by [`TypeArena::reserve`](super::TypeArena::reserve),
/// not yet guaranteed to hold its final value.
///
/// Carries the reserving arena's identity (`arena_id`) so
/// [`TypeArena::fill`](super::TypeArena::fill) can reject a reservation
/// filled in a different arena than the one that issued it — passing it to
/// the wrong arena would otherwise silently overwrite an unrelated slot and
/// invalidate any `ParamRef`/`RecThis` relationship built against it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ReservedTypeId {
    id: TypeId,
    arena_id: u64,
}

impl ReservedTypeId {
    pub(crate) const fn reserved_in(id: TypeId, arena_id: u64) -> Self {
        Self { id, arena_id }
    }

    pub(crate) const fn arena_id(self) -> u64 {
        self.arena_id
    }

    /// The `TypeId` this reservation will resolve to once filled. Usable
    /// immediately as a binder identity in `ParamRef`/`RecThis`, before the
    /// reservation is filled — see the module documentation.
    pub const fn id(self) -> TypeId {
        self.id
    }
}
