//! End-to-end `.tasty`-backed loading through a real `ClassLoader` over a
//! real `DirectoryClassPath` (`docs/classloader.md` §9, Milestone 9),
//! using the real `scalac`-compiled fixtures under
//! `tests/fixtures/tasty_sample/` (copied from `dotty-tasty`'s own
//! fixture tree). Complements `directory_class_path.rs`, which only
//! tests `.tasty`-over-`.class` preference at the `ClassPathEntry`
//! level, and `tasty_symbol`'s own unit tests, which only test decoding
//! in isolation.

use dotty_classloader::classloader::{
    BinaryName, ClassLoader, ClasspathSymbolResolver, CompositeClassPath, DirectoryClassPath,
    LoadingSession,
};
use dotty_core::{
    ClassInfo, Definitions, MemberRequest, Name, Namespace, Packages, ResolutionError,
    ResolverCheckpoint, SemanticStore, SymbolId, SymbolInfo, SymbolKind, SymbolResolver, Type,
    TypeId, Visibility,
};
use dotty_tasty::tasty::{StructuredNode, TastyFile};
use dotty_tasty_unpickler::tasty_unpickler::{
    TastySession, TastySessionError, TastyUnpickler, UnpickleError,
};
use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[path = "support/cross_adapter_annotations.rs"]
mod cross_adapter_annotations;

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("dotty-classloader-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temporary directory should be creatable");
        Self(path)
    }

    fn path(&self) -> &PathBuf {
        &self.0
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn tasty_sample_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tasty_sample")
}

/// Regression for the live package/session handoff: the same real `Open`
/// package entered by the unpickler and found through the classpath resolver
/// must retain its canonical `SymbolId` and `ScopeId`.
#[test]
fn unpickler_and_classpath_resolver_converge_on_the_real_open_package() {
    let path = tasty_visibility_dir();
    let bytes = fs::read(path.join("me/cytrowski/tastyfixtures/visibility/Open.tasty"))
        .expect("real Open.tasty fixture");
    let file = TastyFile::parse_scala_3_9(&bytes).expect("Open.tasty should parse");
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut unpickler = TastyUnpickler::new(&file, &mut store, definitions);
    unpickler
        .enter_symbols()
        .expect("Open symbols should enter");
    let (index, mut packages) = unpickler.into_parts();
    let unpickler_package = index.symbol_at(0).expect("Open package definition");
    let unpickler_scope = packages
        .scope_of(unpickler_package)
        .expect("Open package scope");

    let mut resolver = ClasspathSymbolResolver::new(
        DirectoryClassPath::new(path),
        definitions,
        LoadingSession::new(),
    );
    let resolver_package = dotty_core::SymbolResolver::resolve_package_with_packages(
        &mut resolver,
        &mut store,
        &mut packages,
        &["me", "cytrowski", "tastyfixtures", "visibility"],
    )
    .expect("resolver should find the real classpath package")
    .expect("the package is present in the classpath");

    assert_eq!(resolver_package, unpickler_package);
    assert_eq!(packages.scope_of(resolver_package), Some(unpickler_scope));
    assert_eq!(
        store.symbols.get(resolver_package).name.namespace(),
        Namespace::Term
    );
    assert_eq!(
        store.scopes.get(unpickler_scope).owner,
        Some(resolver_package)
    );
    assert_eq!(
        store.symbols.get(resolver_package).owner,
        Some(
            packages
                .symbol(&["me", "cytrowski", "tastyfixtures"])
                .unwrap()
        )
    );
}

#[test]
fn resolver_first_package_identity_is_adopted_by_the_tasty_session() {
    let path = tasty_visibility_dir();
    let bytes = fs::read(path.join("me/cytrowski/tastyfixtures/visibility/InOwnPackage.tasty"))
        .expect("real InOwnPackage.tasty fixture");
    let file = TastyFile::parse_scala_3_9(&bytes).expect("TASTy should parse");
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut resolver = ClasspathSymbolResolver::new(
        DirectoryClassPath::new(path),
        definitions,
        LoadingSession::new(),
    );
    let resolver_package = dotty_core::SymbolResolver::resolve_package(
        &mut resolver,
        &mut store,
        &["me", "cytrowski", "tastyfixtures", "visibility"],
    )
    .expect("resolver should find the real classpath package")
    .expect("the package is present in the classpath");
    let packages = dotty_core::SymbolResolver::take_packages(&mut resolver)
        .expect("classpath resolver owns package identities");
    let resolver_scope = packages
        .scope_of(resolver_package)
        .expect("resolver package declaration scope");

    let session = TastySession::with_packages(packages)
        .with_resolver(Box::new(resolver))
        .expect("the resolver's package registry becomes the session authority");
    let mut unpickler = TastyUnpickler::with_session(&file, &mut store, definitions, session);
    unpickler
        .enter_symbols()
        .expect("unit symbols should enter");
    let package = unpickler.index().symbol_at(0).expect("package definition");
    let (_, session) = unpickler.into_session_parts();

    assert_eq!(package, resolver_package);
    assert_eq!(session.packages().scope_of(package), Some(resolver_scope));
    assert_eq!(store.symbols.get(package).kind, SymbolKind::Package);
    assert_eq!(store.symbols.get(package).name.namespace(), Namespace::Term);
    assert_eq!(store.scopes.get(resolver_scope).owner, Some(package));
    assert_eq!(
        store.symbols.get(package).owner,
        Some(
            session
                .packages()
                .symbol(&["me", "cytrowski", "tastyfixtures"])
                .unwrap()
        )
    );
}

