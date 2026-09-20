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
use dotty_core::types::{MethodKind, MethodType, PolyType, Type, Variance};
use dotty_tasty::tasty::TastyFile;
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

const AND: u8 = 165;
const FLEXIBLE: u8 = 193;
const POLY: u8 = 169;
const PARAM: u8 = 172;
const TYPEBOUNDS: u8 = 163;
const TYPEREFPKG: u8 = 65;
const SHARED: u8 = 61;
const METHOD: u8 = 180;
const IMPLICIT: u8 = 13;
const ERASED: u8 = 34;
const GIVEN: u8 = 37;

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

// Method (synthetic)

/// A `METHODtype` with `result`, one parameter per entry of `params` (the
/// parameter's type, then the name `p` or `q` by position), and `modifiers`.
fn method_node(result: &[u8], params: &[Vec<u8>], modifiers: &[u8]) -> Vec<u8> {
    let mut payload = result.to_vec();
    for (position, ty) in params.iter().enumerate() {
        payload.extend(ty);
        payload.push(nat(if position == 0 { 1 } else { 2 }));
    }
    payload.extend(modifiers);
    length_node(METHOD, &payload)
}

fn method(store: &SemanticStore, id: TypeId) -> &MethodType {
    match store.types.get(id) {
        Type::Method(method) => method,
        other => panic!("not a method type: {other:?}"),
    }
}

fn param_name(store: &SemanticStore, method: &MethodType, index: usize) -> String {
    store
        .names
        .resolve(method.params[index].name.as_name().text())
        .to_string()
}

fn decode_method(bytes: &[u8]) -> (Session, Result<TypeId, UnpickleError>) {
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    let result = unpickler.unpickle_type(POLY_AT);
    drop(unpickler);
    (session, result)
}

#[test]
fn a_method_keeps_its_parameters_in_wire_order_with_their_names_and_types() {
    let params = [package_ref(), param_type(2, 1)];
    let bytes = file_with(&method_node(&package_ref(), &params, &[]));
    // The second parameter's type names the method's own second parameter,
    // which is legal shape-wise (a dependent parameter type).
    let (session, result) = decode_method(&bytes);
    let id = result.unwrap();

    let method = method(&session.store, id);
    assert_eq!(method.kind, MethodKind::Plain);
    assert_eq!(method.params.len(), 2);
    assert_eq!(param_name(&session.store, method, 0), "p");
    assert_eq!(param_name(&session.store, method, 1), "q");
    assert!(matches!(
        session.store.types.get(method.params[0].ty),
        Type::TypeRef { .. }
    ));
    assert_eq!(
        session.store.types.get(method.params[1].ty),
        &Type::ParamRef {
            binder: id,
            index: 1
        }
    );
    assert!(matches!(
        session.store.types.get(method.result),
        Type::TypeRef { .. }
    ));
}

#[test]
fn a_method_without_parameters_is_valid_and_owns_an_id() {
    // `(): R`, unlike a poly or a type lambda, which need a parameter.
    let bytes = file_with(&method_node(&package_ref(), &[], &[]));
    let (session, result) = decode_method(&bytes);
    let id = result.unwrap();

    let method = method(&session.store, id);
    assert!(method.params.is_empty());
    assert_eq!(method.kind, MethodKind::Plain);
}

#[test]
fn the_modifier_tail_selects_the_clause_kind() {
    let cases: [(&[u8], MethodKind); 4] = [
        (&[], MethodKind::Plain),
        (&[IMPLICIT], MethodKind::Implicit),
        (&[GIVEN], MethodKind::Contextual),
        // Dotty reads the tail as a flag set: a repeat is harmless.
        (&[IMPLICIT, IMPLICIT], MethodKind::Implicit),
    ];
    for (modifiers, expected) in cases {
        let bytes = file_with(&method_node(&package_ref(), &[package_ref()], modifiers));
        let (session, result) = decode_method(&bytes);
        let id = result.unwrap_or_else(|error| panic!("{modifiers:?}: {error:?}"));
        assert_eq!(method(&session.store, id).kind, expected, "{modifiers:?}");
    }
}

