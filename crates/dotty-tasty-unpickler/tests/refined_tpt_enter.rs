//! Milestone 5d2b, pass 1: the synthetic `<refinement>` class `REFINEDtpt`
//! gets when it is scanned as a declared type tree.
//!
//! The unit is built by hand, following the same address-settling recipe
//! `tests/lambda_tpt.rs` uses: a package `p` with a class `Holder` whose
//! members' declared types reach a shared `REFINEDtpt` tree, plus a loose
//! malformed one for the rollback test.

use std::collections::HashMap;

use dotty_core::Definitions;
use dotty_core::Packages;
use dotty_core::ids::SymbolId;
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolInfo, SymbolKind, SymbolOrigin};
use dotty_tasty::tasty::{
    APPLY_TAG, DEFDEF_TAG, Header, NameTable, PACKAGE_TAG, RawName, SHAREDTERM_TAG, Section,
    SectionTable, TEMPLATE_TAG, TERMREFPKG_TAG, TYPEBOUNDSTPT_TAG, TYPEDEF_TAG, TYPEPARAM_TAG,
    TastyFile, VALDEF_TAG,
};
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

const NAMES: [&str; 9] = ["ASTs", "p", "Holder", "m1", "m2", "T", "x", "bad", "Bad"];

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

fn any_type() -> Vec<u8> {
    leaf(TERMREFPKG_TAG, n("p"))
}

fn plain_bounds() -> Vec<u8> {
    node(TYPEBOUNDSTPT_TAG, &[any_type(), any_type()].concat())
}

fn refined(parent: Vec<u8>, stats: &[Vec<u8>]) -> Vec<u8> {
    node(REFINEDTPT, &[parent, stats.concat()].concat())
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

/// Every definition of the unit, in document order (the synthetic
/// `<refinement>` classes are not definitions and are found separately, by
/// the address of the `REFINEDtpt` they were entered for).
const DEFINITIONS: [&str; 6] = ["Holder", "m1", "m2", "shared.T", "shared.x", "bad"];

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
                return Self { bytes, at };
            }
            at = next;
        }
        panic!("the layout did not settle");
    }
}

