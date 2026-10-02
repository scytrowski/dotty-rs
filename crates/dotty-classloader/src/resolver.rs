//! Shared-store implementation of the core symbol-resolution port.

use crate::binary_name::BinaryName;
use crate::class_path::{ClassPathEntry, safe_package_segments};
use crate::error::ClassLoadError;
use crate::loader::ClassLoader;
use crate::session::{LoadingSession, SessionCheckpoint};
use dotty_core::{
    Definitions, MemberRequest, MemberSelector, MemberSpace, ResolutionError, ResolverCheckpoint,
    SemanticStore, Symbol, SymbolId, SymbolInfo, SymbolKind, SymbolResolver, Type, TypeId,
    TypeRefTarget,
};

struct ResolverUndo {
    session: SessionCheckpoint,
    object_class: Symbol,
}

/// Resolves core symbols from a classpath while sharing the caller's store,
/// canonical builtin definitions, and positive class/package identities.
///
/// Prefixes are supported when they are package/class `TypeRef`s,
/// `Applied` types over such class references, or `ThisType`s naming one.
/// This resolver returns declarations only; member adaptation, inheritance
/// lookup, and overload ranking remain the caller's responsibility.
pub struct ClasspathSymbolResolver<E: ClassPathEntry> {
    class_path: Option<E>,
    definitions: Definitions,
    session: LoadingSession,
    undo: Vec<ResolverUndo>,
}

impl<E: ClassPathEntry> ClasspathSymbolResolver<E> {
    pub fn new(class_path: E, definitions: Definitions, session: LoadingSession) -> Self {
        Self {
            class_path: Some(class_path),
            definitions,
            session,
            undo: Vec::new(),
        }
    }

    /// Hands the shared positive class/package cache to another resolver.
    pub fn into_session(self) -> LoadingSession {
        self.session
    }

    fn transact<T>(
        &mut self,
        store: &mut SemanticStore,
        operation: impl FnOnce(&mut Self, &mut SemanticStore) -> Result<Option<T>, ResolutionError>,
    ) -> Result<Option<T>, ResolutionError> {
        let store_checkpoint = store.checkpoint();
        let session_checkpoint = self.session.checkpoint();
        let object_class = store.symbols.get(self.definitions.object_class).clone();

        match operation(self, store) {
            Ok(Some(value)) => {
                let changed = self.session.checkpoint() != session_checkpoint
                    || *store.symbols.get(self.definitions.object_class) != object_class;
                if changed {
                    self.undo.push(ResolverUndo {
                        session: session_checkpoint,
                        object_class,
                    });
                }
                Ok(Some(value))
            }
            Ok(None) => {
                self.rollback_operation(store, store_checkpoint, session_checkpoint, object_class);
                Ok(None)
            }
            Err(error) => {
                self.rollback_operation(store, store_checkpoint, session_checkpoint, object_class);
                Err(error)
            }
        }
    }

    fn rollback_operation(
        &mut self,
        store: &mut SemanticStore,
        store_checkpoint: dotty_core::StoreCheckpoint,
        session_checkpoint: SessionCheckpoint,
        object_class: Symbol,
    ) {
        self.session.rollback_to(store, session_checkpoint);
        *store.symbols.get_mut(self.definitions.object_class) = object_class;
        store.rollback_to(store_checkpoint);
    }

    fn load_class(
        &mut self,
        store: &mut SemanticStore,
        name: &BinaryName,
    ) -> Result<SymbolId, ClassLoadError> {
        let class_path = self
            .class_path
            .take()
            .expect("resolver operation is exclusive");
        let session = std::mem::replace(&mut self.session, LoadingSession::new());
        let mut loader =
            ClassLoader::with_definitions(class_path, store, self.definitions, session);
        let result = loader.load_class(name);
        let (class_path, session) = loader.into_parts();
        self.class_path = Some(class_path);
        self.session = session;
        result
    }

    fn package_scope(&self, package: SymbolId) -> Option<dotty_core::ScopeId> {
        self.session.packages.scope_of(package)
    }

    fn declaration_scope(
        &self,
        store: &SemanticStore,
        class: SymbolId,
    ) -> Option<dotty_core::ScopeId> {
        if !store.symbols.contains(class) {
            return None;
        }
        let SymbolInfo::Complete(info) = store.symbols.get(class).info else {
            return None;
        };
        let Type::ClassInfo(info) = store.types.get(info) else {
            return None;
        };
        Some(info.declarations)
    }

