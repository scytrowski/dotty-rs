//! Milestone 5c: the type parameters of a `LAMBDAtpt` (pass 1) and its
//! projection to a `TypeLambda`.
//!
//! The unit is built by hand: a package `p` with a class `Holder` whose
//! members carry lambda type trees in every declared position, then loose
//! trees under an `APPLY` (so they have addresses of their own and can be
//! shared by `SHAREDterm`).
use std::collections::HashMap;

use dotty_core::Definitions;
use dotty_core::Packages;
use dotty_core::ids::SymbolId;
use dotty_core::names::{Name, Namespace};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolInfo, SymbolKind, SymbolOrigin};
use dotty_tasty::tasty::{
    APPLY_TAG, CONTRAVARIANT_TAG, COVARIANT_TAG, DEFDEF_TAG, Header, NameTable, PACKAGE_TAG,
    PARAM_TAG, RawName, SHAREDTERM_TAG, STABLE_TAG, Section, SectionTable, TEMPLATE_TAG,
    TERMREFPKG_TAG, TYPEBOUNDSTPT_TAG, TYPEDEF_TAG, TYPEPARAM_TAG, TYPEREFDIRECT_TAG,
    TYPEREFPKG_TAG, TastyFile, VALDEF_TAG,
};
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

const NAMES: [&str; 30] = [
    "ASTs", "p", "Holder", "v", "f", "a", "X", "Y", "Z", "T", "Alias", "P", "Q", "HK", "U", "FB",
    "A", "Twice", "u1", "u2", "R", "S", "w", "bad", "C", "K", "I", "N", "B1", "Bad",
];

fn n(text: &str) -> u32 {
    u32::try_from(NAMES.iter().position(|name| *name == text).unwrap()).unwrap()
}

const IDENTTPT: u8 = 111;
const APPLIEDTPT: u8 = 162;
const LAMBDATPT: u8 = 171;
// A tag `enter_lambdas_in` never walks into and `type_of_tpt` never builds a
// type for, used below as an inert "this fails at projection, not at
// entering" body. `REFINEDTPT` no longer fits since Milestone 5d2b: pass 1
// now enters a synthetic refinement class for it, and it is a projectable
// type tree.
const MATCHTPT: u8 = 191;

fn nat(value: u32) -> Vec<u8> {
    let mut groups = vec![u8::try_from(value & 0x7f).unwrap() | 0x80];
    let mut rest = value >> 7;
    while rest > 0 {
        groups.push(u8::try_from(rest & 0x7f).unwrap());
        rest >>= 7;
    }
    groups.reverse();
    groups
}

fn node(tag: u8, payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![tag];
    bytes.extend(nat(u32::try_from(payload.len()).unwrap()));
    bytes.extend(payload);
    bytes
}

fn leaf(tag: u8, value: u32) -> Vec<u8> {
    let mut bytes = vec![tag];
    bytes.extend(nat(value));
    bytes
}

fn named(tag: u8, name: u32, child: &[u8]) -> Vec<u8> {
    [&[tag][..], &nat(name), child].concat()
}

fn any_type() -> Vec<u8> {
    leaf(TYPEREFPKG_TAG, n("p"))
}

fn ident_any() -> Vec<u8> {
    named(IDENTTPT, n("p"), &any_type())
}

