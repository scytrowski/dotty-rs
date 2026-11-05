//! End-to-end `.tasty`-backed loading through a real `ClassLoader` over a
//! real `DirectoryClassPath` (`docs/classloader.md` §9, Milestone 9),
//! using the real `scalac`-compiled fixtures under
//! `tests/fixtures/tasty_sample/` (copied from `dotty-tasty`'s own
//! fixture tree). Complements `directory_class_path.rs`, which only
//! tests `.tasty`-over-`.class` preference at the `ClassPathEntry`
//! level, and `tasty_symbol`'s own unit tests, which only test decoding
//! in isolation.

use dotty_classloader::classloader::{
    BinaryName, ClassFormat, ClassLoadError, ClassLoader, ClassOrigin, ClassPathEntry,
    ClassPathError, ClassResource, ClasspathSymbolResolver, CompositeClassPath, DirectoryClassPath,
    JdkClassPath, LoadingSession,
};
use dotty_core::{
    ClassInfo, Definitions, MemberRequest, Name, Namespace, Packages, ResolutionError,
    ResolverCheckpoint, SemanticStore, SymbolId, SymbolInfo, SymbolKind, SymbolResolver, Type,
    TypeId, Visibility,
};
use dotty_tasty::tasty::{
    DefinitionBody, EncodedSection, LambdaTptNode, NameTable, PackageNode, ParameterNode, RawName,
    RawNode, RawNodes, Reader, StandardSection, StructuredNode, StructuredTree, TastyFile,
    TastyFileBuilder, Writer,
};
use dotty_tasty_unpickler::tasty_unpickler::{
    TastySession, TastySessionError, TastyUnpickler, UnpickleError,
};
use std::cell::{Cell, RefCell};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

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

fn encoded_tree<'a>(bytes: &'a [u8]) -> dotty_tasty::tasty::RawTree<'a> {
    let mut reader = Reader::new(bytes);
    dotty_tasty::tasty::RawTree::decode(&mut reader).expect("encoded tree should decode")
}

fn encoded_raw_node<'a>(bytes: &'a [u8]) -> RawNode<'a> {
    match encoded_tree(bytes) {
        dotty_tasty::tasty::RawTree::LengthNode(node) => RawNode {
            tag: node.tag,
            offset: 0,
            payload: node.payload,
        },
        _ => panic!("encoded category-five node expected"),
    }
}

/// Wraps the authentic package alias RHS in a one-parameter type lambda and
/// re-encodes the enclosing bounded AST nodes and file sections.
fn parameterize_authentic_serializable_alias(bytes: &[u8]) -> Vec<u8> {
    let file = TastyFile::parse_scala_3_9(bytes).expect("authentic package fixture should parse");
    let roots = file.asts().expect("AST section should decode");
    assert_eq!(roots.len(), 1, "package fixture has one root package node");
    let StructuredNode::Package(mut root_package) = roots
        .get(0)
        .expect("package root exists")
        .decode_structured()
        .expect("package root should decode")
    else {
        panic!("package fixture root should be a package")
    };

    let nested_package_index = root_package
        .stats
        .iter()
        .position(|node| node.tag == dotty_tasty::tasty::PACKAGE_TAG)
        .expect("scala package node should be nested below root package");
    let StructuredNode::Package(mut package) = root_package
        .stats
        .get(nested_package_index)
        .expect("nested package node exists")
        .decode_structured()
        .expect("scala package node should decode")
    else {
        panic!("nested scala node should be a package")
    };
    let package_object_index = package
        .stats
        .iter()
        .position(|node| {
            let Ok(StructuredNode::TypeDef(DefinitionBody::TypeDef { name, .. })) =
                node.decode_structured()
            else {
                return false;
            };
            matches!(
                file.render_name(name).ok().as_deref(),
                Some("package" | "package$")
            )
        })
        .expect("rooted package object declaration exists");
    let StructuredNode::TypeDef(mut package_object) = package
        .stats
        .get(package_object_index)
        .expect("package object node exists")
        .decode_structured()
        .expect("package object should decode")
    else {
        panic!("package object should be a type definition")
    };
    let DefinitionBody::TypeDef {
        type_or_template, ..
    } = &mut package_object
    else {
        unreachable!("type definition already matched")
    };
    let StructuredTree::Length(StructuredNode::Template(mut template)) = type_or_template
        .decode_structured()
        .expect("package object body should decode as a template")
    else {
        panic!("package object body should be a template")
    };

    let alias_index = template
        .stats
        .iter()
        .position(|tree| {
            let Ok(StructuredTree::Length(StructuredNode::TypeDef(DefinitionBody::TypeDef {
                name,
                ..
            }))) = tree.decode_structured()
            else {
                return false;
            };
            file.render_name(name).ok().as_deref() == Some("Serializable")
        })
        .expect("direct Serializable alias exists in package object template");
    let StructuredTree::Length(StructuredNode::TypeDef(mut alias)) = template.stats[alias_index]
        .decode_structured()
        .expect("Serializable alias should decode")
    else {
        panic!("Serializable declaration should be a type definition")
    };
    let DefinitionBody::TypeDef {
        type_or_template: rhs,
        ..
    } = &mut alias
    else {
        unreachable!("type definition already matched")
    };
    assert!(matches!(
        rhs.decode_structured(),
        Ok(StructuredTree::Select(selection))
            if selection.tag == dotty_tasty::tasty::SELECTTPT_TAG
    ));

    let parameter = file
        .ast_address_index()
        .expect("AST addresses should index")
        .iter()
        .find(|node| node.tag == dotty_tasty::tasty::TYPEPARAM_TAG)
        .expect("authentic package fixture should contain a type parameter")
        .decode_parameter()
        .expect("authentic type parameter should decode");
    assert!(matches!(parameter, ParameterNode::TypeParam { .. }));

    let mut lambda_writer = Writer::new();
    LambdaTptNode {
        type_params: vec![parameter],
        body: rhs.clone(),
    }
    .encode(&mut lambda_writer)
    .expect("parameterized alias type lambda should encode");
    *rhs = encoded_tree(lambda_writer.as_slice());

    let mut alias_writer = Writer::new();
    alias
        .encode_self(&mut alias_writer)
        .expect("wrapped alias should encode");
    template.stats[alias_index] = encoded_tree(alias_writer.as_slice());
    let mut template_writer = Writer::new();
    template
        .encode(&mut template_writer)
        .expect("updated package template should encode");
    *type_or_template = encoded_tree(template_writer.as_slice());

    let mut package_object_writer = Writer::new();
    package_object
        .encode_self(&mut package_object_writer)
        .expect("updated package object should encode");
    let package_object_bytes = package_object_writer.into_inner();
    let mut package_stats = package.stats.iter().cloned().collect::<Vec<_>>();
    package_stats[package_object_index] = encoded_raw_node(&package_object_bytes);
    package.stats =
        RawNodes::from_entries(package_stats).expect("updated package stats should encode");

    let mut nested_package_writer = Writer::new();
    PackageNode {
        path: package.path,
        stats: package.stats,
    }
    .encode(&mut nested_package_writer)
    .expect("updated scala package should encode");
    let nested_package_bytes = nested_package_writer.into_inner();
    let mut root_stats = root_package.stats.iter().cloned().collect::<Vec<_>>();
    root_stats[nested_package_index] = encoded_raw_node(&nested_package_bytes);
    root_package.stats =
        RawNodes::from_entries(root_stats).expect("updated root package stats should encode");

    let mut root_writer = Writer::new();
    PackageNode {
        path: root_package.path,
        stats: root_package.stats,
    }
    .encode(&mut root_writer)
    .expect("updated root package should encode");
    let root_bytes = root_writer.into_inner();
    let roots = RawNodes::from_entries(vec![encoded_raw_node(&root_bytes)])
        .expect("updated AST roots should encode");
    let ast_section = file
        .section(StandardSection::Asts)
        .expect("AST section exists");
    let mut builder = TastyFileBuilder::new(file.header().clone(), file.names().clone());
    for section in file.sections().iter() {
        if section.name == ast_section.name {
            builder.push_section(
                EncodedSection::asts(section.name, &roots)
                    .expect("wrapped AST section should encode"),
            );
        } else {
            builder.push_section(EncodedSection::raw(section.name, section.payload.to_vec()));
        }
    }
    builder
        .encode()
        .expect("wrapped package fixture should encode")
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct OpenObjectRequestEvidence {
    name: String,
    selector: dotty_core::MemberSelector,
    space: dotty_core::MemberSpace,
    prefix_is_package_term_ref: bool,
    classifier_branch_called: bool,
    prefix_is_canonical_java_lang: bool,
    prefix_symbol_kind: Option<SymbolKind>,
    prefix_namespace: Option<Namespace>,
    prefix_package_identity: Option<SymbolId>,
    canonical_java_lang: Option<SymbolId>,
    type_ref_member_identity: Option<SymbolId>,
}

struct CapturingResolver<E: dotty_classloader::classloader::ClassPathEntry> {
    inner: ClasspathSymbolResolver<E>,
    requests: Rc<RefCell<Vec<OpenObjectRequestEvidence>>>,
    no_prefix: TypeId,
    object_class: SymbolId,
    rollback_restored: Option<Rc<Cell<bool>>>,
    fail_after_object: Option<Rc<Cell<bool>>>,
}

#[derive(Clone, Copy)]
enum ObjectResourceControl {
    Missing,
    Malformed,
}

struct ObjectControlledClassPath {
    inner: CompositeClassPath,
    control: ObjectResourceControl,
}

struct RecordingClassPath {
    inner: CompositeClassPath,
    object_origins: Arc<Mutex<Vec<ClassOrigin>>>,
}

impl ClassPathEntry for RecordingClassPath {
    fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
        let resource = self.inner.find_class(name)?;
        if name.as_internal() == "java/lang/Object"
            && let Some(resource) = &resource
        {
            self.object_origins
                .lock()
                .expect("resource origin recorder lock")
                .push(resource.origin().clone());
        }
        Ok(resource)
    }

    fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
        self.inner.contains_package(package)
    }

    fn contains_package_prefix(&self, package: &[&str]) -> Result<bool, ClassPathError> {
        self.inner.contains_package_prefix(package)
    }
}

