use crate::binary_name::BinaryName;
use crate::class_path::ClassOrigin;
use crate::packages::PackageRegistry;
use crate::symbol::ClassfileMetadata;
use dotty_core::SymbolId;
use std::collections::HashMap;

/// The `SymbolId`-allocating state that must be shared, by value, across
/// every [`ClassLoader`](crate::loader::ClassLoader) loading against one
/// [`SemanticStore`](dotty_core::SemanticStore) session — which class names
/// have already been *successfully* resolved to which `SymbolId`
/// (`resolved`), the [`PackageRegistry`] (which package paths have already
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
/// below): a `SymbolId`'s own metadata/origin, once true, stays true
/// regardless of which loader recorded it.
///
/// Deliberately holds only *positive* results, not a full
/// [`ClassRepository`](crate::repository::ClassRepository) (that stays a
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
    pub(crate) resolved: HashMap<BinaryName, SymbolId>,
    pub(crate) packages: PackageRegistry,
    pub(crate) metadata: HashMap<SymbolId, ClassfileMetadata>,
    pub(crate) origins: HashMap<SymbolId, ClassOrigin>,
}

impl LoadingSession {
    /// A fresh session with no classes or packages resolved yet — the
    /// starting point for the first `ClassLoader` in a sequence sharing one
    /// `SemanticStore`.
    pub fn new() -> Self {
        Self {
            resolved: HashMap::new(),
            packages: PackageRegistry::new(),
            metadata: HashMap::new(),
            origins: HashMap::new(),
        }
    }
}
