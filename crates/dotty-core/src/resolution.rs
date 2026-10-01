//! The boundary between a format adapter and whatever can supply symbols the
//! adapter has not entered itself.
//!
//! An adapter such as the TASTy unpickler meets references that name a member
//! of a prefix (`Foo#Bar`, `pkg.x`) rather than pointing at a definition it
//! entered. It first looks in the semantic state it already has. When that
//! fails it asks a [`SymbolResolver`], which may know the symbol from
//! somewhere else, typically a classpath the adapter must not read itself.
//!
//! Requests are semantic: a prefix `TypeId`, a namespaced `Name`, a selector.
//! No wire concept (addresses, name-table references, tags, signatures in
//! wire form) appears here, so any adapter can use the port and any loader can
//! implement it.
//!
//! # Contract
//!
//! - `Ok(Some(symbol))` is the one symbol the request names.
//! - `Ok(None)` means *this resolver cannot resolve it*. It is not an error
//!   and does not say the symbol does not exist.
//! - `Err(_)` means the resolver saw something it cannot answer soundly, for
//!   example two candidates for a request that names one. It is never lowered
//!   to `None`.
//! - Resolvers receive the session's mutable store (`&mut SemanticStore`) so
//!   they can materialize canonical semantic state while answering a request.
//!   Each call is atomic: `Ok(None)` and `Err(_)` leave both store and resolver
//!   state unchanged. `Ok(Some(symbol))` may add the state needed to answer.
//! - A caller may wrap several resolver calls in a larger transaction. If
//!   that transaction rolls back, it must restore the resolver to the matching
//!   [`ResolverCheckpoint`] before rolling back the store. Resolver
//!   implementations must journal cached store IDs and remove any mutations
//!   to pre-existing store entries made after that checkpoint. Successful
//!   prior cache entries may remain.
//! - A returned symbol belongs to the supplied store. Repeated successful
//!   resolution of the same semantic target returns its canonical identity.

use std::fmt;

use crate::ids::{SymbolId, TypeId};
use crate::names::Name;
use crate::store::SemanticStore;

/// Which member of the prefix a request names.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MemberSelector {
    /// The single member with the request's name and namespace. If the prefix
    /// has several (overloads), the request is ambiguous, not "the first".
    Unique,
}

/// Where the declaration a request names is looked for.
///
/// A reference has two independent inputs: the `prefix` it is viewed from, and
/// the space that declares the symbol. They coincide for an ordinary
/// reference. Scala's pickler writes them apart (`TYPEREFin` / `TERMREFin`)
/// when the symbol is private or shadowed: the name alone, searched in the
/// prefix, would find another declaration or none.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MemberSpace {
    /// The declaration is a member of the request's `prefix`.
    Prefix,
    /// The declaration is found among the declarations of this type (the
    /// declaring owner), whatever the prefix is. The resolver must not fall
    /// back to searching the prefix.
    Explicit(TypeId),
}

/// A request for a member of `prefix`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberRequest {
    /// The type the reference is viewed from. With [`MemberSpace::Prefix`] it
    /// is also the type whose members are searched, and the resolver decides
    /// which prefixes it understands and answers `Ok(None)` for the rest.
    pub prefix: TypeId,
    pub name: Name,
    pub selector: MemberSelector,
    /// Where the declaration is searched.
    pub space: MemberSpace,
}

/// A resolver could not answer soundly.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResolutionError {
    /// The request names one member but the resolver found several.
    Ambiguous { candidates: usize },
    /// The state the resolver reads is inconsistent.
    Malformed { reason: String },
}

/// Opaque resolver-local transaction mark paired with a store checkpoint.
///
/// The value is owned by the resolver implementation. It is public so
/// object-safe [`SymbolResolver`] implementations can store their own mark
/// without exposing their cache representation to callers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolverCheckpoint(u64);

impl ResolverCheckpoint {
    /// Creates a checkpoint from an implementation-owned monotonically
    /// increasing token.
    pub const fn new(token: u64) -> Self {
        Self(token)
    }