impl ClassPathEntry for ObjectControlledClassPath {
    fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
        if name.as_internal() == "java/lang/Object" {
            return Ok(match self.control {
                ObjectResourceControl::Missing => None,
                ObjectResourceControl::Malformed => Some(ClassResource::new(
                    vec![0xca, 0xfe, 0xba, 0xbe],
                    ClassFormat::Class,
                    ClassOrigin::Directory(PathBuf::from("<malformed-java-lang-object>")),
                )),
            });
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

impl<E: dotty_classloader::classloader::ClassPathEntry> SymbolResolver for CapturingResolver<E> {
    fn has_package_root(&self) -> bool {
        self.inner.has_package_root()
    }

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
        if let Some(rollback_restored) = &self.rollback_restored {
            rollback_restored.set(
                packages.symbol(&["java", "lang"]).is_none()
                    && store.symbols.get(self.object_class).info == SymbolInfo::Missing
                    && store.symbols.get(self.object_class).origin
                        == dotty_core::SymbolOrigin::Builtin,
            );
        }
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
        let prefix_package = match store.types.get(request.prefix) {
            Type::TermRef {
                target: dotty_core::TermRefTarget::Symbol(symbol),
                ..
            } if store.symbols.contains(*symbol)
                && store.symbols.get(*symbol).kind == SymbolKind::Package =>
            {
                Some(*symbol)
            }
            _ => None,
        };
        let result = self
            .inner
            .resolve_member_with_packages(store, packages, request);
        let type_ref_member_identity = if result.as_ref().is_ok_and(Option::is_some)
            && request.name.text() == store.names.intern("Object")
        {
            let package = prefix_package.expect("captured Object request has package prefix");
            let prefix = store.types.alloc(Type::type_ref(self.no_prefix, package));
            self.inner.resolve_member_with_packages(
                store,
                packages,
                &MemberRequest {
                    prefix,
                    name: request.name,
                    selector: request.selector.clone(),
                    space: dotty_core::MemberSpace::Prefix,
                },
            )?
        } else {
            None
        };
        let profile = self.inner.take_resolver_profile();
        let canonical_java_lang = packages.symbol(&["java", "lang"]);
        let prefix_symbol_kind = prefix_package.map(|symbol| store.symbols.get(symbol).kind);
        let prefix_namespace =
            prefix_package.map(|symbol| store.symbols.get(symbol).name.namespace());
        self.requests.borrow_mut().push(OpenObjectRequestEvidence {
            name: store.names.resolve(request.name.text()).to_owned(),
            selector: request.selector.clone(),
            space: request.space.clone(),
            prefix_is_package_term_ref: prefix_package.is_some(),
            classifier_branch_called: profile.prefix_classification.calls > 0,
            prefix_is_canonical_java_lang: prefix_package == canonical_java_lang,
            prefix_symbol_kind,
            prefix_namespace,
            prefix_package_identity: prefix_package,
            canonical_java_lang,
            type_ref_member_identity,
        });
        if request.name.namespace() == Namespace::Type
            && request.name.text() == store.names.intern("Object")
            && self
                .fail_after_object
                .as_ref()
                .is_some_and(|fail| fail.replace(false))
        {
            return Err(ResolutionError::Malformed {
                reason: "injected failure after Object materialization".to_owned(),
            });
        }
        result
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
}

/// Pins the unchanged address-15 reference in Open.tasty through the actual
/// unpickler callback and verifies it selects a real JDK-backed Object.
#[test]
fn open_address_15_reaches_classpath_member_resolution_as_package_term_ref() {
    let path = tasty_visibility_dir();
    let bytes = fs::read(path.join("me/cytrowski/tastyfixtures/visibility/Open.tasty"))
        .expect("real Open.tasty fixture");
    let file = TastyFile::parse_scala_3_9(&bytes).expect("Open.tasty should parse");
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let requests = Rc::new(RefCell::new(Vec::new()));
    let object_origins = Arc::new(Mutex::new(Vec::new()));
    let java_home = PathBuf::from(std::env::var_os("JAVA_HOME").expect("JAVA_HOME"));
    let java_base_jmod = java_home.join("jmods/java.base.jmod");
    let resolver = CapturingResolver {
        inner: ClasspathSymbolResolver::new(
            RecordingClassPath {
                inner: CompositeClassPath::new(vec![
                    Box::new(DirectoryClassPath::new(path)),
                    Box::new(
                        JdkClassPath::new(java_home.join("jmods")).expect("genuine JDK jmods"),
                    ),
                ]),
                object_origins: Arc::clone(&object_origins),
            },
            definitions,
            LoadingSession::new(),
        )
        .with_resolver_profile_collection(),
        requests: Rc::clone(&requests),
        no_prefix: definitions.no_prefix,
        object_class: definitions.object_class,
        rollback_restored: None,
        fail_after_object: None,
    };
    let session = TastySession::new()
        .with_resolver(Box::new(resolver))
        .expect("resolver joins the active unpickler session");
    let mut unpickler = TastyUnpickler::with_session(&file, &mut store, definitions, session);
    unpickler.enter_symbols().expect("Open symbols enter");

    let object_type = unpickler
        .unpickle_type(15)
        .expect("package TermRef prefix resolves java.lang.Object");
    let object = store
        .types
        .get(object_type)
        .reference_symbol()
        .expect("Open address 15 is an Object type reference");
    assert_eq!(object, definitions.object_class);
    assert!(matches!(
        store.symbols.get(object).info,
        SymbolInfo::Complete(info) if matches!(store.types.get(info), Type::ClassInfo(_))
    ));
    assert!(matches!(
        store.symbols.get(object).origin,
        dotty_core::SymbolOrigin::Classfile(_)
    ));

    let requests = requests.borrow();
    assert_eq!(requests.len(), 1, "address 15 makes one member request");
    let evidence = &requests[0];
    assert_eq!(evidence.name, "Object");
    assert_eq!(evidence.selector, dotty_core::MemberSelector::Unique);
    assert!(matches!(evidence.space, dotty_core::MemberSpace::Prefix));
    assert!(evidence.prefix_is_package_term_ref);
    assert!(evidence.classifier_branch_called);
    assert!(evidence.prefix_is_canonical_java_lang);
    assert_eq!(evidence.prefix_symbol_kind, Some(SymbolKind::Package));
    assert_eq!(evidence.prefix_namespace, Some(Namespace::Term));
    assert_eq!(
        evidence.prefix_package_identity,
        evidence.canonical_java_lang
    );
    assert_eq!(evidence.type_ref_member_identity, Some(object));
    assert_eq!(
        store.symbols.get(object).owner,
        evidence.prefix_package_identity
    );
    assert!(
        object_origins
            .lock()
            .expect("resource origin recorder lock")
            .contains(&ClassOrigin::Jmod(java_base_jmod))
    );
}

#[test]
fn open_object_materialization_rolls_back_and_retries_in_the_same_session() {
    let path = tasty_visibility_dir();
    let bytes = fs::read(path.join("me/cytrowski/tastyfixtures/visibility/Open.tasty"))
        .expect("real Open.tasty fixture");
    let file = TastyFile::parse_scala_3_9(&bytes).expect("Open.tasty should parse");
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let requests = Rc::new(RefCell::new(Vec::new()));
    let fail_after_object = Rc::new(Cell::new(true));
    let rollback_restored = Rc::new(Cell::new(false));
    let class_path = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(path)),
        Box::new(
            JdkClassPath::new(
                PathBuf::from(std::env::var_os("JAVA_HOME").expect("JAVA_HOME")).join("jmods"),
            )
            .expect("genuine JDK jmods"),
        ),
    ]);
    let resolver = CapturingResolver {
        inner: ClasspathSymbolResolver::new(class_path, definitions, LoadingSession::new())
            .with_resolver_profile_collection(),
        requests: Rc::clone(&requests),
        no_prefix: definitions.no_prefix,
        object_class: definitions.object_class,
        rollback_restored: Some(Rc::clone(&rollback_restored)),
        fail_after_object: Some(Rc::clone(&fail_after_object)),
    };
    let session = TastySession::new()
        .with_resolver(Box::new(resolver))
        .expect("resolver joins the active unpickler session");
    let mut unpickler = TastyUnpickler::with_session(&file, &mut store, definitions, session);
    unpickler.enter_symbols().expect("Open symbols enter");

    let failure = unpickler
        .unpickle_type(15)
        .expect_err("injected late failure follows real Object loading");
    assert!(matches!(
        failure,
        UnpickleError::ResolverFailure {
            error: ResolutionError::Malformed { ref reason },
            ..
        } if reason == "injected failure after Object materialization"
    ));
    assert!(!fail_after_object.get());
    assert!(rollback_restored.get());

    let object_type = unpickler
        .unpickle_type(15)
        .expect("same-session retry materializes java.lang.Object");
    assert_eq!(unpickler.unpickle_type(15), Ok(object_type));
    let (_, session) = unpickler.into_session_parts();
    let object = store
        .types
        .get(object_type)
        .reference_symbol()
        .expect("Open address 15 resolves a class reference");
    assert_eq!(object, definitions.object_class);
    let java_lang = session
        .packages()
        .symbol(&["java", "lang"])
        .expect("retry commits the canonical java.lang package");
    assert_eq!(store.symbols.get(object).owner, Some(java_lang));
    assert!(matches!(
        store.symbols.get(object).info,
        SymbolInfo::Complete(info) if matches!(store.types.get(info), Type::ClassInfo(_))
    ));
    assert!(matches!(
        store.symbols.get(object).origin,
        dotty_core::SymbolOrigin::Classfile(_)
    ));
    let requests = requests.borrow();
    let retry = requests.last().expect("retry member request");
    assert!(retry.prefix_is_canonical_java_lang);
    assert_eq!(retry.type_ref_member_identity, Some(object));
}

