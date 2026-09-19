//! Declaration scopes, namespaces, overloads and address identity of the
//! symbol-entering pass, over real Scala 3.9.0 compiler output.

use dotty_core::Definitions;
use dotty_core::ids::{ScopeId, SymbolId};
use dotty_core::names::{Name, Namespace};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::SymbolKind;
use dotty_tasty::tasty::{PARAM_TAG, TastyFile};
use dotty_tasty_unpickler::tasty_unpickler::{TastySemanticIndex, TastyUnpickler, UnpickleError};

const FOO: &[u8] = include_bytes!("fixtures/semantic/Foo.tasty");
const OVERLOADS: &[u8] = include_bytes!("fixtures/semantic/Overloads.tasty");
const PLAIN: &[u8] = include_bytes!("fixtures/semantic/Plain.tasty");
const BOTH: &[u8] = include_bytes!("fixtures/semantic/Both.tasty");
const MARKER: &[u8] = include_bytes!("fixtures/semantic/Marker.tasty");

/// Address of the single top-level `PACKAGE` node of each fixture.
const PACKAGE_ADDRESS: u32 = 0;

struct Entered {
    store: SemanticStore,
    index: TastySemanticIndex,
}

fn enter(bytes: &[u8]) -> Entered {
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut unpickler = TastyUnpickler::new(&file, &mut store, definitions);
    unpickler.enter_symbols().unwrap();
    let index = unpickler.into_index();
    Entered { store, index }
}

/// Absolute addresses of the file's nodes with `tag`.
fn definition_addresses(bytes: &[u8], tag: u8) -> Vec<u32> {
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let index = file.ast_address_index().unwrap();
    index
        .iter_nodes_with_tag(tag)
        .map(|node| u32::try_from(node.offset).unwrap())
        .collect()
}

impl Entered {
    fn name(&mut self, text: &str, namespace: Namespace) -> Name {
        Name::new(self.store.names.intern(text), namespace)
    }

    fn package(&self) -> SymbolId {
        self.index
            .symbol_at(PACKAGE_ADDRESS)
            .expect("package symbol")
    }

    fn scope(&self, owner: SymbolId) -> ScopeId {
        self.index
            .scope_of(owner)
            .expect("owner has a declaration scope")
    }

    /// Every symbol entered under `name` in `owner`'s declaration scope.
    fn declared(&mut self, owner: SymbolId, text: &str, namespace: Namespace) -> Vec<SymbolId> {
        let name = self.name(text, namespace);
        let scope = self.scope(owner);
        self.store.scopes.get(scope).lookup_all(&name).to_vec()
    }

    /// The single symbol declared under `name` in `owner`'s scope.
    fn only(&mut self, owner: SymbolId, text: &str, namespace: Namespace) -> SymbolId {
        let declared = self.declared(owner, text, namespace);
        assert_eq!(declared.len(), 1, "{text} ({namespace:?}) in scope");
        declared[0]
    }

    fn kind(&self, symbol: SymbolId) -> SymbolKind {
        self.store.symbols.get(symbol).kind
    }

    fn owner(&self, symbol: SymbolId) -> Option<SymbolId> {
        self.store.symbols.get(symbol).owner
    }
}

// --- scopes: class members ---

#[test]
fn foo_is_declared_in_the_package_scope() {
    let mut entered = enter(FOO);
    let package = entered.package();

    let foo = entered.only(package, "Foo", Namespace::Type);

    assert_eq!(entered.kind(foo), SymbolKind::Class);
}

#[test]
fn foo_owns_a_declaration_scope_that_records_it_as_owner() {
    let mut entered = enter(FOO);
    let package = entered.package();
    let foo = entered.only(package, "Foo", Namespace::Type);

    let scope = entered.scope(foo);

    assert_eq!(entered.store.scopes.get(scope).owner, Some(foo));
}

#[test]
fn x_and_bar_are_discoverable_from_foos_scope() {
    let mut entered = enter(FOO);
    let package = entered.package();
    let foo = entered.only(package, "Foo", Namespace::Type);

    let x = entered.only(foo, "x", Namespace::Term);
    let bar = entered.only(foo, "bar", Namespace::Term);

    assert_eq!(entered.kind(x), SymbolKind::Field);
    assert_eq!(entered.kind(bar), SymbolKind::Method);
}

#[test]
fn the_constructor_and_the_class_type_parameter_are_declared_in_foos_scope() {
    let mut entered = enter(FOO);
    let package = entered.package();
    let foo = entered.only(package, "Foo", Namespace::Type);

    let init = entered.only(foo, "<init>", Namespace::Term);
    let a = entered.only(foo, "A", Namespace::Type);

    assert_eq!(entered.kind(init), SymbolKind::Constructor);
    assert_eq!(entered.kind(a), SymbolKind::TypeParameter);
}

