//! End-to-end source typing with the classloader's transactional resolver.

use dotty_classloader::classloader::{
    ClassPathEntry, ClasspathSymbolResolver, CompositeClassPath, DirectoryClassPath, LoadingSession,
};
use dotty_core::ast::{TreeKind, Untyped};
use dotty_core::types::{Type, TypeRefTarget};
use dotty_core::{
    Definitions, Name, Namespace, Packages, ResolutionError, ResolverCheckpoint, SemanticStore,
    SourceId, SourceText, SymbolId, SymbolInfo, SymbolResolver, TreeId,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;
use dotty_parser::{ParseResult, parse_compilation_unit};
use dotty_tasty::tasty::TastyFile;
use dotty_tasty_unpickler::tasty_unpickler::TastyUnpickler;
use dotty_typer::{MemberLookupError, SourceTyper, TyperError};
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

const TASTY_SAMPLE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/crates/dotty-classloader/tests/fixtures/tasty_sample"
);
const WIDGET_TASTY: &[u8] =
    include_bytes!("../crates/dotty-tasty-unpickler/tests/fixtures/semantic/Widget.tasty");

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "dotty-classpath-source-typing-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temporary classpath directory should be created");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_stub_class(root: &Path, internal_name: &str, super_name: Option<&str>, flags: u16) {
    let mut pool = Vec::new();
    let mut next = 1_u16;
    let name_utf8 = next;
    next += 1;
    pool.push(1); // CONSTANT_Utf8
    pool.extend_from_slice(&(internal_name.len() as u16).to_be_bytes());
    pool.extend_from_slice(internal_name.as_bytes());
    let name_class = next;
    next += 1;
    pool.push(7); // CONSTANT_Class
    pool.extend_from_slice(&name_utf8.to_be_bytes());

    let super_class = if let Some(super_name) = super_name {
        let super_utf8 = next;
        next += 1;
        pool.push(1); // CONSTANT_Utf8
        pool.extend_from_slice(&(super_name.len() as u16).to_be_bytes());
        pool.extend_from_slice(super_name.as_bytes());
        let super_class = next;
        next += 1;
        pool.push(7); // CONSTANT_Class
        pool.extend_from_slice(&super_utf8.to_be_bytes());
        super_class
    } else {
        0
    };

    let mut bytes = Vec::new();
    bytes.extend_from_slice(&[0xCA, 0xFE, 0xBA, 0xBE]);
    bytes.extend_from_slice(&[0, 0]); // minor version
    bytes.extend_from_slice(&[0, 0x45]); // Java 21 classfile version
    bytes.extend_from_slice(&next.to_be_bytes());
    bytes.extend_from_slice(&pool);
    bytes.extend_from_slice(&flags.to_be_bytes());
    bytes.extend_from_slice(&name_class.to_be_bytes());
    bytes.extend_from_slice(&super_class.to_be_bytes());
    bytes.extend_from_slice(&[0, 0]); // interfaces
    bytes.extend_from_slice(&[0, 0]); // fields
    bytes.extend_from_slice(&[0, 0]); // methods
    bytes.extend_from_slice(&[0, 0]); // attributes

    let path = root.join(format!("{internal_name}.class"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn class_path(stubs: &TemporaryDirectory) -> CompositeClassPath {
    CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(PathBuf::from(TASTY_SAMPLE))),
        Box::new(DirectoryClassPath::new(stubs.path().to_path_buf())),
    ])
}

fn replace_all(mut input: Vec<u8>, old: &[u8], new: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    while let Some(offset) = input.windows(old.len()).position(|window| window == old) {
        output.extend_from_slice(&input[..offset]);
        output.extend_from_slice(new);
        input.drain(..offset + old.len());
    }
    output.extend_from_slice(&input);
    output
}