#[test]
fn package_term_ref_fails_closed_for_missing_or_malformed_object_resources() {
    for control in [
        ObjectResourceControl::Missing,
        ObjectResourceControl::Malformed,
    ] {
        let jdk_jmods =
            PathBuf::from(std::env::var_os("JAVA_HOME").expect("JAVA_HOME")).join("jmods");
        let class_path = ObjectControlledClassPath {
            inner: CompositeClassPath::new(vec![
                Box::new(DirectoryClassPath::new(tasty_visibility_dir())),
                Box::new(JdkClassPath::new(jdk_jmods).expect("genuine JDK jmods")),
            ]),
            control,
        };
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver =
            ClasspathSymbolResolver::new(class_path, definitions, LoadingSession::new());
        let package = resolver
            .resolve_package(&mut store, &["java", "lang"])
            .expect("JDK package lookup is valid")
            .expect("the real JDK contains java.lang");
        let prefix = store.types.alloc(Type::TermRef {
            prefix: definitions.no_prefix,
            target: dotty_core::TermRefTarget::Symbol(package),
        });
        let request = MemberRequest {
            prefix,
            name: Name::new(store.names.intern("Object"), Namespace::Type),
            selector: dotty_core::MemberSelector::Unique,
            space: dotty_core::MemberSpace::Prefix,
        };
        let checkpoint = store.checkpoint();

        let result = resolver.resolve_member(&mut store, &request);
        match control {
            ObjectResourceControl::Missing => assert_eq!(result, Ok(None)),
            ObjectResourceControl::Malformed => assert!(matches!(
                result,
                Err(ResolutionError::Malformed { ref reason })
                    if reason.contains("java/lang/Object")
            )),
        }
        assert_eq!(store.checkpoint(), checkpoint);
        assert_eq!(
            store.symbols.get(definitions.object_class).info,
            SymbolInfo::Missing
        );
        assert_eq!(
            store.symbols.get(definitions.object_class).origin,
            dotty_core::SymbolOrigin::Builtin
        );
    }
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
    let cross_address = definition_address(&distinct, "cross");
    resumed
        .complete_symbol(cross_address)
        .expect("package TermRef dependencies resolve in the resumed session");
    let cross_state = resumed.symbol_state_at(cross_address);
    assert!(
        matches!(cross_state, Some((_, SymbolInfo::Complete(_)))),
        "cross completion state: {cross_state:?}"
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
fn loads_scala39_some_with_rooted_serializable_alias_from_real_jdk() {
    let java_home = PathBuf::from(std::env::var_os("JAVA_HOME").expect("JAVA_HOME"));
    let java_base = java_home.join("jmods/java.base.jmod");
    let serializable_resource = JdkClassPath::new(java_home.join("jmods"))
        .expect("genuine JDK jmods")
        .find_class(&BinaryName::from_internal("java/io/Serializable"))
        .expect("JDK lookup should succeed")
        .expect("real JDK must provide java.io.Serializable");
    assert_eq!(
        serializable_resource.origin(),
        &ClassOrigin::Jmod(java_base.clone())
    );
    let mut store = SemanticStore::new();
    let class_path = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(published_scala39_library_dir())),
        Box::new(JdkClassPath::new(java_home.join("jmods")).expect("genuine JDK jmods")),
    ]);
    let mut loader = ClassLoader::new(class_path, &mut store);

    let some = loader
        .load_class(&BinaryName::from_internal("scala/Some"))
        .expect("Some's rooted scala.Serializable parent should resolve via scala/package.tasty");
    assert_eq!(
        loader
            .load_class(&BinaryName::from_internal("scala/Some"))
            .expect("the same class should be cached in this session"),
        some
    );
    let serializable = loader
        .load_class(&BinaryName::from_internal("java/io/Serializable"))
        .expect("the alias target should have a canonical class identity");
    assert_eq!(
        loader.origin(serializable),
        Some(&ClassOrigin::Jmod(java_base.clone())),
        "the loaded Java interface should retain its JDK resource origin"
    );
    drop(loader);

    assert!(
        class_info(&store, some)
            .parents
            .iter()
            .any(|parent| { store.types.get(*parent).reference_symbol() == Some(serializable) })
    );
    let java_io = store
        .symbols
        .get(serializable)
        .owner
        .expect("java.io package");
    assert_eq!(
        store.names.resolve(store.symbols.get(java_io).name.text()),
        "io"
    );
    let java = store.symbols.get(java_io).owner.expect("java package");
    assert_eq!(
        store.names.resolve(store.symbols.get(java).name.text()),
        "java"
    );
    assert!(matches!(
        store.symbols.get(serializable).origin,
        dotty_core::SymbolOrigin::Classfile(_)
    ));

    let parent_names = |store: &SemanticStore, class: SymbolId| {
        class_info(store, class)
            .parents
            .iter()
            .map(|parent| {
                let symbol = store
                    .types
                    .get(*parent)
                    .reference_symbol()
                    .expect("class parent");
                store
                    .names
                    .resolve(store.symbols.get(symbol).name.text())
                    .to_owned()
            })
            .collect::<Vec<_>>()
    };
    let expected_parents = parent_names(&store, some);
    let mut fresh_store = SemanticStore::new();
    let fresh_path = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(published_scala39_library_dir())),
        Box::new(JdkClassPath::new(java_home.join("jmods")).expect("genuine JDK jmods")),
    ]);
    let mut fresh_loader = ClassLoader::new(fresh_path, &mut fresh_store);
    let fresh_some = fresh_loader
        .load_class(&BinaryName::from_internal("scala/Some"))
        .expect("a fresh session should resolve the same authentic library class");
    drop(fresh_loader);
    assert_eq!(parent_names(&fresh_store, fresh_some), expected_parents);
}

