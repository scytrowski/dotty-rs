//! Issue #90: `_root_` as a package-path segment, both as a lone reference
//! (`TERMREFpkg _root_`, the root package itself) and as the leading segment
//! of a qualified name (`_root_.p`, entered exactly like `<root>.p` or `p`),
//! on synthetic units so the wire shapes are written exactly.

use dotty_core::Definitions;
use dotty_core::ids::SymbolId;
use dotty_core::store::SemanticStore;
use dotty_core::symbols::SymbolKind;
use dotty_core::types::Type;
use dotty_tasty::tasty::{
    Header, NameTable, PACKAGE_TAG, RawName, Section, SectionTable, TERMREFPKG_TAG, TastyFile,
};
use dotty_tasty_unpickler::tasty_unpickler::TastyUnpickler;

const ASTS: u32 = 0;
const EMPTY: u32 = 1;
const ROOT_SPELLING: u32 = 2;
const P: u32 = 3;
const ROOT_DOT_P: u32 = 4;

fn names() -> NameTable {
    NameTable::from_entries(vec![
        RawName::Utf8("ASTs".to_owned()),
        RawName::Utf8("<empty>".to_owned()),
        RawName::Utf8("_root_".to_owned()),
        RawName::Utf8("p".to_owned()),
        RawName::Qualified {
            prefix: ROOT_SPELLING,
            selector: P,
        },
    ])
    .unwrap()
}

fn nat(value: u32) -> Vec<u8> {
    let mut groups = vec![u8::try_from(value & 0x7f).unwrap() | 0x80];
    let mut rest = value >> 7;
    while rest > 0 {
        groups.push(u8::try_from(rest & 0x7f).unwrap());
        rest >>= 7;
    }
    groups.reverse();
    groups
}

fn node(tag: u8, payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![tag];
    bytes.extend(nat(u32::try_from(payload.len()).unwrap()));
    bytes.extend(payload);
    bytes
}

fn leaf(tag: u8, value: u32) -> Vec<u8> {
    let mut bytes = vec![tag];
    bytes.extend(nat(value));
    bytes
}

/// A unit whose own path names `outer` (the root package as `<empty>` or as
/// `_root_`), with one nested `PACKAGE` whose path names `nested`.
fn unit_with_paths(outer: u32, nested: u32) -> Vec<u8> {
    let inner = node(PACKAGE_TAG, &leaf(TERMREFPKG_TAG, nested));
    let ast = node(PACKAGE_TAG, &[leaf(TERMREFPKG_TAG, outer), inner].concat());
    TastyFile::from_parts(
        Header {
            major_version: 28,
            minor_version: 9,
            experimental_version: 0,
            tooling_version: "Scala 3.9.0".to_owned(),
            uuid: [0; 16],
        },
        names(),
        SectionTable::from_sections(vec![Section::new(ASTS, &ast)]),
    )
    .unwrap()
    .encode()
    .unwrap()
}

/// A unit in the (`<empty>`-spelled) root package with one nested `PACKAGE`
/// whose path names `nested`.
fn unit(nested: u32) -> Vec<u8> {
    unit_with_paths(EMPTY, nested)
}

fn session() -> (SemanticStore, Definitions) {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    (store, definitions)
}

fn text(store: &SemanticStore, symbol: SymbolId) -> &str {
    store.names.resolve(store.symbols.get(symbol).name.text())
}

#[test]
fn a_nested_package_whose_path_is_root_dot_p_enters_like_a_bare_p() {
    let (mut store, definitions) = session();
    let bytes = unit(ROOT_DOT_P);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut unpickler = TastyUnpickler::new(&file, &mut store, definitions);
    unpickler.enter_symbols().unwrap();
    let (_, packages) = unpickler.into_parts();

    let root = packages.symbol::<&str>(&[]).expect("the root");
    let p = packages.symbol(&["p"]).expect("p entered under the root");
    assert_eq!(store.symbols.get(p).kind, SymbolKind::Package);
    assert_eq!(store.symbols.get(p).owner, Some(root));
    assert_eq!(text(&store, p), "p");
    // No symbol was made for the source spelling itself.
    assert_eq!(packages.symbol(&["_root_"]), None);
    assert_eq!(packages.symbol(&["_root_", "p"]), None);
}

#[test]
fn root_dot_p_and_a_bare_p_enter_the_same_symbol_across_units() {
    let (mut store, definitions) = session();
    let bytes_p = unit(P);
    let file_p = TastyFile::parse_scala_3_9(&bytes_p).unwrap();
    let mut unpickler = TastyUnpickler::new(&file_p, &mut store, definitions);
    unpickler.enter_symbols().unwrap();
    let (_, packages) = unpickler.into_parts();
    let plain = packages.symbol(&["p"]).unwrap();

    let bytes_root_dot_p = unit(ROOT_DOT_P);
    let file_root_dot_p = TastyFile::parse_scala_3_9(&bytes_root_dot_p).unwrap();
    let mut unpickler =
        TastyUnpickler::with_packages(&file_root_dot_p, &mut store, definitions, packages);
    unpickler.enter_symbols().unwrap();
    let (_, packages) = unpickler.into_parts();
    let via_root = packages.symbol(&["p"]).unwrap();

    assert_eq!(plain, via_root);
}

#[test]
fn a_bare_root_reference_resolves_to_the_session_root() {
    // The unit's own path (address 2, the first child of the outer
    // `PACKAGE`) is spelled `_root_` instead of `<empty>`.
    let (mut store, definitions) = session();
    let bytes = unit_with_paths(ROOT_SPELLING, P);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let index = file.ast_address_index().unwrap();
    let at = index
        .iter_nodes()
        .filter(|found| found.tag == TERMREFPKG_TAG)
        .map(|found| u32::try_from(found.offset).unwrap())
        .min()
        .unwrap();

    let mut unpickler = TastyUnpickler::new(&file, &mut store, definitions);
    unpickler.enter_symbols().unwrap();
    let ty = unpickler.unpickle_type(at).unwrap();
    let root = unpickler.index().symbol_at(0).unwrap();
    drop(unpickler);
    let Type::TermRef { target, .. } = store.types.get(ty) else {
        panic!("expected a TermRef");
    };
    assert_eq!(target.symbol().unwrap(), root);
    assert_eq!(store.symbols.get(root).kind, SymbolKind::Package);
    assert_eq!(store.symbols.get(root).owner, None);
}