fn file_with(ast: &[u8]) -> Vec<u8> {
    let names = NameTable::from_entries(
        NAMES
            .iter()
            .map(|text| RawName::Utf8((*text).to_owned()))
            .collect(),
    )
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

/// Every definition of the unit, in document order.
const DEFINITIONS: [&str; 27] = [
    "Holder", "v", "v.X", "f", "f.T", "f.a", "f.a.Y", "f.Z", "Alias", "Alias.P", "Alias.Q", "HK",
    "HK.U", "FB", "FB.A", "Twice", "u1", "u2", "w", "bad", "one.R", "two.S", "var.C", "var.K",
    "var.I", "var.N", "bad.B1",
];

/// The loose trees, in order.
const ROOTS: [&str; 4] = ["lambda one", "lambda two", "lambda variances", "lambda bad"];

struct Unit {
    bytes: Vec<u8>,
    at: HashMap<&'static str, u32>,
}

impl Unit {
    fn at(&self, label: &str) -> u32 {
        self.at[label]
    }

    /// `bad`: `v`'s type tree is a lambda whose parameter is not a type
    /// parameter.
    fn new(bad: bool) -> Self {
        let mut at: HashMap<&'static str, u32> = HashMap::new();
        for _ in 0..8 {
            let (ast, found) = assemble(&at, bad);
            let bytes = file_with(&ast);
            let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
            let mut definitions: Vec<u32> = file
                .ast_address_index()
                .unwrap()
                .iter_nodes()
                .filter(|node| {
                    matches!(
                        node.tag,
                        TYPEDEF_TAG | VALDEF_TAG | DEFDEF_TAG | TYPEPARAM_TAG | PARAM_TAG
                    )
                })
                .map(|node| u32::try_from(node.offset).unwrap())
                .collect();
            definitions.sort_unstable();
            let labels: Vec<&str> = DEFINITIONS
                .iter()
                .copied()
                .filter(|label| !(bad && *label == "v.X"))
                .collect();
            assert_eq!(definitions.len(), labels.len());
            let mut next: HashMap<&'static str, u32> =
                labels.iter().copied().zip(definitions).collect();
            next.extend(found);
            if next == at {
                return Self { bytes, at };
            }
            at = next;
        }
        panic!("the layout did not settle");
    }
}

fn assemble(at: &HashMap<&'static str, u32>, bad: bool) -> (Vec<u8>, HashMap<&'static str, u32>) {
    let addr = |label: &str| at.get(label).copied().unwrap_or(0);
    let bounds = |lo: Vec<u8>, hi: Vec<u8>| node(TYPEBOUNDSTPT_TAG, &[lo, hi].concat());
    let plain_bounds = || bounds(any_type(), any_type());
    let param = |name: &str, bounds: Vec<u8>| node(TYPEPARAM_TAG, &[nat(n(name)), bounds].concat());
    let lambda =
        |params: Vec<Vec<u8>>, body: Vec<u8>| node(LAMBDATPT, &[params.concat(), body].concat());
    let val = |name: &str, tpt: Vec<u8>| node(VALDEF_TAG, &[nat(n(name)), tpt].concat());
    let alias = |name: &str, rhs: Vec<u8>| node(TYPEDEF_TAG, &[nat(n(name)), rhs].concat());

    let v_tpt = if bad {
        lambda(vec![ident_any()], ident_any())
    } else {
        lambda(vec![param("X", plain_bounds())], ident_any())
    };
    let f = node(
        DEFDEF_TAG,
        &[
            nat(n("f")),
            param("T", plain_bounds()),
            node(
                PARAM_TAG,
                &[
                    nat(n("a")),
                    lambda(vec![param("Y", plain_bounds())], ident_any()),
                ]
                .concat(),
            ),
            lambda(vec![param("Z", plain_bounds())], ident_any()),
        ]
        .concat(),
    );
    let a_ref = || named(IDENTTPT, n("A"), &leaf(TYPEREFDIRECT_TAG, addr("FB.A")));
    let members = [
        any_type(),
        val("v", v_tpt),
        f,
        alias(
            "Alias",
            lambda(
                vec![param("P", plain_bounds())],
                lambda(
                    vec![param("Q", plain_bounds())],
                    node(
                        APPLIEDTPT,
                        &[
                            named(IDENTTPT, n("P"), &leaf(TYPEREFDIRECT_TAG, addr("Alias.P"))),
                            named(IDENTTPT, n("Q"), &leaf(TYPEREFDIRECT_TAG, addr("Alias.Q"))),
                        ]
                        .concat(),
                    ),
                ),
            ),
        ),
        alias(
            "HK",
            bounds(
                any_type(),
                lambda(vec![param("U", plain_bounds())], ident_any()),
            ),
        ),
        alias(
            "FB",
            lambda(
                vec![param(
                    "A",
                    bounds(
                        any_type(),
                        node(APPLIEDTPT, &[ident_any(), a_ref()].concat()),
                    ),
                )],
                a_ref(),
            ),
        ),
        alias(
            "Twice",
            node(
                APPLIEDTPT,
                &[
                    ident_any(),
                    leaf(SHAREDTERM_TAG, addr("lambda two")),
                    leaf(SHAREDTERM_TAG, addr("lambda two")),
                ]
                .concat(),
            ),
        ),
        val("u1", leaf(SHAREDTERM_TAG, addr("lambda one"))),
        val("u2", leaf(SHAREDTERM_TAG, addr("lambda one"))),
        val("w", leaf(SHAREDTERM_TAG, addr("lambda variances"))),
        val("bad", leaf(SHAREDTERM_TAG, addr("lambda bad"))),
    ];
    let holder = node(
        TYPEDEF_TAG,
        &[nat(n("Holder")), node(TEMPLATE_TAG, &members.concat())].concat(),
    );
    let package = node(
        PACKAGE_TAG,
        &[leaf(TERMREFPKG_TAG, n("p")), holder].concat(),
    );

    let modified = |name: &str, modifier: Option<u8>| {
        node(
            TYPEPARAM_TAG,
            &[
                nat(n(name)),
                plain_bounds(),
                modifier.map(|tag| vec![tag]).unwrap_or_default(),
            ]
            .concat(),
        )
    };
    let loose: Vec<Vec<u8>> = vec![
        lambda(vec![param("R", plain_bounds())], ident_any()),
        lambda(vec![param("S", plain_bounds())], ident_any()),
        lambda(
            vec![
                modified("C", Some(COVARIANT_TAG)),
                modified("K", Some(CONTRAVARIANT_TAG)),
                modified("I", Some(STABLE_TAG)),
                modified("N", None),
            ],
            ident_any(),
        ),
        lambda(
            vec![param("B1", plain_bounds())],
            node(MATCHTPT, &any_type()),
        ),
    ];
    let payload = loose.concat();
    let header = {
        let mut header = vec![APPLY_TAG];
        header.extend(nat(u32::try_from(payload.len()).unwrap()));
        header
    };
    let mut ast = package;
    let mut next = u32::try_from(ast.len() + header.len()).unwrap();
    ast.extend(header);
    let mut found = HashMap::new();
    for (label, tree) in ROOTS.iter().zip(&loose) {
        found.insert(*label, next);
        next += u32::try_from(tree.len()).unwrap();
    }
    ast.extend(payload);
    (ast, found)
}

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

fn make<'a>(file: &'a TastyFile<'a>, session: &'a mut Session) -> TastyUnpickler<'a, 'a, 'a> {
    let mut packages = Packages::new();
    packages.enter(&mut session.store, SymbolOrigin::Synthetic, &["p"]);
    TastyUnpickler::with_packages(file, &mut session.store, session.definitions, packages)
}

