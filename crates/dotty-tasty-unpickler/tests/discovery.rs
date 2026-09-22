//! Milestone 5d2c: pass-1 identity discovery reaches a `LAMBDAtpt`/
//! `REFINEDtpt` through the same structural routes semantic projection can,
//! not only a direct `TypeTree` child.
//!
//! The unit is built by hand, following the same address-settling recipe
//! `tests/refined_tpt_enter.rs` and `tests/refined_tpt_project.rs` use: a
//! package `p` with a class `Holder` whose members' declared types reach
//! several `REFINEDtpt` trees through hidden routes (a `SELECTtpt` qualifier,
//! a `SHAREDtype` link, a `THIS` class reference), plus one genuinely
//! self-referencing member and one poisoned, deliberately unreachable tree.

use std::collections::HashMap;

use dotty_core::Definitions;
use dotty_core::Packages;
use dotty_core::store::SemanticStore;
use dotty_core::symbols::SymbolOrigin;
use dotty_tasty::tasty::{
    APPLIEDTPT_TAG, APPLY_TAG, DEFDEF_TAG, Header, IDENT_TAG, NameTable, PACKAGE_TAG, RawName,
    SELECTTPT_TAG, SHAREDTERM_TAG, SHAREDTYPE_TAG, Section, SectionTable, TEMPLATE_TAG,
    TERMREFPKG_TAG, THIS_TAG, TYPEDEF_TAG, TYPEREF_TAG, TYPEREFDIRECT_TAG, TastyFile, VALDEF_TAG,
};
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

const NAMES: [&str; 13] = [
    "ASTs", "p", "Holder", "m1", "m2", "m3", "dup1", "dup2", "poison", "modesens", "selfref", "x",
    "sel",
];

fn n(text: &str) -> u32 {
    u32::try_from(NAMES.iter().position(|name| *name == text).unwrap()).unwrap()
}

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

/// A tag, a `Nat` and one child tree inline, no length prefix (`IDENT`,
/// `TYPEREF`).
fn named(tag: u8, name: u32, child: &[u8]) -> Vec<u8> {
    [&[tag][..], &nat(name), child].concat()
}

/// A tag and one child tree directly, no length prefix (`THIS`).
fn wrap(tag: u8, child: &[u8]) -> Vec<u8> {
    [&[tag][..], child].concat()
}

fn any_type() -> Vec<u8> {
    leaf(TERMREFPKG_TAG, n("p"))
}

fn def(tag: u8, name: &str, child: Vec<u8>) -> Vec<u8> {
    node(tag, &[nat(n(name)), child].concat())
}

fn refined(parent: Vec<u8>, stats: &[Vec<u8>]) -> Vec<u8> {
    const REFINEDTPT: u8 = 160;
    node(REFINEDTPT, &[parent, stats.concat()].concat())
}

/// `SELECTtpt sel qualifier`, the qualifier a `SHAREDterm` link to `target`.
/// `type_of_term`'s `SHAREDterm` handling resolves the link and, since the
/// tree it names is itself a `TypeTree` tag (`is_type_tree_tag`), hands off
/// to `type_of_tpt` under the qualifier's own address — exactly the real
/// library shape: a `REFINEDtpt` reached *only* through a `SELECTtpt`
/// qualifier's own type resolution (§5/§20 of issue #101), which the old
/// direct-child-only scanner never visited at all (`SELECTtpt` was not in
/// its whitelist).
fn select_via_shared_term(target: u32) -> Vec<u8> {
    named(SELECTTPT_TAG, n("sel"), &leaf(SHAREDTERM_TAG, target))
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
    "Holder", "m1", "m2", "m3", "dup1", "dup2", "poison", "modesens", "selfref",
];

struct Unit {
    bytes: Vec<u8>,
    at: HashMap<&'static str, u32>,
}

impl Unit {
    fn at(&self, label: &str) -> u32 {
        self.at[label]
    }

