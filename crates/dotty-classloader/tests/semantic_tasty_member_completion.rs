//! Explicit, test-only completion audit for the four frozen Scala 3.9 methods
//! in issue #1995. This is deliberately separate from the frozen class-root
//! comparator and its 14-unit/126-field denominator.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use dotty_classloader::classloader::{
    BinaryName, ClassLoader, ClassOrigin, ClassPathEntry, ClassPathError, ClassResource,
    ClasspathSymbolResolver, CompositeClassPath, DirectoryClassPath, JdkClassPath, LoadingSession,
};
use dotty_core::names::Name;
use dotty_core::packages::Packages;
use dotty_core::resolution::{MemberRequest, ResolutionError, ResolverCheckpoint, SymbolResolver};
use dotty_core::types::PolyType;
use dotty_core::{Definitions, SemanticStore, SymbolId, SymbolInfo, Type, TypeParam, Visibility};
use dotty_tasty::tasty::{DEFDEF_TAG, DefinitionBody, StructuredNode, TYPEDEF_TAG, TastyFile};
use dotty_tasty_unpickler::tasty_unpickler::{TastySession, TastyUnpickler, UnpickleError};

const RECEIPT: &str =
    include_str!("../../../docs/classloader-semantic-tasty-convergence-ii/member-completion.tsv");

static NEXT_TEMP_LAYOUT: AtomicU64 = AtomicU64::new(0);

struct TemporaryLayout(PathBuf);

impl Drop for TemporaryLayout {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const MEMBERS: &[(&str, &str, &str, u32)] = &[
    (
        "option-getOrElse",
        "crates/dotty-tasty/tests/fixtures/scala3-library/scala/Option.tasty",
        "getOrElse",
        666,
    ),
    (
        "info-wrapped-m",
        "crates/dotty-tasty-unpickler/tests/fixtures/semantic/InfoWrapped.tasty",
        "m",
        115,
    ),
    (
        "animal-sound",
        "crates/dotty-classloader/tests/fixtures/tasty_sample/me/cytrowski/tastyfixtures/Animal.tasty",
        "sound",
        22,
    ),
    (
        "dog-sound",
        "crates/dotty-classloader/tests/fixtures/tasty_sample/Dog.tasty",
        "sound",
        36,
    ),
];

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_owned()
}

fn classpath() -> CompositeClassPath {
    classpath_with_extra(None)
}

fn classpath_with_extra(extra: Option<PathBuf>) -> CompositeClassPath {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut entries: Vec<Box<dyn ClassPathEntry>> = vec![
        Box::new(DirectoryClassPath::new(
            root.join("tests/fixtures/tasty_sample"),
        )),
        Box::new(DirectoryClassPath::new(
            root.join("../dotty-tasty/tests/fixtures/scala3-library"),
        )),
        Box::new(DirectoryClassPath::new(
            root.join("../dotty-tasty/tests/fixtures/path_dependent"),
        )),
        Box::new(DirectoryClassPath::new(
            root.join("../dotty-tasty-unpickler/tests/fixtures/semantic"),
        )),
        Box::new(
            JdkClassPath::new(
                PathBuf::from(std::env::var_os("JAVA_HOME").expect("JDK 21 JAVA_HOME"))
                    .join("jmods"),
            )
            .expect("JDK 21 jmods are readable"),
        ),
    ];
    if let Some(path) = extra {
        entries.insert(0, Box::new(DirectoryClassPath::new(path)));
    }
    CompositeClassPath::new(entries)
}

struct TracingClassPath {
    inner: CompositeClassPath,
    trace: Arc<Mutex<Vec<String>>>,
}

impl ClassPathEntry for TracingClassPath {
    fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
        let result = self.inner.find_class(name);
        let entry = match &result {
            Ok(Some(resource)) => format!(
                "{}=selected:{:?}:{}",
                name.as_internal(),
                resource.format(),
                origin_label(resource.origin())
            ),
            Ok(None) => format!("{}=missing", name.as_internal()),
            Err(error) => format!("{}=error:{}", name.as_internal(), error.code()),
        };
        self.trace.lock().unwrap().push(entry);
        result
    }

    fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
        self.inner.contains_package(package)
    }

    fn contains_package_prefix(&self, package: &[&str]) -> Result<bool, ClassPathError> {
        self.inner.contains_package_prefix(package)
    }
}

fn origin_label(origin: &ClassOrigin) -> String {
    let (kind, path) = match origin {
        ClassOrigin::Directory(path) => ("directory", path),
        ClassOrigin::Jar(path) => ("jar", path),
        ClassOrigin::Jmod(path) => ("jmod", path),
        _ => return "other".into(),
    };
    format!(
        "{kind}:{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown")
    )
}

fn tracing_classpath(trace: Arc<Mutex<Vec<String>>>) -> CompositeClassPath {
    CompositeClassPath::new(vec![Box::new(TracingClassPath {
        inner: classpath_with_extra(None),
        trace,
    })])
}

fn tracing_classpath_without_jdk(trace: Arc<Mutex<Vec<String>>>) -> CompositeClassPath {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let inner = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(
            root.join("tests/fixtures/tasty_sample"),
        )),
        Box::new(DirectoryClassPath::new(
            root.join("../dotty-tasty/tests/fixtures/scala3-library"),
        )),
        Box::new(DirectoryClassPath::new(
            root.join("../dotty-tasty/tests/fixtures/path_dependent"),
        )),
        Box::new(DirectoryClassPath::new(
            root.join("../dotty-tasty-unpickler/tests/fixtures/semantic"),
        )),
    ]);
    CompositeClassPath::new(vec![Box::new(TracingClassPath { inner, trace })])
}

struct MaskedClassPath {
    inner: CompositeClassPath,
    masked_internal_name: &'static str,
}

impl ClassPathEntry for MaskedClassPath {
    fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
        if name.as_internal() == self.masked_internal_name {
            return Ok(None);
        }
        self.inner.find_class(name)
    }

    fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
        self.inner.contains_package(package)
    }

    fn contains_package_prefix(&self, package: &[&str]) -> Result<bool, ClassPathError> {
        self.inner.contains_package_prefix(package)
    }
}

struct TracingResolver {
    inner: Box<dyn SymbolResolver>,
    trace: Arc<Mutex<Vec<String>>>,
    fail_after_predef_string_lookup: Arc<AtomicBool>,
    rollback_count: Arc<AtomicU64>,
}

impl TracingResolver {
    fn record_request(&self, store: &SemanticStore, request: &MemberRequest) {
        self.trace.lock().unwrap().push(format!(
            "request:prefix={};name={};namespace={:?};space={:?}",
            type_repr(store, request.prefix),
            store.names.resolve(request.name.text()),
            request.name.namespace(),
            request.space
        ));
    }

    fn fail_after_predef_string_lookup(
        &self,
        store: &SemanticStore,
        request: &MemberRequest,
        result: Result<Option<SymbolId>, ResolutionError>,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        if !self.fail_after_predef_string_lookup.load(Ordering::SeqCst)
            || !request.name.is_type()
            || store.names.resolve(request.name.text()) != "String"
        {
            return result;
        }
        let is_predef_module_prefix = match store.types.get(request.prefix) {
            Type::TermRef {
                target: dotty_core::TermRefTarget::Symbol(symbol),
                ..
            } => {
                store.symbols.contains(*symbol)
                    && store.names.resolve(store.symbols.get(*symbol).name.text()) == "Predef"
            }
            Type::TypeRef {
                target: dotty_core::TypeRefTarget::Symbol(symbol),
                ..
            } => {
                store.symbols.contains(*symbol)
                    && store.names.resolve(store.symbols.get(*symbol).name.text()) == "Predef$"
            }
            _ => false,
        };
        if !is_predef_module_prefix {
            return result;
        }
        if let Ok(Some(alias)) = result.as_ref()
            && store.symbols.get(*alias).kind == dotty_core::SymbolKind::TypeAlias
            && matches!(store.symbols.get(*alias).info, SymbolInfo::Complete(_))
        {
            self.fail_after_predef_string_lookup
                .store(false, Ordering::SeqCst);
            self.trace
                .lock()
                .unwrap()
                .push("injected:after_predef_string_target".to_owned());
            return Err(ResolutionError::Malformed {
                reason: "injected enclosing TASTy transaction failure after Predef.String target materialization"
                    .to_owned(),
            });
        }
        result
    }
}