fn entered<'a>(file: &'a TastyFile<'a>, session: &'a mut Session) -> TastyUnpickler<'a, 'a, 'a> {
    let mut unpickler = make(file, session);
    unpickler.enter_symbols().unwrap();
    unpickler
}

fn symbol(unpickler: &TastyUnpickler<'_, '_, '_>, unit: &Unit, label: &str) -> SymbolId {
    unpickler.index().symbol_at(unit.at(label)).unwrap()
}

/// The first child of the definition at `at`: its declared type tree.
fn first_child(file: &TastyFile<'_>, at: u32) -> u32 {
    let index = file.ast_address_index().unwrap();
    let edge = index
        .iter_tree_edges()
        .find(|edge| edge.parent.offset == at as usize)
        .unwrap();
    u32::try_from(edge.child.offset).unwrap()
}

// Entering (pass 1)

#[test]
fn lambda_type_parameters_are_entered_in_every_declared_position() {
    let unit = Unit::new(false);
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let unpickler = entered(&file, &mut session);

    for label in [
        "v.X", "f.a.Y", "f.Z", "Alias.P", "Alias.Q", "HK.U", "FB.A", "one.R", "two.S",
    ] {
        assert!(
            unpickler.symbol_state_at(unit.at(label)).is_some(),
            "{label} was not entered"
        );
    }
    // Every one is a type parameter, entered `Missing`.
    for label in ["v.X", "f.a.Y", "f.Z", "Alias.P", "HK.U", "FB.A", "one.R"] {
        assert_eq!(
            unpickler.symbol_state_at(unit.at(label)),
            Some((SymbolKind::TypeParameter, SymbolInfo::Missing)),
            "{label}"
        );
    }
}

#[test]
fn a_lambda_parameter_is_owned_by_the_enclosing_definition_or_parameter() {
    let unit = Unit::new(false);
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let unpickler = entered(&file, &mut session);
    let owners: Vec<(&str, &str)> = vec![
        ("v.X", "v"),
        ("f.a.Y", "f.a"),
        ("f.Z", "f"),
        ("Alias.P", "Alias"),
        ("HK.U", "HK"),
        ("FB.A", "FB"),
        // Reached first through `u1`; `two` through `Twice`.
        ("one.R", "u1"),
        ("two.S", "Twice"),
    ];
    let expected: Vec<(SymbolId, SymbolId)> = owners
        .iter()
        .map(|(child, owner)| {
            (
                symbol(&unpickler, &unit, child),
                symbol(&unpickler, &unit, owner),
            )
        })
        .collect();
    // A nested lambda's parameter belongs to the definition too, not to the
    // outer lambda parameter.
    let nested = (
        symbol(&unpickler, &unit, "Alias.Q"),
        symbol(&unpickler, &unit, "Alias"),
    );
    drop(unpickler);
    for (child, owner) in expected.into_iter().chain([nested]) {
        assert_eq!(session.store.symbols.get(child).owner, Some(owner));
    }
}

