//! Package symbols, shared between TASTy units.
//!
//! A package is not a single definition node: `package a.b` is one `PACKAGE`
//! node, but it also implies the symbol for `a`, and several nodes, in one
//! unit or in many, may name the same package. Packages are therefore keyed
//! by their path, not by address, and live in a [`TastyPackages`] registry
//! that outlives one unpickler: entering a second unit into the same store
//! with the same registry reuses the package symbols and scopes of the first
//! instead of duplicating `scala.collection` once per file.

use std::collections::HashMap;

use dotty_core::ids::{ScopeId, SymbolId};
use dotty_core::names::{Name, Namespace};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{
    Scope, Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
};

use crate::error::UnpickleError;
use crate::index::TastySemanticIndex;

/// Every declaration made into a scope, in order, so that a failed unit can
/// take its declarations back out of scopes that existed before it.
pub(crate) type ScopeJournal = Vec<(ScopeId, SymbolId)>;

/// Declares `symbol` under `name` in `scope`, journaling the entry.
pub(crate) fn enter_in_scope(
    store: &mut SemanticStore,
    journal: &mut ScopeJournal,
    scope: ScopeId,
    name: Name,
    symbol: SymbolId,
) {
    store.scopes.get_mut(scope).enter(name, symbol);
    journal.push((scope, symbol));
}

/// A package entered into the store.
#[derive(Debug, Clone, Copy)]
struct EnteredPackage {
    symbol: SymbolId,
    scope: ScopeId,
}

/// The package symbols entered into one `SemanticStore`, by path.
///
/// Pass the registry from one unpickler to the next
/// ([`TastyUnpickler::with_packages`](crate::tasty_unpickler::TastyUnpickler::with_packages),
/// [`into_packages`](crate::tasty_unpickler::TastyUnpickler::into_packages))
/// to share packages between units. A registry describes the store it was
/// filled from: using it with another store would hand out ids that mean
/// something else there.
///
/// A shared package keeps the origin of the unit that first entered it.
#[derive(Debug, Default)]
pub struct TastyPackages {
    by_path: HashMap<Vec<String>, EnteredPackage>,
    /// Paths in the order they were entered, so a rollback can forget the
    /// newest ones.
    order: Vec<Vec<String>>,
}

impl TastyPackages {
    pub fn new() -> Self {
        Self::default()
    }

    /// The symbol of the package named by `path`, if it has been entered.
    pub fn symbol(&self, path: &[&str]) -> Option<SymbolId> {
        let key: Vec<String> = path.iter().map(|segment| (*segment).to_owned()).collect();
        self.by_path.get(&key).map(|package| package.symbol)
    }

    /// How many packages have been entered.
    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// A position to [`roll_back_to`](Self::roll_back_to).
    pub(crate) fn mark(&self) -> usize {
        self.order.len()
    }

    /// Forgets every package entered after `mark`. Their symbols and scopes
    /// are freed by the store's own rollback.
    pub(crate) fn roll_back_to(&mut self, mark: usize) {
        while self.order.len() > mark {
            if let Some(path) = self.order.pop() {
                self.by_path.remove(&path);
            }
        }
    }

