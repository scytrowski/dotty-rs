//! The session's package identities.
//!
//! One `SemanticStore` has one [`Packages`] registry, shared by every adapter
//! (TASTy unpickler, classfile loader, source frontend) that enters packages
//! into it, so the same package path is the same `SymbolId` whichever adapter
//! saw it first.
//!
//! # Contract
//!
//! - **One symbol per package path.** Dotty models a package as a term symbol
//!   plus a module-class symbol; this core deliberately keeps one
//!   `SymbolKind::Package` symbol, so `TYPEREFpkg` and `TERMREFpkg` share one
//!   identity. `Type::ThisType` may therefore name a package.
//! - **Term namespace.** A package symbol is named in `Namespace::Term`, as
//!   Scala's package values are.
//! - **Explicit root.** The empty path is the root package: a symbol with an
//!   empty name, no owner and its own scope. It is also the unnamed package,
//!   so a top-level class with no package is owned by it. Every named
//!   package's owner chain ends at the root, and each package is declared in
//!   its owner's scope, so it can be found by name from there.
//! - **Segment names.** A package symbol is named after its own segment, not
//!   its full path; the path is recovered by walking `owner`.
//! - **No completion.** A package symbol keeps `SymbolInfo::Missing`.
//! - **Origin of the first adapter.** A shared package keeps the origin of
//!   whoever entered it first.

use std::collections::HashMap;

use crate::ids::{ScopeId, SymbolId};
use crate::names::{Name, Namespace};
use crate::store::SemanticStore;
use crate::symbols::{
    Scope, Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
};

/// A package entered into the store: its symbol and declaration scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnteredPackage {
    pub symbol: SymbolId,
    pub scope: ScopeId,
}

/// The package symbols entered into one `SemanticStore`, by path.
///
/// A registry describes the store it was filled from: using it with another
/// store would hand out ids that mean something else there. It is threaded by
/// value from one adapter to the next, like `Definitions`.
#[derive(Debug, Default)]
pub struct Packages {
    by_path: HashMap<Vec<String>, EnteredPackage>,
    scopes: HashMap<SymbolId, ScopeId>,
    /// Paths in the order they were entered, the root first, so a rollback
    /// can forget the newest ones.
    order: Vec<Vec<String>>,
}

impl Packages {
    pub fn new() -> Self {
        Self::default()
    }

    /// The package named by `path`, if it has been entered. The empty path is
    /// the root package.
    pub fn get<S: AsRef<str>>(&self, path: &[S]) -> Option<EnteredPackage> {
        let key: Vec<String> = path.iter().map(|s| s.as_ref().to_owned()).collect();
        self.by_path.get(&key).copied()
    }

    /// The symbol of the package named by `path`, if it has been entered.
    pub fn symbol<S: AsRef<str>>(&self, path: &[S]) -> Option<SymbolId> {
        self.get(path).map(|package| package.symbol)
    }

    /// The declaration scope of a package symbol this registry entered.
    pub fn scope_of(&self, symbol: SymbolId) -> Option<ScopeId> {
        self.scopes.get(&symbol).copied()
    }

