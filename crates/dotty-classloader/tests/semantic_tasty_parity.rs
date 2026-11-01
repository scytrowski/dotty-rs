//! Frozen, test-local comparison of the legacy TASTy loader and canonical
//! semantic unpickler. The cohort is checked in before these adapters run.
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    assert_eq!(
        first, expected,
        "checked-in #1948 per-field measurement is stale"
    );
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