#[test]
fn lambda_parameters_are_not_members_of_any_scope() {
    let unit = Unit::new(false);
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let unpickler = entered(&file, &mut session);
    let holder = symbol(&unpickler, &unit, "Holder");
    let scope = unpickler.index().scope_of(holder).unwrap();
    drop(unpickler);

    for text in ["X", "Y", "Z", "P", "Q", "U", "A", "R", "S"] {
        let name = Name::new(session.store.names.intern(text), Namespace::Type);
        assert!(
            session.store.scopes.get(scope).lookup(&name).is_none(),
            "{text} leaked into the class scope"
        );
    }
    // The class's own members are still there.
    let name = Name::new(session.store.names.intern("Alias"), Namespace::Type);
    assert!(session.store.scopes.get(scope).lookup(&name).is_some());
}

#[test]
fn the_owner_of_a_lambda_is_recorded_by_its_address() {
    let unit = Unit::new(false);
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let unpickler = entered(&file, &mut session);

    let alias_lambda = first_child(&file, unit.at("Alias"));
    assert_eq!(
        unpickler.index().lambda_owner(alias_lambda),
        Some(symbol(&unpickler, &unit, "Alias"))
    );
    assert_eq!(unpickler.index().lambda_owner(unit.at("Alias")), None);
    // Through a link: the loose lambda is owned by its first reacher.
    assert_eq!(
        unpickler.index().lambda_owner(unit.at("lambda one")),
        Some(symbol(&unpickler, &unit, "u1"))
    );
}

#[test]
fn a_lambda_reached_from_a_second_owner_keeps_its_first_owner_and_records_the_conflict() {
    let unit = Unit::new(false);
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let unpickler = entered(&file, &mut session);

    // `u1` and `u2` share `lambda one`; `Twice` reaches `lambda two` twice.
    assert!(
        unpickler
            .index()
            .has_lambda_owner_conflict(unit.at("lambda one"))
    );
    assert!(
        !unpickler
            .index()
            .has_lambda_owner_conflict(unit.at("lambda two"))
    );
    assert!(
        !unpickler
            .index()
            .has_lambda_owner_conflict(first_child(&file, unit.at("Alias")))
    );
    let before = unpickler.index().symbol_count();
    drop(unpickler);

    // One symbol per lambda parameter, however many owners reach the tree.
    let mut fresh = Session::new();
    let control = entered(&file, &mut fresh);
    assert_eq!(control.index().symbol_count(), before);
    // 20 definitions plus the package, and nothing was entered twice.
    assert_eq!(before, DEFINITIONS.len() + 1);
}

#[test]
fn a_malformed_lambda_fails_the_whole_enter_and_leaves_the_store_untouched() {
    let unit = Unit::new(true);
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = make(&file, &mut session);

    let failure = unpickler.enter_symbols().unwrap_err();
    assert!(matches!(
        failure,
        UnpickleError::MalformedType { .. } | UnpickleError::Ast(_)
    ));
    assert_eq!(unpickler.index().symbol_count(), 0);
    assert_eq!(unpickler.index().lambda_owner(unit.at("Holder")), None);
    // Retrying fails the same way and enters nothing.
    assert_eq!(unpickler.enter_symbols().unwrap_err(), failure);
    assert_eq!(unpickler.index().symbol_count(), 0);
    drop(unpickler);
    // The arena holds what the package registration made and nothing else:
    // a clean session that entered nothing agrees on the next id.
    let mut control = Session::new();
    let control_unpickler = make(&file, &mut control);
    drop(control_unpickler);
    assert_eq!(probe(&mut session), probe(&mut control));
}

/// The index the next symbol allocation would get.
fn probe(session: &mut Session) -> u32 {
    use dotty_core::{Symbol, SymbolFlags, SymbolLinks, Visibility};
    let name = Name::new(session.store.names.intern("probe"), Namespace::Type);
    session
        .store
        .symbols
        .alloc(Symbol {
            name,
            owner: None,
            kind: SymbolKind::Class,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        })
        .index()
}