#[test]
fn rejects_term_selection_instead_of_following_serializable_type_alias() {
    const SOME_SERIALIZABLE_SELECTION_OFFSET: usize = 1013;
    let java_home = PathBuf::from(std::env::var_os("JAVA_HOME").expect("JAVA_HOME"));
    let original = fs::read(published_scala39_library_dir().join("scala/Some.tasty"))
        .expect("pinned scala/Some.tasty fixture");
    assert_eq!(
        original[SOME_SERIALIZABLE_SELECTION_OFFSET],
        dotty_tasty::tasty::SELECTTPT_TAG,
        "the pinned Serializable parent is a terminal type selection"
    );

    let mut term_selection = original;
    term_selection[SOME_SERIALIZABLE_SELECTION_OFFSET] = dotty_tasty::tasty::SELECT_TAG;
    let mutated_library = TemporaryDirectory::new("some-term-serializable-selection");
    let some_path = mutated_library.path().join("scala/Some.tasty");
    fs::create_dir_all(some_path.parent().expect("scala package directory"))
        .expect("mutated fixture directory should be created");
    fs::write(some_path, term_selection).expect("mutated authentic fixture should be written");

    let class_path = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(mutated_library.path().clone())),
        Box::new(DirectoryClassPath::new(published_scala39_library_dir())),
        Box::new(JdkClassPath::new(java_home.join("jmods")).expect("genuine JDK jmods")),
    ]);
    let mut store = SemanticStore::new();
    let mut loader = ClassLoader::new(class_path, &mut store);
    let error = loader
        .load_class(&BinaryName::from_internal("scala/Some"))
        .expect_err("term-namespace SELECT must not become the Java interface parent");
    assert!(
        matches!(error, ClassLoadError::InvalidTastyFile(ref name, _) if name.as_internal() == "scala/Some"),
        "the malformed neighboring parent shape should remain a typed TASTy failure: {error:?}"
    );
}

