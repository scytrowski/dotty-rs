//! End-to-end source typing with the classloader's transactional resolver.

use dotty_classloader::classloader::{
    BinaryName, ClassLoader, ClassPathEntry, ClasspathSymbolResolver, CompositeClassPath,
    DirectoryClassPath, LoadingSession,
};
use dotty_core::ast::{TreeKind, Untyped};
use dotty_core::types::{TermRefTarget, Type, TypeRefTarget};
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

struct DemandClassPath {
    root: PathBuf,
    reads: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl ClassPathEntry for DemandClassPath {
    fn find_class(
        &self,
        name: &BinaryName,
    ) -> Result<
        Option<dotty_classloader::classloader::ClassResource>,
        dotty_classloader::classloader::ClassPathError,
    > {
        self.reads
            .lock()
            .unwrap()
            .push(name.as_internal().to_owned());
        DirectoryClassPath::new(self.root.clone()).find_class(name)
    }
    fn contains_package(
        &self,
        path: &[&str],
    ) -> Result<bool, dotty_classloader::classloader::ClassPathError> {
        DirectoryClassPath::new(self.root.clone()).contains_package(path)
    }
}

fn write_demand_class(root: &Path, name: &str, parent: Option<&str>, fields: &[(&str, &str)]) {
    use dotty_classfile::{
        access_flags::{ClassAccessFlags, FieldAccessFlags},
        class_file::{ClassFile, ClassFileVersion},
        constant_pool_builder::ConstantPoolBuilder,
        field::FieldInfo,
    };
    let mut pool = ConstantPoolBuilder::new();
    let this_class = pool.class(name).unwrap();
    let super_class = parent.map(|parent| pool.class(parent).unwrap());
    let fields = fields
        .iter()
        .map(|(name, descriptor)| FieldInfo {
            access_flags: FieldAccessFlags(1),
            name_index: pool.utf8(name).unwrap(),
            descriptor_index: pool.utf8(descriptor).unwrap(),
            attributes: Vec::new(),
        })
        .collect();
    let bytes = ClassFile {
        version: ClassFileVersion {
            major: 65,
            minor: 0,
        },
        constant_pool: pool.finish().unwrap(),
        access_flags: ClassAccessFlags(0x0021),
        this_class,
        super_class,
        fields,
        methods: Vec::new(),
        interfaces: Vec::new(),
        attributes: Vec::new(),
    }
    .encode()
    .unwrap();
    let path = root.join(format!("{name}.class"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

#[test]
fn source_member_selection_completes_shells_on_demand_and_rolls_back_in_place() {
    let files = TemporaryDirectory::new("demand-shells");
    write_demand_class(files.path(), "sample/A", None, &[("b", "Lsample/B;")]);
    write_demand_class(files.path(), "sample/B", None, &[("value", "I")]);
    write_demand_class(
        files.path(),
        "sample/Holder",
        None,
        &[("child", "Lsample/Child;")],
    );
    write_demand_class(files.path(), "sample/Child", Some("sample/Parent"), &[]);
    write_demand_class(files.path(), "sample/Parent", None, &[("inherited", "I")]);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let unit = parse_and_name(
        "import sample.A\nimport sample.Holder\nclass Own { val value: Int = 1 }\ndef source(x: Own): Int = x.value\ndef broken(a: A): Int = { val b = a.b; b.missing }\ndef read(a: A): Int = { val b = a.b; b.value }\ndef inherited(h: Holder): Int = { val c = h.child; c.inherited }",
        SourceId::from_index(10),
        &mut store,
        &mut packages,
    );
    let reads = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let path = DemandClassPath {
        root: files.path().to_owned(),
        reads: reads.clone(),
    };
    let mut loader = ClassLoader::with_definitions(
        path,
        &mut store,
        definitions,
        LoadingSession::with_packages(packages),
    );
    let a = loader
        .load_class(&BinaryName::from_internal("sample/A"))
        .unwrap();
    loader
        .load_class(&BinaryName::from_internal("sample/Holder"))
        .unwrap();
    let a_field = loader.metadata(a).expect("A classfile metadata").fields[0].symbol();
    let b = loader
        .ensure_class_identity(&BinaryName::from_internal("sample/B"))
        .unwrap();
    let child = loader
        .ensure_class_identity(&BinaryName::from_internal("sample/Child"))
        .unwrap();
    let (path, session) = loader.into_parts();
    let SymbolInfo::Complete(a_info) = store.symbols.get(a_field).info else {
        panic!("A.b should have a complete field signature");
    };
    assert_eq!(store.symbols.get(b).info, SymbolInfo::Missing);
    assert_eq!(store.symbols.get(child).info, SymbolInfo::Missing);
    assert_eq!(
        *reads.lock().unwrap(),
        ["sample/A", "sample/B", "sample/Holder", "sample/Child"]
    );
    let previous_b = store.symbols.get(b).clone();
    let value_name = Name::new(store.names.intern("value"), Namespace::Term);
    let log = Rc::new(RefCell::new(ResolutionLog::default()));
    let resolver = RecordingResolver::new(
        ClasspathSymbolResolver::new(path, definitions, session),
        log.clone(),
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
    let (method, rhs) = method_rhs(&unit, typer.store(), "source");
    let context = typer.expression_context_for(method).unwrap();
    typer.type_expression(rhs, context).unwrap();
    assert!(
        log.borrow().completions.is_empty(),
        "current source classes use source completion"
    );
    let (method, rhs) = method_rhs(&unit, typer.store(), "broken");
    let context = typer.expression_context_for(method).unwrap();
    let checkpoint = typer.store().checkpoint();
    let error = typer.type_expression(rhs, context).unwrap_err();
    assert!(
        matches!(error, TyperError::MemberNotFound { .. }),
        "{error:?}"
    );
    assert_eq!(typer.store().checkpoint(), checkpoint);
    assert_eq!(typer.store().symbols.get(b), &previous_b);
    assert_eq!(log.borrow().completions, [b]);
    let (method, rhs) = method_rhs(&unit, typer.store(), "read");
    let context = typer.expression_context_for(method).unwrap();
    typer.type_expression(rhs, context).unwrap();
    assert!(matches!(
        typer.store().symbols.get(b).info,
        SymbolInfo::Complete(_)
    ));
    let candidate = typer.lookup_members(a_info, value_name).unwrap()[0];
    assert_eq!(candidate.declaring_class, b);
    assert_eq!(
        typer.store().symbols.get(candidate.symbol).info,
        SymbolInfo::Complete(definitions.int)
    );
    assert_eq!(log.borrow().completions, [b, b]);
    let (method, rhs) = method_rhs(&unit, typer.store(), "inherited");
    let context = typer.expression_context_for(method).unwrap();
    typer.type_expression(rhs, context).unwrap();
    assert!(matches!(
        typer.store().symbols.get(child).info,
        SymbolInfo::Complete(_)
    ));
    assert_eq!(log.borrow().completions, [b, b, child]);
    assert_eq!(
        *reads.lock().unwrap(),
        [
            "sample/A",
            "sample/B",
            "sample/Holder",
            "sample/Child",
            "sample/B",
            "sample/B",
            "sample/Child",
            "sample/Parent"
        ]
    );
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
    completions: Vec<SymbolId>,
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
    fn complete_class(
        &mut self,
        store: &mut SemanticStore,
        symbol: SymbolId,
    ) -> Result<bool, ResolutionError> {
        let result = self.inner().complete_class(store, symbol)?;
        if result {
            self.log.borrow_mut().completions.push(symbol);
        }
        Ok(result)
    }

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

    let exchange_type = {
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
        let name = "exchange";
        let (method, rhs) = method_rhs(&unit, typer.store(), name);
        let context = typer.expression_context_for(method).unwrap();
        let typed_tree = typer.type_expression(rhs, context).unwrap_or_else(|error| {
            let resolutions = log.borrow().members.clone();
            panic!("typing {name} failed: {error:?}; resolutions: {resolutions:?}")
        });
        typer.typed_ast().get(typed_tree).ty
    };

    let mut log = log.borrow_mut();
    let ping = class_id(&log, "Ping");
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

    let exchange = exchange_type;
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
fn source_typer_resolves_a_member_inherited_from_a_real_scala_library_class() {
    let stubs = TemporaryDirectory::new("scala-library-inherited-member");
    write_required_jdk_stubs(stubs.path());
    write_stub_class(
        stubs.path(),
        "java/io/Closeable",
        Some("java/lang/Object"),
        0x0601,
    );
    let scala_library = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("crates/dotty-tasty/tests/fixtures/scala3-library");
    let classpath = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(scala_library)),
        Box::new(DirectoryClassPath::new(PathBuf::from(TASTY_SAMPLE))),
        Box::new(DirectoryClassPath::new(stubs.path().to_path_buf())),
    ]);

    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let unit = parse_and_name(
        "def isEmpty(source: scala.io.Source): Boolean = source.isEmpty",
        SourceId::from_index(11),
        &mut store,
        &mut packages,
    );
    let is_empty = Name::new(store.names.intern("isEmpty"), Namespace::Term);
    let resolver = ClasspathSymbolResolver::new(
        classpath,
        definitions,
        LoadingSession::with_packages(packages),
    );
    let log = Rc::new(RefCell::new(ResolutionLog::default()));
    let resolver = RecordingResolver::new(resolver, Rc::clone(&log));
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
    let (method, rhs) = method_rhs(&unit, typer.store(), "isEmpty");
    let context = typer.expression_context_for(method).unwrap();

    let typed_tree = typer
        .type_expression(rhs, context)
        .expect("Source's inherited isEmpty should type through normal member lookup");
    let bool_type = typer.typed_ast().get(typed_tree).ty;
    let Type::TermRef {
        target: TermRefTarget::Symbol(inherited_method),
        ..
    } = typer.store().types.get(bool_type)
    else {
        panic!("inherited isEmpty should resolve to its declaring method")
    };
    let inherited_method = *inherited_method;

    let source = log
        .borrow()
        .members
        .iter()
        .find_map(|(name, symbol)| (name == "Source").then_some(*symbol))
        .expect("the external Scala Source class should be materialized");
    let SymbolInfo::Complete(info) = typer.store().symbols.get(source).info else {
        panic!("the external Source class should be complete")
    };
    let Type::ClassInfo(info) = typer.store().types.get(info) else {
        panic!("the external Source class should have class info")
    };
    assert!(
        typer
            .store()
            .scopes
            .get(info.declarations)
            .lookup(&is_empty)
            .is_none(),
        "the resolved member comes from a parent rather than Source's direct declarations"
    );
    assert_eq!(
        typer.store().symbols.get(inherited_method).name,
        is_empty,
        "the typed selection should retain the inherited method identity"
    );
    assert_ne!(
        typer.store().symbols.get(inherited_method).owner,
        Some(source),
        "the inherited method remains owned by its declaring parent"
    );
}

#[test]
fn inferred_field_resolves_external_members_and_rolls_back_failed_classpath_completion() {
    let stubs = TemporaryDirectory::new("inferred-field-classpath-stubs");
    write_required_jdk_stubs(stubs.path());
    let source = SourceId::from_index(20);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let unit = parse_and_name(
        "class C(ping: external.Ping) { val broken = ping.missing; val pong = ping.other }",
        source,
        &mut store,
        &mut packages,
    );
    let field = |name: &str, store: &SemanticStore| {
        unit.parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::ValDef(definition) = &node.kind else {
                    return None;
                };
                (store.names.resolve(definition.name.as_name().text()) == name)
                    .then(|| (unit.index.symbol_at(source, tree).unwrap(), definition.tpt))
            })
            .unwrap_or_else(|| panic!("source should define field {name}"))
    };
    let (broken, broken_tpt) = field("broken", &store);
    let (pong, pong_tpt) = field("pong", &store);
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
        source,
        &unit.index,
        &mut store,
        definitions,
        &typer_packages,
    )
    .with_resolver(Box::new(resolver));