// Projection

use dotty_core::ids::TypeId;
use dotty_core::types::{Type, TypeLambda, Variance};

fn lambda_of(session: &Session, ty: TypeId) -> TypeLambda {
    match session.store.types.get(ty) {
        Type::TypeLambda(lambda) => lambda.clone(),
        other => panic!("not a type lambda: {other:?}"),
    }
}

fn param_ref(session: &Session, ty: TypeId) -> (TypeId, u32) {
    match session.store.types.get(ty) {
        Type::ParamRef { binder, index } => (*binder, *index),
        other => panic!("not a parameter reference: {other:?}"),
    }
}

/// The projection of the declared type tree of the definition `label`.
fn project(
    unpickler: &mut TastyUnpickler<'_, '_, '_>,
    file: &TastyFile<'_>,
    unit: &Unit,
    label: &str,
) -> Result<TypeId, UnpickleError> {
    unpickler.unpickle_type_tree_type(first_child(file, unit.at(label)))
}

#[test]
fn a_lambda_type_tree_is_a_type_lambda_with_completed_bounds() {
    let unit = Unit::new(false);
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let ty = project(&mut unpickler, &file, &unit, "v").unwrap();
    // The parameter was completed on the way.
    let param = unpickler.symbol_state_at(unit.at("v.X")).unwrap();
    assert!(matches!(param.1, SymbolInfo::Complete(_)));
    drop(unpickler);
    let lambda = lambda_of(&session, ty);
    assert_eq!(lambda.params.len(), 1);
    assert_eq!(
        session
            .store
            .names
            .resolve(lambda.params[0].name.as_name().text()),
        "X"
    );
    assert!(matches!(
        session.store.types.get(lambda.params[0].bounds),
        Type::Bounds { .. }
    ));
    assert_eq!(lambda.params[0].declared_variance, None);
}

#[test]
fn an_f_bound_and_a_body_name_the_lambda_that_is_built() {
    // `[A <: p[A]] =>> A`
    let unit = Unit::new(false);
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let ty = project(&mut unpickler, &file, &unit, "FB").unwrap();
    let symbol_info = unpickler.symbol_state_at(unit.at("FB.A")).unwrap().1;
    let a = unpickler.index().symbol_at(unit.at("FB.A")).unwrap();
    drop(unpickler);
    let lambda = lambda_of(&session, ty);
    assert_eq!(param_ref(&session, lambda.result), (ty, 0));
    let Type::Bounds { high, .. } = session.store.types.get(lambda.params[0].bounds).clone() else {
        panic!("not bounds");
    };
    let Type::Applied { args, .. } = session.store.types.get(high).clone() else {
        panic!("not applied");
    };
    assert_eq!(param_ref(&session, args[0]), (ty, 0));

    // The parameter symbol's own info is what was completed before the
    // abstraction: it still names the symbol, and no `ParamRef` leaked in.
    let SymbolInfo::Complete(info) = symbol_info else {
        panic!("not complete");
    };
    let Type::Bounds { high, .. } = session.store.types.get(info).clone() else {
        panic!("not bounds");
    };
    let Type::Applied { args, .. } = session.store.types.get(high).clone() else {
        panic!("not applied");
    };
    assert_eq!(session.store.types.get(args[0]).reference_symbol(), Some(a));
}

#[test]
fn a_nested_lambda_refers_to_both_binders() {
    // `[P] =>> [Q] =>> P[Q]`: the inner body names the outer binder by `P`
    // and its own by `Q`.
    let unit = Unit::new(false);
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let outer = project(&mut unpickler, &file, &unit, "Alias").unwrap();
    drop(unpickler);
    let outer_lambda = lambda_of(&session, outer);
    let inner = outer_lambda.result;
    assert_ne!(inner, outer);
    let inner_lambda = lambda_of(&session, inner);
    let Type::Applied { tycon, args } = session.store.types.get(inner_lambda.result).clone() else {
        panic!("not applied");
    };
    assert_eq!(param_ref(&session, tycon), (outer, 0));
    assert_eq!(param_ref(&session, args[0]), (inner, 0));
}