#[test]
fn rejects_term_selection_in_authentic_serializable_alias_rhs() {
    const PACKAGE_SERIALIZABLE_RHS_OFFSET: usize = 1906;
    let java_home = PathBuf::from(std::env::var_os("JAVA_HOME").expect("JAVA_HOME"));
    let original = fs::read(published_scala39_library_dir().join("scala/package.tasty"))
        .expect("pinned scala/package.tasty fixture");
    assert_eq!(
        original[PACKAGE_SERIALIZABLE_RHS_OFFSET],
        dotty_tasty::tasty::SELECTTPT_TAG,
        "the pinned Serializable alias RHS is a terminal type selection"
    );

    let mut term_selection = original;
    term_selection[PACKAGE_SERIALIZABLE_RHS_OFFSET] = dotty_tasty::tasty::SELECT_TAG;
    let mutated_library = TemporaryDirectory::new("package-term-serializable-alias-rhs");
    let package_path = mutated_library.path().join("scala/package.tasty");
    fs::create_dir_all(package_path.parent().expect("scala package directory"))
        .expect("mutated fixture directory should be created");
    fs::write(package_path, term_selection).expect("mutated authentic fixture should be written");

    let class_path = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(mutated_library.path().clone())),
        Box::new(DirectoryClassPath::new(published_scala39_library_dir())),
        Box::new(JdkClassPath::new(java_home.join("jmods")).expect("genuine JDK jmods")),
    ]);
    let mut store = SemanticStore::new();
    let mut loader = ClassLoader::new(class_path, &mut store);
    let error = loader
        .load_class(&BinaryName::from_internal("scala/Some"))
        .expect_err("term-namespace alias RHS must not resolve a Java interface parent");
    assert!(
        matches!(
            error,
            ClassLoadError::DependencyFailure { ref source, .. }
                if matches!(source.as_ref(), ClassLoadError::InvalidTastyFile(name, _) if name.as_internal() == "scala/Option")
        ),
        "the malformed package alias should remain a typed TASTy failure: {error:?}"
    );
}

#[test]
fn rejects_parameterized_wrapper_around_authentic_serializable_alias_rhs() {
    let java_home = PathBuf::from(std::env::var_os("JAVA_HOME").expect("JAVA_HOME"));
    let original = fs::read(published_scala39_library_dir().join("scala/package.tasty"))
        .expect("pinned scala/package.tasty fixture");
    let parameterized = parameterize_authentic_serializable_alias(&original);

    let mut positive_store = SemanticStore::new();
    let positive_path = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(published_scala39_library_dir())),
        Box::new(JdkClassPath::new(java_home.join("jmods")).expect("genuine JDK jmods")),
    ]);
    let mut positive_loader = ClassLoader::new(positive_path, &mut positive_store);
    positive_loader
        .load_class(&BinaryName::from_internal("scala/Some"))
        .expect("authentic parameterless alias and real JDK target should load");

    let mutated_library = TemporaryDirectory::new("parameterized-serializable-alias");
    let package_path = mutated_library.path().join("scala/package.tasty");
    fs::create_dir_all(package_path.parent().expect("scala package directory"))
        .expect("mutated fixture directory should be created");
    fs::write(package_path, parameterized).expect("wrapped authentic fixture should be written");

    let class_path = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(mutated_library.path().clone())),
        Box::new(DirectoryClassPath::new(published_scala39_library_dir())),
        Box::new(JdkClassPath::new(java_home.join("jmods")).expect("genuine JDK jmods")),
    ]);
    let mut store = SemanticStore::new();
    let mut loader = ClassLoader::new(class_path, &mut store);
    let error = loader
        .load_class(&BinaryName::from_internal("scala/Some"))
        .expect_err("a type-parameter wrapper must not be erased into the package alias");
    assert!(
        matches!(
            error,
            ClassLoadError::DependencyFailure { ref source, .. }
                if matches!(source.as_ref(), ClassLoadError::InvalidTastyFile(name, _) if name.as_internal() == "scala/Option")
        ),
        "the unsupported alias wrapper should fail as a typed TASTy error: {error:?}"
    );
}

