//! Frozen, test-local comparison of the legacy TASTy loader and canonical
//! semantic unpickler. The cohort is checked in before these adapters run.
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use dotty_classloader::classloader::{
    BinaryName, ClassLoader, ClasspathSymbolResolver, CompositeClassPath, DirectoryClassPath,
    JdkClassPath, LoadingSession,
};
use dotty_core::names::TypeName;
use dotty_core::types::{AnnotationArguments, PolyType, Type, TypeParam, Variance};
use dotty_core::{Definitions, SemanticStore, SymbolId, SymbolInfo};
use dotty_tasty::tasty::{DefinitionBody, StructuredNode, TastyFile};
use dotty_tasty_unpickler::tasty_unpickler::{TastySession, TastyUnpickler};

const COHORT: &str =
    include_str!("../../../docs/classloader-semantic-tasty-convergence/cohort.tsv");
const OUTCOMES: &[&str] = &[
    "MATCH",
    "SEMANTIC_DELTA",
    "LOADER_UNSUPPORTED",
    "UNPICKLER_UNSUPPORTED",
    "MALFORMED_OR_VERSION",
    "UNRESOLVED_EXTERNAL",
];
const FIELDS: &[&str] = &[
    "identity",
    "visibility",
    "flags",
    "hierarchy",
    "type_parameters",
    "parameter_clauses",
    "member_signatures",
    "companion",
    "annotations",
];

#[derive(Clone, Debug)]
struct Unit {
    id: String,
    lookup: &'static str,
    path: PathBuf,
}

#[derive(Clone, Debug)]
struct Observation {
    outcome: &'static str,
    value: String,
    error: String,
    provenance: String,
}

#[derive(Debug, PartialEq, Eq)]
struct RequestedOwnerMismatch {
    requested: String,
    actual: String,
}

fn validate_requested_owner(requested: &str, actual: &str) -> Result<(), RequestedOwnerMismatch> {
    if requested == actual {
        Ok(())
    } else {
        Err(RequestedOwnerMismatch {
            requested: requested.to_owned(),
            actual: actual.to_owned(),
        })
    }
}

fn units() -> Vec<Unit> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    let mut lines = COHORT.lines();
    let header = lines
        .next()
        .expect("cohort header")
        .split('\t')
        .collect::<Vec<_>>();
    let column = |name: &str| header.iter().position(|field| *field == name).expect(name);
    let (mut ids, mut paths) = (HashSet::new(), HashSet::new());
    let mut units = Vec::new();
    for (index, line) in lines.enumerate() {
        let cells = line.split('\t').collect::<Vec<_>>();
        assert_eq!(
            cells.len(),
            header.len(),
            "malformed cohort row {}",
            index + 2
        );
        let id = cells[column("unit_id")].to_owned();
        let relpath = PathBuf::from(cells[column("fixture_path")]);
        assert!(ids.insert(id.clone()), "duplicate cohort unit {id}");
        assert!(
            paths.insert(relpath.clone()),
            "duplicate fixture path {relpath:?}"
        );
        let path = root.join(&relpath);
        let sha256 = cells[column("sha256")].to_owned();
        assert_eq!(sha256.len(), 64, "malformed SHA-256 for {id}");
        assert!(sha256.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(path.is_file(), "missing frozen fixture {}", path.display());
        let digest = Command::new("sha256sum")
            .arg(&path)
            .output()
            .expect("sha256sum is required by the repository fixture tooling");
        assert!(
            digest.status.success(),
            "sha256sum failed for {}",
            path.display()
        );
        let actual = String::from_utf8(digest.stdout).expect("sha256sum is UTF-8");
        assert_eq!(
            actual.split_whitespace().next(),
            Some(sha256.as_str()),
            "frozen hash changed for {id}"
        );
        let bytes = fs::read(&path).expect("fixture bytes are readable");
        let file = TastyFile::parse(&bytes)
            .unwrap_or_else(|error| panic!("malformed frozen fixture {id}: {error:?}"));
        let header = file.header();
        assert_eq!(header.major_version, 28, "unexpected TASTy major for {id}");
        assert_eq!(
            format!("{}.{}", header.major_version, header.minor_version),
            cells[column("format_minor")],
            "frozen minor changed for {id}"
        );
        assert_eq!(
            header.experimental_version, 0,
            "experimental TASTy version is out of policy for {id}"
        );
        let lookup = match id.as_str() {
            "animal" => "me/cytrowski/tastyfixtures/Animal",
            "dog" => "Dog",
            "fields-body" => "me/cytrowski/tastyfixtures/fields/Body",
            "fields-p" => "me/cytrowski/tastyfixtures/fields/P",
            "fields-q" => "me/cytrowski/tastyfixtures/fields/Q",
            "visibility-holder" => "me/cytrowski/tastyfixtures/visibility/Holder",
            "visibility-open" => "me/cytrowski/tastyfixtures/visibility/Open",
            "visibility-private" => "me/cytrowski/tastyfixtures/visibility/PlainPrivate",
            "function1" => "scala/Function1",
            "bigint" => "scala/math/BigInt",
            "with-filter" => "scala/collection/WithFilter",
            "option" => "scala/Option",
            "some" => "scala/Some",
            "tuple2" => "scala/Tuple2",
            other => panic!("unknown frozen unit id {other}"),
        };
        units.push(Unit { id, lookup, path });
    }
    assert!(units.len() >= 12, "frozen cohort has fewer than 12 units");
    units
}

fn classpath() -> CompositeClassPath {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut entries: Vec<Box<dyn dotty_classloader::classloader::ClassPathEntry>> = vec![
        Box::new(DirectoryClassPath::new(
            root.join("tests/fixtures/tasty_sample"),
        )),
        Box::new(DirectoryClassPath::new(
            root.join("tests/fixtures/tasty_fields"),
        )),
        Box::new(DirectoryClassPath::new(
            root.join("tests/fixtures/tasty_visibility"),
        )),
        Box::new(DirectoryClassPath::new(
            root.join("../dotty-tasty/tests/fixtures/scala3-library"),
        )),
        Box::new(DirectoryClassPath::new(
            root.join("../dotty-tasty/tests/fixtures/path_dependent"),
        )),
    ];
    entries.push(jdk_classpath());
    CompositeClassPath::new(entries)
}

fn classpath_without_jdk() -> CompositeClassPath {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let entries: Vec<Box<dyn dotty_classloader::classloader::ClassPathEntry>> = vec![
        Box::new(DirectoryClassPath::new(
            root.join("tests/fixtures/tasty_sample"),
        )),
        Box::new(DirectoryClassPath::new(
            root.join("tests/fixtures/tasty_fields"),
        )),
        Box::new(DirectoryClassPath::new(
            root.join("tests/fixtures/tasty_visibility"),
        )),
        Box::new(DirectoryClassPath::new(
            root.join("../dotty-tasty/tests/fixtures/scala3-library"),
        )),
        Box::new(DirectoryClassPath::new(
            root.join("../dotty-tasty/tests/fixtures/path_dependent"),
        )),
    ];
    CompositeClassPath::new(entries)
}

fn jdk_classpath() -> Box<dyn dotty_classloader::classloader::ClassPathEntry> {
    let java_home = std::env::var_os("JAVA_HOME").expect("JAVA_HOME is required for this audit");
    Box::new(
        JdkClassPath::new(PathBuf::from(java_home).join("jmods"))
            .expect("real JAVA_HOME jmods should be readable"),
    )
}

fn root_definition_address(
    file: &TastyFile<'_>,
    unpickler: &TastyUnpickler<'_, '_, '_>,
    name: &str,
) -> Option<u32> {
    let ast_index = file.ast_address_index().ok()?;
    ast_index.iter_nodes().find_map(|node| {
        let address = u32::try_from(node.offset).ok()?;
        let raw = ast_index.get(address)?;
        let Ok(StructuredNode::TypeDef(DefinitionBody::TypeDef { name: name_ref, .. })) =
            raw.decode_structured()
        else {
            return None;
        };
        (file.names().get_utf8(name_ref) == Some(name)
            && unpickler.index().symbol_at(address).is_some())
        .then_some(address)
    })
}