fn remap_fixture_class(input: &[u8], replacements: &[(&str, &str)]) -> Vec<u8> {
    assert_eq!(&input[..4], &[0xCA, 0xFE, 0xBA, 0xBE]);
    let constant_pool_count = u16::from_be_bytes([input[8], input[9]]);
    let mut output = input[..10].to_vec();
    let mut offset = 10;
    let mut index = 1;
    while index < constant_pool_count {
        let tag = input[offset];
        output.push(tag);
        offset += 1;
        match tag {
            1 => {
                let length = u16::from_be_bytes([input[offset], input[offset + 1]]) as usize;
                offset += 2;
                let mut value = input[offset..offset + length].to_vec();
                offset += length;
                for (old, new) in replacements {
                    value = replace_all(value, old.as_bytes(), new.as_bytes());
                }
                output.extend_from_slice(&(value.len() as u16).to_be_bytes());
                output.extend_from_slice(&value);
            }
            3 | 4 | 9 | 10 | 11 | 12 | 17 | 18 => {
                output.extend_from_slice(&input[offset..offset + 4]);
                offset += 4;
            }
            5 | 6 => {
                output.extend_from_slice(&input[offset..offset + 8]);
                offset += 8;
                index += 1;
            }
            7 | 8 | 16 | 19 | 20 => {
                output.extend_from_slice(&input[offset..offset + 2]);
                offset += 2;
            }
            15 => {
                output.extend_from_slice(&input[offset..offset + 3]);
                offset += 3;
            }
            other => panic!("unsupported classfile constant pool tag {other}"),
        }
        index += 1;
    }
    output.extend_from_slice(&input[offset..]);
    output
}

fn install_external_fixture(root: &Path, fixture: &str, simple_name: &str) {
    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("crates/dotty-classloader/tests/fixtures")
        .join(fixture);
    let bytes = fs::read(fixture_path).expect("checked-in classfile fixture should exist");
    let remapped = remap_fixture_class(
        &bytes,
        &[
            ("Ping", "external/Ping"),
            ("Pong", "external/Pong"),
            ("GenericSample", "external/GenericSample"),
        ],
    );
    let path = root.join(format!("external/{simple_name}.class"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, remapped).unwrap();
}

fn write_required_jdk_stubs(root: &Path) {
    // The checked-in Java fixtures need only Object, List, and Comparable;
    // these loadable hierarchy/signature stubs keep this test independent of
    // a locally installed JDK.
    write_stub_class(root, "java/lang/Object", None, 0x0021);
    write_stub_class(root, "java/util/List", Some("java/lang/Object"), 0x0601);
    write_stub_class(
        root,
        "java/lang/Comparable",
        Some("java/lang/Object"),
        0x0601,
    );
    install_external_fixture(root, "ping_pong/Ping.class", "Ping");
    install_external_fixture(root, "ping_pong/Pong.class", "Pong");
    install_external_fixture(root, "generic_sample/GenericSample.class", "GenericSample");
}

struct NamedUnit {
    parsed: ParseResult,
    index: dotty_core::SourceSemanticIndex,
    source: SourceId,
}

fn parse_and_name(
    text: &str,
    source: SourceId,
    store: &mut SemanticStore,
    packages: &mut Packages,
) -> NamedUnit {
    let scanner = ContextualScanner::new(text).expect("source should scan");
    let parsed = parse_compilation_unit(
        SourceText::new(text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "ClasspathSourceTyping.scala",
        store,
        packages,
    )
    .expect("source should be named");
    NamedUnit {
        parsed,
        index,
        source,
    }
}

fn method_rhs(unit: &NamedUnit, store: &SemanticStore, name: &str) -> (SymbolId, TreeId<Untyped>) {
    unit.parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            if store.names.resolve(definition.name.as_name().text()) == name {
                Some((unit.index.symbol_at(unit.source, tree)?, definition.rhs?))
            } else {
                None
            }
        })
        .unwrap_or_else(|| panic!("source should define method {name}"))
}

