//! Access modifiers, including qualified `private[Q]` / `protected[Q]`, over
//! real Scala 3.9.0 compiler output (`fixtures/semantic/Access.scala`).

use dotty_core::ids::SymbolId;
use dotty_core::names::{Name, Namespace};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::Visibility;
use dotty_tasty::tasty::TastyFile;
use dotty_tasty_unpickler::tasty_unpickler::{TastySemanticIndex, TastyUnpickler};

const ACCESS: &[u8] = include_bytes!("fixtures/semantic/Access.tasty");
const OUTER: &[u8] = include_bytes!("fixtures/semantic/Outer.tasty");

struct Entered {
    store: SemanticStore,
    index: TastySemanticIndex,
}

fn enter(bytes: &[u8]) -> Entered {
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let mut store = SemanticStore::new();
    let mut unpickler = TastyUnpickler::new(&file, &mut store);
    unpickler.enter_symbols().unwrap();
    let index = unpickler.into_index();
    Entered { store, index }
}

impl Entered {
    fn package(&self) -> SymbolId {
        self.index.symbol_at(0).expect("package symbol")
    }

    fn member(&mut self, owner: SymbolId, text: &str, namespace: Namespace) -> SymbolId {
        let name = Name::new(self.store.names.intern(text), namespace);
        let scope = self.index.scope_of(owner).expect("owner has a scope");
        let found = self.store.scopes.get(scope).lookup_all(&name).to_vec();
        assert_eq!(found.len(), 1, "{text} in scope");
        found[0]
    }

    fn class(&mut self, text: &str) -> SymbolId {
        let package = self.package();
        self.member(package, text, Namespace::Type)
    }

    fn visibility(&self, symbol: SymbolId) -> Visibility {
        self.store.symbols.get(symbol).visibility
    }
}

#[test]
fn private_with_a_package_qualifier_is_private_within_that_package() {
    let mut entered = enter(ACCESS);
    let access = entered.class("Access");
    let member = entered.member(access, "inPackage", Namespace::Term);

    assert_eq!(
        entered.visibility(member),
        Visibility::PrivateWithin(entered.package())
    );
}

#[test]
fn protected_with_a_package_qualifier_is_protected_within_that_package() {
    let mut entered = enter(ACCESS);
    let access = entered.class("Access");
    let member = entered.member(access, "protectedInPackage", Namespace::Term);

    assert_eq!(
        entered.visibility(member),
        Visibility::ProtectedWithin(entered.package())
    );
}

#[test]
fn private_with_the_enclosing_class_as_qualifier_is_private_within_that_class() {
    let mut entered = enter(ACCESS);
    let access = entered.class("Access");
    let member = entered.member(access, "inClass", Namespace::Term);

    assert_eq!(
        entered.visibility(member),
        Visibility::PrivateWithin(access)
    );
}

#[test]
fn private_with_an_outer_class_as_qualifier_resolves_to_that_outer_class() {
    let mut entered = enter(OUTER);
    let outer = entered.class("Outer");
    let inner = entered.member(outer, "Inner", Namespace::Type);
    let member = entered.member(inner, "withinOuter", Namespace::Term);

    assert_eq!(entered.visibility(member), Visibility::PrivateWithin(outer));
}

#[test]
fn a_qualifier_symbol_is_the_one_entered_for_that_definition() {
    let mut entered = enter(ACCESS);
    let access = entered.class("Access");
    let member = entered.member(access, "inClass", Namespace::Term);

    let Visibility::PrivateWithin(qualifier) = entered.visibility(member) else {
        panic!("expected private within");
    };

    assert_eq!(entered.index.symbol_at(4), Some(qualifier));
}

#[test]
fn private_this_stays_an_unqualified_private() {
    let mut entered = enter(ACCESS);
    let access = entered.class("Access");
    let member = entered.member(access, "objectPrivate", Namespace::Term);

    assert_eq!(entered.visibility(member), Visibility::Private);
}

#[test]
fn plain_private_and_protected_stay_unqualified() {
    let mut entered = enter(ACCESS);
    let access = entered.class("Access");
    let private = entered.member(access, "plain", Namespace::Term);
    let protected = entered.member(access, "guarded", Namespace::Term);

    assert_eq!(entered.visibility(private), Visibility::Private);
    assert_eq!(entered.visibility(protected), Visibility::Protected);
}

#[test]
fn the_package_qualifier_reuses_the_units_package_symbol() {
    let mut entered = enter(ACCESS);
    let access = entered.class("Access");
    let in_package = entered.member(access, "inPackage", Namespace::Term);
    let protected_in_package = entered.member(access, "protectedInPackage", Namespace::Term);

    assert_eq!(
        entered.visibility(in_package),
        Visibility::PrivateWithin(entered.package())
    );
    // Both qualified members share the one package symbol.
    let (Visibility::PrivateWithin(a), Visibility::ProtectedWithin(b)) = (
        entered.visibility(in_package),
        entered.visibility(protected_in_package),
    ) else {
        panic!("expected qualified visibilities");
    };
    assert_eq!(a, b);
}
