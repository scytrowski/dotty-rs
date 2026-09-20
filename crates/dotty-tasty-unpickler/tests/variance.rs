//! Milestone 3c: variance-bearing `TYPEBOUNDS` and binder rebinding.
//!
//! The real Scala 3.9.0 fixture is `tests/fixtures/semantic/Bounds.scala`;
//! synthetic wire files cover malformed shapes and rollback.
use std::collections::HashSet;

use dotty_core::Definitions;
use dotty_core::Packages;
use dotty_core::ids::TypeId;
use dotty_core::names::{Name, Namespace};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{
    Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
};
use dotty_core::types::{Type, TypeLambda, TypeRebindError, Variance};
use dotty_tasty::tasty::TastyFile;
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

const BOUNDS: &[u8] = include_bytes!("fixtures/semantic/Bounds.tasty");

/// `TYPEBOUNDS` addresses, and the `TYPELAMBDAtype` each one wraps.
///
/// `type F[+A] = List[A]`
const VARIANT: u32 = 525;
const VARIANT_LAMBDA: u32 = 527;
/// `type F[-A] = A => Unit`
const CONTRA: u32 = 694;
const CONTRA_LAMBDA: u32 = 696;
/// `type F[+A, B, -C] = (C => A, B)`
const MIXED: u32 = 800;
const MIXED_LAMBDA: u32 = 802;
/// `type F[+A] <: Iterable[A]`: two-sided, the marker is on the upper bound.
const UPPER: u32 = 898;
const UPPER_LOW: u32 = 900;
const UPPER_LAMBDA: u32 = 903;
/// `type F[+A <: Ord[A]] = List[A]`
const F_BOUNDED: u32 = 979;
const F_BOUNDED_LAMBDA: u32 = 981;
/// `type G[+A] >: List[A] <: List[A]`: both bounds are `SHAREDtype` links to
/// the lambda at 527, and only the upper one carries the marker.
const SHARED_LAMBDA: u32 = 1149;
const SHARED_LOW: u32 = 1151;
const SHARED_HIGH: u32 = 1154;

struct Session {
    store: SemanticStore,
    definitions: Definitions,
}

fn class(store: &mut SemanticStore, owner: &dotty_core::EnteredPackage, name: &str) {
    let name = Name::new(store.names.intern(name), Namespace::Type);
    let symbol = store.symbols.alloc(Symbol {
        name,
        owner: Some(owner.symbol),
        kind: SymbolKind::Class,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Public,
        info: SymbolInfo::Missing,
        origin: SymbolOrigin::Synthetic,
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    });
    store.scopes.get_mut(owner.scope).enter(name, symbol);
}

/// A session that holds the `scala` classes the fixture references but does
/// not define.
fn session() -> (Session, Packages) {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let scala = packages
        .enter(&mut store, SymbolOrigin::Synthetic, &["scala"])
        .pop()
        .unwrap();
    for name in ["Nothing", "Any", "Function1", "Unit", "Tuple2"] {
        class(&mut store, &scala, name);
    }
    let immutable = packages
        .enter(
            &mut store,
            SymbolOrigin::Synthetic,
            &["scala", "collection", "immutable"],
        )
        .pop()
        .unwrap();
    class(&mut store, &immutable, "List");
    let collection = packages
        .enter(
            &mut store,
            SymbolOrigin::Synthetic,
            &["scala", "collection"],
        )
        .pop()
        .unwrap();
    class(&mut store, &collection, "Iterable");
    (Session { store, definitions }, packages)
}

macro_rules! unit {
    ($file:ident, $session:ident, $unpickler:ident) => {
        let $file = TastyFile::parse_scala_3_9(BOUNDS).unwrap();
        let (mut $session, packages) = session();
        let mut $unpickler = TastyUnpickler::with_packages(
            &$file,
            &mut $session.store,
            $session.definitions,
            packages,
        );
        $unpickler.enter_symbols().unwrap();
    };
}

fn lambda(store: &SemanticStore, id: TypeId) -> &TypeLambda {
    match store.types.get(id) {
        Type::TypeLambda(lambda) => lambda,
        other => panic!("expected a type lambda, got {other:?}"),
    }
}

fn param_name(store: &SemanticStore, id: TypeId, index: usize) -> String {
    store
        .names
        .resolve(lambda(store, id).params[index].name.as_name().text())
        .to_string()
}