fn fixture_provenance(path: &Path) -> String {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    path.strip_prefix(workspace)
        .expect("fixture provenance is rooted in the workspace")
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn run_once(units: &[Unit]) -> String {
    // Each adapter gets its own fresh store, one canonical bootstrap, and
    // independent session/identity state across all selected units.
    let mut legacy_store = SemanticStore::new();
    let legacy_definitions = Definitions::bootstrap(&mut legacy_store);
    let mut legacy_session = LoadingSession::new();
    let mut legacy = Vec::new();
    for unit in units {
        let requested = BinaryName::from_internal(unit.lookup);
        let loaded = {
            let mut loader = ClassLoader::with_definitions(
                classpath(),
                &mut legacy_store,
                legacy_definitions,
                legacy_session,
            );
            let loaded = loader.load_class(&requested);
            legacy_session = loader.into_session();
            loaded
        };
        legacy.push(match loaded {
            Ok(symbol) => observe_symbol(&legacy_store, symbol, &unit.path),
            Err(error) => failed_observation(
                classloader_outcome(&format!("{error:?}")),
                format!("{error:?}"),
                &unit.path,
            ),
        });
    }

    let mut unpickler_store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut unpickler_store);
    let resolver = ClasspathSymbolResolver::new(classpath(), definitions, LoadingSession::new());
    let mut session = TastySession::new()
        .with_resolver(Box::new(resolver))
        .expect("fresh resolver-backed TASTy session");
    let mut unpicked = Vec::new();
    let mut eligible_roots = 0;
    for unit in units {
        let bytes = fs::read(&unit.path).expect("frozen bytes");
        let file = TastyFile::parse(&bytes).expect("cohort already structurally validated");
        let simple_name = unit.lookup.rsplit('/').next().unwrap_or(unit.lookup);
        let mut unpickler = TastyUnpickler::with_session(
            &file,
            &mut unpickler_store,
            definitions,
            std::mem::replace(&mut session, TastySession::new()),
        );
        let (result, next_session) = if let Err(error) = unpickler.enter_symbols() {
            (
                Err(failed_observation(
                    unpickler_outcome(&format!("{error:?}")),
                    format!("{error:?}"),
                    &unit.path,
                )),
                None,
            )
        } else if let Some(address) = root_definition_address(&file, &unpickler, simple_name) {
            let symbol = unpickler
                .index()
                .symbol_at(address)
                .expect("root is entered");
            eligible_roots += 1;
            match unpickler.complete_symbol(address) {
                Ok(_) => (Ok(symbol), Some(())),
                Err(error) => (
                    Err(failed_observation(
                        unpickler_outcome(&format!("{error:?}")),
                        format!("{error:?}"),
                        &unit.path,
                    )),
                    None,
                ),
            }
        } else {
            (
                Err(failed_observation(
                    "UNPICKLER_UNSUPPORTED",
                    format!("no root definition named {simple_name}"),
                    &unit.path,
                )),
                None,
            )
        };
        let (_, next) = unpickler.into_session_parts();
        session = next;
        unpicked.push(match result {
            Ok(symbol) if next_session.is_some() => {
                observe_symbol(&unpickler_store, symbol, &unit.path)
            }
            Ok(_) => unreachable!("successful completion carries a session marker"),
            Err(observation) => observation,
        });
    }
    assert!(
        eligible_roots >= 12,
        "only {eligible_roots} frozen units have a class/module root"
    );
    let unpickled = unpicked;

    let mut report = String::from(
        "unit_id\tfield\tlegacy_outcome\tlegacy_value\tlegacy_error\tlegacy_provenance\tunpickler_outcome\tunpickler_value\tunpickler_error\tunpickler_provenance\tnormalized_delta\n",
    );
    for ((unit, left), right) in units.iter().zip(legacy).zip(unpickled) {
        let left_fields = split_fields(&left.value);
        let right_fields = split_fields(&right.value);
        for field in FIELDS {
            let lvalue = left_fields.get(*field).cloned().unwrap_or_default();
            let rvalue = right_fields.get(*field).cloned().unwrap_or_default();
            let legacy_outcome = observation_field_outcome("LOADER", &left, &lvalue);
            let unpickler_outcome = observation_field_outcome("UNPICKLER", &right, &rvalue);
            let delta = if legacy_outcome != "MATCH" || unpickler_outcome != "MATCH" {
                "not-comparable"
            } else if lvalue == rvalue {
                "equal"
            } else {
                "different"
            };
            row(
                &mut report,
                &[
                    &unit.id,
                    field,
                    legacy_outcome,
                    &lvalue,
                    &left.error,
                    &left.provenance,
                    unpickler_outcome,
                    &rvalue,
                    &right.error,
                    &right.provenance,
                    delta,
                ],
            );
        }
    }
    report
}

fn observation_field_outcome(
    adapter: &str,
    observation: &Observation,
    value: &str,
) -> &'static str {
    if observation.outcome != "MATCH" {
        observation.outcome
    } else {
        field_outcome(adapter, value).unwrap_or("MATCH")
    }
}

fn field_outcome(adapter: &str, value: &str) -> Option<&'static str> {
    if value.contains("unresolved-type-name") || value.contains("unresolved-term-name") {
        Some("UNRESOLVED_EXTERNAL")
    } else if [
        "<missing>",
        "<deferred>",
        "<error>",
        "<unavailable>",
        "Error(",
        "unknown-semantic-type-shape",
    ]
    .iter()
    .any(|marker| value.contains(marker))
    {
        Some(if adapter == "LOADER" {
            "LOADER_UNSUPPORTED"
        } else {
            "UNPICKLER_UNSUPPORTED"
        })
    } else {
        None
    }
}

fn split_fields(value: &str) -> std::collections::BTreeMap<&str, String> {
    value
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key, value.to_owned()))
        .collect()
}

fn row(out: &mut String, cells: &[&str]) {
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            out.push('\t');
        }
        out.push_str(&cell.replace(['\t', '\n', '\r'], " "));
    }
    out.push('\n');
}

fn failed_observation(outcome: &'static str, error: String, path: &Path) -> Observation {
    Observation {
        outcome,
        value: String::new(),
        error: normalize_allocation_ids(&error),
        provenance: fixture_provenance(path),
    }
}

/// Keep diagnostic meaning while removing arena allocation details from the
/// checked-in comparison. Type IDs are local to one semantic store and can
/// change when an unrelated dependency is materialized first.
fn normalize_allocation_ids(error: &str) -> String {
    let mut normalized = String::with_capacity(error.len());
    let mut rest = error;
    while let Some(index) = rest.find("TypeId(") {
        let (prefix, id_and_tail) = rest.split_at(index);
        normalized.push_str(prefix);
        normalized.push_str("TypeId(*)");
        let after_prefix = &id_and_tail["TypeId(".len()..];
        if let Some(close) = after_prefix.find(')')
            && !after_prefix[..close].is_empty()
            && after_prefix[..close]
                .bytes()
                .all(|byte| byte.is_ascii_digit())
        {
            rest = &after_prefix[close + 1..];
        } else {
            // Preserve malformed or future debug output verbatim rather than
            // accidentally consuming text that is not a numeric TypeId.
            normalized.truncate(normalized.len() - "TypeId(*)".len());
            normalized.push_str("TypeId(");
            rest = after_prefix;
        }
    }
    normalized.push_str(rest);
    normalized
}

fn classloader_outcome(error: &str) -> &'static str {
    if error.contains("NotFound") || error.contains("DependencyFailure") {
        "UNRESOLVED_EXTERNAL"
    } else if error.contains("InvalidTastyFile")
        && (error.contains("UnresolvedSupertype") || error.contains("MissingTypeDef"))
    {
        "LOADER_UNSUPPORTED"
    } else if error.contains("InvalidTastyFile") || error.contains("InvalidClassFile") {
        "MALFORMED_OR_VERSION"
    } else {
        "LOADER_UNSUPPORTED"
    }
}

fn unpickler_outcome(error: &str) -> &'static str {
    if error.contains("Unsupported") {
        "UNPICKLER_UNSUPPORTED"
    } else if error.contains("Unresolved") || error.contains("Missing") {
        "UNRESOLVED_EXTERNAL"
    } else {
        "MALFORMED_OR_VERSION"
    }
}

fn observe_symbol(store: &SemanticStore, id: SymbolId, path: &Path) -> Observation {
    let value = normalized_symbol(store, id);
    Observation {
        outcome: "MATCH",
        value,
        error: String::new(),
        provenance: fixture_provenance(path),
    }
}

