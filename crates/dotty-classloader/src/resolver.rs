//! Shared-store implementation of the core symbol-resolution port.

use crate::binary_name::BinaryName;
use crate::class_path::{ClassPathEntry, safe_package_segments};
use crate::error::ClassLoadError;
use crate::loader::{ClassLoadProfile, ClassLoader};
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
    load_profiles: Vec<ClassLoadProfile>,
    collect_load_profiles: bool,
}

impl<E: ClassPathEntry> ClasspathSymbolResolver<E> {
    pub fn new(class_path: E, definitions: Definitions, session: LoadingSession) -> Self {
        Self {
            class_path: Some(class_path),
            definitions,
            session,
            undo: Vec::new(),
            load_profiles: Vec::new(),
            collect_load_profiles: false,
        }
    }

    /// Enables per-request class dependency observations for corpus audits
    /// and diagnostics. Disabled by default to avoid collecting session data
    /// in ordinary compiler use.
    pub fn with_load_profile_collection(mut self) -> Self {
        self.collect_load_profiles = true;
        self
    }

    /// Hands the shared positive class/package cache to another resolver.
    pub fn into_session(self) -> LoadingSession {
        self.session
    }

    /// Returns and clears class dependency profiles collected by resolver
    /// requests since the previous call.
    pub fn take_load_profiles(&mut self) -> Vec<ClassLoadProfile> {
        std::mem::take(&mut self.load_profiles)
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
        loader.set_load_profile_collection(self.collect_load_profiles);
        let result = loader.load_class(name);
        self.load_profiles.extend(loader.take_load_profiles());
        let (class_path, session) = loader.into_parts();
        self.class_path = Some(class_path);
        self.session = session;
        result
    }

    fn package_scope(&self, package: SymbolId) -> Option<dotty_core::ScopeId> {
        self.session.packages.scope_of(package)
    }

