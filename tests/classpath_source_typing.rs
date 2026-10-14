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

fn scala_library_classpath(stubs: &TemporaryDirectory) -> CompositeClassPath {
    let scala_library = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("crates/dotty-tasty/tests/fixtures/scala3-library");
    CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(scala_library)),
        Box::new(DirectoryClassPath::new(PathBuf::from(TASTY_SAMPLE))),
        Box::new(DirectoryClassPath::new(stubs.path().to_path_buf())),
    ])
}

fn write_scala_source_jdk_stubs(stubs: &TemporaryDirectory) {
    write_required_jdk_stubs(stubs.path());
    write_stub_class(
        stubs.path(),
        "java/io/Closeable",
        Some("java/lang/Object"),
        0x0601,
    );
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

fn write_deep_both_tasty(root: &Path) {
    let package = root.join("me/cytrowski/tastyfixtures/semantic");
    fs::create_dir_all(&package).unwrap();
    fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("crates/dotty-tasty-unpickler/tests/fixtures/semantic/Both.tasty"),
        package.join("Both.tasty"),
    )
    .unwrap();
    write_stub_class(root, "java/lang/Object", None, 0x0021);
}

#[test]
fn source_member_selection_completes_shells_on_demand_and_rolls_back_in_place() {
    let files = TemporaryDirectory::new("demand-shells");
    write_demand_class(
        files.path(),
        "sample/A",
        None,
        &[("b", "Lsample/B;"), ("unused", "Lsample/Unused;")],
    );
    write_demand_class(files.path(), "sample/Unused", Some("sample/NeverRead"), &[]);
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
    let unused = loader
        .ensure_class_identity(&BinaryName::from_internal("sample/Unused"))
        .unwrap();
    let (path, session) = loader.into_parts();
    let SymbolInfo::Complete(a_info) = store.symbols.get(a_field).info else {
        panic!("A.b should have a complete field signature");
    };
    assert_eq!(store.symbols.get(b).info, SymbolInfo::Missing);
    assert_eq!(store.symbols.get(child).info, SymbolInfo::Missing);
    assert_eq!(
        *reads.lock().unwrap(),
        [
            "sample/A",
            "sample/B",
            "sample/Unused",
            "sample/Holder",
            "sample/Child"
        ]
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
    let before_reuse = reads.lock().unwrap().clone();
    let (method, rhs) = method_rhs(&unit, typer.store(), "read");
    let context = typer.expression_context_for(method).unwrap();
    typer.type_expression(rhs, context).unwrap();
    assert_eq!(
        typer.lookup_members(a_info, value_name).unwrap()[0].declaring_class,
        b
    );
    assert_eq!(typer.store().symbols.get(unused).info, SymbolInfo::Missing);
    assert_eq!(*reads.lock().unwrap(), before_reuse);
    assert_eq!(log.borrow().completions, [b, b, child]);
    assert!(
        !reads
            .lock()
            .unwrap()
            .iter()
            .any(|name| name == "sample/NeverRead")
    );
    assert_eq!(
        *reads.lock().unwrap(),
        [
            "sample/A",
            "sample/B",
            "sample/Unused",
            "sample/Holder",
            "sample/Child",
            "sample/B",
            "sample/B",
            "sample/Child",
            "sample/Parent"
        ]
    );
}

#[test]
fn direct_import_member_completion_participates_in_expression_rollback() {
    let files = TemporaryDirectory::new("direct-shell-import");
    write_demand_class(files.path(), "External", None, &[("value", "I")]);
    // A static final JVM field is a stable imported value in the existing typer model.
    let bytes = fs::read(files.path().join("External.class")).unwrap();
    let mut reader = dotty_classfile::reader::Reader::new(&bytes);
    let mut class = dotty_classfile::class_file::ClassFile::decode(&mut reader).unwrap();
    class.fields[0].access_flags = dotty_classfile::access_flags::FieldAccessFlags(0x0019);
    fs::write(files.path().join("External.class"), class.encode().unwrap()).unwrap();
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let unit = parse_and_name(
        "def broken(): Int = { import External.value; val x = value; missing }\ndef valid(): Int = { import External.value; value }",
        SourceId::from_index(11),
        &mut store,
        &mut packages,
    );
    let reads = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut loader = ClassLoader::with_definitions(
        DemandClassPath {
            root: files.path().to_owned(),
            reads: reads.clone(),
        },
        &mut store,
        definitions,
        LoadingSession::with_packages(packages),
    );
    let external = loader
        .ensure_class_identity(&BinaryName::from_internal("External"))
        .unwrap();
    let (path, session) = loader.into_parts();
    let previous = store.symbols.get(external).clone();
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
    let (method, rhs) = method_rhs(&unit, typer.store(), "broken");
    let context = typer.expression_context_for(method).unwrap();
    let checkpoint = typer.store().checkpoint();
    let typed_count = typer.typed_ast().iter().count();
    let mappings: Vec<_> = unit
        .parsed
        .ast
        .iter()
        .map(|(tree, _)| (tree, typer.source_typed_index().get(unit.source, tree)))
        .collect();
    let source_types: Vec<_> = unit
        .parsed
        .ast
        .iter()
        .map(|(tree, _)| (tree, typer.source_type_index().type_at(unit.source, tree)))
        .collect();
    let source_symbols = unit
        .parsed
        .ast
        .iter()
        .filter_map(|(tree, _)| unit.index.symbol_at(unit.source, tree))
        .map(|id| (id, typer.store().symbols.get(id).clone()))
        .collect::<Vec<_>>();
    let error = typer.type_expression(rhs, context).unwrap_err();
    assert!(
        matches!(error, TyperError::TermNameNotFound { .. }),
        "{error:?}"
    );
    assert_eq!(typer.store().checkpoint(), checkpoint);
    for (id, previous) in source_symbols {
        assert_eq!(typer.store().symbols.get(id), &previous);
    }

    assert_eq!(typer.store().symbols.get(external), &previous);
    assert_eq!(typer.typed_ast().iter().count(), typed_count);
    for (tree, before) in mappings {
        assert_eq!(typer.source_typed_index().get(unit.source, tree), before);
    }
    for (tree, before) in source_types {
        assert_eq!(typer.source_type_index().type_at(unit.source, tree), before);
    }
    let member = class_id(&log.borrow(), "value");
    assert!(!typer.store().symbols.contains(member));
    assert!(
        log.borrow().completions.is_empty(),
        "this path uses direct resolve_member, not the completion port"
    );
    let (method, rhs) = method_rhs(&unit, typer.store(), "valid");
    let context = typer.expression_context_for(method).unwrap();
    typer.type_expression(rhs, context).unwrap();
    let member = log
        .borrow()
        .members
        .iter()
        .rev()
        .find_map(|(name, id)| (name == "value").then_some(*id))
        .unwrap();
    assert_eq!(typer.store().symbols.get(member).owner, Some(external));
    assert_eq!(
        typer.store().symbols.get(member).info,
        SymbolInfo::Complete(definitions.int)
    );
    assert_eq!(*reads.lock().unwrap(), ["External", "External", "External"]);
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
    shell_dependencies: Vec<(String, SymbolId)>,
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
    fn module_class_of_object(
        &self,
        store: &SemanticStore,
        object: SymbolId,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        self.inner
            .as_ref()
            .unwrap()
            .module_class_of_object(store, object)
    }

    fn complete_module_class(
        &mut self,
        store: &mut SemanticStore,
        module_class: SymbolId,
    ) -> Result<bool, ResolutionError> {
        let result = self.inner().complete_module_class(store, module_class)?;
        if result {
            self.log.borrow_mut().completions.push(module_class);
        }
        Ok(result)
    }

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
        assert_eq!(self.inner().checkpoint(), checkpoint);
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
            if let SymbolInfo::Complete(info) = store.symbols.get(symbol).info
                && let Type::ClassInfo(info) = store.types.get(info)
            {
                let mut dependencies = store
                    .scopes
                    .get(info.declarations)
                    .entered_symbols()
                    .filter_map(|member| {
                        let SymbolInfo::Complete(ty) = store.symbols.get(member).info else {
                            return None;
                        };
                        let Type::TypeRef {
                            target: dotty_core::types::TypeRefTarget::Symbol(id),
                            ..
                        } = store.types.get(ty)
                        else {
                            return None;
                        };
                        (store.symbols.get(*id).info == SymbolInfo::Missing).then(|| {
                            (
                                store
                                    .names
                                    .resolve(store.symbols.get(*id).name.text())
                                    .to_owned(),
                                *id,
                            )
                        })
                    })
                    .collect::<Vec<_>>();
                dependencies.sort_by(|left, right| {
                    left.0
                        .cmp(&right.0)
                        .then(left.1.index().cmp(&right.1.index()))
                });
                dependencies.dedup();
                self.log
                    .borrow_mut()
                    .shell_dependencies
                    .extend(dependencies);
            }
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
fn source_import_resolves_a_class_through_descendant_only_packages() {
    let files = TemporaryDirectory::new("descendant-only-class-import");
    write_demand_class(files.path(), "a/b/c/Leaf", None, &[]);
    let classpath = DirectoryClassPath::new(files.path().to_path_buf());
    assert!(!classpath.contains_package(&["a"]).unwrap());
    assert!(!classpath.contains_package(&["a", "b"]).unwrap());
    assert!(classpath.contains_package_prefix(&["a"]).unwrap());
    assert!(classpath.contains_package_prefix(&["a", "b"]).unwrap());

    let source = SourceId::from_index(50);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let unit = parse_and_name(
        "package a { class Existing }\npackage client { import a.b.c.Leaf\ndef use(value: Leaf): Leaf = value }",
        source,
        &mut store,
        &mut packages,
    );
    let root = packages.symbol::<&str>(&[]).unwrap();
    let source_package_a = packages.symbol(&["a"]).unwrap();
    let existing_name = Name::new(store.names.intern("Existing"), Namespace::Type);
    let parameter_type = unit
        .parsed
        .ast
        .iter()
        .find_map(|(_, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == "use")
                .then(|| definition.value_param_clauses[0][0])
        })
        .and_then(|parameter| match &unit.parsed.ast.get(parameter).kind {
            TreeKind::ValDef(parameter) => Some(parameter.tpt),
            _ => None,
        })
        .unwrap();
    let log = Rc::new(RefCell::new(ResolutionLog::default()));
    let resolver = RecordingResolver::new(
        ClasspathSymbolResolver::new(
            classpath,
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
    let (method, rhs) = method_rhs(&unit, typer.store(), "use");
    let context = typer.expression_context_for(method).unwrap();
    let typed = typer
        .type_expression(rhs, context)
        .expect("the import qualifier should traverse the canonical package chain");
    let Type::TermRef {
        target: TermRefTarget::Symbol(parameter_symbol),
        ..
    } = typer.store().types.get(typer.typed_ast().get(typed).ty)
    else {
        panic!(
            "the typed expression should remain a reference to its parameter: {:?}",
            typer.store().types.get(typer.typed_ast().get(typed).ty)
        )
    };
    let parameter_symbol = *parameter_symbol;
    let leaf = class_id(&log.borrow(), "Leaf");
    assert_eq!(
        typer.store().symbols.get(parameter_symbol).kind,
        dotty_core::SymbolKind::Parameter
    );
    assert!(
        matches!(
            typer
                .store()
                .types
                .get(typer.source_type_index().type_at(source, parameter_type).unwrap()),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == leaf
        ),
        "parameter type={:?}, typed expression type={:?}, leaf={leaf:?}",
        typer.store().types.get(
            typer
                .source_type_index()
                .type_at(source, parameter_type)
                .unwrap()
        ),
        typer.store().types.get(typer.typed_ast().get(typed).ty)
    );
    assert_eq!(
        typer.store().symbols.get(leaf).kind,
        dotty_core::SymbolKind::Class
    );
    let package_c = typer.store().symbols.get(leaf).owner.unwrap();
    let package_b = typer.store().symbols.get(package_c).owner.unwrap();
    let package_a = typer.store().symbols.get(package_b).owner.unwrap();
    assert_eq!(typer.store().symbols.get(package_a).owner, Some(root));
    assert_eq!(package_a, source_package_a);
    assert!(matches!(
        typer.store().symbols.get(package_a).origin,
        dotty_core::SymbolOrigin::Source(_)
    ));
    assert_eq!(
        typer
            .store()
            .names
            .resolve(typer.store().symbols.get(package_a).name.text()),
        "a"
    );
    assert_eq!(
        typer
            .store()
            .names
            .resolve(typer.store().symbols.get(package_b).name.text()),
        "b"
    );
    assert_eq!(
        typer
            .store()
            .names
            .resolve(typer.store().symbols.get(package_c).name.text()),
        "c"
    );
    let package_a_scope = unit
        .index
        .scope_of(package_a)
        .expect("source package a should retain its declaration scope");
    assert_eq!(
        typer
            .store()
            .scopes
            .get(package_a_scope)
            .lookup_all(&existing_name)
            .len(),
        1,
        "materializing a.b.c must preserve source declarations in a"
    );
    let package_ids: Vec<_> = log
        .borrow()
        .packages
        .iter()
        .map(|(_, symbol)| *symbol)
        .collect();
    assert!(package_ids.contains(&package_b));
    assert!(package_ids.contains(&package_c));
    drop(typer);
    let session = log.borrow_mut().session.take().unwrap();
    let shared = session.into_packages();
    assert_eq!(shared.symbol(&["a"]), Some(package_a));
    assert_eq!(shared.symbol(&["a", "b"]), Some(package_b));
    assert_eq!(shared.symbol(&["a", "b", "c"]), Some(package_c));
}

#[test]
fn source_import_resolves_a_real_tasty_object_member_through_deep_packages() {
    let cases = [
        "import me.cytrowski.tastyfixtures.semantic.Both.make\ndef use = make",
        "import me.cytrowski.tastyfixtures.semantic.Both.{make => create}\ndef use = create",
        "import me.cytrowski.tastyfixtures.semantic.Both.*\ndef use = make",
        "def use = { import me.cytrowski.tastyfixtures.semantic.Both; Both.make }",
    ];

    for (index, source_text) in cases.into_iter().enumerate() {
        let files = TemporaryDirectory::new(&format!("deep-tasty-object-import-{index}"));
        write_deep_both_tasty(files.path());
        let classpath = DirectoryClassPath::new(files.path().to_path_buf());
        assert!(!classpath.contains_package(&["me"]).unwrap());
        assert!(!classpath.contains_package(&["me", "cytrowski"]).unwrap());
        assert!(
            classpath
                .contains_package_prefix(&["me", "cytrowski"])
                .unwrap()
        );

        let source = SourceId::from_index(60 + index as u32);
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let unit = parse_and_name(source_text, source, &mut store, &mut packages);
        let log = Rc::new(RefCell::new(ResolutionLog::default()));
        let resolver = RecordingResolver::new(
            ClasspathSymbolResolver::new(
                classpath,
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
        let (method, rhs) = method_rhs(&unit, typer.store(), "use");
        let context = typer.expression_context_for(method).unwrap();
        let typed = typer
            .type_expression(rhs, context)
            .unwrap_or_else(|error| panic!("deep Object import {index} failed: {error:?}"));
        let Type::TermRef {
            target: TermRefTarget::Symbol(member),
            ..
        } = typer.store().types.get(typer.typed_ast().get(typed).ty)
        else {
            panic!("deep Object import {index} should retain a stable member reference")
        };
        let member = *member;
        assert_eq!(
            typer
                .store()
                .names
                .resolve(typer.store().symbols.get(member).name.text()),
            "make"
        );
        let module_class = typer.store().symbols.get(member).owner.unwrap();
        assert_eq!(
            typer.store().symbols.get(module_class).kind,
            dotty_core::SymbolKind::ModuleClass
        );
        assert_eq!(
            typer
                .store()
                .names
                .resolve(typer.store().symbols.get(module_class).name.text()),
            "Both$"
        );
        let package_ids: Vec<_> = log
            .borrow()
            .packages
            .iter()
            .map(|(_, symbol)| *symbol)
            .collect();
        assert!(package_ids.len() >= 4, "every package segment is observed");
    }
}

#[test]
fn root_qualified_import_bypasses_a_local_name_that_shadows_a_package() {
    let cases = [
        (
            "def use: Int = { val me: Int = 1; import me.cytrowski.tastyfixtures.semantic.Both.make; 1 }",
            false,
        ),
        (
            "def use = { val me: Int = 1; import _root_.me.cytrowski.tastyfixtures.semantic.Both.make; make }",
            true,
        ),
    ];

    for (index, (source_text, succeeds)) in cases.into_iter().enumerate() {
        let files = TemporaryDirectory::new(&format!("root-qualified-import-{index}"));
        write_deep_both_tasty(files.path());
        let source = SourceId::from_index(70 + index as u32);
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let unit = parse_and_name(source_text, source, &mut store, &mut packages);
        let resolver = ClasspathSymbolResolver::new(
            DirectoryClassPath::new(files.path().to_path_buf()),
            definitions,
            LoadingSession::with_packages(packages),
        );
        let log = Rc::new(RefCell::new(ResolutionLog::default()));
        let resolver = RecordingResolver::new(resolver, Rc::clone(&log));
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
        let (method, rhs) = method_rhs(&unit, typer.store(), "use");
        let context = typer.expression_context_for(method).unwrap();
        let checkpoint = typer.store().checkpoint();
        let typed_checkpoint = typer.typed_ast().checkpoint();
        let result = typer.type_expression(rhs, context);
        if succeeds {
            result.expect("_root_ must reach the canonical classpath root");
        } else {
            assert!(matches!(
                result,
                Err(TyperError::ImportQualifierNotFound { .. })
            ));
            assert_eq!(typer.store().checkpoint(), checkpoint);
            assert_eq!(typer.typed_ast().checkpoint(), typed_checkpoint);
        }
        drop(typer);
        let packages = log.borrow_mut().session.take().unwrap().into_packages();
        if succeeds {
            let package_me = packages.symbol(&["me"]).unwrap();
            let semantic = packages
                .symbol(&["me", "cytrowski", "tastyfixtures", "semantic"])
                .unwrap();
            let scope = packages.scope_of(semantic).unwrap();
            let object_name = Name::new(store.names.intern("Both"), Namespace::Term);
            let class_name = Name::new(store.names.intern("Both"), Namespace::Type);
            let object = store.scopes.get(scope).lookup(&object_name).unwrap();
            let class = store.scopes.get(scope).lookup(&class_name).unwrap();
            assert_eq!(
                store.symbols.get(object).kind,
                dotty_core::SymbolKind::Object
            );
            assert_eq!(store.symbols.get(class).kind, dotty_core::SymbolKind::Class);
            assert_ne!(object, class);
            assert_eq!(
                store.symbols.get(package_me).kind,
                dotty_core::SymbolKind::Package
            );
            assert_eq!(packages.symbol(&["_root_"]), None);
        } else {
            assert_eq!(packages.symbol(&["me"]), None);
        }
    }
}

#[test]
fn failed_expression_rolls_back_a_deep_object_import_and_retry_is_canonical() {
    let files = TemporaryDirectory::new("deep-object-import-rollback");
    write_deep_both_tasty(files.path());
    let source = SourceId::from_index(80);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let unit = parse_and_name(
        "def broken: Int = { import me.cytrowski.tastyfixtures.semantic.Both.make; missing }\ndef valid = { import me.cytrowski.tastyfixtures.semantic.Both; Both.make }",
        source,
        &mut store,
        &mut packages,
    );
    let root_package = packages.symbol::<&str>(&[]).unwrap();
    let root_scope = packages.scope_of(root_package).unwrap();
    let me_name = Name::new(store.names.intern("me"), Namespace::Term);
    let log = Rc::new(RefCell::new(ResolutionLog::default()));
    let resolver = RecordingResolver::new(
        ClasspathSymbolResolver::new(
            DirectoryClassPath::new(files.path().to_path_buf()),
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

    let (broken_method, broken_rhs) = method_rhs(&unit, typer.store(), "broken");
    let broken_context = typer.expression_context_for(broken_method).unwrap();
    let store_checkpoint = typer.store().checkpoint();
    let typed_checkpoint = typer.typed_ast().checkpoint();
    let source_mappings: Vec<_> = unit
        .parsed
        .ast
        .iter()
        .map(|(tree, _)| {
            (
                tree,
                typer.source_typed_index().get(source, tree),
                typer.source_type_index().type_at(source, tree),
            )
        })
        .collect();
    let error = typer
        .type_expression(broken_rhs, broken_context)
        .expect_err("the expression should fail after materializing its Object import");
    assert!(
        matches!(error, TyperError::TermNameNotFound { .. }),
        "{error:?}"
    );
    assert_eq!(typer.store().checkpoint(), store_checkpoint);
    assert_eq!(typer.typed_ast().checkpoint(), typed_checkpoint);
    assert!(
        typer
            .store()
            .scopes
            .get(root_scope)
            .lookup_all(&me_name)
            .is_empty()
    );
    for (tree, typed, ty) in source_mappings {
        assert_eq!(typer.source_typed_index().get(source, tree), typed);
        assert_eq!(typer.source_type_index().type_at(source, tree), ty);
    }
    let stale_import_ids: Vec<_> = log
        .borrow()
        .members
        .iter()
        .filter_map(|(name, symbol)| (name == "Both" || name == "make").then_some(*symbol))
        .collect();
    assert!(!stale_import_ids.is_empty());
    assert!(
        stale_import_ids
            .iter()
            .all(|symbol| !typer.store().symbols.contains(*symbol))
    );

    let (valid_method, valid_rhs) = method_rhs(&unit, typer.store(), "valid");
    let valid_context = typer.expression_context_for(valid_method).unwrap();
    let typed = typer.type_expression(valid_rhs, valid_context).unwrap();
    assert_eq!(
        typer.source_typed_index().get(source, valid_rhs),
        Some(typed)
    );
    let live_both = log
        .borrow()
        .members
        .iter()
        .rev()
        .find_map(|(name, symbol)| {
            (name == "Both" && typer.store().symbols.contains(*symbol)).then_some(*symbol)
        })
        .expect("retry should publish one live canonical Object");
    assert_eq!(
        typer.store().symbols.get(live_both).kind,
        dotty_core::SymbolKind::Object
    );
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
fn source_typer_selects_a_method_through_a_validated_external_object() {
    let stubs = TemporaryDirectory::new("scala-source-object-jdk-stubs");
    write_scala_source_jdk_stubs(&stubs);
    let classpath = scala_library_classpath(&stubs);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let unit = parse_and_name(
        "def first: scala.io.BufferedSource = scala.io.Source.stdin; def again: scala.io.BufferedSource = scala.io.Source.stdin",
        SourceId::from_index(12),
        &mut store,
        &mut packages,
    );
    let resolver = ClasspathSymbolResolver::new(
        classpath,
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

    let mut selected_field = None;
    let mut repeated = None;
    for method_name in ["first", "again"] {
        let (method, rhs) = method_rhs(&unit, typer.store(), method_name);
        let context = typer.expression_context_for(method).unwrap();
        let typed = typer
            .type_expression(rhs, context)
            .expect("a validated external Object should provide its ModuleClass members");
        let Type::TermRef {
            target: TermRefTarget::Symbol(field),
            ..
        } = typer.store().types.get(typer.typed_ast().get(typed).ty)
        else {
            panic!("external object field selection should retain a term reference")
        };
        let field = *field;
        assert_eq!(
            typer.store().symbols.get(field).kind,
            dotty_core::SymbolKind::Method
        );
        let module_class = typer.store().symbols.get(field).owner.unwrap();
        assert_eq!(
            typer.store().symbols.get(module_class).kind,
            dotty_core::SymbolKind::ModuleClass
        );
        assert_eq!(
            selected_field.get_or_insert(field),
            &field,
            "repeated source references must reuse the canonical field symbol"
        );
        if method_name == "again" {
            repeated = Some((method, rhs, typed));
        }
    }
    let (method, rhs, typed) = repeated.unwrap();
    let context = typer.expression_context_for(method).unwrap();
    assert_eq!(
        typer.type_expression(rhs, context).unwrap(),
        typed,
        "successful repeated typing should preserve the published typed tree"
    );
}

#[test]
fn external_object_import_forms_resolve_the_same_module_member() {
    let cases = [
        (
            "import scala.io.Source\ndef use: scala.io.BufferedSource = Source.stdin",
            "Source",
        ),
        (
            "import scala.io.Source.stdin\ndef use: scala.io.BufferedSource = stdin",
            "stdin",
        ),
        (
            "import scala.io.Source.{stdin => input}\ndef use: scala.io.BufferedSource = input",
            "input",
        ),
        (
            "import scala.io.Source.*\ndef use: scala.io.BufferedSource = stdin",
            "stdin",
        ),
        (
            "def use: scala.io.BufferedSource = { import scala.io.Source; Source.stdin }",
            "Source",
        ),
    ];

    for (index, (source_text, _reference_name)) in cases.into_iter().enumerate() {
        let stubs = TemporaryDirectory::new(&format!("scala-source-object-import-{index}"));
        write_scala_source_jdk_stubs(&stubs);
        let classpath = scala_library_classpath(&stubs);
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let unit = parse_and_name(
            source_text,
            SourceId::from_index(30 + index as u32),
            &mut store,
            &mut packages,
        );
        let object_name = Name::new(store.names.intern("Source"), Namespace::Term);
        let class_name = Name::new(store.names.intern("Source"), Namespace::Type);
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
        let (method, rhs) = method_rhs(&unit, typer.store(), "use");
        let context = typer.expression_context_for(method).unwrap();
        let typed = typer
            .type_expression(rhs, context)
            .unwrap_or_else(|error| panic!("import case {index} failed: {error:?}"));
        let Type::TermRef {
            target: TermRefTarget::Symbol(member),
            ..
        } = typer.store().types.get(typer.typed_ast().get(typed).ty)
        else {
            panic!("import case {index} should preserve the selected member reference")
        };
        let member = *member;
        assert_eq!(
            typer
                .store()
                .names
                .resolve(typer.store().symbols.get(member).name.text()),
            "stdin",
            "all import forms should identify the same Scala library member"
        );
        assert_eq!(
            typer.store().symbols.get(member).kind,
            dotty_core::SymbolKind::Method
        );
        let module_class = typer.store().symbols.get(member).owner.unwrap();
        assert_eq!(
            typer.store().symbols.get(module_class).kind,
            dotty_core::SymbolKind::ModuleClass
        );
        let package = typer.store().symbols.get(module_class).owner.unwrap();
        drop(typer);
        let session = log
            .borrow_mut()
            .session
            .take()
            .expect("resolver session should survive typing");
        let package_scope = store
            .scopes
            .get(session.into_packages().scope_of(package).unwrap());
        let object = package_scope
            .lookup(&object_name)
            .expect("the package should retain the Scala Source object identity");
        let companion_class = package_scope
            .lookup(&class_name)
            .expect("the package should retain the distinct Scala Source class identity");
        assert_ne!(object, companion_class);
        assert_eq!(
            store.symbols.get(object).kind,
            dotty_core::SymbolKind::Object
        );
        assert_eq!(
            store.symbols.get(companion_class).kind,
            dotty_core::SymbolKind::Class
        );
    }
}

#[test]
fn local_declaration_keeps_precedence_over_an_external_object_wildcard_import() {
    let stubs = TemporaryDirectory::new("scala-source-local-shadowing");
    write_scala_source_jdk_stubs(&stubs);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let unit = parse_and_name(
        "def use: Int = { import scala.io.Source.*; val stdin: Int = 1; stdin }",
        SourceId::from_index(45),
        &mut store,
        &mut packages,
    );
    let resolver = ClasspathSymbolResolver::new(
        scala_library_classpath(&stubs),
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
    let (method, rhs) = method_rhs(&unit, typer.store(), "use");
    let context = typer.expression_context_for(method).unwrap();
    let typed = typer
        .type_expression(rhs, context)
        .expect("a local declaration should shadow the imported Object member");
    assert_eq!(
        typer
            .widen_expression_type(typer.typed_ast().get(typed).ty)
            .unwrap(),
        definitions.int
    );
    let use_tree = unit
        .parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::Ident(identifier)
                if typer.store().names.resolve(identifier.name.text()) == "stdin" =>
            {
                Some(tree)
            }
            _ => None,
        })
        .expect("source should reference the local stdin declaration");
    let typed_use = typer
        .source_typed_index()
        .get(unit.source, use_tree)
        .expect("the local reference should have a typed mapping");
    let Type::TermRef {
        target: TermRefTarget::Symbol(local),
        ..
    } = typer.store().types.get(typer.typed_ast().get(typed_use).ty)
    else {
        panic!("the local reference should remain a term reference")
    };
    let local = *local;
    assert_eq!(
        typer.store().symbols.get(local).kind,
        dotty_core::SymbolKind::Local
    );
    assert_eq!(typer.store().symbols.get(local).owner, Some(method));
}

#[test]
fn failed_external_object_selection_rolls_back_and_can_be_retried() {
    let stubs = TemporaryDirectory::new("scala-source-object-rollback");
    write_scala_source_jdk_stubs(&stubs);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let unit = parse_and_name(
        "def broken: scala.io.BufferedSource = scala.io.Source.missing\ndef valid: scala.io.BufferedSource = scala.io.Source.stdin",
        SourceId::from_index(46),
        &mut store,
        &mut packages,
    );
    let root_package = packages.symbol::<&str>(&[]).unwrap();
    let root_scope = packages.scope_of(root_package).unwrap();
    let scala_package_name = Name::new(store.names.intern("scala"), Namespace::Term);
    let log = Rc::new(RefCell::new(ResolutionLog::default()));
    let resolver = RecordingResolver::new(
        ClasspathSymbolResolver::new(
            scala_library_classpath(&stubs),
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
    let checkpoint = typer.store().checkpoint();
    let error = typer
        .type_expression(broken_rhs, broken_context)
        .expect_err("an absent external module member should fail typing");
    assert!(matches!(error, TyperError::MemberNotFound { .. }));
    assert_eq!(typer.store().checkpoint(), checkpoint);
    assert!(
        typer
            .store()
            .scopes
            .get(root_scope)
            .lookup_all(&scala_package_name)
            .is_empty(),
        "failed external module resolution must remove its package link"
    );
    let stale_module_ids = log
        .borrow()
        .members
        .iter()
        .filter_map(|(name, symbol)| (name == "Source").then_some(*symbol))
        .collect::<Vec<_>>();
    assert!(
        !stale_module_ids.is_empty(),
        "the failed attempt should load Source"
    );
    assert!(
        stale_module_ids
            .iter()
            .all(|symbol| !typer.store().symbols.contains(*symbol)),
        "failed attempt left live Source ids: {:?}",
        stale_module_ids
            .iter()
            .map(|symbol| (symbol, typer.store().symbols.get(*symbol).clone()))
            .collect::<Vec<_>>()
    );

    let (valid_method, valid_rhs) = method_rhs(&unit, typer.store(), "valid");
    let valid_context = typer.expression_context_for(valid_method).unwrap();
    let typed = typer.type_expression(valid_rhs, valid_context).unwrap();
    assert_eq!(
        typer.source_typed_index().get(unit.source, valid_rhs),
        Some(typed),
        "a successful retry should publish the typed source mapping"
    );
    let live_source = log
        .borrow()
        .members
        .iter()
        .rev()
        .find_map(|(name, symbol)| {
            (name == "Source" && typer.store().symbols.contains(*symbol)).then_some(*symbol)
        })
        .expect("retry should leave a live Scala Source object");
    assert_eq!(
        typer.store().symbols.get(live_source).kind,
        dotty_core::SymbolKind::Object
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
    let source_symbols = unit
        .parsed
        .ast
        .iter()
        .filter_map(|(tree, _)| unit.index.symbol_at(unit.source, tree))
        .map(|id| (id, typer.store().symbols.get(id).clone()))
        .collect::<Vec<_>>();
    let error = typer
        .type_expression(broken_rhs, broken_context)
        .expect_err("a missing external member should fail typing");
    assert!(matches!(error, TyperError::MemberNotFound { .. }));
    assert_eq!(typer.store().checkpoint(), store_checkpoint);
    for (id, previous) in source_symbols {
        assert_eq!(typer.store().symbols.get(id), &previous);
    }

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

    let pong_shell = log
        .borrow()
        .shell_dependencies
        .iter()
        .find_map(|(name, id)| (name == "Pong").then_some(*id))
        .expect("the failed expression must have created the Ping.other shell");
    assert!(!typer.store().symbols.contains(pong_shell));
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
