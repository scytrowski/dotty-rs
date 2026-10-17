use crate::binary_name::BinaryName;
use crate::class_path::ClassOrigin;
use crate::packages::PackageRegistry;
use crate::symbol::ClassfileMetadata;
use dotty_core::{Symbol, SymbolId};
use std::collections::HashMap;

/// The `SymbolId`-allocating state that must be shared, by value, across
/// every [`ClassLoader`](crate::loader::ClassLoader) loading against one
/// [`SemanticStore`](dotty_core::SemanticStore) session — which class names
/// have a canonical identity (`identified`), or have been fully loaded and
/// completed (`resolved`), the `PackageRegistry` (which package paths have already
/// been resolved to which package `SymbolId`), and each resolved symbol's
/// own [`ClassfileMetadata`]/[`ClassOrigin`] sidecar data.
///
/// `Definitions` (`ClassLoader::with_definitions`) already solved this for
/// the builtin `Object`/`Any`/`Nothing`/primitive identities — bootstrap
/// once, then pass the same value to every loader. `LoadingSession` extends
/// that to *ordinary* classes and packages: two `ClassLoader`s constructed
/// sequentially over the same `SemanticStore` (the documented pattern —
/// one per classpath root, or one per incremental recompilation unit, see
/// `ClassLoader::new`'s doc comment) must agree on the `SymbolId` for e.g.
/// `java/util/List` and its `java/util` package the same way they already
/// agree on `Object`'s — otherwise `TypeRef` equality and any later
/// AST/Symbol lookup would depend on which loader happened to load a given
/// class first. `metadata`/`origins` extend that same reasoning to
/// provenance: a `SymbolId` a later loader picks up from `resolved` (rather
/// than loading it itself) must still resolve to real `ClassLoader::metadata`/
/// `ClassLoader::origin` lookups, not silently go `None` just because a
/// *different* loader instance was the one that actually populated them —
/// unlike `resolved` itself, these are keyed by the already-resolved
/// `SymbolId`, not by a classpath-dependent `BinaryName`, so sharing them
/// carries none of the negative-caching risk `ClassRepository` has (see
/// below). When a previously identified external class shell is proven to
/// be a Scala module class by authoritative TASTy, its symbol origin is
/// upgraded and this sidecar origin follows transactionally.
///
/// Deliberately holds only *positive* results, not a full
/// `ClassRepository` (that stays a
/// loader-local field on `ClassLoader`, covering `Loading`/`Failed` too):
/// a name one loader's own classpath doesn't have is not evidence that a
/// *different* loader's classpath doesn't have it either (the
/// one-per-classpath-root pattern this exists for is exactly a case where
/// two loaders' classpaths legitimately differ), so caching a negative
/// result here would make a later loader wrongly skip probing its own,
/// different classpath. A `Loading` entry is loader-call-local by
/// construction too — it only ever matters during one loader's own
/// still-in-progress recursive `load_class` call, never across the
/// loader-instance boundary `into_session`/`with_definitions` crosses.
///
/// A `ClassLoader` cannot simply borrow one `&mut LoadingSession` for its
/// entire lifetime while another `ClassLoader` also needs one: both would
/// also need to borrow the same `&mut SemanticStore`, which the borrow
/// checker already forbids two `ClassLoader`s from doing concurrently — so
/// only one `ClassLoader` is ever alive over a given `store` at a time.
/// `LoadingSession` is therefore threaded through *by value*, handed to
/// `ClassLoader::with_definitions` and handed back out via
/// [`ClassLoader::into_session`](crate::loader::ClassLoader::into_session)
/// once that loader is done, for the next loader to take over — the same
/// ownership-passing shape `Definitions` already uses (`Definitions` is
/// `Copy` and needs no such hand-back; `LoadingSession` owns growable
/// `HashMap`s and so is moved instead).
#[derive(Debug, Default)]
pub struct LoadingSession {
    pub(crate) identified: HashMap<BinaryName, SymbolId>,
    identified_order: Vec<BinaryName>,
    pub(crate) modules: HashMap<BinaryName, crate::loader::ModuleIdentity>,
    pub(crate) modules_by_object: HashMap<SymbolId, BinaryName>,
    pub(crate) modules_by_module_class: HashMap<SymbolId, BinaryName>,
    module_order: Vec<BinaryName>,
    pub(crate) resolved: HashMap<BinaryName, SymbolId>,
    resolved_order: Vec<BinaryName>,
    pub(crate) packages: PackageRegistry,
    pub(crate) metadata: HashMap<SymbolId, ClassfileMetadata>,
    metadata_order: Vec<SymbolId>,
    pub(crate) origins: HashMap<SymbolId, ClassOrigin>,
    origin_undo: Vec<(SymbolId, Option<ClassOrigin>)>,
    symbol_undo: Vec<(SymbolId, Symbol)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SessionCheckpoint {
    identified: usize,
    modules: usize,
    resolved: usize,
    metadata: usize,
    origins: usize,
    symbols: usize,
    packages: (usize, usize),
}

impl LoadingSession {
    /// A fresh session that continues from the session-wide package
    /// registry another adapter (for example the TASTy unpickler) has been
    /// filling, so both agree on every package's `SymbolId`.
    pub fn with_packages(packages: dotty_core::Packages) -> Self {
        Self {
            packages: PackageRegistry::from_packages(packages),
            ..Self::new()
        }
    }