#[test]
fn independently_entered_conflicting_package_registries_fail_closed() {
    let path = tasty_visibility_dir();
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut resolver = ClasspathSymbolResolver::new(
        DirectoryClassPath::new(path),
        definitions,
        LoadingSession::new(),
    );
    dotty_core::SymbolResolver::resolve_package(
        &mut resolver,
        &mut store,
        &["me", "cytrowski", "tastyfixtures", "visibility"],
    )
    .unwrap()
    .unwrap();
    let mut tasty_packages = Packages::new();
    tasty_packages.enter(
        &mut store,
        dotty_core::SymbolOrigin::Synthetic,
        &["me", "cytrowski", "tastyfixtures", "visibility"],
    );
    let tasty_package = tasty_packages
        .symbol(&["me", "cytrowski", "tastyfixtures", "visibility"])
        .unwrap();

    let error = match TastySession::with_packages(tasty_packages).with_resolver(Box::new(resolver))
    {
        Err(error) => error,
        Ok(_) => panic!("independently entered roots must conflict"),
    };
    assert_eq!(
        error.reason(),
        TastySessionError::ConflictingPackageAuthorities
    );
    let (mut session, resolver) = error.into_parts();
    assert!(resolver.has_package_root());
    assert!(
        session
            .packages()
            .symbol(&["me", "cytrowski", "tastyfixtures", "visibility"])
            .is_some()
    );

    session = session
        .with_resolver(Box::new(ClasspathSymbolResolver::new(
            DirectoryClassPath::new(tasty_visibility_dir()),
            definitions,
            LoadingSession::new(),
        )))
        .expect("a fresh resolver can attach after the rejected resolver is recovered");
    let bytes =
        fs::read(tasty_visibility_dir().join("me/cytrowski/tastyfixtures/visibility/Open.tasty"))
            .expect("real Open.tasty fixture");
    let file = TastyFile::parse_scala_3_9(&bytes).expect("Open.tasty should parse");
    let mut unpickler = TastyUnpickler::with_session(&file, &mut store, definitions, session);
    unpickler
        .enter_symbols()
        .expect("retry session can enter the unit");
    assert_eq!(unpickler.index().symbol_at(0), Some(tasty_package));
}

#[test]
fn rejected_unpickler_resolver_attachment_returns_the_unpickler_and_resolver() {
    let file = TastyFile::parse_scala_3_9(DISTINCT).expect("Distinct.tasty should parse");
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let package = store.symbols.alloc(dotty_core::Symbol {
        name: Name::new(store.names.intern("lang"), Namespace::Term),
        owner: None,
        kind: SymbolKind::Package,
        flags: dotty_core::SymbolFlags::EMPTY,
        visibility: Visibility::Public,
        info: dotty_core::SymbolInfo::Missing,
        origin: dotty_core::SymbolOrigin::Synthetic,
        annotations: Vec::new(),
        position: None,
        links: dotty_core::SymbolLinks::default(),
    });
    let object = store.symbols.alloc(dotty_core::Symbol {
        name: Name::new(store.names.intern("Object"), Namespace::Type),
        owner: Some(package),
        kind: SymbolKind::Class,
        flags: dotty_core::SymbolFlags::EMPTY,
        visibility: Visibility::Public,
        info: dotty_core::SymbolInfo::Missing,
        origin: dotty_core::SymbolOrigin::Synthetic,
        annotations: Vec::new(),
        position: None,
        links: dotty_core::SymbolLinks::default(),
    });
    let member_called = Rc::new(Cell::new(false));
    let session = TastySession::new()
        .with_resolver(Box::new(AttachmentTestResolver {
            package,
            member: object,
            member_called: Rc::clone(&member_called),
        }))
        .expect("first resolver attaches");
    let unpickler = TastyUnpickler::with_session(&file, &mut store, definitions, session);
    let error = match unpickler.with_resolver(Box::new(dotty_core::NoResolver)) {
        Err(error) => error,
        Ok(_) => panic!("a second resolver cannot replace the session resolver"),
    };
    assert_eq!(error.reason(), TastySessionError::ResolverAlreadyAttached);
    let (mut unpickler, incoming_resolver) = error.into_parts();
    assert!(!incoming_resolver.has_package_root());

    unpickler
        .unpickle_type(38)
        .expect("original resolver remains usable after rejected replacement");
    assert!(member_called.get());
}

struct AttachmentTestResolver {
    package: SymbolId,
    member: SymbolId,
    member_called: Rc<Cell<bool>>,
}

impl SymbolResolver for AttachmentTestResolver {
    fn checkpoint(&self) -> ResolverCheckpoint {
        ResolverCheckpoint::new(0)
    }

    fn rollback_to(&mut self, _store: &mut SemanticStore, _checkpoint: ResolverCheckpoint) {}

    fn resolve_member(
        &mut self,
        _store: &mut SemanticStore,
        _request: &MemberRequest,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        self.member_called.set(true);
        Ok(Some(self.member))
    }

    fn resolve_package(
        &mut self,
        _store: &mut SemanticStore,
        path: &[&str],
    ) -> Result<Option<SymbolId>, ResolutionError> {
        Ok((path == ["java", "lang"]).then_some(self.package))
    }
}

fn published_scala39_library_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dotty-tasty/tests/fixtures/scala3-library")
}

const COMPLETION: &[u8] =
    include_bytes!("../../dotty-tasty-unpickler/tests/fixtures/semantic/CompletionHolder.tasty");
const DISTINCT: &[u8] =
    include_bytes!("../../dotty-tasty-unpickler/tests/fixtures/semantic/Distinct.tasty");