    /// Returns the implementation-owned token used to restore this mark.
    pub const fn token(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ResolutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ambiguous { candidates } => {
                write!(f, "the request names one member but {candidates} match")
            }
            Self::Malformed { reason } => write!(f, "malformed resolver state: {reason}"),
        }
    }
}

impl std::error::Error for ResolutionError {}

/// Supplies symbols an adapter did not enter itself. See the module docs for
/// the contract.
pub trait SymbolResolver {
    /// Records resolver-local cache/session state before a caller transaction.
    fn checkpoint(&self) -> ResolverCheckpoint;

    /// Restores resolver-local state to `checkpoint` before the paired store
    /// rollback. Implementations must discard cached IDs for symbols/types
    /// allocated after the checkpoint and undo changes to surviving store
    /// entries made since it.
    fn rollback_to(&mut self, store: &mut SemanticStore, checkpoint: ResolverCheckpoint);

    /// The member of `request.prefix` the request names.
    ///
    /// Implementations make this call atomic. `Ok(Some(symbol))` may add
    /// canonical semantic state; `Ok(None)` and `Err(_)` leave both the store
    /// and resolver-local state as they were before this call.
    fn resolve_member(
        &mut self,
        store: &mut SemanticStore,
        request: &MemberRequest,
    ) -> Result<Option<SymbolId>, ResolutionError>;

    /// The package named by `path`, outermost segment first, for a package the
    /// adapter has not entered. The same atomicity rules as
    /// [`resolve_member`](Self::resolve_member) apply.
    fn resolve_package(
        &mut self,
        store: &mut SemanticStore,
        path: &[&str],
    ) -> Result<Option<SymbolId>, ResolutionError>;
}

/// The resolver that knows nothing: every request is unresolved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoResolver;

impl SymbolResolver for NoResolver {
    fn checkpoint(&self) -> ResolverCheckpoint {
        ResolverCheckpoint::new(0)
    }

    fn rollback_to(&mut self, _store: &mut SemanticStore, _checkpoint: ResolverCheckpoint) {}

    fn resolve_member(
        &mut self,
        _store: &mut SemanticStore,
        _request: &MemberRequest,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        Ok(None)
    }

    fn resolve_package(
        &mut self,
        _store: &mut SemanticStore,
        _path: &[&str],
    ) -> Result<Option<SymbolId>, ResolutionError> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::Namespace;
    use crate::packages::Packages;
    use crate::symbols::{
        Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
    };
    use std::collections::HashMap;

    #[derive(Default)]
    struct MemberResolver {
        members: HashMap<Name, SymbolId>,
        journal: Vec<Name>,
        fail_after_allocation: bool,
        unresolve_after_allocation: bool,
    }

    impl MemberResolver {
        fn materialize(&mut self, store: &mut SemanticStore, name: Name) -> SymbolId {
            if let Some(symbol) = self.members.get(&name).copied() {
                if store.symbols.contains(symbol) {
                    return symbol;
                }
            }
            let symbol = store.symbols.alloc(Symbol {
                name,
                owner: None,
                kind: SymbolKind::Field,
                flags: SymbolFlags::EMPTY,
                visibility: Visibility::Public,
                info: SymbolInfo::Missing,
                origin: SymbolOrigin::Synthetic,
                annotations: Vec::new(),
                position: None,
                links: SymbolLinks::default(),
            });
            self.members.insert(name, symbol);
            self.journal.push(name);
            symbol
        }
    }

    impl SymbolResolver for MemberResolver {
        fn checkpoint(&self) -> ResolverCheckpoint {
            ResolverCheckpoint::new(self.journal.len() as u64)
        }

        fn rollback_to(&mut self, _store: &mut SemanticStore, checkpoint: ResolverCheckpoint) {
            while self.journal.len() > checkpoint.token() as usize {
                if let Some(name) = self.journal.pop() {
                    self.members.remove(&name);
                }
            }
        }