    /// Hands the package registry back, to give to the next adapter working
    /// on the same store. The session's other state is dropped.
    pub fn into_packages(self) -> dotty_core::Packages {
        self.packages.into_packages()
    }

    /// A fresh session with no classes or packages resolved yet — the
    /// starting point for the first `ClassLoader` in a sequence sharing one
    /// `SemanticStore`.
    pub fn new() -> Self {
        Self {
            identified: HashMap::new(),
            identified_order: Vec::new(),
            modules: HashMap::new(),
            modules_by_object: HashMap::new(),
            modules_by_module_class: HashMap::new(),
            module_order: Vec::new(),
            resolved: HashMap::new(),
            resolved_order: Vec::new(),
            packages: PackageRegistry::new(),
            metadata: HashMap::new(),
            metadata_order: Vec::new(),
            origins: HashMap::new(),
            origin_undo: Vec::new(),
            symbol_undo: Vec::new(),
        }
    }

    pub(crate) fn checkpoint(&self) -> SessionCheckpoint {
        SessionCheckpoint {
            identified: self.identified_order.len(),
            modules: self.module_order.len(),
            resolved: self.resolved_order.len(),
            metadata: self.metadata_order.len(),
            origins: self.origin_undo.len(),
            symbols: self.symbol_undo.len(),
            packages: self.packages.checkpoint(),
        }
    }

    pub(crate) fn cache_resolved(&mut self, name: BinaryName, symbol: SymbolId) {
        self.cache_identified(name.clone(), symbol);
        if self.resolved.insert(name.clone(), symbol).is_none() {
            self.resolved_order.push(name);
        }
    }

    pub(crate) fn record_symbol_update(&mut self, store: &dotty_core::SemanticStore, id: SymbolId) {
        self.symbol_undo.push((id, store.symbols.get(id).clone()));
    }

    pub(crate) fn cache_identified(&mut self, name: BinaryName, symbol: SymbolId) {
        if self.identified.insert(name.clone(), symbol).is_none() {
            self.identified_order.push(name);
        }
    }

    pub(crate) fn cache_module(
        &mut self,
        name: BinaryName,
        identity: crate::loader::ModuleIdentity,
    ) {
        self.modules_by_object.insert(identity.object, name.clone());
        self.modules_by_module_class
            .insert(identity.module_class, name.clone());
        if self.modules.insert(name.clone(), identity).is_none() {
            self.module_order.push(name);
        }
    }

    pub(crate) fn record_origin(&mut self, symbol: SymbolId, origin: ClassOrigin) {
        if self.origins.get(&symbol) == Some(&origin) {
            return;
        }
        let previous = self.origins.insert(symbol, origin);
        self.origin_undo.push((symbol, previous));
    }

    pub(crate) fn record_metadata(&mut self, symbol: SymbolId, metadata: ClassfileMetadata) {
        if !self.metadata.contains_key(&symbol) {
            self.metadata_order.push(symbol);
        }
        self.metadata.insert(symbol, metadata);
    }