    fn resolve_target(
        &mut self,
        store: &mut SemanticStore,
        target: PrefixTarget,
    ) -> Result<Option<(SymbolId, dotty_core::ScopeId)>, ResolutionError> {
        let (symbol, scope) = match target {
            PrefixTarget::Package(package) => {
                let Some(scope) = self.package_scope(package) else {
                    return Ok(None);
                };
                if !store.scopes.contains(scope) {
                    return Err(ResolutionError::Malformed {
                        reason: format!("package {package:?} has an invalid declaration scope"),
                    });
                }
                (package, scope)
            }
            PrefixTarget::Class(class) => {
                let mut scope = self.declaration_scope(store, class);
                if scope.is_none() && class == self.definitions.object_class {
                    match self.load_class(store, &BinaryName::from_internal("java/lang/Object")) {
                        Ok(loaded) if loaded == class => {
                            scope = self.declaration_scope(store, class);
                        }
                        Ok(_) => {
                            return Err(ResolutionError::Malformed {
                                reason:
                                    "loading java/lang/Object changed its canonical symbol identity"
                                        .to_owned(),
                            });
                        }
                        Err(ClassLoadError::NotFound(_)) => return Ok(None),
                        Err(error) => {
                            return Err(ResolutionError::Malformed {
                                reason: error.to_string(),
                            });
                        }
                    }
                }
                let Some(scope) = scope else {
                    // A symbol already present in the caller's store is its
                    // canonical identity. If it is incomplete, this adapter
                    // cannot complete it by loading a second symbol with the
                    // same binary name.
                    return Ok(None);
                };
                if !store.scopes.contains(scope) {
                    return Err(ResolutionError::Malformed {
                        reason: format!("class {class:?} has an invalid declaration scope"),
                    });
                }
                (class, scope)
            }
        };
        Ok(Some((symbol, scope)))
    }

    fn prefix_target(
        &self,
        store: &SemanticStore,
        ty: TypeId,
    ) -> Result<Option<PrefixTarget>, ResolutionError> {
        self.prefix_target_bounded(store, ty, 0)
    }

    fn prefix_target_bounded(
        &self,
        store: &SemanticStore,
        ty: TypeId,
        depth: u8,
    ) -> Result<Option<PrefixTarget>, ResolutionError> {
        if depth >= 16 || !store.types.contains(ty) {
            return Ok(None);
        }
        match store.types.get(ty) {
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } => self.symbol_target(store, *symbol),
            Type::ThisType { class } => self.symbol_target(store, *class),
            Type::Applied { tycon, .. } => self.prefix_target_bounded(store, *tycon, depth + 1),
            _ => Ok(None),
        }
    }

    fn symbol_target(
        &self,
        store: &SemanticStore,
        symbol: SymbolId,
    ) -> Result<Option<PrefixTarget>, ResolutionError> {
        let Some(symbol_data) = store
            .symbols
            .contains(symbol)
            .then(|| store.symbols.get(symbol))
        else {
            return Err(ResolutionError::Malformed {
                reason: format!("prefix refers to unknown symbol {symbol:?}"),
            });
        };
        if symbol_data.kind == SymbolKind::Package {
            return Ok(Some(PrefixTarget::Package(symbol)));
        }
        if !matches!(
            symbol_data.kind,
            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
        ) {
            return Ok(None);
        }
        Ok(Some(PrefixTarget::Class(symbol)))
    }

    fn compiler_builtin_class_alias(
        &self,
        store: &SemanticStore,
        package: SymbolId,
        member: dotty_core::Name,
    ) -> Option<SymbolId> {
        if !member.is_type() {
            return None;
        }
        let package_path = package_path_of_symbol(store, package)?;
        if package_path.as_slice() != ["scala"] {
            return None;
        }

        match store.names.resolve(member.text()) {
            "Any" => Some(self.definitions.any_class),
            "AnyRef" => Some(self.definitions.object_class),
            "Nothing" => Some(self.definitions.nothing_class),
            _ => None,
        }
    }

    fn resolve_member_inner(
        &mut self,
        store: &mut SemanticStore,
        request: &MemberRequest,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        if !matches!(request.selector, MemberSelector::Unique) {
            return Ok(None);
        }
        let owner_type = match request.space {
            MemberSpace::Prefix => request.prefix,
            MemberSpace::Explicit(owner) => owner,
            _ => return Ok(None),
        };
        let Some(target) = self.prefix_target(store, owner_type)? else {
            return Ok(None);
        };
        let Some((resolved_owner, scope)) = self.resolve_target(store, target)? else {
            return Ok(None);
        };
        if store.symbols.get(resolved_owner).kind == SymbolKind::Package
            && let Some(alias) =
                self.compiler_builtin_class_alias(store, resolved_owner, request.name)
        {
            return Ok(Some(alias));
        }
        let candidates = store.scopes.get(scope).lookup_all(&request.name);
        match candidates {
            [] if store.symbols.get(resolved_owner).kind == SymbolKind::Package
                && request.name.is_type() =>
            {
                let package_path =
                    package_path_of_symbol(store, resolved_owner).ok_or_else(|| {
                        ResolutionError::Malformed {
                            reason: "package prefix has an invalid owner chain".to_owned(),
                        }
                    })?;
                let simple_name = store.names.resolve(request.name.text());
                let binary_name = if package_path.is_empty() {
                    simple_name.to_owned()
                } else {
                    format!("{}/{simple_name}", package_path.join("/"))
                };
                let binary_name = BinaryName::from_internal(binary_name);
                if !binary_name.is_path_safe() {
                    return Ok(None);
                }
                match self.load_class(store, &binary_name) {
                    Ok(symbol) => Ok(Some(symbol)),
                    Err(ClassLoadError::NotFound(_)) => Ok(None),
                    Err(error) => Err(ResolutionError::Malformed {
                        reason: error.to_string(),
                    }),
                }
            }
            [] => Ok(None),
            [symbol] => Ok(Some(*symbol)),
            many => Err(ResolutionError::Ambiguous {
                candidates: many.len(),
            }),
        }
    }

    fn resolve_package_inner(
        &mut self,
        store: &mut SemanticStore,
        path: &[&str],
    ) -> Result<Option<SymbolId>, ResolutionError> {
        if !safe_package_segments(path) {
            return Ok(None);
        }
        if let Some(package) = self.session.packages.get(path) {
            return Ok(Some(package));
        }
        if !self
            .class_path
            .as_ref()
            .expect("resolver operation is exclusive")
            .contains_package(path)
            .map_err(|error| ResolutionError::Malformed {
                reason: error.to_string(),
            })?
        {
            return Ok(None);
        }
        Ok(Some(
            self.session
                .packages
                .resolve_package(store, &path.join("/")),
        ))
    }
}