#[test]
fn implicit_and_given_together_are_malformed() {
    for modifiers in [[IMPLICIT, GIVEN], [GIVEN, IMPLICIT]] {
        let bytes = file_with(&method_node(&package_ref(), &[package_ref()], &modifiers));
        let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
        let mut session = Session::new();
        let mut unpickler = unpickler_for(&file, &mut session);
        let before = unpickler.index().type_count();

        assert!(
            matches!(
                unpickler.unpickle_type(POLY_AT),
                Err(UnpickleError::MalformedType {
                    address: POLY_AT,
                    ..
                })
            ),
            "{modifiers:?}"
        );
        // Refused before reserving: nothing was published.
        assert_eq!(unpickler.index().type_count(), before);
        assert_eq!(unpickler.index().type_at(POLY_AT), None);
    }
}

#[test]
fn a_modifier_a_method_type_does_not_use_is_a_typed_error() {
    // `ERASED` is a valid modifier on the wire, but erasure of a method
    // parameter is an annotation on its type, not a clause modifier.
    for modifiers in [&[ERASED][..], &[IMPLICIT, ERASED]] {
        let bytes = file_with(&method_node(&package_ref(), &[package_ref()], modifiers));
        let (_, result) = decode_method(&bytes);
        assert_eq!(
            result,
            Err(UnpickleError::InvalidMethodModifier {
                address: POLY_AT,
                tag: ERASED
            }),
            "{modifiers:?}"
        );
    }
}

#[test]
fn a_method_parameter_is_neither_erased_nor_varargs() {
    // Neither is on the wire of a `METHODtype`: see the module documentation.
    let params = [package_ref(), package_ref()];
    let bytes = file_with(&method_node(&package_ref(), &params, &[GIVEN]));
    let (session, result) = decode_method(&bytes);

    for param in &method(&session.store, result.unwrap()).params {
        assert!(!param.erased);
        assert!(!param.varargs);
    }
}

#[test]
fn a_method_parameter_index_at_or_past_the_arity_is_an_error() {
    // One parameter: index 1 is past the end. Zero parameters: index 0 is.
    for (params, index, arity) in [(vec![package_ref()], 1, 1), (vec![], 0, 0)] {
        let bytes = file_with(&method_node(&param_type(2, index), &params, &[]));
        let (_, result) = decode_method(&bytes);
        assert!(
            matches!(
                result,
                Err(UnpickleError::InvalidParameterIndex { index: found, arity: found_arity, .. })
                    if found == u32::from(index) && found_arity == arity
            ),
            "{result:?}"
        );
    }
}