struct HideSerializableOnce {
    inner: CompositeClassPath,
    hide: Arc<AtomicBool>,
}

impl ClassPathEntry for HideSerializableOnce {
    fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
        if name.as_internal() == "java/io/Serializable" && self.hide.swap(false, Ordering::SeqCst) {
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

#[test]
fn failed_serializable_alias_load_can_retry_after_classpath_evidence_appears() {
    let java_home = PathBuf::from(std::env::var_os("JAVA_HOME").expect("JAVA_HOME"));
    let hide = Arc::new(AtomicBool::new(true));
    let make_class_path = || HideSerializableOnce {
        inner: CompositeClassPath::new(vec![
            Box::new(DirectoryClassPath::new(published_scala39_library_dir())),
            Box::new(JdkClassPath::new(java_home.join("jmods")).expect("genuine JDK jmods")),
        ]),
        hide: Arc::clone(&hide),
    };
    let mut store = SemanticStore::new();
    {
        let mut loader = ClassLoader::new(make_class_path(), &mut store);
        assert!(
            loader
                .load_class(&BinaryName::from_internal("scala/Some"))
                .is_err(),
            "an alias target absent from the classpath must fail closed"
        );
    }

    let mut loader = ClassLoader::new(make_class_path(), &mut store);
    let some = loader
        .load_class(&BinaryName::from_internal("scala/Some"))
        .expect("the same session store should accept a valid retry");
    drop(loader);
    assert!(matches!(
        store.symbols.get(some).info,
        SymbolInfo::Complete(info) if matches!(store.types.get(info), Type::ClassInfo(_))
    ));
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

#[test]
fn loads_pinned_tuple2_with_authenticated_applied_constructor_parent() {
    let mut store = SemanticStore::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scala_library = root.join("../dotty-tasty/tests/fixtures/scala3-library");
    let jdk = JdkClassPath::new(
        PathBuf::from(std::env::var_os("JAVA_HOME").expect("JDK 21 JAVA_HOME is required"))
            .join("jmods"),
    )
    .expect("JDK 21 jmods are readable");
    let class_path = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(scala_library.clone())),
        Box::new(jdk),
    ]);
    let tuple2 = BinaryName::from_internal("scala/Tuple2");
    let mut loader = ClassLoader::new(class_path, &mut store);
    let class = loader
        .load_class(&tuple2)
        .expect("pinned Tuple2 and all four classpath parents should complete");
    assert_eq!(
        loader.load_class(&tuple2).unwrap(),
        class,
        "a repeated same-session load must preserve Tuple2 identity"
    );
    let tuple2_origin = loader.origin(class).cloned();
    let times_colon = loader
        .load_class(&BinaryName::from_internal("scala/$times$colon"))
        .unwrap();
    let times_colon_origin = loader.origin(times_colon).cloned();
    let product2 = loader
        .load_class(&BinaryName::from_internal("scala/Product2"))
        .unwrap();
    let product2_origin = loader.origin(product2).cloned();
    let definitions = loader.definitions();
    let session = loader.into_session();
    let mut second_loader = ClassLoader::with_definitions(
        CompositeClassPath::new(vec![
            Box::new(DirectoryClassPath::new(scala_library.clone())),
            Box::new(
                JdkClassPath::new(
                    PathBuf::from(
                        std::env::var_os("JAVA_HOME").expect("JDK 21 JAVA_HOME is required"),
                    )
                    .join("jmods"),
                )
                .expect("JDK 21 jmods are readable"),
            ),
        ]),
        &mut store,
        definitions,
        session,
    );
    assert_eq!(
        second_loader.load_class(&tuple2).unwrap(),
        class,
        "a loader handoff must reuse the same session identity"
    );
    drop(second_loader);

