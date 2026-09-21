//! Units in the default package, over real Scala 3.9.0 output
//! (`tests/fixtures/semantic/DefaultPackage.scala`).
//!
//! A unit with no `package` clause is written against the package named
//! `<empty>`. The session's package model (`dotty_core::Packages`) has one
//! root that is also the unnamed package, so such a unit's package is that
//! root: no symbol named `<empty>` exists, and its top-level classes are owned
//! by, and declared in, the root.

use dotty_core::ids::SymbolId;
use dotty_core::names::{Name, Namespace};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::SymbolKind;
use dotty_core::types::Type;
use dotty_core::{Definitions, Packages};
use dotty_tasty::tasty::TastyFile;
use dotty_tasty_unpickler::tasty_unpickler::{TastySemanticIndex, TastyUnpickler};

const WIDGET: &[u8] = include_bytes!("fixtures/semantic/Widget.tasty");
const HOLDER: &[u8] = include_bytes!("fixtures/semantic/Holder.tasty");

/// The `PACKAGE` node is the first node of the AST.
const PACKAGE_ADDRESS: u32 = 0;

struct Entered {
    store: SemanticStore,
    definitions: Definitions,
}

impl Entered {
    fn name(&mut self, text: &str, namespace: Namespace) -> Name {
        Name::new(self.store.names.intern(text), namespace)
    }

    fn text(&self, symbol: SymbolId) -> &str {
        self.store
            .names
            .resolve(self.store.symbols.get(symbol).name.text())
    }
}

/// Enters `bytes` with `packages`; returns the index and the registry.
fn enter(
    session: &mut Entered,
    bytes: &[u8],
    packages: Packages,
) -> (TastySemanticIndex, Packages) {
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let mut unpickler =
        TastyUnpickler::with_packages(&file, &mut session.store, session.definitions, packages);
    unpickler.enter_symbols().unwrap();
    unpickler.into_parts()
}

fn session() -> Entered {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    Entered { store, definitions }
}

/// The address of the top-level definition named `text` (as a term or type).
fn top_level(
    session: &mut Entered,
    index: &TastySemanticIndex,
    root: SymbolId,
    text: &str,
) -> Vec<SymbolId> {
    let scope = index.scope_of(root).expect("the root has a scope");
    let mut found = Vec::new();
    for namespace in [Namespace::Type, Namespace::Term] {
        let name = session.name(text, namespace);
        found.extend_from_slice(session.store.scopes.get(scope).lookup_all(&name));
    }
    found
}

#[test]
fn a_default_package_unit_enters_and_its_package_is_the_session_root() {
    let mut session = session();

    let (index, packages) = enter(&mut session, WIDGET, Packages::new());

    let package = index.symbol_at(PACKAGE_ADDRESS).expect("package symbol");
    let root = packages.symbol::<&str>(&[]).expect("the root");
    assert_eq!(package, root);
    let symbol = session.store.symbols.get(root);
    assert_eq!(symbol.kind, SymbolKind::Package);
    assert_eq!(symbol.owner, None);
    assert_eq!(session.text(root), "");
    // No named package was made for `<empty>`.
    assert_eq!(packages.len(), 0);
    assert_eq!(packages.symbol(&["<empty>"]), None);
    assert_eq!(index.scope_of(root), packages.scope_of(root));
}

#[test]
fn top_level_definitions_are_owned_by_and_declared_in_the_root() {
    let mut session = session();
    let (index, packages) = enter(&mut session, WIDGET, Packages::new());
    let root = packages.symbol::<&str>(&[]).unwrap();

    let widgets = top_level(&mut session, &index, root, "Widget");

    assert_eq!(widgets.len(), 1);
    let widget = session.store.symbols.get(widgets[0]);
    assert_eq!(widget.kind, SymbolKind::Class);
    assert_eq!(widget.owner, Some(root));
}