        fn resolve_member(
            &mut self,
            store: &mut SemanticStore,
            request: &MemberRequest,
        ) -> Result<Option<SymbolId>, ResolutionError> {
            let store_checkpoint = store.checkpoint();
            let resolver_checkpoint = self.checkpoint();
            let symbol = self.materialize(store, request.name);
            if self.fail_after_allocation {
                self.rollback_to(store, resolver_checkpoint);
                store.rollback_to(store_checkpoint);
                return Err(ResolutionError::Malformed {
                    reason: "synthetic materialization failure".to_owned(),
                });
            }
            if self.unresolve_after_allocation {
                self.rollback_to(store, resolver_checkpoint);
                store.rollback_to(store_checkpoint);
                return Ok(None);
            }
            Ok(Some(symbol))
        }

        fn resolve_package(
            &mut self,
            _store: &mut SemanticStore,
            _path: &[&str],
        ) -> Result<Option<SymbolId>, ResolutionError> {
            Ok(None)
        }
    }

    #[derive(Default)]
    struct PackageResolver {
        packages: Packages,
    }

    impl SymbolResolver for PackageResolver {
        fn checkpoint(&self) -> ResolverCheckpoint {
            ResolverCheckpoint::new(self.packages.mark() as u64)
        }

        fn rollback_to(&mut self, store: &mut SemanticStore, checkpoint: ResolverCheckpoint) {
            self.packages
                .roll_back_to(store, checkpoint.token() as usize);
        }

        fn resolve_member(
            &mut self,
            _store: &mut SemanticStore,
            _request: &MemberRequest,
        ) -> Result<Option<SymbolId>, ResolutionError> {
            Ok(None)
        }

        fn resolve_package(
            &mut self,
            store: &mut SemanticStore,
            path: &[&str],
        ) -> Result<Option<SymbolId>, ResolutionError> {
            Ok(self
                .packages
                .enter(store, SymbolOrigin::Synthetic, path)
                .last()
                .map(|package| package.symbol))
        }
    }

    fn request(store: &mut SemanticStore) -> MemberRequest {
        MemberRequest {
            prefix: TypeId::new(0),
            name: Name::new(store.names.intern("Inner"), Namespace::Type),
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        }
    }

    #[test]
    fn the_no_op_resolver_leaves_every_request_unresolved() {
        let mut store = SemanticStore::new();
        let request = request(&mut store);
        let mut resolver = NoResolver;
        let checkpoint = store.checkpoint();

        assert_eq!(resolver.resolve_member(&mut store, &request), Ok(None));
        assert_eq!(resolver.resolve_package(&mut store, &["scala"]), Ok(None));
        assert_eq!(store.checkpoint(), checkpoint);
    }

    #[test]
    fn a_resolver_is_usable_as_a_trait_object() {
        let mut store = SemanticStore::new();
        let request = request(&mut store);
        let mut resolver: Box<dyn SymbolResolver> = Box::new(NoResolver);

        assert_eq!(resolver.resolve_member(&mut store, &request), Ok(None));
    }

    #[test]
    fn an_explicit_space_is_distinct_from_the_prefix_it_accompanies() {
        let prefix = MemberSpace::Prefix;
        let explicit = MemberSpace::Explicit(TypeId::new(0));

        assert_ne!(prefix, explicit);
        assert_ne!(explicit, MemberSpace::Explicit(TypeId::new(1)));
    }

    #[test]
    fn the_no_op_resolver_answers_none_for_an_explicit_space_too() {
        let mut store = SemanticStore::new();
        let mut request = request(&mut store);
        request.space = MemberSpace::Explicit(TypeId::new(1));

        assert_eq!(NoResolver.resolve_member(&mut store, &request), Ok(None));
    }

    #[test]
    fn ambiguity_is_distinct_from_not_found() {
        let ambiguous: Result<Option<SymbolId>, _> =
            Err(ResolutionError::Ambiguous { candidates: 2 });

        assert_ne!(ambiguous, Ok(None));
        assert_eq!(
            ResolutionError::Ambiguous { candidates: 2 }.to_string(),
            "the request names one member but 2 match"
        );
    }