fn declared(store: &SemanticStore, id: TypeId) -> Vec<Option<Variance>> {
    lambda(store, id)
        .params
        .iter()
        .map(|param| param.declared_variance)
        .collect()
}

/// Every binder named by a `ParamRef` reachable from `root`, following the
/// types this fixture builds. The binder of a reference is never followed.
fn binders_named_from(store: &SemanticStore, root: TypeId) -> HashSet<TypeId> {
    let mut named = HashSet::new();
    let mut seen = HashSet::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        match store.types.get(id) {
            Type::ParamRef { binder, .. } => {
                named.insert(*binder);
            }
            Type::TermRef { prefix, .. } | Type::TypeRef { prefix, .. } => stack.push(*prefix),
            Type::Applied { tycon, args } => {
                stack.push(*tycon);
                stack.extend(args);
            }
            Type::Bounds { low, high } => stack.extend([*low, *high]),
            Type::AliasingBounds { alias } => stack.push(*alias),
            Type::And { left, right } | Type::Or { left, right } => stack.extend([*left, *right]),
            Type::TypeLambda(lambda) => {
                stack.push(lambda.result);
                stack.extend(lambda.params.iter().map(|param| param.bounds));
            }
            _ => {}
        }
    }
    named
}

/// The one binder a `ParamRef` inside `root` names, asserting there is one.
fn only_binder(store: &SemanticStore, root: TypeId) -> TypeId {
    let named = binders_named_from(store, root);
    assert_eq!(named.len(), 1, "{named:?}");
    *named.iter().next().unwrap()
}

#[test]
fn an_alias_of_a_variant_lambda_is_a_derived_lambda_that_names_itself() {
    unit!(file, session, unpickler);
    let bounds = unpickler.unpickle_type(VARIANT).unwrap();
    let original = unpickler.index().type_at(VARIANT_LAMBDA).unwrap();
    drop(unpickler);

    let Type::AliasingBounds { alias: derived } = *session.store.types.get(bounds) else {
        panic!("not alias bounds");
    };
    // The lambda at its own address is the plain one; the bounds hold a new one.
    assert_ne!(derived, original);
    assert_eq!(declared(&session.store, original), [None]);
    assert_eq!(
        declared(&session.store, derived),
        [Some(Variance::Covariant)]
    );
    assert_eq!(param_name(&session.store, derived, 0), "A");
    // Each graph names its own binder, and only its own.
    assert_eq!(
        only_binder(&session.store, lambda(&session.store, derived).result),
        derived
    );
    assert_eq!(
        only_binder(&session.store, lambda(&session.store, original).result),
        original
    );
}

#[test]
fn a_contravariant_marker_is_declared_contravariant() {
    unit!(file, session, unpickler);
    let bounds = unpickler.unpickle_type(CONTRA).unwrap();
    let original = unpickler.index().type_at(CONTRA_LAMBDA).unwrap();
    drop(unpickler);

    let Type::AliasingBounds { alias } = *session.store.types.get(bounds) else {
        panic!("not alias bounds");
    };
    assert_eq!(
        declared(&session.store, alias),
        [Some(Variance::Contravariant)]
    );
    assert_eq!(declared(&session.store, original), [None]);
}

#[test]
fn several_markers_keep_their_order_and_stable_is_a_declared_invariant() {
    unit!(file, session, unpickler);
    let bounds = unpickler.unpickle_type(MIXED).unwrap();
    let original = unpickler.index().type_at(MIXED_LAMBDA).unwrap();
    drop(unpickler);

    let Type::AliasingBounds { alias } = *session.store.types.get(bounds) else {
        panic!("not alias bounds");
    };
    // `[+A, B, -C]`: `B` has the wire marker STABLE, which is an explicit
    // declaration of invariance, not the absence of one.
    assert_eq!(
        declared(&session.store, alias),
        [
            Some(Variance::Covariant),
            Some(Variance::Invariant),
            Some(Variance::Contravariant)
        ]
    );
    for (index, name) in ["A", "B", "C"].into_iter().enumerate() {
        assert_eq!(param_name(&session.store, alias, index), name);
    }
    assert_eq!(declared(&session.store, original), [None, None, None]);
    assert_eq!(
        only_binder(&session.store, lambda(&session.store, alias).result),
        alias
    );
}