struct FailOnceAfterPackage<E: dotty_classloader::classloader::ClassPathEntry> {
    inner: ClasspathSymbolResolver<E>,
    fail_once: Rc<Cell<bool>>,
    materialized: Rc<Cell<bool>>,
    rollback_removed: Rc<Cell<bool>>,
    member_callback: Rc<Cell<bool>>,
}

impl<E: dotty_classloader::classloader::ClassPathEntry> SymbolResolver for FailOnceAfterPackage<E> {
    fn take_packages(&mut self) -> Option<Packages> {
        self.inner.take_packages()
    }

    fn checkpoint_with_packages(&self, packages: &Packages) -> ResolverCheckpoint {
        self.inner.checkpoint_with_packages(packages)
    }

    fn checkpoint(&self) -> ResolverCheckpoint {
        self.inner.checkpoint()
    }

    fn rollback_to(&mut self, store: &mut SemanticStore, checkpoint: ResolverCheckpoint) {
        self.inner.rollback_to(store, checkpoint);
    }

    fn rollback_to_with_packages(
        &mut self,
        store: &mut SemanticStore,
        checkpoint: ResolverCheckpoint,
        packages: &mut Packages,
    ) {
        self.inner
            .rollback_to_with_packages(store, checkpoint, packages);
        self.rollback_removed
            .set(packages.symbol(&["scala"]).is_none());
    }

    fn resolve_member(
        &mut self,
        store: &mut SemanticStore,
        request: &MemberRequest,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        self.inner.resolve_member(store, request)
    }

    fn resolve_member_with_packages(
        &mut self,
        store: &mut SemanticStore,
        packages: &mut Packages,
        request: &MemberRequest,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        self.member_callback.set(true);
        self.inner
            .resolve_member_with_packages(store, packages, request)
    }

    fn resolve_package(
        &mut self,
        store: &mut SemanticStore,
        path: &[&str],
    ) -> Result<Option<SymbolId>, ResolutionError> {
        self.inner.resolve_package(store, path)
    }

    fn resolve_package_with_packages(
        &mut self,
        store: &mut SemanticStore,
        packages: &mut Packages,
        path: &[&str],
    ) -> Result<Option<SymbolId>, ResolutionError> {
        let result = self
            .inner
            .resolve_package_with_packages(store, packages, path);
        if result.as_ref().is_ok_and(Option::is_some) && self.fail_once.replace(false) {
            self.materialized.set(true);
            Err(ResolutionError::Malformed {
                reason: "injected late session handoff failure".to_owned(),
            })
        } else {
            result
        }
    }
}

fn definition_address(file: &TastyFile<'_>, definition: &str) -> u32 {
    let index = file.ast_address_index().expect("TASTy AST index");
    index
        .iter_nodes()
        .find_map(|node| {
            let at = u32::try_from(node.offset).ok()?;
            let raw = index.get(at)?;
            let name = match raw.decode_structured().ok()? {
                StructuredNode::DefDef(body) => body.name,
                StructuredNode::ValDef(body) => body.name(),
                _ => return None,
            };
            (file.names().get_utf8(name) == Some(definition)).then_some(at)
        })
        .unwrap_or_else(|| panic!("definition {definition} in TASTy fixture"))
}

#[test]
fn active_unpickler_resolves_and_retains_a_new_real_classpath_package() {
    let file = TastyFile::parse_scala_3_9(COMPLETION).expect("Completion.tasty should parse");
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let resolver = ClasspathSymbolResolver::new(
        DirectoryClassPath::new(published_scala39_library_dir()),
        definitions,
        LoadingSession::new(),
    );
    let session = TastySession::new()
        .with_resolver(Box::new(resolver))
        .expect("an empty resolver registry can join the TASTy session");
    let mut unpickler = TastyUnpickler::with_session(&file, &mut store, definitions, session);
    unpickler
        .enter_symbols()
        .expect("Completion symbols should enter");
    let ast = file.ast_address_index().expect("TASTy AST index");
    let package_references: Vec<_> = [64, 65]
        .into_iter()
        .flat_map(|tag| ast.iter_nodes_with_tag(tag))
        .filter_map(|node| u32::try_from(node.offset).ok())
        .collect();
    assert!(!package_references.is_empty());
    for address in package_references {
        let _ = unpickler.unpickle_type(address);
    }
    let (_, session) = unpickler.into_session_parts();

    let scala = session
        .packages()
        .symbol(&["scala"])
        .expect("the resolver-created scala package remains in the canonical registry");
    let scope = session
        .packages()
        .scope_of(scala)
        .expect("scala package scope");
    assert_eq!(store.symbols.get(scala).kind, SymbolKind::Package);
    assert_eq!(store.scopes.get(scope).owner, Some(scala));
}