#[test]
fn a_symbols_owner_and_the_scope_it_is_declared_in_agree_for_members() {
    let mut entered = enter(FOO);
    let package = entered.package();
    let foo = entered.only(package, "Foo", Namespace::Type);

    let bar = entered.only(foo, "bar", Namespace::Term);

    assert_eq!(entered.owner(bar), Some(foo));
}

// --- scopes: what is owned but not declared ---

#[test]
fn method_parameters_are_not_declared_in_the_class_scope() {
    let mut entered = enter(FOO);
    let package = entered.package();
    let foo = entered.only(package, "Foo", Namespace::Type);

    assert_eq!(entered.declared(foo, "b", Namespace::Term), []);
    assert_eq!(entered.declared(foo, "B", Namespace::Type), []);
}

#[test]
fn the_constructors_parameter_copies_are_not_declared_in_the_class_scope() {
    let mut entered = enter(FOO);
    let package = entered.package();
    let foo = entered.only(package, "Foo", Namespace::Type);

    // Exactly one `x` and one `A` are members: the class's own, not the
    // copies the constructor node carries.
    assert_eq!(entered.declared(foo, "x", Namespace::Term).len(), 1);
    assert_eq!(entered.declared(foo, "A", Namespace::Type).len(), 1);
}

#[test]
fn a_method_has_no_declaration_scope() {
    let mut entered = enter(FOO);
    let package = entered.package();
    let foo = entered.only(package, "Foo", Namespace::Type);
    let bar = entered.only(foo, "bar", Namespace::Term);

    assert_eq!(entered.index.scope_of(bar), None);
}

#[test]
fn a_private_this_constructor_parameter_is_a_parameter_not_a_member() {
    let mut entered = enter(PLAIN);
    let package = entered.package();
    let plain = entered.only(package, "Plain", Namespace::Type);

    assert_eq!(entered.declared(plain, "y", Namespace::Term), []);
}

// --- namespaces ---

#[test]
fn a_term_name_does_not_find_a_type_of_the_same_text() {
    let mut entered = enter(FOO);
    let package = entered.package();
    let foo = entered.only(package, "Foo", Namespace::Type);

    assert_eq!(entered.declared(foo, "A", Namespace::Term), []);
    assert_eq!(entered.declared(package, "Foo", Namespace::Term), []);
}

#[test]
fn a_class_and_its_companion_object_are_distinct_symbols_in_distinct_namespaces() {
    let mut entered = enter(BOTH);
    let package = entered.package();

    let class = entered.only(package, "Both", Namespace::Type);
    let object = entered.only(package, "Both", Namespace::Term);

    assert_ne!(class, object);
    assert_eq!(entered.kind(class), SymbolKind::Class);
    assert_eq!(entered.kind(object), SymbolKind::Object);
}

#[test]
fn an_objects_module_class_is_a_third_symbol_named_with_a_dollar() {
    let mut entered = enter(BOTH);
    let package = entered.package();

    let module_class = entered.only(package, "Both$", Namespace::Type);

    assert_eq!(entered.kind(module_class), SymbolKind::ModuleClass);
    assert_eq!(entered.owner(module_class), Some(package));
}

#[test]
fn the_module_class_owns_the_objects_members() {
    let mut entered = enter(BOTH);
    let package = entered.package();
    let module_class = entered.only(package, "Both$", Namespace::Type);

    let make = entered.only(module_class, "make", Namespace::Term);

    assert_eq!(entered.kind(make), SymbolKind::Method);
    assert_eq!(entered.owner(make), Some(module_class));
}

#[test]
fn a_trait_is_a_trait_and_owns_its_members() {
    let mut entered = enter(MARKER);
    let package = entered.package();

    let marker = entered.only(package, "Marker", Namespace::Type);
    let mark = entered.only(marker, "mark", Namespace::Term);

    assert_eq!(entered.kind(marker), SymbolKind::Trait);
    assert_eq!(entered.kind(mark), SymbolKind::Method);
}

// --- overloads ---

#[test]
fn overloaded_methods_keep_every_entry_in_declaration_order() {
    let mut entered = enter(OVERLOADS);
    let package = entered.package();
    let overloads = entered.only(package, "Overloads", Namespace::Type);

    let fs = entered.declared(overloads, "f", Namespace::Term);

    assert_eq!(fs.len(), 2);
    assert_ne!(fs[0], fs[1]);
    assert!(fs.iter().all(|f| entered.kind(*f) == SymbolKind::Method));
    assert!(fs.iter().all(|f| entered.owner(*f) == Some(overloads)));
    assert!(
        fs[0].index() < fs[1].index(),
        "insertion order is source order"
    );
}

