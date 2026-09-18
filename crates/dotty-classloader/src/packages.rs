use crate::binary_name::BinaryName;
use dotty_core::{
    Name, Namespace, SemanticStore, Symbol, SymbolFlags, SymbolId, SymbolInfo, SymbolKind,
    SymbolLinks, SymbolOrigin, Visibility,
};
use std::collections::HashMap;

/// A hierarchical package registry: one stable [`SymbolId`] per package
/// *segment* (`java/util` is `<root> -> java -> util`, three symbols, not
/// one), reused across repeated loads.
///
/// Each package symbol's `owner` is its immediately enclosing package —
/// `java/util`'s owner is `java`'s symbol, whose own owner is the root/
/// unnamed-package symbol (`owner: None`) — the same owner-chain shape
/// `docs/classloader.md`'s architecture expects everywhere else. A
/// package symbol's own `name` is just its segment (`"util"`, not
/// `"java/util"`); the full path is only ever reconstructed by walking
/// `owner`, mirroring how `dotty-core`'s `Symbol` already expects class
/// members to be found (`Symbol` doc comment: "owner answers who
/// semantically holds this symbol"). A class with no `/` in its binary
/// name (the unnamed package) is owned directly by the root symbol, which
/// doubles as the unnamed package itself — every class has *some* owning
/// package, even the default one.
///
/// Package symbols carry no `SymbolInfo::Complete` (`info` stays
/// `SymbolInfo::Missing`): unlike a class, a package has no `Type` this
/// crate's model gives a shape to (no `dotty_core::Type` variant models
/// "this is a package's own type"), and no code here needs one — the
/// owner chain alone is enough for `Visibility::Package` and for a
/// class's `Symbol::owner` to be real and stable.
#[derive(Debug, Default)]
pub(crate) struct PackageRegistry {
    packages: HashMap<String, SymbolId>,
}

impl PackageRegistry {
    pub(crate) fn new() -> Self {
        Self {
            packages: HashMap::new(),
        }
    }

    /// Returns `name`'s containing package's `SymbolId` — the deepest
    /// segment, e.g. `util`'s symbol for `java/util/List` — allocating any
    /// not-yet-seen segment along the way, from the root down.
    pub(crate) fn resolve(&mut self, store: &mut SemanticStore, name: &BinaryName) -> SymbolId {
        self.resolve_path(store, name.package_path())
    }

    /// Resolves (allocating as needed) the package symbol for `path`, a
    /// `/`-joined package path (`"java/util"`, or `""` for the root/
    /// unnamed package). Recurses on the parent path first so a package's
    /// `owner` is always already allocated before the package itself is.
    fn resolve_path(&mut self, store: &mut SemanticStore, path: &str) -> SymbolId {
        if let Some(&id) = self.packages.get(path) {
            return id;
        }

        let (owner, segment) = match path.rsplit_once('/') {
            Some((parent, segment)) => (Some(self.resolve_path(store, parent)), segment),
            None if path.is_empty() => (None, ""),
            None => (Some(self.resolve_path(store, "")), path),
        };

        let text = store.names.intern(segment);
        let package_name = Name::new(text, Namespace::Type);
        let id = store.symbols.alloc(Symbol {
            name: package_name,
            owner,
            kind: SymbolKind::Package,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        self.packages.insert(path.to_owned(), id);
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_package_resolves_to_the_same_symbol_id_on_repeated_loads() {
        let mut store = SemanticStore::new();
        let mut packages = PackageRegistry::new();

        let first = packages.resolve(&mut store, &BinaryName::from_internal("java/util/List"));
        let second = packages.resolve(&mut store, &BinaryName::from_internal("java/util/Map"));

        assert_eq!(first, second);
    }

    #[test]
    fn different_packages_get_different_symbol_ids() {
        let mut store = SemanticStore::new();
        let mut packages = PackageRegistry::new();

        let util = packages.resolve(&mut store, &BinaryName::from_internal("java/util/List"));
        let lang = packages.resolve(&mut store, &BinaryName::from_internal("java/lang/Object"));

        assert_ne!(util, lang);
    }

    #[test]
    fn the_unnamed_package_gets_its_own_stable_symbol() {
        let mut store = SemanticStore::new();
        let mut packages = PackageRegistry::new();

        let first = packages.resolve(&mut store, &BinaryName::from_internal("PoolSample"));
        let second = packages.resolve(&mut store, &BinaryName::from_internal("OtherTopLevel"));

        assert_eq!(first, second);
    }

    #[test]
    fn a_package_symbol_is_named_after_its_own_segment_not_its_full_path() {
        let mut store = SemanticStore::new();
        let mut packages = PackageRegistry::new();

        let id = packages.resolve(&mut store, &BinaryName::from_internal("java/util/List"));

        let symbol = store.symbols.get(id);
        assert_eq!(symbol.kind, SymbolKind::Package);
        assert_eq!(store.names.resolve(symbol.name.text()), "util");
    }

    #[test]
    fn a_packages_owner_chain_reconstructs_its_full_path() {
        let mut store = SemanticStore::new();
        let mut packages = PackageRegistry::new();

        let util = packages.resolve(&mut store, &BinaryName::from_internal("java/util/List"));

        let util_symbol = store.symbols.get(util);
        let java = util_symbol.owner.expect("java/util is owned by java");
        let java_symbol = store.symbols.get(java);
        assert_eq!(store.names.resolve(java_symbol.name.text()), "java");

        let root = java_symbol
            .owner
            .expect("java is owned by the root package");
        let root_symbol = store.symbols.get(root);
        assert_eq!(store.names.resolve(root_symbol.name.text()), "");
        assert_eq!(root_symbol.owner, None);
    }

    #[test]
    fn a_top_level_packages_owner_is_the_same_root_as_the_unnamed_package() {
        let mut store = SemanticStore::new();
        let mut packages = PackageRegistry::new();

        let root = packages.resolve(&mut store, &BinaryName::from_internal("PoolSample"));
        // A single-segment package path: `resolve` returns its own symbol
        // directly (unlike `java/lang/Object`, where it would return
        // `lang`'s symbol, one level below `java`).
        let com = packages.resolve(&mut store, &BinaryName::from_internal("com/Thing"));
        let com_symbol = store.symbols.get(com);

        assert_eq!(com_symbol.owner, Some(root));
    }

    #[test]
    fn sibling_packages_share_their_common_ancestor_but_not_each_other() {
        let mut store = SemanticStore::new();
        let mut packages = PackageRegistry::new();

        let util = packages.resolve(&mut store, &BinaryName::from_internal("java/util/List"));
        let lang = packages.resolve(&mut store, &BinaryName::from_internal("java/lang/Object"));

        assert_ne!(util, lang);
        assert_eq!(
            store.symbols.get(util).owner,
            store.symbols.get(lang).owner,
            "java/util and java/lang share the same java owner"
        );
    }
}
