//! Milestone 2c1: compositional types (`APPLIEDtype`, `ANDtype`, `ORtype`,
//! `SUPERtype`, `BYNAMEtype`), over the real Scala 3.9.0 `Compound.tasty`
//! (`tests/fixtures/semantic/Compound.scala`).
//!
//! Addresses of the compound nodes the compiler wrote in the fixture:
//!
//! | address | node                                                     |
//! |---------|----------------------------------------------------------|
//! | 370     | `APPLIEDtype` `Box[Item]`                                |
//! | 414     | `APPLIEDtype` `Pair[Item, Box[Item]]`                    |
//! | 453     | `APPLIEDtype` `Box[Box[Item]]`                           |
//! | 835     | `ANDtype` `A & B`                                        |
//! | 897     | `ANDtype` `(A & B) & C`                                  |
//! | 866     | `ORtype` `A | B`                                          |
//! | 940     | `ORtype` `A | (B | C)`, whose right operand is 945      |
//! | 1026    | `APPLIEDtype` of `scala.Function0`, an unentered package |
//!
//! Real `SHAREDtype` links: 422 and 458 -> 370, 796 -> 453, 899 -> 835.
//! The compiler writes no `SUPERtype` and only tree-level `BYNAMEtpt` in this
//! unit, so those two are reached by retagging nodes of the same wire shape;
//! every patch first asserts what it overwrites.
use dotty_core::Definitions;
use dotty_core::ids::TypeId;
use dotty_core::store::SemanticStore;
use dotty_core::types::Type;
use dotty_tasty::tasty::{StandardSection, TastyFile};
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

const COMPOUND: &[u8] = include_bytes!("fixtures/semantic/Compound.tasty");

fn show(store: &SemanticStore, id: TypeId) -> String {
    let name = |symbol| {
        let symbol = store.symbols.get(symbol);
        store.names.resolve(symbol.name.text()).to_string()
    };
    match store.types.get(id) {
        Type::TypeRef { symbol, .. } => name(*symbol),
        Type::TermRef { symbol, .. } => format!("term {}", name(*symbol)),
        Type::ThisType { class } => format!("this {}", name(*class)),
        Type::Applied { tycon, args } => {
            let args: Vec<_> = args.iter().map(|arg| show(store, *arg)).collect();
            format!("{}[{}]", show(store, *tycon), args.join(", "))
        }
        Type::And { left, right } => format!("({} & {})", show(store, *left), show(store, *right)),
        Type::Or { left, right } => format!("({} | {})", show(store, *left), show(store, *right)),
        Type::ByName { result } => format!("=> {}", show(store, *result)),
        Type::SuperType {
            this_type,
            super_type,
        } => format!(
            "super({}, {})",
            show(store, *this_type),
            show(store, *super_type)
        ),
        other => format!("{other:?}"),
    }
}

const SINGLE: u32 = 370;
const SEVERAL: u32 = 414;
const NESTED: u32 = 453;
const AND: u32 = 835;
const AND_CHAIN: u32 = 897;
const OR: u32 = 866;
const OR_DEEP: u32 = 940;
const OR_DEEP_RIGHT: u32 = 945;
const EXTERNAL_APPLIED: u32 = 1026;
const SHARED_SINGLE: u32 = 422;
const SHARED_NESTED: u32 = 796;
const SHARED_AND: u32 = 899;
const BOX_REFERENCE: u32 = 356;

struct Session {
    store: SemanticStore,
    definitions: Definitions,
}

impl Session {
    fn new() -> Self {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        Self { store, definitions }
    }
}

/// Decodes each of `addresses` in one fresh session and renders the results.
fn decode_all(bytes: &[u8], addresses: &[u32]) -> Vec<String> {
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    unpickler.enter_symbols().unwrap();
    let ids: Vec<TypeId> = addresses
        .iter()
        .map(|at| unpickler.unpickle_type(*at).unwrap())
        .collect();
    ids.into_iter().map(|id| show(&session.store, id)).collect()
}

#[test]
fn an_applied_type_keeps_its_constructor_and_arguments_in_order() {
    assert_eq!(
        decode_all(COMPOUND, &[SINGLE, SEVERAL, NESTED]),
        ["Box[Item]", "Pair[Item, Box[Item]]", "Box[Box[Item]]"]
    );
}

#[test]
fn an_applied_type_is_the_applied_core_type_with_its_children_as_type_ids() {
    let file = TastyFile::parse_scala_3_9(COMPOUND).unwrap();
    let mut session = Session::new();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    unpickler.enter_symbols().unwrap();
    let nested = unpickler.unpickle_type(NESTED).unwrap();
    let store = &session.store;
    let Type::Applied { tycon, args } = store.types.get(nested) else {
        panic!("not applied");
    };
    assert_eq!(show(store, *tycon), "Box");
    let [inner] = args.as_slice() else {
        panic!("one argument expected");
    };
    let Type::Applied { tycon, args } = store.types.get(*inner) else {
        panic!("the argument is not applied");
    };
    assert_eq!(show(store, *tycon), "Box");
    assert_eq!(show(store, args[0]), "Item");
}