fn normalized_symbol(store: &SemanticStore, id: SymbolId) -> String {
    let symbol = store.symbols.get(id);
    let kind = format!("{:?}", symbol.kind);
    let name = store.names.resolve(symbol.name.text());
    let path = symbol_path(store, id);
    let visibility_value = visibility(store, &symbol.visibility);
    let flags = flags(symbol.flags);
    let companion = symbol
        .links
        .companion
        .map(|other| symbol_path(store, other))
        .unwrap_or_else(|| "none".into());
    let annotations = symbol
        .annotations
        .iter()
        .map(|annotation| {
            let annotation = store.annotations.get(*annotation);
            format!(
                "{}:{}",
                type_repr(store, annotation.ty),
                arguments_repr(store, &annotation.arguments)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let (hierarchy, type_parameters, parameter_clauses, members) = match symbol.info {
        SymbolInfo::Complete(info) => match store.types.get(info) {
            Type::ClassInfo(info) => {
                let parents = info
                    .parents
                    .iter()
                    .map(|ty| type_repr(store, *ty))
                    .collect::<Vec<_>>()
                    .join("&");
                let entered = store
                    .scopes
                    .get(info.declarations)
                    .entered_symbols()
                    .collect::<Vec<_>>();
                let type_params = entered.iter().find_map(|member_id| {
                    let member = store.symbols.get(*member_id);
                    if member.kind != dotty_core::SymbolKind::Constructor {
                        return None;
                    }
                    let SymbolInfo::Complete(constructor_type) = member.info else {
                        return None;
                    };
                    match store.types.get(constructor_type) {
                        Type::Poly(poly) => {
                            Some(format!("[{}]", type_params_repr(store, &poly.params)))
                        }
                        Type::Method(_) => Some("[]".into()),
                        _ => None,
                    }
                });
                let type_params = type_params.unwrap_or_else(|| "<unavailable>".into());
                let mut members = entered
                    .into_iter()
                    .map(|member| {
                        let member = store.symbols.get(member);
                        let name = store.names.resolve(member.name.text());
                        let ty = match member.info {
                            SymbolInfo::Complete(info) => type_repr(store, info),
                            SymbolInfo::Missing => "<missing>".into(),
                            SymbolInfo::Deferred(_) => "<deferred>".into(),
                            SymbolInfo::Error => "<error>".into(),
                        };
                        format!(
                            "{}:{:?}:{}:{}",
                            name,
                            member.kind,
                            visibility(store, &member.visibility),
                            ty
                        )
                    })
                    .collect::<Vec<_>>();
                members.sort();
                let constructors = members
                    .iter()
                    .filter(|member| member.contains(":Constructor:"))
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(";");
                (parents, type_params, constructors, members.join(";"))
            }
            other => (
                format!("<class-info:{other:?}>"),
                "<unavailable>".into(),
                "<unavailable>".into(),
                "<unavailable>".into(),
            ),
        },
        SymbolInfo::Missing => (
            "<missing>".into(),
            "<unavailable>".into(),
            "<unavailable>".into(),
            "<unavailable>".into(),
        ),
        SymbolInfo::Deferred(_) => (
            "<deferred>".into(),
            "<unavailable>".into(),
            "<unavailable>".into(),
            "<unavailable>".into(),
        ),
        SymbolInfo::Error => (
            "<error>".into(),
            "<unavailable>".into(),
            "<unavailable>".into(),
            "<unavailable>".into(),
        ),
    };
    format!(
        "identity={path}|{kind}|{name}\nvisibility={visibility_value}\nflags={flags}\nhierarchy={hierarchy}\ntype_parameters={type_parameters}\nparameter_clauses={parameter_clauses}\nmember_signatures={members}\ncompanion={companion}\nannotations={annotations}"
    )
}

fn symbol_path(store: &SemanticStore, id: SymbolId) -> String {
    let mut parts = Vec::new();
    let mut current = Some(id);
    while let Some(symbol) = current {
        let value = store.symbols.get(symbol);
        let name = store.names.resolve(value.name.text());
        if !name.is_empty() && name != "_root_" {
            parts.push(name.to_owned());
        }
        current = value.owner;
    }
    parts.reverse();
    parts.join(".")
}

fn visibility(store: &SemanticStore, visibility: &dotty_core::Visibility) -> String {
    match visibility {
        dotty_core::Visibility::Public => "public".into(),
        dotty_core::Visibility::Private => "private".into(),
        dotty_core::Visibility::Protected => "protected".into(),
        dotty_core::Visibility::Package(owner) => format!("package:{}", symbol_path(store, *owner)),
        dotty_core::Visibility::PrivateWithin(owner) => {
            format!("private-within:{}", symbol_path(store, *owner))
        }
        dotty_core::Visibility::ProtectedWithin(owner) => {
            format!("protected-within:{}", symbol_path(store, *owner))
        }
    }
}

fn flags(flags: dotty_core::SymbolFlags) -> String {
    let values = [
        (dotty_core::SymbolFlags::ABSTRACT, "abstract"),
        (dotty_core::SymbolFlags::FINAL, "final"),
        (dotty_core::SymbolFlags::SEALED, "sealed"),
        (dotty_core::SymbolFlags::CASE, "case"),
        (dotty_core::SymbolFlags::IMPLICIT, "implicit"),
        (dotty_core::SymbolFlags::GIVEN, "given"),
        (dotty_core::SymbolFlags::LAZY, "lazy"),
        (dotty_core::SymbolFlags::MUTABLE, "mutable"),
        (dotty_core::SymbolFlags::INLINE, "inline"),
        (dotty_core::SymbolFlags::TRANSPARENT, "transparent"),
        (dotty_core::SymbolFlags::OPAQUE, "opaque"),
        (dotty_core::SymbolFlags::EXTENSION, "extension"),
        (dotty_core::SymbolFlags::STATIC, "static"),
        (dotty_core::SymbolFlags::SYNTHETIC, "synthetic"),
        (dotty_core::SymbolFlags::ERASED, "erased"),
        (dotty_core::SymbolFlags::OVERRIDE, "override"),
        (dotty_core::SymbolFlags::ENUM, "enum"),
        (dotty_core::SymbolFlags::EXPORTED, "exported"),
    ];
    values
        .iter()
        .filter_map(|(flag, name)| flags.contains(*flag).then_some(*name))
        .collect::<Vec<_>>()
        .join(",")
}

fn type_repr(store: &SemanticStore, id: dotty_core::TypeId) -> String {
    match store.types.get(id) {
        Type::NoType => "NoType".into(),
        Type::NoPrefix => "NoPrefix".into(),
        Type::Error(error) => format!("Error({})", store.names.resolve(error.message)),
        Type::TypeRef { target, .. } => target
            .symbol()
            .map(|symbol| symbol_path(store, symbol))
            .unwrap_or_else(|| "unresolved-type-name".into()),
        Type::TermRef { target, .. } => target
            .symbol()
            .map(|symbol| symbol_path(store, symbol))
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
        Type::ThisType { class } => format!("this({})", symbol_path(store, *class)),
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

fn arguments_repr(_store: &SemanticStore, arguments: &AnnotationArguments) -> String {
    match arguments {
        AnnotationArguments::Unavailable => "unavailable".into(),
        AnnotationArguments::Known(values) => format!("known:{}", values.len()),
    }
}

#[test]
fn frozen_cohort_is_complete_and_both_fresh_adapter_runs_are_deterministic() {
    let units = units();
    let first = run_once(&units);
    let second = run_once(&units);
    assert_eq!(
        first.as_bytes(),
        second.as_bytes(),
        "normalized report differs across fresh sessions"
    );
    if std::env::var_os("DOTTY_DUMP_SEMANTIC_TASTY_PARITY").is_some() {
        println!("{first}");
        return;
    }
    let expected =
        include_str!("../../../docs/classloader-semantic-tasty-convergence/comparison-1948.tsv");
    assert_live_report_matches_frozen_with_selected_option_member_delta(&first, expected);
    let mut keys = HashSet::new();
    for row in first.lines().skip(1) {
        let cells = row.split('\t').collect::<Vec<_>>();
        assert_eq!(cells.len(), 11, "malformed comparison row: {row}");
        assert!(
            cells[5].starts_with("crates/"),
            "non-portable legacy provenance: {row}"
        );
        assert!(
            cells[9].starts_with("crates/"),
            "non-portable unpickler provenance: {row}"
        );
        let key = (cells[0], cells[1]);
        assert!(keys.insert(key), "duplicate comparison row: {row}");
        let outcome = cells[2];
        assert!(
            OUTCOMES.contains(&outcome),
            "unknown or malformed outcome {outcome}"
        );
    }
    assert_eq!(keys.len(), units.len() * FIELDS.len());

    let function1_type_parameters = first
        .lines()
        .skip(1)
        .find_map(|row| {
            let cells = row.split('\t').collect::<Vec<_>>();
            (cells[0] == "function1" && cells[1] == "type_parameters").then_some(cells[3])
        })
        .expect("Function1 class type parameter row");
    assert!(
        function1_type_parameters.starts_with("[T1:"),
        "Function1's first declared parameter must be T1: {function1_type_parameters}"
    );
    assert!(
        function1_type_parameters.contains(",R:"),
        "Function1's second declared parameter must be R: {function1_type_parameters}"
    );
}

fn assert_live_report_matches_frozen_with_selected_option_member_delta(
    live: &str,
    historical: &str,
) {
    let rows = |report: &str| {
        report
            .lines()
            .map(|line| line.split('\t').map(str::to_owned).collect::<Vec<_>>())
            .collect::<Vec<_>>()
    };
    let live = rows(live);
    let historical = rows(historical);
    assert_eq!(
        live.first(),
        historical.first(),
        "comparison schema changed"
    );
    assert_eq!(live.len(), 127, "the live comparison must retain 126 rows");
    assert_eq!(
        historical.len(),
        127,
        "the frozen comparison must retain its original 126 rows"
    );

    let changed = live
        .iter()
        .skip(1)
        .zip(historical.iter().skip(1))
        .filter(|(current, frozen)| current != frozen)
        .collect::<Vec<_>>();
    assert_eq!(changed.len(), 1, "only the selected member row may change");
    let (current, frozen) = changed[0];
    assert_eq!(
        (current[0].as_str(), current[1].as_str()),
        ("option", "member_signatures")
    );
    assert_eq!(current.len(), 11);
    assert_eq!(frozen.len(), 11);
    for column in 0..current.len() {
        if column != 3 {
            assert_eq!(
                current[column], frozen[column],
                "unapproved comparator column changed at {column}"
            );
        }
    }
    assert_eq!(current[2], "LOADER_UNSUPPORTED");
    assert!(frozen[3].contains(
        "getOrElse:Method:public:[B:None:bounds(scala.Option.A,Any)]=>(Error(unresolved .tasty member type: TASTy decoder could not reduce the declared type tree to a supported shape))Plain:ParamRef[0]"
    ));
    assert!(current[3].contains(
        "getOrElse:Method:public:[B:None:bounds(scala.Option.A,Any)]=>(by-name(ParamRef[0]))Plain:ParamRef[0]"
    ));
    let current_members = current[3].split(';').collect::<Vec<_>>();
    let frozen_members = frozen[3].split(';').collect::<Vec<_>>();
    assert_eq!(current_members.len(), frozen_members.len());
    let changed_members = current_members
        .iter()
        .zip(&frozen_members)
        .filter(|(current, frozen)| current != frozen)
        .collect::<Vec<_>>();
    assert_eq!(
        changed_members.len(),
        1,
        "the member decoder change must not expand to neighboring Option signatures"
    );
    assert!(changed_members[0].0.starts_with("getOrElse:"));
}

#[derive(Debug, PartialEq, Eq)]
struct OpenConstructorObservation {
    outcome: String,
    class_address: u32,
    constructor_address: u32,
    graph: String,
}

fn complete_open_constructor_once() -> OpenConstructorObservation {
    use dotty_core::{SymbolInfo, SymbolKind};
    use dotty_tasty::tasty::DEFDEF_TAG;

    let unit = units()
        .into_iter()
        .find(|unit| unit.id == "visibility-open")
        .expect("frozen Open fixture");
    let bytes = fs::read(&unit.path).expect("frozen Open bytes");
    let file = TastyFile::parse(&bytes).expect("frozen Open TASTy parses");
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let resolver = ClasspathSymbolResolver::new(classpath(), definitions, LoadingSession::new());
    let mut unpickler = TastyUnpickler::with_session(
        &file,
        &mut store,
        definitions,
        TastySession::new()
            .with_resolver(Box::new(resolver))
            .expect("fresh resolver-backed TASTy session"),
    );
    unpickler.enter_symbols().expect("enter Open symbols");
    let class_address = root_definition_address(&file, &unpickler, "Open").expect("Open TYPEDEF");
    let class = unpickler
        .index()
        .symbol_at(class_address)
        .expect("Open symbol");
    let class_info_type = unpickler
        .complete_symbol(class_address)
        .expect("complete Open class with genuine dependencies");
    assert!(matches!(
        unpickler.symbol_state_at(class_address),
        Some((SymbolKind::Class, SymbolInfo::Complete(_)))
    ));

    let ast = file.ast_address_index().expect("Open address index");
    let constructors = ast
        .iter_nodes()
        .filter(|node| node.tag == DEFDEF_TAG)
        .filter_map(|node| u32::try_from(node.offset).ok())
        .filter(|address| {
            matches!(
                unpickler.symbol_state_at(*address),
                Some((SymbolKind::Constructor, _))
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        constructors.len(),
        1,
        "Open has one original constructor DEFDEF"
    );
    let constructor_address = constructors[0];
    let constructor = unpickler.index().symbol_at(constructor_address).unwrap();
    assert_eq!(
        unpickler.symbol_state_at(constructor_address),
        Some((SymbolKind::Constructor, SymbolInfo::Missing)),
        "class completion leaves its constructor pending"
    );

    let batch_error = unpickler
        .complete_symbols(&[constructor_address, u32::MAX])
        .expect_err("the batch completes the constructor, then fails on an invalid address");
    assert_eq!(
        format!("{batch_error:?}"),
        "MissingEnteredSymbol { address: 4294967295 }"
    );
    assert_eq!(
        unpickler.symbol_state_at(constructor_address),
        Some((SymbolKind::Constructor, SymbolInfo::Missing)),
        "failed batch rolls back the constructor completion"
    );
    assert_eq!(
        unpickler.symbol_state_at(class_address),
        Some((SymbolKind::Class, SymbolInfo::Complete(class_info_type))),
        "failed batch preserves completed class identity and info"
    );
    let rolled_back_type_count = unpickler.index().type_count();
    let constructor_type = unpickler
        .complete_symbol(constructor_address)
        .expect("retry the original constructor after batch rollback");
    assert!(
        unpickler.index().type_count() > rolled_back_type_count,
        "retry reconstructs the constructor type allocations"
    );
    let type_count = unpickler.index().type_count();
    assert_eq!(
        unpickler.complete_symbol(constructor_address),
        Ok(constructor_type)
    );
    assert_eq!(unpickler.index().type_count(), type_count);
    drop(unpickler);

    let method = match store.types.get(constructor_type) {
        Type::Method(method) => method,
        other => panic!("Open constructor must have an ordinary method binder: {other:?}"),
    };
    assert!(method.params.is_empty());
    assert_eq!(method.kind, dotty_core::types::MethodKind::Plain);
    let result_class = match store.types.get(method.result) {
        Type::TypeRef { target, .. } => target.symbol().expect("resolved constructor result"),
        other => panic!("constructor result must be a class reference: {other:?}"),
    };
    assert_eq!(
        result_class, class,
        "constructor result keeps original Open identity"
    );
    let class_info = match store.types.get(class_info_type) {
        Type::ClassInfo(info) => info,
        other => panic!("Open info must be ClassInfo: {other:?}"),
    };
    let parents = class_info
        .parents
        .iter()
        .filter_map(|parent| match store.types.get(*parent) {
            Type::TypeRef { target, .. } => target.symbol(),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(parents, vec![definitions.object_class]);
    assert_eq!(store.symbols.get(constructor).owner, Some(class));
    assert_eq!(
        store.symbols.get(constructor).visibility,
        dotty_core::Visibility::Public
    );
    assert_eq!(store.symbols.get(constructor).kind, SymbolKind::Constructor);
    assert!(
        !store
            .scopes
            .get(class_info.declarations)
            .entered_symbols()
            .any(|id| store.symbols.get(id).kind == SymbolKind::TypeParameter)
    );
    assert!(
        store
            .scopes
            .get(class_info.declarations)
            .entered_symbols()
            .any(|id| id == constructor)
    );
    assert_eq!(
        store
            .scopes
            .get(class_info.declarations)
            .entered_symbols()
            .filter(|id| *id == constructor)
            .count(),
        1,
        "constructor appears exactly once in Open's declaration scope"
    );
    assert!(!matches!(store.types.get(constructor_type), Type::Poly(_)));
    assert_eq!(
        store.symbols.get(class).info,
        SymbolInfo::Complete(class_info_type)
    );
    assert_eq!(
        store.symbols.get(constructor).info,
        SymbolInfo::Complete(constructor_type)
    );

    let graph = format!(
        "class={}|owner={}|kind={:?}|visibility={:?}|constructor=DEFDEF@{}|binder=Method({:?},params={})|result={}|parents={}",
        symbol_path(&store, class),
        symbol_path(&store, store.symbols.get(constructor).owner.unwrap()),
        store.symbols.get(constructor).kind,
        store.symbols.get(constructor).visibility,
        constructor_address,
        method.kind,
        method.params.len(),
        symbol_path(&store, result_class),
        parents
            .iter()
            .map(|id| symbol_path(&store, *id))
            .collect::<Vec<_>>()
            .join("&"),
    );
    OpenConstructorObservation {
        outcome: "SUPPORTED".into(),
        class_address,
        constructor_address,
        graph,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct AuthenticConstructorObservation {
    class_address: u32,
    constructor_address: u32,
    class_path: String,
    constructor_owner: String,
    regular_clause: bool,
    parameter_count: usize,
    constructor_owned_formal_symbols: usize,
    parameter_type: String,
    serialized_result_name_refs: String,
    effective_result: String,
    class_template_parameters: usize,
    class_template_stats: usize,
    header_field_and_constructor_formal_are_distinct: bool,
}

fn complete_authentic_constructor_once(simple_name: &str) -> AuthenticConstructorObservation {
    use dotty_core::types::MethodKind;
    use dotty_core::{SymbolInfo, SymbolKind};
    use dotty_tasty::tasty::{DEFDEF_TAG, PARAM_TAG, TEMPLATE_TAG};

    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../dotty-tasty-unpickler/tests/fixtures/semantic");
    let bytes = fs::read(fixture_dir.join(format!("{simple_name}.tasty")))
        .expect("original Scala 3.9 semantic fixture");
    let fixture_root = std::env::temp_dir().join(format!(
        "dotty-rs-constructor-audit-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos()
    ));
    let package_dir = fixture_root.join("me/cytrowski/tastyfixtures/semantic");
    fs::create_dir_all(&package_dir).expect("test-local package-relative classpath");
    fs::write(package_dir.join(format!("{simple_name}.tasty")), &bytes)
        .expect("copy original TASTy bytes without modification");
    let file = TastyFile::parse(&bytes).expect("fixture parses");
    assert_eq!(file.header().major_version, 28);
    assert_eq!(file.header().minor_version, 9);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let resolver = ClasspathSymbolResolver::new(
        CompositeClassPath::new(vec![
            Box::new(DirectoryClassPath::new(&fixture_root)),
            Box::new(classpath()),
        ]),
        definitions,
        LoadingSession::new(),
    );
    let mut unpickler = TastyUnpickler::with_session(
        &file,
        &mut store,
        definitions,
        TastySession::new()
            .with_resolver(Box::new(resolver))
            .expect("fresh resolver-backed session"),
    );
    unpickler.enter_symbols().expect("enter original symbols");
    let address_index = file.ast_address_index().expect("original AST addresses");
    let class_address = root_definition_address(&file, &unpickler, simple_name)
        .expect("original class TYPEDEF address");
    let class = unpickler
        .index()
        .symbol_at(class_address)
        .expect("class symbol");
    let constructor_addresses = address_index
        .iter_nodes()
        .filter(|node| node.tag == DEFDEF_TAG)
        .filter_map(|node| u32::try_from(node.offset).ok())
        .filter(|address| {
            matches!(
                unpickler.symbol_state_at(*address),
                Some((SymbolKind::Constructor, _))
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(constructor_addresses.len(), 1);
    let constructor_address = constructor_addresses[0];
    let neighboring_roles_are_distinct = if simple_name == "CtorWithVal" {
        let field = unpickler
            .index()
            .symbol_at(9)
            .expect("header field at original AST address 9");
        let constructor_formal = unpickler
            .index()
            .symbol_at(31)
            .expect("constructor's own formal at original AST address 31");
        assert_ne!(field, constructor_formal);
        true
    } else {
        false
    };

    let _class_info = unpickler
        .complete_symbol(class_address)
        .expect("complete class");
    let constructor_type = unpickler
        .complete_symbol(constructor_address)
        .expect("complete primary constructor");
    let constructor = unpickler
        .index()
        .symbol_at(constructor_address)
        .expect("entered constructor symbol");
    let formal_symbols = address_index
        .iter_nodes()
        .filter(|node| node.tag == PARAM_TAG)
        .filter_map(|node| u32::try_from(node.offset).ok())
        .filter_map(|address| unpickler.index().symbol_at(address))
        .collect::<Vec<_>>();
    assert!(matches!(
        unpickler.symbol_state_at(class_address),
        Some((SymbolKind::Class, SymbolInfo::Complete(_)))
    ));
    assert_eq!(
        unpickler.symbol_state_at(constructor_address),
        Some((
            SymbolKind::Constructor,
            SymbolInfo::Complete(constructor_type)
        ))
    );
    let semantic_index = unpickler.into_index();
    let constructor_owned_formal_symbols = formal_symbols
        .iter()
        .filter(|symbol| store.symbols.get(**symbol).owner == Some(constructor))
        .count();
    assert_eq!(constructor_owned_formal_symbols, 1);
    drop(semantic_index);
    fs::remove_dir_all(&fixture_root).expect("remove test-local classpath after resolution");

    let method = match store.types.get(constructor_type) {
        Type::Method(method) => method,
        other => panic!("constructor must be a regular Method type: {other:?}"),
    };
    let reference_symbol = |type_id| match store.types.get(type_id) {
        Type::TypeRef { target, .. } => target.symbol(),
        _ => None,
    };
    let parameter_symbol = method
        .params
        .first()
        .and_then(|param| reference_symbol(param.ty))
        .expect("constructor formal must resolve to a named type");
    let canonical_int = store
        .types
        .get(definitions.int)
        .reference_symbol()
        .expect("bootstrapped canonical Int reference");
    let parameter_type = if parameter_symbol == canonical_int {
        "scala.Int".to_owned()
    } else {
        symbol_path(&store, parameter_symbol)
    };
    let result = reference_symbol(method.result).expect("effective class result");
    let asts = file.asts().expect("package tree");
    let package = asts.get(0).expect("root package");
    let package = package.decode_package().expect("package structure");
    let class_root = package.stats.get(0).expect("top-level class");
    let DefinitionBody::TypeDef {
        type_or_template, ..
    } = class_root.decode_definition_body().expect("package body")
    else {
        panic!("root package must decode as a package")
    };
    let dotty_tasty::tasty::RawTree::LengthNode(template) = type_or_template else {
        panic!("class template is a length node")
    };
    assert_eq!(template.tag, TEMPLATE_TAG);
    let template_shape = template
        .decode_template_structure()
        .expect("template structure");
    let constructor_node = address_index
        .get(constructor_address)
        .expect("indexed original constructor DEFDEF");
    let serialized_return = constructor_node
        .decode_defdef_body()
        .expect("constructor DEFDEF body")
        .return_type;
    let dotty_tasty::tasty::RawTree::NatAst { value, .. } = serialized_return else {
        panic!("constructor result is encoded as a named package type")
    };
    let serialized_result = file
        .render_name(value)
        .expect("render serialized result name");

    AuthenticConstructorObservation {
        class_address,
        constructor_address,
        class_path: symbol_path(&store, class),
        constructor_owner: symbol_path(&store, store.symbols.get(constructor).owner.unwrap()),
        regular_clause: method.kind == MethodKind::Plain,
        parameter_count: method.params.len(),
        constructor_owned_formal_symbols,
        parameter_type,
        serialized_result_name_refs: serialized_result,
        effective_result: symbol_path(&store, result),
        class_template_parameters: template_shape.term_params.len(),
        class_template_stats: template_shape.stats.len(),
        header_field_and_constructor_formal_are_distinct: neighboring_roles_are_distinct,
    }
}

#[test]
fn authentic_plain_constructor_is_completed_repeatably_and_val_neighbor_is_distinct() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../dotty-tasty-unpickler/tests/fixtures/semantic");
    for (name, expected) in [
        (
            "Constructors.scala",
            "ee054964fa24c1e2084350b257b0875d2c9c38038b73750b7eefb12b70ffcd63",
        ),
        (
            "CtorPlain.tasty",
            "0d2f01cc5444bda30ae6a920162b3eb89fd2a5a7aefc84a1e22bef8a35120ddb",
        ),
        (
            "CtorWithVal.tasty",
            "53d335f857224d38559335eaf1efba92006a55ad9ed8e74d1f58a9e77f8a0192",
        ),
    ] {
        let output = Command::new("sha256sum")
            .arg(root.join(name))
            .output()
            .expect("sha256sum for frozen compiler fixtures");
        assert!(output.status.success());
        let output = String::from_utf8(output.stdout).expect("sha256sum UTF-8");
        assert_eq!(
            output.split_whitespace().next(),
            Some(expected),
            "{name} hash"
        );
    }
    let source = fs::read_to_string(root.join("Constructors.scala")).expect("Scala source");
    assert!(source.contains("class CtorPlain(x: Int)"));
    assert!(source.contains("class CtorWithVal(val x: Int)"));

    let first = complete_authentic_constructor_once("CtorPlain");
    let second = complete_authentic_constructor_once("CtorPlain");
    assert_eq!(first, second, "fresh-store normalized receipts are stable");
    assert_eq!(first.class_address, 4);
    assert_eq!(first.constructor_address, 30);
    assert_eq!(
        first.class_path,
        "me.cytrowski.tastyfixtures.semantic.CtorPlain"
    );
    assert_eq!(first.constructor_owner, first.class_path);
    assert!(first.regular_clause);
    assert_eq!(first.parameter_count, 1);
    assert_eq!(first.constructor_owned_formal_symbols, 1);
    assert_eq!(first.parameter_type, "scala.Int");
    assert_eq!(first.effective_result, first.class_path);
    assert_eq!(first.serialized_result_name_refs, "Unit");
    assert_eq!(first.class_template_parameters, 1);
    assert_eq!(first.class_template_stats, 1);

    let with_val = complete_authentic_constructor_once("CtorWithVal");
    assert_eq!(with_val.class_address, 4);
    assert_eq!(with_val.constructor_address, 28);
    assert_eq!(with_val.class_template_parameters, 1);
    assert_eq!(with_val.class_template_stats, 1);
    assert!(with_val.header_field_and_constructor_formal_are_distinct);
}

fn complete_visibility_constructor_once(unit_id: &str) -> Result<String, String> {
    use dotty_core::{SymbolInfo, SymbolKind};
    use dotty_tasty::tasty::{
        DEFDEF_TAG, DefinitionBody, RawTree, StructuredNode, StructuredTree, TermValue,
    };

    let unit = units()
        .into_iter()
        .find(|unit| unit.id == unit_id)
        .unwrap_or_else(|| panic!("missing frozen unit {unit_id}"));
    let bytes = fs::read(&unit.path).map_err(|error| format!("fixture read: {error}"))?;
    let file = TastyFile::parse(&bytes).map_err(|error| format!("TASTy parse: {error:?}"))?;
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let resolver = ClasspathSymbolResolver::new(classpath(), definitions, LoadingSession::new());
    let mut unpickler = TastyUnpickler::with_session(
        &file,
        &mut store,
        definitions,
        TastySession::new()
            .with_resolver(Box::new(resolver))
            .map_err(|error| format!("attach resolver: {error:?}"))?,
    );
    unpickler
        .enter_symbols()
        .map_err(|error| format!("enter symbols: {error:?}"))?;
    let simple_name = unit.lookup.rsplit('/').next().unwrap_or(unit.lookup);
    let class_address = root_definition_address(&file, &unpickler, simple_name)
        .ok_or_else(|| format!("no indexed TYPEDEF for {simple_name}"))?;
    let class = unpickler
        .index()
        .symbol_at(class_address)
        .ok_or_else(|| "class TYPEDEF has no symbol".to_owned())?;
    let class_type = unpickler
        .complete_symbol(class_address)
        .map_err(|error| format!("complete class TYPEDEF@{class_address}: {error:?}"))?;
    if !matches!(
        unpickler.symbol_state_at(class_address),
        Some((SymbolKind::Class, SymbolInfo::Complete(_)))
    ) {
        return Err(format!("class TYPEDEF@{class_address} is not complete"));
    }

    let ast = file
        .ast_address_index()
        .map_err(|error| format!("AST address index: {error:?}"))?;
    let constructors = ast
        .iter_nodes()
        .filter(|node| node.tag == DEFDEF_TAG)
        .filter_map(|node| u32::try_from(node.offset).ok())
        .filter(|address| {
            matches!(
                unpickler.symbol_state_at(*address),
                Some((SymbolKind::Constructor, _))
            )
        })
        .collect::<Vec<_>>();
    if constructors.len() != 1 {
        return Err(format!(
            "expected one indexed primary constructor DEFDEF for {simple_name}; found {} at {:?}",
            constructors.len(),
            constructors
        ));
    }
    let constructor_address = constructors[0];
    let constructor_node = ast
        .get(constructor_address)
        .ok_or_else(|| format!("missing original constructor DEFDEF@{constructor_address}"))?
        .decode_structured()
        .map_err(|error| format!("decode original constructor DEFDEF: {error:?}"))?;
    let (serialized_clause_count, serialized_result, constructor_tail) = match constructor_node {
        StructuredNode::DefDef(body) => {
            let result = body
                .return_type
                .decode_structured()
                .map_err(|error| format!("decode serialized constructor result: {error:?}"))?;
            let result_name = match result {
                StructuredTree::Reference(reference) => {
                    let name = file
                        .names()
                        .get_utf8(reference.reference)
                        .unwrap_or("<name>");
                    let qualifier = match reference.qualifier {
                        RawTree::Leaf(term) => match term.value {
                            TermValue::NameRef(name) => file.names().get_utf8(name).unwrap_or(""),
                            _ => "",
                        },
                        _ => "",
                    };
                    if qualifier.is_empty() {
                        name.to_owned()
                    } else {
                        format!("{qualifier}.{name}")
                    }
                }
                _ => "<non-reference-result-tree>".to_owned(),
            };
            (body.clauses.len(), result_name, format!("{:?}", body.tail))
        }
        other => return Err(format!("constructor address decoded as {other:?}")),
    };
    let class_node = ast
        .get(class_address)
        .ok_or_else(|| format!("missing original class TYPEDEF@{class_address}"))?
        .decode_structured()
        .map_err(|error| format!("decode original class TYPEDEF: {error:?}"))?;
    let class_tail = match class_node {
        StructuredNode::TypeDef(DefinitionBody::TypeDef { tail, .. }) => format!("{tail:?}"),
        other => return Err(format!("class address decoded as {other:?}")),
    };
    let constructor = unpickler
        .index()
        .symbol_at(constructor_address)
        .ok_or_else(|| format!("constructor DEFDEF@{constructor_address} has no symbol"))?;
    if !matches!(
        unpickler.symbol_state_at(constructor_address),
        Some((SymbolKind::Constructor, SymbolInfo::Missing))
    ) {
        return Err(format!(
            "constructor DEFDEF@{constructor_address} was not pending before explicit completion"
        ));
    }
    let constructor_type = unpickler
        .complete_symbol(constructor_address)
        .map_err(|error| format!("complete constructor DEFDEF@{constructor_address}: {error:?}"))?;
    if !matches!(
        unpickler.symbol_state_at(constructor_address),
        Some((SymbolKind::Constructor, SymbolInfo::Complete(_)))
    ) {
        return Err(format!(
            "constructor DEFDEF@{constructor_address} is not complete"
        ));
    }
    drop(unpickler);

    let class_info = match store.types.get(class_type) {
        Type::ClassInfo(info) => info,
        other => return Err(format!("class result is not ClassInfo: {other:?}")),
    };
    let method = match store.types.get(constructor_type) {
        Type::Method(method) => method,
        other => return Err(format!("constructor result is not Method: {other:?}")),
    };
    if store.symbols.get(constructor).owner != Some(class) {
        return Err("constructor owner is not the selected class".to_owned());
    }
    let object_parent =
        class_info
            .parents
            .iter()
            .find_map(|parent| match store.types.get(*parent) {
                Type::TypeRef { target, .. } => target.symbol(),
                _ => None,
            });
    if object_parent != Some(definitions.object_class) {
        return Err(format!(
            "constructor class parent is not canonical java.lang.Object: {object_parent:?}"
        ));
    }
    let result_class = match store.types.get(method.result) {
        Type::TypeRef { target, .. } => target.symbol(),
        other => {
            return Err(format!(
                "constructor result is not a class reference: {other:?}"
            ));
        }
    };
    if result_class != Some(class) {
        return Err(format!(
            "constructor result does not retain entered class identity: {result_class:?}"
        ));
    }
    let parents = class_info
        .parents
        .iter()
        .map(|id| type_repr(&store, *id))
        .collect::<Vec<_>>()
        .join("&");
    let result = type_repr(&store, method.result);
    Ok(format!(
        "fixture={unit_id}\tclass_address={class_address}\tconstructor_address={constructor_address}\tclass={}\tclass_kind={:?}\tclass_visibility={}\tclass_flags={}\tclass_modifiers={class_tail}\tparents={parents}\tconstructor={}\tconstructor_owner={}\tconstructor_kind={:?}\tconstructor_visibility={}\tconstructor_modifiers={constructor_tail}\tserialized_clause_count={serialized_clause_count}\tconstructor_signature={}\tserialized_result={serialized_result}\teffective_result={result}",
        symbol_path(&store, class),
        store.symbols.get(class).kind,
        visibility(&store, &store.symbols.get(class).visibility),
        flags(store.symbols.get(class).flags),
        symbol_path(&store, constructor),
        symbol_path(
            &store,
            store
                .symbols
                .get(constructor)
                .owner
                .ok_or_else(|| "constructor has no owner".to_owned())?
        ),
        store.symbols.get(constructor).kind,
        visibility(&store, &store.symbols.get(constructor).visibility),
        type_repr(&store, constructor_type)
    ))
}

#[test]
fn semantic_negative_controls_use_original_bytes_and_resolver_transactions() {
    use dotty_core::{SymbolInfo, SymbolKind};
    use dotty_tasty_unpickler::tasty_unpickler::UnpickleError;

    for unit_id in ["visibility-open", "visibility-private"] {
        let unit = units()
            .into_iter()
            .find(|unit| unit.id == unit_id)
            .expect("selected input is in frozen manifest");
        let bytes = fs::read(&unit.path).expect("selected original fixture bytes");
        let file = TastyFile::parse(&bytes).expect("selected original TASTy parses");
        let simple_name = unit.lookup.rsplit('/').next().unwrap();

        // Present each selected original TASTy to the resolver-backed semantic
        // completion path without JDK modules. Completion must report a typed
        // resolution error and roll back all dependency and class mutations.
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let resolver = ClasspathSymbolResolver::new(
            classpath_without_jdk(),
            definitions,
            LoadingSession::new(),
        );
        let session = TastySession::new()
            .with_resolver(Box::new(resolver))
            .expect("resolver attaches to a fresh TASTy session");
        let mut unpickler = TastyUnpickler::with_session(&file, &mut store, definitions, session);
        unpickler.enter_symbols().expect("enter selected symbols");
        let class_address = root_definition_address(&file, &unpickler, simple_name)
            .expect("selected root TYPEDEF exists");
        let class = unpickler.index().symbol_at(class_address).unwrap();
        let types_before_completion = unpickler.index().type_count();
        let failure = unpickler
            .complete_symbol(class_address)
            .expect_err("class completion without JDK Object must fail closed");
        assert!(
            matches!(
                failure,
                UnpickleError::ResolverFailure { .. } | UnpickleError::UnresolvedPackage { .. }
            ),
            "missing Object must produce a typed resolver failure, got {failure:?}"
        );
        assert_eq!(unpickler.index().type_count(), types_before_completion);
        assert_eq!(
            unpickler.symbol_state_at(class_address),
            Some((SymbolKind::Class, SymbolInfo::Missing)),
            "failed semantic class completion restores the entered class"
        );
        drop(unpickler);
        assert_eq!(
            store.symbols.get(class).info,
            SymbolInfo::Missing,
            "failed completion must not publish partial class info"
        );

        // A requested binary owner is checked against the actual symbol
        // entered from the original bytes; this specifically prevents the old
        // absent-file lookup from standing in for an owner-mismatch control.
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let resolver =
            ClasspathSymbolResolver::new(classpath(), definitions, LoadingSession::new());
        let session = TastySession::new()
            .with_resolver(Box::new(resolver))
            .expect("resolver attaches to a fresh TASTy session");
        let mut unpickler = TastyUnpickler::with_session(&file, &mut store, definitions, session);
        unpickler.enter_symbols().expect("enter selected symbols");
        let class_address = root_definition_address(&file, &unpickler, simple_name)
            .expect("selected root TYPEDEF exists");
        let class = unpickler.index().symbol_at(class_address).unwrap();
        let class_state = unpickler.symbol_state_at(class_address);
        drop(unpickler);
        let actual_owner = symbol_path(&store, class);
        let wrong_owner = format!("me.cytrowski.tastyfixtures.elsewhere.{simple_name}");
        let outcome = validate_requested_owner(&wrong_owner, &actual_owner);
        assert_eq!(
            outcome,
            Err(RequestedOwnerMismatch {
                requested: wrong_owner,
                actual: actual_owner,
            }),
            "typed requested-owner check must reject the original class identity"
        );
        assert_eq!(class_state, Some((SymbolKind::Class, SymbolInfo::Missing)));
        assert_eq!(
            store.symbols.get(class).info,
            SymbolInfo::Missing,
            "owner mismatch check does not complete or mutate the original class"
        );
    }
}

fn load_visibility_legacy_once(unit_id: &str) -> String {
    use dotty_core::{SymbolInfo, SymbolKind};

    let unit = units()
        .into_iter()
        .find(|unit| unit.id == unit_id)
        .unwrap_or_else(|| panic!("missing frozen unit {unit_id}"));
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut loader =
        ClassLoader::with_definitions(classpath(), &mut store, definitions, LoadingSession::new());
    let requested = BinaryName::from_internal(unit.lookup);
    match loader.load_class(&requested) {
        Ok(class) => {
            let object_class = loader
                .load_class(&BinaryName::from_internal("java/lang/Object"))
                .expect("canonical JDK Object dependency remains loaded");
            let origin = format!("{:?}", loader.origin(class));
            drop(loader);
            let class_info_type = match store.symbols.get(class).info {
                SymbolInfo::Complete(class_info_type) => class_info_type,
                other => panic!("legacy class is not complete: {other:?}"),
            };
            let class_info = match store.types.get(class_info_type) {
                Type::ClassInfo(info) => info,
                other => panic!("legacy class info has unexpected type: {other:?}"),
            };
            let parent_symbols = class_info
                .parents
                .iter()
                .filter_map(|parent| match store.types.get(*parent) {
                    Type::TypeRef { target, .. } => target.symbol(),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                parent_symbols,
                vec![object_class],
                "legacy parent must retain the resolver's canonical JDK Object identity"
            );
            let constructors = store
                .scopes
                .get(class_info.declarations)
                .entered_symbols()
                .filter(|symbol| store.symbols.get(*symbol).kind == SymbolKind::Constructor)
                .collect::<Vec<_>>();
            assert_eq!(constructors.len(), 1, "legacy class has one constructor");
            let constructor_type = match store.symbols.get(constructors[0]).info {
                SymbolInfo::Complete(ty) => ty,
                other => panic!("legacy constructor is not complete: {other:?}"),
            };
            let method = match store.types.get(constructor_type) {
                Type::Method(method) => method,
                other => panic!("legacy constructor type is not a method: {other:?}"),
            };
            assert_eq!(
                method.result, definitions.unit,
                "legacy constructor must point at the canonical Unit TypeId"
            );
            format!(
                "outcome=CLASS_LOADED\torigin={origin}\t{}",
                normalized_symbol(&store, class)
            )
        }
        Err(error) => format!(
            "outcome=ERROR\terror={:?}",
            normalize_allocation_ids(&format!("{error:?}"))
        ),
    }
}

#[test]
fn selected_visibility_classes_and_primary_constructors_are_explicitly_completed() {
    let mut receipt = String::from(
        "fixture\tclass_address\tconstructor_address\tclass\tclass_kind\tclass_visibility\tclass_flags\tparents\tconstructor\tconstructor_owner\tconstructor_kind\tconstructor_visibility\tconstructor_signature\tserialized_result\teffective_result\n",
    );
    for unit_id in ["visibility-open", "visibility-private"] {
        let first = complete_visibility_constructor_once(unit_id).unwrap_or_else(|error| {
            format!("fixture={unit_id}\toutcome=UNSUPPORTED\terror={error}")
        });
        let second = complete_visibility_constructor_once(unit_id).unwrap_or_else(|error| {
            format!("fixture={unit_id}\toutcome=UNSUPPORTED\terror={error}")
        });
        assert_eq!(first, second, "fresh sessions differ for {unit_id}");
        let legacy_first = load_visibility_legacy_once(unit_id);
        assert_eq!(
            legacy_first,
            load_visibility_legacy_once(unit_id),
            "legacy results differ across fresh sessions for {unit_id}"
        );
        assert!(legacy_first.contains("outcome=CLASS_LOADED"));
        assert!(legacy_first.contains("origin=Some(Directory("));
        assert!(legacy_first.contains("tests/fixtures/tasty_visibility\"))"));
        assert!(legacy_first.contains("parameter_clauses=<init>:Constructor:public:()Plain:Unit"));
        assert!(legacy_first.contains("hierarchy=java.lang.Object"));
        assert!(first.contains("constructor_owner=me.cytrowski.tastyfixtures.visibility."));
        assert!(first.contains("constructor_visibility=public"));
        assert!(
            first.contains("constructor_signature=()Plain:me.cytrowski.tastyfixtures.visibility.")
        );
        assert!(first.contains("parents=java.lang.Object"));
        assert!(first.contains("serialized_result=scala.Unit"));
        assert!(first.contains("effective_result=me.cytrowski.tastyfixtures.visibility."));
        match unit_id {
            "visibility-open" => assert!(first.contains("class_visibility=public")),
            "visibility-private" => {
                assert!(first.contains(
                    "class_visibility=private-within:me.cytrowski.tastyfixtures.visibility"
                ))
            }
            _ => unreachable!(),
        }
        receipt.push_str(&first);
        receipt.push('\n');
    }
    if std::env::var_os("DOTTY_DUMP_VISIBILITY_AUDIT").is_some() {
        eprintln!("{receipt}");
    }
}

#[test]
fn neighboring_non_empty_constructor_is_explicitly_completed_as_a_bounded_probe() {
    use dotty_core::{SymbolInfo, SymbolKind};
    use dotty_tasty::tasty::{DEFDEF_TAG, StructuredNode};

    let unit = units()
        .into_iter()
        .find(|unit| unit.id == "fields-body")
        .expect("bounded neighboring fixture is in the frozen cohort");
    let bytes = fs::read(&unit.path).expect("frozen Body bytes");
    let file = TastyFile::parse(&bytes).expect("frozen Body TASTy parses");
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let resolver = ClasspathSymbolResolver::new(classpath(), definitions, LoadingSession::new());
    let session = TastySession::new()
        .with_resolver(Box::new(resolver))
        .expect("resolver attaches to a fresh TASTy session");
    let mut unpickler = TastyUnpickler::with_session(&file, &mut store, definitions, session);
    unpickler.enter_symbols().expect("enter Body symbols");
    let class_address = root_definition_address(&file, &unpickler, "Body").expect("Body TYPEDEF");
    unpickler
        .complete_symbol(class_address)
        .expect("complete Body class before its constructor");
    let body_class = unpickler.index().symbol_at(class_address).unwrap();
    let ast = file.ast_address_index().expect("Body address index");
    let constructors = ast
        .iter_nodes()
        .filter(|node| node.tag == DEFDEF_TAG)
        .filter_map(|node| u32::try_from(node.offset).ok())
        .filter(|address| {
            matches!(
                unpickler.symbol_state_at(*address),
                Some((SymbolKind::Constructor, _))
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(constructors.len(), 1, "Body has one primary constructor");
    let constructor_address = constructors[0];
    let constructor_node = ast
        .get(constructor_address)
        .unwrap()
        .decode_structured()
        .unwrap();
    let StructuredNode::DefDef(body) = constructor_node else {
        panic!("Body constructor is not a DEFDEF")
    };
    assert_eq!(
        body.parameters.len(),
        1,
        "Body constructor has one parameter"
    );
    let result = unpickler
        .complete_symbol(constructor_address)
        .expect("bounded one-parameter constructor completes");
    assert_eq!(
        unpickler.symbol_state_at(constructor_address),
        Some((SymbolKind::Constructor, SymbolInfo::Complete(result))),
        "explicit completion records the constructor method type"
    );
    drop(unpickler);
    let method = match store.types.get(result) {
        Type::Method(method) => method,
        other => panic!("Body constructor did not produce a method type: {other:?}"),
    };
    assert_eq!(method.params.len(), 1);
    assert_eq!(type_repr(&store, method.params[0].ty), "Int");
    let result_class = match store.types.get(method.result) {
        Type::TypeRef { target, .. } => target.symbol(),
        other => panic!("Body constructor result is not a class reference: {other:?}"),
    };
    assert_eq!(result_class, Some(body_class));
    if std::env::var_os("DOTTY_DUMP_VISIBILITY_AUDIT").is_some() {
        eprintln!(
            "probe=fields-body\tconstructor_address={constructor_address}\tparameter_count={}\tresult=SUPPORTED(one Int parameter, owning class result)",
            body.parameters.len()
        );
    }
}

#[test]
fn selected_visibility_loads_fail_closed_for_wrong_owner_and_missing_object() {
    for unit_id in ["visibility-open", "visibility-private"] {
        let unit = units()
            .into_iter()
            .find(|unit| unit.id == unit_id)
            .expect("selected input is in frozen manifest");
        let requested = BinaryName::from_internal(unit.lookup);

        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let checkpoint = store.checkpoint();
        let failure = {
            let mut loader = ClassLoader::with_definitions(
                classpath_without_jdk(),
                &mut store,
                definitions,
                LoadingSession::new(),
            );
            loader
                .load_class(&requested)
                .expect_err("without the JDK, java.lang.Object must remain unresolved")
        };
        let error = format!("{failure:?}");
        assert!(
            error.contains("NotFound") || error.contains("DependencyFailure"),
            "missing java.lang.Object must be an explicit classpath error: {error}"
        );
        assert_eq!(
            store.checkpoint(),
            checkpoint,
            "failed dependency load must not leave a partial {unit_id}"
        );

        let wrong_owner = if unit_id == "visibility-open" {
            "me/cytrowski/tastyfixtures/elsewhere/Open"
        } else {
            "me/cytrowski/tastyfixtures/elsewhere/PlainPrivate"
        };
        let failure = {
            let mut loader = ClassLoader::with_definitions(
                classpath(),
                &mut store,
                definitions,
                LoadingSession::new(),
            );
            loader
                .load_class(&BinaryName::from_internal(wrong_owner))
                .expect_err("wrong requested package/owner must not alias the fixture")
        };
        let error = format!("{failure:?}");
        assert!(error.contains("NotFound"), "wrong owner error: {error}");
        assert_eq!(
            store.checkpoint(),
            checkpoint,
            "wrong-owner lookup must not leave a partial {unit_id}"
        );

        let bytes = fs::read(&unit.path).expect("selected original fixture bytes");
        assert!(
            TastyFile::parse(&bytes[..bytes.len() - 1]).is_err(),
            "truncated {unit_id} must be rejected as malformed"
        );
        assert_eq!(store.checkpoint(), checkpoint);
    }
}

#[test]
fn frozen_open_constructor_is_explicitly_completed_with_real_dependencies() {
    let first = complete_open_constructor_once();
    let second = complete_open_constructor_once();
    assert_eq!(
        first, second,
        "fresh resolver-backed sessions normalize identically"
    );
    assert!(
        first
            .graph
            .contains("result=me.cytrowski.tastyfixtures.visibility.Open")
    );
    if std::env::var_os("DOTTY_DUMP_OPEN_CONSTRUCTOR").is_some() {
        println!("{first:?}");
    }
}

#[test]
fn polymorphic_parameter_order_bounds_variance_and_errors_are_normalized() {
    let mut store = SemanticStore::new();
    let lower = store.types.alloc(Type::NoType);
    let error_name = store.names.intern("unsupported bound");
    let error = store.types.alloc(Type::Error(dotty_core::ErrorType {
        message: error_name,
    }));
    let other_bound = store.types.alloc(Type::NoPrefix);
    let bounded = store.types.alloc(Type::Bounds {
        low: lower,
        high: error,
    });
    let distinct = store.types.alloc(Type::Bounds {
        low: lower,
        high: other_bound,
    });
    let first = TypeParam {
        name: TypeName::new(store.names.intern("A")),
        bounds: bounded,
        declared_variance: Some(Variance::Covariant),
    };
    let second = TypeParam {
        name: TypeName::new(store.names.intern("B")),
        bounds: distinct,
        declared_variance: Some(Variance::Contravariant),
    };
    let ordered = store.types.alloc(Type::Poly(PolyType {
        params: vec![first, second],
        result: lower,
    }));
    let reversed = store.types.alloc(Type::Poly(PolyType {
        params: vec![second, first],
        result: lower,
    }));
    let same_parameter_different_bound = TypeParam {
        bounds: distinct,
        ..first
    };
    let changed_bound = store.types.alloc(Type::Poly(PolyType {
        params: vec![same_parameter_different_bound],
        result: lower,
    }));
    let original_bound = store.types.alloc(Type::Poly(PolyType {
        params: vec![first],
        result: lower,
    }));

    let ordered_repr = type_repr(&store, ordered);
    let reversed_repr = type_repr(&store, reversed);
    assert_ne!(ordered_repr, reversed_repr, "parameter order is semantic");
    assert_ne!(
        type_repr(&store, original_bound),
        type_repr(&store, changed_bound),
        "distinct bounds remain observable for otherwise identical parameters"
    );
    assert!(ordered_repr.contains("A:Some(Covariant):bounds(NoType,Error("));
    assert!(ordered_repr.contains("B:Some(Contravariant):bounds(NoType,NoPrefix)"));
    assert_eq!(
        field_outcome("UNPICKLER", &ordered_repr),
        Some("UNPICKLER_UNSUPPORTED"),
        "an error nested in a parameter bound must remain unsupported"
    );
}

#[test]
fn equivalent_failures_with_independent_type_ids_normalize_identically() {
    let mut first_store = SemanticStore::new();
    let first_prefix = first_store.types.alloc(Type::NoType);
    let mut second_store = SemanticStore::new();
    let _unrelated = second_store.types.alloc(Type::NoPrefix);
    let second_prefix = second_store.types.alloc(Type::NoType);
    assert_ne!(first_prefix, second_prefix);

    let first =
        format!("UnresolvedMember {{ address: 1699, prefix: {first_prefix:?}, name: \"Null\" }}");
    let second =
        format!("UnresolvedMember {{ address: 1699, prefix: {second_prefix:?}, name: \"Null\" }}");
    assert_eq!(
        normalize_allocation_ids(&first),
        normalize_allocation_ids(&second),
        "equivalent semantic failures must compare independently of arena IDs"
    );
    assert_eq!(
        normalize_allocation_ids(&first),
        "UnresolvedMember { address: 1699, prefix: TypeId(*), name: \"Null\" }"
    );
}

#[test]
fn malformed_and_missing_dependency_failures_preserve_store_and_allow_retry() {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let bootstrapped = store.checkpoint();

    let valid = fs::read(
        units()
            .into_iter()
            .find(|unit| unit.id == "visibility-open")
            .expect("frozen real TASTy fixture")
            .path,
    )
    .expect("frozen fixture bytes");
    let truncated = TastyFile::parse(&valid[..valid.len() - 1]);
    assert!(
        truncated.is_err(),
        "a one-byte truncation of a real fixture must be rejected"
    );
    assert_eq!(store.checkpoint(), bootstrapped);

    let mut session = LoadingSession::new();
    let missing_dependency = {
        let mut loader = ClassLoader::with_definitions(
            classpath_without_jdk(),
            &mut store,
            definitions,
            session,
        );
        let result = loader.load_class(&BinaryName::from_internal("Dog"));
        session = loader.into_session();
        result
    };
    let error = missing_dependency.expect_err("Dog's java.lang.Object dependency is absent");
    assert!(
        format!("{error:?}").contains("NotFound")
            || format!("{error:?}").contains("DependencyFailure"),
        "missing dependency must remain explicit: {error:?}"
    );
    assert_eq!(
        store.checkpoint(),
        bootstrapped,
        "failed dependency loading must roll back semantic allocations"
    );

    let recovered = {
        let mut loader =
            ClassLoader::with_definitions(classpath(), &mut store, definitions, session);
        loader.load_class(&BinaryName::from_internal(
            "me/cytrowski/tastyfixtures/visibility/Open",
        ))
    }
    .expect("a fresh classpath can complete valid work after the failed load");
    assert_eq!(
        symbol_path(&store, recovered),
        "me.cytrowski.tastyfixtures.visibility.Open"
    );
    assert_ne!(store.checkpoint(), bootstrapped);
}