#[derive(Default)]
struct ResolutionLog {
    packages: Vec<(Vec<String>, SymbolId)>,
    members: Vec<(String, SymbolId)>,
    session: Option<LoadingSession>,
}

struct RecordingResolver<E: ClassPathEntry> {
    inner: Option<ClasspathSymbolResolver<E>>,
    log: Rc<RefCell<ResolutionLog>>,
    ambiguous_member: Option<String>,
}

impl<E: ClassPathEntry> RecordingResolver<E> {
    fn new(resolver: ClasspathSymbolResolver<E>, log: Rc<RefCell<ResolutionLog>>) -> Self {
        Self {
            inner: Some(resolver),
            log,
            ambiguous_member: None,
        }
    }

    fn with_ambiguous_member(mut self, name: &str) -> Self {
        self.ambiguous_member = Some(name.to_owned());
        self
    }

    fn inner(&mut self) -> &mut ClasspathSymbolResolver<E> {
        self.inner.as_mut().expect("resolver is present until drop")
    }
}

impl<E: ClassPathEntry> SymbolResolver for RecordingResolver<E> {
    fn checkpoint(&self) -> ResolverCheckpoint {
        self.inner.as_ref().unwrap().checkpoint()
    }

    fn rollback_to(&mut self, store: &mut SemanticStore, checkpoint: ResolverCheckpoint) {
        self.inner().rollback_to(store, checkpoint);
    }

    fn resolve_member(
        &mut self,
        store: &mut SemanticStore,
        request: &dotty_core::MemberRequest,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        if self.ambiguous_member.as_deref() == Some(store.names.resolve(request.name.text())) {
            return Err(ResolutionError::Ambiguous { candidates: 2 });
        }
        let result = self.inner().resolve_member(store, request)?;
        if let Some(symbol) = result {
            self.log
                .borrow_mut()
                .members
                .push((store.names.resolve(request.name.text()).to_owned(), symbol));
        }
        Ok(result)
    }

    fn resolve_package(
        &mut self,
        store: &mut SemanticStore,
        path: &[&str],
    ) -> Result<Option<SymbolId>, ResolutionError> {
        let result = self.inner().resolve_package(store, path)?;
        if let Some(symbol) = result {
            self.log
                .borrow_mut()
                .packages
                .push((path.iter().map(|part| (*part).to_owned()).collect(), symbol));
        }
        Ok(result)
    }
}

impl<E: ClassPathEntry> Drop for RecordingResolver<E> {
    fn drop(&mut self) {
        if let Some(resolver) = self.inner.take() {
            self.log.borrow_mut().session = Some(resolver.into_session());
        }
    }
}

fn class_id(log: &ResolutionLog, name: &str) -> SymbolId {
    log.members
        .iter()
        .find_map(|(resolved, symbol)| (resolved == name).then_some(*symbol))
        .unwrap_or_else(|| panic!("class/member {name} should have been resolved"))
}

