//! Runs the symbol-entering pass over the real `semantic/Foo.tasty` fixture
//! and checks the result through `dotty-core`.

use dotty_core::Definitions;
use dotty_core::ids::SymbolId;
use dotty_core::names::Namespace;
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolInfo, SymbolKind, SymbolOrigin, Visibility};
use dotty_tasty::tasty::TastyFile;
use dotty_tasty_unpickler::tasty_unpickler::{TastySemanticIndex, TastyUnpickler};

const FOO_TASTY: &[u8] = include_bytes!("fixtures/semantic/Foo.tasty");

/// Address of the fixture's single top-level `PACKAGE` node.
const PACKAGE_ADDRESS: u32 = 0;

// Absolute addresses of the fixture's definitions (pinned by
// `semantic_fixture.rs`). The class template header holds `A` and `x`; the
// constructor and `bar` carry their own type and term parameters.
const FOO: u32 = 4;
const CLASS_A: u32 = 9;
const CLASS_X: u32 = 24;
const INIT: u32 = 46;
const INIT_A: u32 = 49;
const INIT_X: u32 = 58;
const BAR: u32 = 70;
const BAR_B: u32 = 73;
const BAR_B_PARAM: u32 = 82;

struct Entered {
    store: SemanticStore,
    index: TastySemanticIndex,
    origin: SymbolOrigin,
}

fn enter_foo() -> Entered {
    let file = TastyFile::parse_scala_3_9(FOO_TASTY).unwrap();
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut unpickler = TastyUnpickler::new(&file, &mut store, definitions);
    unpickler.enter_symbols().unwrap();
    let origin = unpickler.origin();
    let index = unpickler.into_index();
    Entered {
        store,
        index,
        origin,
    }
}

impl Entered {
    /// The symbol entered for the definition at `address`.
    fn symbol(&self, address: u32) -> SymbolId {
        self.index
            .symbol_at(address)
            .unwrap_or_else(|| panic!("no symbol entered for address {address}"))
    }

    fn owner(&self, address: u32) -> Option<SymbolId> {
        self.store.symbols.get(self.symbol(address)).owner
    }

    fn kind(&self, address: u32) -> SymbolKind {
        self.store.symbols.get(self.symbol(address)).kind
    }

    fn name(&self, symbol: SymbolId) -> &str {
        self.store
            .names
            .resolve(self.store.symbols.get(symbol).name.text())
    }

    fn package_chain_ids(&self, symbol: SymbolId) -> Vec<SymbolId> {
        let mut chain = vec![symbol];
        let mut current = symbol;
        while let Some(owner) = self.store.symbols.get(current).owner {
            chain.push(owner);
            current = owner;
        }
        chain.reverse();
        chain
    }

    /// The package chain from the outermost package down to `symbol`.
    fn package_chain(&self, symbol: SymbolId) -> Vec<&str> {
        let mut chain = vec![self.name(symbol)];
        let mut current = symbol;
        while let Some(owner) = self.store.symbols.get(current).owner {
            chain.push(self.name(owner));
            current = owner;
        }
        chain.reverse();
        chain
    }
}

#[test]
fn the_package_node_maps_to_the_innermost_package_symbol() {
    let entered = enter_foo();

    let package = entered
        .index
        .symbol_at(PACKAGE_ADDRESS)
        .expect("package symbol");

    assert_eq!(
        entered.package_chain(package),
        ["", "me", "cytrowski", "tastyfixtures", "semantic"]
    );
}

#[test]
fn every_package_segment_is_a_package_symbol() {
    let entered = enter_foo();
    let mut current = entered.index.symbol_at(PACKAGE_ADDRESS);

    let mut kinds = Vec::new();
    while let Some(symbol) = current {
        kinds.push(entered.store.symbols.get(symbol).kind);
        current = entered.store.symbols.get(symbol).owner;
    }

    // Four segments and the session's root package.
    assert_eq!(kinds, [SymbolKind::Package; 5]);
}

#[test]
fn the_outermost_package_is_owned_by_the_root_which_has_no_owner() {
    let entered = enter_foo();
    let package = entered.index.symbol_at(PACKAGE_ADDRESS).unwrap();

    let outermost = *entered
        .package_chain_ids(package)
        .first()
        .expect("non-empty chain");

    let root = entered.store.symbols.get(outermost);
    assert_eq!(root.owner, None);
    assert_eq!(entered.name(outermost), "");
}

#[test]
fn package_symbols_are_public_term_named_and_untyped() {
    let entered = enter_foo();
    let package = entered
        .store
        .symbols
        .get(entered.index.symbol_at(PACKAGE_ADDRESS).unwrap());

    assert_eq!(package.visibility, Visibility::Public);
    assert_eq!(package.name.namespace(), Namespace::Term);
    assert_eq!(package.info, SymbolInfo::Missing);
}

#[test]
fn package_symbols_carry_the_unpicklers_tasty_origin() {
    let entered = enter_foo();
    let package = entered
        .store
        .symbols
        .get(entered.index.symbol_at(PACKAGE_ADDRESS).unwrap());

    assert!(matches!(package.origin, SymbolOrigin::Tasty(_)));
    assert_eq!(package.origin, entered.origin);
}

#[test]
fn the_innermost_package_owns_a_declaration_scope() {
    let entered = enter_foo();
    let package = entered.index.symbol_at(PACKAGE_ADDRESS).unwrap();

    let scope = entered.index.scope_of(package).expect("package scope");

    assert_eq!(entered.store.scopes.get(scope).owner, Some(package));
}

