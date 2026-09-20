//! Milestone 3b: `POLYtype`, `METHODtype` and `PARAMtype` on all three binder
//! kinds.
//!
//! Small synthetic wire files cover what real compiler output cannot
//! (malformed shapes, rollback, on-demand binders); the real Scala 3.9.0
//! fixture `tests/fixtures/semantic/Methodic.scala` follows below.
use dotty_core::Definitions;
use dotty_core::Packages;
use dotty_core::ids::TypeId;
use dotty_core::store::SemanticStore;
use dotty_core::symbols::SymbolOrigin;
use dotty_core::types::{PolyType, Type, Variance};
use dotty_tasty::tasty::TastyFile;
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

const AND: u8 = 165;
const FLEXIBLE: u8 = 193;
const POLY: u8 = 169;
const PARAM: u8 = 172;
const TYPEBOUNDS: u8 = 163;
const TYPEREFPKG: u8 = 65;
const SHARED: u8 = 61;

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

/// A Nat holding a small value: the value with the stop bit.
fn nat(value: u8) -> u8 {
    assert!(value < 128);
    0x80 | value
}

/// `tag length payload`, the only shape a top-level AST node may have.
fn length_node(tag: u8, payload: &[u8]) -> Vec<u8> {
    let mut node = vec![tag, nat(u8::try_from(payload.len()).unwrap())];
    node.extend(payload);
    node
}