    fn new() -> Self {
        let mut at: HashMap<&'static str, u32> = HashMap::new();
        for _ in 0..8 {
            let (ast, found) = assemble(&at);
            let bytes = file_with(&ast);
            let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
            let mut definitions: Vec<u32> = file
                .ast_address_index()
                .unwrap()
                .iter()
                .filter(|node| matches!(node.tag, TYPEDEF_TAG | DEFDEF_TAG))
                .map(|node| u32::try_from(node.offset).unwrap())
                .collect();
            definitions.sort_unstable();
            assert_eq!(definitions.len(), DEFINITIONS.len());
            let mut next: HashMap<&'static str, u32> =
                DEFINITIONS.iter().copied().zip(definitions).collect();
            next.extend(found);
            if next == at {
                return Self { bytes, at };
            }
            at = next;
        }
        panic!("the layout did not settle");
    }
}

/// Builds the unit's AST and reports every loose tree's actual address, fed
/// back into the next attempt until addresses settle (only `selfref`'s own
/// member depends on its own placement; every other loose tree's address is
/// stable from the first attempt, since every `nat` involved stays a single
/// byte).
fn assemble(at: &HashMap<&'static str, u32>) -> (Vec<u8>, HashMap<&'static str, u32>) {
    let addr = |label: &str| at.get(label).copied().unwrap_or(0);

    // `m1: p.SomeMember` reached through `SELECTtpt`'s qualifier, a
    // `SHAREDterm` link to the loose `hidden_a` tree: `SELECTtpt` was not in
    // the old direct-child-only scanner's whitelist at all.
    let m1 = def(DEFDEF_TAG, "m1", select_via_shared_term(addr("hidden_a")));

    // `m2: this.type` where the class argument of `THIS` is a direct
    // reference to the loose `hidden_b` tree, and `m3` where it is instead a
    // `SHAREDtype` link to `hidden_e`: the critical case (§10), `ClassRef`
    // mode entering a `REFINEDtpt` reached via `THIS`, with and without a
    // `SHAREDtype` hop.
    let m2 = def(
        DEFDEF_TAG,
        "m2",
        wrap(THIS_TAG, &leaf(TYPEREFDIRECT_TAG, addr("hidden_b"))),
    );
    let m3 = def(
        DEFDEF_TAG,
        "m3",
        wrap(THIS_TAG, &leaf(SHAREDTYPE_TAG, addr("hidden_e"))),
    );

    // `dup1`/`dup2` both reach the *same* hidden tree through a `SELECTtpt`
    // qualifier: broader discovery reveals this conflict, which the old
    // direct-child-only scanner could never have found (§17/§24).
    let dup1 = def(
        DEFDEF_TAG,
        "dup1",
        select_via_shared_term(addr("shared_hidden")),
    );
    let dup2 = def(
        DEFDEF_TAG,
        "dup2",
        select_via_shared_term(addr("shared_hidden")),
    );

    // `poison`'s declared type is an `APPLY`, a term shape semantic
    // projection never reads through (`type_of_term`'s fallback, and
    // `type_at`, both have no arm for it): the `poisoned` tree nested in its
    // argument must never be discovered (§23).
    let poison_arg = select_via_shared_term(addr("poisoned"));
    let poison_fn = named(IDENT_TAG, n("sel"), &any_type());
    let poison = def(
        DEFDEF_TAG,
        "poison",
        node(APPLY_TAG, &[poison_fn, poison_arg].concat()),
    );

    // `modesens` reaches `hidden_c` twice: first as the prefix of a
    // name-based `TYPEREF` (`SemanticType` mode, which does not descend into
    // a `REFINEDtpt` tag at all — a shallow, non-entering visit), then as the
    // class argument of a `THIS` (`ClassRef` mode, which does). The
    // mode-sensitive memo (§3/§22) must not let the first, shallow visit
    // suppress the second.
    let tycon = named(
        TYPEREF_TAG,
        n("sel"),
        &leaf(SHAREDTYPE_TAG, addr("hidden_c")),
    );
    let modesens_arg = wrap(THIS_TAG, &leaf(TYPEREFDIRECT_TAG, addr("hidden_c")));
    let modesens = def(
        DEFDEF_TAG,
        "modesens",
        node(APPLIEDTPT_TAG, &[tycon, modesens_arg].concat()),
    );

    // `selfref: p { val x: this.type }`: `x`'s own declared type names the
    // refinement's own synthetic class through `THIS`, exactly the real
    // library shape Milestone 5d2b's bug fix (`this_class`'s `REFINEDtpt`
    // arm) targets. Its own owner (`selfref`, the enclosing `DEFDEF`) must
    // not be overwritten by the member's discovery reaching it "again" as if
    // it were a fresh, differently-owned identity (the false-conflict bug
    // fixed while implementing this module).
    let this_of_selfref = wrap(THIS_TAG, &leaf(TYPEREFDIRECT_TAG, addr("selfref.refined")));
    let x_stat = def(VALDEF_TAG, "x", this_of_selfref);
    let selfref_body = refined(any_type(), &[x_stat]);
    let selfref_name = nat(n("selfref"));
    let selfref_payload = [selfref_name.clone(), selfref_body].concat();
    let selfref = node(DEFDEF_TAG, &selfref_payload);
    let selfref_refined_offset = u32::try_from(
        1 + nat(u32::try_from(selfref_payload.len()).unwrap()).len() + selfref_name.len(),
    )
    .unwrap();

    let holder = node(
        TYPEDEF_TAG,
        &[
            nat(n("Holder")),
            node(
                TEMPLATE_TAG,
                &[
                    any_type(),
                    m1,
                    m2,
                    m3,
                    dup1,
                    dup2,
                    poison,
                    modesens,
                    selfref,
                ]
                .concat(),
            ),
        ]
        .concat(),
    );
    let package = node(
        PACKAGE_TAG,
        &[leaf(TERMREFPKG_TAG, n("p")), holder].concat(),
    );

    // The loose trees, in the order they are appended (their sizes are all
    // fixed regardless of the placeholder addresses used above, since every
    // `nat` involved stays a single byte across attempts).
    let hidden_a = refined(any_type(), &[]);
    let hidden_b = refined(any_type(), &[]);
    let shared_hidden = refined(any_type(), &[]);
    let poisoned = refined(any_type(), &[]);
    let hidden_c = refined(any_type(), &[]);
    let hidden_e = refined(any_type(), &[]);
    let loose = [
        hidden_a,
        hidden_b,
        shared_hidden,
        poisoned,
        hidden_c,
        hidden_e,
    ];
    let labels = [
        "hidden_a",
        "hidden_b",
        "shared_hidden",
        "poisoned",
        "hidden_c",
        "hidden_e",
    ];

    let mut ast = package;
    let mut next = u32::try_from(ast.len()).unwrap();
    let mut found = HashMap::new();
    for (label, tree) in labels.iter().zip(&loose) {
        found.insert(*label, next);
        next += u32::try_from(tree.len()).unwrap();
    }
    for tree in &loose {
        ast.extend(tree);
    }
    found.insert("selfref.refined", addr("selfref") + selfref_refined_offset);
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

fn entered<'a>(
    file: &'a TastyFile<'a>,
    session: &'a mut Session,
) -> Result<TastyUnpickler<'a, 'a, 'a>, UnpickleError> {
    let mut packages = Packages::new();
    packages.enter(&mut session.store, SymbolOrigin::Synthetic, &["p"]);
    let mut unpickler =
        TastyUnpickler::with_packages(file, &mut session.store, session.definitions, packages);
    unpickler.enter_symbols()?;
    Ok(unpickler)
}

