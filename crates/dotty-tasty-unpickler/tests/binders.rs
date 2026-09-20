//! Milestone 3a: binder identity (`TYPELAMBDAtype`, `PARAMtype`).
//!
//! Small synthetic wire files cover what real compiler output cannot: a
//! `PARAMtype` that names a wrong or missing binder, and cycles.
use dotty_core::Definitions;
use dotty_core::Packages;
use dotty_core::ids::TypeId;
use dotty_core::names::{Name, Namespace};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{
    Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
};
use dotty_core::types::{Type, TypeLambda, Variance};
use dotty_tasty::tasty::TastyFile;
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

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

/// A file whose ASTs are exactly `ast`, over the names `ASTs`, `p`.
fn file_with_ast(ast: &[u8]) -> Vec<u8> {
    use dotty_tasty::tasty::{Header, NameTable, RawName, Section, SectionTable};
    let names = NameTable::from_entries(vec![
        RawName::Utf8("ASTs".to_owned()),
        RawName::Utf8("p".to_owned()),
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
    length_node(172, &[nat(binder), nat(number)])
}

/// An `ANDtype` wrapper (a top-level node must be length-prefixed) around
/// `TYPEREFpkg p` at address 2 and `children`, which start at address 4.
fn file_with(children: &[u8]) -> Vec<u8> {
    let mut payload = vec![65, nat(1)];
    payload.extend(children);
    file_with_ast(&length_node(165, &payload))
}

fn decode(bytes: &[u8], at: u32) -> Result<TypeId, UnpickleError> {
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let mut session = Session::new();
    let mut packages = Packages::new();
    packages.enter(&mut session.store, SymbolOrigin::Synthetic, &["p"]);
    let mut unpickler =
        TastyUnpickler::with_packages(&file, &mut session.store, session.definitions, packages);
    unpickler.unpickle_type(at)
}

#[test]
fn a_parameter_type_naming_no_node_is_an_invalid_binder_reference() {
    // Address 1 is the length byte of the wrapper, not the start of a node.
    let bytes = file_with(&param_type(1, 0));
    assert_eq!(
        decode(&bytes, 4),
        Err(UnpickleError::InvalidBinderReference { from: 4, binder: 1 })
    );
    let far = file_with(&param_type(100, 0));
    assert_eq!(
        decode(&far, 4),
        Err(UnpickleError::InvalidBinderReference {
            from: 4,
            binder: 100
        })
    );
}

#[test]
fn a_parameter_type_naming_a_non_binder_is_an_invalid_binder_kind() {
    // Address 2 is `TYPEREFpkg p`, a perfectly good type that binds nothing.
    let bytes = file_with(&param_type(2, 0));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut packages = Packages::new();
    packages.enter(&mut session.store, SymbolOrigin::Synthetic, &["p"]);
    let mut unpickler =
        TastyUnpickler::with_packages(&file, &mut session.store, session.definitions, packages);
    let before = unpickler.index().type_count();

    let result = unpickler.unpickle_type(4);
    assert!(
        matches!(
            result,
            Err(UnpickleError::InvalidBinderKind { from: 4, .. })
        ),
        "{result:?}"
    );
    // Nothing of the failed call survives, not even the package reference.
    assert_eq!(unpickler.index().type_count(), before);
    assert_eq!(unpickler.index().type_at(2), None);
    assert_eq!(unpickler.index().type_at(4), None);
}

#[test]
fn a_parameter_type_naming_itself_is_an_error_not_a_stack_overflow() {
    let bytes = file_with(&param_type(4, 0));
    assert!(matches!(
        decode(&bytes, 4),
        Err(UnpickleError::InvalidReferenceTarget { from: 4, .. })
    ));
}

#[test]
fn two_parameter_types_naming_each_other_are_an_error_not_a_stack_overflow() {
    // PARAMtype at 4 names the one at 8 and vice versa.
    let mut children = param_type(8, 0);
    children.extend(param_type(4, 0));
    let bytes = file_with(&children);
    assert!(matches!(
        decode(&bytes, 4),
        Err(UnpickleError::InvalidReferenceTarget { .. })
    ));
}

#[test]
fn a_malformed_parameter_type_payload_is_a_structural_error() {
    // A trailing byte after the two Nats.
    let bytes = file_with(&length_node(172, &[nat(2), nat(0), nat(0)]));
    assert!(matches!(decode(&bytes, 4), Err(UnpickleError::Ast(_))));
}

// Real Scala 3.9.0 output: `tests/fixtures/semantic/Binders.scala`.

const BINDERS: &[u8] = include_bytes!("fixtures/semantic/Binders.tasty");

/// `[A] =>> A`: result `PARAMtype` at 236 naming this node, bounds shared.
const ID: u32 = 234;
const ID_PARAM: u32 = 236;
/// `[A <: Top] =>> A`.
const UPPER: u32 = 288;
/// `[A] =>> [B] =>> A`: the outer lambda, whose result is the inner one; the
/// inner result (357) names the OUTER lambda.
const OUTER: u32 = 353;
const OUTER_INNER: u32 = 355;
const OUTER_PARAM: u32 = 357;
/// `[A] =>> [B] =>> B`: the inner lambda (420), whose result (422) names it.
const INNER_OUTER: u32 = 418;
const INNER: u32 = 420;
const INNER_PARAM: u32 = 422;
/// `[A <: Ord[A]] =>> A`: the parameter's own bound names the lambda.
const F_BOUND: u32 = 544;
/// `[A, B] =>> (A, B)`; the parameters are at 630 and 635.
const TWO: u32 = 623;
const TWO_FIRST: u32 = 630;
const TWO_SECOND: u32 = 635;

/// A session that already holds package `scala` with `Nothing`, `Any` and
/// `Tuple2`, which the unit references but does not define.
fn session_with_scala() -> (Session, Packages) {
    let mut session = Session::new();
    let mut packages = Packages::new();
    let chain = packages.enter(&mut session.store, SymbolOrigin::Synthetic, &["scala"]);
    let scala = chain.last().unwrap();
    for class in ["Nothing", "Any", "Tuple2"] {
        let name = Name::new(session.store.names.intern(class), Namespace::Type);
        let symbol = session.store.symbols.alloc(Symbol {
            name,
            owner: Some(scala.symbol),
            kind: SymbolKind::Class,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        session
            .store
            .scopes
            .get_mut(scala.scope)
            .enter(name, symbol);
    }
    (session, packages)
}

fn lambda(store: &SemanticStore, id: TypeId) -> &TypeLambda {
    match store.types.get(id) {
        Type::TypeLambda(lambda) => lambda,
        other => panic!("expected a type lambda, got {other:?}"),
    }
}

fn param_name(store: &SemanticStore, lambda: &TypeLambda, index: usize) -> String {
    store
        .names
        .resolve(lambda.params[index].name.as_name().text())
        .to_string()
}

macro_rules! unit {
    ($file:ident, $session:ident, $unpickler:ident) => {
        let $file = TastyFile::parse_scala_3_9(BINDERS).unwrap();
        let (mut $session, packages) = session_with_scala();
        let mut $unpickler = TastyUnpickler::with_packages(
            &$file,
            &mut $session.store,
            $session.definitions,
            packages,
        );
        $unpickler.enter_symbols().unwrap();
    };
}

#[test]
fn a_type_lambda_owns_its_id_and_its_result_names_that_exact_id() {
    unit!(file, session, unpickler);
    let id = unpickler.unpickle_type(ID).unwrap();
    drop(unpickler);

    let lambda = lambda(&session.store, id);
    assert_eq!(param_name(&session.store, lambda, 0), "A");
    assert_eq!(lambda.params[0].variance, Variance::Invariant);
    // The point of the whole design: the result's binder is this very id.
    assert_eq!(
        session.store.types.get(lambda.result),
        &Type::ParamRef {
            binder: id,
            index: 0
        }
    );
}

#[test]
fn parameter_bounds_are_bounds_types() {
    unit!(file, session, unpickler);
    let plain = unpickler.unpickle_type(ID).unwrap();
    let upper = unpickler.unpickle_type(UPPER).unwrap();
    drop(unpickler);

    for id in [plain, upper] {
        let bounds = lambda(&session.store, id).params[0].bounds;
        assert!(
            matches!(
                session.store.types.get(bounds),
                Type::Bounds { .. } | Type::AliasingBounds { .. }
            ),
            "{:?}",
            session.store.types.get(bounds)
        );
    }
}

#[test]
fn the_lambda_is_decoded_once_and_decoding_again_returns_its_id() {
    unit!(file, session, unpickler);
    let first = unpickler.unpickle_type(ID).unwrap();

    assert_eq!(unpickler.unpickle_type(ID), Ok(first));
    assert_eq!(unpickler.index().type_at(ID), Some(first));
}

#[test]
fn a_parameter_type_decoded_first_builds_its_binder_and_is_not_duplicated() {
    unit!(file, session, unpickler);
    // The root is the parameter reference; its binder has not been decoded.
    assert_eq!(unpickler.index().type_at(ID), None);
    let param = unpickler.unpickle_type(ID_PARAM).unwrap();

    // The binder was built on demand, and the reference made while building it
    // is the one returned (a second allocation would be `DuplicateType`).
    let binder = unpickler.index().type_at(ID).expect("binder was decoded");
    assert_eq!(unpickler.index().type_at(ID_PARAM), Some(param));
    assert_eq!(unpickler.unpickle_type(ID_PARAM), Ok(param));
    assert_eq!(unpickler.unpickle_type(ID), Ok(binder));
    drop(unpickler);

    assert_eq!(
        session.store.types.get(param),
        &Type::ParamRef { binder, index: 0 }
    );
    assert_eq!(lambda(&session.store, binder).result, param);
}

#[test]
fn a_nested_result_names_the_outer_binder_by_address() {
    unit!(file, session, unpickler);
    let outer = unpickler.unpickle_type(OUTER).unwrap();
    let inner = unpickler.unpickle_type(OUTER_INNER).unwrap();
    let reference = unpickler.unpickle_type(OUTER_PARAM).unwrap();
    drop(unpickler);

    assert_ne!(outer, inner);
    assert_eq!(lambda(&session.store, outer).result, inner);
    assert_eq!(lambda(&session.store, inner).result, reference);
    // `[A] =>> [B] =>> A`: the innermost result is A, the OUTER parameter.
    assert_eq!(
        session
            .store
            .types
            .get(lambda(&session.store, inner).result),
        &Type::ParamRef {
            binder: outer,
            index: 0
        }
    );
}

#[test]
fn a_nested_result_names_the_inner_binder_when_it_says_b() {
    unit!(file, session, unpickler);
    let outer = unpickler.unpickle_type(INNER_OUTER).unwrap();
    let inner = unpickler.unpickle_type(INNER).unwrap();
    let reference = unpickler.unpickle_type(INNER_PARAM).unwrap();
    drop(unpickler);

    assert_eq!(
        session
            .store
            .types
            .get(lambda(&session.store, inner).result),
        &Type::ParamRef {
            binder: inner,
            index: 0
        }
    );
    assert_eq!(lambda(&session.store, inner).result, reference);
    assert_ne!(inner, outer);
}

#[test]
fn a_parameter_bound_may_name_its_own_binder() {
    unit!(file, session, unpickler);
    let id = unpickler.unpickle_type(F_BOUND).unwrap();
    drop(unpickler);

    let bounds = lambda(&session.store, id).params[0].bounds;
    // `A <: Ord[A]`: the argument of `Ord` inside the bound is the parameter.
    let Type::Bounds { high, .. } = session.store.types.get(bounds) else {
        panic!("expected bounds, got {:?}", session.store.types.get(bounds));
    };
    let Type::Applied { args, .. } = session.store.types.get(*high) else {
        panic!("expected Ord[A]");
    };
    assert_eq!(
        session.store.types.get(args[0]),
        &Type::ParamRef {
            binder: id,
            index: 0
        }
    );
}

#[test]
fn several_parameters_are_indexed_in_order() {
    unit!(file, session, unpickler);
    let id = unpickler.unpickle_type(TWO).unwrap();
    let first = unpickler.unpickle_type(TWO_FIRST).unwrap();
    let second = unpickler.unpickle_type(TWO_SECOND).unwrap();
    drop(unpickler);

    let lambda = lambda(&session.store, id);
    assert_eq!(lambda.params.len(), 2);
    assert_eq!(param_name(&session.store, lambda, 0), "A");
    assert_eq!(param_name(&session.store, lambda, 1), "B");
    assert_eq!(
        session.store.types.get(first),
        &Type::ParamRef {
            binder: id,
            index: 0
        }
    );
    assert_eq!(
        session.store.types.get(second),
        &Type::ParamRef {
            binder: id,
            index: 1
        }
    );
}
