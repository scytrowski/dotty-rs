//! Runs the symbol-entering pass over the real `semantic/Foo.tasty` fixture
//! and checks the result through `dotty-core`.

use dotty_core::ids::SymbolId;
use dotty_core::names::Namespace;
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolInfo, SymbolKind, SymbolOrigin, Visibility};
use dotty_tasty::tasty::TastyFile;
use dotty_tasty_unpickler::tasty_unpickler::{TastySemanticIndex, TastyUnpickler};

const FOO: &[u8] = include_bytes!("fixtures/semantic/Foo.tasty");

/// Address of the fixture's single top-level `PACKAGE` node.
const PACKAGE_ADDRESS: u32 = 0;

struct Entered {
    store: SemanticStore,
    index: TastySemanticIndex,
    origin: SymbolOrigin,
}

fn enter_foo() -> Entered {
    let file = TastyFile::parse_scala_3_9(FOO).unwrap();
    let mut store = SemanticStore::new();
    let mut unpickler = TastyUnpickler::new(&file, &mut store);
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
        ["me", "cytrowski", "tastyfixtures", "semantic"]
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

    assert_eq!(kinds, [SymbolKind::Package; 4]);
}

#[test]
fn the_outermost_package_has_no_owner() {
    let entered = enter_foo();
    let package = entered.index.symbol_at(PACKAGE_ADDRESS).unwrap();

    let outermost = *entered
        .package_chain_ids(package)
        .first()
        .expect("non-empty chain");

    assert_eq!(entered.store.symbols.get(outermost).owner, None);
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