    fn complete_class_inner(
        &mut self,
        store: &mut SemanticStore,
        symbol: SymbolId,
    ) -> Result<Option<bool>, ResolutionError> {
        let names: Vec<_> = self
            .session
            .identified
            .iter()
            .filter(|(_, id)| **id == symbol)
            .map(|(name, _)| name.clone())
            .collect();
        if names.is_empty() {
            return Ok(None);
        }
        let malformed = |reason: &str| ResolutionError::Malformed {
            reason: format!("cannot complete external class {symbol:?}: {reason}"),
        };
        if !store.symbols.contains(symbol) {
            return Err(malformed("identified symbol no longer exists"));
        }
        let item = store.symbols.get(symbol);
        if !matches!(
            item.origin,
            dotty_core::SymbolOrigin::Classfile(_) | dotty_core::SymbolOrigin::Tasty(_)
        ) {
            return Ok(None);
        }
        if !matches!(
            item.kind,
            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
        ) {
            return Err(malformed("identified symbol is not class-like"));
        }
        let name = names
            .into_iter()
            .find(|name| {
                if !item.name.is_type()
                    || store.names.resolve(item.name.text()) != name.simple_name()
                {
                    return false;
                }
                let path: Vec<_> = name
                    .package_path()
                    .split('/')
                    .filter(|s| !s.is_empty())
                    .collect();
                let Some(package) = self.session.packages.get(&path) else {
                    return false;
                };
                let Some(scope) = self.package_scope(package) else {
                    return false;
                };
                store.symbols.contains(package)
                    && store.symbols.get(package).kind == SymbolKind::Package
                    && store.scopes.contains(scope)
                    && store.scopes.get(scope).owner == Some(package)
                    && store.scopes.get(scope).lookup_all(&item.name) == [symbol]
                    && (item.owner == Some(package)
                        || is_nested_class_indexed_in_package(store, symbol, package))
            })
            .ok_or_else(|| malformed("identity does not match its canonical package/name"))?;

        match item.info {
            SymbolInfo::Missing => {
                let loaded =
                    self.load_class(store, &name)
                        .map_err(|error| ResolutionError::Malformed {
                            reason: format!("external class completion failed for {name}: {error}"),
                        })?;
                if loaded != symbol || self.session.identified.get(&name) != Some(&symbol) {
                    return Err(malformed("loading changed the canonical symbol identity"));
                }
            }
            SymbolInfo::Complete(_) => {}
            SymbolInfo::Error => return Err(malformed("identified symbol has Error info")),
            SymbolInfo::Deferred(_) => {
                return Err(malformed("identified symbol has unsupported Deferred info"));
            }
        }
        let SymbolInfo::Complete(info) = store.symbols.get(symbol).info else {
            return Err(malformed("loading did not complete ClassInfo"));
        };
        let Some(Type::ClassInfo(info)) = store.types.try_get(info) else {
            return Err(malformed("completed info is not a valid ClassInfo"));
        };
        if info.class != symbol
            || !store.scopes.contains(info.declarations)
            || store.scopes.get(info.declarations).owner != Some(symbol)
        {
            return Err(malformed(
                "ClassInfo identity or declaration scope is invalid",
            ));
        }
        self.session.cache_resolved(name, symbol);
        Ok(Some(true))
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
        let Some(Type::ClassInfo(info)) = store.types.try_get(info) else {
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
                if scope.is_none() {
                    // Stay inside resolve_member's transaction: None/ambiguity
                    // must undo even a successful prefix completion.
                    self.complete_class_inner(store, class)?;
                    scope = self.declaration_scope(store, class);
                }
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
                    if store.symbols.contains(class)
                        && matches!(store.symbols.get(class).info, SymbolInfo::Complete(_))
                    {
                        return Err(ResolutionError::Malformed {
                            reason: format!("class {class:?} has invalid ClassInfo"),
                        });
                    }
                    // Unknown/source identities are declined by the hook.
                    return Ok(None);
                };
                if !store.scopes.contains(scope) {
                    return Err(ResolutionError::Malformed {
                        reason: format!("class {class:?} has an invalid declaration scope"),
                    });
                }
                let SymbolInfo::Complete(info) = store.symbols.get(class).info else {
                    return Err(ResolutionError::Malformed {
                        reason: format!("class {class:?} lost completed info"),
                    });
                };
                let Some(Type::ClassInfo(info)) = store.types.try_get(info) else {
                    return Err(ResolutionError::Malformed {
                        reason: format!("class {class:?} has invalid ClassInfo"),
                    });
                };
                if info.class != class || store.scopes.get(scope).owner != Some(class) {
                    return Err(ResolutionError::Malformed {
                        reason: format!(
                            "class {class:?} has inconsistent ClassInfo identity/scope"
                        ),
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
            [symbol]
                if store.symbols.get(resolved_owner).kind == SymbolKind::Package
                    && request.name.is_type() =>
            {
                let member = store.symbols.get(*symbol);
                if matches!(
                    member.kind,
                    SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
                ) {
                    let owner_mismatch = member.owner != Some(resolved_owner)
                        && !is_nested_class_indexed_in_package(store, *symbol, resolved_owner);
                    if owner_mismatch || member.name != request.name {
                        return Err(ResolutionError::Malformed {
                            reason: format!(
                                "package type member {symbol:?} does not match its canonical package/name"
                            ),
                        });
                    }
                    let SymbolInfo::Complete(info) = member.info else {
                        // A source-owned class is already the canonical identity.
                        // This adapter cannot complete it in place, and loading a
                        // classpath twin would create a duplicate identity.
                        return Ok(None);
                    };
                    let Some(ty) = store.types.try_get(info) else {
                        return Err(ResolutionError::Malformed {
                            reason: format!(
                                "package class {symbol:?} has an invalid or unfilled completion type"
                            ),
                        });
                    };
                    let Type::ClassInfo(class_info) = ty else {
                        return Err(ResolutionError::Malformed {
                            reason: format!(
                                "package class {symbol:?} is complete without ClassInfo"
                            ),
                        });
                    };
                    if class_info.class != *symbol
                        || !store.scopes.contains(class_info.declarations)
                        || store.scopes.get(class_info.declarations).owner != Some(*symbol)
                    {
                        return Err(ResolutionError::Malformed {
                            reason: format!(
                                "package class {symbol:?} has invalid class identity or declaration scope"
                            ),
                        });
                    }
                }
                Ok(Some(*symbol))
            }
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
    fn complete_class(
        &mut self,
        store: &mut SemanticStore,
        symbol: SymbolId,
    ) -> Result<bool, ResolutionError> {
        self.transact(store, |resolver, store| {
            resolver.complete_class_inner(store, symbol)
        })
        .map(|completed| completed.unwrap_or(false))
    }

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

    fn enter_synthetic_package_member(
        &mut self,
        store: &mut SemanticStore,
        package: SymbolId,
        name: dotty_core::Name,
        member: SymbolId,
    ) -> Result<bool, ResolutionError> {
        self.transact(store, |resolver, store| {
            if resolver.session.packages.scope_of(package).is_none() {
                return Ok(None);
            }
            resolver
                .session
                .packages
                .enter_class(store, package, name, member);
            Ok(Some(true))
        })
        .map(|entered| entered.unwrap_or(false))
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

/// Nested JVM classes are also indexed in their binary package scope (for
/// example, `Outer$Inner` under `example`), while their semantic owner is the
/// enclosing class. Accept that shape only when the enclosing class itself is
/// owned by the requested package and its binary name prefixes the member.
fn is_nested_class_indexed_in_package(
    store: &SemanticStore,
    member: SymbolId,
    package: SymbolId,
) -> bool {
    const MAX_OWNER_DEPTH: usize = 64;

    let mut current = member;
    let mut visited = std::collections::HashSet::new();
    for depth in 0..MAX_OWNER_DEPTH {
        if !store.symbols.contains(current) || !visited.insert(current) {
            return false;
        }
        let symbol = store.symbols.get(current);
        let Some(owner) = symbol.owner else {
            return false;
        };
        if !store.symbols.contains(owner) {
            return false;
        }
        let enclosing = store.symbols.get(owner);
        if enclosing.kind == SymbolKind::Package {
            return depth > 0 && owner == package;
        }
        if !matches!(
            enclosing.kind,
            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
        ) {
            return false;
        }

        let enclosing_name = store.names.resolve(enclosing.name.text());
        let member_name = store.names.resolve(symbol.name.text());
        if !member_name
            .strip_prefix(enclosing_name)
            .is_some_and(|suffix| suffix.starts_with('$') && suffix.len() > 1)
        {
            return false;
        }
        current = owner;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::class_path::{
        ClassFormat, ClassOrigin, ClassPathEntry, ClassPathError, ClassResource,
        CompositeClassPath, DirectoryClassPath,
    };
    use crate::jdk_class_path::JdkClassPath;
    use crate::jmod_class_path::JmodClassPath;
    use dotty_core::{ClassInfo, MemberSelector, Name, Namespace, Scope, SourceId};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

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

    #[derive(Clone)]
    struct CompletionClassPath {
        classes: Arc<Mutex<std::collections::HashMap<BinaryName, Vec<u8>>>>,
        calls: Arc<AtomicUsize>,
    }

    fn member_class_path() -> CompletionClassPath {
        use dotty_classfile::{
            access_flags::{ClassAccessFlags, FieldAccessFlags, MethodAccessFlags},
            attribute::Attribute,
            class_file::{ClassFile, ClassFileVersion},
            constant_pool_builder::ConstantPoolBuilder,
            field::FieldInfo,
            method::MethodInfo,
        };
        let mut pool = ConstantPoolBuilder::new();
        pool.utf8("Signature").unwrap();
        let this_class = pool.class("sample/B").unwrap();
        let parent = pool.class("java/lang/Object").unwrap();
        let signature = pool
            .utf8("<T:Ljava/lang/Object;>Ljava/lang/Object;")
            .unwrap();
        let fields = vec![FieldInfo {
            access_flags: FieldAccessFlags(1),
            name_index: pool.utf8("value").unwrap(),
            descriptor_index: pool.utf8("I").unwrap(),
            attributes: Vec::new(),
        }];
        let methods = [
            ("get", "()Ljava/lang/String;", None),
            ("generic", "()Ljava/lang/Object;", Some("()TT;")),
            ("over", "()I", None),
            ("over", "(I)I", None),
        ]
        .into_iter()
        .map(|(name, descriptor, signature)| MethodInfo {
            access_flags: MethodAccessFlags(0x0401),
            name_index: pool.utf8(name).unwrap(),
            descriptor_index: pool.utf8(descriptor).unwrap(),
            attributes: signature
                .into_iter()
                .map(|sig| Attribute::Signature(pool.utf8(sig).unwrap()))
                .collect(),
        })
        .collect();
        let bytes = ClassFile {
            version: ClassFileVersion {
                major: 65,
                minor: 0,
            },
            constant_pool: pool.finish().unwrap(),
            access_flags: ClassAccessFlags(0x0421),
            this_class,
            super_class: Some(parent),
            interfaces: Vec::new(),
            fields,
            methods,
            attributes: vec![Attribute::Signature(signature)],
        }
        .encode()
        .unwrap();
        let path = CompletionClassPath::new();
        path.classes.lock().unwrap().extend([
            (BinaryName::from_internal("sample/B"), bytes),
            (
                BinaryName::from_internal("java/lang/Object"),
                test_class_stub("java/lang/Object", 0x0021),
            ),
            (
                BinaryName::from_internal("java/lang/String"),
                test_class_stub("java/lang/String", 0x0021),
            ),
        ]);
        path
    }

    #[test]
    fn direct_shell_members_support_all_prefix_views_and_reuse_declarations() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let path = member_class_path();
        let mut loader = ClassLoader::with_definitions(
            path.clone(),
            &mut store,
            definitions,
            LoadingSession::new(),
        );
        let b = loader
            .ensure_class_identity(&BinaryName::from_internal("sample/B"))
            .unwrap();
        let a = loader
            .ensure_class_identity(&BinaryName::from_internal("sample/A"))
            .unwrap();
        let session = loader.into_session();
        let b_ref = store.types.alloc(Type::type_ref(definitions.no_prefix, b));
        let a_ref = store.types.alloc(Type::type_ref(definitions.no_prefix, a));
        let applied = store.types.alloc(Type::Applied {
            tycon: b_ref,
            args: vec![definitions.int],
        });
        let this = store.types.alloc(Type::ThisType { class: b });
        let value = Name::new(store.names.intern("value"), Namespace::Term);
        let get = Name::new(store.names.intern("get"), Namespace::Term);
        let mut resolver = ClasspathSymbolResolver::new(path.clone(), definitions, session);
        let mut field = None;
        let mut method = None;
        for (prefix, space) in [
            (b_ref, MemberSpace::Prefix),
            (applied, MemberSpace::Prefix),
            (this, MemberSpace::Prefix),
            (a_ref, MemberSpace::Explicit(b_ref)),
        ] {
            for _ in 0..2 {
                let f = resolver
                    .resolve_member(
                        &mut store,
                        &MemberRequest {
                            prefix,
                            name: value,
                            selector: MemberSelector::Unique,
                            space: space.clone(),
                        },
                    )
                    .unwrap()
                    .unwrap();
                let m = resolver
                    .resolve_member(
                        &mut store,
                        &MemberRequest {
                            prefix,
                            name: get,
                            selector: MemberSelector::Unique,
                            space: space.clone(),
                        },
                    )
                    .unwrap()
                    .unwrap();
                assert_eq!(*field.get_or_insert(f), f);
                assert_eq!(*method.get_or_insert(m), m);
                assert_eq!(store.symbols.get(f).owner, Some(b));
                assert_eq!(store.symbols.get(f).kind, SymbolKind::Field);
                assert_eq!(
                    store.symbols.get(f).info,
                    SymbolInfo::Complete(definitions.int)
                );
                assert_eq!(store.symbols.get(m).owner, Some(b));
                assert_eq!(store.symbols.get(m).kind, SymbolKind::Method);
                assert_eq!(path.calls.load(Ordering::Relaxed), 5);
                assert_eq!(store.symbols.get(a).info, SymbolInfo::Missing);
            }
        }
        let SymbolInfo::Complete(info) = store.symbols.get(method.unwrap()).info else {
            panic!("method info")
        };
        let Type::Method(info) = store.types.get(info) else {
            panic!("Method")
        };
        let string = store.types.get(info.result).reference_symbol().unwrap();
        assert_eq!(
            store.names.resolve(store.symbols.get(string).name.text()),
            "String"
        );
        assert_eq!(store.symbols.get(string).info, SymbolInfo::Missing);
        let t_name = Name::new(store.names.intern("T"), Namespace::Type);
        let t = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: b_ref,
                    name: t_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(store.symbols.get(t).kind, SymbolKind::TypeParameter);
        let generic_name = Name::new(store.names.intern("generic"), Namespace::Term);
        let generic = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: applied,
                    name: generic_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let SymbolInfo::Complete(info) = store.symbols.get(generic).info else {
            panic!("generic info")
        };
        let Type::Method(info) = store.types.get(info) else {
            panic!("Method")
        };
        assert_eq!(
            store.types.get(info.result).reference_symbol(),
            Some(t),
            "resolver does not substitute applied receiver arguments"
        );
        let mark = store.checkpoint();
        assert_eq!(
            resolver
                .resolve_member(
                    &mut store,
                    &MemberRequest {
                        prefix: b_ref,
                        name: value,
                        selector: MemberSelector::Unique,
                        space: MemberSpace::Explicit(a_ref)
                    }
                )
                .unwrap(),
            None
        );
        assert_eq!(store.symbols.get(a).info, SymbolInfo::Missing);
        assert_eq!(store.checkpoint(), mark);
    }

    #[test]
    fn missing_ambiguous_and_failed_members_are_atomic_and_retryable() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let path = member_class_path();
        let mut loader = ClassLoader::with_definitions(
            path.clone(),
            &mut store,
            definitions,
            LoadingSession::new(),
        );
        let b = loader
            .ensure_class_identity(&BinaryName::from_internal("sample/B"))
            .unwrap();
        let session = loader.into_session();
        let original = store.symbols.get(b).clone();
        let prefix = store.types.alloc(Type::type_ref(definitions.no_prefix, b));
        let mut resolver = ClasspathSymbolResolver::new(path.clone(), definitions, session);
        for name in ["absent", "over"] {
            let request = MemberRequest {
                prefix,
                name: Name::new(store.names.intern(name), Namespace::Term),
                selector: MemberSelector::Unique,
                space: MemberSpace::Prefix,
            };
            let mark = store.checkpoint();
            let resolver_mark = resolver.checkpoint();
            let session_mark = resolver.session.checkpoint();
            for _ in 0..2 {
                let result = resolver.resolve_member(&mut store, &request);
                if name == "absent" {
                    assert_eq!(result, Ok(None));
                } else {
                    assert_eq!(result, Err(ResolutionError::Ambiguous { candidates: 2 }));
                }
                assert_eq!(store.symbols.get(b), &original);
                assert_eq!(store.checkpoint(), mark);
                assert_eq!(resolver.checkpoint(), resolver_mark);
                assert_eq!(resolver.session.checkpoint(), session_mark);
            }
        }
        let request = MemberRequest {
            prefix,
            name: Name::new(store.names.intern("value"), Namespace::Term),
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };
        let bytes = path
            .classes
            .lock()
            .unwrap()
            .insert(BinaryName::from_internal("sample/B"), vec![0, 1, 2])
            .unwrap();
        let mark = store.checkpoint();
        assert!(
            matches!(resolver.resolve_member(&mut store, &request), Err(ResolutionError::Malformed { reason }) if reason.contains("invalid class file"))
        );
        assert_eq!(store.checkpoint(), mark);
        assert_eq!(store.symbols.get(b), &original);
        path.classes
            .lock()
            .unwrap()
            .insert(BinaryName::from_internal("sample/B"), bytes);
        assert!(
            resolver
                .resolve_member(&mut store, &request)
                .unwrap()
                .is_some()
        );
    }

    impl CompletionClassPath {
        fn new() -> Self {
            Self {
                classes: Arc::new(Mutex::new(std::collections::HashMap::from([
                    (
                        BinaryName::from_internal("sample/B"),
                        test_class_stub("sample/B", 0x0021),
                    ),
                    (
                        BinaryName::from_internal("sample/A"),
                        test_class_with_object_fields("sample/A", None, &[("b", "sample/B")]),
                    ),
                ]))),
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    impl ClassPathEntry for CompletionClassPath {
        fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(self.classes.lock().unwrap().get(name).map(|bytes| {
                ClassResource::new(
                    bytes.clone(),
                    crate::class_path::ClassFormat::Class,
                    crate::class_path::ClassOrigin::Directory("<completion>".into()),
                )
            }))
        }

        fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            let prefix = format!("{}/", package.join("/"));
            Ok(self
                .classes
                .lock()
                .unwrap()
                .keys()
                .any(|name| name.as_internal().starts_with(&prefix)))
        }
    }

    #[test]
    fn complete_class_outer_rollback_removes_shells_created_in_the_transaction() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver = ClasspathSymbolResolver::new(
            CompletionClassPath::new(),
            definitions,
            LoadingSession::new(),
        );
        let package = resolver
            .resolve_package(&mut store, &["sample"])
            .unwrap()
            .unwrap();
        let prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let request = MemberRequest {
            prefix,
            name: Name::new(store.names.intern("A"), Namespace::Type),
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };
        let mark = resolver.checkpoint();
        let store_mark = store.checkpoint();
        let a = resolver
            .resolve_member(&mut store, &request)
            .unwrap()
            .unwrap();
        let b_name = BinaryName::from_internal("sample/B");
        let b = resolver.session.identified[&b_name];
        assert_eq!(store.symbols.get(b).info, SymbolInfo::Missing);
        assert_eq!(resolver.complete_class(&mut store, b), Ok(true));
        resolver.rollback_to(&mut store, mark);
        store.rollback_to(store_mark);
        assert!(!store.symbols.contains(a));
        assert!(!store.symbols.contains(b));
        assert!(resolver.session.identified.is_empty());
        assert!(resolver.session.resolved.is_empty());
        assert_eq!(
            resolver.resolve_member(&mut store, &request).unwrap(),
            Some(a)
        );
        let again = resolver.session.identified[&b_name];
        assert_eq!(resolver.complete_class(&mut store, again), Ok(true));
    }

    #[test]
    fn complete_class_handoff_is_idempotent_and_outer_rollback_restores_shell() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let path = CompletionClassPath::new();
        let c_name = BinaryName::from_internal("sample/C");
        path.classes.lock().unwrap().extend([
            (
                BinaryName::from_internal("sample/B"),
                test_class_with_object_fields("sample/B", None, &[("c", "sample/C")]),
            ),
            (c_name.clone(), test_class_stub("sample/C", 0x0021)),
        ]);
        let mut loader = ClassLoader::with_definitions(
            path.clone(),
            &mut store,
            definitions,
            LoadingSession::new(),
        );
        let b_name = BinaryName::from_internal("sample/B");
        let b = loader.ensure_class_identity(&b_name).unwrap();
        let session = loader.into_session();
        let previous = store.symbols.get(b).clone();
        let mut resolver = ClasspathSymbolResolver::new(path.clone(), definitions, session);
        let mark = resolver.checkpoint();
        let store_mark = store.checkpoint();
        assert_eq!(resolver.complete_class(&mut store, b), Ok(true));
        assert_eq!(resolver.session.resolved.get(&b_name), Some(&b));
        let c = resolver.session.identified[&c_name];
        assert_eq!(store.symbols.get(c).info, SymbolInfo::Missing);
        let SymbolInfo::Complete(completed_info) = store.symbols.get(b).info else {
            panic!("completed B")
        };
        let completed = store.checkpoint();
        assert_eq!(resolver.complete_class(&mut store, b), Ok(true));
        assert_eq!(store.checkpoint(), completed);
        assert_eq!(path.calls.load(Ordering::Relaxed), 3);
        let package = previous.owner.unwrap();
        let scope = resolver.package_scope(package).unwrap();
        assert_eq!(store.scopes.get(scope).lookup_all(&previous.name), &[b]);
        resolver.rollback_to(&mut store, mark);
        store.rollback_to(store_mark);
        assert_eq!(store.symbols.get(b), &previous);
        assert_eq!(resolver.session.identified.get(&b_name), Some(&b));
        assert!(!resolver.session.resolved.contains_key(&b_name));
        assert!(!resolver.session.metadata.contains_key(&b));
        assert_eq!(store.checkpoint(), store_mark);
        assert!(!store.symbols.contains(c));
        assert!(!store.types.contains(completed_info));
        assert!(!resolver.session.identified.contains_key(&c_name));
        assert!(!resolver.session.origins.contains_key(&c));
        let c_text = store.names.intern("C");
        assert!(
            store
                .scopes
                .get(scope)
                .lookup_all(&Name::new(c_text, Namespace::Type))
                .is_empty()
        );
        assert_eq!(resolver.complete_class(&mut store, b), Ok(true));
        assert_eq!(path.calls.load(Ordering::Relaxed), 5);
    }

    #[test]
    fn complete_class_refuses_unowned_symbols_and_failed_completion_is_atomic() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let path = CompletionClassPath::new();
        let name = BinaryName::from_internal("sample/B");
        let mut loader = ClassLoader::with_definitions(
            path.clone(),
            &mut store,
            definitions,
            LoadingSession::new(),
        );
        let b = loader.ensure_class_identity(&name).unwrap();
        assert_eq!(
            loader
                .ensure_class_identity(&BinaryName::from_internal("java/lang/Object"))
                .unwrap(),
            definitions.object_class
        );
        let session = loader.into_session();
        let previous = store.symbols.get(b).clone();
        let foreign = store.symbols.alloc(previous.clone());
        let mut resolver = ClasspathSymbolResolver::new(path.clone(), definitions, session);
        let mark = store.checkpoint();
        let session_mark = resolver.session.checkpoint();
        for unowned in [foreign, definitions.object_class] {
            assert_eq!(resolver.complete_class(&mut store, unowned), Ok(false));
            assert_eq!(store.checkpoint(), mark);
            assert_eq!(resolver.session.checkpoint(), session_mark);
        }
        path.classes
            .lock()
            .unwrap()
            .insert(name.clone(), vec![0, 1, 2]);
        assert!(
            matches!(resolver.complete_class(&mut store, b), Err(ResolutionError::Malformed { reason })
            if reason.contains("external class completion failed") && reason.contains("invalid class file"))
        );
        assert_eq!(store.symbols.get(b), &previous);
        assert_eq!(store.checkpoint(), mark);
        assert_eq!(resolver.session.checkpoint(), session_mark);
        path.classes
            .lock()
            .unwrap()
            .insert(name, test_class_stub("sample/B", 0x0021));
        assert_eq!(resolver.complete_class(&mut store, b), Ok(true));
    }

    #[test]
    fn complete_class_rejects_corrupt_owned_state_without_loading_or_mutation() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let path = CompletionClassPath::new();
        let mut loader = ClassLoader::with_definitions(
            path.clone(),
            &mut store,
            definitions,
            LoadingSession::new(),
        );
        let b = loader
            .ensure_class_identity(&BinaryName::from_internal("sample/B"))
            .unwrap();
        let session = loader.into_session();
        let original = store.symbols.get(b).clone();
        let mut resolver = ClasspathSymbolResolver::new(path.clone(), definitions, session);
        for (info, expected) in [
            (SymbolInfo::Error, "Error info"),
            (
                SymbolInfo::Complete(definitions.no_prefix),
                "not a valid ClassInfo",
            ),
        ] {
            store.symbols.get_mut(b).info = info;
            let checkpoint = store.checkpoint();
            let session_checkpoint = resolver.session.checkpoint();
            assert!(
                matches!(resolver.complete_class(&mut store, b), Err(ResolutionError::Malformed { reason }) if reason.contains(expected))
            );
            assert_eq!(store.symbols.get(b).info, info);
            assert_eq!(store.checkpoint(), checkpoint);
            assert_eq!(resolver.session.checkpoint(), session_checkpoint);
        }
        *store.symbols.get_mut(b) = original.clone();
        let wrong_name = Name::new(store.names.intern("Wrong"), Namespace::Type);
        store.symbols.get_mut(b).name = wrong_name;
        assert!(
            matches!(resolver.complete_class(&mut store, b), Err(ResolutionError::Malformed { reason }) if reason.contains("canonical package/name"))
        );
        assert_eq!(store.symbols.get(b).name, wrong_name);
        *store.symbols.get_mut(b) = original;
        assert_eq!(path.calls.load(Ordering::Relaxed), 1);
        path.classes
            .lock()
            .unwrap()
            .remove(&BinaryName::from_internal("sample/B"));
        assert!(
            matches!(resolver.complete_class(&mut store, b), Err(ResolutionError::Malformed { reason }) if reason.contains("class not found: sample/B"))
        );
        assert_eq!(store.symbols.get(b).info, SymbolInfo::Missing);
        assert!(
            !resolver
                .session
                .resolved
                .contains_key(&BinaryName::from_internal("sample/B"))
        );
    }

    #[test]
    fn complete_class_rejects_invalid_owner_and_package_scope_without_panicking() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let path = CompletionClassPath::new();
        let mut loader = ClassLoader::with_definitions(
            path.clone(),
            &mut store,
            definitions,
            LoadingSession::new(),
        );
        let b = loader
            .ensure_class_identity(&BinaryName::from_internal("sample/B"))
            .unwrap();
        let session = loader.into_session();
        let original = store.symbols.get(b).clone();
        let mut resolver = ClasspathSymbolResolver::new(path.clone(), definitions, session);
        let checkpoint = store.checkpoint();
        let invalid_owner = store.symbols.alloc(original.clone());
        store.rollback_to(checkpoint);
        store.symbols.get_mut(b).owner = Some(invalid_owner);
        assert!(
            matches!(resolver.complete_class(&mut store, b), Err(ResolutionError::Malformed { reason }) if reason.contains("canonical package/name"))
        );
        assert_eq!(store.symbols.get(b).owner, Some(invalid_owner));
        *store.symbols.get_mut(b) = original.clone();
        let scope = resolver.package_scope(original.owner.unwrap()).unwrap();
        store.scopes.get_mut(scope).owner = None;
        assert!(
            matches!(resolver.complete_class(&mut store, b), Err(ResolutionError::Malformed { reason }) if reason.contains("canonical package/name"))
        );
        assert_eq!(store.scopes.get(scope).owner, None);
        assert_eq!(store.checkpoint(), checkpoint);
        assert_eq!(path.calls.load(Ordering::Relaxed), 1);
    }

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

    struct SyntheticTastyJdk;

    impl ClassPathEntry for SyntheticTastyJdk {
        fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
            let (bytes, flags) = match name.as_internal() {
                "java/lang/Object" => (test_class_stub("java/lang/Object", 0x0021), 0x0021),
                "java/io/Closeable" => (test_class_stub("java/io/Closeable", 0x0601), 0x0601),
                _ => return Ok(None),
            };
            Ok(Some(ClassResource::new(
                bytes,
                ClassFormat::Class,
                ClassOrigin::Directory(PathBuf::from(format!("<synthetic-{flags:04x}>"))),
            )))
        }

        fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            Ok(package == ["java", "lang"] || package == ["java", "io"])
        }
    }

    struct JdkCorpusDayOfWeek;

    impl ClassPathEntry for JdkCorpusDayOfWeek {
        fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
            let (bytes, origin) = match name.as_internal() {
                "java/time/DayOfWeek" => (
                    include_bytes!("../../dotty-classfile/tests/fixtures/jdk_corpus/jdk26/java/time/DayOfWeek.class").to_vec(),
                    fixture_path("../dotty-classfile/tests/fixtures/jdk_corpus/jdk26"),
                ),
                "java/lang/Object" => (
                    test_class_stub("java/lang/Object", 0x0021),
                    PathBuf::from("<synthetic-JDK-dependency>"),
                ),
                _ => (
                    test_class_stub_with_super(name.as_internal(), "java/lang/Object"),
                    PathBuf::from("<synthetic-JDK-dependency>"),
                ),
            };
            Ok(Some(ClassResource::new(
                bytes,
                ClassFormat::Class,
                ClassOrigin::Directory(origin),
            )))
        }

        fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            Ok(package == ["java", "time"])
        }
    }

    struct DelayedCloseableClassPath {
        inner: CompositeClassPath,
        allow_closeable: Arc<AtomicBool>,
    }

    impl ClassPathEntry for DelayedCloseableClassPath {
        fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
            if name.as_internal() == "java/io/Closeable"
                && !self.allow_closeable.load(Ordering::Relaxed)
            {
                return Ok(None);
            }
            self.inner.find_class(name)
        }

        fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            self.inner.contains_package(package)
        }
    }

    struct EnclosingGenericRetryClassPath(Mutex<bool>);

    impl ClassPathEntry for EnclosingGenericRetryClassPath {
        fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
            let bytes = match name.as_internal() {
                "enclosing_generics/Outer" => {
                    include_bytes!("../tests/fixtures/enclosing_generics/Outer.class").to_vec()
                }
                "enclosing_generics/Outer$Inner" => {
                    include_bytes!("../tests/fixtures/enclosing_generics/Outer$Inner.class")
                        .to_vec()
                }
                "enclosing_generics/Outer$Middle" => {
                    include_bytes!("../tests/fixtures/enclosing_generics/Outer$Middle.class")
                        .to_vec()
                }
                "enclosing_generics/Outer$Middle$Deep" => {
                    include_bytes!("../tests/fixtures/enclosing_generics/Outer$Middle$Deep.class")
                        .to_vec()
                }
                "enclosing_generics/Outer$Middle$Deep$Leaf" => include_bytes!(
                    "../tests/fixtures/enclosing_generics/Outer$Middle$Deep$Leaf.class"
                )
                .to_vec(),
                "enclosing_generics/Outer$Static" => {
                    let mut bytes =
                        include_bytes!("../tests/fixtures/enclosing_generics/Outer$Static.class")
                            .to_vec();
                    if *self.0.lock().expect("fixture state lock") {
                        let signature = bytes
                            .windows(3)
                            .position(|window| window == b"TU;")
                            .expect("Static.value should have a TU; Signature");
                        bytes[signature..signature + 3].copy_from_slice(b"TK;");
                    }
                    bytes
                }
                "java/lang/Object" => test_class_stub("java/lang/Object", 0x0021),
                _ => return Ok(None),
            };
            Ok(Some(ClassResource::new(
                bytes,
                ClassFormat::Class,
                ClassOrigin::Directory(PathBuf::from("<enclosing-generics-fixture>")),
            )))
        }

        fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            Ok(package == ["enclosing_generics"] || package == ["java", "lang"])
        }
    }

    #[test]
    fn failed_nested_class_load_rolls_back_and_retries_with_stable_symbols() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let class_path = EnclosingGenericRetryClassPath(Mutex::new(true));
        let mut resolver =
            ClasspathSymbolResolver::new(class_path, definitions, LoadingSession::new());
        let package = resolver
            .resolve_package(&mut store, &["enclosing_generics"])
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let package_scope = resolver.session.packages.scope_of(package).unwrap();
        let store_checkpoint = store.checkpoint();
        let static_name = Name::new(store.names.intern("Outer$Static"), Namespace::Type);
        let request = MemberRequest {
            prefix: package_prefix,
            name: static_name,
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };

        let error = resolver
            .resolve_member(&mut store, &request)
            .expect_err("malformed static signature should fail after loading Outer and Inner");
        assert!(matches!(
            error,
            ResolutionError::Malformed { reason }
                if reason.contains("unresolved type variable K")
        ));
        assert_eq!(store.checkpoint(), store_checkpoint);
        for class_name in ["Outer", "Outer$Inner", "Outer$Static"] {
            assert!(
                store
                    .scopes
                    .get(package_scope)
                    .lookup_all(&Name::new(store.names.intern(class_name), Namespace::Type,))
                    .is_empty(),
                "failed transaction left {class_name} in the package scope"
            );
            assert!(
                !resolver
                    .session
                    .resolved
                    .contains_key(&BinaryName::from_internal(format!(
                        "enclosing_generics/{class_name}"
                    ),)),
                "failed transaction left {class_name} in the positive cache"
            );
        }

        *resolver.class_path.as_ref().unwrap().0.lock().unwrap() = false;
        let static_class = resolver
            .resolve_member(&mut store, &request)
            .expect("retry should succeed after fixing the classfile bytes")
            .expect("Outer$Static should resolve");
        let again = resolver
            .resolve_member(&mut store, &request)
            .unwrap()
            .unwrap();
        assert_eq!(static_class, again);
        let outer_name = BinaryName::from_internal("enclosing_generics/Outer");
        let outer = resolver.session.identified[&outer_name];
        assert_eq!(store.symbols.get(outer).info, SymbolInfo::Missing);
        assert!(!resolver.session.resolved.contains_key(&outer_name));
    }

    #[test]
    fn repeated_package_lookup_preserves_deeply_nested_class_identity() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver = ClasspathSymbolResolver::new(
            EnclosingGenericRetryClassPath(Mutex::new(false)),
            definitions,
            LoadingSession::new(),
        )
        .with_load_profile_collection();
        let package = resolver
            .resolve_package(&mut store, &["enclosing_generics"])
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let request = MemberRequest {
            prefix: package_prefix,
            name: Name::new(
                store.names.intern("Outer$Middle$Deep$Leaf"),
                Namespace::Type,
            ),
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };

        let first = resolver
            .resolve_member(&mut store, &request)
            .expect("the first request should load the nested class and its outers")
            .expect("the fixture contains the nested class");
        let load_profiles = resolver.take_load_profiles();
        assert_eq!(load_profiles.len(), 1);
        assert_eq!(
            load_profiles[0].root,
            "enclosing_generics/Outer$Middle$Deep$Leaf"
        );
        assert!(load_profiles[0].max_dependency_depth > 0);
        for class in [
            "enclosing_generics/Outer",
            "enclosing_generics/Outer$Middle",
            "enclosing_generics/Outer$Middle$Deep",
            "enclosing_generics/Outer$Middle$Deep$Leaf",
        ] {
            assert!(
                load_profiles[0].classes.contains(class),
                "dependency profile should include {class}"
            );
        }
        let second = resolver
            .resolve_member(&mut store, &request)
            .expect("the repeated request should pass package owner validation")
            .expect("the class should remain in the package's binary-name index");

        assert_eq!(first, second);
        assert!(resolver.take_load_profiles().is_empty());
        let package_scope = resolver.session.packages.scope_of(package).unwrap();
        assert_eq!(
            store.scopes.get(package_scope).lookup_all(&request.name),
            &[first]
        );
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
    fn package_type_lookup_does_not_return_an_incomplete_source_class() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let probes = Arc::new(AtomicUsize::new(0));
        let mut resolver = ClasspathSymbolResolver::new(
            EmptyCountingClassPath(Arc::clone(&probes)),
            definitions,
            LoadingSession::new(),
        );
        let package = resolver
            .session
            .packages
            .resolve_package(&mut store, "example");
        let package_scope = resolver.package_scope(package).unwrap();
        let class_name = Name::new(store.names.intern("Pending"), Namespace::Type);
        let source_class = store.symbols.alloc(Symbol {
            name: class_name,
            owner: Some(package),
            kind: SymbolKind::Class,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Missing,
            origin: dotty_core::SymbolOrigin::Source(SourceId::from_index(0)),
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        });
        store
            .scopes
            .get_mut(package_scope)
            .enter(class_name, source_class);
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));

        let result = resolver.resolve_member(
            &mut store,
            &MemberRequest {
                prefix: package_prefix,
                name: class_name,
                selector: MemberSelector::Unique,
                space: MemberSpace::Prefix,
            },
        );

        assert_eq!(result, Ok(None));
        assert_eq!(probes.load(Ordering::Relaxed), 0);
        assert_eq!(
            store.scopes.get(package_scope).lookup_all(&class_name),
            &[source_class],
            "the incomplete source identity remains in place without a classpath twin"
        );
        assert!(resolver.session.resolved.is_empty());
    }

    #[test]
    fn package_type_lookup_rejects_a_class_with_a_foreign_declaration_scope() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver = ClasspathSymbolResolver::new(
            EmptyCountingClassPath(Arc::new(AtomicUsize::new(0))),
            definitions,
            LoadingSession::new(),
        );
        let package = resolver
            .session
            .packages
            .resolve_package(&mut store, "example");
        let package_scope = resolver.package_scope(package).unwrap();
        let class_name = Name::new(store.names.intern("Malformed"), Namespace::Type);
        let foreign_scope = store.scopes.alloc(Scope::new(Some(package)));
        let class = store.symbols.alloc(Symbol {
            name: class_name,
            owner: Some(package),
            kind: SymbolKind::Class,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Missing,
            origin: dotty_core::SymbolOrigin::Source(SourceId::from_index(0)),
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        });
        let class_info = store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: definitions.no_prefix,
            class,
            parents: Vec::new(),
            declarations: foreign_scope,
            self_type: None,
        }));
        store.symbols.get_mut(class).info = SymbolInfo::Complete(class_info);
        store.scopes.get_mut(package_scope).enter(class_name, class);
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));

        let error = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: class_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .expect_err("a class cannot publish another owner's declaration scope");

        assert!(matches!(
            error,
            ResolutionError::Malformed { reason }
                if reason.contains("invalid class identity or declaration scope")
        ));
    }

    #[test]
    fn package_type_lookup_rejects_an_unfilled_completion_slot_without_mutation() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver = ClasspathSymbolResolver::new(
            EmptyCountingClassPath(Arc::new(AtomicUsize::new(0))),
            definitions,
            LoadingSession::new(),
        );
        let package = resolver
            .session
            .packages
            .resolve_package(&mut store, "example");
        let package_scope = resolver.package_scope(package).unwrap();
        let class_name = Name::new(store.names.intern("Reserved"), Namespace::Type);
        let reserved_type = store.types.reserve().id();
        let class = store.symbols.alloc(Symbol {
            name: class_name,
            owner: Some(package),
            kind: SymbolKind::Class,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Complete(reserved_type),
            origin: dotty_core::SymbolOrigin::Tasty(store.origins.register_tasty()),
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        });
        store.scopes.get_mut(package_scope).enter(class_name, class);
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let checkpoint = store.checkpoint();

        let error = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: class_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .expect_err("an unfilled type reservation is malformed class metadata");

        assert!(matches!(
            error,
            ResolutionError::Malformed { reason }
                if reason.contains("invalid or unfilled completion type")
        ));
        assert_eq!(store.checkpoint(), checkpoint);
        assert!(resolver.session.resolved.is_empty());
    }

    #[test]
    fn package_type_lookup_rejects_an_external_class_owned_by_another_package() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver = ClasspathSymbolResolver::new(
            EmptyCountingClassPath(Arc::new(AtomicUsize::new(0))),
            definitions,
            LoadingSession::new(),
        );
        let package = resolver
            .session
            .packages
            .resolve_package(&mut store, "example");
        let package_scope = resolver.package_scope(package).unwrap();
        let foreign_package = resolver
            .session
            .packages
            .resolve_package(&mut store, "foreign");
        let class_name = Name::new(store.names.intern("WrongOwner"), Namespace::Type);
        let class = store.symbols.alloc(Symbol {
            name: class_name,
            owner: Some(foreign_package),
            kind: SymbolKind::Class,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Missing,
            origin: dotty_core::SymbolOrigin::Tasty(store.origins.register_tasty()),
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        });
        let class_scope = store.scopes.alloc(Scope::new(Some(class)));
        let class_info = store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: definitions.no_prefix,
            class,
            parents: Vec::new(),
            declarations: class_scope,
            self_type: None,
        }));
        store.symbols.get_mut(class).info = SymbolInfo::Complete(class_info);
        store.scopes.get_mut(package_scope).enter(class_name, class);
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let checkpoint = store.checkpoint();

        let error = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: class_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .expect_err("an external package entry cannot return a class from another package");

        assert!(matches!(
            error,
            ResolutionError::Malformed { reason }
                if reason.contains("canonical package/name")
        ));
        assert_eq!(store.checkpoint(), checkpoint);
        assert!(resolver.session.resolved.is_empty());
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

        let java_lang = resolver
            .resolve_package(&mut store, &["java", "lang"])
            .expect("java.lang package lookup should succeed")
            .expect("the JDK classpath contains java.lang");
        let java_lang_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, java_lang));
        let object_name = Name::new(store.names.intern("Object"), Namespace::Type);
        let object = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: java_lang_prefix,
                    name: object_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .expect("package type lookup should load java.lang.Object")
            .expect("java.lang.Object should materialize from the real JDK");

        assert_eq!(
            object, object_class,
            "JDK Object reuses Definitions identity"
        );
        assert!(matches!(
            store.symbols.get(object).info,
            SymbolInfo::Complete(info) if matches!(store.types.get(info), Type::ClassInfo(_))
        ));
        let object_scope = resolver
            .declaration_scope(&store, object)
            .expect("the real JDK Object has a declaration scope");
        assert!(store.scopes.contains(object_scope));
        assert_eq!(
            store
                .scopes
                .get(resolver.package_scope(java_lang).unwrap())
                .lookup(&object_name),
            Some(object),
            "package and loader must expose the same canonical Object symbol"
        );
        assert_eq!(
            resolver
                .resolve_member(
                    &mut store,
                    &MemberRequest {
                        prefix: java_lang_prefix,
                        name: object_name,
                        selector: MemberSelector::Unique,
                        space: MemberSpace::Prefix,
                    },
                )
                .unwrap(),
            Some(object),
            "repeated package type requests reuse the canonical identity"
        );

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
        let class_name = BinaryName::from_internal("java/lang/Class");
        let class_symbol = *resolver
            .session
            .identified
            .get(&class_name)
            .expect("getClass's result should establish java/lang/Class identity");
        assert_ne!(class_symbol, object_class);
        assert_eq!(store.symbols.get(class_symbol).info, SymbolInfo::Missing);
        assert!(!resolver.session.resolved.contains_key(&class_name));
        let SymbolInfo::Complete(method_type) = store.symbols.get(method).info else {
            panic!("getClass should have complete method info");
        };
        let Type::Method(method_info) = store.types.get(method_type) else {
            panic!("getClass should have a Method type");
        };
        let Type::Applied { tycon, args } = store.types.get(method_info.result) else {
            panic!("getClass's generic result should be Class[?]");
        };
        assert_eq!(
            store.types.get(*tycon).reference_symbol(),
            Some(class_symbol)
        );
        assert_eq!(args.len(), 1);
        assert!(matches!(store.types.get(args[0]), Type::Wildcard { .. }));

        // Inspecting Class's parents requires explicit completion on this session.
        let class_path = resolver.class_path.take().unwrap();
        let session = resolver.into_session();
        let mut loader =
            ClassLoader::with_definitions(class_path, &mut store, definitions, session);
        assert_eq!(loader.load_class(&class_name).unwrap(), class_symbol);
        drop(loader);
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
    fn tasty_parent_failure_rolls_back_loaded_dependencies_and_retry_succeeds() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let allow_closeable = Arc::new(AtomicBool::new(false));
        let inner = CompositeClassPath::new(vec![
            Box::new(DirectoryClassPath::new(fixture_path(
                "../dotty-tasty/tests/fixtures/scala3-library",
            ))),
            Box::new(SyntheticTastyJdk),
        ]);
        let class_path = DelayedCloseableClassPath {
            inner,
            allow_closeable: Arc::clone(&allow_closeable),
        };
        let mut resolver =
            ClasspathSymbolResolver::new(class_path, definitions, LoadingSession::new());
        let scala_io = resolver
            .resolve_package(&mut store, &["scala", "io"])
            .unwrap()
            .expect("the Scala library fixture contains scala.io");
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, scala_io));
        let package_scope = resolver.session.packages.scope_of(scala_io).unwrap();
        let source_name = Name::new(store.names.intern("Source"), Namespace::Type);

        let error = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: source_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .expect_err("Source's later Closeable parent should fail its first load");
        assert!(matches!(
            error,
            ResolutionError::Malformed { reason } if reason.contains("java/io/Closeable")
        ));
        for name in ["scala/io/Source", "scala/collection/Iterator"] {
            assert!(
                !resolver
                    .session
                    .resolved
                    .contains_key(&BinaryName::from_internal(name)),
                "failed TASTy load left {name} in the positive cache"
            );
        }
        assert!(resolver.session.metadata.is_empty());
        assert!(
            store
                .scopes
                .get(package_scope)
                .lookup_all(&source_name)
                .is_empty()
        );
        if let Some(collection) = resolver.session.packages.get(&["scala", "collection"]) {
            let collection_scope = resolver.session.packages.scope_of(collection).unwrap();
            let iterator_name = Name::new(store.names.intern("Iterator"), Namespace::Type);
            assert!(
                store
                    .scopes
                    .get(collection_scope)
                    .lookup_all(&iterator_name)
                    .is_empty()
            );
        }

        allow_closeable.store(true, Ordering::Relaxed);
        let source = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: source_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .expect("retry should load Source after the later parent becomes available")
            .expect("Source should be materialized on retry");
        assert_eq!(
            store.names.resolve(store.symbols.get(source).name.text()),
            "Source"
        );
    }

    #[test]
    fn real_scala_library_package_type_materializes_class_and_method() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let class_path = CompositeClassPath::new(vec![
            Box::new(DirectoryClassPath::new(fixture_path(
                "../dotty-tasty/tests/fixtures/scala3-library",
            ))),
            Box::new(SyntheticTastyJdk),
        ]);
        let mut resolver =
            ClasspathSymbolResolver::new(class_path, definitions, LoadingSession::new());
        let package = resolver
            .resolve_package(&mut store, &["scala", "io"])
            .expect("Scala package lookup should succeed")
            .expect("the checked-in Scala library fixture has scala.io");
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let source_name = Name::new(store.names.intern("Source"), Namespace::Type);
        let request = MemberRequest {
            prefix: package_prefix,
            name: source_name,
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };

        let source = resolver
            .resolve_member(&mut store, &request)
            .expect("the real Scala 3.9 Source TASTy should load")
            .expect("scala.io.Source should be available as a package type");

        assert_eq!(store.symbols.get(source).owner, Some(package));
        assert_eq!(store.symbols.get(source).name, source_name);
        assert!(matches!(
            store.symbols.get(source).kind,
            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
        ));
        assert!(matches!(
            store.symbols.get(source).info,
            SymbolInfo::Complete(info) if matches!(store.types.get(info), Type::ClassInfo(_))
        ));
        let declarations = resolver
            .declaration_scope(&store, source)
            .expect("Source has a declaration scope");
        assert!(store.scopes.contains(declarations));
        let package_scope = resolver.package_scope(package).unwrap();
        assert_eq!(
            store.scopes.get(package_scope).lookup(&source_name),
            Some(source),
            "the package scope and resolver must share the canonical symbol"
        );
        assert_eq!(
            resolver.resolve_member(&mut store, &request).unwrap(),
            Some(source),
            "repeated package type requests must reuse the same identity"
        );

        let source_prefix = store.types.alloc(Type::ThisType { class: source });
        let get_lines = Name::new(store.names.intern("getLines"), Namespace::Term);
        let method = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: source_prefix,
                    name: get_lines,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .expect("Source declaration lookup should succeed")
            .expect("real scala.io.Source should expose getLines");
        assert_eq!(store.symbols.get(method).owner, Some(source));
        assert!(store.scopes.get(declarations).lookup(&get_lines).is_some());
    }

    #[test]
    fn real_jdk_package_type_materializes_from_checked_in_classfile() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver =
            ClasspathSymbolResolver::new(JdkCorpusDayOfWeek, definitions, LoadingSession::new());
        let package = resolver
            .resolve_package(&mut store, &["java", "time"])
            .expect("java.time package lookup should succeed")
            .expect("the checked-in JDK fixture contains java.time");
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let name = Name::new(store.names.intern("DayOfWeek"), Namespace::Type);
        let request = MemberRequest {
            prefix: package_prefix,
            name,
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };

        let class = resolver
            .resolve_member(&mut store, &request)
            .expect("the JDK 26 classfile should load")
            .expect("java.time.DayOfWeek should materialize from the JDK classfile");

        assert_eq!(store.symbols.get(class).owner, Some(package));
        assert_eq!(store.symbols.get(class).name, name);
        assert_eq!(store.symbols.get(class).kind, SymbolKind::Class);
        assert_eq!(
            resolver.session.origins.get(&class),
            Some(&ClassOrigin::Directory(fixture_path(
                "../dotty-classfile/tests/fixtures/jdk_corpus/jdk26"
            ))),
            "the real class keeps the checked-in JDK fixture as its origin"
        );
        assert!(matches!(
            store.symbols.get(class).info,
            SymbolInfo::Complete(info) if matches!(store.types.get(info), Type::ClassInfo(_))
        ));
        let declarations = resolver
            .declaration_scope(&store, class)
            .expect("DayOfWeek has a declaration scope");
        assert!(store.scopes.contains(declarations));
        assert_eq!(
            resolver.resolve_member(&mut store, &request).unwrap(),
            Some(class),
            "repeated requests reuse the loaded JDK class identity"
        );

        let is_weekend = Name::new(store.names.intern("getValue"), Namespace::Term);
        let class_prefix = store.types.alloc(Type::ThisType { class });
        let method = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: class_prefix,
                    name: is_weekend,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .expect("DayOfWeek member lookup should succeed")
            .expect("the real JDK class declares getValue");
        assert_eq!(store.symbols.get(method).owner, Some(class));
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
