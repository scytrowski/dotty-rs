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

/// A rejected resolver attachment leaves the existing owner untouched and
/// returns both it and the incoming resolver so their state remains
/// recoverable.
pub struct ResolverAttachmentFailure<T> {
    reason: TastySessionError,
    owner: Box<T>,
    resolver: Box<dyn SymbolResolver>,
}

impl<T> ResolverAttachmentFailure<T> {
    pub(crate) fn new(
        reason: TastySessionError,
        owner: T,
        resolver: Box<dyn SymbolResolver>,
    ) -> Self {
        Self {
            reason,
            owner: Box::new(owner),
            resolver,
        }
    }

    /// The reason the resolver could not be attached.
    pub fn reason(&self) -> TastySessionError {
        self.reason
    }

    /// Recovers the unchanged owner and resolver passed to the rejected
    /// attachment.
    pub fn into_parts(self) -> (T, Box<dyn SymbolResolver>) {
        (*self.owner, self.resolver)
    }
}

impl<T> std::fmt::Debug for ResolverAttachmentFailure<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolverAttachmentFailure")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

impl<T> std::fmt::Display for ResolverAttachmentFailure<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.reason.fmt(f)
    }
}

impl<T: 'static> std::error::Error for ResolverAttachmentFailure<T> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.reason)
    }
}

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
    /// first one. If attachment fails, the error returns this unchanged session
    /// and the incoming resolver through [`ResolverAttachmentFailure::into_parts`].
    pub fn with_resolver(
        mut self,
        mut resolver: Box<dyn SymbolResolver>,
    ) -> Result<Self, ResolverAttachmentFailure<Self>> {
        if self.resolver.is_some() {
            return Err(ResolverAttachmentFailure::new(
                TastySessionError::ResolverAlreadyAttached,
                self,
                resolver,
            ));
        }
        if let Err(reason) = adopt_resolver_packages(&mut self.packages, resolver.as_mut()) {
            return Err(ResolverAttachmentFailure::new(reason, self, resolver));
        }
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
        // `has_package_root` is the preflight part of the transfer protocol.
        // A resolver returning a rooted registry after reporting no root is a
        // contract violation; there is no sound way to merge the two stores.
        (true, true) => unreachable!("resolver package-root preflight was inconsistent"),
    }
    Ok(())
}