impl<E: ClassPathEntry> SymbolResolver for ClasspathSymbolResolver<E> {
    fn checkpoint(&self) -> ResolverCheckpoint {
        ResolverCheckpoint::new(self.undo.len() as u64)
    }

    fn rollback_to(&mut self, store: &mut SemanticStore, checkpoint: ResolverCheckpoint) {
        let target = checkpoint.token() as usize;
        while self.undo.len() > target {
            let undo = self.undo.pop().expect("length checked");
            self.session.rollback_to(store, undo.session);
            *store.symbols.get_mut(self.definitions.object_class) = undo.object_class;
        }
    }

    fn resolve_member(
        &mut self,
        store: &mut SemanticStore,
        request: &MemberRequest,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        self.transact(store, |resolver, store| {
            resolver.resolve_member_inner(store, request)
        })
    }

    fn resolve_package(
        &mut self,
        store: &mut SemanticStore,
        path: &[&str],
    ) -> Result<Option<SymbolId>, ResolutionError> {
        self.transact(store, |resolver, store| {
            resolver.resolve_package_inner(store, path)
        })
    }
}

enum PrefixTarget {
    Package(SymbolId),
    Class(SymbolId),
}

fn package_path_of_symbol(store: &SemanticStore, package: SymbolId) -> Option<Vec<String>> {
    let mut current = package;
    let mut segments = Vec::new();
    let mut visited = std::collections::HashSet::new();
    loop {
        if !visited.insert(current) {
            return None;
        }
        let item = store.symbols.get(current);
        if item.kind != SymbolKind::Package {
            return None;
        }
        let text = store.names.resolve(item.name.text());
        if !text.is_empty() {
            segments.push(text.to_owned());
        }
        let Some(owner) = item.owner else {
            break;
        };
        current = owner;
    }
    segments.reverse();
    Some(segments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::class_path::{
        ClassFormat, ClassOrigin, ClassPathEntry, ClassPathError, ClassResource,
    };
    use crate::jdk_class_path::JdkClassPath;
    use crate::jmod_class_path::JmodClassPath;
    use dotty_core::{ClassInfo, MemberSelector, Name, Namespace, Scope};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn fixture_path(relative: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
    }

    struct FixtureClassPath(JdkClassPath);

    impl ClassPathEntry for FixtureClassPath {
        fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
            if name.as_internal() == "java/lang/Object" {
                return Ok(Some(ClassResource::new(
                    test_class_stub("java/lang/Object", 0x0021),
                    ClassFormat::Class,
                    ClassOrigin::Directory(PathBuf::from("<test-stub>")),
                )));
            }
            if name.as_internal() == "java/lang/Runnable" {
                return Ok(Some(ClassResource::new(
                    test_class_stub("java/lang/Runnable", 0x0601),
                    ClassFormat::Class,
                    ClassOrigin::Directory(PathBuf::from("<test-stub>")),
                )));
            }
            self.0.find_class(name)
        }

        fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            if package == ["java", "lang"] {
                return Ok(true);
            }
            self.0.contains_package(package)
        }
    }

    struct EmptyCountingClassPath(Arc<AtomicUsize>);

    impl ClassPathEntry for EmptyCountingClassPath {
        fn find_class(&self, _name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(None)
        }

        fn contains_package(&self, _package: &[&str]) -> Result<bool, ClassPathError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(false)
        }
    }

    struct RealObjectBootstrapClassPath(JmodClassPath);

    impl ClassPathEntry for RealObjectBootstrapClassPath {
        fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
            match name.as_internal() {
                "java/lang/Object" => self.0.find_class(name),
                "java/lang/Class" => Ok(Some(ClassResource::new(
                    test_class_stub_with_super("java/lang/Class", "java/lang/Object"),
                    ClassFormat::Class,
                    ClassOrigin::Directory(PathBuf::from("<bootstrap-stub>")),
                ))),
                "java/lang/String"
                | "java/lang/CloneNotSupportedException"
                | "java/lang/InterruptedException" => Ok(Some(ClassResource::new(
                    test_class_stub(name.as_internal(), 0x0021),
                    ClassFormat::Class,
                    ClassOrigin::Directory(PathBuf::from("<descriptor-stub>")),
                ))),
                _ => Ok(None),
            }
        }

        fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            Ok(package == ["java", "lang"])
        }
    }

    struct Issue605ClassPath;

    impl ClassPathEntry for Issue605ClassPath {
        fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
            let bytes = match name.as_internal() {
                "java/lang/Object" => test_class_with_object_fields(
                    "java/lang/Object",
                    None,
                    &[
                        ("class", "java/lang/Class"),
                        ("broken", "missing/NoSuchClass"),
                    ],
                ),
                "java/lang/Class" => {
                    test_class_stub_with_super("java/lang/Class", "java/lang/Object")
                }
                "cycle/A" => test_class_stub_with_super("cycle/A", "cycle/B"),
                "cycle/B" => test_class_stub_with_super("cycle/B", "cycle/A"),
                _ => return Ok(None),
            };
            Ok(Some(ClassResource::new(
                bytes,
                ClassFormat::Class,
                ClassOrigin::Directory(PathBuf::from("<issue-605-fixture>")),
            )))
        }

        fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            Ok(package == ["java", "lang"] || package == ["cycle"])
        }
    }

    fn test_class_stub(binary_name: &str, access_flags: u16) -> Vec<u8> {
        let name = binary_name.as_bytes();
        let mut bytes = vec![0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 61, 0, 3, 1];
        bytes.extend_from_slice(&(name.len() as u16).to_be_bytes());
        bytes.extend_from_slice(name);
        bytes.extend_from_slice(&[7, 0, 1]);
        bytes.extend_from_slice(&access_flags.to_be_bytes());
        bytes.extend_from_slice(&[0, 2, 0, 0]);
        bytes.extend_from_slice(&[0; 8]);
        bytes
    }

    fn test_class_stub_with_super(binary_name: &str, super_name: &str) -> Vec<u8> {
        let this_name = binary_name.as_bytes();
        let super_name = super_name.as_bytes();
        let mut bytes = vec![0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 61, 0, 5, 1];
        bytes.extend_from_slice(&(this_name.len() as u16).to_be_bytes());
        bytes.extend_from_slice(this_name);
        bytes.extend_from_slice(&[7, 0, 1, 1]);
        bytes.extend_from_slice(&(super_name.len() as u16).to_be_bytes());
        bytes.extend_from_slice(super_name);
        bytes.extend_from_slice(&[7, 0, 3]);
        bytes.extend_from_slice(&[0, 0x21, 0, 2, 0, 4]);
        bytes.extend_from_slice(&[0; 8]);
        bytes
    }

    fn test_class_with_object_fields(
        binary_name: &str,
        super_name: Option<&str>,
        fields: &[(&str, &str)],
    ) -> Vec<u8> {
        fn push_utf8(pool: &mut Vec<u8>, next_index: &mut u16, text: &str) -> u16 {
            let index = *next_index;
            pool.push(1);
            pool.extend_from_slice(&(text.len() as u16).to_be_bytes());
            pool.extend_from_slice(text.as_bytes());
            *next_index += 1;
            index
        }

        fn push_class(pool: &mut Vec<u8>, next_index: &mut u16, name_index: u16) -> u16 {
            let index = *next_index;
            pool.extend_from_slice(&[7]);
            pool.extend_from_slice(&name_index.to_be_bytes());
            *next_index += 1;
            index
        }

        let mut pool = Vec::new();
        let mut next_index = 1;
        let this_name = push_utf8(&mut pool, &mut next_index, binary_name);
        let this_class = push_class(&mut pool, &mut next_index, this_name);
        let super_class = super_name.map(|super_name| {
            let super_name = push_utf8(&mut pool, &mut next_index, super_name);
            push_class(&mut pool, &mut next_index, super_name)
        });
        let mut field_entries = Vec::new();
        for (field_name, field_type) in fields {
            let field_name = push_utf8(&mut pool, &mut next_index, field_name);
            let descriptor = push_utf8(&mut pool, &mut next_index, &format!("L{field_type};"));
            field_entries.extend_from_slice(&[0, 1]);
            field_entries.extend_from_slice(&field_name.to_be_bytes());
            field_entries.extend_from_slice(&descriptor.to_be_bytes());
            field_entries.extend_from_slice(&[0, 0]);
        }

        let mut bytes = vec![0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 61];
        bytes.extend_from_slice(&next_index.to_be_bytes());
        bytes.extend_from_slice(&pool);
        bytes.extend_from_slice(&[0, 0x21]);
        bytes.extend_from_slice(&this_class.to_be_bytes());
        bytes.extend_from_slice(&super_class.unwrap_or(0).to_be_bytes());
        bytes.extend_from_slice(&[0, 0]);
        bytes.extend_from_slice(&(fields.len() as u16).to_be_bytes());
        bytes.extend_from_slice(&field_entries);
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes
    }

    fn class_with_missing_supertype() -> Vec<u8> {
        let this_name = b"bad/Broken";
        let super_name = b"missing/Dependency";
        let mut bytes = vec![0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 61, 0, 5];
        bytes.push(1);
        bytes.extend_from_slice(&(this_name.len() as u16).to_be_bytes());
        bytes.extend_from_slice(this_name);
        bytes.extend_from_slice(&[7, 0, 1, 1]);
        bytes.extend_from_slice(&(super_name.len() as u16).to_be_bytes());
        bytes.extend_from_slice(super_name);
        bytes.extend_from_slice(&[7, 0, 3]);
        bytes.extend_from_slice(&[0, 0x21, 0, 2, 0, 4]);
        bytes.extend_from_slice(&[0; 8]);
        bytes
    }

    fn setup() -> (
        SemanticStore,
        Definitions,
        ClasspathSymbolResolver<FixtureClassPath>,
    ) {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let path = FixtureClassPath(
            JdkClassPath::new(fixture_path("tests/fixtures/jdk_classpath")).unwrap(),
        );
        let resolver = ClasspathSymbolResolver::new(path, definitions, LoadingSession::new());
        (store, definitions, resolver)
    }

    #[test]
    fn resolves_only_packages_with_classpath_evidence_and_reuses_their_identity() {
        let (mut store, _, mut resolver) = setup();
        let first = resolver
            .resolve_package(&mut store, &["pool"])
            .unwrap()
            .unwrap();
        let second = resolver
            .resolve_package(&mut store, &["pool"])
            .unwrap()
            .unwrap();

        assert_eq!(first, second);
        assert_eq!(store.symbols.get(first).kind, SymbolKind::Package);
        assert!(
            resolver
                .resolve_package(&mut store, &["not_a_package"])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn compiler_builtin_aliases_are_scala_type_members_without_classpath_probes() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let probes = Arc::new(AtomicUsize::new(0));
        let class_path = EmptyCountingClassPath(Arc::clone(&probes));
        let mut resolver =
            ClasspathSymbolResolver::new(class_path, definitions, LoadingSession::new());
        let scala = resolver
            .session
            .packages
            .resolve_package(&mut store, "scala");
        let scala_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, scala));
        let scala_scope = resolver.session.packages.scope_of(scala).unwrap();
        let any_ref_name = Name::new(store.names.intern("AnyRef"), Namespace::Type);
        let noncanonical_any_ref = store.symbols.alloc(Symbol {
            name: any_ref_name,
            owner: Some(scala),
            kind: SymbolKind::Class,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Missing,
            origin: dotty_core::SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        });
        store
            .scopes
            .get_mut(scala_scope)
            .enter(any_ref_name, noncanonical_any_ref);

        for (name, expected) in [
            ("Any", definitions.any_class),
            ("AnyRef", definitions.object_class),
            ("Nothing", definitions.nothing_class),
        ] {
            let name = Name::new(store.names.intern(name), Namespace::Type);
            let resolved = resolver
                .resolve_member(
                    &mut store,
                    &MemberRequest {
                        prefix: scala_prefix,
                        name,
                        selector: MemberSelector::Unique,
                        space: MemberSpace::Prefix,
                    },
                )
                .unwrap();
            assert_eq!(resolved, Some(expected));
        }

        assert_eq!(
            probes.load(Ordering::Relaxed),
            0,
            "compiler-only aliases never reach classpath I/O"
        );

        let scala_int = Name::new(store.names.intern("Int"), Namespace::Type);
        assert!(
            resolver
                .resolve_member(
                    &mut store,
                    &MemberRequest {
                        prefix: scala_prefix,
                        name: scala_int,
                        selector: MemberSelector::Unique,
                        space: MemberSpace::Prefix,
                    },
                )
                .unwrap()
                .is_none()
        );
        assert_eq!(
            probes.load(Ordering::Relaxed),
            1,
            "loadable Scala classes are not builtin aliases"
        );

        let other = resolver
            .session
            .packages
            .resolve_package(&mut store, "example");
        let other_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, other));
        let any_ref = Name::new(store.names.intern("AnyRef"), Namespace::Type);
        assert!(
            resolver
                .resolve_member(
                    &mut store,
                    &MemberRequest {
                        prefix: other_prefix,
                        name: any_ref,
                        selector: MemberSelector::Unique,
                        space: MemberSpace::Prefix,
                    },
                )
                .unwrap()
                .is_none()
        );
        assert_eq!(
            probes.load(Ordering::Relaxed),
            2,
            "alias ownership is exactly scala.AnyRef"
        );

        let any_ref_term = Name::new(store.names.intern("AnyRef"), Namespace::Term);
        assert!(
            resolver
                .resolve_member(
                    &mut store,
                    &MemberRequest {
                        prefix: scala_prefix,
                        name: any_ref_term,
                        selector: MemberSelector::Unique,
                        space: MemberSpace::Prefix,
                    },
                )
                .unwrap()
                .is_none()
        );
        assert_eq!(
            probes.load(Ordering::Relaxed),
            2,
            "aliases occupy only the type namespace"
        );
    }

    #[test]
    #[ignore = "requires JAVA_HOME pointing to a JDK with jmods/java.base.jmod"]
    fn real_jdk_object_and_get_class_materialize_through_the_resolver() {
        let java_home = std::env::var_os("JAVA_HOME")
            .expect("set JAVA_HOME to run this real-JDK classpath regression");
        let java_base = PathBuf::from(java_home).join("jmods/java.base.jmod");
        let class_path = RealObjectBootstrapClassPath(
            crate::jmod_class_path::JmodClassPath::new(java_base)
                .expect("JAVA_HOME should contain a readable java.base.jmod"),
        );
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let object_class = definitions.object_class;
        let mut resolver =
            ClasspathSymbolResolver::new(class_path, definitions, LoadingSession::new());
        let get_class = Name::new(store.names.intern("getClass"), Namespace::Term);

        let method = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: definitions.object_type,
                    name: get_class,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .expect("loading java/lang/Object through the JDK classpath should succeed")
            .expect("java.lang.Object should declare getClass");

        assert_eq!(store.symbols.get(method).owner, Some(object_class));
        assert!(matches!(
            store.symbols.get(method).origin,
            dotty_core::SymbolOrigin::Classfile(_)
        ));
        let class_symbol = *resolver
            .session
            .resolved
            .get(&BinaryName::from_internal("java/lang/Class"))
            .expect("getClass's descriptor should materialize java/lang/Class");
        assert_ne!(class_symbol, object_class);
        assert!(matches!(
            store.symbols.get(class_symbol).info,
            SymbolInfo::Complete(info)
                if matches!(store.types.get(info), Type::ClassInfo(class_info)
                    if class_info.parents.iter().any(|parent| {
                        store.types.get(*parent).reference_symbol() == Some(object_class)
                    }))
        ));
    }

    #[test]
    fn resolver_rollback_removes_a_successful_dependency_of_a_failed_object_load() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let canonical_object = store.symbols.get(definitions.object_class).clone();
        let mut resolver =
            ClasspathSymbolResolver::new(Issue605ClassPath, definitions, LoadingSession::new());
        let java_lang = resolver
            .resolve_package(&mut store, &["java", "lang"])
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, java_lang));
        let package_scope = resolver.session.packages.scope_of(java_lang).unwrap();

        for _ in 0..2 {
            let object_name = Name::new(store.names.intern("Object"), Namespace::Type);
            let error = resolver
                .resolve_member(
                    &mut store,
                    &MemberRequest {
                        prefix: package_prefix,
                        name: object_name,
                        selector: MemberSelector::Unique,
                        space: MemberSpace::Prefix,
                    },
                )
                .expect_err("Object's second field deliberately has no classpath class");
            assert!(matches!(
                error,
                ResolutionError::Malformed { reason }
                    if reason.contains("missing/NoSuchClass")
            ));

            assert_eq!(
                store.symbols.get(definitions.object_class),
                &canonical_object
            );
            for binary_name in ["java/lang/Object", "java/lang/Class"] {
                assert!(
                    !resolver
                        .session
                        .resolved
                        .contains_key(&BinaryName::from_internal(binary_name)),
                    "failed transaction left {binary_name} in the positive cache"
                );
            }
            assert!(resolver.session.metadata.is_empty());
            for class_name in ["Object", "Class"] {
                let name = Name::new(store.names.intern(class_name), Namespace::Type);
                assert!(store.scopes.get(package_scope).lookup_all(&name).is_empty());
            }
        }
    }

    #[test]
    fn resolver_rollback_does_not_cache_a_genuine_inheritance_cycle() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let object_before = store.symbols.get(definitions.object_class).clone();
        let mut resolver =
            ClasspathSymbolResolver::new(Issue605ClassPath, definitions, LoadingSession::new());
        let cycle_package = resolver
            .resolve_package(&mut store, &["cycle"])
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, cycle_package));

        let class_a_name = Name::new(store.names.intern("A"), Namespace::Type);
        let error = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: class_a_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .expect_err("A extends B, B extends A must remain an error");

        assert!(matches!(
            error,
            ResolutionError::Malformed { reason }
                if reason.contains("circular inheritance involving cycle/A")
        ));
        assert_eq!(store.symbols.get(definitions.object_class), &object_before);
        for binary_name in ["cycle/A", "cycle/B"] {
            assert!(
                !resolver
                    .session
                    .resolved
                    .contains_key(&BinaryName::from_internal(binary_name)),
                "failed cycle left {binary_name} in the positive cache"
            );
        }
        let package_scope = resolver.session.packages.scope_of(cycle_package).unwrap();
        for class_name in ["A", "B"] {
            let name = Name::new(store.names.intern(class_name), Namespace::Type);
            assert!(store.scopes.get(package_scope).lookup_all(&name).is_empty());
        }
    }

    #[test]
    fn package_and_class_prefixes_resolve_class_and_term_members() {
        let (mut store, definitions, mut resolver) = setup();
        let package = resolver
            .resolve_package(&mut store, &["pool"])
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let class_name = store.names.intern("PoolSample");
        let class = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: Name::new(class_name, Namespace::Type),
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let class_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, class));
        let run_name = store.names.intern("run");
        let run = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: class_prefix,
                    name: Name::new(run_name, Namespace::Term),
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let repeated = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: class_prefix,
                    name: Name::new(run_name, Namespace::Term),
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();

        assert_eq!(run, repeated);
        assert_eq!(
            store.names.resolve(store.symbols.get(run).name.text()),
            "run"
        );
    }

    #[test]
    fn applied_prefix_keeps_member_ownership_on_its_underlying_class() {
        let (mut store, definitions, mut resolver) = setup();
        let class = resolver
            .resolve_package(&mut store, &["pool"])
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, class));
        let class_name = store.names.intern("PoolSample");
        let class_symbol = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: Name::new(class_name, Namespace::Type),
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let tycon = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, class_symbol));
        let applied = store.types.alloc(Type::Applied {
            tycon,
            args: vec![definitions.int],
        });

        let run_name = store.names.intern("run");
        let run = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: applied,
                    name: Name::new(run_name, Namespace::Term),
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap();
        assert!(run.is_some());
        assert!(
            matches!(store.types.get(applied), Type::Applied { args, .. } if args == &vec![definitions.int])
        );
        let this_type = store.types.alloc(Type::ThisType {
            class: class_symbol,
        });
        assert!(
            resolver
                .resolve_member(
                    &mut store,
                    &MemberRequest {
                        prefix: this_type,
                        name: Name::new(run_name, Namespace::Term),
                        selector: MemberSelector::Unique,
                        space: MemberSpace::Prefix,
                    },
                )
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn explicit_owner_is_used_without_falling_back_to_the_prefix() {
        let (mut store, definitions, mut resolver) = setup();
        let package = resolver
            .resolve_package(&mut store, &["pool"])
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let class_name = store.names.intern("PoolSample");
        let class = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: Name::new(class_name, Namespace::Type),
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let class_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, class));

        let run_name = store.names.intern("run");
        let result = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: class_prefix,
                    name: Name::new(run_name, Namespace::Term),
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Explicit(package_prefix),
                },
            )
            .unwrap();

        assert_eq!(result, None);
    }

    #[test]
    fn resolver_rollback_discards_new_package_state_before_store_rollback() {
        let (mut store, _, mut resolver) = setup();
        let store_checkpoint = store.checkpoint();
        let resolver_checkpoint = resolver.checkpoint();
        let package = resolver
            .resolve_package(&mut store, &["pool"])
            .unwrap()
            .unwrap();

        resolver.rollback_to(&mut store, resolver_checkpoint);
        store.rollback_to(store_checkpoint);
        let resolved_again = resolver
            .resolve_package(&mut store, &["pool"])
            .unwrap()
            .unwrap();

        assert_eq!(package, resolved_again);
        assert!(store.symbols.contains(resolved_again));
    }

    #[test]
    fn term_and_type_namespaces_are_resolved_independently() {
        let (mut store, definitions, mut resolver) = setup();
        let package = resolver
            .resolve_package(&mut store, &["pool"])
            .unwrap()
            .unwrap();
        let package_scope = resolver.session.packages.scope_of(package).unwrap();
        let shared_text = store.names.intern("PoolSample");
        let term_name = Name::new(shared_text, Namespace::Term);
        let term_symbol = store.symbols.alloc(Symbol {
            name: term_name,
            owner: Some(package),
            kind: SymbolKind::Value,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Missing,
            origin: dotty_core::SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        });
        store
            .scopes
            .get_mut(package_scope)
            .enter(term_name, term_symbol);
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));

        let type_symbol = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: Name::new(shared_text, Namespace::Type),
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let term_result = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: term_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap();

        assert_ne!(type_symbol, term_symbol);
        assert_eq!(term_result, Some(term_symbol));
    }

    #[test]
    fn a_unique_overload_request_reports_ambiguity() {
        let (mut store, definitions, mut resolver) = setup();
        let package = resolver
            .resolve_package(&mut store, &["pool"])
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let class_name = store.names.intern("PoolSample");
        let class = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: Name::new(class_name, Namespace::Type),
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let declarations = resolver.declaration_scope(&store, class).unwrap();
        let run_text = store.names.intern("run");
        let run_name = Name::new(run_text, Namespace::Term);
        let overload = store.symbols.alloc(Symbol {
            name: run_name,
            owner: Some(class),
            kind: SymbolKind::Method,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Missing,
            origin: dotty_core::SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        });
        store.scopes.get_mut(declarations).enter(run_name, overload);
        let class_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, class));

        let error = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: class_prefix,
                    name: run_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap_err();

        assert_eq!(error, ResolutionError::Ambiguous { candidates: 2 });
    }

    #[test]
    fn a_failed_class_load_leaves_no_store_or_session_changes() {
        struct BadClassPath;

        impl ClassPathEntry for BadClassPath {
            fn find_class(
                &self,
                name: &BinaryName,
            ) -> Result<Option<ClassResource>, ClassPathError> {
                if name.as_internal() == "bad/Broken" {
                    Ok(Some(ClassResource::new(
                        class_with_missing_supertype(),
                        ClassFormat::Class,
                        ClassOrigin::Directory(PathBuf::from("<bad-fixture>")),
                    )))
                } else {
                    Ok(None)
                }
            }

            fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
                Ok(package == ["bad"])
            }
        }

        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver =
            ClasspathSymbolResolver::new(BadClassPath, definitions, LoadingSession::new());
        let package = resolver
            .resolve_package(&mut store, &["bad"])
            .unwrap()
            .unwrap();
        let prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let broken = store.names.intern("Broken");
        let checkpoint = store.checkpoint();

        let result = resolver.resolve_member(
            &mut store,
            &MemberRequest {
                prefix,
                name: Name::new(broken, Namespace::Type),
                selector: MemberSelector::Unique,
                space: MemberSpace::Prefix,
            },
        );

        assert!(matches!(result, Err(ResolutionError::Malformed { .. })));
        assert_eq!(store.checkpoint(), checkpoint);
        let package_scope = resolver.session.packages.scope_of(package).unwrap();
        assert!(
            store
                .scopes
                .get(package_scope)
                .lookup_all(&Name::new(broken, Namespace::Type))
                .is_empty()
        );
        assert_eq!(
            resolver.resolve_package(&mut store, &["bad"]).unwrap(),
            Some(package)
        );
    }

    #[test]
    fn resolver_instances_reuse_class_and_package_identities_from_one_session() {
        let (mut store, definitions, resolver) = setup();
        let mut first = resolver;
        let package = first
            .resolve_package(&mut store, &["pool"])
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let class_name = store.names.intern("PoolSample");
        let class = first
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: Name::new(class_name, Namespace::Type),
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let session = first.into_session();
        let class_path = FixtureClassPath(
            JdkClassPath::new(fixture_path("tests/fixtures/jdk_classpath")).unwrap(),
        );
        let mut second = ClasspathSymbolResolver::new(class_path, definitions, session);
        let package_again = second
            .resolve_package(&mut store, &["pool"])
            .unwrap()
            .unwrap();
        let class_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, class));
        let run_name = store.names.intern("run");
        let run = second
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: class_prefix,
                    name: Name::new(run_name, Namespace::Term),
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();

        assert_eq!(package, package_again);
        assert_eq!(store.symbols.get(class).owner, Some(package));
        assert_eq!(
            store.names.resolve(store.symbols.get(run).name.text()),
            "run"
        );
    }

    #[test]
    fn outer_rollback_removes_loaded_class_cache_and_package_scope_links() {
        let (mut store, definitions, mut resolver) = setup();
        let package = resolver
            .resolve_package(&mut store, &["pool"])
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let class_name = store.names.intern("PoolSample");
        let package_scope = resolver.session.packages.scope_of(package).unwrap();
        let object_before = store.symbols.get(definitions.object_class).clone();
        let store_checkpoint = store.checkpoint();
        let resolver_checkpoint = resolver.checkpoint();

        let class = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: Name::new(class_name, Namespace::Type),
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        assert!(
            store
                .scopes
                .get(package_scope)
                .lookup_all(&Name::new(class_name, Namespace::Type))
                .contains(&class)
        );

        resolver.rollback_to(&mut store, resolver_checkpoint);
        store.rollback_to(store_checkpoint);

        assert!(
            store
                .scopes
                .get(package_scope)
                .lookup_all(&Name::new(class_name, Namespace::Type))
                .is_empty()
        );
        assert_eq!(store.symbols.get(definitions.object_class), &object_before);
        assert!(!store.symbols.contains(class));
        let reloaded = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: Name::new(class_name, Namespace::Type),
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(reloaded, class);
    }

    #[test]
    fn class_prefix_reuses_completed_symbol_already_in_the_shared_store() {
        let (mut store, definitions, mut resolver) = setup();
        let package = resolver
            .resolve_package(&mut store, &["pool"])
            .unwrap()
            .unwrap();
        let class_name = Name::new(store.names.intern("AlreadyThere"), Namespace::Type);
        let class = store.symbols.alloc(Symbol {
            name: class_name,
            owner: Some(package),
            kind: SymbolKind::Class,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Missing,
            origin: dotty_core::SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        });
        let declarations = store.scopes.alloc(Scope::new(Some(class)));
        let member_name = Name::new(store.names.intern("fromTasty"), Namespace::Term);
        let member = store.symbols.alloc(Symbol {
            name: member_name,
            owner: Some(class),
            kind: SymbolKind::Method,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Missing,
            origin: dotty_core::SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        });
        store
            .scopes
            .get_mut(declarations)
            .enter(member_name, member);
        let class_info = store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: definitions.no_prefix,
            class,
            parents: Vec::new(),
            declarations,
            self_type: None,
        }));
        store.symbols.get_mut(class).info = SymbolInfo::Complete(class_info);
        let prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, class));
        let checkpoint = store.checkpoint();

        let resolved = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix,
                    name: member_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap();

        assert_eq!(resolved, Some(member));
        assert_eq!(store.checkpoint(), checkpoint);
        assert_eq!(store.symbols.get(member).owner, Some(class));
    }
}