    /// How many named packages have been entered; the root is not counted.
    pub fn len(&self) -> usize {
        self.order.len() - usize::from(!self.order.is_empty())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// A position to [`roll_back_to`](Self::roll_back_to).
    pub fn mark(&self) -> usize {
        self.order.len()
    }

    /// Forgets every package entered after `mark` and takes each one back out
    /// of its owner's scope. Call it before rolling the store back: the
    /// symbols and scopes themselves are freed by the store's own rollback,
    /// but an owner entered before `mark` survives it.
    pub fn roll_back_to(&mut self, store: &mut SemanticStore, mark: usize) {
        while self.order.len() > mark {
            let Some(path) = self.order.pop() else { break };
            let Some(package) = self.by_path.remove(&path) else {
                continue;
            };
            self.scopes.remove(&package.symbol);
            if let Some(parent) = path
                .split_last()
                .and_then(|(_, parent)| self.by_path.get(parent))
            {
                store.scopes.get_mut(parent.scope).remove(package.symbol);
            }
        }
    }

    /// Enters the package for `path`, and the root and every missing
    /// enclosing package before it, and returns the packages on the path from
    /// the outermost one to the last; the empty path yields the root alone.
    ///
    /// New packages carry `origin`; existing ones keep theirs.
    pub fn enter<S: AsRef<str>>(
        &mut self,
        store: &mut SemanticStore,
        origin: SymbolOrigin,
        path: &[S],
    ) -> Vec<EnteredPackage> {
        let path: Vec<&str> = path.iter().map(AsRef::as_ref).collect();
        let mut chain = Vec::with_capacity(path.len());
        let mut owner = self.enter_one(store, origin, &[], None);
        if path.is_empty() {
            chain.push(owner);
            return chain;
        }
        for depth in 1..=path.len() {
            owner = self.enter_one(store, origin, &path[..depth], Some(owner));
            chain.push(owner);
        }
        chain
    }

    fn enter_one(
        &mut self,
        store: &mut SemanticStore,
        origin: SymbolOrigin,
        path: &[&str],
        owner: Option<EnteredPackage>,
    ) -> EnteredPackage {
        if let Some(existing) = self.get(path) {
            return existing;
        }
        let segment = path.last().copied().unwrap_or("");
        let name = Name::new(store.names.intern(segment), Namespace::Term);
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
        if let Some(owner) = owner {
            store.scopes.get_mut(owner.scope).enter(name, symbol);
        }
        let entered = EnteredPackage { symbol, scope };
        let key: Vec<String> = path.iter().map(|s| (*s).to_owned()).collect();
        self.scopes.insert(symbol, scope);
        self.by_path.insert(key.clone(), entered);
        self.order.push(key);
        entered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (SemanticStore, Packages, SymbolOrigin) {
        let mut store = SemanticStore::new();
        let origin = SymbolOrigin::Tasty(store.origins.register_tasty());
        (store, Packages::new(), origin)
    }

    #[test]
    fn a_path_enters_the_root_and_a_package_for_every_segment() {
        let (mut store, mut packages, origin) = setup();

        let chain = packages.enter(&mut store, origin, &["me", "cytrowski"]);

        assert_eq!(chain.len(), 2);
        let leaf = store.symbols.get(chain[1].symbol);
        assert_eq!(leaf.kind, SymbolKind::Package);
        assert_eq!(leaf.owner, Some(chain[0].symbol));
        let outer = store.symbols.get(chain[0].symbol);
        let root = packages.symbol::<&str>(&[]).unwrap();
        assert_eq!(outer.owner, Some(root));
        assert_eq!(store.symbols.get(root).owner, None);
        assert_eq!(store.names.resolve(store.symbols.get(root).name.text()), "");
        assert_eq!(packages.len(), 2);
    }

    #[test]
    fn the_empty_path_is_the_root_which_is_the_unnamed_package() {
        let (mut store, mut packages, origin) = setup();

        let chain = packages.enter::<&str>(&mut store, origin, &[]);

        assert_eq!(chain.len(), 1);
        assert_eq!(packages.symbol::<&str>(&[]), Some(chain[0].symbol));
        assert!(packages.is_empty());
    }

    #[test]
    fn entering_a_path_twice_returns_the_same_identities() {
        let (mut store, mut packages, origin) = setup();

        let first = packages.enter(&mut store, origin, &["a", "b"]);
        let second = packages.enter(&mut store, origin, &["a", "b"]);

        assert_eq!(first, second);
        assert_eq!(packages.len(), 2);
    }

    #[test]
    fn a_longer_path_reuses_the_packages_of_a_shorter_one() {
        let (mut store, mut packages, origin) = setup();

        let short = packages.enter(&mut store, origin, &["a", "b"]);
        let long = packages.enter(&mut store, origin, &["a", "b", "c"]);

        assert_eq!(long[..2], short[..]);
        assert_eq!(
            store.symbols.get(long[2].symbol).owner,
            Some(short[1].symbol)
        );
    }

    #[test]
    fn packages_are_term_named_after_their_segment_and_uncompleted() {
        let (mut store, mut packages, origin) = setup();

        let chain = packages.enter(&mut store, origin, &["java", "util"]);

        let symbol = store.symbols.get(chain[1].symbol);
        assert_eq!(symbol.name.namespace(), Namespace::Term);
        assert_eq!(store.names.resolve(symbol.name.text()), "util");
        assert_eq!(symbol.origin, origin);
        assert_eq!(symbol.info, SymbolInfo::Missing);
    }

    #[test]
    fn each_package_owns_a_scope_and_is_declared_in_its_owners_scope() {
        let (mut store, mut packages, origin) = setup();

        let chain = packages.enter(&mut store, origin, &["a", "b"]);

        let scope = store.scopes.get(chain[1].scope);
        assert_eq!(scope.owner, Some(chain[1].symbol));
        let name = store.symbols.get(chain[1].symbol).name;
        assert_eq!(
            store.scopes.get(chain[0].scope).lookup_all(&name),
            &[chain[1].symbol]
        );
        let outer = store.symbols.get(chain[0].symbol).name;
        let root = packages.get::<&str>(&[]).unwrap();
        assert_eq!(
            store.scopes.get(root.scope).lookup_all(&outer),
            &[chain[0].symbol]
        );
    }

    #[test]
    fn a_shared_package_keeps_the_origin_of_the_first_adapter() {
        let (mut store, mut packages, first) = setup();
        let chain = packages.enter(&mut store, first, &["a"]);

        packages.enter(&mut store, SymbolOrigin::Synthetic, &["a", "b"]);

        assert_eq!(store.symbols.get(chain[0].symbol).origin, first);
    }

    #[test]
    fn the_scope_of_a_package_is_found_from_its_symbol() {
        let (mut store, mut packages, origin) = setup();

        let chain = packages.enter(&mut store, origin, &["a"]);

        assert_eq!(packages.scope_of(chain[0].symbol), Some(chain[0].scope));
        assert_eq!(packages.scope_of(SymbolId::new(999)), None);
    }

    #[test]
    fn rolling_back_forgets_new_packages_and_undeclares_them_from_surviving_owners() {
        let (mut store, mut packages, origin) = setup();
        let kept = packages.enter(&mut store, origin, &["a"]);
        let mark = packages.mark();
        let checkpoint = store.checkpoint();

        let added = packages.enter(&mut store, origin, &["a", "b"]);
        let name = store.symbols.get(added[1].symbol).name;
        packages.roll_back_to(&mut store, mark);
        store.rollback_to(checkpoint);

        assert_eq!(packages.symbol(&["a"]), Some(kept[0].symbol));
        assert_eq!(packages.symbol(&["a", "b"]), None);
        assert!(store.scopes.get(kept[0].scope).lookup_all(&name).is_empty());
        assert_eq!(packages.len(), 1);
        assert_eq!(packages.scope_of(added[1].symbol), None);
    }
}