#[test]
fn source_typer_resolves_external_imports_and_types_member_applications() {
    let stubs = TemporaryDirectory::new("jdk-stubs");
    write_required_jdk_stubs(stubs.path());

    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let unit = parse_and_name(
        "import external.Ping\nimport external.Pong\ndef exchange(p: Ping): Pong = p.exchange(p.other)",
        SourceId::from_index(0),
        &mut store,
        &mut packages,
    );
    let root = packages.symbol::<&str>(&[]).unwrap();
    let classpath = class_path(&stubs);
    let resolver = ClasspathSymbolResolver::new(
        classpath,
        definitions,
        LoadingSession::with_packages(packages),
    );
    let log = Rc::new(RefCell::new(ResolutionLog::default()));
    let resolver = RecordingResolver::new(resolver, Rc::clone(&log));

    let typed_ids = {
        let typer_packages = Packages::new();
        let mut typer = SourceTyper::new(
            &unit.parsed.ast,
            unit.source,
            &unit.index,
            &mut store,
            definitions,
            &typer_packages,
        )
        .with_resolver(Box::new(resolver));
        let mut typed = Vec::new();
        for name in ["exchange"] {
            let (method, rhs) = method_rhs(&unit, typer.store(), name);
            let context = typer.expression_context_for(method).unwrap();
            let typed_tree = typer.type_expression(rhs, context).unwrap_or_else(|error| {
                let resolutions = log.borrow().members.clone();
                panic!("typing {name} failed: {error:?}; resolutions: {resolutions:?}")
            });
            let result_type = typer.typed_ast().get(typed_tree).ty;
            typed.push((name, result_type));
        }
        typed
    };

    let mut log = log.borrow_mut();
    let ping = class_id(&log, "Ping");
    let exchange_type = typed_ids
        .iter()
        .find(|(name, _)| *name == "exchange")
        .unwrap()
        .1;
    let Type::TypeRef {
        target: TypeRefTarget::Symbol(pong),
        ..
    } = store.types.get(exchange_type)
    else {
        panic!("the external exchange method should return a named Pong type")
    };
    let pong = *pong;
    let package = store.symbols.get(ping).owner.unwrap();
    assert_eq!(store.symbols.get(pong).owner, Some(package));
    assert_eq!(store.symbols.get(package).owner, Some(root));

    assert_eq!(
        store.symbols.get(package).kind,
        dotty_core::SymbolKind::Package
    );
    assert_eq!(
        store.names.resolve(store.symbols.get(package).name.text()),
        "external"
    );
    let package_resolutions: Vec<_> = log
        .packages
        .iter()
        .filter(|(path, _)| path.as_slice() == ["external"])
        .map(|(_, symbol)| *symbol)
        .collect();
    assert!(!package_resolutions.is_empty());
    assert!(package_resolutions.iter().all(|symbol| *symbol == package));

    let exchange = typed_ids
        .iter()
        .find(|(name, _)| *name == "exchange")
        .unwrap()
        .1;
    assert!(
        matches!(store.types.get(exchange), Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == pong)
    );

    let SymbolInfo::Complete(class_info_id) = store.symbols.get(ping).info else {
        panic!("external Ping should be completed in the shared store")
    };
    let Type::ClassInfo(class_info) = store.types.get(class_info_id) else {
        panic!("external Ping should carry class information")
    };
    let exchange_name = Name::new(store.names.intern("exchange"), Namespace::Term);
    let exchange_symbol = store
        .scopes
        .get(class_info.declarations)
        .lookup(&exchange_name)
        .expect("exchange should come from Ping's declaration scope");
    assert_eq!(store.symbols.get(exchange_symbol).owner, Some(ping));
    let session = log.session.take().expect("resolver session survives typer");
    let shared_packages = session.into_packages();
    assert_eq!(shared_packages.symbol(&["external"]), Some(package));
}

#[test]
fn generic_external_receiver_reports_the_current_signature_limitation() {
    let stubs = TemporaryDirectory::new("generic-jdk-stubs");
    write_required_jdk_stubs(stubs.path());
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let unit = parse_and_name(
        "import external.GenericSample\ndef genericFirst[A](sample: GenericSample[A]): A = sample.first()",
        SourceId::from_index(1),
        &mut store,
        &mut packages,
    );
    let resolver = ClasspathSymbolResolver::new(
        class_path(&stubs),
        definitions,
        LoadingSession::with_packages(packages),
    );
    let typer_packages = Packages::new();
    let mut typer = SourceTyper::new(
        &unit.parsed.ast,
        unit.source,
        &unit.index,
        &mut store,
        definitions,
        &typer_packages,
    )
    .with_resolver(Box::new(resolver));
    let (method, rhs) = method_rhs(&unit, typer.store(), "genericFirst");
    let context = typer.expression_context_for(method).unwrap();

    let error = typer
        .type_expression(rhs, context)
        .expect_err("external generic receiver adaptation remains explicitly deferred");
    assert!(matches!(
        error,
        TyperError::MemberLookup(error)
            if matches!(
                *error,
                MemberLookupError::ParentTypeAdaptation {
                    error: TyperError::ExternalGenericInstantiationDeferred { .. },
                    ..
                }
            )
    ));
}