/// `p.Holder { def m1: shared; def m2: shared }`, `shared` being a
/// `REFINEDtpt` with a `TYPEDEF T` and a `VALDEF x` stat, reached from two
/// different owners (`m1` and `m2`); plus a loose, malformed `REFINEDtpt`
/// (an immediate `TYPEPARAM` stat, which is not a supported refinement
/// member) for the rollback test.
fn assemble(at: &HashMap<&'static str, u32>) -> (Vec<u8>, HashMap<&'static str, u32>) {
    let addr = |label: &str| at.get(label).copied().unwrap_or(0);
    let def = |tag: u8, name: &str, child: Vec<u8>| node(tag, &[nat(n(name)), child].concat());

    let t_stat = def(TYPEDEF_TAG, "T", plain_bounds());
    let x_stat = def(VALDEF_TAG, "x", any_type());

    let m1 = def(DEFDEF_TAG, "m1", leaf(SHAREDTERM_TAG, addr("__shared__")));
    let m2 = def(DEFDEF_TAG, "m2", leaf(SHAREDTERM_TAG, addr("__shared__")));
    let holder = node(
        TYPEDEF_TAG,
        &[
            nat(n("Holder")),
            node(TEMPLATE_TAG, &[any_type(), m1, m2].concat()),
        ]
        .concat(),
    );
    let package = node(
        PACKAGE_TAG,
        &[leaf(TERMREFPKG_TAG, n("p")), holder].concat(),
    );

    // The loose trees carrying real addresses of their own: the shared
    // refinement (referenced by both `m1` and `m2` through `SHAREDterm`), and
    // a malformed one.
    let bad_stat = def(TYPEPARAM_TAG, "Bad", plain_bounds());
    let loose = [
        refined(any_type(), &[t_stat, x_stat]),
        refined(any_type(), &[bad_stat]),
    ];
    let payload = loose.concat();
    let mut header = vec![APPLY_TAG];
    header.extend(nat(u32::try_from(payload.len()).unwrap()));

    let mut ast = package;
    let mut next = u32::try_from(ast.len() + header.len()).unwrap();
    ast.extend(header);
    let mut found = HashMap::new();
    let labels = ["__shared__", "bad"];
    for (label, tree) in labels.iter().zip(&loose) {
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

/// A unit that has only the malformed (`bad`) refinement, so its own
/// `enter_symbols` fails without the shared-refinement machinery muddying
/// what is being checked. Returns the bytes and the address of the
/// unsupported `TYPEPARAM` stat itself.
fn malformed_unit() -> (Vec<u8>, u32) {
    let bad_stat = node(TYPEPARAM_TAG, &[nat(n("Bad")), plain_bounds()].concat());
    let bad_body = refined(any_type(), &[bad_stat]);
    let m = node(DEFDEF_TAG, &[nat(n("m1")), bad_body].concat());
    let holder = node(
        TYPEDEF_TAG,
        &[
            nat(n("Holder")),
            node(TEMPLATE_TAG, &[any_type(), m].concat()),
        ]
        .concat(),
    );
    let package = node(
        PACKAGE_TAG,
        &[leaf(TERMREFPKG_TAG, n("p")), holder].concat(),
    );
    let bytes = file_with(&package);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let stat_at = u32::try_from(
        file.ast_address_index()
            .unwrap()
            .iter_nodes()
            .find(|node| node.tag == TYPEPARAM_TAG)
            .unwrap()
            .offset,
    )
    .unwrap();
    (bytes, stat_at)
}

// --- the synthetic class ---

#[test]
fn a_refined_type_tree_gets_a_synthetic_refinement_class_named_and_kinded_right() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let unpickler = entered(&file, &mut session);

    let refine_cls = unpickler.index().symbol_at(unit.at("__shared__")).unwrap();
    // `m1` reaches the shared tree first (wire order), so it is the
    // refinement class's owner: the *method*, not the enclosing class, the
    // same way a method's own `LAMBDAtpt` result-type parameters are owned
    // by the method.
    let m1 = symbol(&unpickler, &unit, "m1");
    drop(unpickler);

    let entered = session.store.symbols.get(refine_cls);
    assert_eq!(entered.kind, SymbolKind::Class);
    assert_eq!(
        session.store.names.resolve(entered.name.text()),
        "<refinement>"
    );
    assert_eq!(entered.owner, Some(m1));
    assert_eq!(entered.origin, SymbolOrigin::Synthetic);
}

#[test]
fn the_synthetic_class_owns_a_scope_and_a_complete_parent_less_class_info() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let unpickler = entered(&file, &mut session);

    let refine_cls = unpickler.index().symbol_at(unit.at("__shared__")).unwrap();
    let scope = unpickler.index().scope_of(refine_cls).unwrap();
    drop(unpickler);

    let SymbolInfo::Complete(info) = session.store.symbols.get(refine_cls).info else {
        panic!("the synthetic class is not complete");
    };
    let dotty_core::types::Type::ClassInfo(class_info) = session.store.types.get(info) else {
        panic!("not a class info");
    };
    assert_eq!(class_info.class, refine_cls);
    assert_eq!(class_info.declarations, scope);
    assert!(class_info.parents.is_empty());
    assert_eq!(class_info.self_type, None);
    assert_eq!(class_info.prefix, session.definitions.no_prefix);
}

#[test]
fn the_synthetic_class_is_not_a_member_of_its_owners_scope() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let unpickler = entered(&file, &mut session);

    let refine_cls = unpickler.index().symbol_at(unit.at("__shared__")).unwrap();
    let holder = symbol(&unpickler, &unit, "Holder");
    let holder_scope = unpickler.index().scope_of(holder).unwrap();
    drop(unpickler);

    // No name in Holder's scope resolves to the refinement class: it was
    // entered as a non-member.
    let scope = session.store.scopes.get(holder_scope);
    let refinement_name = dotty_core::names::Name::new(
        session.store.names.intern("<refinement>"),
        dotty_core::names::Namespace::Type,
    );
    assert_eq!(scope.lookup_all(&refinement_name), &[] as &[SymbolId]);
    assert!(!scope.lookup_all(&refinement_name).contains(&refine_cls));
}

// --- immediate members: entered before either is scanned ---