#[test]
fn in_two_sided_bounds_the_markers_go_to_the_upper_bound_only() {
    unit!(file, session, unpickler);
    let bounds = unpickler.unpickle_type(UPPER).unwrap();
    let low = unpickler.unpickle_type(UPPER_LOW).unwrap();
    let original = unpickler.index().type_at(UPPER_LAMBDA).unwrap();
    drop(unpickler);

    let Type::Bounds {
        low: found_low,
        high,
    } = *session.store.types.get(bounds)
    else {
        panic!("not two-sided bounds");
    };
    // `low` is the plain decoded child; `high` is the rebound lambda.
    assert_eq!(found_low, low);
    assert_ne!(high, original);
    assert_eq!(declared(&session.store, high), [Some(Variance::Covariant)]);
    assert_eq!(declared(&session.store, original), [None]);
    assert_eq!(
        only_binder(&session.store, lambda(&session.store, high).result),
        high
    );
}

#[test]
fn an_f_bound_of_the_derived_lambda_names_the_derived_binder() {
    unit!(file, session, unpickler);
    let bounds = unpickler.unpickle_type(F_BOUNDED).unwrap();
    let original = unpickler.index().type_at(F_BOUNDED_LAMBDA).unwrap();
    drop(unpickler);

    let Type::AliasingBounds { alias: derived } = *session.store.types.get(bounds) else {
        panic!("not alias bounds");
    };
    // The parameter's own bound (`A <: Ord[A]`) and the result both hold a
    // `ParamRef`; between them nothing names the old lambda.
    let mut named = HashSet::new();
    named.extend(binders_named_from(
        &session.store,
        lambda(&session.store, derived).result,
    ));
    named.extend(binders_named_from(
        &session.store,
        lambda(&session.store, derived).params[0].bounds,
    ));
    assert_eq!(named, HashSet::from([derived]));
    // And the original graph is as it was.
    let mut old_named = binders_named_from(&session.store, lambda(&session.store, original).result);
    old_named.extend(binders_named_from(
        &session.store,
        lambda(&session.store, original).params[0].bounds,
    ));
    assert_eq!(old_named, HashSet::from([original]));
}

#[test]
fn a_lambda_shared_between_both_bounds_is_rebound_only_as_the_upper_bound() {
    unit!(file, session, unpickler);
    let bounds = unpickler.unpickle_type(SHARED_LAMBDA).unwrap();
    let original = unpickler.index().type_at(VARIANT_LAMBDA).unwrap();
    drop(unpickler);

    let Type::Bounds { low, high } = *session.store.types.get(bounds) else {
        panic!("not two-sided bounds");
    };
    // The lower bound is the link's target, untouched; the upper is derived.
    assert_eq!(low, original);
    assert_ne!(high, original);
    assert_eq!(declared(&session.store, low), [None]);
    assert_eq!(declared(&session.store, high), [Some(Variance::Covariant)]);
}

// Identity, order, sharing

#[test]
fn the_child_and_the_bounds_decode_in_either_order_to_the_same_relationship() {
    // Child first.
    unit!(file, session, unpickler);
    let child = unpickler.unpickle_type(VARIANT_LAMBDA).unwrap();
    let bounds = unpickler.unpickle_type(VARIANT).unwrap();
    assert_eq!(unpickler.index().type_at(VARIANT_LAMBDA), Some(child));
    drop(unpickler);
    let Type::AliasingBounds { alias: derived } = *session.store.types.get(bounds) else {
        panic!("not alias bounds");
    };
    assert_ne!(derived, child);
    assert_eq!(declared(&session.store, child), [None]);
    assert_eq!(
        declared(&session.store, derived),
        [Some(Variance::Covariant)]
    );

    // Bounds first: decoding them decodes and caches the child.
    unit!(file, session, unpickler);
    assert_eq!(unpickler.index().type_at(VARIANT_LAMBDA), None);
    let bounds = unpickler.unpickle_type(VARIANT).unwrap();
    let child = unpickler.index().type_at(VARIANT_LAMBDA).unwrap();
    assert_eq!(unpickler.unpickle_type(VARIANT_LAMBDA), Ok(child));
    drop(unpickler);
    let Type::AliasingBounds { alias: derived } = *session.store.types.get(bounds) else {
        panic!("not alias bounds");
    };
    assert_ne!(derived, child);
    assert_eq!(declared(&session.store, child), [None]);
    assert_eq!(
        declared(&session.store, derived),
        [Some(Variance::Covariant)]
    );
}

