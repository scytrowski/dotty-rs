//! A `PACKAGE` whose path is a `SHAREDtype` link (issue #29), over the real
//! `scala3-library/scala/package.tasty`.
//!
//! The unit has two `PACKAGE` nodes: `@0` over `<empty>`, and a nested one at
//! `@21` whose path is not written out but linked to the `TERMREFpkg("scala")`
//! leaf at address 9, which sits inside an earlier import.

use dotty_core::Definitions;
use dotty_core::store::SemanticStore;
use dotty_core::symbols::SymbolKind;
use dotty_tasty::tasty::{StandardSection, TastyFile};
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

const PACKAGE_UNIT: &[u8] =
    include_bytes!("../../dotty-tasty/tests/fixtures/scala3-library/scala/package.tasty");

const OUTER_PACKAGE: u32 = 0;
const NESTED_PACKAGE: u32 = 21;
/// Where the nested package's path leaf is: `SHAREDtype(9)`, two bytes.
const LINK_AT: usize = 24;

fn enter(
    bytes: &[u8],
) -> Result<
    (
        SemanticStore,
        dotty_tasty_unpickler::tasty_unpickler::TastySemanticIndex,
    ),
    UnpickleError,
> {
    let file = TastyFile::parse_compatible_with(bytes, 28, 9, 0).unwrap();
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut unpickler = TastyUnpickler::new(&file, &mut store, definitions);
    unpickler.enter_symbols()?;
    let index = unpickler.into_index();
    Ok((store, index))
}

/// The unit with the address of the link at [`LINK_AT`] replaced.
fn linked_to(address: u8) -> Vec<u8> {
    let file = TastyFile::parse_compatible_with(PACKAGE_UNIT, 28, 9, 0).unwrap();
    let payload = file.section(StandardSection::Asts).unwrap().payload;
    let start = payload.as_ptr() as usize - PACKAGE_UNIT.as_ptr() as usize;
    assert_eq!(
        payload[LINK_AT..LINK_AT + 2],
        [61, 0x80 | 9],
        "the fixture no longer has SHAREDtype(9) at {LINK_AT}"
    );
    let mut bytes = PACKAGE_UNIT.to_vec();
    bytes[start + LINK_AT + 1] = 0x80 | address;
    bytes
}

fn name_of(store: &SemanticStore, symbol: dotty_core::ids::SymbolId) -> &str {
    store.names.resolve(store.symbols.get(symbol).name.text())
}

#[test]
fn the_unit_with_a_shared_package_path_enters() {
    let (store, index) = enter(PACKAGE_UNIT).unwrap();

    let outer = index.symbol_at(OUTER_PACKAGE).unwrap();
    let nested = index.symbol_at(NESTED_PACKAGE).unwrap();
    assert_eq!(store.symbols.get(nested).kind, SymbolKind::Package);
    assert_eq!(name_of(&store, nested), "scala");
    assert_eq!(name_of(&store, outer), "<empty>");
}

#[test]
fn a_chain_of_shared_links_is_followed() {
    // Address 17 is itself `SHAREDtype(9)`, so this is two links deep.
    let (store, index) = enter(&linked_to(17)).unwrap();

    let nested = index.symbol_at(NESTED_PACKAGE).unwrap();
    assert_eq!(name_of(&store, nested), "scala");
}

#[test]
fn a_link_to_an_address_inside_a_node_is_rejected() {
    // 10 is the name reference inside the leaf at 9, not the start of a node.
    let error = enter(&linked_to(10)).map(|_| ()).unwrap_err();

    assert_eq!(
        error,
        UnpickleError::InvalidReferenceTarget {
            from: NESTED_PACKAGE,
            to: 10
        }
    );
}

#[test]
fn a_link_that_leads_back_to_itself_is_rejected_as_a_cycle() {
    let error = enter(&linked_to(LINK_AT as u8)).map(|_| ()).unwrap_err();

    assert!(matches!(
        error,
        UnpickleError::InvalidReferenceTarget {
            from: NESTED_PACKAGE,
            ..
        }
    ));
}

#[test]
fn a_link_to_a_node_that_is_not_a_package_is_unsupported() {
    // Address 5 is the `IMPORT` node holding the shared leaf.
    let error = enter(&linked_to(5)).map(|_| ()).unwrap_err();

    assert_eq!(
        error,
        UnpickleError::UnsupportedPackagePath {
            address: NESTED_PACKAGE
        }
    );
}

#[test]
fn a_failed_unit_leaves_the_store_untouched() {
    let file_bytes = linked_to(5);
    let file = TastyFile::parse_compatible_with(&file_bytes, 28, 9, 0).unwrap();
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let before = store.checkpoint();

    let mut unpickler = TastyUnpickler::new(&file, &mut store, definitions);
    assert!(unpickler.enter_symbols().is_err());
    drop(unpickler);

    assert_eq!(store.checkpoint(), before);
}
