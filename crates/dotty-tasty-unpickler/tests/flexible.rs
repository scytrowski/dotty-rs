//! Milestone 2c2: `FLEXIBLEtype`, over the real Scala 3.9.0 `Flex.tasty`
//! (`tests/fixtures/semantic/explicit_nulls/Flex.scala`, compiled with
//! `-Yexplicit-nulls`).
//!
//! The compiler wrote one `FLEXIBLEtype` (at 109) for `System.getProperty`,
//! whose child (111) is the name-based `java.lang.String`. The unit does not
//! define it, so tests that need it decoded enter `java.lang` and a `String`
//! class into the package registry first.
use dotty_core::ids::TypeId;
use dotty_core::names::{Name, Namespace};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{
    Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
};
use dotty_core::types::Type;
use dotty_core::{Definitions, Packages};
use dotty_tasty::tasty::TastyFile;
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

const FLEX: &[u8] = include_bytes!("fixtures/semantic/Flex.tasty");

const FLEXIBLE: u32 = 109;
const CHILD: usize = 111;
/// A `SHAREDtype` link to the child.
const SHARED_CHILD: u32 = 133;

struct Session {
    store: SemanticStore,
    definitions: Definitions,
}

impl Session {
    fn new() -> Self {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        Self { store, definitions }
    }
}

/// A session whose registry already holds `java.lang` with a class `String`,
/// which the unit references but does not define.
fn session_with_string() -> (Session, Packages) {
    let mut session = Session::new();
    let mut packages = Packages::new();
    let chain = packages.enter(
        &mut session.store,
        SymbolOrigin::Synthetic,
        &["java", "lang"],
    );
    let java_lang = chain.last().unwrap();
    let name = Name::new(session.store.names.intern("String"), Namespace::Type);
    let string = session.store.symbols.alloc(Symbol {
        name,
        owner: Some(java_lang.symbol),
        kind: SymbolKind::Class,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Public,
        info: SymbolInfo::Missing,
        origin: SymbolOrigin::Synthetic,
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    });
    session
        .store
        .scopes
        .get_mut(java_lang.scope)
        .enter(name, string);
    (session, packages)
}

fn name_of(store: &SemanticStore, id: TypeId) -> String {
    let Some(symbol) = store.types.get(id).reference_symbol() else {
        panic!("expected a type reference, got {:?}", store.types.get(id));
    };
    store
        .names
        .resolve(store.symbols.get(symbol).name.text())
        .to_string()
}

#[test]
fn a_flexible_type_is_a_wrapper_around_its_underlying_type() {
    let file = TastyFile::parse_scala_3_9(FLEX).unwrap();
    let (mut session, packages) = session_with_string();
    let mut unpickler =
        TastyUnpickler::with_packages(&file, &mut session.store, session.definitions, packages);
    unpickler.enter_symbols().unwrap();
    let id = unpickler.unpickle_type(FLEXIBLE).unwrap();
    let child = unpickler.unpickle_type(CHILD as u32).unwrap();
    drop(unpickler);

    assert_eq!(
        session.store.types.get(id),
        &Type::Flexible { underlying: child }
    );
    assert_ne!(id, child);
    assert_eq!(name_of(&session.store, child), "String");
}

#[test]
fn decoding_a_flexible_type_twice_returns_the_same_id() {
    let file = TastyFile::parse_scala_3_9(FLEX).unwrap();
    let (mut session, packages) = session_with_string();
    let mut unpickler =
        TastyUnpickler::with_packages(&file, &mut session.store, session.definitions, packages);
    unpickler.enter_symbols().unwrap();
    let first = unpickler.unpickle_type(FLEXIBLE).unwrap();

    assert_eq!(unpickler.unpickle_type(FLEXIBLE), Ok(first));
}