    #[test]
    fn a_mutating_resolver_materializes_a_canonical_member() {
        let mut store = SemanticStore::new();
        let request = request(&mut store);
        let mut resolver: Box<dyn SymbolResolver> = Box::new(MemberResolver::default());

        let first = resolver
            .resolve_member(&mut store, &request)
            .unwrap()
            .unwrap();
        let second = resolver
            .resolve_member(&mut store, &request)
            .unwrap()
            .unwrap();

        assert_eq!(first, second);
        assert!(store.symbols.contains(first));
        assert_eq!(store.symbols.get(first).name, request.name);
    }

    #[test]
    fn an_error_after_materialization_leaves_no_partial_store_or_cache_state() {
        let mut store = SemanticStore::new();
        let request = request(&mut store);
        let mut resolver = MemberResolver {
            fail_after_allocation: true,
            ..MemberResolver::default()
        };
        let checkpoint = store.checkpoint();

        assert!(matches!(
            resolver.resolve_member(&mut store, &request),
            Err(ResolutionError::Malformed { .. })
        ));

        assert_eq!(store.checkpoint(), checkpoint);
        assert!(resolver.members.is_empty());
    }

    #[test]
    fn an_unresolved_result_after_materialization_leaves_no_partial_state() {
        let mut store = SemanticStore::new();
        let request = request(&mut store);
        let mut resolver = MemberResolver {
            unresolve_after_allocation: true,
            ..MemberResolver::default()
        };
        let checkpoint = store.checkpoint();

        assert_eq!(resolver.resolve_member(&mut store, &request), Ok(None));

        assert_eq!(store.checkpoint(), checkpoint);
        assert!(resolver.members.is_empty());
    }

    #[test]
    fn an_outer_rollback_removes_resolver_cache_entries_with_the_store_symbols() {
        let mut store = SemanticStore::new();
        let request = request(&mut store);
        let mut resolver = MemberResolver::default();
        let store_checkpoint = store.checkpoint();
        let resolver_checkpoint = resolver.checkpoint();
        let removed = resolver
            .resolve_member(&mut store, &request)
            .unwrap()
            .unwrap();

        resolver.rollback_to(&mut store, resolver_checkpoint);
        store.rollback_to(store_checkpoint);
        let other_name = Name::new(store.names.intern("other"), Namespace::Term);
        store.symbols.alloc(Symbol {
            name: other_name,
            owner: None,
            kind: SymbolKind::Field,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        assert!(store.symbols.contains(removed));

        let resolved = resolver
            .resolve_member(&mut store, &request)
            .unwrap()
            .unwrap();
        assert_ne!(store.symbols.get(resolved).name, other_name);
        assert_eq!(store.symbols.get(resolved).name, request.name);
    }

    #[test]
    fn package_resolution_is_canonical_and_outer_rollback_undeclares_it() {
        let mut store = SemanticStore::new();
        let mut resolver = PackageResolver::default();
        let root = resolver
            .packages
            .enter::<&str>(&mut store, SymbolOrigin::Synthetic, &[])[0];
        let store_checkpoint = store.checkpoint();
        let resolver_checkpoint = resolver.checkpoint();

        let first = resolver
            .resolve_package(&mut store, &["java", "util"])
            .unwrap()
            .unwrap();
        let second = resolver
            .resolve_package(&mut store, &["java", "util"])
            .unwrap()
            .unwrap();
        assert_eq!(first, second);
        assert!(
            store
                .scopes
                .get(root.scope)
                .lookup(&Name::new(store.names.intern("java"), Namespace::Term,))
                .is_some()
        );

        resolver.rollback_to(&mut store, resolver_checkpoint);
        store.rollback_to(store_checkpoint);

        assert_eq!(resolver.packages.symbol(&["java", "util"]), None);
        assert_eq!(
            store
                .scopes
                .get(root.scope)
                .lookup(&Name::new(store.names.intern("java"), Namespace::Term,)),
            None
        );
    }
}