#[test]
fn each_overloads_parameter_is_a_separate_symbol_owned_by_its_own_method() {
    let mut entered = enter(OVERLOADS);
    let package = entered.package();
    let overloads = entered.only(package, "Overloads", Namespace::Type);
    let fs = entered.declared(overloads, "f", Namespace::Term);

    let owners: Vec<_> = definition_addresses(OVERLOADS, PARAM_TAG)
        .into_iter()
        .map(|address| {
            let parameter = entered.index.symbol_at(address).expect("parameter symbol");
            assert_eq!(entered.kind(parameter), SymbolKind::Parameter);
            (parameter, entered.owner(parameter).unwrap())
        })
        .collect();

    assert_eq!(owners.len(), 2);
    assert_ne!(owners[0].0, owners[1].0);
    let mut method_owners: Vec<_> = owners.iter().map(|(_, owner)| *owner).collect();
    method_owners.sort_by_key(|owner| owner.index());
    assert_eq!(method_owners, fs);
}

// --- address identity ---

#[test]
fn repeated_index_lookups_return_the_same_symbol() {
    let entered = enter(FOO);

    for address in [0, 4, 9, 24, 46, 49, 58, 70, 73, 82] {
        assert_eq!(
            entered.index.symbol_at(address),
            entered.index.symbol_at(address),
            "address {address}"
        );
        assert!(
            entered.index.symbol_at(address).is_some(),
            "address {address}"
        );
    }
}

#[test]
fn an_address_that_is_not_a_definition_has_no_symbol() {
    let entered = enter(FOO);

    // 7 is the class's TEMPLATE, 12 a type-bounds node.
    assert_eq!(entered.index.symbol_at(7), None);
    assert_eq!(entered.index.symbol_at(12), None);
}

#[test]
fn entering_the_same_file_twice_is_rejected_without_duplicating_symbols() {
    let file = TastyFile::parse_scala_3_9(FOO).unwrap();
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut unpickler = TastyUnpickler::new(&file, &mut store, definitions);
    unpickler.enter_symbols().unwrap();
    let before = unpickler.index().symbol_count();

    let second = unpickler.enter_symbols().map(|_| ());

    assert_eq!(
        second,
        Err(UnpickleError::DuplicateDefinition {
            address: PACKAGE_ADDRESS
        })
    );
    assert_eq!(unpickler.index().symbol_count(), before);
}

/// `Foo.tasty` with the name reference in the byte at 124 zeroed, which turns
/// a definition late in the file into one whose name cannot be read
/// (`UnsupportedName { reference: 13 }`). The enclosing packages, `Foo` and
/// its parameters have been entered by then.
fn late_failing_foo() -> Vec<u8> {
    let mut bytes = FOO.to_vec();
    bytes[124] = 0;
    bytes
}

#[test]
fn a_late_failure_leaves_the_store_exactly_as_it_was() {
    let bytes = late_failing_foo();
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    // Something allocated before the call must survive it.
    let older = store.symbols.alloc(dotty_core::symbols::Symbol {
        name: Name::new(store.names.intern("older"), Namespace::Term),
        owner: None,
        kind: SymbolKind::Value,
        flags: dotty_core::symbols::SymbolFlags::EMPTY,
        visibility: dotty_core::symbols::Visibility::Public,
        info: dotty_core::symbols::SymbolInfo::Missing,
        origin: dotty_core::symbols::SymbolOrigin::Synthetic,
        annotations: Vec::new(),
        position: None,
        links: dotty_core::symbols::SymbolLinks::default(),
    });
    let before = store.checkpoint();

    let mut unpickler = TastyUnpickler::new(&file, &mut store, definitions);
    let result = unpickler.enter_symbols().map(|_| ());

    assert_eq!(
        result,
        Err(UnpickleError::UnsupportedName { reference: 13 })
    );
    assert_eq!(unpickler.index().symbol_count(), 0);
    assert_eq!(unpickler.index().symbol_at(PACKAGE_ADDRESS), None);
    drop(unpickler);
    assert_eq!(store.checkpoint(), before);
    assert_eq!(store.symbols.get(older).kind, SymbolKind::Value);
}

#[test]
fn an_unpickler_that_failed_can_be_retried() {
    let bytes = late_failing_foo();
    let broken = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut unpickler = TastyUnpickler::new(&broken, &mut store, definitions);
    assert!(unpickler.enter_symbols().is_err());
    // The failed attempt left no package behind, so entering again fails the
    // same way instead of tripping over half-entered state.
    assert_eq!(
        unpickler.enter_symbols().map(|_| ()),
        Err(UnpickleError::UnsupportedName { reference: 13 })
    );
    assert_eq!(unpickler.index().symbol_count(), 0);
}

#[test]
fn a_failure_in_a_later_unit_does_not_disturb_an_earlier_one() {
    let good = TastyFile::parse_scala_3_9(FOO).unwrap();
    let bytes = late_failing_foo();
    let broken = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);

    let mut first = TastyUnpickler::new(&good, &mut store, definitions);
    first.enter_symbols().unwrap();
    let index = first.into_index();
    let after_first = store.checkpoint();

    let mut second = TastyUnpickler::new(&broken, &mut store, definitions);
    assert!(second.enter_symbols().is_err());
    drop(second);

    assert_eq!(store.checkpoint(), after_first);
    let foo = index.symbol_at(4).unwrap();
    assert_eq!(store.symbols.get(foo).kind, SymbolKind::Class);
}