impl SymbolResolver for TracingResolver {
    fn has_package_root(&self) -> bool {
        self.inner.has_package_root()
    }

    fn take_packages(&mut self) -> Option<Packages> {
        self.inner.take_packages()
    }

    fn checkpoint_with_packages(&self, packages: &Packages) -> ResolverCheckpoint {
        self.inner.checkpoint_with_packages(packages)
    }

    fn class_type_parameters(
        &self,
        store: &SemanticStore,
        class: SymbolId,
    ) -> Option<Vec<SymbolId>> {
        self.inner.class_type_parameters(store, class)
    }

    fn package_object_collision(
        &mut self,
        store: &mut SemanticStore,
        package: SymbolId,
        name: Name,
    ) -> Result<bool, ResolutionError> {
        self.inner.package_object_collision(store, package, name)
    }

    fn module_class_of_object(
        &self,
        store: &SemanticStore,
        object: SymbolId,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        let result = self.inner.module_class_of_object(store, object);
        let object_path = owner_path(store, object);
        let observation = match &result {
            Ok(Some(module_class)) => {
                format!(
                    "module_class_of_object:{object_path}={}",
                    owner_path(store, *module_class)
                )
            }
            Ok(None) => format!("module_class_of_object:{object_path}=none"),
            Err(error) => format!(
                "module_class_of_object:{object_path}=error:{}",
                normalize_error(&format!("{error:?}"))
            ),
        };
        self.trace.lock().unwrap().push(observation);
        result
    }

    fn complete_module_class(
        &mut self,
        store: &mut SemanticStore,
        module_class: SymbolId,
    ) -> Result<bool, ResolutionError> {
        let path = owner_path(store, module_class);
        let result = self.inner.complete_module_class(store, module_class);
        let observation = match &result {
            Ok(completed) => format!("complete_module_class:{path}={completed}"),
            Err(error) => format!(
                "complete_module_class:{path}=error:{}",
                normalize_error(&format!("{error:?}"))
            ),
        };
        self.trace.lock().unwrap().push(observation);
        result
    }

    fn complete_class(
        &mut self,
        store: &mut SemanticStore,
        symbol: SymbolId,
    ) -> Result<bool, ResolutionError> {
        self.inner.complete_class(store, symbol)
    }

    fn checkpoint(&self) -> ResolverCheckpoint {
        self.inner.checkpoint()
    }

    fn rollback_to(&mut self, store: &mut SemanticStore, checkpoint: ResolverCheckpoint) {
        self.rollback_count.fetch_add(1, Ordering::SeqCst);
        self.inner.rollback_to(store, checkpoint)
    }

    fn rollback_to_with_packages(
        &mut self,
        store: &mut SemanticStore,
        checkpoint: ResolverCheckpoint,
        packages: &mut Packages,
    ) {
        self.rollback_count.fetch_add(1, Ordering::SeqCst);
        self.inner
            .rollback_to_with_packages(store, checkpoint, packages)
    }

    fn resolve_member(
        &mut self,
        store: &mut SemanticStore,
        request: &MemberRequest,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        self.record_request(store, request);
        let result = self.inner.resolve_member(store, request);
        self.fail_after_predef_string_lookup(store, request, result)
    }

    fn resolve_member_with_packages(
        &mut self,
        store: &mut SemanticStore,
        packages: &mut Packages,
        request: &MemberRequest,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        self.record_request(store, request);
        let result = self
            .inner
            .resolve_member_with_packages(store, packages, request);
        self.fail_after_predef_string_lookup(store, request, result)
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
        self.inner
            .resolve_package_with_packages(store, packages, path)
    }

    fn enter_synthetic_package_member(
        &mut self,
        store: &mut SemanticStore,
        package: SymbolId,
        name: Name,
        member: SymbolId,
    ) -> Result<bool, ResolutionError> {
        self.inner
            .enter_synthetic_package_member(store, package, name, member)
    }
}

fn member_address(file: &TastyFile<'_>, name: &str) -> Option<u32> {
    let index = file.ast_address_index().ok()?;
    let mut matches = index
        .iter_nodes()
        .filter(|node| node.tag == DEFDEF_TAG)
        .filter_map(|node| {
            let address = u32::try_from(node.offset).ok()?;
            let body = index.get(address)?.decode_defdef_body().ok()?;
            (file.names().get_utf8(body.name) == Some(name)).then_some(address)
        })
        .collect::<Vec<_>>();
    matches.sort_unstable();
    (matches.len() == 1).then(|| matches[0])
}

fn owner_address(file: &TastyFile<'_>, name: &str) -> Option<u32> {
    let index = file.ast_address_index().ok()?;
    let mut matches = index
        .iter_nodes()
        .filter(|node| node.tag == TYPEDEF_TAG)
        .filter_map(|node| {
            let address = u32::try_from(node.offset).ok()?;
            let StructuredNode::TypeDef(DefinitionBody::TypeDef { name: name_ref, .. }) =
                index.get(address)?.decode_structured().ok()?
            else {
                return None;
            };
            (file.names().get_utf8(name_ref) == Some(name)).then_some(address)
        })
        .collect::<Vec<_>>();
    matches.sort_unstable();
    (matches.len() == 1).then(|| matches[0])
}

fn member_owner_address(file: &TastyFile<'_>, address: u32) -> Option<u32> {
    let index = file.ast_address_index().ok()?;
    let mut node = index.get_node(address)?;
    while let Some(parent) = index.parent_of(u32::try_from(node.offset).ok()?) {
        if parent.tag == TYPEDEF_TAG {
            return u32::try_from(parent.offset).ok();
        }
        node = parent;
    }
    None
}

fn type_def_name(file: &TastyFile<'_>, address: u32) -> Option<String> {
    let index = file.ast_address_index().ok()?;
    let StructuredNode::TypeDef(DefinitionBody::TypeDef { name, .. }) =
        index.get(address)?.decode_structured().ok()?
    else {
        return None;
    };
    Some(file.names().get_utf8(name)?.to_owned())
}

fn owner_path(store: &SemanticStore, id: SymbolId) -> String {
    let mut parts = Vec::new();
    let mut current = Some(id);
    while let Some(symbol) = current {
        let value = store.symbols.get(symbol);
        let text = store.names.resolve(value.name.text());
        if !text.is_empty() && text != "_root_" {
            parts.push(text.to_owned());
        }
        current = value.owner;
    }
    parts.reverse();
    parts.join(".")
}

#[derive(Debug, PartialEq, Eq)]
struct ForeignOwner {
    requested: String,
    actual: String,
}

#[derive(Debug)]
enum AuditRequestError {
    ForeignOwner(ForeignOwner),
    InvalidOwnerIndex { address: u32 },
    Completion(UnpickleError),
}

fn complete_audit_request(
    file: &TastyFile<'_>,
    unpickler: &mut TastyUnpickler<'_, '_, '_>,
    address: u32,
    requested_owner: &str,
) -> Result<dotty_core::TypeId, AuditRequestError> {
    if unpickler.index().symbol_at(address).is_none() {
        return Err(AuditRequestError::Completion(
            UnpickleError::MissingEnteredSymbol { address },
        ));
    }
    let actual_owner_address = member_owner_address(file, address)
        .ok_or(AuditRequestError::InvalidOwnerIndex { address })?;
    let actual_owner = type_def_name(file, actual_owner_address)
        .ok_or(AuditRequestError::InvalidOwnerIndex { address })?;
    if unpickler.index().symbol_at(actual_owner_address).is_none() {
        return Err(AuditRequestError::Completion(
            UnpickleError::MissingEnteredSymbol {
                address: actual_owner_address,
            },
        ));
    }
    if requested_owner == actual_owner {
        unpickler
            .complete_symbol(address)
            .map_err(AuditRequestError::Completion)
    } else {
        Err(AuditRequestError::ForeignOwner(ForeignOwner {
            requested: requested_owner.to_owned(),
            actual: actual_owner,
        }))
    }
}