fn normalized_live_session_receipt() -> Vec<String> {
    let file = TastyFile::parse_scala_3_9(COMPLETION).expect("Completion.tasty should parse");
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let resolver = ClasspathSymbolResolver::new(
        DirectoryClassPath::new(published_scala39_library_dir()),
        definitions,
        LoadingSession::new(),
    );
    let session = TastySession::new()
        .with_resolver(Box::new(resolver))
        .expect("resolver registry is empty before the first TASTy unit");
    let mut unpickler = TastyUnpickler::with_session(&file, &mut store, definitions, session);
    unpickler
        .enter_symbols()
        .expect("Completion symbols should enter");
    let ast = file.ast_address_index().expect("TASTy AST index");
    let package_references: Vec<_> = [64, 65]
        .into_iter()
        .flat_map(|tag| ast.iter_nodes_with_tag(tag))
        .filter_map(|node| u32::try_from(node.offset).ok())
        .collect();
    let mut receipt = package_references
        .into_iter()
        .map(|address| match unpickler.unpickle_type(address) {
            Ok(_) => "package-reference=resolved".to_owned(),
            Err(error) => format!("package-reference={:?}", error.category()),
        })
        .collect::<Vec<_>>();
    let (_, session) = unpickler.into_session_parts();
    let scala = session
        .packages()
        .symbol(&["scala"])
        .expect("resolver-created scala package");
    let symbol = store.symbols.get(scala);
    let name = store.names.resolve(symbol.name.text());
    let owner = symbol.owner.expect("scala package has the root owner");
    let owner_symbol = store.symbols.get(owner);
    receipt.push(format!(
        "package={name}:kind={:?}:namespace={:?}:owner={}",
        symbol.kind,
        symbol.name.namespace(),
        store.names.resolve(owner_symbol.name.text())
    ));
    receipt.sort();
    receipt
}

#[test]
fn independent_live_sessions_have_the_same_normalized_package_receipt() {
    assert_eq!(
        normalized_live_session_receipt(),
        normalized_live_session_receipt()
    );
}

#[test]
fn late_failure_after_real_package_materialization_rolls_back_and_same_session_retry_works() {
    let file = TastyFile::parse_scala_3_9(COMPLETION).expect("Completion.tasty should parse");
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let failed_once = Rc::new(Cell::new(true));
    let materialized = Rc::new(Cell::new(false));
    let rollback_removed = Rc::new(Cell::new(false));
    let member_callback = Rc::new(Cell::new(false));
    let resolver = FailOnceAfterPackage {
        inner: ClasspathSymbolResolver::new(
            DirectoryClassPath::new(published_scala39_library_dir()),
            definitions,
            LoadingSession::new(),
        ),
        fail_once: Rc::clone(&failed_once),
        materialized: Rc::clone(&materialized),
        rollback_removed: Rc::clone(&rollback_removed),
        member_callback: Rc::clone(&member_callback),
    };
    let session = TastySession::new()
        .with_resolver(Box::new(resolver))
        .expect("resolver registry is empty before the first TASTy unit");
    let mut unpickler = TastyUnpickler::with_session(&file, &mut store, definitions, session);
    unpickler
        .enter_symbols()
        .expect("Completion symbols should enter");
    let ast = file.ast_address_index().expect("TASTy AST index");
    let package_references: Vec<_> = [64, 65]
        .into_iter()
        .flat_map(|tag| ast.iter_nodes_with_tag(tag))
        .filter_map(|node| u32::try_from(node.offset).ok())
        .collect();

    let mut failed_address = None;
    for address in package_references {
        let result = unpickler.unpickle_type(address);
        if materialized.get() {
            assert!(matches!(
                result,
                Err(UnpickleError::ResolverFailure {
                    error: ResolutionError::Malformed { ref reason },
                    ..
                }) if reason == "injected late session handoff failure"
            ));
            failed_address = Some(address);
            break;
        }
    }
    let address = failed_address.expect("a real Scala package was materialized by the resolver");
    assert!(
        rollback_removed.get(),
        "rollback removes the materialized package"
    );
    assert!(!failed_once.get());

    unpickler
        .unpickle_type(address)
        .expect("retry in the same session succeeds");
    assert!(
        unpickler
            .complete_symbol(definition_address(&file, "generic"))
            .is_err()
    );
    assert!(
        member_callback.get(),
        "member requests use the shared package registry"
    );
    member_callback.set(false);
    let (_, session) = unpickler.into_session_parts();
    let scala = session
        .packages()
        .symbol(&["scala"])
        .expect("retry commits the canonical scala package");
    assert_eq!(store.symbols.get(scala).kind, SymbolKind::Package);

    let distinct = TastyFile::parse_scala_3_9(DISTINCT).expect("Distinct.tasty should parse");
    let mut resumed = TastyUnpickler::with_session(&distinct, &mut store, definitions, session);
    resumed
        .enter_symbols()
        .expect("a second real unit should enter in the same session");
    assert!(
        resumed
            .complete_symbol(definition_address(&distinct, "cross"))
            .is_err()
    );
    assert!(
        member_callback.get(),
        "the resolver survives into_session_parts and serves the next unit"
    );
}

/// A minimal, hand-built, clearly-synthetic `java/lang/Object` class
/// file: none of this crate's real fixtures define `Object` itself, and
/// a real one isn't needed here — only its existence as a loadable,
/// no-superclass, no-interfaces class matters, to let `Dog`/`Animal`'s
/// implicit superclass resolve successfully. Written under its own
/// temporary classpath root, kept separate from `tasty_sample/`'s real
/// `scalac` output.
fn write_synthetic_object_class(root: &Path) {
    write_synthetic_class(root, "java/lang/Object", 0x0021, None);
}

fn write_synthetic_interface(root: &Path, internal_name: &str) {
    write_synthetic_class(root, internal_name, 0x0601, Some("java/lang/Object"));
}