    let checkpoint = typer.store().checkpoint();
    let typed_checkpoint = typer.typed_ast().checkpoint();
    let error = typer
        .complete_symbol(broken)
        .expect_err("missing members in an inferred field initializer should fail");
    assert!(matches!(error, TyperError::MemberNotFound { .. }));
    assert_eq!(typer.store().checkpoint(), checkpoint);
    assert_eq!(typer.store().symbols.info(broken), &SymbolInfo::Missing);
    assert_eq!(typer.source_type_index().type_at(source, broken_tpt), None);
    assert_eq!(typer.typed_ast().checkpoint(), typed_checkpoint);

    let failed_ping = log
        .borrow()
        .members
        .iter()
        .find_map(|(name, symbol)| (name == "Ping").then_some(*symbol))
        .expect("the failed field probe should load external.Ping");
    assert!(
        !typer.store().symbols.contains(failed_ping),
        "failed field completion must roll back the loaded class symbol"
    );

    let inferred = typer
        .complete_symbol(pong)
        .expect("a valid external member should be usable in an inferred field");
    assert_eq!(
        typer.source_type_index().type_at(source, pong_tpt),
        Some(inferred)
    );
    assert_eq!(
        typer.store().symbols.info(pong),
        &SymbolInfo::Complete(inferred)
    );
    assert!(matches!(
        typer.store().types.get(inferred),
        Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }
            if typer.store().names.resolve(typer.store().symbols.get(*symbol).name.text()) == "Pong"
    ));
    assert!(
        log.borrow()
            .members
            .iter()
            .filter(|(name, _)| name == "Ping")
            .any(|(_, symbol)| typer.store().symbols.contains(*symbol)),
        "the successful retry must materialize a live external class symbol"
    );
}