#[test]
fn a_shared_link_to_the_underlying_type_gets_the_wrapped_id_not_the_wrapper() {
    let file = TastyFile::parse_scala_3_9(FLEX).unwrap();
    let (mut session, packages) = session_with_string();
    let mut unpickler =
        TastyUnpickler::with_packages(&file, &mut session.store, session.definitions, packages);
    unpickler.enter_symbols().unwrap();
    let flexible = unpickler.unpickle_type(FLEXIBLE).unwrap();
    let shared = unpickler.unpickle_type(SHARED_CHILD).unwrap();
    drop(unpickler);

    let Type::Flexible { underlying } = session.store.types.get(flexible) else {
        panic!("not flexible");
    };
    assert_eq!(shared, *underlying);
}

#[test]
fn an_unresolved_underlying_type_is_reported_and_rolls_back() {
    let file = TastyFile::parse_scala_3_9(FLEX).unwrap();
    let mut session = Session::new();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    unpickler.enter_symbols().unwrap();
    let before = unpickler.index().type_count();

    assert!(matches!(
        unpickler.unpickle_type(FLEXIBLE),
        Err(UnpickleError::UnresolvedPackage { .. })
    ));
    assert_eq!(unpickler.index().type_count(), before);
    assert_eq!(unpickler.index().type_at(FLEXIBLE), None);
}

/// A file whose ASTs are exactly `ast`, over the names `ASTs`, `p`, `C`.
fn file_with_ast(ast: &[u8]) -> Vec<u8> {
    let names = dotty_tasty::tasty::NameTable::from_entries(
        ["ASTs", "p", "C"]
            .map(|text| dotty_tasty::tasty::RawName::Utf8(text.to_owned()))
            .to_vec(),
    )
    .unwrap();
    let sections =
        dotty_tasty::tasty::SectionTable::from_sections(vec![dotty_tasty::tasty::Section::new(
            0, ast,
        )]);
    TastyFile::from_parts(
        dotty_tasty::tasty::Header {
            major_version: 28,
            minor_version: 9,
            experimental_version: 0,
            tooling_version: "Scala 3.9.0".to_owned(),
            uuid: [0; 16],
        },
        names,
        sections,
    )
    .unwrap()
    .encode()
    .unwrap()
}

/// `TYPEREF C prefix` where the prefix is `prefix_tag Length (TYPEREFpkg p)`,
/// itself inside an outer `FLEXIBLEtype` so the node is at address 2.
fn reference_to_c_through(prefix_tag: u8) -> Vec<u8> {
    let prefix = [prefix_tag, 0x82, 65, 0x81];
    let mut inner = vec![117, 0x82];
    inner.extend(prefix);
    let mut outer = vec![193, 0x80 | inner.len() as u8];
    outer.extend(inner);
    file_with_ast(&outer)
}

fn session_with_p_c() -> (Session, Packages) {
    let mut session = Session::new();
    let mut packages = Packages::new();
    let chain = packages.enter(&mut session.store, SymbolOrigin::Synthetic, &["p"]);
    let package = chain.last().unwrap();
    let name = Name::new(session.store.names.intern("C"), Namespace::Type);
    let class = session.store.symbols.alloc(Symbol {
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
    session
        .store
        .scopes
        .get_mut(package.scope)
        .enter(name, class);
    (session, packages)
}

#[test]
fn a_named_reference_through_a_flexible_prefix_is_found_in_the_wrapped_prefix() {
    // `TYPEREF C (FLEXIBLEtype (TYPEREFpkg p))`: `C` is a member of package
    // `p`, which the wrapper must not hide.
    let bytes = reference_to_c_through(193);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let (mut session, packages) = session_with_p_c();
    let mut unpickler =
        TastyUnpickler::with_packages(&file, &mut session.store, session.definitions, packages);
    let id = unpickler.unpickle_type(2).unwrap();
    drop(unpickler);
    let Type::TypeRef { prefix, target } = session.store.types.get(id) else {
        panic!("expected a type reference");
    };
    let symbol = &target.symbol().unwrap();
    assert!(matches!(
        session.store.types.get(*prefix),
        Type::Flexible { .. }
    ));
    let name = session.store.symbols.get(*symbol).name.text();
    assert_eq!(session.store.names.resolve(name), "C");
}