#[test]
fn a_shared_link_and_a_repeated_decode_return_the_methods_exact_id() {
    let mut children = method_node(&package_ref(), &[package_ref()], &[]);
    let shared_at = 2 + children.len();
    children.extend([SHARED, nat(2)]);
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
fn a_parameter_type_asked_first_decodes_the_method_on_demand_once() {
    let bytes = file_with(&method_node(&param_type(2, 0), &[package_ref()], &[]));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    // The result is a `PARAMtype` to the method, which is not decoded yet.
    let reference = unpickler.unpickle_type(RESULT_AT).unwrap();
    let binder = unpickler.index().type_at(POLY_AT).unwrap();
    let count = unpickler.index().type_count();
    assert_eq!(unpickler.unpickle_type(POLY_AT), Ok(binder));
    assert_eq!(unpickler.index().type_count(), count);
    drop(unpickler);

    assert_eq!(method(&session.store, binder).result, reference);
    assert_eq!(
        session.store.types.get(reference),
        &Type::ParamRef { binder, index: 0 }
    );
}

#[test]
fn a_parameter_type_may_not_name_a_node_that_is_not_a_binder() {
    // The wrapper's first child is bounds, which bind nothing; the second is a
    // `PARAMtype` naming it.
    let mut children = length_node(TYPEBOUNDS, &package_ref());
    children.extend(param_type(2, 0));
    let bytes = file_with_ast(&length_node(AND, &children));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    let second = 2 + u32::try_from(length_node(TYPEBOUNDS, &package_ref()).len()).unwrap();

    assert!(matches!(
        unpickler.unpickle_type(second),
        Err(UnpickleError::InvalidBinderKind { from, .. }) if from == second
    ));
}

#[test]
fn a_named_member_of_a_parameter_reference_is_an_unsupported_prefix_not_a_panic() {
    // `(p: T): x.Member`, selected by NAME from a `PARAMtype` prefix: the
    // parameter's type is not looked through, so the member cannot be found.
    let mut result = vec![117, nat(1)];
    result.extend(param_type(2, 0));
    let bytes = file_with(&method_node(&result, &[package_ref()], &[]));
    let (_, outcome) = decode_method(&bytes);

    assert!(
        matches!(
            outcome,
            Err(UnpickleError::UnsupportedResolutionPrefix { .. })
        ),
        "{outcome:?}"
    );
}

#[test]
fn a_failure_after_one_parameter_is_decoded_leaves_nothing_behind() {
    // The second parameter's type is an unsupported form (`66`): the method is
    // reserved, published, and one parameter decoded before it fails.
    let bytes = file_with(&method_node(
        &package_ref(),
        &[package_ref(), vec![66, nat(1)]],
        &[],
    ));
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

    assert_eq!(next_id_after(true), next_id_after(false));
}

#[test]
fn a_failed_method_inside_a_poly_fails_the_whole_call_and_forgets_both() {
    // Poly (pending) -> Method (pending) -> an unsupported parameter type.
    let inner = method_node(&package_ref(), &[vec![66, nat(1)]], &[]);
    let bad = poly_node(&inner, 1);
    // A good method follows in the same file, at a known address, naming itself.
    let good_at = 2 + u8::try_from(bad.len()).unwrap();
    let good = method_node(&param_type(good_at, 0), &[package_ref()], &[]);
    let good_at = u32::from(good_at);
    let mut children = bad;
    children.extend(good);
    let bytes = file_with_ast(&length_node(AND, &children));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    let before = unpickler.index().type_count();

    for _ in 0..2 {
        // Twice: a stale pending binder or index entry would change the second.
        let result = unpickler.unpickle_type(POLY_AT);
        assert!(
            matches!(result, Err(UnpickleError::UnsupportedType { tag: 66, .. })),
            "{result:?}"
        );
        assert_eq!(unpickler.index().type_count(), before);
        assert_eq!(unpickler.index().type_at(POLY_AT), None);
    }
    // An independent binder in the same file still decodes, naming itself.
    let id = unpickler.unpickle_type(good_at).unwrap();
    assert_eq!(unpickler.index().type_at(good_at), Some(id));
}

#[test]
fn nested_binders_are_told_apart_by_address_not_by_position() {
    // Poly P at 2 whose result is Method M with a parameter of type `P.0` and
    // result `M.0`: each reference names the binder at ITS address.
    let mut poly_payload = Vec::new();
    // The method sits at address 4, after the poly's two header bytes.
    poly_payload.extend(method_node(&param_type(4, 0), &[param_type(2, 0)], &[]));
    poly_payload.extend(alias_param());
    let bytes = file_with(&length_node(POLY, &poly_payload));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let poly_id = unpickler.unpickle_type(POLY_AT).unwrap();
    let method_id = unpickler.index().type_at(RESULT_AT).unwrap();
    drop(unpickler);

    assert_ne!(poly_id, method_id);
    assert_eq!(poly(&session.store, poly_id).result, method_id);
    let method = method(&session.store, method_id);
    assert_eq!(
        session.store.types.get(method.params[0].ty),
        &Type::ParamRef {
            binder: poly_id,
            index: 0
        }
    );
    assert_eq!(
        session.store.types.get(method.result),
        &Type::ParamRef {
            binder: method_id,
            index: 0
        }
    );
}

// Real Scala 3.9.0 output: `tests/fixtures/semantic/Methodic.scala`.

const METHODIC: &[u8] = include_bytes!("fixtures/semantic/Methodic.tasty");

/// A session that already holds `scala.{Int, Long, Boolean, Any, Nothing}`,
/// which the unit references but does not define.
fn session_with_scala() -> (Session, Packages) {
    use dotty_core::names::{Name, Namespace};
    use dotty_core::symbols::{
        Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, Visibility,
    };
    let mut session = Session::new();
    let mut packages = Packages::new();
    let chain = packages.enter(&mut session.store, SymbolOrigin::Synthetic, &["scala"]);
    let scala = chain.last().unwrap();
    for class in ["Int", "Long", "Boolean", "Any", "Nothing"] {
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

/// The refinement infos of the fixture's structural types.
const PLAIN: u32 = 293;
const EMPTY: u32 = 329;
const CONTEXTUAL: u32 = 369;
const LEGACY: u32 = 414;
const TWO: u32 = 468;
/// `Gen { def id[A](x: A): A }`: a poly (531) over a method (533), whose
/// parameter type is the `PARAMtype` at 535.
const GENERIC: u32 = 531;
const GENERIC_METHOD: u32 = 533;
const GENERIC_PARAM: u32 = 535;
/// `def use[A <: Ord[A]](x: A): A`.
const F_BOUND: u32 = 604;
/// `def get(x: Box): x.Out`: the result's prefix is the `PARAMtype` at 670.
const DEPENDENT: u32 = 666;
const DEPENDENT_PARAM: u32 = 670;
/// `def get(x: Box)(y: x.Out): x.Out`: an outer clause (722) over an inner one
/// (724), whose `x` is the `PARAMtype` at 728.
const CURRIED: u32 = 722;
const CURRIED_INNER: u32 = 724;
const CURRIED_PARAM: u32 = 728;
/// `def f[A](a: A)(b: A): A`: a poly (797) over two clauses (799, 801).
const POLY_CURRIED: u32 = 797;
const POLY_CURRIED_OUTER: u32 = 799;
const POLY_CURRIED_INNER: u32 = 801;
const POLY_CURRIED_PARAM: u32 = 803;

/// A session that also holds the classes the fixture references but does not
/// define, with the unit's symbols entered.
macro_rules! unit {
    ($file:ident, $session:ident, $unpickler:ident) => {
        let $file = TastyFile::parse_scala_3_9(METHODIC).unwrap();
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

/// The simple name of the class a `TypeRef` points at.
fn class_name(store: &SemanticStore, id: TypeId) -> String {
    let Type::TypeRef { symbol, .. } = store.types.get(id) else {
        panic!("not a type reference: {:?}", store.types.get(id));
    };
    store
        .names
        .resolve(store.symbols.get(*symbol).name.text())
        .to_string()
}

fn param_ref(binder: TypeId, index: u32) -> Type {
    Type::ParamRef { binder, index }
}

#[test]
fn a_real_plain_method_has_its_parameter_and_result() {
    unit!(file, session, unpickler);
    let id = unpickler.unpickle_type(PLAIN).unwrap();
    drop(unpickler);

    let method = method(&session.store, id);
    assert_eq!(method.kind, MethodKind::Plain);
    assert_eq!(method.params.len(), 1);
    assert_eq!(param_name(&session.store, method, 0), "x");
    assert_eq!(class_name(&session.store, method.params[0].ty), "Int");
    assert_eq!(class_name(&session.store, method.result), "Boolean");
    assert!(!method.params[0].erased && !method.params[0].varargs);
}

#[test]
fn a_real_empty_method_is_valid() {
    unit!(file, session, unpickler);
    let id = unpickler.unpickle_type(EMPTY).unwrap();
    drop(unpickler);

    let method = method(&session.store, id);
    assert!(method.params.is_empty());
    assert_eq!(class_name(&session.store, method.result), "Boolean");
}

#[test]
fn real_clause_kinds_are_plain_contextual_and_implicit() {
    unit!(file, session, unpickler);
    let plain = unpickler.unpickle_type(PLAIN).unwrap();
    let contextual = unpickler.unpickle_type(CONTEXTUAL).unwrap();
    let legacy = unpickler.unpickle_type(LEGACY).unwrap();
    drop(unpickler);

    // `using` writes GIVEN and `implicit` writes IMPLICIT: two different kinds.
    assert_eq!(method(&session.store, plain).kind, MethodKind::Plain);
    assert_eq!(
        method(&session.store, contextual).kind,
        MethodKind::Contextual
    );
    assert_eq!(method(&session.store, legacy).kind, MethodKind::Implicit);
}

#[test]
fn real_parameters_keep_wire_order_and_names() {
    unit!(file, session, unpickler);
    let id = unpickler.unpickle_type(TWO).unwrap();
    drop(unpickler);

    let method = method(&session.store, id);
    assert_eq!(param_name(&session.store, method, 0), "a");
    assert_eq!(param_name(&session.store, method, 1), "b");
    assert_eq!(class_name(&session.store, method.params[0].ty), "Int");
    assert_eq!(class_name(&session.store, method.params[1].ty), "Boolean");
    assert_eq!(class_name(&session.store, method.result), "Long");
}

#[test]
fn a_real_poly_binds_its_parameter_for_the_method_inside_it() {
    unit!(file, session, unpickler);
    let poly_id = unpickler.unpickle_type(GENERIC).unwrap();
    let method_id = unpickler.index().type_at(GENERIC_METHOD).unwrap();
    drop(unpickler);

    let poly = poly(&session.store, poly_id);
    assert_eq!(poly.params.len(), 1);
    assert_eq!(poly.params[0].variance, Variance::Invariant);
    assert_eq!(
        session
            .store
            .names
            .resolve(poly.params[0].name.as_name().text()),
        "A"
    );
    assert!(matches!(
        session.store.types.get(poly.params[0].bounds),
        Type::Bounds { .. } | Type::AliasingBounds { .. }
    ));
    // Poly -> Method: two binders, two ids, and the method's references are to
    // the poly (the type parameter `A`), not to the method itself.
    assert_ne!(poly_id, method_id);
    assert_eq!(poly.result, method_id);
    let method = method(&session.store, method_id);
    assert_eq!(
        session.store.types.get(method.params[0].ty),
        &param_ref(poly_id, 0)
    );
    assert_eq!(
        session.store.types.get(method.result),
        &param_ref(poly_id, 0)
    );
}

#[test]
fn a_real_f_bound_parameter_names_its_own_poly() {
    unit!(file, session, unpickler);
    let poly_id = unpickler.unpickle_type(F_BOUND).unwrap();
    drop(unpickler);

    // `[A <: Ord[A]]`: the upper bound is `Ord[A]` with `A` a reference to the
    // poly being built.
    let bounds = poly(&session.store, poly_id).params[0].bounds;
    let Type::Bounds { high, .. } = session.store.types.get(bounds) else {
        panic!("not two-sided bounds");
    };
    let Type::Applied { args, .. } = session.store.types.get(*high) else {
        panic!("not an application");
    };
    assert_eq!(session.store.types.get(args[0]), &param_ref(poly_id, 0));
}

#[test]
fn a_dependent_result_names_the_exact_method_binder() {
    unit!(file, session, unpickler);
    let id = unpickler.unpickle_type(DEPENDENT).unwrap();
    drop(unpickler);

    // `(x: Box): x.Out`: the result is `Out` selected from a reference to the
    // method's own first parameter.
    let method = method(&session.store, id);
    assert_eq!(class_name(&session.store, method.params[0].ty), "Box");
    let Type::TypeRef { prefix, symbol } = session.store.types.get(method.result) else {
        panic!("not a type reference");
    };
    assert_eq!(session.store.types.get(*prefix), &param_ref(id, 0));
    assert_eq!(
        session
            .store
            .names
            .resolve(session.store.symbols.get(*symbol).name.text()),
        "Out"
    );
}

#[test]
fn an_inner_clause_can_refer_to_the_outer_clauses_parameter() {
    unit!(file, session, unpickler);
    let outer_id = unpickler.unpickle_type(CURRIED).unwrap();
    let inner_id = unpickler.index().type_at(CURRIED_INNER).unwrap();
    drop(unpickler);

    // `(x: Box)(y: x.Out): x.Out`: the inner method is the outer's result, and
    // the `x` inside it is the OUTER binder, not the inner one being built.
    let outer = method(&session.store, outer_id);
    assert_eq!(outer.result, inner_id);
    let inner = method(&session.store, inner_id);
    assert_ne!(outer_id, inner_id);
    for ty in [inner.params[0].ty, inner.result] {
        let Type::TypeRef { prefix, .. } = session.store.types.get(ty) else {
            panic!("not a type reference");
        };
        assert_eq!(session.store.types.get(*prefix), &param_ref(outer_id, 0));
    }
}

#[test]
fn a_poly_over_two_clauses_keeps_three_binders_apart() {
    unit!(file, session, unpickler);
    let poly_id = unpickler.unpickle_type(POLY_CURRIED).unwrap();
    let outer_id = unpickler.index().type_at(POLY_CURRIED_OUTER).unwrap();
    let inner_id = unpickler.index().type_at(POLY_CURRIED_INNER).unwrap();
    drop(unpickler);

    assert_ne!(poly_id, outer_id);
    assert_ne!(outer_id, inner_id);
    assert_ne!(poly_id, inner_id);
    // Every `A` in `(a: A)(b: A): A` is the poly's parameter, at any depth.
    let outer = method(&session.store, outer_id);
    let inner = method(&session.store, inner_id);
    assert_eq!(outer.result, inner_id);
    for ty in [outer.params[0].ty, inner.params[0].ty, inner.result] {
        assert_eq!(session.store.types.get(ty), &param_ref(poly_id, 0));
    }
}

#[test]
fn real_parameter_types_asked_first_build_the_right_kind_of_binder_once() {
    // Poly-bound `A` inside the method: the poly is decoded on demand.
    unit!(file, session, unpickler);
    let reference = unpickler.unpickle_type(GENERIC_PARAM).unwrap();
    let poly_id = unpickler.index().type_at(GENERIC).unwrap();
    let count = unpickler.index().type_count();
    assert_eq!(unpickler.unpickle_type(GENERIC), Ok(poly_id));
    assert_eq!(unpickler.index().type_count(), count);
    drop(unpickler);
    assert_eq!(session.store.types.get(reference), &param_ref(poly_id, 0));
    assert!(matches!(session.store.types.get(poly_id), Type::Poly(_)));

    // Method-bound `x`: the method is decoded on demand.
    unit!(file, session, unpickler);
    let reference = unpickler.unpickle_type(DEPENDENT_PARAM).unwrap();
    let method_id = unpickler.index().type_at(DEPENDENT).unwrap();
    drop(unpickler);
    assert_eq!(session.store.types.get(reference), &param_ref(method_id, 0));
    assert!(matches!(
        session.store.types.get(method_id),
        Type::Method(_)
    ));

    // The outer clause's `x`, asked from inside the inner one.
    unit!(file, session, unpickler);
    let reference = unpickler.unpickle_type(CURRIED_PARAM).unwrap();
    let outer_id = unpickler.index().type_at(CURRIED).unwrap();
    let inner_id = unpickler.index().type_at(CURRIED_INNER).unwrap();
    drop(unpickler);
    assert_ne!(outer_id, inner_id);
    assert_eq!(session.store.types.get(reference), &param_ref(outer_id, 0));

    // A `PARAMtype` of the innermost poly parameter.
    unit!(file, session, unpickler);
    let reference = unpickler.unpickle_type(POLY_CURRIED_PARAM).unwrap();
    let poly_id = unpickler.index().type_at(POLY_CURRIED).unwrap();
    drop(unpickler);
    assert_eq!(session.store.types.get(reference), &param_ref(poly_id, 0));
}

#[test]
fn equal_looking_real_methods_at_different_addresses_have_distinct_ids() {
    // `f(a: Int): Int` and `g(a: Int): Int` in one refinement: the same shape,
    // but no structural interning, so two binders.
    unit!(file, session, unpickler);
    let first = unpickler.unpickle_type(872).unwrap();
    let second = unpickler.unpickle_type(881).unwrap();
    drop(unpickler);

    assert_ne!(first, second);
    assert_eq!(
        method(&session.store, first).params.len(),
        method(&session.store, second).params.len()
    );
}
