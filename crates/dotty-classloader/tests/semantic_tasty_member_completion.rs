//! Explicit, test-only completion audit for the four frozen Scala 3.9 methods
//! in issue #1995. This is deliberately separate from the frozen class-root
//! comparator and its 14-unit/126-field denominator.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use dotty_classloader::classloader::{
    BinaryName, ClassLoader, ClassPathEntry, ClasspathSymbolResolver, CompositeClassPath,
    DirectoryClassPath, JdkClassPath, LoadingSession,
};
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

fn validate_owner(requested: &str, actual: &str) -> Result<(), ForeignOwner> {
    if requested == actual {
        Ok(())
    } else {
        Err(ForeignOwner {
            requested: requested.to_owned(),
            actual: actual.to_owned(),
        })
    }
}

fn type_repr(store: &SemanticStore, id: dotty_core::TypeId) -> String {
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
            type_repr(store, *tycon),
            args.iter()
                .map(|arg| type_repr(store, *arg))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Type::Method(method) => format!(
            "({}){:?}:{}",
            method
                .params
                .iter()
                .map(|param| type_repr(store, param.ty))
                .collect::<Vec<_>>()
                .join(","),
            method.kind,
            type_repr(store, method.result)
        ),
        Type::Poly(poly) => format!(
            "[{}]=>{}",
            type_params_repr(store, &poly.params),
            type_repr(store, poly.result)
        ),
        Type::Bounds { low, high } => format!(
            "bounds({},{})",
            type_repr(store, *low),
            type_repr(store, *high)
        ),
        Type::AliasingBounds { alias } => format!("alias({})", type_repr(store, *alias)),
        Type::ByName { result } => format!("by-name({})", type_repr(store, *result)),
        Type::Flexible { underlying } => format!("flexible({})", type_repr(store, *underlying)),
        Type::And { left, right } => format!(
            "and({},{})",
            type_repr(store, *left),
            type_repr(store, *right)
        ),
        Type::Or { left, right } => format!(
            "or({},{})",
            type_repr(store, *left),
            type_repr(store, *right)
        ),
        Type::ThisType { class } => format!("this({})", owner_path(store, *class)),
        Type::SuperType { super_type, .. } => format!("super({})", type_repr(store, *super_type)),
        Type::JavaArray { element } => format!("Array[{}]", type_repr(store, *element)),
        Type::Repeated { element } => format!("Repeated[{}]", type_repr(store, *element)),
        Type::ParamRef { index, .. } => format!("ParamRef[{index}]"),
        Type::Constant(value) => format!("constant({value:?})"),
        Type::Wildcard { bounds } => format!("Wildcard({})", type_repr(store, *bounds)),
        _ => "unknown-semantic-type-shape".into(),
    }
}

fn type_params_repr(store: &SemanticStore, params: &[TypeParam]) -> String {
    params
        .iter()
        .map(|param| {
            format!(
                "{}:{:?}:{}",
                store.names.resolve(param.name.as_name().text()),
                param.declared_variance,
                type_repr(store, param.bounds)
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
        "member\tpath\tsha256\ttasty_version\towner\towner_kind\towner_address\taddress\tnamespace\tkind\tvisibility\tlegacy_outcome\tlegacy_signature\tcompletion\tcanonical_signature\tcomparison\terror\n",
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
        let resolver =
            ClasspathSymbolResolver::new(classpath(), definitions, LoadingSession::new());
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
        let (outcome, signature, error) = match unpickler.complete_symbol(address) {
            Ok(signature) => ("COMPLETE".to_owned(), Some(signature), String::new()),
            Err(error) => (
                classify(&error).to_owned(),
                None,
                normalize_error(&format!("{error:?}")),
            ),
        };
        drop(unpickler);
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
    let symbol = unpickler.index().symbol_at(address).unwrap();
    drop(unpickler);
    let owner = store.symbols.get(symbol).owner.unwrap();
    let actual_owner = owner_path(&store, owner);
    let foreign = "scala.Option";
    let committed = store.checkpoint();
    assert_eq!(
        validate_owner(foreign, &actual_owner),
        Err(ForeignOwner {
            requested: foreign.to_owned(),
            actual: actual_owner.clone()
        }),
        "same-named methods under a foreign owner fail with an explicit typed receipt"
    );
    assert_eq!(
        store.checkpoint(),
        committed,
        "foreign-owner rejection leaves committed state usable"
    );
    assert_eq!(
        owner_path(&store, owner),
        "me.cytrowski.tastyfixtures.semantic.InfoWrapped"
    );
}