#[test]
fn expression_failure_rolls_back_loaded_class_and_allows_a_later_resolution() {
    let stubs = TemporaryDirectory::new("rollback-jdk-stubs");
    write_required_jdk_stubs(stubs.path());
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let unit = parse_and_name(
        "import external.Ping\nimport external.Pong\ndef broken(p: Ping): Pong = p.missing\ndef valid(p: Ping): Pong = p.exchange(p.other)",
        SourceId::from_index(2),
        &mut store,
        &mut packages,
    );
    let log = Rc::new(RefCell::new(ResolutionLog::default()));
    let resolver = RecordingResolver::new(
        ClasspathSymbolResolver::new(
            class_path(&stubs),
            definitions,
            LoadingSession::with_packages(packages),
        ),
        Rc::clone(&log),
    );
    let typer_packages = Packages::new();
    let mut typer = SourceTyper::new(
        &unit.parsed.ast,
        unit.source,
        &unit.index,
        &mut store,
        definitions,
        &typer_packages,
    )
    .with_resolver(Box::new(resolver));

    let (broken_method, broken_rhs) = method_rhs(&unit, typer.store(), "broken");
    let broken_context = typer.expression_context_for(broken_method).unwrap();
    let error = typer
        .type_expression(broken_rhs, broken_context)
        .expect_err("a missing external member should fail typing");
    assert!(matches!(error, TyperError::MemberNotFound { .. }));

    let first_ping = log
        .borrow()
        .members
        .iter()
        .find_map(|(name, symbol)| (name == "Ping").then_some(*symbol))
        .expect("the failed expression should have loaded Ping first");
    assert!(
        !typer.store().symbols.contains(first_ping),
        "the failed expression must not leave the loaded class in the store"
    );

    let (valid_method, valid_rhs) = method_rhs(&unit, typer.store(), "valid");
    let valid_context = typer.expression_context_for(valid_method).unwrap();
    let typed = typer.type_expression(valid_rhs, valid_context).unwrap();
    assert!(
        typer
            .store()
            .types
            .contains(typer.typed_ast().get(typed).ty)
    );
    assert!(
        log.borrow()
            .members
            .iter()
            .filter(|(name, _)| name == "Ping")
            .all(|(_, symbol)| typer.store().symbols.contains(*symbol)),
        "the later successful resolution must not observe a stale cached SymbolId"
    );
}

