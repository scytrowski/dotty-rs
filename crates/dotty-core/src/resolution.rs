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
//! - Resolvers read the store but do not change it (`&SemanticStore`), so a
//!   failed decode leaves nothing behind for the adapter to undo. A resolver
//!   that must first load symbols does so before the adapter runs, and is
//!   allowed to allocate only once this port is given a transactional
//!   contract.

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

/// A request for a member of `prefix`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberRequest {
    /// The type whose members are searched. The resolver decides which
    /// prefixes it understands and answers `Ok(None)` for the rest.
    pub prefix: TypeId,
    pub name: Name,
    pub selector: MemberSelector,
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
    /// The member of `request.prefix` the request names.
    fn resolve_member(
        &mut self,
        store: &SemanticStore,
        request: &MemberRequest,
    ) -> Result<Option<SymbolId>, ResolutionError>;

    /// The package named by `path`, outermost segment first, for a package the
    /// adapter has not entered.
    fn resolve_package(
        &mut self,
        store: &SemanticStore,
        path: &[&str],
    ) -> Result<Option<SymbolId>, ResolutionError>;
}

/// The resolver that knows nothing: every request is unresolved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoResolver;

impl SymbolResolver for NoResolver {
    fn resolve_member(
        &mut self,
        _store: &SemanticStore,
        _request: &MemberRequest,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        Ok(None)
    }

    fn resolve_package(
        &mut self,
        _store: &SemanticStore,
        _path: &[&str],
    ) -> Result<Option<SymbolId>, ResolutionError> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::Namespace;

    fn request(store: &mut SemanticStore) -> MemberRequest {
        MemberRequest {
            prefix: TypeId::new(0),
            name: Name::new(store.names.intern("Inner"), Namespace::Type),
            selector: MemberSelector::Unique,
        }
    }

    #[test]
    fn the_no_op_resolver_leaves_every_request_unresolved() {
        let mut store = SemanticStore::new();
        let request = request(&mut store);
        let mut resolver = NoResolver;

        assert_eq!(resolver.resolve_member(&store, &request), Ok(None));
        assert_eq!(resolver.resolve_package(&store, &["scala"]), Ok(None));
    }

    #[test]
    fn a_resolver_is_usable_as_a_trait_object() {
        let mut store = SemanticStore::new();
        let request = request(&mut store);
        let mut resolver: Box<dyn SymbolResolver> = Box::new(NoResolver);

        assert_eq!(resolver.resolve_member(&store, &request), Ok(None));
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
}