    let info = class_info(&store, class);
    assert_eq!(info.parents.len(), 4);
    let parent_symbols = info
        .parents
        .iter()
        .map(|parent| match store.types.get(*parent) {
            Type::Applied { tycon, .. } => store
                .types
                .get(*tycon)
                .reference_symbol()
                .expect("applied parent tycon should retain its class identity"),
            _ => parent_symbol(&store, *parent),
        })
        .collect::<Vec<_>>();
    let parent_names = parent_symbols
        .iter()
        .map(|parent| {
            store
                .names
                .resolve(store.symbols.get(*parent).name.text())
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        parent_names,
        ["$times$colon", "Product2", "Product", "Serializable"]
    );
    assert_eq!(
        tuple2_origin,
        Some(ClassOrigin::Directory(scala_library.clone()))
    );
    assert_eq!(
        times_colon_origin,
        Some(ClassOrigin::Directory(scala_library.clone())),
        "the encoded identity must resolve to the authentic Scala library resource"
    );
    assert_eq!(
        product2_origin,
        Some(ClassOrigin::Directory(scala_library.clone())),
        "the independent Product2 parent must keep its own classpath identity"
    );

    let Type::Applied { tycon, args } = store.types.get(info.parents[1]) else {
        panic!("Tuple2 parent 1 should retain its applied Product2 view");
    };
    assert_eq!(
        store.types.get(*tycon).reference_symbol(),
        Some(parent_symbols[1])
    );
    assert_eq!(args.len(), 2);
    let argument_symbols = args
        .iter()
        .map(|argument| store.types.get(*argument).reference_symbol())
        .collect::<Vec<_>>();
    assert!(argument_symbols.iter().all(Option::is_some));
    let argument_names = argument_symbols
        .iter()
        .map(|symbol| {
            store
                .names
                .resolve(store.symbols.get(symbol.unwrap()).name.text())
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(argument_names, ["T1", "T2"]);

    let mut fresh_store = SemanticStore::new();
    let fresh = ClassLoader::new(
        CompositeClassPath::new(vec![
            Box::new(DirectoryClassPath::new(scala_library)),
            Box::new(
                JdkClassPath::new(
                    PathBuf::from(
                        std::env::var_os("JAVA_HOME").expect("JDK 21 JAVA_HOME is required"),
                    )
                    .join("jmods"),
                )
                .expect("JDK 21 jmods are readable"),
            ),
        ]),
        &mut fresh_store,
    )
    .load_class(&tuple2)
    .expect("a fresh semantic session should load the same pinned Tuple2");
    let fresh_info = class_info(&fresh_store, fresh);
    let fresh_parent_names = fresh_info
        .parents
        .iter()
        .map(|parent| match fresh_store.types.get(*parent) {
            Type::Applied { tycon, .. } => fresh_store
                .types
                .get(*tycon)
                .reference_symbol()
                .expect("fresh applied parent tycon has a canonical identity"),
            _ => parent_symbol(&fresh_store, *parent),
        })
        .map(|parent| {
            fresh_store
                .names
                .resolve(fresh_store.symbols.get(parent).name.text())
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(fresh_parent_names, parent_names);
    let Type::Applied { args, .. } = fresh_store.types.get(fresh_info.parents[1]) else {
        panic!("fresh Tuple2 parent 1 should retain its applied Product2 view");
    };
    let fresh_argument_names = args
        .iter()
        .map(|argument| {
            let symbol = fresh_store
                .types
                .get(*argument)
                .reference_symbol()
                .expect("fresh Product2 argument should remain a type parameter reference");
            fresh_store
                .names
                .resolve(fresh_store.symbols.get(symbol).name.text())
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(fresh_argument_names, argument_names);
}

struct MaskedTimesColonClassPath {
    inner: CompositeClassPath,
}

impl ClassPathEntry for MaskedTimesColonClassPath {
    fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
        if name.as_internal() == "scala/$times$colon" {
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

struct MutatedTuple2ClassPath {
    inner: DirectoryClassPath,
    mutated_tuple2: Vec<u8>,
    mutated_times_colon: Option<Vec<u8>>,
}

impl ClassPathEntry for MutatedTuple2ClassPath {
    fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
        if name.as_internal() == "scala/Tuple2" {
            let Some(resource) = self.inner.find_class(name)? else {
                return Ok(None);
            };
            return Ok(Some(ClassResource::new(
                self.mutated_tuple2.clone(),
                ClassFormat::Tasty,
                resource.origin().clone(),
            )));
        }
        if name.as_internal() == "scala/$times$colon"
            && let Some(mutated_times_colon) = &self.mutated_times_colon
        {
            let Some(resource) = self.inner.find_class(name)? else {
                return Ok(None);
            };
            return Ok(Some(ClassResource::new(
                mutated_times_colon.clone(),
                ClassFormat::Tasty,
                resource.origin().clone(),
            )));
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

#[test]
fn term_reference_at_tuple2_constructor_target_fails_closed_through_classloader() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scala_library = root.join("../dotty-tasty/tests/fixtures/scala3-library");
    assert_mutated_tuple2_rejected(
        &scala_library,
        162,
        dotty_tasty::tasty::TYPEREF_TAG,
        dotty_tasty::tasty::TERMREF_TAG,
    );
}

#[test]
fn string_constant_at_tuple2_package_prefix_fails_closed_through_classloader() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scala_library = root.join("../dotty-tasty/tests/fixtures/scala3-library");
    assert_mutated_tuple2_rejected(
        &scala_library,
        27,
        dotty_tasty::tasty::TERMREFPKG_TAG,
        dotty_tasty::tasty::STRINGCONST_TAG,
    );
}

#[test]
fn signed_result_name_cannot_authenticate_tuple2_constructor_through_classloader() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scala_library = root.join("../dotty-tasty/tests/fixtures/scala3-library");
    let tuple2_path = scala_library.join("scala/Tuple2.tasty");
    let original = fs::read(&tuple2_path).expect("pinned Tuple2 fixture");
    let file = TastyFile::parse(&original).expect("pinned Tuple2 TASTy");
    let address_index = file.ast_address_index().expect("Tuple2 AST address index");
    assert_eq!(
        address_index
            .get_node(162)
            .expect("constructor type reference")
            .tag,
        dotty_tasty::tasty::TYPEREF_TAG
    );

    let mut names = file.names().entries().to_vec();
    let star_refs = names
        .iter()
        .enumerate()
        .filter_map(|(index, name)| {
            matches!(name, RawName::Utf8(value) if value == "*:").then_some(index)
        })
        .collect::<Vec<_>>();
    assert_eq!(star_refs.len(), 1, "fixture has one direct *: name entry");
    let star_ref = star_refs[0] as u32;
    let product2_ref = names
        .iter()
        .position(|name| matches!(name, RawName::Utf8(value) if value == "Product2"))
        .expect("Tuple2 parent uses Product2") as u32;
    let result_signature_ref = names.len() as u32;
    names.push(RawName::Utf8("*:".to_owned()));
    names[star_ref as usize] = RawName::Signed {
        original: product2_ref,
        result_signature: result_signature_ref,
        parameter_signatures: Vec::new(),
    };
    let names = NameTable::from_entries(names).expect("mutated name table is valid");
    let mut builder = TastyFileBuilder::new(file.header().clone(), names);
    for section in file.sections().iter() {
        builder.push_section(EncodedSection::raw(section.name, section.payload.to_vec()));
    }
    let mutated = builder.encode().expect("mutated Tuple2 fixture encodes");
    let mutated_file = TastyFile::parse(&mutated).expect("mutated Tuple2 TASTy parses");
    assert_eq!(
        mutated_file
            .section(StandardSection::Asts)
            .expect("Tuple2 AST section")
            .payload,
        file.section(StandardSection::Asts)
            .expect("original Tuple2 AST section")
            .payload,
        "the reproducer changes only name-table interpretation"
    );

    assert_tuple2_bytes_rejected(&scala_library, mutated);
}

#[test]
fn signed_package_name_cannot_authenticate_tuple2_target_through_classloader() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scala_library = root.join("../dotty-tasty/tests/fixtures/scala3-library");
    let target_path = scala_library.join("scala/$times$colon.tasty");
    let original = fs::read(&target_path).expect("pinned *: target fixture");
    let file = TastyFile::parse(&original).expect("pinned *: target TASTy");
    let mut names = file.names().entries().to_vec();
    let scala_refs = names
        .iter()
        .enumerate()
        .filter_map(|(index, name)| {
            matches!(name, RawName::Utf8(value) if value == "scala").then_some(index)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        scala_refs.len(),
        1,
        "fixture has one direct scala name entry"
    );
    let scala_ref = scala_refs[0] as u32;
    let foreign_ref = names.len() as u32;
    let result_signature_ref = foreign_ref + 1;
    names.push(RawName::Utf8("foreign".to_owned()));
    names.push(RawName::Utf8("scala".to_owned()));
    names[scala_ref as usize] = RawName::Signed {
        original: foreign_ref,
        result_signature: result_signature_ref,
        parameter_signatures: Vec::new(),
    };
    let names = NameTable::from_entries(names).expect("mutated target name table is valid");
    let mut builder = TastyFileBuilder::new(file.header().clone(), names);
    for section in file.sections().iter() {
        builder.push_section(EncodedSection::raw(section.name, section.payload.to_vec()));
    }
    let mutated = builder.encode().expect("mutated target fixture encodes");
    let mutated_file = TastyFile::parse(&mutated).expect("mutated target TASTy parses");
    assert_eq!(
        mutated_file
            .section(StandardSection::Asts)
            .expect("target AST section")
            .payload,
        file.section(StandardSection::Asts)
            .expect("original target AST section")
            .payload,
        "the reproducer changes only target name-table interpretation"
    );

    assert_tuple2_bytes_and_target_rejected(&scala_library, None, Some(mutated), "MissingTypeDef");
}

fn assert_mutated_tuple2_rejected(
    scala_library: &Path,
    address: u32,
    expected_tag: u8,
    replacement_tag: u8,
) {
    let tuple2_path = scala_library.join("scala/Tuple2.tasty");
    let mut mutated_tuple2 = fs::read(&tuple2_path).expect("pinned Tuple2 fixture");
    let target_tag_offset = {
        let file = TastyFile::parse(&mutated_tuple2).expect("pinned Tuple2 TASTy");
        let index = file.ast_address_index().unwrap();
        let target = index.get_node(address).unwrap_or_else(|| {
            panic!("the tuple constructor reference lives at AST address {address}")
        });
        assert_eq!(target.tag, expected_tag);
        file.section(StandardSection::Asts)
            .expect("Tuple2 has an AST section")
            .offset
            + target.offset
    };
    assert_eq!(mutated_tuple2[target_tag_offset], expected_tag);
    mutated_tuple2[target_tag_offset] = replacement_tag;
    let mutated_file = TastyFile::parse(&mutated_tuple2)
        .expect("the one-byte reference-tag mutation remains structurally parseable");
    assert_eq!(
        mutated_file
            .ast_address_index()
            .unwrap()
            .get_node(address)
            .unwrap()
            .tag,
        replacement_tag
    );

    assert_tuple2_bytes_rejected(scala_library, mutated_tuple2);
}

fn assert_tuple2_bytes_rejected(scala_library: &Path, mutated_tuple2: Vec<u8>) {
    assert_tuple2_bytes_and_target_rejected(
        scala_library,
        Some(mutated_tuple2),
        None,
        "UnresolvedSupertype",
    );
}

fn assert_tuple2_bytes_and_target_rejected(
    scala_library: &Path,
    mutated_tuple2: Option<Vec<u8>>,
    mutated_times_colon: Option<Vec<u8>>,
    expected_error: &str,
) {
    let jdk = JdkClassPath::new(
        PathBuf::from(std::env::var_os("JAVA_HOME").expect("JDK 21 JAVA_HOME")).join("jmods"),
    )
    .expect("JDK 21 jmods are readable");
    let class_path = CompositeClassPath::new(vec![
        Box::new(MutatedTuple2ClassPath {
            inner: DirectoryClassPath::new(scala_library),
            mutated_tuple2: mutated_tuple2.unwrap_or_else(|| {
                fs::read(scala_library.join("scala/Tuple2.tasty")).expect("pinned Tuple2 fixture")
            }),
            mutated_times_colon,
        }),
        Box::new(jdk),
    ]);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let checkpoint = store.checkpoint();
    let mut loader =
        ClassLoader::with_definitions(class_path, &mut store, definitions, LoadingSession::new());
    let error = loader
        .load_class(&BinaryName::from_internal("scala/Tuple2"))
        .expect_err(
            "a one-byte-mutated Tuple2 reference must not authenticate the nominal *: parent",
        );
    assert!(format!("{error:?}").contains(expected_error), "{error:?}");
    drop(loader);
    assert_eq!(
        store.checkpoint(),
        checkpoint,
        "rejecting the malformed parent must roll back the entire Tuple2 load"
    );
}

#[test]
fn missing_tuple2_constructor_target_rolls_back_and_same_session_retry_succeeds() {
    let mut store = SemanticStore::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scala_library = root.join("../dotty-tasty/tests/fixtures/scala3-library");
    let jdk = || {
        Box::new(
            JdkClassPath::new(
                PathBuf::from(std::env::var_os("JAVA_HOME").expect("JDK 21 JAVA_HOME"))
                    .join("jmods"),
            )
            .expect("JDK 21 jmods are readable"),
        ) as Box<dyn ClassPathEntry>
    };
    let scala =
        || Box::new(DirectoryClassPath::new(scala_library.clone())) as Box<dyn ClassPathEntry>;
    let masked = CompositeClassPath::new(vec![Box::new(MaskedTimesColonClassPath {
        inner: CompositeClassPath::new(vec![scala(), jdk()]),
    })]);
    let tuple2 = BinaryName::from_internal("scala/Tuple2");
    let definitions = Definitions::bootstrap(&mut store);
    let checkpoint = store.checkpoint();
    let mut first_loader =
        ClassLoader::with_definitions(masked, &mut store, definitions, LoadingSession::new());
    let error = first_loader
        .load_class(&tuple2)
        .expect_err("the authenticated encoded target must be required on the classpath");
    assert!(
        format!("{error:?}").contains("scala/$times$colon"),
        "missing target error should identify the encoded nominal dependency: {error:?}"
    );
    let session = first_loader.into_session();
    assert_eq!(
        store.checkpoint(),
        checkpoint,
        "a late missing-parent failure must roll back every allocated symbol/type/scope"
    );

    let mut retry_loader = ClassLoader::with_definitions(
        CompositeClassPath::new(vec![scala(), jdk()]),
        &mut store,
        definitions,
        session,
    );
    let class = retry_loader
        .load_class(&tuple2)
        .expect("the same session should retry once the authentic target is present");
    assert_eq!(
        retry_loader.load_class(&tuple2).unwrap(),
        class,
        "the successful retry should commit one stable Tuple2 identity"
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