fn write_synthetic_class(
    root: &Path,
    internal_name: &str,
    access_flags: u16,
    super_name: Option<&str>,
) {
    let name = internal_name.as_bytes();
    let mut pool = Vec::new();
    pool.push(1); // CONSTANT_Utf8 #1
    pool.extend_from_slice(&(name.len() as u16).to_be_bytes());
    pool.extend_from_slice(name);
    pool.push(7); // CONSTANT_Class #2 -> #1
    pool.extend_from_slice(&1u16.to_be_bytes());
    if let Some(super_name) = super_name {
        pool.push(1); // CONSTANT_Utf8 #3
        pool.extend_from_slice(&(super_name.len() as u16).to_be_bytes());
        pool.extend_from_slice(super_name.as_bytes());
        pool.push(7); // CONSTANT_Class #4 -> #3
        pool.extend_from_slice(&3u16.to_be_bytes());
    }

    let mut bytes = Vec::new();
    bytes.extend_from_slice(&[0xCA, 0xFE, 0xBA, 0xBE]);
    bytes.extend_from_slice(&[0x00, 0x00]); // minor
    bytes.extend_from_slice(&[0x00, 0x45]); // class-file major = 69
    bytes.extend_from_slice(
        &(if super_name.is_some() {
            0x0005u16
        } else {
            0x0003u16
        })
        .to_be_bytes(),
    ); // constant_pool_count
    bytes.extend_from_slice(&pool);
    bytes.extend_from_slice(&access_flags.to_be_bytes());
    bytes.extend_from_slice(&2u16.to_be_bytes()); // this_class -> #2
    bytes.extend_from_slice(&if super_name.is_some() { 4u16 } else { 0u16 }.to_be_bytes());
    bytes.extend_from_slice(&[0x00, 0x00]); // interfaces_count
    bytes.extend_from_slice(&[0x00, 0x00]); // fields_count
    bytes.extend_from_slice(&[0x00, 0x00]); // methods_count
    bytes.extend_from_slice(&[0x00, 0x00]); // attributes_count

    let path = root.join(internal_name).with_extension("class");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn class_loader_over_tasty_sample<'store>(
    name: &str,
    store: &'store mut SemanticStore,
) -> (ClassLoader<'store, CompositeClassPath>, TemporaryDirectory) {
    let synthetic_jdk = TemporaryDirectory::new(name);
    write_synthetic_object_class(synthetic_jdk.path());

    let class_path = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(tasty_sample_dir())),
        Box::new(DirectoryClassPath::new(synthetic_jdk.path().clone())),
    ]);
    (ClassLoader::new(class_path, store), synthetic_jdk)
}

/// The class's `Type::ClassInfo`, read back through its `Symbol`'s
/// `SymbolInfo::Complete`.
fn class_info(store: &SemanticStore, id: SymbolId) -> &ClassInfo {
    let symbol = store.symbols.get(id);
    let SymbolInfo::Complete(info_id) = symbol.info else {
        panic!("expected the class to have complete info");
    };
    let Type::ClassInfo(class_info) = store.types.get(info_id) else {
        panic!("expected a ClassInfo");
    };
    class_info
}

/// The `SymbolId` a `Type::TypeRef` (a `ClassInfo` parent) points at.
fn parent_symbol(store: &SemanticStore, ty: TypeId) -> SymbolId {
    let Some(symbol) = store.types.get(ty).reference_symbol() else {
        panic!("expected a TypeRef");
    };
    symbol
}

/// `Dog.tasty`'s `Animal` mixin is a real, post-typecheck reference
/// carrying its own compiled `TERMREFpkg` prefix
/// (`me.cytrowski.tastyfixtures`, this fixture's real source package —
/// see `tasty_symbol::resolve_reference_name`), so `Animal` is looked up
/// at that real qualified path, not bare — `tasty_sample/me/cytrowski/
/// tastyfixtures/Animal.tasty` is a second copy of the same
/// `Animal.tasty` bytes, alongside the pre-existing root-level
/// `Animal.tasty`/`Animal.class` the tests below still load directly by
/// their own bare request.
#[test]
fn loads_dog_with_animal_resolved_through_tasty_as_a_real_interface() {
    let mut store = SemanticStore::new();
    let (mut loader, _synthetic_jdk) =
        class_loader_over_tasty_sample("tasty-loading-dog-synthetic-jdk", &mut store);

    let dog = loader
        .load_class(&BinaryName::from_internal("Dog"))
        .expect("Dog should load from its real .tasty fixture");
    drop(loader);

    assert_eq!(store.symbols.get(dog).kind, SymbolKind::Class);
    let info = class_info(&store, dog);
    assert_eq!(info.parents.len(), 2);

    let super_symbol = parent_symbol(&store, info.parents[0]);
    assert_eq!(
        store
            .names
            .resolve(store.symbols.get(super_symbol).name.text()),
        "Object"
    );

    let interface_symbol = parent_symbol(&store, info.parents[1]);
    assert_eq!(
        store
            .names
            .resolve(store.symbols.get(interface_symbol).name.text()),
        "Animal"
    );
    assert_eq!(store.symbols.get(interface_symbol).kind, SymbolKind::Trait);
}

/// `tasty_sample/` also has a co-located dummy `Animal.class` (real
/// bytes, but for an unrelated class — see `directory_class_path.rs`'s
/// preference tests). If `.tasty`-over-`.class` preference broke and
/// this dummy file were decoded instead, `ClassLoader` would report
/// `NameMismatch` (its `this_class` is not `Animal`) rather than
/// produce this trait's real shape — so this test's success also
/// exercises that preference end-to-end through the full loader, not
/// just `ClassPathEntry::find_class` in isolation.
#[test]
fn loads_animal_directly_as_an_interface_with_no_declared_interfaces() {
    let mut store = SemanticStore::new();
    let (mut loader, _synthetic_jdk) =
        class_loader_over_tasty_sample("tasty-loading-animal-synthetic-jdk", &mut store);

    let animal = loader
        .load_class(&BinaryName::from_internal("Animal"))
        .expect("Animal should load from its real .tasty fixture, not the co-located dummy .class");
    drop(loader);

    assert_eq!(store.symbols.get(animal).kind, SymbolKind::Trait);
    let info = class_info(&store, animal);
    assert_eq!(info.parents.len(), 1);

    let super_symbol = parent_symbol(&store, info.parents[0]);
    assert_eq!(
        store
            .names
            .resolve(store.symbols.get(super_symbol).name.text()),
        "Object"
    );
}