#[test]
fn missing_external_classes_and_resolver_failures_reach_typer_errors() {
    let stubs = TemporaryDirectory::new("resolution-errors");
    write_required_jdk_stubs(stubs.path());
    let broken = stubs.path().join("broken/Broken.class");
    fs::create_dir_all(broken.parent().unwrap()).unwrap();
    fs::write(&broken, b"malformed class file").unwrap();

    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let unit = parse_and_name(
        "import external.DoesNotExist\ndef missing(value: DoesNotExist): Int = 1",
        SourceId::from_index(3),
        &mut store,
        &mut packages,
    );
    let resolver = ClasspathSymbolResolver::new(
        class_path(&stubs),
        definitions,
        LoadingSession::with_packages(packages),
    );
    let typer_packages = Packages::new();
    let mut typer = SourceTyper::new(
        &unit.parsed.ast,
        unit.source,
        &unit.index,
        &mut store,
        definitions,
        &typer_packages,
    )
    .with_resolver(Box::new(resolver));
    let method = method_rhs(&unit, typer.store(), "missing").0;
    let error = typer
        .complete_symbol(method)
        .expect_err("the missing external class should fail signature completion");
    assert!(matches!(error, TyperError::TypeNameNotFound { .. }));

    drop(typer);

    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let unit = parse_and_name(
        "import external.Ping\ndef ambiguous(value: Ping): Int = 1",
        SourceId::from_index(4),
        &mut store,
        &mut packages,
    );
    let resolver = RecordingResolver::new(
        ClasspathSymbolResolver::new(
            class_path(&stubs),
            definitions,
            LoadingSession::with_packages(packages),
        ),
        Rc::new(RefCell::new(ResolutionLog::default())),
    )
    .with_ambiguous_member("Ping");
    let typer_packages = Packages::new();
    let mut typer = SourceTyper::new(
        &unit.parsed.ast,
        unit.source,
        &unit.index,
        &mut store,
        definitions,
        &typer_packages,
    )
    .with_resolver(Box::new(resolver));
    let method = method_rhs(&unit, typer.store(), "ambiguous").0;
    let error = typer
        .complete_symbol(method)
        .expect_err("an ambiguous resolver answer should fail signature completion");
    assert!(matches!(
        error,
        TyperError::SymbolResolution {
            error: ResolutionError::Ambiguous { candidates: 2 },
            ..
        }
    ));

    drop(typer);

    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let unit = parse_and_name(
        "import broken.Broken\ndef malformed(value: Broken): Int = 1",
        SourceId::from_index(5),
        &mut store,
        &mut packages,
    );
    let resolver = ClasspathSymbolResolver::new(
        class_path(&stubs),
        definitions,
        LoadingSession::with_packages(packages),
    );
    let typer_packages = Packages::new();
    let mut typer = SourceTyper::new(
        &unit.parsed.ast,
        unit.source,
        &unit.index,
        &mut store,
        definitions,
        &typer_packages,
    )
    .with_resolver(Box::new(resolver));
    let method = method_rhs(&unit, typer.store(), "malformed").0;
    let error = typer
        .complete_symbol(method)
        .expect_err("a malformed classfile should fail signature completion");
    assert!(matches!(
        error,
        TyperError::SymbolResolution {
            error: ResolutionError::Malformed { .. },
            ..
        }
    ));
}

#[test]
fn classloader_resolver_reuses_symbols_entered_by_the_tasty_adapter() {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let tasty = TastyFile::parse_scala_3_9(WIDGET_TASTY).unwrap();
    let (_index, packages) = {
        let mut unpickler =
            TastyUnpickler::with_packages(&tasty, &mut store, definitions, Packages::new());
        unpickler.enter_symbols().unwrap();
        unpickler.into_parts()
    };
    let root = packages.symbol::<&str>(&[]).unwrap();
    let widget_name = Name::new(store.names.intern("Widget"), Namespace::Type);
    let widget = store
        .scopes
        .get(packages.scope_of(root).unwrap())
        .lookup(&widget_name)
        .expect("TASTy adapter entered Widget into the root package");
    let root_prefix = store.types.alloc(Type::ThisType { class: root });
    let request = dotty_core::MemberRequest {
        prefix: root_prefix,
        name: Name::new(store.names.intern("Widget"), Namespace::Type),
        selector: dotty_core::MemberSelector::Unique,
        space: dotty_core::MemberSpace::Prefix,
    };
    let stubs = TemporaryDirectory::new("tasty-handoff-stubs");
    write_required_jdk_stubs(stubs.path());
    let mut resolver = ClasspathSymbolResolver::new(
        class_path(&stubs),
        definitions,
        LoadingSession::with_packages(packages),
    );

    let resolved = resolver
        .resolve_member(&mut store, &request)
        .unwrap()
        .expect("the resolver should find the class entered by the TASTy adapter");

    assert_eq!(resolved, widget, "both adapters must retain one SymbolId");
    let packages = resolver.into_session().into_packages();
    assert_eq!(packages.symbol::<&str>(&[]), Some(root));
}
