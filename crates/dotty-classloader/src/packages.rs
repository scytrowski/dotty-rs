use crate::binary_name::BinaryName;
use dotty_core::{Name, Packages, ScopeId, SemanticStore, SymbolId, SymbolOrigin};

/// The classloader's view of the session's package registry.
///
/// Package identities are not the classloader's own: they live in
/// [`dotty_core::Packages`] and follow its contract (one term-named
/// `SymbolKind::Package` symbol per path, an explicit root that is also the
/// unnamed package, each package declared in its owner's scope), so a package
/// entered here and the same path entered by the TASTy unpickler are one
/// `SymbolId`. A class with no `/` in its binary name is owned directly by
/// the root.
#[derive(Debug, Default)]
pub(crate) struct PackageRegistry {
    packages: Packages,
    class_entries: Vec<(ScopeId, SymbolId)>,
}

impl PackageRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Adopts the session-wide registry another adapter has been filling.
    pub(crate) fn from_packages(packages: Packages) -> Self {
        Self {
            packages,
            class_entries: Vec::new(),
        }
    }

    /// Hands the session-wide registry back, to give to the next adapter.
    pub(crate) fn into_packages(self) -> Packages {
        self.packages
    }

    /// Returns the package's declaration scope while this adapter still
    /// owns the shared registry.
    pub(crate) fn scope_of(&self, package: SymbolId) -> Option<ScopeId> {
        self.packages.scope_of(package)
    }

    pub(crate) fn get<S: AsRef<str>>(&self, path: &[S]) -> Option<SymbolId> {
        self.packages.symbol(path)
    }

    pub(crate) fn checkpoint(&self) -> (usize, usize) {
        (self.packages.mark(), self.class_entries.len())
    }

    /// Undoes package-scope links and package entries added after `mark`.
    /// Must run before the matching SemanticStore rollback.
    pub(crate) fn rollback_to(&mut self, store: &mut SemanticStore, mark: (usize, usize)) {
        while self.class_entries.len() > mark.1 {
            let (scope, symbol) = self.class_entries.pop().expect("length checked");
            if store.scopes.contains(scope) {
                store.scopes.get_mut(scope).remove(symbol);
            }
        }
        self.packages.roll_back_to(store, mark.0);
    }

    /// Returns `name`'s containing package's `SymbolId` — the deepest
    /// segment, e.g. `util`'s symbol for `java/util/List` — entering any
    /// not-yet-seen segment along the way, from the root down.
    pub(crate) fn resolve(&mut self, store: &mut SemanticStore, name: &BinaryName) -> SymbolId {
        self.resolve_path(store, name.package_path())
    }

    /// The package symbol for `path`, a `/`-joined package path (`"java/util"`),
    /// entering any missing segment.
    pub(crate) fn resolve_package(&mut self, store: &mut SemanticStore, path: &str) -> SymbolId {
        self.resolve_path(store, path)
    }

    /// Enters a loaded class in its package's name-lookup scope. The
    /// class's owner already identifies the package; this separate entry
    /// makes package members discoverable without scanning all symbols.
    /// Re-entering the same identity is harmless when a package registry is
    /// handed between adapters or loader sessions.
    pub(crate) fn enter_class(
        &mut self,
        store: &mut SemanticStore,
        package: SymbolId,
        name: Name,
        class: SymbolId,
    ) {
        let scope = self
            .scope_of(package)
            .expect("a resolved package has a declaration scope");
        let declarations = store.scopes.get_mut(scope);
        if !declarations.lookup_all(&name).contains(&class) {
            declarations.enter(name, class);
            self.class_entries.push((scope, class));
        }
    }

    /// `path` is a `/`-joined package path, or `""` for the root/unnamed
    /// package.
    fn resolve_path(&mut self, store: &mut SemanticStore, path: &str) -> SymbolId {
        let segments: Vec<&str> = if path.is_empty() {
            Vec::new()
        } else {
            path.split('/').collect()
        };
        self.packages
            .enter(store, SymbolOrigin::Synthetic, &segments)
            .last()
            .map(|package| package.symbol)
            .expect("entering a path yields at least the root")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::{Namespace, SymbolKind};

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

    /// The other half of the shared contract: the same path entered by the
    /// TASTy unpickler and by this registry is one symbol, whichever adapter
    /// entered it first.
    mod convergence_with_the_tasty_unpickler {
        use super::*;
        use dotty_core::Definitions;
        use dotty_tasty::tasty::TastyFile;
        use dotty_tasty_unpickler::tasty_unpickler::TastyUnpickler;

        const DISTINCT: &[u8] =
            include_bytes!("../../dotty-tasty-unpickler/tests/fixtures/semantic/Distinct.tasty");
        const PATH: [&str; 4] = ["me", "cytrowski", "tastyfixtures", "semantic"];
        const SLASHED: &str = "me/cytrowski/tastyfixtures/semantic";

        fn tasty_packages(store: &mut SemanticStore) -> Packages {
            let file = TastyFile::parse_scala_3_9(DISTINCT).unwrap();
            let definitions = Definitions::bootstrap(store);
            let mut unpickler = TastyUnpickler::new(&file, store, definitions);
            unpickler.enter_symbols().unwrap();
            unpickler.into_parts().1
        }

        #[test]
        fn a_package_the_unpickler_entered_is_the_one_this_registry_resolves() {
            let mut store = SemanticStore::new();
            let tasty = tasty_packages(&mut store);
            let expected = tasty.symbol(&PATH).unwrap();
            let entered = tasty.len();

            let mut classfile = PackageRegistry::from_packages(tasty);
            let resolved = classfile.resolve_package(&mut store, SLASHED);

            assert_eq!(resolved, expected);
            assert_eq!(classfile.into_packages().len(), entered);
        }

        #[test]
        fn a_package_the_classloader_entered_is_the_one_the_unpickler_reuses() {
            let mut store = SemanticStore::new();
            let mut classfile = PackageRegistry::new();
            let expected = classfile.resolve_package(&mut store, SLASHED);
            let packages = classfile.into_packages();

            let file = TastyFile::parse_scala_3_9(DISTINCT).unwrap();
            let definitions = Definitions::bootstrap(&mut store);
            let mut unpickler =
                TastyUnpickler::with_packages(&file, &mut store, definitions, packages);
            let index = unpickler.enter_symbols().unwrap();
            let unit_package = index.symbol_at(0).unwrap();
            let packages = unpickler.into_parts().1;

            assert_eq!(unit_package, expected);
            assert_eq!(packages.symbol(&PATH), Some(expected));
            assert_eq!(packages.len(), 4);
        }

        /// A unit with no `package` clause is written against `<empty>`; the
        /// classloader maps a class with no `/` to the root. They are one symbol.
        #[test]
        fn a_default_package_unit_and_a_class_with_no_slash_share_the_root() {
            const WIDGET: &[u8] =
                include_bytes!("../../dotty-tasty-unpickler/tests/fixtures/semantic/Widget.tasty");
            let mut store = SemanticStore::new();
            let file = TastyFile::parse_scala_3_9(WIDGET).unwrap();
            let definitions = Definitions::bootstrap(&mut store);
            let mut unpickler = TastyUnpickler::new(&file, &mut store, definitions);
            let index = unpickler.enter_symbols().unwrap();
            let unit_package = index.symbol_at(0).unwrap();
            let tasty = unpickler.into_parts().1;

            let mut classfile = PackageRegistry::from_packages(tasty);
            let owner_of_widget =
                classfile.resolve(&mut store, &BinaryName::from_internal("Widget"));

            assert_eq!(owner_of_widget, unit_package);
            assert_eq!(store.symbols.get(unit_package).owner, None);
            assert_eq!(classfile.into_packages().len(), 0);
        }

        #[test]
        fn both_adapters_agree_on_the_root_the_namespace_and_the_owner_chain() {
            let mut store = SemanticStore::new();
            let tasty = tasty_packages(&mut store);
            let tasty_leaf = tasty.symbol(&PATH).unwrap();
            let mut classfile = PackageRegistry::new();
            let mut other = SemanticStore::new();
            let class_leaf = classfile.resolve_package(&mut other, SLASHED);

            for (store, leaf) in [(&store, tasty_leaf), (&other, class_leaf)] {
                let mut chain = Vec::new();
                let mut current = Some(leaf);
                while let Some(symbol) = current {
                    let symbol = store.symbols.get(symbol);
                    assert_eq!(symbol.kind, SymbolKind::Package);
                    assert_eq!(symbol.name.namespace(), Namespace::Term);
                    chain.push(store.names.resolve(symbol.name.text()).to_owned());
                    current = symbol.owner;
                }
                chain.reverse();
                assert_eq!(chain, ["", "me", "cytrowski", "tastyfixtures", "semantic"]);
            }
        }
    }
}