#[test]
fn decoding_the_bounds_again_returns_them_and_makes_no_second_derived_lambda() {
    let next_id_after = |repeats: usize| {
        unit!(file, session, unpickler);
        let bounds = unpickler.unpickle_type(VARIANT).unwrap();
        for _ in 0..repeats {
            assert_eq!(unpickler.unpickle_type(VARIANT), Ok(bounds));
        }
        drop(unpickler);
        session.store.types.alloc(Type::NoType)
    };

    // A repeat allocates nothing: the next id is the same as after one decode.
    assert_eq!(next_id_after(0), next_id_after(3));
}

#[test]
fn a_shared_link_to_the_lambda_still_returns_the_original_not_the_derived_one() {
    unit!(file, session, unpickler);
    let bounds = unpickler.unpickle_type(SHARED_LAMBDA).unwrap();
    let original = unpickler.index().type_at(VARIANT_LAMBDA).unwrap();

    // Both bounds children are `SHAREDtype` links to the lambda at 527.
    assert_eq!(unpickler.unpickle_type(SHARED_LOW), Ok(original));
    assert_eq!(unpickler.unpickle_type(SHARED_HIGH), Ok(original));
    drop(unpickler);

    let Type::Bounds { high, .. } = *session.store.types.get(bounds) else {
        panic!("not two-sided bounds");
    };
    assert_ne!(high, original);
    assert_eq!(declared(&session.store, original), [None]);
}

#[test]
fn a_link_decoded_before_the_bounds_gives_the_same_original() {
    unit!(file, session, unpickler);
    let linked = unpickler.unpickle_type(SHARED_HIGH).unwrap();
    let bounds = unpickler.unpickle_type(SHARED_LAMBDA).unwrap();
    drop(unpickler);

    let Type::Bounds { low, high } = *session.store.types.get(bounds) else {
        panic!("not two-sided bounds");
    };
    assert_eq!(low, linked);
    assert_ne!(high, linked);
}

// Malformed markers and rollback (synthetic wire files)

const FLEXIBLE: u8 = 193;
const AND: u8 = 165;
const POLY: u8 = 169;
const LAMBDA: u8 = 170;
const PARAM: u8 = 172;
const TYPEBOUNDS: u8 = 163;
const TYPEREFPKG: u8 = 65;
const COVARIANT: u8 = 28;
const CONTRAVARIANT: u8 = 29;

fn nat(value: u8) -> u8 {
    assert!(value < 128);
    0x80 | value
}

fn length_node(tag: u8, payload: &[u8]) -> Vec<u8> {
    let mut node = vec![tag, nat(u8::try_from(payload.len()).unwrap())];
    node.extend(payload);
    node
}

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

/// `TYPEREFpkg p`.
fn package_ref() -> Vec<u8> {
    vec![TYPEREFPKG, nat(1)]
}

fn param_type(binder: u8, number: u8) -> Vec<u8> {
    length_node(PARAM, &[nat(binder), nat(number)])
}

/// A `TYPELAMBDAtype` or `POLYtype` (`tag`) with `arity` parameters, each with
/// alias bounds and the name `p`.
fn binder_node(tag: u8, result: &[u8], arity: usize) -> Vec<u8> {
    let mut payload = result.to_vec();
    for _ in 0..arity {
        payload.extend(length_node(TYPEBOUNDS, &package_ref()));
        payload.push(nat(1));
    }
    length_node(tag, &payload)
}

fn bounds_node(children: &[&[u8]], markers: &[u8]) -> Vec<u8> {
    let mut payload = Vec::new();
    for child in children {
        payload.extend(*child);
    }
    payload.extend(markers);
    length_node(TYPEBOUNDS, &payload)
}

/// A file whose ASTs are a `FLEXIBLEtype` wrapper around `child`, at address 2.
fn file_with(child: &[u8]) -> Vec<u8> {
    file_with_ast(&length_node(FLEXIBLE, child))
}

fn synthetic(bytes: &[u8]) -> (TastyFile<'_>, Session, Packages) {
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    packages.enter(&mut store, SymbolOrigin::Synthetic, &["p"]);
    (file, Session { store, definitions }, packages)
}

const AT: u32 = 2;
const CHILD: u32 = 4;

