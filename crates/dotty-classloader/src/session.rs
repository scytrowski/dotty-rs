use crate::packages::PackageRegistry;
use crate::repository::ClassRepository;

/// The `SymbolId`-allocating state that must be shared, by value, across
/// every [`ClassLoader`](crate::loader::ClassLoader) loading against one
/// [`SemanticStore`](dotty_core::SemanticStore) session — the
/// [`ClassRepository`] cache (which class names have already been resolved
/// to which `SymbolId`) and the [`PackageRegistry`] (which package paths
/// have already been resolved to which package `SymbolId`).
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
/// class first.
///
/// A `ClassLoader` cannot simply borrow one `&mut LoadingSession` for its
/// entire lifetime while another `ClassLoader` also needs one: both would
/// also need to borrow the same `&mut SemanticStore`, which the borrow
/// checker already forbids two `ClassLoader`s from doing concurrently — so
/// only one `ClassLoader` is ever alive over a given `store` at a time.
/// `LoadingSession` is therefore threaded through *by value*, handed to
/// `ClassLoader::with_definitions`/[`ClassLoader::with_session`] and handed
/// back out via [`ClassLoader::into_session`](crate::loader::ClassLoader::into_session)
/// once that loader is done, for the next loader to take over — the same
/// ownership-passing shape `Definitions` already uses (`Definitions` is
/// `Copy` and needs no such hand-back; `LoadingSession` owns growable
/// `HashMap`s and so is moved instead).
#[derive(Debug, Default)]
pub struct LoadingSession {
    pub(crate) repository: ClassRepository,
    pub(crate) packages: PackageRegistry,
}

impl LoadingSession {
    /// A fresh session with no classes or packages resolved yet — the
    /// starting point for the first `ClassLoader` in a sequence sharing one
    /// `SemanticStore`.
    pub fn new() -> Self {
        Self {
            repository: ClassRepository::new(),
            packages: PackageRegistry::new(),
        }
    }
}