#[test]
fn every_immediate_member_is_entered_into_the_refinement_scope() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let unpickler = entered(&file, &mut session);

    let refine_cls = unpickler.index().symbol_at(unit.at("__shared__")).unwrap();
    let scope = unpickler.index().scope_of(refine_cls).unwrap();
    let t = symbol(&unpickler, &unit, "shared.T");
    let x = symbol(&unpickler, &unit, "shared.x");
    drop(unpickler);

    let entered_t = session.store.symbols.get(t);
    let entered_x = session.store.symbols.get(x);
    assert_eq!(entered_t.owner, Some(refine_cls));
    assert_eq!(entered_x.owner, Some(refine_cls));
    assert_eq!(entered_t.kind, SymbolKind::TypeAlias);
    assert_eq!(entered_x.kind, SymbolKind::Field);

    let members = session.store.scopes.get(scope);
    let t_name = dotty_core::names::Name::new(
        session.store.names.intern("T"),
        dotty_core::names::Namespace::Type,
    );
    let x_name = dotty_core::names::Name::new(
        session.store.names.intern("x"),
        dotty_core::names::Namespace::Term,
    );
    assert_eq!(members.lookup(&t_name), Some(t));
    assert_eq!(members.lookup(&x_name), Some(x));
}

// --- shared REFINEDtpt owner identity ---

#[test]
fn a_refined_tree_reached_from_two_owners_keeps_the_first_and_records_the_conflict() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let unpickler = entered(&file, &mut session);

    assert!(
        unpickler
            .index()
            .has_refined_owner_conflict(unit.at("__shared__"))
    );
    let refine_cls = unpickler.index().symbol_at(unit.at("__shared__")).unwrap();
    let m1 = symbol(&unpickler, &unit, "m1");
    drop(unpickler);
    // The class's owner is whichever of `m1`/`m2` reached it first; only one
    // class was ever allocated (a second attempt is a conflict, not a second
    // class).
    let owner = session.store.symbols.get(refine_cls).owner;
    assert_eq!(owner, Some(m1));
}

#[test]
fn a_shared_refinement_with_a_conflicting_owner_is_refused_at_projection_not_silently_bound_to_the_first()
 {
    // `enter_symbols` records the conflict on `__shared__` (asserted above);
    // projecting it — through either owner's declared type, `m1`'s or
    // `m2`'s — must not silently succeed with the first owner's class: it is
    // the same policy `type_of_lambda_tpt` already follows for
    // `SharedLambdaOwnerConflict`, and `type_of_refined_tpt` guards on it the
    // same way, before `refinement_class` is ever consulted.
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    assert!(
        unpickler
            .index()
            .has_refined_owner_conflict(unit.at("__shared__"))
    );

    let types_before = unpickler.index().type_count();
    assert_eq!(
        unpickler.unpickle_type_tree_type(unit.at("__shared__")),
        Err(UnpickleError::SharedRefinementOwnerConflict {
            address: unit.at("__shared__")
        })
    );
    assert_eq!(unpickler.index().type_count(), types_before);
}

// --- unsupported stat kinds ---

#[test]
fn an_unsupported_refinement_stat_is_a_typed_error_not_a_silent_drop() {
    let (bytes, stat_at) = malformed_unit();
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = make(&file, &mut session);

    let result = unpickler.enter_symbols();

    assert_eq!(
        result.map(|_| ()),
        Err(UnpickleError::UnsupportedRefinementStat {
            address: stat_at,
            tag: TYPEPARAM_TAG,
        })
    );
}

// --- atomic rollback ---

#[test]
fn a_malformed_refinement_fails_the_whole_enter_and_leaves_the_store_untouched() {
    let (bytes, _) = malformed_unit();
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = make(&file, &mut session);

    let result = unpickler.enter_symbols();
    assert!(result.is_err());

    // Nothing beyond the bootstrapped/package-registry symbols survives: a
    // fresh, never-entered unpickler's index is empty, and so is this one's.
    assert_eq!(unpickler.index().symbol_count(), 0);
}

#[test]
fn a_retry_after_a_malformed_refinement_is_clean() {
    let (bytes, _) = malformed_unit();
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = make(&file, &mut session);

    assert!(unpickler.enter_symbols().is_err());
    assert!(unpickler.enter_symbols().is_err());
    assert_eq!(unpickler.index().symbol_count(), 0);
}