#[test]
fn markers_on_a_bound_that_is_not_a_lambda_leave_it_alone_as_in_dotty() {
    // `readVariances` matches `HKTypeLambda` and otherwise returns the type
    // as it is (`case _ => tp`), still consuming the markers.
    for (label, child) in [
        ("a package reference", package_ref()),
        ("a poly", binder_node(POLY, &package_ref(), 1)),
    ] {
        let bytes = file_with(&bounds_node(&[&child], &[COVARIANT]));
        let (file, mut session, packages) = synthetic(&bytes);
        let mut unpickler =
            TastyUnpickler::with_packages(&file, &mut session.store, session.definitions, packages);

        let bounds = unpickler
            .unpickle_type(AT)
            .unwrap_or_else(|e| panic!("{label}: {e:?}"));
        let target = unpickler.index().type_at(CHILD).unwrap();
        drop(unpickler);

        // The alias is the decoded child itself: not rebound, not copied.
        assert_eq!(
            session.store.types.get(bounds),
            &Type::AliasingBounds { alias: target },
            "{label}"
        );
    }
}

#[test]
fn a_marker_count_that_differs_from_the_arity_is_a_typed_error() {
    let cases: [(usize, &[u8]); 3] = [
        (1, &[COVARIANT, CONTRAVARIANT]),
        (2, &[COVARIANT]),
        (3, &[COVARIANT, COVARIANT]),
    ];
    for (arity, markers) in cases {
        let lambda = binder_node(LAMBDA, &package_ref(), arity);
        let bytes = file_with(&bounds_node(&[&lambda], markers));
        let (file, mut session, packages) = synthetic(&bytes);
        let mut unpickler =
            TastyUnpickler::with_packages(&file, &mut session.store, session.definitions, packages);
        let before = unpickler.index().type_count();

        let result = unpickler.unpickle_type(AT);
        assert!(
            matches!(
                result,
                Err(UnpickleError::BoundsVarianceArityMismatch { address: AT, expected, actual, .. })
                    if expected == arity && actual == markers.len()
            ),
            "{arity} parameters, {} markers: {result:?}",
            markers.len()
        );
        assert_eq!(unpickler.index().type_count(), before);
    }
}

#[test]
fn a_two_sided_marker_never_applies_to_the_lower_bound() {
    // The lower bound is a perfectly good lambda, the upper one is not: the
    // marker goes to the upper bound, which is left alone, so the lower
    // lambda keeps no declared variance.
    let low = binder_node(LAMBDA, &package_ref(), 1);
    let bytes = file_with(&bounds_node(&[&low, &package_ref()], &[COVARIANT]));
    let (file, mut session, packages) = synthetic(&bytes);
    let mut unpickler =
        TastyUnpickler::with_packages(&file, &mut session.store, session.definitions, packages);

    let bounds = unpickler.unpickle_type(AT).unwrap();
    let low = unpickler.index().type_at(CHILD).unwrap();
    drop(unpickler);

    let Type::Bounds { low: found_low, .. } = *session.store.types.get(bounds) else {
        panic!("not two-sided bounds");
    };
    assert_eq!(found_low, low);
    assert_eq!(declared(&session.store, low), [None]);
}

#[test]
fn a_lambda_still_being_decoded_cannot_take_markers_and_is_a_typed_error() {
    // `[p = <bounds over a link to this lambda> with +] =>> p`: the marker's
    // target is the lambda at 2, whose slot is reserved and not filled yet.
    let mut payload = param_type(2, 0);
    let link = [61, nat(2)];
    payload.extend(bounds_node(&[&link], &[COVARIANT]));
    payload.push(nat(1));
    let bytes = file_with(&length_node(LAMBDA, &payload));
    let (file, mut session, packages) = synthetic(&bytes);
    let mut unpickler =
        TastyUnpickler::with_packages(&file, &mut session.store, session.definitions, packages);
    let before = unpickler.index().type_count();

    let result = unpickler.unpickle_type(AT);
    assert!(
        matches!(
            result,
            Err(UnpickleError::BoundsVarianceTargetPending { .. })
        ),
        "{result:?}"
    );
    assert_eq!(unpickler.index().type_count(), before);
    assert_eq!(unpickler.index().type_at(AT), None);
}

#[test]
fn a_synthetic_lambda_with_a_self_reference_is_rebound_to_itself() {
    // `[p] =>> p` with `+`: the result is a `PARAMtype` to the lambda at 4.
    let lambda = binder_node(LAMBDA, &param_type(4, 0), 1);
    let bytes = file_with(&bounds_node(&[&lambda], &[COVARIANT]));
    let (file, mut session, packages) = synthetic(&bytes);
    let mut unpickler =
        TastyUnpickler::with_packages(&file, &mut session.store, session.definitions, packages);

    let bounds = unpickler.unpickle_type(AT).unwrap();
    let original = unpickler.index().type_at(CHILD).unwrap();
    drop(unpickler);

    let Type::AliasingBounds { alias } = *session.store.types.get(bounds) else {
        panic!("not alias bounds");
    };
    assert_eq!(
        session
            .store
            .types
            .get(lambda_result(&session.store, alias)),
        &Type::ParamRef {
            binder: alias,
            index: 0
        }
    );
    assert_eq!(
        session
            .store
            .types
            .get(lambda_result(&session.store, original)),
        &Type::ParamRef {
            binder: original,
            index: 0
        }
    );
}

