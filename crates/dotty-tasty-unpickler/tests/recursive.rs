//! Milestone 4a: recursive types (`RECtype`, `RECthis`) and refinements
//! (`REFINEDtype`).
//!
//! Small synthetic wire files cover malformed shapes, identity and rollback;
//! the real Scala 3.9.0 fixture `tests/fixtures/semantic/RecursiveRefined.scala`
//! is used further down.
use dotty_core::Definitions;
use dotty_core::Packages;
use dotty_core::ids::TypeId;
use dotty_core::store::SemanticStore;
use dotty_core::symbols::SymbolOrigin;
use dotty_core::types::Type;
use dotty_tasty::tasty::TastyFile;
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

const AND: u8 = 165;
const FLEXIBLE: u8 = 193;
const PARAM: u8 = 172;
const RECTYPE: u8 = 100;
const SHARED: u8 = 61;
const TYPEREFPKG: u8 = 65;
/// `IMPORTED`, a one-`Nat` form the type pass does not decode.
const UNSUPPORTED: u8 = 75;

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

fn nat(value: u8) -> u8 {
    assert!(value < 128);
    0x80 | value
}

fn length_node(tag: u8, payload: &[u8]) -> Vec<u8> {
    let mut node = vec![tag, nat(u8::try_from(payload.len()).unwrap())];
    node.extend(payload);
    node
}

/// A file whose ASTs are exactly `ast`, over the names `ASTs`, `p`, `m`.
fn file_with_ast(ast: &[u8]) -> Vec<u8> {
    use dotty_tasty::tasty::{Header, NameTable, RawName, Section, SectionTable};
    let names = NameTable::from_entries(vec![
        RawName::Utf8("ASTs".to_owned()),
        RawName::Utf8("p".to_owned()),
        RawName::Utf8("m".to_owned()),
    ])
    .unwrap();
    TastyFile::from_parts(
        Header {
            major_version: 28,
            minor_version: 9,
            experimental_version: 0,
            tooling_version: "Scala 3.9.0".to_owned(),
            uuid: [0; 16],
        },
        names,
        SectionTable::from_sections(vec![Section::new(0, ast)]),
    )
    .unwrap()
    .encode()
    .unwrap()
}

/// `TYPEREFpkg p`.
fn package_ref() -> Vec<u8> {
    vec![TYPEREFPKG, nat(1)]
}

/// `PARAMtype Length binder_ASTRef paramNum_Nat`.
fn param_type(binder: u8, number: u8) -> Vec<u8> {
    length_node(PARAM, &[nat(binder), nat(number)])
}

/// `RECtype Type`: a tag and its one child tree, no length.
fn rec_type(parent: &[u8]) -> Vec<u8> {
    let mut node = vec![RECTYPE];
    node.extend(parent);
    node
}

/// A file whose only top-level node is a `FLEXIBLEtype` wrapper (a top-level
/// node must be length-prefixed) around the one type `child`, at address 2.
fn file_with(child: &[u8]) -> Vec<u8> {
    file_with_ast(&length_node(FLEXIBLE, child))
}

/// A file whose only top-level node is an `ANDtype` over two children, the
/// first at address 2.
fn file_with_two(first: &[u8], second: &[u8]) -> Vec<u8> {
    let mut payload = first.to_vec();
    payload.extend(second);
    file_with_ast(&length_node(AND, &payload))
}

fn unpickler_for<'a>(
    file: &'a TastyFile<'a>,
    session: &'a mut Session,
) -> TastyUnpickler<'a, 'a, 'a> {
    let mut packages = Packages::new();
    packages.enter(&mut session.store, SymbolOrigin::Synthetic, &["p"]);
    TastyUnpickler::with_packages(file, &mut session.store, session.definitions, packages)
}

const REC_AT: u32 = 2;

fn recursive_parent(store: &SemanticStore, id: TypeId) -> TypeId {
    match store.types.get(id) {
        Type::Recursive { parent } => *parent,
        other => panic!("not a recursive type: {other:?}"),
    }
}

// RECtype

#[test]
fn a_recursive_type_owns_one_id_and_its_parent_is_decoded() {
    let bytes = file_with(&rec_type(&package_ref()));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let id = unpickler.unpickle_type(REC_AT).unwrap();
    assert_eq!(unpickler.index().type_at(REC_AT), Some(id));
    assert_eq!(unpickler.unpickle_type(REC_AT), Ok(id));
    drop(unpickler);

    let parent = recursive_parent(&session.store, id);
    assert!(matches!(
        session.store.types.get(parent),
        Type::TypeRef { .. }
    ));
}

