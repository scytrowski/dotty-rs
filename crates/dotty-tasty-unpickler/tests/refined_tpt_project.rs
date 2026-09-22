//! Milestone 5d2b, projection: `REFINEDtpt` -> `Refined`/`Recursive`.
//!
//! The unit is built by hand, following the same address-settling recipe
//! `tests/refined_tpt_enter.rs` uses: a package `p` with a class `Holder`
//! whose members' declared types are `REFINEDtpt` trees covering a
//! self-referencing member (closes over the synthetic class), a
//! non-recursive member (stays a plain `Refined` chain), and a duplicate
//! member name (rejected as an unsupported overload).

use std::collections::HashMap;

use dotty_core::Definitions;
use dotty_core::Packages;
use dotty_core::ids::TypeId;
use dotty_core::names::Namespace;
use dotty_core::store::SemanticStore;
use dotty_core::symbols::SymbolOrigin;
use dotty_core::types::Type;
use dotty_tasty::tasty::{
    DEFDEF_TAG, Header, NameTable, PACKAGE_TAG, RawName, Section, SectionTable, TEMPLATE_TAG,
    TERMREFPKG_TAG, THIS_TAG, TYPEBOUNDSTPT_TAG, TYPEDEF_TAG, TYPEPARAM_TAG, TYPEREFDIRECT_TAG,
    TastyFile, VALDEF_TAG,
};
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

const NAMES: [&str; 10] = [
    "ASTs", "p", "Holder", "m", "T", "x", "dup", "a", "plain", "y",
];

fn n(text: &str) -> u32 {
    u32::try_from(NAMES.iter().position(|name| *name == text).unwrap()).unwrap()
}

const REFINEDTPT: u8 = 160;

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

/// A category-3 node: tag plus one child tree directly, no length prefix
/// (`THIS`).
fn wrap(tag: u8, child: &[u8]) -> Vec<u8> {
    [&[tag][..], child].concat()
}

fn any_type() -> Vec<u8> {
    leaf(TERMREFPKG_TAG, n("p"))
}

fn plain_bounds() -> Vec<u8> {
    node(TYPEBOUNDSTPT_TAG, &[any_type(), any_type()].concat())
}

fn refined(parent: Vec<u8>, stats: &[Vec<u8>]) -> Vec<u8> {
    node(REFINEDTPT, &[parent, stats.concat()].concat())
}

fn def(tag: u8, name: &str, child: Vec<u8>) -> Vec<u8> {
    node(tag, &[nat(n(name)), child].concat())
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
const DEFINITIONS: [&str; 9] = [
    "Holder",
    "m",
    "m.refined.T",
    "m.refined.x",
    "dup",
    "dup.a1",
    "dup.a2",
    "plain",
    "plain.refined.y",
];

struct Unit {
    bytes: Vec<u8>,
}

impl Unit {
    /// Addresses are recovered from the bytes as needed (`defdef_result_at`),
    /// rather than kept alongside; only `m`'s own `REFINEDtpt` address needs
    /// settling, for its self-reference.
    fn new() -> Self {
        let mut at: HashMap<&'static str, u32> = HashMap::new();
        for _ in 0..8 {
            let (ast, found) = assemble(&at);
            let bytes = file_with(&ast);
            let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
            let mut definitions: Vec<u32> = file
                .ast_address_index()
                .unwrap()
                .iter_nodes()
                .filter(|node| {
                    matches!(
                        node.tag,
                        TYPEDEF_TAG | VALDEF_TAG | DEFDEF_TAG | TYPEPARAM_TAG
                    )
                })
                .map(|node| u32::try_from(node.offset).unwrap())
                .collect();
            definitions.sort_unstable();
            assert_eq!(definitions.len(), DEFINITIONS.len());
            let mut next: HashMap<&'static str, u32> =
                DEFINITIONS.iter().copied().zip(definitions).collect();
            next.extend(found);
            if next == at {
                return Self { bytes };
            }
            at = next;
        }
        panic!("the layout did not settle");
    }
}

/// `p.Holder`:
/// - `def m: p { type T; val x: <THIS of m's own REFINEDtpt> }` (recursive);
/// - `def dup: p { val a: p; val a: p }` (duplicate member name);
/// - `def plain: p { val y: p }` (non-recursive).
fn assemble(at: &HashMap<&'static str, u32>) -> (Vec<u8>, HashMap<&'static str, u32>) {
    let addr = |label: &str| at.get(label).copied().unwrap_or(0);

    // `x`'s declared type is `THIS` of `m`'s own `REFINEDtpt` — an address
    // that exists only once `m` itself is placed, so it is fed back through
    // `found` and settled the same way `DEFINITIONS` addresses are: this
    // iteration builds with the previous guess (0 first time), and reports
    // the address it actually placed things at for the next one.
    let t_stat = def(TYPEDEF_TAG, "T", plain_bounds());
    let this_of_m = wrap(THIS_TAG, &leaf(TYPEREFDIRECT_TAG, addr("m.refined")));
    let x_stat = def(VALDEF_TAG, "x", this_of_m);
    let m_body = refined(any_type(), &[t_stat, x_stat]);
    let m_name = nat(n("m"));
    let m_payload = [m_name.clone(), m_body].concat();
    let m = node(DEFDEF_TAG, &m_payload);
    // Offset of `m_body` (the REFINEDtpt) from the start of `m` (its
    // DEFDEF): one tag byte, the payload's own length prefix, then the
    // name. All addresses in this fixture stay well under 128, so every
    // `nat` above is exactly one byte and this offset is stable across
    // iterations regardless of the placeholder used above.
    let m_refined_offset =
        u32::try_from(1 + nat(u32::try_from(m_payload.len()).unwrap()).len() + m_name.len())
            .unwrap();

    let a1 = def(VALDEF_TAG, "a", any_type());
    let a2 = def(VALDEF_TAG, "a", any_type());
    let dup_body = refined(any_type(), &[a1, a2]);
    let dup = def(DEFDEF_TAG, "dup", dup_body);

    let y_stat = def(VALDEF_TAG, "y", any_type());
    let plain_body = refined(any_type(), &[y_stat]);
    let plain = def(DEFDEF_TAG, "plain", plain_body);

    let holder = node(
        TYPEDEF_TAG,
        &[
            nat(n("Holder")),
            node(TEMPLATE_TAG, &[any_type(), m, dup, plain].concat()),
        ]
        .concat(),
    );
    let package = node(
        PACKAGE_TAG,
        &[leaf(TERMREFPKG_TAG, n("p")), holder].concat(),
    );

    let mut found = HashMap::new();
    // `m` has not settled to its real address yet on the first call, but
    // this converges together with `DEFINITIONS["m"]` in the outer loop.
    found.insert("m.refined", addr("m") + m_refined_offset);
    (package, found)
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

// --- the parent projects first, its exact TypeId is preserved ---

#[test]
fn a_non_recursive_refinement_is_a_plain_refined_chain_not_recursive() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let plain_at = plain_result_at(&file);

    let ty = unpickler.unpickle_type_tree_type(plain_at).unwrap();

    let Type::Refined { parent, name, info } = session.store.types.get(ty) else {
        panic!(
            "expected a plain Refined chain, got {:?}",
            session.store.types.get(ty)
        );
    };
    assert_eq!(session.store.names.resolve(name.text()), "y");
    assert_eq!(name.namespace(), Namespace::Term);
    // The parent is a reference to `p`, not wrapped in anything.
    assert!(matches!(
        session.store.types.get(*parent),
        Type::TermRef { .. }
    ));
    assert!(matches!(
        session.store.types.get(*info),
        Type::TermRef { .. }
    ));
}

#[test]
fn a_self_referencing_member_closes_over_the_synthetic_class_into_one_recursive() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let m_at = defdef_result_at(&file, "m");

    let ty = unpickler.unpickle_type_tree_type(m_at).unwrap();

    let Type::Recursive { parent } = session.store.types.get(ty) else {
        panic!("expected Recursive, got {:?}", session.store.types.get(ty));
    };
    // Refined(x) -> Refined(T) -> parent (p); x's info is the canonical
    // RecThis of the very Recursive this is.
    let Type::Refined {
        name: x_name,
        info: x_info,
        parent: refined_t,
    } = session.store.types.get(*parent)
    else {
        panic!("expected the outer Refined (x)");
    };
    assert_eq!(session.store.names.resolve(x_name.text()), "x");
    assert_eq!(
        session.store.types.get(*x_info),
        &Type::RecThis { binder: ty }
    );
    let Type::Refined { name: t_name, .. } = session.store.types.get(*refined_t) else {
        panic!("expected the inner Refined (T)");
    };
    assert_eq!(session.store.names.resolve(t_name.text()), "T");
}

