use crate::binary_name::BinaryName;
use crate::error::ClassLoadError;
use dotty_core::SymbolId;
use std::collections::HashMap;

/// A class's state in a [`ClassRepository`].
///
/// There is no explicit `Missing` variant: a name absent from the
/// repository *is* missing (`docs/classloader.md` §5/§6). `Loading` marks
/// a class whose `SymbolId` has been entered (`dotty-core`'s
/// enter-before-complete rule — see `docs/classloader.md` §5) but whose
/// superclass/interfaces are still being resolved — the state that lets a
/// `ClassLoader` detect circular inheritance instead of recursing
/// forever. Because a `SymbolId` is already a stable, `Copy`, session-wide
/// identity (unlike the `Rc<ClassSymbol>` this replaced), a legitimate
/// mutual member-type reference (two classes each having a field/method
/// typed as the other, `docs/classloader.md` §9 Milestone 6) can reuse the
/// exact same `Loading` entry's `SymbolId` directly, with no shared
/// ownership machinery needed to see it "filled in" once the original
/// load completes — completion happens in the `SemanticStore` itself, via
/// `SymbolInfo::Complete`, not by mutating anything this repository owns.
///
/// `Failed` carries the same `SymbolId` too, when one was already
/// allocated (i.e. the failure happened while this entry was `Loading` —
/// a `NotFound`/`InvalidClassFile`/etc. failure that never reached
/// `enter_class` has none). A caller can then still name the half-built
/// `Symbol` a failed load left behind — see `ClassLoader::load_class`,
/// which marks it `SymbolInfo::Error` right before recording this state.
#[derive(Debug, Clone)]
pub(crate) enum ClassEntry {
    Loading(SymbolId),
    Loaded(SymbolId),
    Failed(Option<SymbolId>, ClassLoadError),
}

impl ClassEntry {
    /// The `SymbolId` this entry is currently associated with, when one
    /// has been allocated — always `Some` for `Loading`/`Loaded`, and for
    /// `Failed` only when the failure happened after `enter_class` had
    /// already run.
    pub(crate) fn symbol(&self) -> Option<SymbolId> {
        match self {
            ClassEntry::Loading(symbol) | ClassEntry::Loaded(symbol) => Some(*symbol),
            ClassEntry::Failed(symbol, _) => *symbol,
        }
    }
}

/// A cache of [`ClassEntry`] state keyed by [`BinaryName`], crate-private
/// per `docs/classloader.md` (only `ClassLoader` drives it).
#[derive(Debug, Default)]
pub(crate) struct ClassRepository {
    entries: HashMap<BinaryName, ClassEntry>,
}

impl ClassRepository {
    pub(crate) fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// The class's current entry, or `None` if it is missing (never seen).
    pub(crate) fn get(&self, name: &BinaryName) -> Option<&ClassEntry> {
        self.entries.get(name)
    }

    pub(crate) fn mark_loading(&mut self, name: BinaryName, symbol: SymbolId) {
        self.entries.insert(name, ClassEntry::Loading(symbol));
    }

    pub(crate) fn mark_loaded(&mut self, name: BinaryName, symbol: SymbolId) {
        self.entries.insert(name, ClassEntry::Loaded(symbol));
    }

    pub(crate) fn mark_failed(
        &mut self,
        name: BinaryName,
        symbol: Option<SymbolId>,
        error: ClassLoadError,
    ) {
        self.entries.insert(name, ClassEntry::Failed(symbol, error));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::{
        Name, Namespace, SemanticStore, Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks,
        SymbolOrigin, Visibility,
    };

    fn some_symbol_id(store: &mut SemanticStore, name: &str) -> SymbolId {
        let text = store.names.intern(name);
        store.symbols.alloc(Symbol {
            name: Name::new(text, Namespace::Type),
            owner: None,
            kind: SymbolKind::Class,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        })
    }

    #[test]
    fn an_unseen_class_is_missing() {
        let repository = ClassRepository::new();
        assert!(
            repository
                .get(&BinaryName::from_internal("Unseen"))
                .is_none()
        );
    }

    #[test]
    fn marks_and_reports_a_class_as_loading() {
        let mut store = SemanticStore::new();
        let mut repository = ClassRepository::new();
        let name = BinaryName::from_internal("Loading");
        let symbol = some_symbol_id(&mut store, "Loading");

        repository.mark_loading(name.clone(), symbol);

        match repository.get(&name) {
            Some(ClassEntry::Loading(entry_symbol)) => assert_eq!(*entry_symbol, symbol),
            other => panic!("expected Loading, got {other:?}"),
        }
    }

    #[test]
    fn marks_and_reports_a_class_as_loaded() {
        let mut store = SemanticStore::new();
        let mut repository = ClassRepository::new();
        let name = BinaryName::from_internal("Loaded");
        let symbol = some_symbol_id(&mut store, "Loaded");

        repository.mark_loaded(name.clone(), symbol);

        match repository.get(&name) {
            Some(ClassEntry::Loaded(entry_symbol)) => assert_eq!(*entry_symbol, symbol),
            other => panic!("expected Loaded, got {other:?}"),
        }
    }

    #[test]
    fn marks_and_reports_a_class_as_failed_and_caches_the_error() {
        let mut repository = ClassRepository::new();
        let name = BinaryName::from_internal("Failed");
        repository.mark_failed(name.clone(), None, ClassLoadError::NotFound(name.clone()));

        match repository.get(&name) {
            Some(ClassEntry::Failed(None, ClassLoadError::NotFound(missing))) => {
                assert_eq!(missing, &name);
            }
            other => panic!("expected Failed(None, NotFound), got {other:?}"),
        }
    }

    #[test]
    fn a_later_state_overwrites_an_earlier_one_for_the_same_name() {
        let mut store = SemanticStore::new();
        let mut repository = ClassRepository::new();
        let name = BinaryName::from_internal("Transitioning");
        let symbol = some_symbol_id(&mut store, "Transitioning");

        repository.mark_loading(name.clone(), symbol);
        repository.mark_failed(
            name.clone(),
            Some(symbol),
            ClassLoadError::CircularInheritance(name.clone()),
        );

        match repository.get(&name) {
            Some(ClassEntry::Failed(entry_symbol, ClassLoadError::CircularInheritance(_))) => {
                assert_eq!(*entry_symbol, Some(symbol));
            }
            other => panic!("expected Failed(Some, CircularInheritance), got {other:?}"),
        }
    }
}