#[test]
fn a_refinement_hidden_behind_a_selecttpt_qualifier_is_entered() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let unpickler = entered(&file, &mut session).unwrap();

    let hidden_a = unit.at("hidden_a");
    assert!(unpickler.index().symbol_at(hidden_a).is_some());
    assert!(
        unpickler
            .index()
            .scope_of(unpickler.index().symbol_at(hidden_a).unwrap())
            .is_some()
    );
    let m1 = unpickler.index().symbol_at(unit.at("m1")).unwrap();
    assert_eq!(unpickler.index().refined_owner(hidden_a), Some(m1));
}

#[test]
fn a_refinement_hidden_behind_a_this_class_reference_is_entered() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let unpickler = entered(&file, &mut session).unwrap();

    let hidden_b = unit.at("hidden_b");
    assert!(unpickler.index().symbol_at(hidden_b).is_some());
    let m2 = unpickler.index().symbol_at(unit.at("m2")).unwrap();
    assert_eq!(unpickler.index().refined_owner(hidden_b), Some(m2));
}

#[test]
fn a_refinement_hidden_behind_a_this_class_reference_through_a_sharedtype_link_is_entered() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let unpickler = entered(&file, &mut session).unwrap();

    let hidden_e = unit.at("hidden_e");
    assert!(unpickler.index().symbol_at(hidden_e).is_some());
    let m3 = unpickler.index().symbol_at(unit.at("m3")).unwrap();
    assert_eq!(unpickler.index().refined_owner(hidden_e), Some(m3));
}