fn verify_alias_owner(
    store: &SemanticStore,
    alias: SymbolId,
    expected_owner: SymbolId,
) -> Result<SymbolId, AuditRequestError> {
    let symbol = store.symbols.get(alias);
    if symbol.kind != dotty_core::SymbolKind::TypeAlias
        || symbol.name.namespace() != dotty_core::Namespace::Type
    {
        return Err(AuditRequestError::InvalidOwnerIndex { address: 378 });
    }
    let owner = symbol
        .owner
        .ok_or(AuditRequestError::InvalidOwnerIndex { address: 378 })?;
    let expected = store.symbols.get(expected_owner);
    if expected.kind != dotty_core::SymbolKind::ModuleClass
        || expected.name.namespace() != dotty_core::Namespace::Type
    {
        return Err(AuditRequestError::InvalidOwnerIndex { address: 378 });
    }
    if owner == expected_owner {
        Ok(alias)
    } else {
        Err(AuditRequestError::ForeignOwner(ForeignOwner {
            requested: store.names.resolve(expected.name.text()).to_owned(),
            actual: store
                .names
                .resolve(store.symbols.get(owner).name.text())
                .to_owned(),
        }))
    }
}

#[derive(Default)]
struct BinderNames {
    ids: std::collections::HashMap<dotty_core::TypeId, usize>,
}

impl BinderNames {
    fn identify(&mut self, id: dotty_core::TypeId) -> usize {
        let next = self.ids.len();
        *self.ids.entry(id).or_insert(next)
    }
}

fn type_repr(store: &SemanticStore, id: dotty_core::TypeId) -> String {
    type_repr_inner(store, id, &mut BinderNames::default())
}