#[test]
fn loads_published_scala39_with_filter_and_its_map_member() {
    let mut store = SemanticStore::new();
    let synthetic_jdk = TemporaryDirectory::new("published-scala39-with-filter");
    write_synthetic_object_class(synthetic_jdk.path());
    write_synthetic_interface(synthetic_jdk.path(), "java/io/Serializable");
    let class_path = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(published_scala39_library_dir())),
        Box::new(DirectoryClassPath::new(synthetic_jdk.path().clone())),
    ]);
    let mut loader = ClassLoader::new(class_path, &mut store);

    let with_filter = loader
        .load_class(&BinaryName::from_internal("scala/collection/WithFilter"))
        .expect("published Scala 3.9 WithFilter should load through its real TASTy");
    drop(loader);

    assert_eq!(store.symbols.get(with_filter).kind, SymbolKind::Class);
    let declarations = class_info(&store, with_filter).declarations;
    let map_name = Name::new(store.names.intern("map"), Namespace::Term);
    let map = store
        .scopes
        .get(declarations)
        .lookup(&map_name)
        .expect("WithFilter.map should be materialized into its declarations");
    assert_eq!(store.symbols.get(map).kind, SymbolKind::Method);
    assert!(matches!(
        store.symbols.get(map).info,
        SymbolInfo::Complete(_)
    ));
}

#[test]
fn completes_a_real_scala39_module_class_with_its_overloads() {
    let mut store = SemanticStore::new();
    let synthetic_jdk = TemporaryDirectory::new("published-scala39-module");
    write_synthetic_object_class(synthetic_jdk.path());
    let class_path = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(published_scala39_library_dir())),
        Box::new(DirectoryClassPath::new(synthetic_jdk.path().clone())),
    ]);
    let mut loader = ClassLoader::new(class_path, &mut store);
    let identity = loader
        .complete_module(&BinaryName::from_internal("scala/math/BigInt"))
        .expect("BigInt should complete from its Scala 3.9 TASTy");
    drop(loader);

    assert_eq!(
        store.symbols.get(identity.module_class).kind,
        SymbolKind::ModuleClass
    );
    let declarations = class_info(&store, identity.module_class).declarations;
    let apply = Name::new(store.names.intern("apply"), Namespace::Term);
    assert!(
        store.scopes.get(declarations).lookup_all(&apply).len() > 1,
        "BigInt should preserve its overloaded apply declarations"
    );
    assert_eq!(
        store
            .types
            .get(match store.symbols.get(identity.object).info {
                SymbolInfo::Complete(ty) => ty,
                other => panic!("expected a completed module term, found {other:?}"),
            })
            .reference_symbol(),
        Some(identity.module_class)
    );
}

#[test]
fn loads_scala39_function1_with_a_shared_anyref_parent() {
    let mut store = SemanticStore::new();
    let synthetic_jdk = TemporaryDirectory::new("published-scala39-function1");
    write_synthetic_object_class(synthetic_jdk.path());
    let class_path = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(published_scala39_library_dir())),
        Box::new(DirectoryClassPath::new(synthetic_jdk.path().clone())),
    ]);
    let mut loader = ClassLoader::new(class_path, &mut store);

    let function1 = loader
        .load_class(&BinaryName::from_internal("scala/Function1"))
        .expect("Function1 should resolve its shared AnyRef parent to java/lang/Object");
    drop(loader);

    let parents = &class_info(&store, function1).parents;
    assert_eq!(parents.len(), 1);
    let parent = store.types.get(parents[0]).reference_symbol().unwrap();
    assert_eq!(
        store.names.resolve(store.symbols.get(parent).name.text()),
        "Object"
    );
}

#[test]
fn loads_scala39_source_with_its_applied_iterator_alias_parent() {
    let mut store = SemanticStore::new();
    let synthetic_jdk = TemporaryDirectory::new("published-scala39-source");
    write_synthetic_object_class(synthetic_jdk.path());
    write_synthetic_interface(synthetic_jdk.path(), "java/io/Closeable");
    let class_path = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(published_scala39_library_dir())),
        Box::new(DirectoryClassPath::new(synthetic_jdk.path().clone())),
    ]);
    let mut loader = ClassLoader::new(class_path, &mut store);

    let source = loader
        .load_class(&BinaryName::from_internal("scala/io/Source"))
        .expect("Source should resolve the qualified and applied parents from Scala TASTy");
    drop(loader);

    let parents = &class_info(&store, source).parents;
    let parent_names = parents
        .iter()
        .map(|parent| {
            let tycon = match store.types.get(*parent) {
                Type::Applied { tycon, .. } => *tycon,
                _ => *parent,
            };
            let symbol = store.types.get(tycon).reference_symbol().unwrap();
            store.names.resolve(store.symbols.get(symbol).name.text())
        })
        .collect::<Vec<_>>();
    assert!(parent_names.contains(&"Closeable"));
    assert!(parent_names.contains(&"Iterator"));
    let iterator_parent = parents
        .iter()
        .copied()
        .find(|parent| {
            let tycon = match store.types.get(*parent) {
                Type::Applied { tycon, .. } => *tycon,
                _ => *parent,
            };
            store
                .types
                .get(tycon)
                .reference_symbol()
                .is_some_and(|symbol| {
                    store.names.resolve(store.symbols.get(symbol).name.text()) == "Iterator"
                })
        })
        .expect("Source's Iterator parent should be present");
    let Type::Applied { args, .. } = store.types.get(iterator_parent) else {
        panic!("Source should retain the applied Iterator[Char] parent view")
    };
    assert_eq!(args.len(), 1);
    let char_symbol = store.types.get(args[0]).reference_symbol().unwrap();
    assert_eq!(
        store
            .names
            .resolve(store.symbols.get(char_symbol).name.text()),
        "Char"
    );
}