#[test]
fn nested_recursive_types_have_distinct_ids() {
    let bytes = file_with(&rec_type(&rec_type(&package_ref())));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let outer = unpickler.unpickle_type(REC_AT).unwrap();
    let inner = unpickler.index().type_at(REC_AT + 1).unwrap();
    drop(unpickler);

    assert_ne!(outer, inner);
    assert_eq!(recursive_parent(&session.store, outer), inner);
}

#[test]
fn a_shared_link_to_a_recursive_type_returns_its_exact_id() {
    let rec = rec_type(&package_ref());
    let shared_at = 2 + u8::try_from(rec.len()).unwrap();
    let bytes = file_with_two(&rec, &[SHARED, nat(2)]);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let id = unpickler.unpickle_type(REC_AT).unwrap();
    assert_eq!(unpickler.unpickle_type(u32::from(shared_at)), Ok(id));
    drop(unpickler);
    assert!(matches!(
        session.store.types.get(id),
        Type::Recursive { .. }
    ));

    // A link decoded first builds the one recursive type.
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    let linked = unpickler.unpickle_type(u32::from(shared_at)).unwrap();
    assert_eq!(unpickler.unpickle_type(REC_AT), Ok(linked));
}

#[test]
fn a_recursive_parent_that_links_to_itself_is_a_cycle_not_a_panic() {
    // As with a link to a lambda still being built: Dotty registers the type
    // before reading its parent, so the link resolves to the reserved id.
    let bytes = file_with(&rec_type(&[SHARED, nat(2)]));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let id = unpickler.unpickle_type(REC_AT).unwrap();
    drop(unpickler);
    assert_eq!(recursive_parent(&session.store, id), id);
}

#[test]
fn a_parameter_type_cannot_name_a_recursive_binder() {
    // A `PARAMtype` (at 3) naming the `RECtype` (at 2) that contains it: the
    // binder is pending, and binds no parameters.
    let bytes = file_with(&rec_type(&param_type(2, 0)));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    let before = unpickler.index().type_count();

    let result = unpickler.unpickle_type(REC_AT);
    assert!(
        matches!(
            result,
            Err(UnpickleError::InvalidBinderKind { from: 3, .. })
        ),
        "{result:?}"
    );
    assert_eq!(unpickler.index().type_count(), before);
    assert_eq!(unpickler.index().type_at(REC_AT), None);
}

#[test]
fn a_parameter_type_cannot_name_a_completed_recursive_type_either() {
    // The recursive type is decoded first; a later `PARAMtype` naming it is
    // refused by kind (it is neither a lambda, a poly nor a method).
    let rec = rec_type(&package_ref());
    let param_at = 2 + u8::try_from(rec.len()).unwrap();
    let bytes = file_with_two(&rec, &param_type(2, 0));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    unpickler.unpickle_type(REC_AT).unwrap();

    let result = unpickler.unpickle_type(u32::from(param_at));
    assert!(
        matches!(result, Err(UnpickleError::InvalidBinderKind { .. })),
        "{result:?}"
    );
}

#[test]
fn a_failure_after_the_id_is_published_leaves_nothing_behind() {
    let bytes = file_with(&rec_type(&[UNSUPPORTED, nat(1)]));
    let next_id_after = |fail: bool| {
        let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
        let mut session = Session::new();
        let mut unpickler = unpickler_for(&file, &mut session);
        if fail {
            let before = unpickler.index().type_count();
            let result = unpickler.unpickle_type(REC_AT);
            assert!(
                matches!(
                    result,
                    Err(UnpickleError::UnsupportedType {
                        tag: UNSUPPORTED,
                        ..
                    })
                ),
                "{result:?}"
            );
            assert_eq!(unpickler.index().type_count(), before);
            assert_eq!(unpickler.index().type_at(REC_AT), None);
        }
        drop(unpickler);
        session.store.types.alloc(Type::NoType)
    };

    // The reserved slot went back with the rest.
    assert_eq!(next_id_after(true), next_id_after(false));
}

#[test]
fn a_member_lookup_through_a_recursive_type_still_being_decoded_is_an_error_not_a_panic() {
    // `TYPEREF p <link to the RECtype>`: a member selected by name from the
    // binder whose slot is reserved but unfilled.
    let bytes = file_with(&rec_type(&[117, nat(1), SHARED, nat(2)]));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let result = unpickler.unpickle_type(REC_AT);
    assert!(
        matches!(
            result,
            Err(UnpickleError::UnsupportedResolutionPrefix { .. })
        ),
        "{result:?}"
    );
    assert_eq!(unpickler.index().type_at(REC_AT), None);
}