#[test]
fn an_object_and_its_module_class_are_both_owned_by_the_root() {
    let mut session = session();
    let (index, packages) = enter(&mut session, HOLDER, Packages::new());
    let root = packages.symbol::<&str>(&[]).unwrap();

    let object = top_level(&mut session, &index, root, "Holder");
    let module_class = top_level(&mut session, &index, root, "Holder$");

    assert_eq!(object.len(), 1);
    assert_eq!(module_class.len(), 1);
    assert_eq!(
        session.store.symbols.get(object[0]).kind,
        SymbolKind::Object
    );
    assert_eq!(session.store.symbols.get(object[0]).owner, Some(root));
    assert_eq!(
        session.store.symbols.get(module_class[0]).kind,
        SymbolKind::ModuleClass
    );
    assert_eq!(session.store.symbols.get(module_class[0]).owner, Some(root));
}

#[test]
fn two_default_package_units_share_the_one_root() {
    let mut session = session();

    let (widget_index, packages) = enter(&mut session, WIDGET, Packages::new());
    let (holder_index, packages) = enter(&mut session, HOLDER, packages);

    let root = packages.symbol::<&str>(&[]).unwrap();
    assert_eq!(widget_index.symbol_at(PACKAGE_ADDRESS), Some(root));
    assert_eq!(holder_index.symbol_at(PACKAGE_ADDRESS), Some(root));
    assert_eq!(packages.len(), 0);
}

#[test]
fn the_package_reference_of_the_unit_is_the_root_with_the_canonical_prefix() {
    // The `TERMREFpkg <empty>` at address 2 is the path of the `PACKAGE`.
    let mut session = session();
    let file = TastyFile::parse_scala_3_9(WIDGET).unwrap();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    unpickler.enter_symbols().unwrap();

    let ty = unpickler.unpickle_type(2).unwrap();
    let root = unpickler.index().symbol_at(PACKAGE_ADDRESS).unwrap();
    drop(unpickler);
    let Type::TermRef { prefix, target } = session.store.types.get(ty) else {
        panic!("expected a TermRef");
    };
    let symbol = &target.symbol().unwrap();
    assert_eq!(*symbol, root);
    assert_eq!(*prefix, session.definitions.no_prefix);
}

#[test]
fn a_name_based_reference_to_a_default_package_class_of_another_unit_resolves() {
    // `Holder.use(p: Widget)` names `Widget` of the other unit by name, in the
    // prefix `<empty>`.
    let mut session = session();
    let (widget_index, packages) = enter(&mut session, WIDGET, Packages::new());
    let root = packages.symbol::<&str>(&[]).unwrap();
    let widget = top_level(&mut session, &widget_index, root, "Widget")[0];

    let file = TastyFile::parse_scala_3_9(HOLDER).unwrap();
    let addresses: Vec<u32> = file
        .ast_address_index()
        .unwrap()
        .iter_nodes()
        .filter(|node| node.tag == 117)
        .map(|node| u32::try_from(node.offset).unwrap())
        .collect();
    let mut unpickler =
        TastyUnpickler::with_packages(&file, &mut session.store, session.definitions, packages);
    unpickler.enter_symbols().unwrap();
    let resolved: Vec<_> = addresses
        .iter()
        .filter_map(|&at| unpickler.unpickle_type(at).ok())
        .collect();
    drop(unpickler);

    let widgets: Vec<_> = resolved
        .iter()
        .filter_map(|&ty| match session.store.types.get(ty) {
            ty @ Type::TypeRef { prefix, .. } if ty.reference_symbol() == Some(widget) => {
                Some(*prefix)
            }
            _ => None,
        })
        .collect();
    assert!(!widgets.is_empty(), "no reference resolved to Widget");
    for prefix in widgets {
        // The compiler writes a package prefix as `THIS`; both name the root.
        match session.store.types.get(prefix) {
            ty @ (Type::TermRef { .. } | Type::TypeRef { .. }) => {
                assert_eq!(ty.reference_symbol(), Some(root));
            }
            Type::ThisType { class } => assert_eq!(*class, root),
            other => panic!("expected a package prefix, got {other:?}"),
        }
    }
}