    /// Returns the package symbol for `path`, entering it and every missing
    /// enclosing package first.
    ///
    /// Each new package owns a declaration scope, and is entered into its
    /// parent's scope so it can be found by name from there. The outermost
    /// package has no owner: `Definitions` has no root package to hang it on.
    /// Every package on the path, new or shared, has its scope recorded in
    /// `index`, so the unit can find it with `scope_of`.
    pub(crate) fn enter(
        &mut self,
        store: &mut SemanticStore,
        index: &mut TastySemanticIndex,
        journal: &mut ScopeJournal,
        origin: SymbolOrigin,
        path: &[String],
    ) -> Result<SymbolId, UnpickleError> {
        let mut owner: Option<EnteredPackage> = None;

        for depth in 1..=path.len() {
            let prefix = &path[..depth];
            if let Some(&existing) = self.by_path.get(prefix) {
                index.share_scope(existing.symbol, existing.scope);
                owner = Some(existing);
                continue;
            }

            let name = Name::new(store.names.intern(&path[depth - 1]), Namespace::Term);
            let symbol = store.symbols.alloc(Symbol {
                name,
                owner: owner.map(|owner| owner.symbol),
                kind: SymbolKind::Package,
                flags: SymbolFlags::EMPTY,
                visibility: Visibility::Public,
                info: SymbolInfo::Missing,
                origin,
                annotations: Vec::new(),
                position: None,
                links: SymbolLinks::default(),
            });
            let scope = store.scopes.alloc(Scope::new(Some(symbol)));
            index.insert_scope(symbol, scope)?;
            if let Some(owner) = owner {
                enter_in_scope(store, journal, owner.scope, name, symbol);
            }

            let entered = EnteredPackage { symbol, scope };
            self.by_path.insert(prefix.to_vec(), entered);
            self.order.push(prefix.to_vec());
            owner = Some(entered);
        }

        // An empty path names no package.
        owner
            .map(|package| package.symbol)
            .ok_or(UnpickleError::InvalidNameReference { reference: 0 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(segments: &[&str]) -> Vec<String> {
        segments
            .iter()
            .map(|segment| (*segment).to_owned())
            .collect()
    }

    struct Fixture {
        store: SemanticStore,
        index: TastySemanticIndex,
        registry: TastyPackages,
        journal: ScopeJournal,
        origin: SymbolOrigin,
    }

    impl Fixture {
        fn new() -> Self {
            let mut store = SemanticStore::new();
            let origin = SymbolOrigin::Tasty(store.origins.register_tasty());
            Self {
                store,
                index: TastySemanticIndex::new(),
                registry: TastyPackages::new(),
                journal: ScopeJournal::new(),
                origin,
            }
        }

        fn enter(&mut self, segments: &[&str]) -> SymbolId {
            self.registry
                .enter(
                    &mut self.store,
                    &mut self.index,
                    &mut self.journal,
                    self.origin,
                    &path(segments),
                )
                .unwrap()
        }
    }

    #[test]
    fn a_path_enters_a_package_for_every_segment() {
        let mut fixture = Fixture::new();

        let leaf = fixture.enter(&["me", "cytrowski"]);

        let leaf_symbol = fixture.store.symbols.get(leaf);
        assert_eq!(leaf_symbol.kind, SymbolKind::Package);
        let parent = leaf_symbol.owner.expect("cytrowski is owned by me");
        assert_eq!(fixture.store.symbols.get(parent).owner, None);
        assert_eq!(
            fixture
                .store
                .names
                .resolve(fixture.store.symbols.get(parent).name.text()),
            "me"
        );
    }

    #[test]
    fn entering_the_same_path_twice_returns_the_same_symbol() {
        let mut fixture = Fixture::new();

        let first = fixture.enter(&["me", "cytrowski"]);
        let second = fixture.enter(&["me", "cytrowski"]);

        assert_eq!(first, second);
    }

    #[test]
    fn a_longer_path_reuses_the_packages_of_a_shorter_one() {
        let mut fixture = Fixture::new();

        let short = fixture.enter(&["me", "cytrowski"]);
        let long = fixture.enter(&["me", "cytrowski", "semantic"]);

        assert_eq!(fixture.store.symbols.get(long).owner, Some(short));
    }

    #[test]
    fn sibling_packages_share_their_parent() {
        let mut fixture = Fixture::new();

        let left = fixture.enter(&["me", "left"]);
        let right = fixture.enter(&["me", "right"]);

        assert_ne!(left, right);
        assert_eq!(
            fixture.store.symbols.get(left).owner,
            fixture.store.symbols.get(right).owner
        );
    }

    #[test]
    fn packages_are_term_named_and_use_the_given_origin() {
        let mut fixture = Fixture::new();

        let package = fixture.enter(&["me"]);

        let symbol = fixture.store.symbols.get(package);
        assert_eq!(symbol.name.namespace(), Namespace::Term);
        assert_eq!(symbol.origin, fixture.origin);
        assert_eq!(symbol.info, SymbolInfo::Missing);
    }

    #[test]
    fn every_package_owns_a_scope_that_records_its_owner() {
        let mut fixture = Fixture::new();

        let package = fixture.enter(&["me"]);

        let scope = fixture.index.scope_of(package).expect("package scope");
        assert_eq!(fixture.store.scopes.get(scope).owner, Some(package));
    }

    #[test]
    fn a_subpackage_is_found_by_name_in_its_parents_scope() {
        let mut fixture = Fixture::new();

        let child = fixture.enter(&["me", "cytrowski"]);

        let parent = fixture.store.symbols.get(child).owner.unwrap();
        let parent_scope = fixture.index.scope_of(parent).unwrap();
        let name = fixture.store.symbols.get(child).name;
        assert_eq!(
            fixture.store.scopes.get(parent_scope).lookup_all(&name),
            &[child]
        );
    }

    #[test]
    fn re_entering_a_package_does_not_duplicate_it_in_its_parents_scope() {
        let mut fixture = Fixture::new();
        let child = fixture.enter(&["me", "cytrowski"]);
        fixture.enter(&["me", "cytrowski"]);

        let parent = fixture.store.symbols.get(child).owner.unwrap();
        let parent_scope = fixture.index.scope_of(parent).unwrap();
        let name = fixture.store.symbols.get(child).name;
        assert_eq!(
            fixture
                .store
                .scopes
                .get(parent_scope)
                .lookup_all(&name)
                .len(),
            1
        );
    }

    #[test]
    fn an_empty_path_names_no_package() {
        let mut fixture = Fixture::new();

        let result = fixture.registry.enter(
            &mut fixture.store,
            &mut fixture.index,
            &mut fixture.journal,
            fixture.origin,
            &[],
        );

        assert_eq!(
            result,
            Err(UnpickleError::InvalidNameReference { reference: 0 })
        );
    }
}
