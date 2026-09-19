//! Package symbols and scopes shared between TASTy units entered into one
//! store (issue #12), over real Scala 3.9.0 compiler output.

use dotty_core::Definitions;
use dotty_core::ids::SymbolId;
use dotty_core::names::{Name, Namespace};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::SymbolKind;
use dotty_tasty::tasty::TastyFile;
use dotty_tasty_unpickler::tasty_unpickler::{
    TastyPackages, TastySemanticIndex, TastyUnpickler, UnpickleError,
};

const FOO: &[u8] = include_bytes!("fixtures/semantic/Foo.tasty");
const OVERLOADS: &[u8] = include_bytes!("fixtures/semantic/Overloads.tasty");

/// Address of the single top-level `PACKAGE` node of each fixture.
const PACKAGE_ADDRESS: u32 = 0;

const PACKAGE: [&str; 4] = ["me", "cytrowski", "tastyfixtures", "semantic"];

/// Enters `bytes` into `store` with `packages`, returning the unit's index
/// and the registry for the next unit.
fn enter_shared(
    bytes: &[u8],
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: TastyPackages,
) -> (TastySemanticIndex, TastyPackages) {
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let mut unpickler = TastyUnpickler::with_packages(&file, store, definitions, packages);
    unpickler.enter_symbols().unwrap();
    unpickler.into_parts()
}

fn type_name(store: &mut SemanticStore, text: &str) -> Name {
    Name::new(store.names.intern(text), Namespace::Type)
}

#[test]
fn two_units_share_one_symbol_for_their_common_package() {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);

    let (foo, packages) = enter_shared(FOO, &mut store, definitions, TastyPackages::new());
    let (overloads, packages) = enter_shared(OVERLOADS, &mut store, definitions, packages);

    let package = foo.symbol_at(PACKAGE_ADDRESS).unwrap();
    assert_eq!(overloads.symbol_at(PACKAGE_ADDRESS), Some(package));
    assert_eq!(packages.symbol(&PACKAGE), Some(package));
    // Four segments, entered once.
    assert_eq!(packages.len(), 4);
    assert_eq!(store.symbols.get(package).kind, SymbolKind::Package);
}

#[test]
fn both_units_see_the_one_package_scope_holding_both_units_members() {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);

    let (foo, packages) = enter_shared(FOO, &mut store, definitions, TastyPackages::new());
    let (overloads, _) = enter_shared(OVERLOADS, &mut store, definitions, packages);

    let package = foo.symbol_at(PACKAGE_ADDRESS).unwrap();
    let scope = foo.scope_of(package).unwrap();
    assert_eq!(overloads.scope_of(package), Some(scope));

    let foo_name = type_name(&mut store, "Foo");
    let overloads_name = type_name(&mut store, "Overloads");
    let scope = store.scopes.get(scope);
    assert!(scope.lookup(&foo_name).is_some());
    assert!(scope.lookup(&overloads_name).is_some());
}

#[test]
fn every_enclosing_package_is_shared_too() {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);

    let (foo, packages) = enter_shared(FOO, &mut store, definitions, TastyPackages::new());
    let (overloads, _) = enter_shared(OVERLOADS, &mut store, definitions, packages);

    let chain = |index: &TastySemanticIndex| {
        let mut symbol = index.symbol_at(PACKAGE_ADDRESS).unwrap();
        let mut owners: Vec<SymbolId> = vec![symbol];
        while let Some(owner) = store.symbols.get(symbol).owner {
            owners.push(owner);
            symbol = owner;
        }
        owners
    };
    assert_eq!(chain(&foo), chain(&overloads));
}

#[test]
fn units_entered_without_a_shared_registry_get_their_own_package() {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);

    let (foo, _) = enter_shared(FOO, &mut store, definitions, TastyPackages::new());
    let (overloads, _) = enter_shared(OVERLOADS, &mut store, definitions, TastyPackages::new());

    assert_ne!(
        foo.symbol_at(PACKAGE_ADDRESS),
        overloads.symbol_at(PACKAGE_ADDRESS)
    );
}

/// `Foo.tasty` with the byte at 124 zeroed fails late, after the shared
/// package and `Foo` were entered (see `scopes_and_identity.rs`).
fn late_failing_foo() -> Vec<u8> {
    let mut bytes = FOO.to_vec();
    bytes[124] = 0;
    bytes
}

#[test]
fn a_failed_unit_leaves_a_shared_package_as_the_earlier_unit_left_it() {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let (overloads, packages) =
        enter_shared(OVERLOADS, &mut store, definitions, TastyPackages::new());
    let package = overloads.symbol_at(PACKAGE_ADDRESS).unwrap();
    let scope = overloads.scope_of(package).unwrap();
    let foo_name = type_name(&mut store, "Foo");
    let overloads_name = type_name(&mut store, "Overloads");
    let before = store.checkpoint();
    let packages_before = packages.len();

    let bytes = late_failing_foo();
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut unpickler = TastyUnpickler::with_packages(&file, &mut store, definitions, packages);
    let result = unpickler.enter_symbols().map(|_| ());
    let (_, packages) = unpickler.into_parts();

    assert_eq!(
        result,
        Err(UnpickleError::UnsupportedName { reference: 13 })
    );
    assert_eq!(store.checkpoint(), before);
    assert_eq!(packages.len(), packages_before);
    assert_eq!(packages.symbol(&PACKAGE), Some(package));
    // The failed unit's `Foo` is gone from the shared scope; the earlier
    // unit's members are untouched.
    let scope = store.scopes.get(scope);
    assert_eq!(scope.lookup(&foo_name), None);
    assert!(scope.lookup(&overloads_name).is_some());
}

#[test]
fn a_unit_can_be_entered_after_another_unit_failed_in_the_same_package() {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let bytes = late_failing_foo();
    let broken = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut unpickler = TastyUnpickler::new(&broken, &mut store, definitions);
    assert!(unpickler.enter_symbols().is_err());
    let (_, packages) = unpickler.into_parts();
    assert!(packages.is_empty());

    let (overloads, packages) = enter_shared(OVERLOADS, &mut store, definitions, packages);

    let package = overloads.symbol_at(PACKAGE_ADDRESS).unwrap();
    assert_eq!(packages.symbol(&PACKAGE), Some(package));
    let overloads_name = type_name(&mut store, "Overloads");
    let scope = overloads.scope_of(package).unwrap();
    assert!(store.scopes.get(scope).lookup(&overloads_name).is_some());
}
