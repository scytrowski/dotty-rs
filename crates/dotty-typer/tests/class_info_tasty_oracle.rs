//! Checks source class completion against real Scala 3.9.0 TASTy semantics.

use dotty_core::names::{Name, Namespace};
use dotty_core::symbols::{SymbolInfo, SymbolKind, SymbolOrigin};
use dotty_core::types::{ClassInfo, Type};
use dotty_core::{
    Definitions, Packages, SemanticStore, Symbol, SymbolFlags, SymbolLinks, Visibility,
};
use dotty_tasty::tasty::{DefinitionBody, StructuredNode, TastyFile};
use dotty_tasty_unpickler::tasty_unpickler::TastyUnpickler;

const INFO_BASE_TASTY: &[u8] =
    include_bytes!("../../dotty-tasty-unpickler/tests/fixtures/semantic/InfoBase.tasty");

#[test]
fn scala_390_tasty_trait_without_a_source_parent_has_object_parent() {
    let file = TastyFile::parse_scala_3_9(INFO_BASE_TASTY).unwrap();
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    for (path, names) in [
        (&["scala"][..], &["Any", "Nothing"][..]),
        (&["java", "lang"][..], &["Object"][..]),
    ] {
        let package = packages
            .enter(&mut store, SymbolOrigin::Synthetic, path)
            .pop()
            .unwrap();
        for text in names {
            let name = Name::new(store.names.intern(text), Namespace::Type);
            let symbol = store.symbols.alloc(Symbol {
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
            store.scopes.get_mut(package.scope).enter(name, symbol);
        }
    }

    let mut unpickler = TastyUnpickler::with_packages(&file, &mut store, definitions, packages);
    unpickler.enter_symbols().unwrap();
    let ast = file.ast_address_index().unwrap();
    let address = ast
        .iter_nodes()
        .find_map(|node| {
            let address = u32::try_from(node.offset).ok()?;
            let raw = ast.get(address)?;
            let StructuredNode::TypeDef(DefinitionBody::TypeDef { name, .. }) =
                raw.decode_structured().ok()?
            else {
                return None;
            };
            (file.names().get_utf8(name) == Some("InfoBase")
                && unpickler
                    .symbol_state_at(address)
                    .is_some_and(|state| state.0 == SymbolKind::Trait))
            .then_some(address)
        })
        .expect("the Scala 3.9.0 oracle fixture contains its InfoBase trait");

    let completed = unpickler.complete_symbol(address).unwrap();
    let (index, _) = unpickler.into_parts();
    let class = index.symbol_at(address).unwrap();
    assert_eq!(*store.symbols.info(class), SymbolInfo::Complete(completed));
    let Type::ClassInfo(ClassInfo { parents, .. }) = store.types.get(completed) else {
        panic!("expected ClassInfo for the oracle trait");
    };
    let [parent] = parents[..] else {
        panic!("the Scala 3.9.0 trait oracle must contain only Object: {parents:?}");
    };
    let Type::TypeRef {
        target: dotty_core::types::TypeRefTarget::Symbol(object),
        ..
    } = store.types.get(parent)
    else {
        panic!(
            "the trait's sole parent must be Object: {:?}",
            store.types.get(parent)
        );
    };
    let object = store.symbols.get(*object);
    assert_eq!(object.kind, SymbolKind::Class);
    assert_eq!(store.names.resolve(object.name.text()), "Object");
    let lang = store.symbols.get(object.owner.unwrap());
    assert_eq!(store.names.resolve(lang.name.text()), "lang");
    let java = store.symbols.get(lang.owner.unwrap());
    assert_eq!(store.names.resolve(java.name.text()), "java");
}
