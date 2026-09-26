use dotty_core::ast::TreeKind;
use dotty_core::names::{Name, Namespace};
use dotty_core::types::{TermRefTarget, Type, TypeRefTarget};
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceId, SourceText, SymbolId, SymbolKind,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;
use dotty_parser::parse_compilation_unit;
use dotty_tasty::tasty::TastyFile;
use dotty_tasty_unpickler::tasty_unpickler::TastyUnpickler;
use dotty_typer::SourceTyper;

const WIDGET: &[u8] =
    include_bytes!("../../dotty-tasty-unpickler/tests/fixtures/semantic/Widget.tasty");
const HOLDER: &[u8] =
    include_bytes!("../../dotty-tasty-unpickler/tests/fixtures/semantic/Holder.tasty");

fn package_prefix_symbol(store: &SemanticStore, prefix: dotty_core::TypeId) -> SymbolId {
    let package = match store.types.get(prefix) {
        Type::ThisType { class } => *class,
        Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        }
        | Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } => *symbol,
        other => panic!("expected a package prefix, got {other:?}"),
    };
    assert_eq!(store.symbols.get(package).kind, SymbolKind::Package);
    package
}

#[test]
fn source_and_tasty_references_use_the_same_package_prefix_identity() {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();

    let widget_file = TastyFile::parse_scala_3_9(WIDGET).unwrap();
    let (widget_index, next_packages) = {
        let mut unpickler =
            TastyUnpickler::with_packages(&widget_file, &mut store, definitions, packages);
        unpickler.enter_symbols().unwrap();
        unpickler.into_parts()
    };
    packages = next_packages;
    let root = packages.symbol::<&str>(&[]).unwrap();
    let widget_name = Name::new(store.names.intern("Widget"), Namespace::Type);
    let widget = store
        .scopes
        .get(packages.scope_of(root).unwrap())
        .lookup(&widget_name)
        .unwrap();
    assert_eq!(widget_index.symbol_at(0), Some(root));

    let holder_file = TastyFile::parse_scala_3_9(HOLDER).unwrap();
    let addresses: Vec<_> = holder_file
        .ast_address_index()
        .unwrap()
        .iter_nodes()
        .filter(|node| node.tag == 117)
        .map(|node| u32::try_from(node.offset).unwrap())
        .collect();
    let (tasty_types, next_packages) = {
        let mut unpickler =
            TastyUnpickler::with_packages(&holder_file, &mut store, definitions, packages);
        unpickler.enter_symbols().unwrap();
        let mut found = Vec::new();
        for address in addresses {
            let Ok(ty) = unpickler.unpickle_type(address) else {
                continue;
            };
            found.push(ty);
        }
        let (_, packages) = unpickler.into_parts();
        (found, packages)
    };
    packages = next_packages;
    let tasty_prefix = tasty_types
        .iter()
        .find_map(|ty| match store.types.get(*ty) {
            Type::TypeRef { prefix, .. }
                if store.types.get(*ty).reference_symbol() == Some(widget) =>
            {
                Some(*prefix)
            }
            _ => None,
        })
        .expect("Holder contains a type reference to Widget");

    let source = SourceId::from_index(73);
    let source_text = "val x: Widget = 1";
    let scanner = ContextualScanner::new(source_text).unwrap();
    let parsed = parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "Source.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let (value, type_tree) = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::ValDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == "x" =>
            {
                Some((index.symbol_at(source, tree).unwrap(), definition.tpt))
            }
            _ => None,
        })
        .unwrap();
    let source_prefix = {
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let projected = typer.complete_symbol(value).unwrap();
        let Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(symbol),
        } = typer.store().types.get(projected)
        else {
            panic!("expected the source Widget reference")
        };
        assert_eq!(*symbol, widget);
        assert_eq!(
            typer.source_type_index().type_at(source, type_tree),
            Some(projected)
        );
        *prefix
    };

    assert_eq!(
        package_prefix_symbol(&store, source_prefix),
        package_prefix_symbol(&store, tasty_prefix)
    );
    assert_eq!(package_prefix_symbol(&store, source_prefix), root);
}
