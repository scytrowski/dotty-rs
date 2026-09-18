//! Package symbols for one TASTy unit.
//!
//! A package is not a single definition node: `package a.b` is one `PACKAGE`
//! node, but it also implies the symbol for `a`, and several nodes may name
//! the same package. Packages are therefore keyed by their path, not by
//! address. The registry is per unpickler; sharing package symbols between
//! units is not handled yet (see the project document).

use std::collections::HashMap;

use dotty_core::ids::SymbolId;
use dotty_core::names::{Name, Namespace};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{
    Scope, Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
};

use crate::error::UnpickleError;
use crate::index::TastySemanticIndex;

/// Package symbols entered so far, by path.
#[derive(Debug, Default)]
pub(crate) struct PackageRegistry {
    by_path: HashMap<Vec<String>, SymbolId>,
}

impl PackageRegistry {
    /// Returns the package symbol for `path`, entering it and every missing
    /// enclosing package first.
    ///
    /// Each new package owns a declaration scope, and is entered into its
    /// parent's scope so it can be found by name from there. The outermost
    /// package has no owner: `Definitions` has no root package to hang it on.
    pub fn enter(
        &mut self,
        store: &mut SemanticStore,
        index: &mut TastySemanticIndex,
        origin: SymbolOrigin,
        path: &[String],
    ) -> Result<SymbolId, UnpickleError> {
        let mut owner: Option<SymbolId> = None;

        for depth in 1..=path.len() {
            let prefix = &path[..depth];
            if let Some(&existing) = self.by_path.get(prefix) {
                owner = Some(existing);
                continue;
            }

            let name = Name::new(store.names.intern(&path[depth - 1]), Namespace::Term);
            let symbol = store.symbols.alloc(Symbol {
                name,
                owner,
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
            if let Some(owner_scope) = owner.and_then(|owner| index.scope_of(owner)) {
                store.scopes.get_mut(owner_scope).enter(name, symbol);
            }

            self.by_path.insert(prefix.to_vec(), symbol);
            owner = Some(symbol);
        }

        // An empty path names no package.
        owner.ok_or(UnpickleError::InvalidNameReference { reference: 0 })
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
        registry: PackageRegistry,
        origin: SymbolOrigin,
    }

    impl Fixture {
        fn new() -> Self {
            let mut store = SemanticStore::new();
            let origin = SymbolOrigin::Tasty(store.origins.register_tasty());
            Self {
                store,
                index: TastySemanticIndex::new(),
                registry: PackageRegistry::default(),
                origin,
            }
        }

        fn enter(&mut self, segments: &[&str]) -> SymbolId {
            self.registry
                .enter(
                    &mut self.store,
                    &mut self.index,
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

        let result =
            fixture
                .registry
                .enter(&mut fixture.store, &mut fixture.index, fixture.origin, &[]);

        assert_eq!(
            result,
            Err(UnpickleError::InvalidNameReference { reference: 0 })
        );
    }
}