#[test]
fn projecting_the_same_refined_tree_twice_is_the_same_id() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let m_at = defdef_result_at(&file, "m");

    let first = unpickler.unpickle_type_tree_type(m_at).unwrap();
    let second = unpickler.unpickle_type_tree_type(m_at).unwrap();

    assert_eq!(first, second);
}

#[test]
fn a_duplicate_member_name_is_an_unsupported_overload_not_a_second_binding() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let dup_at = defdef_result_at(&file, "dup");

    let result = unpickler.unpickle_type_tree_type(dup_at);

    assert_eq!(
        result,
        Err(UnpickleError::UnsupportedRefinementOverload {
            address: dup_at,
            name: "a".to_owned(),
        })
    );
}

#[test]
fn a_failed_projection_rolls_back_and_a_retry_matches_a_clean_run() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();

    let mut control = Session::new();
    drop(entered(&file, &mut control));
    let control_types = next_type(&mut control);

    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let dup_at = defdef_result_at(&file, "dup");
    let types = unpickler.index().type_count();
    let trees = unpickler.index().type_tree_count();

    for _ in 0..2 {
        assert!(matches!(
            unpickler.unpickle_type_tree_type(dup_at),
            Err(UnpickleError::UnsupportedRefinementOverload { .. })
        ));
        assert_eq!(unpickler.index().type_count(), types);
        assert_eq!(unpickler.index().type_tree_count(), trees);
    }
    drop(unpickler);
    assert_eq!(next_type(&mut session), control_types);
}

fn next_type(session: &mut Session) -> TypeId {
    session.store.types.alloc(Type::NoType)
}

/// The address of the `REFINEDtpt` result type of the `DEFDEF` named `name`.
fn defdef_result_at(file: &TastyFile<'_>, name: &str) -> u32 {
    let target = n(name);
    let index = file.ast_address_index().unwrap();
    let defdef = index
        .iter()
        .find(|node| {
            node.tag == DEFDEF_TAG
                && node
                    .decode_definition()
                    .is_ok_and(|def| def.name() == target)
        })
        .unwrap();
    let defdef_at = defdef.offset;
    index
        .iter_tree_edges()
        .find(|edge| edge.parent.offset == defdef_at && edge.child.tag == REFINEDTPT)
        .map(|edge| u32::try_from(edge.child.offset).unwrap())
        .unwrap()
}

fn plain_result_at(file: &TastyFile<'_>) -> u32 {
    defdef_result_at(file, "plain")
}