/// Overwrites the byte at AST address `at`, after checking it is `from`.
fn retagged(bytes: &[u8], at: usize, from: u8, to: u8) -> Vec<u8> {
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let payload = file.section(StandardSection::Asts).unwrap().payload;
    assert_eq!(
        payload[at], from,
        "the fixture no longer has {from} at {at}"
    );
    let start = payload.as_ptr() as usize - bytes.as_ptr() as usize;
    let mut patched = bytes.to_vec();
    patched[start + at] = to;
    patched
}

#[test]
fn decoding_an_applied_type_twice_returns_the_same_type_id() {
    let file = TastyFile::parse_scala_3_9(COMPOUND).unwrap();
    let mut session = Session::new();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    unpickler.enter_symbols().unwrap();
    let first = unpickler.unpickle_type(SEVERAL).unwrap();
    assert_eq!(unpickler.unpickle_type(SEVERAL), Ok(first));
    assert_eq!(unpickler.index().type_at(SEVERAL), Some(first));
}

#[test]
fn a_shared_type_reuses_the_exact_id_of_the_compound_node_it_points_at() {
    let file = TastyFile::parse_scala_3_9(COMPOUND).unwrap();
    let mut session = Session::new();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    unpickler.enter_symbols().unwrap();
    for (shared, target) in [
        (SHARED_SINGLE, SINGLE),
        (SHARED_NESTED, NESTED),
        (SHARED_AND, AND),
    ] {
        let direct = unpickler.unpickle_type(target).unwrap();
        assert_eq!(unpickler.unpickle_type(shared), Ok(direct), "@{shared}");
        // A `SHAREDtype` allocates nothing and owns no entry.
        assert_eq!(unpickler.index().type_at(shared), None);
    }
}

#[test]
fn a_shared_type_decoded_first_creates_the_compound_node_it_points_at() {
    let file = TastyFile::parse_scala_3_9(COMPOUND).unwrap();
    let mut session = Session::new();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    unpickler.enter_symbols().unwrap();
    let through_link = unpickler.unpickle_type(SHARED_NESTED).unwrap();
    assert_eq!(unpickler.unpickle_type(NESTED), Ok(through_link));
}

#[test]
fn an_applied_type_child_that_needs_a_missing_package_reports_that_error() {
    let file = TastyFile::parse_scala_3_9(COMPOUND).unwrap();
    let mut session = Session::new();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    unpickler.enter_symbols().unwrap();
    // The applied type itself is supported; its constructor `scala.Function0`
    // is in a package nobody entered.
    assert!(matches!(
        unpickler.unpickle_type(EXTERNAL_APPLIED),
        Err(UnpickleError::UnresolvedPackage { package, .. }) if package == "scala"
    ));
}

#[test]
fn the_constructor_of_an_applied_type_can_be_resolved_by_name() {
    // Retag the constructor `TYPEREFsymbol Box` into the name-based
    // `TYPEREF Box` (same wire shape), so it must be found by looking `Box` up
    // in its `THIS` prefix.
    let file = TastyFile::parse_scala_3_9(COMPOUND).unwrap();
    let box_name = file.names().find_utf8("Box").unwrap();
    let box_name = u8::try_from(box_name).unwrap();
    let patched = retagged_named(COMPOUND, BOX_REFERENCE as usize, 116, 117, box_name);
    assert_eq!(decode_all(&patched, &[SINGLE]), ["Box[Item]"]);

    // The name, not the old address, decides the symbol: naming `Item` in the
    // same place makes the constructor `Item`.
    let item_name = u8::try_from(file.names().find_utf8("Item").unwrap()).unwrap();
    let renamed = retagged_named(COMPOUND, BOX_REFERENCE as usize, 116, 117, item_name);
    assert_eq!(decode_all(&renamed, &[SINGLE]), ["Item[Item]"]);
}

/// Retags the `tag natural` node at `at` to a name-based reference to `value`.
fn retagged_named(bytes: &[u8], at: usize, from: u8, tag: u8, value: u8) -> Vec<u8> {
    assert!(value < 128);
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let payload = file.section(StandardSection::Asts).unwrap().payload;
    assert_eq!(payload[at], from, "unexpected node at {at}");
    let start = payload.as_ptr() as usize - bytes.as_ptr() as usize;
    let mut patched = bytes.to_vec();
    patched[start + at] = tag;
    if payload[at + 1] & 0x80 != 0 {
        patched[start + at + 1] = 0x80 | value;
    } else {
        assert!(payload[at + 2] & 0x80 != 0, "the natural at {at} is longer");
        patched[start + at + 1] = 0;
        patched[start + at + 2] = 0x80 | value;
    }
    patched
}