fn lambda_result(store: &SemanticStore, id: TypeId) -> TypeId {
    lambda(store, id).result
}

#[test]
fn a_failure_after_the_rebinding_rolls_back_the_derived_lambda_but_not_the_older_child() {
    // `AND(bounds-with-marker, <unsupported form>)`: the left operand decodes
    // and its lambda is rebound, then the right one fails.
    let lambda = binder_node(LAMBDA, &param_type(4, 0), 1);
    // AND is the wrapper at 0: its first child, the bounds, is at 2; the
    // lambda inside them at 4.
    let mut children = bounds_node(&[&lambda], &[COVARIANT]);
    children.extend([75, nat(1)]);
    let bytes = file_with_ast(&length_node(AND, &children));
    let (file, mut session, packages) = synthetic(&bytes);
    let mut unpickler =
        TastyUnpickler::with_packages(&file, &mut session.store, session.definitions, packages);

    // The lambda is decoded on its own first, so it exists before the failure.
    let older = unpickler.unpickle_type(CHILD).unwrap();
    let before = unpickler.index().type_count();

    for _ in 0..2 {
        let result = unpickler.unpickle_type(0);
        assert!(
            matches!(result, Err(UnpickleError::UnsupportedType { tag: 75, .. })),
            "{result:?}"
        );
        // Nothing of the failed call survives: no bounds, no derived lambda,
        // no copied node, and no stale pending state to trip the retry.
        assert_eq!(unpickler.index().type_count(), before);
        assert_eq!(unpickler.index().type_at(AT), None);
        // What existed before the call is intact.
        assert_eq!(unpickler.index().type_at(CHILD), Some(older));
    }
    drop(unpickler);
    assert_eq!(declared(&session.store, older), [None]);
}

#[test]
fn a_failed_rebinding_call_gives_back_every_id_it_took() {
    let lambda = binder_node(LAMBDA, &param_type(4, 0), 1);
    let mut children = bounds_node(&[&lambda], &[COVARIANT]);
    children.extend([75, nat(1)]);
    let bytes = file_with_ast(&length_node(AND, &children));
    let next_id_after = |fail: bool| {
        let (file, mut session, packages) = synthetic(&bytes);
        let mut unpickler =
            TastyUnpickler::with_packages(&file, &mut session.store, session.definitions, packages);
        unpickler.unpickle_type(CHILD).unwrap();
        if fail {
            assert!(unpickler.unpickle_type(0).is_err());
        }
        drop(unpickler);
        session.store.types.alloc(Type::NoType)
    };

    assert_eq!(next_id_after(true), next_id_after(false));
}

#[test]
fn a_lambda_that_reaches_a_binder_still_being_built_is_a_typed_error_not_a_panic() {
    // A poly (pending) whose parameter info is `= [p] =>> <link to the poly>`
    // with `+`: the lambda's result is a `SHAREDtype` to the poly at 2, whose
    // slot is reserved and not filled while the bounds are decoded. Rebinding
    // would have to read it.
    let lambda = binder_node(LAMBDA, &[61, nat(2)], 1);
    let mut payload = package_ref();
    payload.extend(bounds_node(&[&lambda], &[COVARIANT]));
    payload.push(nat(1));
    let bytes = file_with(&length_node(POLY, &payload));
    let (file, mut session, packages) = synthetic(&bytes);
    let mut unpickler =
        TastyUnpickler::with_packages(&file, &mut session.store, session.definitions, packages);
    let before = unpickler.index().type_count();

    let result = unpickler.unpickle_type(AT);
    assert!(
        matches!(
            result,
            Err(UnpickleError::RebindFailed {
                error: TypeRebindError::UnfilledType { .. },
                ..
            })
        ),
        "{result:?}"
    );
    assert_eq!(unpickler.index().type_count(), before);
    assert_eq!(unpickler.index().type_at(AT), None);
}