fn type_repr_inner(
    store: &SemanticStore,
    id: dotty_core::TypeId,
    binders: &mut BinderNames,
) -> String {
    match store.types.get(id) {
        Type::NoType => "NoType".into(),
        Type::NoPrefix => "NoPrefix".into(),
        Type::Error(error) => format!("Error({})", store.names.resolve(error.message)),
        Type::TypeRef { target, .. } => target
            .symbol()
            .map(|symbol| owner_path(store, symbol))
            .unwrap_or_else(|| "unresolved-type-name".into()),
        Type::TermRef { target, .. } => target
            .symbol()
            .map(|symbol| owner_path(store, symbol))
            .unwrap_or_else(|| "unresolved-term-name".into()),
        Type::Applied { tycon, args } => format!(
            "{}[{}]",
            type_repr_inner(store, *tycon, binders),
            args.iter()
                .map(|arg| type_repr_inner(store, *arg, binders))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Type::Method(method) => {
            let binder = binders.identify(id);
            format!(
                "method[binder{binder}]({}){:?}:{}",
                method
                    .params
                    .iter()
                    .map(|param| type_repr_inner(store, param.ty, binders))
                    .collect::<Vec<_>>()
                    .join(","),
                method.kind,
                type_repr_inner(store, method.result, binders)
            )
        }
        Type::Poly(poly) => {
            let binder = binders.identify(id);
            format!(
                "[binder{binder};{}]=>{}",
                type_params_repr(store, &poly.params, binders),
                type_repr_inner(store, poly.result, binders)
            )
        }
        Type::Bounds { low, high } => format!(
            "bounds({},{})",
            type_repr_inner(store, *low, binders),
            type_repr_inner(store, *high, binders)
        ),
        Type::AliasingBounds { alias } => {
            format!("alias({})", type_repr_inner(store, *alias, binders))
        }
        Type::ByName { result } => format!("by-name({})", type_repr_inner(store, *result, binders)),
        Type::Flexible { underlying } => {
            format!("flexible({})", type_repr_inner(store, *underlying, binders))
        }
        Type::And { left, right } => format!(
            "and({},{})",
            type_repr_inner(store, *left, binders),
            type_repr_inner(store, *right, binders)
        ),
        Type::Or { left, right } => format!(
            "or({},{})",
            type_repr_inner(store, *left, binders),
            type_repr_inner(store, *right, binders)
        ),
        Type::ThisType { class } => format!("this({})", owner_path(store, *class)),
        Type::SuperType { super_type, .. } => {
            format!("super({})", type_repr_inner(store, *super_type, binders))
        }
        Type::JavaArray { element } => {
            format!("Array[{}]", type_repr_inner(store, *element, binders))
        }
        Type::Repeated { element } => {
            format!("Repeated[{}]", type_repr_inner(store, *element, binders))
        }
        Type::ParamRef { binder, index } => match binders.ids.get(binder) {
            Some(identity) => format!("ParamRef[binder{identity};{index}]"),
            None => format!("ParamRef[external-binder;{index}]"),
        },
        Type::TypeLambda(lambda) => {
            let binder = binders.identify(id);
            format!(
                "type-lambda[binder{binder};{}]=>{}",
                type_params_repr(store, &lambda.params, binders),
                type_repr_inner(store, lambda.result, binders)
            )
        }
        Type::Constant(value) => format!("constant({value:?})"),
        Type::Wildcard { bounds } => {
            format!("Wildcard({})", type_repr_inner(store, *bounds, binders))
        }
        _ => "unknown-semantic-type-shape".into(),
    }
}

fn type_params_repr(
    store: &SemanticStore,
    params: &[TypeParam],
    binders: &mut BinderNames,
) -> String {
    params
        .iter()
        .map(|param| {
            format!(
                "{}:{:?}:{}",
                store.names.resolve(param.name.as_name().text()),
                param.declared_variance,
                type_repr_inner(store, param.bounds, binders)
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn visibility(store: &SemanticStore, visibility: &Visibility) -> String {
    match visibility {
        Visibility::Public => "public".into(),
        Visibility::Private => "private".into(),
        Visibility::Protected => "protected".into(),
        Visibility::Package(owner) => format!("package:{}", owner_path(store, *owner)),
        Visibility::PrivateWithin(owner) => format!("private-within:{}", owner_path(store, *owner)),
        Visibility::ProtectedWithin(owner) => {
            format!("protected-within:{}", owner_path(store, *owner))
        }
    }
}

fn run_once() -> String {
    let root = workspace();
    let mut receipt = String::from(
        "member\tpath\tsha256\ttasty_version\towner\towner_kind\towner_address\taddress\tnamespace\tkind\tvisibility\tlegacy_outcome\tlegacy_signature\tcompletion\tcanonical_signature\tcomparison\tcanonical_request\tcanonical_resolution_trace\terror\n",
    );
    for (id, relpath, method, requested_address) in MEMBERS {
        let path = root.join(relpath);
        let bytes = fs::read(&path).expect("frozen fixture is present");
        let digest = Command::new("sha256sum")
            .arg(&path)
            .output()
            .expect("sha256sum available");
        assert!(digest.status.success());
        let hash = String::from_utf8(digest.stdout)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .to_owned();
        let file =
            TastyFile::parse_and_validate_scala_3_9(&bytes).expect("Scala 3.9 fixture parses");
        let address =
            member_address(&file, method).expect("exactly one named DEFDEF in original unit");
        if *requested_address != 0 {
            assert_eq!(
                address, *requested_address,
                "verified frozen address for {id}"
            );
        }
        let header = file.header();
        assert_eq!(header.major_version, 28);
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let resolution_trace = Arc::new(Mutex::new(Vec::new()));
        let resolver = TracingResolver {
            inner: Box::new(ClasspathSymbolResolver::new(
                tracing_classpath(Arc::clone(&resolution_trace)),
                definitions,
                LoadingSession::new(),
            )),
            trace: Arc::clone(&resolution_trace),
            fail_after_predef_string_lookup: Arc::new(AtomicBool::new(false)),
            rollback_count: Arc::new(AtomicU64::new(0)),
        };
        let session = TastySession::new()
            .with_resolver(Box::new(resolver))
            .expect("resolver-backed fresh session");
        let mut unpickler = TastyUnpickler::with_session(&file, &mut store, definitions, session);
        unpickler
            .enter_symbols()
            .expect("all four original units enter");
        let owner_simple = match *id {
            "option-getOrElse" => "Option",
            "info-wrapped-m" => "InfoWrapped",
            "animal-sound" => "Animal",
            "dog-sound" => "Dog",
            _ => unreachable!(),
        };
        let owner_address = owner_address(&file, owner_simple).expect("original owner TYPEDEF");
        let owner_id = unpickler
            .index()
            .symbol_at(owner_address)
            .expect("owner from unit-local index");
        let member_id = unpickler
            .index()
            .symbol_at(address)
            .expect("method address maps through this unit's index");
        assert_eq!(
            unpickler.symbol_state_at(address).unwrap().0,
            dotty_core::SymbolKind::Method
        );
        let (outcome, signature, error) =
            match complete_audit_request(&file, &mut unpickler, address, owner_simple) {
                Ok(signature) => ("COMPLETE".to_owned(), Some(signature), String::new()),
                Err(AuditRequestError::Completion(error)) => (
                    classify(&error).to_owned(),
                    None,
                    normalize_error(&format!("{error:?}")),
                ),
                Err(AuditRequestError::ForeignOwner(error)) => {
                    panic!("unexpected owner mismatch: {error:?}")
                }
                Err(AuditRequestError::InvalidOwnerIndex { address }) => {
                    panic!("member {address} has no indexed TYPEDEF owner")
                }
            };
        drop(unpickler);
        let trace = resolution_trace.lock().unwrap();
        let canonical_request = trace
            .iter()
            .find(|entry| entry.starts_with("request:"))
            .cloned()
            .unwrap_or_default();
        let canonical_resolution_trace = trace
            .iter()
            .filter(|entry| {
                !entry.starts_with("request:")
                    && (entry.starts_with("scala/Predef=")
                        || entry == &"scala/Predef/String=missing"
                        || entry == &"scala/Predef$String=missing")
            })
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(";");
        drop(trace);
        if *id == "animal-sound" || *id == "dog-sound" {
            assert!(canonical_request.contains("prefix=scala;name=Predef;namespace=Term"));
            assert!(
                canonical_resolution_trace
                    .contains("scala/Predef=selected:Tasty:directory:scala3-library")
            );
            assert_eq!(outcome, "COMPLETE");
            assert!(error.is_empty());
        }
        let owner = store
            .symbols
            .get(member_id)
            .owner
            .expect("method has canonical owner");
        assert_eq!(
            owner, owner_id,
            "member owner equals this unit's indexed owner symbol"
        );
        let owner_name = owner_path(&store, owner);
        let owner_kind = format!("{:?}", store.symbols.get(owner).kind);
        let member_namespace = format!("{:?}", store.symbols.get(member_id).name.namespace());
        let member_visibility = visibility(&store, &store.symbols.get(member_id).visibility);
        let signature = signature
            .map(|id| type_repr(&store, id))
            .unwrap_or_default();
        let legacy = legacy_method(id);
        let comparison = if outcome == "COMPLETE" && legacy.1 == signature {
            "semantic-equal"
        } else if outcome == "COMPLETE" && legacy.0 == "MATCH" {
            "semantic-different"
        } else {
            "not-comparable"
        };
        assert_eq!(
            owner_name.rsplit('.').next(),
            Some(match *id {
                "option-getOrElse" => "Option",
                "info-wrapped-m" => "InfoWrapped",
                "animal-sound" => "Animal",
                "dog-sound" => "Dog",
                _ => unreachable!(),
            })
        );
        row(
            &mut receipt,
            &[
                id,
                relpath,
                &hash,
                &format!("{}.{}", header.major_version, header.minor_version),
                &owner_name,
                &owner_kind,
                &owner_address.to_string(),
                &address.to_string(),
                &member_namespace,
                "Method",
                &member_visibility,
                &legacy.0,
                &legacy.1,
                &outcome,
                &signature,
                comparison,
                &canonical_request,
                &canonical_resolution_trace,
                &error,
            ],
        );
    }
    receipt
}

fn legacy_method(id: &str) -> (String, String) {
    let lookup = match id {
        "option-getOrElse" => "scala/Option",
        "info-wrapped-m" => "me/cytrowski/tastyfixtures/semantic/InfoWrapped",
        "animal-sound" => "me/cytrowski/tastyfixtures/Animal",
        "dog-sound" => "Dog",
        _ => unreachable!(),
    };
    let method_name = match id {
        "option-getOrElse" => "getOrElse",
        "info-wrapped-m" => "m",
        _ => "sound",
    };
    // The semantic fixture generator stores packaged TASTy files flat. Put
    // the exact original bytes in their package-relative classpath layout for
    // the legacy reader; no fixture is generated or semantically rewritten.
    let layout = if id == "info-wrapped-m" {
        let dir = std::env::temp_dir().join(format!(
            "dotty-semantic-layout-{}-{}",
            std::process::id(),
            NEXT_TEMP_LAYOUT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).expect("unique temporary package root");
        let layout = TemporaryLayout(dir);
        let package = layout.0.join("me/cytrowski/tastyfixtures/semantic");
        fs::create_dir_all(&package).expect("temporary package layout");
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../dotty-tasty-unpickler/tests/fixtures/semantic");
        fs::copy(
            source.join("InfoWrapped.tasty"),
            package.join("InfoWrapped.tasty"),
        )
        .unwrap();
        fs::copy(
            source.join("InfoBase.tasty"),
            package.join("InfoBase.tasty"),
        )
        .unwrap();
        Some(layout)
    } else {
        None
    };
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut loader = ClassLoader::with_definitions(
        classpath_with_extra(layout.as_ref().map(|layout| layout.0.clone())),
        &mut store,
        definitions,
        LoadingSession::new(),
    );
    let loaded = loader.load_class(&BinaryName::from_internal(lookup));
    drop(loader);
    drop(layout);
    let owner = match loaded {
        Ok(owner) => owner,
        Err(error) => {
            return (
                "LEGACY_ERROR".into(),
                normalize_error(&format!("{error:?}")),
            );
        }
    };
    let info = match store.symbols.get(owner).info {
        SymbolInfo::Complete(info) => info,
        _ => return ("LEGACY_MISSING_OWNER_INFO".into(), String::new()),
    };
    let declarations = match store.types.get(info) {
        Type::ClassInfo(class) => class.declarations,
        _ => return ("LEGACY_INVALID_OWNER_INFO".into(), String::new()),
    };
    let member = store
        .scopes
        .get(declarations)
        .entered_symbols()
        .find(|member| store.names.resolve(store.symbols.get(*member).name.text()) == method_name);
    let Some(member) = member else {
        return ("LEGACY_MEMBER_MISSING".into(), String::new());
    };
    match store.symbols.get(member).info {
        SymbolInfo::Complete(signature) => ("MATCH".into(), type_repr(&store, signature)),
        _ => ("LEGACY_MEMBER_INFO_MISSING".into(), String::new()),
    }
}

fn classify(error: &UnpickleError) -> &'static str {
    use dotty_tasty_unpickler::tasty_unpickler::UnpickleErrorCategory as C;
    match error.category() {
        C::MalformedInput => "MALFORMED",
        C::UnsupportedFeature => "UNSUPPORTED_SHAPE",
        C::ResolutionFailure => "UNRESOLVED_EXTERNAL",
        _ => "INVALID_OWNER",
    }
}

fn normalize_error(error: &str) -> String {
    let mut result = String::new();
    let mut rest = error;
    while let Some(start) = rest.find("TypeId(") {
        result.push_str(&rest[..start]);
        result.push_str("TypeId(*)");
        let after = &rest[start + 7..];
        if let Some(end) = after.find(')') {
            rest = &after[end + 1..];
        } else {
            rest = "";
            break;
        }
    }
    result.push_str(rest);
    result
}

fn row(output: &mut String, cells: &[&str]) {
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            output.push('\t');
        }
        let cell = if cell.is_empty() { "-" } else { cell };
        output.push_str(&cell.replace('\t', "\\t").replace('\n', "\\n"));
    }
    output.push('\n');
}

#[test]
fn four_frozen_members_are_explicitly_completed_in_fresh_deterministic_runs() {
    let first = run_once();
    let second = run_once();
    assert_eq!(
        first, second,
        "normalized receipts are byte-identical across fresh stores"
    );
    assert_eq!(
        first.lines().count(),
        5,
        "exactly the frozen four methods are attempted"
    );
    for (member, address) in [("animal-sound", "22"), ("dog-sound", "36")] {
        let current = first
            .lines()
            .find(|line| line.starts_with(&format!("{member}\t")))
            .expect("current A2 sound receipt row");
        let cells = current.split('\t').collect::<Vec<_>>();
        assert_eq!(cells[7], address);
        assert_eq!(cells[13], "COMPLETE");
        assert_eq!(cells[14], "by-name(scala.Predef$.String)");
        assert_eq!(cells[15], "semantic-different");
        assert_eq!(cells[18], "-");
    }
    assert!(RECEIPT.contains("animal-sound\t"));
    assert!(RECEIPT.contains("UNRESOLVED_EXTERNAL"));
    assert!(RECEIPT.contains("UnresolvedMember { address: 27"));
    assert!(RECEIPT.contains("UnresolvedMember { address: 41"));
    println!("{first}");
}

#[test]
fn external_option_is_empty_uses_the_canonical_boolean_definition() {
    let path =
        workspace().join("crates/dotty-tasty/tests/fixtures/scala3-library/scala/Option.tasty");
    let bytes = fs::read(&path).expect("pinned Scala 3.9 Option fixture exists");
    let file =
        TastyFile::parse_and_validate_scala_3_9(&bytes).expect("Scala 3.9 Option TASTy parses");
    assert!(member_address(&file, "isEmpty").is_some());
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut loader =
        ClassLoader::with_definitions(classpath(), &mut store, definitions, LoadingSession::new());
    let option = loader
        .load_class(&BinaryName::from_internal("scala/Option"))
        .expect("external Option TASTy loads through the classloader");
    assert!(
        matches!(
            loader.origin(option),
            Some(ClassOrigin::Directory(origin)) if origin.ends_with("scala3-library")
        ),
        "Option origin: {:?}",
        loader.origin(option)
    );
    drop(loader);
    let SymbolInfo::Complete(option_info) = store.symbols.get(option).info else {
        panic!("Option class information is complete");
    };
    let Type::ClassInfo(option_info) = store.types.get(option_info) else {
        panic!("Option has class information");
    };
    let method = store
        .scopes
        .get(option_info.declarations)
        .entered_symbols()
        .find(|symbol| store.names.resolve(store.symbols.get(*symbol).name.text()) == "isEmpty")
        .expect("Option.isEmpty declaration is entered");
    let SymbolInfo::Complete(method_info) = store.symbols.get(method).info else {
        panic!("Option.isEmpty has completed method information");
    };
    let Type::ByName { result } = store.types.get(method_info) else {
        panic!("Option.isEmpty preserves its parameterless by-name type");
    };
    let boolean_class = store
        .types
        .get(definitions.boolean)
        .reference_symbol()
        .expect("canonical Boolean is class-backed");
    assert_eq!(
        *result, definitions.boolean,
        "external Option.isEmpty must reuse the canonical Boolean TypeId"
    );
    assert_eq!(
        store.types.get(*result).reference_symbol(),
        Some(boolean_class)
    );
    assert_eq!(store.symbols.get(boolean_class).owner, None);
    assert_eq!(
        store.symbols.get(boolean_class).origin,
        dotty_core::SymbolOrigin::Builtin
    );
    assert!(
        definitions
            .boolean_primitive_operator_symbols(&store)
            .is_some()
    );
}

#[test]
fn normalized_param_refs_distinguish_their_binder() {
    let mut store = SemanticStore::new();
    let correct = store.types.reserve();
    let other = store.types.reserve();
    let correct_ref = store.types.alloc(Type::ParamRef {
        binder: correct.id(),
        index: 0,
    });
    let wrong_ref = store.types.alloc(Type::ParamRef {
        binder: other.id(),
        index: 0,
    });
    let correct_poly = store.types.fill(
        correct,
        Type::Poly(PolyType {
            params: Vec::new(),
            result: correct_ref,
        }),
    );
    store.types.fill(
        other,
        Type::Poly(PolyType {
            params: Vec::new(),
            result: wrong_ref,
        }),
    );
    assert_eq!(
        type_repr(&store, correct_poly),
        "[binder0;]=>ParamRef[binder0;0]"
    );
    let mut context = BinderNames::default();
    context.identify(correct_poly);
    assert_eq!(
        type_repr_inner(&store, wrong_ref, &mut context),
        "ParamRef[external-binder;0]"
    );
}

#[test]
fn unknown_method_address_is_typed_and_does_not_poison_a_valid_retry() {
    let (_, relpath, method, _) = MEMBERS[1];
    let path = workspace().join(relpath);
    let bytes = fs::read(path).unwrap();
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let resolver = ClasspathSymbolResolver::new(classpath(), definitions, LoadingSession::new());
    let session = TastySession::new()
        .with_resolver(Box::new(resolver))
        .unwrap();
    let mut unpickler = TastyUnpickler::with_session(&file, &mut store, definitions, session);
    unpickler.enter_symbols().unwrap();
    let error = unpickler
        .complete_symbol(u32::MAX)
        .expect_err("dangling address is rejected");
    assert!(matches!(
        error,
        UnpickleError::MissingEnteredSymbol { address: u32::MAX }
    ));
    let address = member_address(&file, method).unwrap();
    assert_eq!(
        unpickler.symbol_state_at(address).unwrap().1,
        dotty_core::SymbolInfo::Missing,
        "dangling address leaves the method pending"
    );
    unpickler
        .complete_symbol(address)
        .expect("valid method remains completable after negative control");
}

#[test]
fn malformed_bytes_and_foreign_owner_controls_fail_closed() {
    let (_, relpath, method, _) = MEMBERS[1];
    let path = workspace().join(relpath);
    let bytes = fs::read(&path).unwrap();
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let bootstrapped = store.checkpoint();
    assert!(
        TastyFile::parse_scala_3_9(&bytes[..bytes.len() - 1]).is_err(),
        "truncated original bytes are malformed"
    );
    assert_eq!(
        store.checkpoint(),
        bootstrapped,
        "malformed parse leaves the committed bootstrap state intact"
    );
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let resolver = ClasspathSymbolResolver::new(classpath(), definitions, LoadingSession::new());
    let session = TastySession::new()
        .with_resolver(Box::new(resolver))
        .unwrap();
    let mut unpickler = TastyUnpickler::with_session(&file, &mut store, definitions, session);
    unpickler.enter_symbols().unwrap();
    let address = member_address(&file, method).unwrap();
    unpickler
        .complete_symbol(address)
        .expect("original member remains completable after the malformed-input control");
    drop(unpickler);
    let dog_path = workspace().join(MEMBERS[3].1);
    let dog_bytes = fs::read(dog_path).unwrap();
    let dog_file = TastyFile::parse_scala_3_9(&dog_bytes).unwrap();
    let mut dog_store = SemanticStore::new();
    let dog_definitions = Definitions::bootstrap(&mut dog_store);
    let dog_resolver =
        ClasspathSymbolResolver::new(classpath(), dog_definitions, LoadingSession::new());
    let dog_session = TastySession::new()
        .with_resolver(Box::new(dog_resolver))
        .unwrap();
    let mut dog_unpickler =
        TastyUnpickler::with_session(&dog_file, &mut dog_store, dog_definitions, dog_session);
    dog_unpickler.enter_symbols().unwrap();
    let dog_address = member_address(&dog_file, "sound").unwrap();
    let before = dog_unpickler.symbol_state_at(dog_address).unwrap();
    let error = complete_audit_request(&dog_file, &mut dog_unpickler, dog_address, "Animal")
        .expect_err("a real Dog.sound address cannot be requested as Animal.sound");
    assert!(matches!(
        error,
        AuditRequestError::ForeignOwner(ForeignOwner { requested, actual })
            if requested == "Animal" && actual == "Dog"
    ));
    assert_eq!(
        dog_unpickler.symbol_state_at(dog_address).unwrap(),
        before,
        "foreign-owner rejection leaves the committed method state unchanged"
    );
    complete_audit_request(&dog_file, &mut dog_unpickler, dog_address, "Dog")
        .expect("canonical Dog.sound signature completes via the Predef String alias");
}

#[test]
fn enclosing_tasty_rollback_restores_predef_target_and_allows_retry() {
    let path = workspace().join(MEMBERS[2].1);
    let bytes = fs::read(path).expect("pinned Animal TASTy fixture");
    let file = TastyFile::parse_and_validate_scala_3_9(&bytes).expect("Scala 3.9 Animal TASTy");
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let trace = Arc::new(Mutex::new(Vec::new()));
    let fail = Arc::new(AtomicBool::new(true));
    let rollback_count = Arc::new(AtomicU64::new(0));
    let resolver = TracingResolver {
        inner: Box::new(ClasspathSymbolResolver::new(
            tracing_classpath(Arc::clone(&trace)),
            definitions,
            LoadingSession::new(),
        )),
        trace: Arc::clone(&trace),
        fail_after_predef_string_lookup: Arc::clone(&fail),
        rollback_count: Arc::clone(&rollback_count),
    };
    let session = TastySession::new()
        .with_resolver(Box::new(resolver))
        .expect("resolver-backed TASTy session");
    let mut unpickler = TastyUnpickler::with_session(&file, &mut store, definitions, session);
    unpickler.enter_symbols().expect("Animal symbols entered");
    let address = member_address(&file, "sound").expect("the single Animal.sound method");
    let before = unpickler.symbol_state_at(address).unwrap();
    let error = unpickler
        .complete_symbol(address)
        .expect_err("late injected failure should abort the enclosing TASTy completion");
    assert_eq!(classify(&error), "UNRESOLVED_EXTERNAL");
    assert!(
        trace
            .lock()
            .unwrap()
            .iter()
            .any(|entry| entry == "injected:after_predef_string_target")
    );
    assert!(rollback_count.load(Ordering::SeqCst) > 0);
    assert_eq!(unpickler.symbol_state_at(address).unwrap(), before);

    fail.store(false, Ordering::SeqCst);
    unpickler
        .complete_symbol(address)
        .expect("the same unpickler store and resolver should retry after rollback");
    let method = unpickler.index().symbol_at(address).unwrap();
    drop(unpickler);
    let SymbolInfo::Complete(method_info) = store.symbols.get(method).info else {
        panic!("successful retry should complete the selected method signature");
    };
    let result = match store.types.get(method_info) {
        Type::ByName { result } => *result,
        _ => panic!("Animal.sound fixture result should preserve its by-name wrapper"),
    };
    let Type::TypeRef {
        target: dotty_core::TypeRefTarget::Symbol(alias),
        ..
    } = store.types.get(result)
    else {
        panic!("retry should refer to the source-owned Predef.String alias");
    };
    assert_eq!(
        store.symbols.get(*alias).kind,
        dotty_core::SymbolKind::TypeAlias
    );
    let SymbolInfo::Complete(alias_info) = store.symbols.get(*alias).info else {
        panic!("Predef.String alias target should survive the successful retry");
    };
    let Type::AliasingBounds { alias: target } = store.types.get(alias_info) else {
        panic!("Predef.String target should preserve alias bounds");
    };
    assert!(matches!(
        store.types.get(*target),
        Type::TypeRef {
            target: dotty_core::TypeRefTarget::Symbol(target_class),
            ..
        } if store.names.resolve(store.symbols.get(*target_class).name.text()) == "String"
    ));
}

fn predef_alias_probe(with_jdk: bool) -> (String, String, String) {
    let path =
        workspace().join("crates/dotty-tasty/tests/fixtures/scala3-library/scala/Predef.tasty");
    let bytes = fs::read(&path).expect("pinned Predef fixture");
    let file = TastyFile::parse_and_validate_scala_3_9(&bytes).expect("Scala 3.9 Predef TASTy");
    assert_eq!(
        sha256_file(&path),
        "2faef7420109c4e44074ab0385dce2c31e751fd992621fd1de312eed6c362fb2"
    );
    if with_jdk {
        let java_home = PathBuf::from(std::env::var_os("JAVA_HOME").expect("JDK 21 JAVA_HOME"));
        assert_eq!(
            sha256_file(&java_home.join("jmods/java.base.jmod")),
            "fad51abeafe6a1ecb0c2696a88684dc417cc534b74b5d865042df7381d9d0e91"
        );
        let version = Command::new("java")
            .arg("--version")
            .output()
            .expect("JDK 21 java");
        assert!(version.status.success());
        assert!(String::from_utf8_lossy(&version.stdout).contains("21.0.13"));
    }
    let mut identity_store = SemanticStore::new();
    let identity_definitions = Definitions::bootstrap(&mut identity_store);
    let mut identity_unpickler = TastyUnpickler::with_session(
        &file,
        &mut identity_store,
        identity_definitions,
        TastySession::new(),
    );
    identity_unpickler
        .enter_symbols()
        .expect("Predef identities entered");
    let alias_id = identity_unpickler
        .index()
        .symbol_at(378)
        .expect("String alias entered");
    let object_id = identity_unpickler
        .index()
        .symbol_at(107)
        .expect("Predef term object entered");
    let canonical_module_class = identity_unpickler
        .index()
        .symbol_at(128)
        .expect("canonical Predef module class entered");
    drop(identity_unpickler);
    assert_eq!(
        identity_store.symbols.get(alias_id).info,
        dotty_core::SymbolInfo::Missing
    );
    let original_owner = identity_store.symbols.get(alias_id).owner;
    assert_eq!(original_owner, Some(canonical_module_class));
    identity_store.symbols.get_mut(alias_id).owner = Some(object_id);
    let wrong_owner = verify_alias_owner(&identity_store, alias_id, canonical_module_class)
        .expect_err("the Predef term object cannot own this type alias");
    assert!(matches!(
        &wrong_owner,
        AuditRequestError::ForeignOwner(ForeignOwner { requested, actual })
            if requested == "Predef$" && actual == "Predef"
    ));
    identity_store.symbols.get_mut(alias_id).owner = original_owner;
    assert_eq!(
        identity_store.symbols.get(alias_id).info,
        dotty_core::SymbolInfo::Missing,
        "wrong-owner control is observational and leaves the alias pending"
    );
    let mut foreign_owner = identity_store.symbols.get(canonical_module_class).clone();
    foreign_owner.owner = None;
    let foreign_owner_id = identity_store.symbols.alloc(foreign_owner);
    assert_ne!(foreign_owner_id, canonical_module_class);
    assert_eq!(
        identity_store
            .names
            .resolve(identity_store.symbols.get(foreign_owner_id).name.text()),
        "Predef$"
    );
    identity_store.symbols.get_mut(alias_id).owner = Some(foreign_owner_id);
    let foreign_same_name = verify_alias_owner(&identity_store, alias_id, canonical_module_class)
        .expect_err("a distinct same-named owner cannot own the indexed alias");
    assert!(matches!(
        &foreign_same_name,
        AuditRequestError::ForeignOwner(ForeignOwner { requested, actual })
            if requested == "Predef$" && actual == "Predef$"
    ));
    identity_store.symbols.get_mut(alias_id).owner = original_owner;
    assert!(matches!(
        verify_alias_owner(&identity_store, alias_id, canonical_module_class),
        Ok(id) if id == alias_id
    ));

    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let trace = Arc::new(Mutex::new(Vec::new()));
    let classpath = if with_jdk {
        tracing_classpath(Arc::clone(&trace))
    } else {
        tracing_classpath_without_jdk(Arc::clone(&trace))
    };
    let resolver = TracingResolver {
        inner: Box::new(ClasspathSymbolResolver::new(
            classpath,
            definitions,
            LoadingSession::new(),
        )),
        trace: Arc::clone(&trace),
        fail_after_predef_string_lookup: Arc::new(AtomicBool::new(false)),
        rollback_count: Arc::new(AtomicU64::new(0)),
    };
    let session = TastySession::new()
        .with_resolver(Box::new(resolver))
        .expect("resolver-backed alias audit session");
    let mut unpickler = TastyUnpickler::with_session(&file, &mut store, definitions, session);
    unpickler.enter_symbols().expect("Predef symbols entered");
    assert_eq!(
        unpickler.symbol_state_at(378),
        Some((
            dotty_core::SymbolKind::TypeAlias,
            dotty_core::SymbolInfo::Missing
        ))
    );
    let error = unpickler
        .complete_symbol(378)
        .expect_err("the bounded classpath resolver does not complete the java.lang target");
    assert_eq!(
        unpickler.symbol_state_at(378),
        Some((
            dotty_core::SymbolKind::TypeAlias,
            dotty_core::SymbolInfo::Missing
        )),
        "failed target completion must leave the alias pending for a retry"
    );
    if with_jdk {
        assert!(matches!(
            &error,
            UnpickleError::UnresolvedMember { address: 383, .. }
        ));
    } else {
        assert!(matches!(
            &error,
            UnpickleError::UnresolvedPackage { address: 385, .. }
        ));
    }
    let error = normalize_error(&format!("{error:?}"));
    let trace = trace.lock().expect("trace lock").join(";");
    drop(unpickler);
    let foreign = format!("{wrong_owner:?}");
    (
        foreign,
        error,
        if trace.is_empty() {
            "-".to_owned()
        } else {
            trace
        },
    )
}

fn original_sound_probe(id: &str) -> String {
    let (relpath, owner_name, expected_method, expected_address, first_failure_address) = match id {
        "animal-sound" => (
            "crates/dotty-classloader/tests/fixtures/tasty_sample/me/cytrowski/tastyfixtures/Animal.tasty",
            "Animal",
            "sound",
            22,
            27,
        ),
        "dog-sound" => (
            "crates/dotty-classloader/tests/fixtures/tasty_sample/Dog.tasty",
            "Dog",
            "sound",
            36,
            41,
        ),
        _ => panic!("unsupported frozen sound probe {id}"),
    };
    let path = workspace().join(relpath);
    let bytes = fs::read(&path).expect("frozen sound fixture");
    let file = TastyFile::parse_and_validate_scala_3_9(&bytes).expect("sound TASTy");
    let address = member_address(&file, expected_method).expect("unique frozen sound method");
    assert_eq!(address, expected_address);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let trace = Arc::new(Mutex::new(Vec::new()));
    let resolver = TracingResolver {
        inner: Box::new(ClasspathSymbolResolver::new(
            tracing_classpath(Arc::clone(&trace)),
            definitions,
            LoadingSession::new(),
        )),
        trace: Arc::clone(&trace),
        fail_after_predef_string_lookup: Arc::new(AtomicBool::new(false)),
        rollback_count: Arc::new(AtomicU64::new(0)),
    };
    let session = TastySession::new()
        .with_resolver(Box::new(resolver))
        .expect("resolver-backed fresh sound session");
    let mut unpickler = TastyUnpickler::with_session(&file, &mut store, definitions, session);
    unpickler
        .enter_symbols()
        .expect("sound unit symbols entered");
    // These are the two acceptance calls for the original Scala-generated
    // method definitions, not class-root completion or scope membership.
    let owner_address = owner_address(&file, owner_name).expect("original owner TYPEDEF");
    let owner_id = unpickler
        .index()
        .symbol_at(owner_address)
        .expect("indexed original method owner");
    let method_owner = member_owner_address(&file, address).expect("method owner address");
    assert_eq!(method_owner, owner_address);
    let signature_id = unpickler
        .complete_symbol(address)
        .expect("original sound DEFDEF completes through resolved Predef.String");
    let method_id = unpickler
        .index()
        .symbol_at(address)
        .expect("original method identity");
    assert_eq!(
        unpickler
            .complete_symbol(address)
            .expect("same-session repeated completion reuses the method"),
        signature_id
    );
    assert_eq!(
        unpickler
            .complete_symbol(address)
            .expect("third completion remains idempotent"),
        signature_id
    );
    drop(unpickler);
    assert_eq!(store.symbols.get(method_id).owner, Some(owner_id));
    assert_eq!(
        store.symbols.get(method_id).kind,
        dotty_core::SymbolKind::Method
    );
    assert_eq!(store.symbols.get(method_id).visibility, Visibility::Public);
    let dotty_core::SymbolInfo::Complete(method_info) = store.symbols.get(method_id).info else {
        panic!("completed sound member has completed method information")
    };
    let signature = store.types.get(method_info);
    let signature = match signature {
        dotty_core::Type::ByName { result } => store.types.get(*result),
        signature => signature,
    };
    let dotty_core::Type::TypeRef {
        target: dotty_core::TypeRefTarget::Symbol(alias_id),
        ..
    } = signature
    else {
        panic!("sound method result is a symbol-backed TypeRef")
    };
    let alias = store.symbols.get(*alias_id);
    assert_eq!(alias.kind, dotty_core::SymbolKind::TypeAlias);
    assert_eq!(store.names.resolve(alias.name.text()), "String");
    assert_eq!(
        owner_path(&store, alias.owner.expect("alias module-class owner")),
        "scala.Predef$"
    );
    let dotty_core::SymbolInfo::Complete(alias_info) = alias.info else {
        panic!("canonical Predef.String alias should retain its completed target");
    };
    let dotty_core::Type::AliasingBounds { alias: target } = store.types.get(alias_info) else {
        panic!("Predef.String target should remain represented by AliasingBounds");
    };
    let dotty_core::Type::TypeRef {
        target: dotty_core::TypeRefTarget::Symbol(target_class),
        ..
    } = store.types.get(*target)
    else {
        panic!("Predef.String target should reference the canonical class identity");
    };
    assert_eq!(
        store
            .names
            .resolve(store.symbols.get(*target_class).name.text()),
        "String"
    );
    assert_eq!(
        owner_path(&store, store.symbols.get(*target_class).owner.unwrap()),
        "java.lang"
    );
    assert_ne!(*alias_id, *target_class);
    let alias_id = *alias_id;
    let observations = trace.lock().expect("trace lock").clone();
    let request = observations
        .iter()
        .find(|entry| entry.starts_with("request:"))
        .expect("canonical Predef request")
        .clone();
    assert!(request.contains("prefix=scala;name=Predef;namespace=Term"));
    assert!(
        observations
            .iter()
            .any(|entry| { entry == "scala/Predef=selected:Tasty:directory:scala3-library" })
    );
    let target_origin = observations
        .iter()
        .find(|entry| entry.starts_with("java/lang/String=selected:Class:jmod:"))
        .expect("java.lang.String must come from the real JDK jmod")
        .clone();
    assert!(target_origin.ends_with("java.base.jmod"));
    assert!(alias_id != method_id);
    format!(
        "777528f19a58e794c9954a42f433373472ec57f8\t{owner_name}.sound\t{relpath}\t{}\t{:?}\t{owner_address}\t{address}\tTerm\tMethod\tpublic\t{first_failure_address}\t{}\t{}.{}\t{}.{}\tjava.base.jmod\tCOMPLETE",
        sha256_file(&path),
        store.symbols.get(owner_id).kind,
        type_repr(&store, signature_id),
        owner_path(&store, alias.owner.unwrap()),
        store.names.resolve(alias.name.text()),
        owner_path(&store, store.symbols.get(*target_class).owner.unwrap()),
        store
            .names
            .resolve(store.symbols.get(*target_class).name.text())
    )
}

#[test]
fn missing_predef_and_jdk_string_resources_fail_typed_without_partial_method_state() {
    let path = workspace().join(
        "crates/dotty-classloader/tests/fixtures/tasty_sample/me/cytrowski/tastyfixtures/Animal.tasty",
    );
    let bytes = fs::read(path).expect("frozen Animal TASTy");
    let file = TastyFile::parse_and_validate_scala_3_9(&bytes).expect("Animal TASTy");
    let address = member_address(&file, "sound").expect("Animal.sound DEFDEF");
    assert_eq!(address, 22);
    let owner_address = owner_address(&file, "Animal").expect("Animal TYPEDEF");

    for (masked_internal_name, error_address, expected_error_text) in [
        ("scala/Predef", 29, "Predef"),
        ("java/lang/String", 27, "java/lang/String"),
    ] {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let classpath = CompositeClassPath::new(vec![Box::new(MaskedClassPath {
            inner: classpath(),
            masked_internal_name,
        })]);
        let session = TastySession::new()
            .with_resolver(Box::new(ClasspathSymbolResolver::new(
                classpath,
                definitions,
                LoadingSession::new(),
            )))
            .expect("resolver-backed masked-resource session");
        let mut unpickler = TastyUnpickler::with_session(&file, &mut store, definitions, session);
        unpickler.enter_symbols().expect("Animal symbols entered");
        let owner_id = unpickler
            .index()
            .symbol_at(owner_address)
            .expect("Animal owner identity");
        let method_id = unpickler
            .index()
            .symbol_at(address)
            .expect("Animal.sound identity");
        assert!(owner_id != method_id);
        let error = unpickler
            .complete_symbol(address)
            .expect_err("masked dependency must fail closed");
        assert!(
            matches!(
                &error,
                UnpickleError::ResolverFailure { address, .. }
                    | UnpickleError::UnresolvedMember { address, .. }
                    | UnpickleError::UnresolvedPackage { address, .. }
                    if *address == error_address
            ),
            "masked {masked_internal_name}: {error:?}"
        );
        assert!(format!("{error:?}").contains(expected_error_text));
        assert_eq!(
            unpickler.symbol_state_at(address),
            Some((
                dotty_core::SymbolKind::Method,
                dotty_core::SymbolInfo::Missing
            )),
            "failed external resolution leaves the original method incomplete"
        );
    }
}

fn predef_alias_audit_once() -> String {
    let root = workspace();
    let predef_path =
        root.join("crates/dotty-tasty/tests/fixtures/scala3-library/scala/Predef.tasty");
    let predef_bytes = fs::read(&predef_path).expect("pinned Predef fixture");
    assert_eq!(
        sha256_file(&predef_path),
        "2faef7420109c4e44074ab0385dce2c31e751fd992621fd1de312eed6c362fb2"
    );
    let predef = TastyFile::parse_and_validate_scala_3_9(&predef_bytes).expect("Predef TASTy");
    assert_eq!(
        (predef.header().major_version, predef.header().minor_version),
        (28, 9)
    );
    let index = predef
        .ast_address_index()
        .expect("Predef AST address index");
    let StructuredNode::TypeDef(DefinitionBody::TypeDef {
        name,
        type_or_template,
        tail,
    }) = index
        .get(378)
        .expect("String alias address")
        .decode_structured()
        .expect("String alias node")
    else {
        panic!("TASTy address 378 must be the String TYPEDEF")
    };
    assert_eq!(predef.names().get_utf8(name), Some("String"));
    assert!(
        tail.is_empty(),
        "the public source alias has no private/protected modifier"
    );
    let target = match type_or_template {
        dotty_tasty::tasty::RawTree::NatAst {
            tag: 113,
            value: 68,
            child,
            ..
        } => match *child {
            dotty_tasty::tasty::RawTree::NatAst {
                tag: 112,
                value: 47,
                child,
                ..
            } => match *child {
                dotty_tasty::tasty::RawTree::Leaf(term)
                    if matches!(term.value, dotty_tasty::tasty::TermValue::NameRef(46)) =>
                {
                    "java.lang.String"
                }
                other => panic!("unexpected alias target leaf: {other:?}"),
            },
            other => panic!("unexpected alias target prefix: {other:?}"),
        },
        other => panic!("unexpected alias target tree: {other:?}"),
    };
    assert_eq!(target, "java.lang.String");
    assert_eq!(predef.names().get_utf8(46), Some("java"));
    assert_eq!(predef.names().get_utf8(47), Some("lang"));

    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut unpickler =
        TastyUnpickler::with_session(&predef, &mut store, definitions, TastySession::new());
    unpickler.enter_symbols().expect("Predef symbols enter");
    let identities = [107, 128, 378].map(|address| {
        let id = unpickler
            .index()
            .symbol_at(address)
            .expect("frozen symbol address");
        (address, id)
    });
    drop(unpickler);
    let object = store.symbols.get(identities[0].1);
    let module_class = store.symbols.get(identities[1].1);
    let alias = store.symbols.get(identities[2].1);
    assert_eq!(object.kind, dotty_core::SymbolKind::Object);
    assert_eq!(store.names.resolve(object.name.text()), "Predef");
    assert_eq!(store.names.resolve(module_class.name.text()), "Predef$");
    assert_eq!(module_class.kind, dotty_core::SymbolKind::ModuleClass);
    assert_eq!(store.names.resolve(alias.name.text()), "String");
    assert_eq!(alias.kind, dotty_core::SymbolKind::TypeAlias);
    assert_eq!(alias.name.namespace(), dotty_core::Namespace::Type);
    assert_eq!(alias.owner, Some(identities[1].1));
    assert_eq!(alias.visibility, Visibility::Public);
    assert_eq!(object.name.namespace(), dotty_core::Namespace::Term);
    assert_ne!(identities[0].1, identities[1].1);
    assert_eq!(module_class.owner, object.owner);
    assert_eq!(
        object.owner,
        alias.owner.and_then(|id| store.symbols.get(id).owner)
    );
    assert_eq!(
        store
            .names
            .resolve(store.symbols.get(object.owner.unwrap()).name.text()),
        "scala"
    );
    assert_eq!(
        store
            .names
            .resolve(store.symbols.get(alias.owner.unwrap()).name.text()),
        "Predef$"
    );

    let old = run_once();
    let repeated = run_once();
    assert_eq!(
        old, repeated,
        "member completions are deterministic in fresh stores"
    );
    let animal_target_receipt = original_sound_probe("animal-sound");
    let dog_target_receipt = original_sound_probe("dog-sound");
    assert_eq!(
        animal_target_receipt,
        original_sound_probe("animal-sound"),
        "fresh Animal sessions should produce byte-identical normalized receipts"
    );
    assert_eq!(
        dog_target_receipt,
        original_sound_probe("dog-sound"),
        "fresh Dog sessions should produce byte-identical normalized receipts"
    );
    let sound_receipt = format!(
        "source_revision\tmethod\tfixture\tsha256\towner_kind\towner_address\tdefinition_address\tnamespace\tkind\tvisibility\tfirst_failure_address\tcanonical_method_type\tresolved_alias\talias_target\talias_target_origin\tcompletion\n{animal_target_receipt}\n{dog_target_receipt}\n"
    );
    assert_eq!(
        sound_receipt,
        include_str!(
            "../../../docs/classloader-semantic-tasty-convergence-iii/animal-dog-method-acceptance.tsv"
        )
    );
    predef_alias_probe(true);
    predef_alias_probe(false);
    include_str!("../../../docs/classloader-semantic-tasty-convergence-iii/predef-alias-audit.tsv")
        .to_owned()
}

#[test]
fn original_animal_and_dog_sound_methods_resolve_predef_string_target() {
    let animal = original_sound_probe("animal-sound");
    let dog = original_sound_probe("dog-sound");
    assert_ne!(animal, dog, "receipts retain their exact original owners");
    println!("{animal}\n{dog}");
}

fn sha256_file(path: &Path) -> String {
    let digest = Command::new("sha256sum")
        .arg(path)
        .output()
        .expect("sha256sum available");
    assert!(digest.status.success(), "{}", path.display());
    String::from_utf8(digest.stdout)
        .expect("sha256sum output is UTF-8")
        .split_whitespace()
        .next()
        .expect("sha256sum digest")
        .to_owned()
}

#[test]
fn predef_string_alias_provenance_and_member_failures_are_frozen_and_deterministic() {
    let first = predef_alias_audit_once();
    let second = predef_alias_audit_once();
    assert_eq!(
        first, second,
        "two independent fresh-store runs must be byte-identical"
    );
    assert_eq!(
        first,
        include_str!(
            "../../../docs/classloader-semantic-tasty-convergence-iii/predef-alias-audit.tsv"
        )
    );
}