#[test]
fn a_failure_after_several_children_decoded_leaves_no_trace() {
    // `Pair[Item, Box[Item]]`: retag the last argument (a `SHAREDtype` at 422)
    // to a tag the decoder does not support. By then the constructor, the
    // first argument and the prefixes below them have all been allocated.
    let patched = retagged(COMPOUND, SHARED_SINGLE as usize, 61, 66);
    let file = TastyFile::parse_scala_3_9(&patched).unwrap();
    let mut session = Session::new();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    unpickler.enter_symbols().unwrap();
    let before = unpickler.index().type_count();
    assert!(matches!(
        unpickler.unpickle_type(SEVERAL),
        Err(UnpickleError::UnsupportedType { tag: 66, .. })
    ));
    assert_eq!(unpickler.index().type_count(), before);
    assert_eq!(unpickler.index().type_at(SEVERAL), None);
    // What survives is what a session that never made the call would hold:
    // the next allocation reuses the freed id.
    let after_failure = unpickler.unpickle_type(SINGLE).unwrap();
    drop(unpickler);
    let mut clean = Session::new();
    let mut fresh = TastyUnpickler::new(&file, &mut clean.store, clean.definitions);
    fresh.enter_symbols().unwrap();
    assert_eq!(fresh.unpickle_type(SINGLE), Ok(after_failure));
}

#[test]
fn an_intersection_keeps_its_operand_order() {
    assert_eq!(decode_all(COMPOUND, &[AND]), ["(A & B)"]);
}

#[test]
fn a_nested_intersection_is_neither_flattened_nor_reordered() {
    assert_eq!(decode_all(COMPOUND, &[AND_CHAIN]), ["((A & B) & C)"]);
    // The core type is a binary tree of type ids.
    let file = TastyFile::parse_scala_3_9(COMPOUND).unwrap();
    let mut session = Session::new();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    unpickler.enter_symbols().unwrap();
    let chain = unpickler.unpickle_type(AND_CHAIN).unwrap();
    let Type::And { left, right } = session.store.types.get(chain) else {
        panic!("not an intersection");
    };
    assert!(matches!(session.store.types.get(*left), Type::And { .. }));
    assert_eq!(show(&session.store, *right), "C");
}

#[test]
fn a_union_keeps_its_operand_order() {
    assert_eq!(decode_all(COMPOUND, &[OR]), ["(A | B)"]);
}

#[test]
fn a_nested_union_keeps_the_nesting_the_compiler_wrote() {
    assert_eq!(
        decode_all(COMPOUND, &[OR_DEEP, OR_DEEP_RIGHT]),
        ["(A | (B | C))", "(B | C)"]
    );
}

#[test]
fn a_nested_union_operand_is_the_same_type_as_its_own_node() {
    // The right operand of 940 is the node at 945: one address, one id.
    let file = TastyFile::parse_scala_3_9(COMPOUND).unwrap();
    let mut session = Session::new();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    unpickler.enter_symbols().unwrap();
    let deep = unpickler.unpickle_type(OR_DEEP).unwrap();
    let index = unpickler.into_index();
    let Type::Or { right, .. } = session.store.types.get(deep) else {
        panic!("not a union");
    };
    let right = *right;
    assert_eq!(index.type_at(OR_DEEP_RIGHT), Some(right));
}

#[test]
fn intersections_and_unions_do_not_intern_by_shape() {
    // `A & B` and `A | B` share operands but are different nodes and types.
    let file = TastyFile::parse_scala_3_9(COMPOUND).unwrap();
    let mut session = Session::new();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    unpickler.enter_symbols().unwrap();
    let and = unpickler.unpickle_type(AND).unwrap();
    let or = unpickler.unpickle_type(OR).unwrap();
    assert_ne!(and, or);
    assert_eq!(unpickler.unpickle_type(AND), Ok(and));
}

#[test]
fn a_failing_operand_is_reported_as_itself_and_rolls_back() {
    // Make the right operand of `(A & B) & C` unsupported.
    let file = TastyFile::parse_scala_3_9(COMPOUND).unwrap();
    let right_operand = *children_of(&file, AND_CHAIN).last().unwrap();
    drop(file);
    let patched = retagged(COMPOUND, right_operand as usize, 61, 66);
    let file = TastyFile::parse_scala_3_9(&patched).unwrap();
    let mut session = Session::new();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    unpickler.enter_symbols().unwrap();
    let before = unpickler.index().type_count();
    assert!(matches!(
        unpickler.unpickle_type(AND_CHAIN),
        Err(UnpickleError::UnsupportedType { tag: 66, .. })
    ));
    assert_eq!(unpickler.index().type_count(), before);
    assert_eq!(unpickler.index().type_at(AND_CHAIN), None);
}

/// The absolute addresses of the direct children of the node at `at`.
fn children_of(file: &TastyFile<'_>, at: u32) -> Vec<u32> {
    file.ast_address_index()
        .unwrap()
        .iter_tree_edges()
        .filter(|edge| edge.parent.offset == at as usize)
        .map(|edge| u32::try_from(edge.child.offset).unwrap())
        .collect()
}
