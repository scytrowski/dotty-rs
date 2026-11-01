//! State shared while entering multiple TASTy units into one store.

use std::collections::HashMap;

use dotty_core::Packages;
use dotty_core::ids::{ScopeId, SymbolId, TypeId};
use dotty_core::names::Name;
use dotty_core::resolution::SymbolResolver;

/// Package identities and owner scopes shared by several TASTy units.
///
/// Carry this value from one unpickler to the next with
/// [`TastyUnpickler::with_session`](crate::tasty_unpickler::TastyUnpickler::with_session)
/// and [`TastyUnpickler::into_session_parts`](crate::tasty_unpickler::TastyUnpickler::into_session_parts).
/// Keeping an entered class scope here lets a later unit pair a sibling
/// declaration without forcing that class's `SymbolInfo`.
#[derive(Default)]
pub struct TastySession {
    pub(crate) packages: Packages,
    pub(crate) owner_scopes: HashMap<SymbolId, ScopeId>,
    pub(crate) scope_order: Vec<SymbolId>,
    /// Opaque implementations waiting for their owner's complete ClassInfo.
    pub(crate) pending_opaque_aliases: Vec<(SymbolId, Name, TypeId)>,
    /// Optional resolver whose positive loading state must survive each
    /// active unpickler and final session handoff.
    pub(crate) resolver: Option<Box<dyn SymbolResolver>>,
}

/// A package registry cannot be joined when both adapters already entered a
/// root independently: their IDs may refer to different symbols in the same
/// store, so the session fails closed instead of guessing which one is valid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TastySessionError {
    ConflictingPackageAuthorities,
    ResolverAlreadyAttached,
}

impl std::fmt::Display for TastySessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConflictingPackageAuthorities => {
                f.write_str("both TASTy and resolver sessions already own package registries")
            }
            Self::ResolverAlreadyAttached => f.write_str("a TASTy session already owns a resolver"),
        }
    }
}

impl std::error::Error for TastySessionError {}

impl TastySession {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a TASTy session with package identities handed off by another
    /// adapter. The caller must pass packages created in the same store.
    pub fn with_packages(packages: Packages) -> Self {
        Self {
            packages,
            ..Self::default()
        }
    }

    /// Attaches a resolver to this session when exactly one side owns package
    /// identities. All other resolver state remains owned by it and is carried
    /// through later unpickler handoffs. A second resolver cannot replace the
    /// first one.
    pub fn with_resolver(
        mut self,
        resolver: Box<dyn SymbolResolver>,
    ) -> Result<Self, TastySessionError> {
        if self.resolver.is_some() {
            return Err(TastySessionError::ResolverAlreadyAttached);
        }
        let mut resolver = resolver;
        adopt_resolver_packages(&mut self.packages, resolver.as_mut())?;
        self.resolver = Some(resolver);
        Ok(self)
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

pub(crate) fn adopt_resolver_packages(
    packages: &mut Packages,
    resolver: &mut dyn SymbolResolver,
) -> Result<(), TastySessionError> {
    if packages.symbol::<&str>(&[]).is_some() && resolver.has_package_root() {
        return Err(TastySessionError::ConflictingPackageAuthorities);
    }
    let Some(resolver_packages) = resolver.take_packages() else {
        return Ok(());
    };
    let session_has_root = packages.symbol::<&str>(&[]).is_some();
    let resolver_has_root = resolver_packages.symbol::<&str>(&[]).is_some();
    match (session_has_root, resolver_has_root) {
        (false, true) => *packages = resolver_packages,
        (true, false) | (false, false) => {}
        (true, true) => return Err(TastySessionError::ConflictingPackageAuthorities),
    }
    Ok(())
}
