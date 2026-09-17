use crate::binary_name::BinaryName;
use crate::error::ClassLoadError;
use crate::symbol::ClassSymbol;
use std::collections::HashMap;
use std::rc::Rc;

/// A class's state in a [`ClassRepository`].
///
/// There is no explicit `Missing` variant: a name absent from the
/// repository *is* missing (`docs/classloader.md` §5/§6). `Loading` marks
/// a class whose symbol shell has been entered but whose superclass/
/// interfaces are still being resolved — the state that lets a
/// `ClassLoader` detect circular inheritance instead of recursing forever.
#[derive(Debug, Clone)]
pub(crate) enum ClassEntry {
    Loading,
    Loaded(Rc<ClassSymbol>),
    Failed(ClassLoadError),
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

    pub(crate) fn mark_loading(&mut self, name: BinaryName) {
        self.entries.insert(name, ClassEntry::Loading);
    }

    pub(crate) fn mark_loaded(&mut self, name: BinaryName, symbol: Rc<ClassSymbol>) {
        self.entries.insert(name, ClassEntry::Loaded(symbol));
    }

    pub(crate) fn mark_failed(&mut self, name: BinaryName, error: ClassLoadError) {
        self.entries.insert(name, ClassEntry::Failed(error));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_classfile::access_flags::ClassAccessFlags;

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
        let mut repository = ClassRepository::new();
        let name = BinaryName::from_internal("Loading");
        repository.mark_loading(name.clone());

        assert!(matches!(repository.get(&name), Some(ClassEntry::Loading)));
    }

    #[test]
    fn marks_and_reports_a_class_as_loaded() {
        let mut repository = ClassRepository::new();
        let name = BinaryName::from_internal("Loaded");
        let symbol = Rc::new(ClassSymbol::new(
            name.clone(),
            ClassAccessFlags(0x0021),
            None,
            Vec::new(),
            Vec::new(),
        ));

        repository.mark_loaded(name.clone(), symbol.clone());

        match repository.get(&name) {
            Some(ClassEntry::Loaded(loaded)) => assert_eq!(loaded.name(), symbol.name()),
            other => panic!("expected Loaded, got {other:?}"),
        }
    }

    #[test]
    fn marks_and_reports_a_class_as_failed_and_caches_the_error() {
        let mut repository = ClassRepository::new();
        let name = BinaryName::from_internal("Failed");
        repository.mark_failed(name.clone(), ClassLoadError::NotFound(name.clone()));

        match repository.get(&name) {
            Some(ClassEntry::Failed(ClassLoadError::NotFound(missing))) => {
                assert_eq!(missing, &name);
            }
            other => panic!("expected Failed(NotFound), got {other:?}"),
        }
    }

    #[test]
    fn a_later_state_overwrites_an_earlier_one_for_the_same_name() {
        let mut repository = ClassRepository::new();
        let name = BinaryName::from_internal("Transitioning");

        repository.mark_loading(name.clone());
        repository.mark_failed(
            name.clone(),
            ClassLoadError::CircularInheritance(name.clone()),
        );

        assert!(matches!(
            repository.get(&name),
            Some(ClassEntry::Failed(ClassLoadError::CircularInheritance(_)))
        ));
    }
}