#[test]
fn loads_scala39_annotation_types_with_their_declared_parents() {
    let mut store = SemanticStore::new();
    let synthetic_jdk = TemporaryDirectory::new("published-scala39-retains-cap");
    write_synthetic_object_class(synthetic_jdk.path());
    let class_path = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(published_scala39_library_dir())),
        Box::new(DirectoryClassPath::new(synthetic_jdk.path().clone())),
    ]);
    let mut loader = ClassLoader::new(class_path, &mut store);

    let annotation_types = ["scala/annotation/retainsCap", "scala/annotation/switch"];
    let loaded = annotation_types
        .iter()
        .map(|name| {
            (
                *name,
                loader
                    .load_class(&BinaryName::from_internal(*name))
                    .unwrap_or_else(|error| panic!("{name} parents should resolve: {error:?}")),
            )
        })
        .collect::<Vec<_>>();
    drop(loader);

    for (name, id) in loaded {
        let parent_names = class_info(&store, id)
            .parents
            .iter()
            .map(|parent| {
                let symbol = store.types.get(*parent).reference_symbol().unwrap();
                store.names.resolve(store.symbols.get(symbol).name.text())
            })
            .collect::<Vec<_>>();
        assert_eq!(parent_names, ["Annotation", "StaticAnnotation"], "{name}");
    }
}

#[test]
fn enters_a_tasty_class_into_its_package_scope() {
    let mut store = SemanticStore::new();
    let (mut loader, _synthetic_jdk) =
        class_loader_over_tasty_sample("tasty-package-scope", &mut store);

    let animal = loader
        .load_class(&BinaryName::from_internal(
            "me/cytrowski/tastyfixtures/Animal",
        ))
        .expect("packaged Animal should load from its .tasty fixture");
    let packages = loader.into_session().into_packages();
    let package = store
        .symbols
        .get(animal)
        .owner
        .expect("Animal should have a package owner");
    let package_scope = packages
        .scope_of(package)
        .expect("the package registry should retain the scope");
    let animal_name = Name::new(store.symbols.get(animal).name.text(), Namespace::Type);

    assert_eq!(
        store.scopes.get(package_scope).lookup_all(&animal_name),
        &[animal],
        "a TASTy-loaded class should be discoverable through its package scope"
    );
}

fn tasty_visibility_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tasty_visibility")
}

/// Loads `class` (a class of the `visibility` fixture package) and returns
/// its symbol.
fn load_visibility_class(store: &mut SemanticStore, class: &str) -> SymbolId {
    let synthetic_jdk = TemporaryDirectory::new(&format!("tasty-visibility-{class}"));
    write_synthetic_object_class(synthetic_jdk.path());
    let class_path = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(tasty_visibility_dir())),
        Box::new(DirectoryClassPath::new(synthetic_jdk.path().clone())),
    ]);
    let mut loader = ClassLoader::new(class_path, store);
    loader
        .load_class(&BinaryName::from_internal(format!(
            "me/cytrowski/tastyfixtures/visibility/{class}"
        )))
        .unwrap_or_else(|error| panic!("{class} should load: {error:?}"))
}

/// The package `levels_up` owners above the class (0 is its own package).
fn package_above(store: &SemanticStore, class: SymbolId, levels_up: usize) -> SymbolId {
    let mut symbol = store
        .symbols
        .get(class)
        .owner
        .expect("a class has a package");
    for _ in 0..levels_up {
        symbol = store
            .symbols
            .get(symbol)
            .owner
            .expect("an enclosing package");
    }
    symbol
}

/// Issue #16: `private[X]` on a `.tasty` class used to leave it `Public`.
#[test]
fn a_class_private_to_its_own_package_is_private_within_that_package() {
    let mut store = SemanticStore::new();

    let class = load_visibility_class(&mut store, "InOwnPackage");

    assert_eq!(
        store.symbols.get(class).visibility,
        Visibility::PrivateWithin(package_above(&store, class, 0))
    );
}

#[test]
fn a_class_private_to_an_enclosing_package_is_private_within_that_package() {
    let mut store = SemanticStore::new();

    let in_parent = load_visibility_class(&mut store, "InEnclosingPackage");
    let in_outer = load_visibility_class(&mut store, "InOuterPackage");

    // `visibility` is one level below `tastyfixtures`, and two below `me`'s
    // child `cytrowski`: me / cytrowski / tastyfixtures / visibility.
    assert_eq!(
        store.symbols.get(in_parent).visibility,
        Visibility::PrivateWithin(package_above(&store, in_parent, 1))
    );
    assert_eq!(
        store.symbols.get(in_outer).visibility,
        Visibility::PrivateWithin(package_above(&store, in_outer, 3))
    );
}

#[test]
fn a_top_level_private_class_is_private_within_its_package() {
    let mut store = SemanticStore::new();

    let class = load_visibility_class(&mut store, "PlainPrivate");

    assert_eq!(
        store.symbols.get(class).visibility,
        Visibility::PrivateWithin(package_above(&store, class, 0))
    );
}