#[test]
fn a_hidden_refinement_reached_from_two_owners_through_selecttpt_still_conflicts() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let unpickler = entered(&file, &mut session).unwrap();

    let shared = unit.at("shared_hidden");
    assert!(unpickler.index().symbol_at(shared).is_some());
    assert!(unpickler.index().has_refined_owner_conflict(shared));
    // Only one synthetic class was ever allocated: the first owner
    // (`dup1`, wire order) keeps it.
    let dup1 = unpickler.index().symbol_at(unit.at("dup1")).unwrap();
    assert_eq!(unpickler.index().refined_owner(shared), Some(dup1));
}

#[test]
fn an_unsupported_term_body_is_not_over_scanned_for_a_hidden_identity() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let unpickler = entered(&file, &mut session).unwrap();

    // `poison`'s own declared type (the `APPLY` node) is never itself
    // entered as anything (`APPLY` is not an identity-bearing form), and the
    // `poisoned` tree nested in its argument must never be discovered.
    let poisoned = unit.at("poisoned");
    assert_eq!(unpickler.index().symbol_at(poisoned), None);
}

#[test]
fn an_identity_first_visited_under_a_shallow_mode_is_still_entered_under_a_richer_one() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let unpickler = entered(&file, &mut session).unwrap();

    // `hidden_c` is reached twice from `modesens`: first as a `TYPEREF`
    // prefix (`SemanticType` mode, which has no arm for a bare `REFINEDtpt`
    // tag and does not enter it), then as `THIS`'s class argument
    // (`ClassRef` mode, which does). With a memo keyed only by
    // `(tree, owner)` the first, shallow visit would have marked the address
    // "seen" and the second, entering visit would never run.
    let hidden_c = unit.at("hidden_c");
    assert!(unpickler.index().symbol_at(hidden_c).is_some());
    let modesens = unpickler.index().symbol_at(unit.at("modesens")).unwrap();
    assert_eq!(unpickler.index().refined_owner(hidden_c), Some(modesens));
}

#[test]
fn a_member_naming_its_own_enclosing_refinement_through_this_keeps_the_refinements_true_owner() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let unpickler = entered(&file, &mut session).unwrap();

    // `x`'s own declared type discovery (owned by `x` itself, the way a
    // nested `LAMBDAtpt` parameter would be) reaches the refinement's own
    // address through `THIS`. That address was already entered by the
    // refinement's *own* declared-type position (`selfref`'s result type,
    // Milestone 5d2b's two-stage entering, before any member's type is
    // scanned) — this must resolve to the existing identity, not be
    // misreported as a second, conflicting owner.
    let selfref_refined = unit.at("selfref.refined");
    assert!(
        !unpickler
            .index()
            .has_refined_owner_conflict(selfref_refined)
    );
    let selfref = unpickler.index().symbol_at(unit.at("selfref")).unwrap();
    assert_eq!(
        unpickler.index().refined_owner(selfref_refined),
        Some(selfref)
    );
}

/// The address of the sole child (the declared result type) of the `DEFDEF`
/// at `defdef_at`.
fn result_type_at(file: &TastyFile<'_>, defdef_at: u32) -> u32 {
    let index = file.ast_address_index().unwrap();
    index
        .iter_tree_edges()
        .find(|edge| u32::try_from(edge.parent.offset).unwrap() == defdef_at)
        .map(|edge| u32::try_from(edge.child.offset).unwrap())
        .unwrap()
}

#[test]
fn projection_succeeds_for_a_refinement_only_reachable_through_a_hidden_route() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session).unwrap();

    // `m2: this.type` projects cleanly now that pass 1 has already entered
    // the hidden `hidden_b` identity: projection never has to discover a
    // missing symbol on its own.
    let result_at = result_type_at(&file, unit.at("m2"));
    let ty = unpickler.unpickle_type_tree_type(result_at);
    assert!(ty.is_ok(), "projection failed: {ty:?}");
}