#[test]
fn local_import_qualifiers_resolve_through_the_classpath() {
    let stubs = TemporaryDirectory::new("local-import-jdk-stubs");
    write_required_jdk_stubs(stubs.path());

    let source = SourceId::from_index(0);
    let text = "def localImport(p: external.Ping): external.Ping = { import external.Ping; def local(value: Ping): Ping = value; local(p) }";
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let unit = parse_and_name(text, source, &mut store, &mut packages);
    let root = packages.symbol::<&str>(&[]).unwrap();
    let resolver = ClasspathSymbolResolver::new(
        class_path(&stubs),
        definitions,
        LoadingSession::with_packages(packages),
    );
    let log = Rc::new(RefCell::new(ResolutionLog::default()));
    let resolver = RecordingResolver::new(resolver, Rc::clone(&log));
    let (method, rhs) = method_rhs(&unit, &store, "localImport");

    let typed_type = {
        let typer_packages = Packages::new();
        let mut typer = SourceTyper::new(
            &unit.parsed.ast,
            source,
            &unit.index,
            &mut store,
            definitions,
            &typer_packages,
        )
        .with_resolver(Box::new(resolver));
        let context = typer.expression_context_for(method).unwrap();
        let typed = typer.type_expression(rhs, context).unwrap();
        typer.typed_ast().get(typed).ty
    };

    let external_ping = log
        .borrow()
        .members
        .iter()
        .find_map(|(name, symbol)| (name == "Ping").then_some(*symbol))
        .expect("local import should resolve external.Ping through the classpath");
    let external_package = store.symbols.get(external_ping).owner.unwrap();
    assert_eq!(store.symbols.get(external_package).owner, Some(root));
    assert_eq!(
        store
            .names
            .resolve(store.symbols.get(external_package).name.text()),
        "external"
    );
    assert!(matches!(
        store.types.get(typed_type),
        Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == external_ping
    ));
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
    let mut resolver = ClasspathSymbolResolver::new(
        class_path(&stubs),
        definitions,
        LoadingSession::with_packages(packages),
    );
    let external = resolver
        .resolve_package(&mut store, &["external"])
        .unwrap()
        .expect("the fixture has the external package");
    let external_prefix = store
        .types
        .alloc(Type::type_ref(definitions.no_prefix, external));
    let generic_name = Name::new(store.names.intern("GenericSample"), Namespace::Type);
    let generic_class = resolver
        .resolve_member(
            &mut store,
            &dotty_core::MemberRequest {
                prefix: external_prefix,
                name: generic_name,
                selector: dotty_core::MemberSelector::Unique,
                space: dotty_core::MemberSpace::Prefix,
            },
        )
        .unwrap()
        .expect("GenericSample should materialize before receiver adaptation");
    assert_eq!(store.symbols.get(generic_class).owner, Some(external));
    assert!(matches!(
        store.symbols.get(generic_class).info,
        SymbolInfo::Complete(info) if matches!(store.types.get(info), Type::ClassInfo(_))
    ));
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
    let root_package = packages.symbol::<&str>(&[]).unwrap();
    let root_scope = packages.scope_of(root_package).unwrap();
    let external_package_name = Name::new(store.names.intern("external"), Namespace::Term);
    let exchange_member_name = Name::new(store.names.intern("exchange"), Namespace::Term);
    let ping_type_name = Name::new(store.names.intern("Ping"), Namespace::Type);
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
    let store_checkpoint = typer.store().checkpoint();
    let typed_node_count = typer.typed_ast().iter().count();
    let typed_mappings = unit
        .parsed
        .ast
        .iter()
        .map(|(tree, _)| (tree, typer.source_typed_index().get(unit.source, tree)))
        .collect::<Vec<_>>();
    let source_type_mappings = unit
        .parsed
        .ast
        .iter()
        .map(|(tree, _)| (tree, typer.source_type_index().type_at(unit.source, tree)))
        .collect::<Vec<_>>();
    let error = typer
        .type_expression(broken_rhs, broken_context)
        .expect_err("a missing external member should fail typing");
    assert!(matches!(error, TyperError::MemberNotFound { .. }));
    assert_eq!(typer.store().checkpoint(), store_checkpoint);
    assert_eq!(typer.typed_ast().iter().count(), typed_node_count);
    assert_eq!(
        unit.parsed
            .ast
            .iter()
            .map(|(tree, _)| (tree, typer.source_typed_index().get(unit.source, tree)))
            .collect::<Vec<_>>(),
        typed_mappings,
        "a failed expression must not leave source-to-typed-tree mappings"
    );
    assert_eq!(
        unit.parsed
            .ast
            .iter()
            .map(|(tree, _)| (tree, typer.source_type_index().type_at(unit.source, tree)))
            .collect::<Vec<_>>(),
        source_type_mappings,
        "a failed expression must restore source type projections"
    );
    assert!(
        typer
            .store()
            .scopes
            .get(root_scope)
            .lookup_all(&external_package_name)
            .is_empty(),
        "the failed resolver transaction must remove its package link"
    );

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
    assert_eq!(
        typer.source_typed_index().get(unit.source, valid_rhs),
        Some(typed),
        "the successful retry must publish its typed source mapping"
    );
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
    let ping = log
        .borrow()
        .members
        .iter()
        .rev()
        .find_map(|(name, symbol)| {
            (name == "Ping" && typer.store().symbols.contains(*symbol)).then_some(*symbol)
        })
        .expect("retry should leave a live external Ping symbol");
    let ping_package = typer.store().symbols.get(ping).owner.unwrap();
    let ping_scope = match typer.store().symbols.get(ping).info {
        SymbolInfo::Complete(info) => match typer.store().types.get(info) {
            Type::ClassInfo(class_info) => class_info.declarations,
            other => panic!("Ping should have ClassInfo, found {other:?}"),
        },
        other => panic!("Ping should be complete after retry, found {other:?}"),
    };
    assert_eq!(typer.store().scopes.get(ping_scope).owner, Some(ping));
    assert!(
        typer
            .store()
            .scopes
            .get(ping_scope)
            .lookup(&exchange_member_name)
            .is_some(),
        "the retry should retain Ping's member declaration"
    );
    drop(typer);
    let session = log
        .borrow_mut()
        .session
        .take()
        .expect("resolver session should survive the successful retry");
    let shared_packages = session.into_packages();
    let external_scope = shared_packages
        .scope_of(ping_package)
        .expect("external package should remain after successful retry");
    assert_eq!(
        store.scopes.get(external_scope).lookup(&ping_type_name),
        Some(ping),
        "package and class declaration scopes should agree on the retry identity"
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
    let widget_address = tasty
        .ast_address_index()
        .unwrap()
        .iter_nodes()
        .find(|node| node.tag == dotty_tasty::tasty::TYPEDEF_TAG)
        .and_then(|node| u32::try_from(node.offset).ok())
        .expect("the Widget TASTy fixture has a top-level class definition");
    let mut packages = Packages::new();
    let java_lang = packages
        .enter(
            &mut store,
            dotty_core::SymbolOrigin::Synthetic,
            &["java", "lang"],
        )
        .last()
        .unwrap()
        .symbol;
    let object_name = Name::new(store.names.intern("Object"), Namespace::Type);
    store
        .scopes
        .get_mut(packages.scope_of(java_lang).unwrap())
        .enter(object_name, definitions.object_class);
    let (_index, packages) = {
        let mut unpickler =
            TastyUnpickler::with_packages(&tasty, &mut store, definitions, packages);
        unpickler.enter_symbols().unwrap();
        unpickler.complete_symbol(widget_address).unwrap();
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
