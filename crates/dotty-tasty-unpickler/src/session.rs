//! State shared while entering multiple TASTy units into one store.

use std::collections::HashMap;

use dotty_core::Packages;
use dotty_core::ids::{ScopeId, SymbolId, TypeId};
use dotty_core::names::Name;

/// Package identities and owner scopes shared by several TASTy units.
///
/// Carry this value from one unpickler to the next with
/// [`TastyUnpickler::with_session`](crate::tasty_unpickler::TastyUnpickler::with_session)
/// and [`TastyUnpickler::into_session_parts`](crate::tasty_unpickler::TastyUnpickler::into_session_parts).
/// Keeping an entered class scope here lets a later unit pair a sibling
/// declaration without forcing that class's `SymbolInfo`.
#[derive(Debug, Default)]
pub struct TastySession {
    pub(crate) packages: Packages,
    pub(crate) owner_scopes: HashMap<SymbolId, ScopeId>,
    pub(crate) scope_order: Vec<SymbolId>,
    /// Opaque implementations waiting for their owner's complete ClassInfo.
    pub(crate) pending_opaque_aliases: Vec<(SymbolId, Name, TypeId)>,
}

impl TastySession {
    pub fn new() -> Self {
        Self::default()
    }

    /// The package registry carried by this session.
    pub fn packages(&self) -> &Packages {
        &self.packages
    }

    /// The previously entered declaration scope for an owner, if it has one.
    pub fn scope_of(&self, owner: SymbolId) -> Option<ScopeId> {
        self.owner_scopes
            .get(&owner)
            .copied()
            .or_else(|| self.packages.scope_of(owner))
    }

    /// The number of non-package owner scopes held for cross-unit lookup.
    pub fn owner_scope_count(&self) -> usize {
        self.owner_scopes.len()
    }
}
