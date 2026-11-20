//! Default metadata at symbol entry, independent of completing any method.
use dotty_core::{Definitions, SemanticStore, SymbolId};
use dotty_tasty::tasty::TastyFile;
use dotty_tasty_unpickler::tasty_unpickler::TastyUnpickler;

const READER: &[u8] = include_bytes!(
    "../../dotty-typer/tests/fixtures/contextual-arguments/binary/defaults/Reader.tasty"
);

fn enter(file: &TastyFile<'_>) -> (SemanticStore, Vec<SymbolId>) {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut reader = TastyUnpickler::new(file, &mut store, defs);
    let symbols = reader.enter_symbols().unwrap().entered_symbols();
    (store, symbols)
}

fn named(store: &SemanticStore, symbols: &[SymbolId], name: &str) -> SymbolId {
    let matches: Vec<_> = symbols
        .iter()
        .copied()
        .filter(|s| store.names.resolve(store.symbols.get(*s).name.text()) == name)
        .collect();
    let [symbol] = matches.as_slice() else {
        panic!("expected exactly one {name}: {matches:?}")
    };
    *symbol
}

#[test]
fn scala_method_defaults_use_the_flattened_term_ordinal() {
    let file = TastyFile::parse_scala_3_9(READER).unwrap();
    let (store, symbols) = enter(&file);
    for (name, getter_name, ordinal) in [
        ("read", "read$default$1", 0),
        ("offset", "offset$default$2", 1),
    ] {
        let method = named(&store, &symbols, name);
        let getter = named(&store, &symbols, getter_name);
        assert_eq!(
            store.default_getter(method, ordinal),
            Some(getter),
            "{name}"
        );
        assert_eq!(store.default_getter_origin(getter), Some((method, ordinal)));
        assert_eq!(store.default_getter(method, 1 - ordinal), None);
    }
}
