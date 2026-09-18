use crate::binary_name::BinaryName;
use dotty_core::{
    Name, Namespace, SemanticStore, Symbol, SymbolFlags, SymbolId, SymbolInfo, SymbolKind,
    SymbolLinks, SymbolOrigin, Visibility,
};
use std::collections::HashMap;

/// A minimal, flat package registry: one stable [`SymbolId`] per full
/// package path (`java/util` is a single `Package` symbol), reused across
/// repeated loads.
///
/// This is deliberately not the full `<root> -> java -> util -> List`
/// owner chain a complete package model would build — each package having
/// its own parent package symbol needs a design for where the root
/// package's identity lives, which is out of scope for this migration
/// step (see `docs/classloader.md`'s packages-and-ownership section).
/// What this *does* guarantee, because `Symbol.owner` and
/// `Visibility::Package` need it now rather than later, is real: a
/// class's owner is a stable, `SymbolId` reused on every repeated load of
/// a class in the same package, not a raw string compared for equality
/// ad hoc at every call site.
pub(crate) struct PackageRegistry {
    packages: HashMap<String, SymbolId>,
}

impl PackageRegistry {
    pub(crate) fn new() -> Self {
        Self {
            packages: HashMap::new(),
        }
    }

    /// Returns `name`'s containing package's `SymbolId`, allocating a
    /// fresh `SymbolKind::Package` symbol in `store` the first time this
    /// package path is seen. A class with no `/` in its binary name (the
    /// unnamed package) gets its own stable symbol too, keyed by `""` —
    /// every class has *some* owning package, even the default one.
    pub(crate) fn resolve(&mut self, store: &mut SemanticStore, name: &BinaryName) -> SymbolId {
        let path = name.package_path();
        if let Some(&id) = self.packages.get(path) {
            return id;
        }

        let text = store.names.intern(path);
        let package_name = Name::new(text, Namespace::Type);
        let id = store.symbols.alloc(Symbol {
            name: package_name,
            owner: None,
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
    fn a_package_symbol_is_named_after_its_full_path() {
        let mut store = SemanticStore::new();
        let mut packages = PackageRegistry::new();

        let id = packages.resolve(&mut store, &BinaryName::from_internal("java/util/List"));

        let symbol = store.symbols.get(id);
        assert_eq!(symbol.kind, SymbolKind::Package);
        assert_eq!(store.names.resolve(symbol.name.text()), "java/util");
    }
}