/// A file whose ASTs are exactly `ast`, over the names `ASTs`, `p`, `q`.
fn file_with_ast(ast: &[u8]) -> Vec<u8> {
    use dotty_tasty::tasty::{Header, NameTable, RawName, Section, SectionTable};
    let names = NameTable::from_entries(vec![
        RawName::Utf8("ASTs".to_owned()),
        RawName::Utf8("p".to_owned()),
        RawName::Utf8("q".to_owned()),
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

/// `PARAMtype Length binder_ASTRef paramNum_Nat`.
fn param_type(binder: u8, number: u8) -> Vec<u8> {
    length_node(PARAM, &[nat(binder), nat(number)])
}

/// `TYPEREFpkg p`.
fn package_ref() -> Vec<u8> {
    vec![TYPEREFPKG, nat(1)]
}

/// The alias bounds `= p`, with the parameter name `p` after them.
fn alias_param() -> Vec<u8> {
    let mut param = length_node(TYPEBOUNDS, &package_ref());
    param.push(nat(1));
    param
}

/// A `POLYtype` with `arity` parameters (alias bounds, name `p`) and `result`.
fn poly_node(result: &[u8], arity: usize) -> Vec<u8> {
    let mut payload = result.to_vec();
    for _ in 0..arity {
        payload.extend(alias_param());
    }
    length_node(POLY, &payload)
}

/// A file whose only top-level node is a `FLEXIBLEtype` wrapper (a top-level
/// node must be length-prefixed) around the one type `child`, at address 2.
fn file_with(child: &[u8]) -> Vec<u8> {
    file_with_ast(&length_node(FLEXIBLE, child))
}

fn unpickler_for<'a>(
    file: &'a TastyFile<'a>,
    session: &'a mut Session,
) -> TastyUnpickler<'a, 'a, 'a> {
    let mut packages = Packages::new();
    packages.enter(&mut session.store, SymbolOrigin::Synthetic, &["p"]);
    TastyUnpickler::with_packages(file, &mut session.store, session.definitions, packages)
}

fn poly(store: &SemanticStore, id: TypeId) -> &PolyType {
    match store.types.get(id) {
        Type::Poly(poly) => poly,
        other => panic!("not a poly type: {other:?}"),
    }
}

// Poly

/// The poly is the wrapper's first child, at address 2; its result follows
/// its two header bytes, at 4.
const POLY_AT: u32 = 2;
const RESULT_AT: u32 = 4;

#[test]
fn a_poly_owns_its_id_and_its_result_names_that_exact_id() {
    let bytes = file_with(&poly_node(&param_type(2, 0), 1));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let id = unpickler.unpickle_type(POLY_AT).unwrap();
    assert_eq!(unpickler.index().type_at(POLY_AT), Some(id));
    drop(unpickler);

    let poly = poly(&session.store, id);
    assert_eq!(poly.params.len(), 1);
    assert_eq!(
        session.store.types.get(poly.result),
        &Type::ParamRef {
            binder: id,
            index: 0
        }
    );
}

#[test]
fn a_poly_parameter_has_its_name_bounds_and_is_invariant() {
    let bytes = file_with(&poly_node(&package_ref(), 1));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let id = unpickler.unpickle_type(POLY_AT).unwrap();
    drop(unpickler);

    let poly = poly(&session.store, id);
    let name = poly.params[0].name.as_name();
    assert_eq!(session.store.names.resolve(name.text()), "p");
    assert_eq!(poly.params[0].variance, Variance::Invariant);
    assert!(matches!(
        session.store.types.get(poly.params[0].bounds),
        Type::AliasingBounds { .. }
    ));
}

#[test]
fn a_poly_without_parameters_is_malformed_and_publishes_nothing() {
    // The wire grammar (`Type NameRef*`) allows it; Dotty's `PolyType` does not.
    let bytes = file_with(&poly_node(&package_ref(), 0));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    let before = unpickler.index().type_count();

    assert!(matches!(
        unpickler.unpickle_type(POLY_AT),
        Err(UnpickleError::MalformedType {
            address: POLY_AT,
            ..
        })
    ));
    assert_eq!(unpickler.index().type_count(), before);
    assert_eq!(unpickler.index().type_at(POLY_AT), None);
}

#[test]
fn a_poly_parameter_info_that_is_not_bounds_is_an_error() {
    let mut payload = package_ref();
    payload.extend(package_ref());
    payload.push(nat(1));
    let bytes = file_with(&length_node(POLY, &payload));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let result = unpickler.unpickle_type(POLY_AT);
    assert!(
        matches!(
            result,
            Err(UnpickleError::InvalidTypeParameterBounds { index: 0, .. })
        ),
        "{result:?}"
    );
    assert_eq!(unpickler.index().type_at(POLY_AT), None);
}

#[test]
fn a_poly_can_bound_a_parameter_by_itself() {
    // `[A = A]`: the parameter's own bounds name the poly (an F-bound).
    let mut payload = package_ref();
    payload.extend(length_node(TYPEBOUNDS, &param_type(2, 0)));
    payload.push(nat(1));
    let bytes = file_with(&length_node(POLY, &payload));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let id = unpickler.unpickle_type(POLY_AT).unwrap();
    drop(unpickler);

    let bounds = poly(&session.store, id).params[0].bounds;
    let Type::AliasingBounds { alias } = session.store.types.get(bounds) else {
        panic!("not alias bounds");
    };
    assert_eq!(
        session.store.types.get(*alias),
        &Type::ParamRef {
            binder: id,
            index: 0
        }
    );
}

#[test]
fn the_same_poly_address_is_the_same_id_and_a_shared_link_returns_it() {
    let mut children = poly_node(&package_ref(), 1);
    let shared_at = 2 + children.len();
    children.extend([SHARED, nat(2)]);
    // An `ANDtype` wrapper holds the poly and the link.
    let bytes = file_with_ast(&length_node(AND, &children));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let id = unpickler.unpickle_type(POLY_AT).unwrap();
    assert_eq!(unpickler.unpickle_type(POLY_AT), Ok(id));
    assert_eq!(
        unpickler.unpickle_type(u32::try_from(shared_at).unwrap()),
        Ok(id)
    );
}

#[test]
fn a_parameter_type_asked_first_decodes_the_poly_on_demand_once() {
    let bytes = file_with(&poly_node(&param_type(2, 0), 1));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let reference = unpickler.unpickle_type(RESULT_AT).unwrap();
    let binder = unpickler.index().type_at(POLY_AT).unwrap();
    let count = unpickler.index().type_count();
    // The poly's own decode reached the `PARAMtype` again: still one node.
    assert_eq!(unpickler.unpickle_type(POLY_AT), Ok(binder));
    assert_eq!(unpickler.index().type_count(), count);
    drop(unpickler);

    assert_eq!(poly(&session.store, binder).result, reference);
    assert_eq!(
        session.store.types.get(reference),
        &Type::ParamRef { binder, index: 0 }
    );
}

#[test]
fn a_poly_parameter_index_past_the_arity_is_an_error() {
    let bytes = file_with(&poly_node(&param_type(2, 1), 1));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let result = unpickler.unpickle_type(POLY_AT);
    assert!(
        matches!(
            result,
            Err(UnpickleError::InvalidParameterIndex {
                address: RESULT_AT,
                index: 1,
                arity: 1,
                ..
            })
        ),
        "{result:?}"
    );
    assert_eq!(unpickler.index().type_at(POLY_AT), None);
}

#[test]
fn a_failure_after_one_bound_is_decoded_leaves_nothing_behind() {
    // Two parameters; the second's info is an unsupported form (`66`), so the
    // poly is reserved, published, and one bound decoded before it fails.
    let mut payload = package_ref();
    payload.extend(alias_param());
    payload.extend([66, nat(1), nat(2)]);
    let bytes = file_with(&length_node(POLY, &payload));
    let next_id_after = |fail: bool| {
        let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
        let mut session = Session::new();
        let mut unpickler = unpickler_for(&file, &mut session);
        if fail {
            let before = unpickler.index().type_count();
            let result = unpickler.unpickle_type(POLY_AT);
            assert!(
                matches!(result, Err(UnpickleError::UnsupportedType { tag: 66, .. })),
                "{result:?}"
            );
            assert_eq!(unpickler.index().type_count(), before);
            assert_eq!(unpickler.index().type_at(POLY_AT), None);
        }
        drop(unpickler);
        session.store.types.alloc(Type::NoType)
    };

    // Reserved slots go back with the rest: the next id is as if nothing ran.
    assert_eq!(next_id_after(true), next_id_after(false));
}
