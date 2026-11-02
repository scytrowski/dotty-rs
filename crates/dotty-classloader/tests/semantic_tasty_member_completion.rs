//! Explicit, test-only completion audit for the four frozen Scala 3.9 methods
//! in issue #1995. This is deliberately separate from the frozen class-root
//! comparator and its 14-unit/126-field denominator.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
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

struct TracingResolver {
    inner: Box<dyn SymbolResolver>,
    trace: Arc<Mutex<Vec<String>>>,
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
        self.inner.module_class_of_object(store, object)
    }

    fn complete_module_class(
        &mut self,
        store: &mut SemanticStore,
        module_class: SymbolId,
    ) -> Result<bool, ResolutionError> {
        self.inner.complete_module_class(store, module_class)
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
        self.inner.rollback_to(store, checkpoint)
    }

    fn rollback_to_with_packages(
        &mut self,
        store: &mut SemanticStore,
        checkpoint: ResolverCheckpoint,
        packages: &mut Packages,
    ) {
        self.inner
            .rollback_to_with_packages(store, checkpoint, packages)
    }

    fn resolve_member(
        &mut self,
        store: &mut SemanticStore,
        request: &MemberRequest,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        self.record_request(store, request);
        self.inner.resolve_member(store, request)
    }

    fn resolve_member_with_packages(
        &mut self,
        store: &mut SemanticStore,
        packages: &mut Packages,
        request: &MemberRequest,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        self.record_request(store, request);
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
    Completion(UnpickleError),
}

fn complete_audit_request(
    unpickler: &mut TastyUnpickler<'_, '_, '_>,
    address: u32,
    requested_owner: &str,
    actual_owner: &str,
) -> Result<dotty_core::TypeId, AuditRequestError> {
    if unpickler.index().symbol_at(address).is_none() {
        return Err(AuditRequestError::Completion(
            UnpickleError::MissingEnteredSymbol { address },
        ));
    }
    if requested_owner == actual_owner {
        unpickler
            .complete_symbol(address)
            .map_err(AuditRequestError::Completion)
    } else {
        Err(AuditRequestError::ForeignOwner(ForeignOwner {
            requested: requested_owner.to_owned(),
            actual: actual_owner.to_owned(),
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
            match complete_audit_request(&mut unpickler, address, owner_simple, owner_simple) {
                Ok(signature) => ("COMPLETE".to_owned(), Some(signature), String::new()),
                Err(AuditRequestError::Completion(error)) => (
                    classify(&error).to_owned(),
                    None,
                    normalize_error(&format!("{error:?}")),
                ),
                Err(AuditRequestError::ForeignOwner(error)) => {
                    panic!("unexpected owner mismatch: {error:?}")
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
            assert!(canonical_resolution_trace.contains("scala/Predef/String=missing"));
            assert!(canonical_resolution_trace.contains("scala/Predef$String=missing"));
            assert!(error.contains("UnresolvedMember"));
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
        first, RECEIPT,
        "the supplemental report matches current input and outcomes"
    );
    assert_eq!(
        first.lines().count(),
        5,
        "exactly the frozen four methods are attempted"
    );
    println!("{first}");
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
    let error = complete_audit_request(
        &mut dog_unpickler,
        dog_address,
        "me.cytrowski.tastyfixtures.Animal",
        "me.cytrowski.tastyfixtures.Dog",
    )
    .expect_err("a real Dog.sound address cannot be requested as Animal.sound");
    assert!(matches!(
        error,
        AuditRequestError::ForeignOwner(ForeignOwner { requested, actual })
            if requested == "me.cytrowski.tastyfixtures.Animal"
                && actual == "me.cytrowski.tastyfixtures.Dog"
    ));
    assert_eq!(
        dog_unpickler.symbol_state_at(dog_address).unwrap(),
        before,
        "foreign-owner rejection leaves the committed method state unchanged"
    );
    let retry = complete_audit_request(
        &mut dog_unpickler,
        dog_address,
        "me.cytrowski.tastyfixtures.Dog",
        "me.cytrowski.tastyfixtures.Dog",
    );
    assert!(matches!(
        retry,
        Err(AuditRequestError::Completion(
            UnpickleError::UnresolvedMember { .. }
        ))
    ));
}
