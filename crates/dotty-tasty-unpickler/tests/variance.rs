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
use dotty_core::types::{Type, TypeLambda, Variance};
use dotty_tasty::tasty::TastyFile;
use dotty_tasty_unpickler::tasty_unpickler::TastyUnpickler;

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