    pub(crate) fn rollback_to(
        &mut self,
        store: &mut dotty_core::SemanticStore,
        checkpoint: SessionCheckpoint,
    ) {
        while self.symbol_undo.len() > checkpoint.symbols {
            let (id, symbol) = self.symbol_undo.pop().expect("length checked");
            if store.symbols.contains(id) {
                *store.symbols.get_mut(id) = symbol;
            }
        }
        while self.identified_order.len() > checkpoint.identified {
            let name = self.identified_order.pop().expect("length checked");
            self.identified.remove(&name);
        }
        while self.module_order.len() > checkpoint.modules {
            let name = self.module_order.pop().expect("length checked");
            if let Some(identity) = self.modules.remove(&name) {
                self.modules_by_object.remove(&identity.object);
                self.modules_by_module_class.remove(&identity.module_class);
            }
        }
        while self.resolved_order.len() > checkpoint.resolved {
            let name = self.resolved_order.pop().expect("length checked");
            self.resolved.remove(&name);
        }
        while self.metadata_order.len() > checkpoint.metadata {
            let symbol = self.metadata_order.pop().expect("length checked");
            self.metadata.remove(&symbol);
        }
        while self.origin_undo.len() > checkpoint.origins {
            let (symbol, previous) = self.origin_undo.pop().expect("length checked");
            if let Some(previous) = previous {
                self.origins.insert(symbol, previous);
            } else {
                self.origins.remove(&symbol);
            }
        }
        self.packages.rollback_to(store, checkpoint.packages);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbol::ClassfileMetadata;
    use dotty_classfile::access_flags::ClassAccessFlags;
    use dotty_core::{
        Name, Namespace, SemanticStore, Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks,
        SymbolOrigin, Visibility,
    };

    fn symbol(store: &mut SemanticStore, name: &str) -> SymbolId {
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
    fn identified_shell_is_not_reported_as_fully_resolved() {
        let mut store = SemanticStore::new();
        let identity = symbol(&mut store, "Shell");
        let name = BinaryName::from_internal("sample/Shell");
        let mut session = LoadingSession::new();

        session.cache_identified(name.clone(), identity);

        assert_eq!(session.identified.get(&name), Some(&identity));
        assert!(!session.resolved.contains_key(&name));
    }

    #[test]
    fn resolving_a_class_also_records_its_canonical_identity() {
        let mut store = SemanticStore::new();
        let identity = symbol(&mut store, "Complete");
        let name = BinaryName::from_internal("sample/Complete");
        let mut session = LoadingSession::new();

        session.cache_resolved(name.clone(), identity);

        assert_eq!(session.identified.get(&name), Some(&identity));
        assert_eq!(session.resolved.get(&name), Some(&identity));
    }

    #[test]
    fn module_reverse_indexes_follow_cache_and_rollback() {
        let mut store = SemanticStore::new();
        let object = symbol(&mut store, "Module");
        let module_class = symbol(&mut store, "Module$");
        let name = BinaryName::from_internal("sample/Module");
        let module_class_name = BinaryName::from_internal("sample/Module$");
        let mut session = LoadingSession::new();
        let checkpoint = session.checkpoint();

        session.cache_module(
            name.clone(),
            crate::loader::ModuleIdentity {
                object,
                module_class,
                companion_class: None,
                object_name: name.clone(),
                module_class_name,
            },
        );

        assert_eq!(session.modules_by_object.get(&object), Some(&name));
        assert_eq!(
            session.modules_by_module_class.get(&module_class),
            Some(&name)
        );

        session.rollback_to(&mut store, checkpoint);

        assert!(session.modules.is_empty());
        assert!(session.modules_by_object.is_empty());
        assert!(session.modules_by_module_class.is_empty());
    }

    #[test]
    fn rollback_removes_a_new_shell_identity() {
        let mut store = SemanticStore::new();
        let identity = symbol(&mut store, "Shell");
        let name = BinaryName::from_internal("sample/Shell");
        let mut session = LoadingSession::new();
        let checkpoint = session.checkpoint();

        session.cache_identified(name.clone(), identity);
        session.rollback_to(&mut store, checkpoint);

        assert!(!session.identified.contains_key(&name));
        assert!(!session.resolved.contains_key(&name));
    }

    #[test]
    fn metadata_rollback_keeps_the_origin_of_a_preexisting_identity_shell() {
        let mut store = SemanticStore::new();
        let identity = symbol(&mut store, "Shell");
        let mut session = LoadingSession::new();
        let origin = ClassOrigin::Directory("<memory>".into());
        session.record_origin(identity, origin.clone());
        let checkpoint = session.checkpoint();
        session.record_origin(identity, ClassOrigin::Jar("<upgraded>".into()));
        session.record_metadata(
            identity,
            ClassfileMetadata {
                binary_name: BinaryName::from_internal("sample/Shell"),
                access_flags: ClassAccessFlags(0x0021),
                fields: Vec::new(),
                methods: Vec::new(),
                signature: None,
                nest_host: None,
                nest_members: Vec::new(),
                permitted_subclasses: Vec::new(),
                inner_classes: Vec::new(),
                enclosing_method: None,
                record_components: None,
                annotations: Vec::new(),
            },
        );

        session.rollback_to(&mut store, checkpoint);

        assert!(!session.metadata.contains_key(&identity));
        assert_eq!(session.origins.get(&identity), Some(&origin));
    }
}