#[test]
fn foo_is_a_class_owned_by_the_innermost_package() {
    let entered = enter_foo();

    assert_eq!(entered.kind(FOO), SymbolKind::Class);
    assert_eq!(entered.owner(FOO), Some(entered.symbol(PACKAGE_ADDRESS)));
    assert_eq!(entered.name(entered.symbol(FOO)), "Foo");
}

#[test]
fn foo_is_a_public_type_named_symbol() {
    let entered = enter_foo();
    let foo = entered.store.symbols.get(entered.symbol(FOO));

    assert_eq!(foo.visibility, Visibility::Public);
    assert_eq!(foo.name.namespace(), Namespace::Type);
}

#[test]
fn the_class_type_parameter_is_owned_by_foo() {
    let entered = enter_foo();

    assert_eq!(entered.owner(CLASS_A), Some(entered.symbol(FOO)));
    assert_eq!(entered.kind(CLASS_A), SymbolKind::TypeParameter);
    assert_eq!(entered.name(entered.symbol(CLASS_A)), "A");
    assert_eq!(
        entered
            .store
            .symbols
            .get(entered.symbol(CLASS_A))
            .name
            .namespace(),
        Namespace::Type
    );
}

#[test]
fn the_class_type_parameter_is_private_this() {
    let entered = enter_foo();

    let a = entered.store.symbols.get(entered.symbol(CLASS_A));

    assert_eq!(a.visibility, Visibility::Private);
}

#[test]
fn the_val_constructor_parameter_x_is_a_public_field_of_foo() {
    let entered = enter_foo();
    let x = entered.store.symbols.get(entered.symbol(CLASS_X));

    assert_eq!(x.owner, Some(entered.symbol(FOO)));
    assert_eq!(x.kind, SymbolKind::Field);
    assert_eq!(x.visibility, Visibility::Public);
    assert_eq!(x.name.namespace(), Namespace::Term);
    assert_eq!(entered.name(entered.symbol(CLASS_X)), "x");
}

#[test]
fn bar_is_a_method_owned_by_foo() {
    let entered = enter_foo();

    assert_eq!(entered.owner(BAR), Some(entered.symbol(FOO)));
    assert_eq!(entered.kind(BAR), SymbolKind::Method);
    assert_eq!(entered.name(entered.symbol(BAR)), "bar");
}

#[test]
fn the_constructor_is_a_constructor_owned_by_foo() {
    let entered = enter_foo();

    assert_eq!(entered.owner(INIT), Some(entered.symbol(FOO)));
    assert_eq!(entered.kind(INIT), SymbolKind::Constructor);
    assert_eq!(entered.name(entered.symbol(INIT)), "<init>");
}

#[test]
fn the_method_type_parameter_b_is_owned_by_bar() {
    let entered = enter_foo();

    assert_eq!(entered.owner(BAR_B), Some(entered.symbol(BAR)));
    assert_eq!(entered.kind(BAR_B), SymbolKind::TypeParameter);
    assert_eq!(entered.name(entered.symbol(BAR_B)), "B");
}

#[test]
fn the_method_parameter_b_is_a_parameter_owned_by_bar() {
    let entered = enter_foo();

    assert_eq!(entered.owner(BAR_B_PARAM), Some(entered.symbol(BAR)));
    assert_eq!(entered.kind(BAR_B_PARAM), SymbolKind::Parameter);
    assert_eq!(entered.name(entered.symbol(BAR_B_PARAM)), "b");
}

#[test]
fn the_constructor_has_its_own_copies_of_the_class_parameters() {
    let entered = enter_foo();

    assert_eq!(entered.owner(INIT_A), Some(entered.symbol(INIT)));
    assert_eq!(entered.kind(INIT_A), SymbolKind::TypeParameter);
    assert_eq!(entered.owner(INIT_X), Some(entered.symbol(INIT)));
    assert_eq!(entered.kind(INIT_X), SymbolKind::Parameter);
    assert_ne!(entered.symbol(INIT_A), entered.symbol(CLASS_A));
    assert_ne!(entered.symbol(INIT_X), entered.symbol(CLASS_X));
}

#[test]
fn every_entered_symbol_is_untyped_and_tasty_origin() {
    let entered = enter_foo();

    for address in [
        FOO,
        CLASS_A,
        CLASS_X,
        INIT,
        INIT_A,
        INIT_X,
        BAR,
        BAR_B,
        BAR_B_PARAM,
    ] {
        let symbol = entered.store.symbols.get(entered.symbol(address));
        assert_eq!(symbol.info, SymbolInfo::Missing, "address {address}");
        assert_eq!(symbol.origin, entered.origin, "address {address}");
    }
}

#[test]
fn each_definition_address_has_exactly_one_symbol() {
    let entered = enter_foo();
    let addresses = [
        FOO,
        CLASS_A,
        CLASS_X,
        INIT,
        INIT_A,
        INIT_X,
        BAR,
        BAR_B,
        BAR_B_PARAM,
    ];

    let mut symbols: Vec<_> = addresses.iter().map(|a| entered.symbol(*a)).collect();
    symbols.sort_by_key(|symbol| symbol.index());
    symbols.dedup();

    assert_eq!(symbols.len(), addresses.len());
    // The nine definitions plus the package node.
    assert_eq!(entered.index.symbol_count(), addresses.len() + 1);
}