#[test]
fn declared_variance_is_kept_and_absence_is_not_invariance() {
    let unit = Unit::new(false);
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let ty = project(&mut unpickler, &file, &unit, "w").unwrap();
    drop(unpickler);
    let variances: Vec<Option<Variance>> = lambda_of(&session, ty)
        .params
        .iter()
        .map(|param| param.declared_variance)
        .collect();
    assert_eq!(
        variances,
        vec![
            Some(Variance::Covariant),
            Some(Variance::Contravariant),
            Some(Variance::Invariant),
            None,
        ]
    );
}

#[test]
fn projecting_a_lambda_again_is_the_same_binder_and_written_twice_is_two() {
    let unit = Unit::new(false);
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let first = project(&mut unpickler, &file, &unit, "v").unwrap();
    let types = unpickler.index().type_count();
    let trees = unpickler.index().type_tree_count();
    assert_eq!(project(&mut unpickler, &file, &unit, "v"), Ok(first));
    assert_eq!(unpickler.index().type_count(), types);
    assert_eq!(unpickler.index().type_tree_count(), trees);
    // `f`'s result lambda has the same shape and is another tree: another type.
    let other = project(&mut unpickler, &file, &unit, "f.a").unwrap();
    assert_ne!(other, first);
}

#[test]
fn a_shared_lambda_from_one_owner_is_the_exact_target_type_every_time() {
    // `Twice = p[lambda two, lambda two]`: both arguments are the target.
    let unit = Unit::new(false);
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let twice = project(&mut unpickler, &file, &unit, "Twice").unwrap();
    let target = unpickler
        .unpickle_type_tree_type(unit.at("lambda two"))
        .unwrap();
    assert_eq!(
        unpickler.index().type_tree_type_at(unit.at("lambda two")),
        Some(target)
    );
    drop(unpickler);
    let Type::Applied { args, .. } = session.store.types.get(twice).clone() else {
        panic!("not applied");
    };
    assert_eq!(args, vec![target, target]);
    assert!(matches!(
        session.store.types.get(target),
        Type::TypeLambda(_)
    ));
}

#[test]
fn a_lambda_shared_by_different_owners_is_refused_not_given_a_second_owner() {
    let unit = Unit::new(false);
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let owner = unpickler.index().lambda_owner(unit.at("lambda one"));

    let types = unpickler.index().type_count();
    for label in ["u1", "u2"] {
        assert_eq!(
            project(&mut unpickler, &file, &unit, label),
            Err(UnpickleError::SharedLambdaOwnerConflict {
                address: unit.at("lambda one")
            })
        );
    }
    assert_eq!(unpickler.index().type_count(), types);
    assert_eq!(unpickler.index().lambda_owner(unit.at("lambda one")), owner);
    let r = unpickler.index().symbol_at(unit.at("one.R")).unwrap();
    drop(unpickler);
    // The parameter still belongs to the first owner.
    assert_eq!(session.store.symbols.get(r).owner, owner);
}

#[test]
fn completing_a_type_alias_of_a_lambda_wraps_the_lambda_in_alias_bounds() {
    let unit = Unit::new(false);
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let info = unpickler.complete_symbol(unit.at("Alias")).unwrap();
    // Both parameters were completed by the projection.
    for label in ["Alias.P", "Alias.Q"] {
        assert!(matches!(
            unpickler.symbol_state_at(unit.at(label)).unwrap().1,
            SymbolInfo::Complete(_)
        ));
    }
    drop(unpickler);
    let Type::AliasingBounds { alias } = session.store.types.get(info) else {
        panic!("not an alias");
    };
    assert!(matches!(
        session.store.types.get(*alias),
        Type::TypeLambda(_)
    ));
}

#[test]
fn a_lambda_whose_body_fails_undoes_the_completed_parameters_and_can_be_retried() {
    let unit = Unit::new(false);
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let types = unpickler.index().type_count();
    let result = unpickler.complete_symbol(unit.at("bad"));
    assert!(
        matches!(
            result,
            Err(UnpickleError::UnsupportedTypeTree { tag: MATCHTPT, .. })
        ),
        "{result:?}"
    );
    // `B1` was completed before the body was projected, and is `Missing` again.
    assert_eq!(
        unpickler.symbol_state_at(unit.at("bad.B1")).unwrap().1,
        SymbolInfo::Missing
    );
    assert_eq!(
        unpickler.symbol_state_at(unit.at("bad")).unwrap().1,
        SymbolInfo::Missing
    );
    assert_eq!(unpickler.index().type_count(), types);
    assert_eq!(unpickler.index().type_tree_count(), 0);
    assert_eq!(unpickler.complete_symbol(unit.at("bad")), result);
}