#[test]
fn a_class_with_no_access_modifier_stays_public() {
    let mut store = SemanticStore::new();

    let class = load_visibility_class(&mut store, "Open");

    assert_eq!(store.symbols.get(class).visibility, Visibility::Public);
}

fn tasty_fields_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tasty_fields")
}

/// The names of the fields the loader entered into the declarations scope of
/// `class` (of the `fields` fixture package), for each of `probes`.
fn entered_fields<'a>(class: &str, probes: &[&'a str]) -> Vec<&'a str> {
    let synthetic_jdk = TemporaryDirectory::new(&format!("tasty-fields-{class}"));
    write_synthetic_object_class(synthetic_jdk.path());
    let class_path = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(tasty_fields_dir())),
        Box::new(DirectoryClassPath::new(synthetic_jdk.path().clone())),
    ]);
    let mut store = SemanticStore::new();
    let mut loader = ClassLoader::new(class_path, &mut store);
    let symbol = loader
        .load_class(&BinaryName::from_internal(format!(
            "me/cytrowski/tastyfixtures/fields/{class}"
        )))
        .unwrap_or_else(|error| panic!("{class} should load: {error:?}"));
    drop(loader);

    let scope = class_info(&store, symbol).declarations;
    probes
        .iter()
        .copied()
        .filter(|probe| {
            let text = store.names.intern(probe);
            store
                .scopes
                .get(scope)
                .lookup(&Name::new(text, Namespace::Term))
                .is_some()
        })
        .collect()
}

/// Issue #11: a `val`/`var` constructor parameter is a field of a class
/// loaded from `.tasty`, as it is when loaded from `.class`; a plain
/// parameter is not.
#[test]
fn val_and_var_constructor_parameters_are_entered_as_fields() {
    assert_eq!(entered_fields("P", &["a", "b", "c"]), vec!["a", "c"]);
    assert_eq!(entered_fields("Q", &["d", "e"]), vec!["d", "e"]);
    assert_eq!(entered_fields("Body", &["x", "inBody"]), vec!["inBody"]);
}

/// The `ClassInfo` contract both adapters build (docs/dotty-core-design.md):
/// the canonical `no_prefix`, the class itself, and a declaration scope owned
/// by that class. Parent graphs may differ (TASTy keeps richer types).
fn assert_core_class_info_contract(
    store: &SemanticStore,
    definitions: dotty_core::Definitions,
    class: SymbolId,
) {
    let info = class_info(store, class);
    assert_eq!(info.prefix, definitions.no_prefix);
    assert_eq!(info.class, class);
    assert_eq!(store.scopes.get(info.declarations).owner, Some(class));
    for parent in &info.parents {
        assert!(store.types.get(*parent).reference_symbol().is_some());
    }
    assert_eq!(info.self_type, None);
}

/// Declares `names` as unresolved-until-needed classes of the package `path`,
/// as the unpickler's own tests do for the classes a fixture mentions.
fn stub_classes(
    store: &mut SemanticStore,
    packages: &mut dotty_core::Packages,
    path: &[&str],
    names: &[&str],
) {
    use dotty_core::{Symbol, SymbolFlags, SymbolLinks, SymbolOrigin, Visibility};
    let package = packages
        .enter(store, SymbolOrigin::Synthetic, path)
        .pop()
        .unwrap();
    for class in names {
        let name = Name::new(store.names.intern(class), Namespace::Type);
        let symbol = store.symbols.alloc(Symbol {
            name,
            owner: Some(package.symbol),
            kind: SymbolKind::Class,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        store.scopes.get_mut(package.scope).enter(name, symbol);
    }
}

#[test]
fn classloader_and_tasty_unpickler_class_infos_share_one_core_contract() {
    // `InfoBase` is a parameterized trait with no self type; `Dog` is loaded
    // by the classloader from its own `.tasty`.
    const INFO_BASE: &[u8] =
        include_bytes!("../../dotty-tasty-unpickler/tests/fixtures/semantic/InfoBase.tasty");

    let mut store = SemanticStore::new();
    let (mut loader, _synthetic_jdk) =
        class_loader_over_tasty_sample("tasty-loading-convergence-synthetic-jdk", &mut store);
    let dog = loader
        .load_class(&BinaryName::from_internal("Dog"))
        .expect("Dog should load");
    let definitions = loader.definitions();
    drop(loader);

    let mut packages = dotty_core::Packages::new();
    stub_classes(&mut store, &mut packages, &["scala"], &["Any", "Nothing"]);
    stub_classes(&mut store, &mut packages, &["java", "lang"], &["Object"]);
    let file = dotty_tasty::tasty::TastyFile::parse_scala_3_9(INFO_BASE).unwrap();
    let mut unpickler = dotty_tasty_unpickler::tasty_unpickler::TastyUnpickler::with_packages(
        &file,
        &mut store,
        definitions,
        packages,
    );
    unpickler.enter_symbols().unwrap();
    let definition = file
        .ast_address_index()
        .unwrap()
        .iter_nodes()
        .filter(|node| node.tag == 131)
        .map(|node| u32::try_from(node.offset).unwrap())
        .min()
        .unwrap();
    unpickler
        .complete_symbol(definition)
        .expect("InfoBase completes");
    let base = unpickler.index().symbol_at(definition).unwrap();
    drop(unpickler);

    assert_core_class_info_contract(&store, definitions, dog);
    assert_core_class_info_contract(&store, definitions, base);
}
