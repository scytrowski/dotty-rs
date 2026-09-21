//! Milestone 5b: term-tree `tpe` projection, and the type trees built on it
//! (`SELECTtpt`, `SINGLETONtpt`, `ANNOTATEDtpt`).
//!
//! The unit is built by hand: a package `p` with `Box` (a class with a type
//! member `Out`) and `Holder` (a `val x`, a `var v` and a method `f` with a
//! by-name parameter), then loose trees under an `APPLY` (whose arguments are
//! arbitrary trees), each with an address of its own.
use std::collections::HashMap;

use dotty_core::Definitions;
use dotty_core::Packages;
use dotty_core::ids::SymbolId;
use dotty_core::store::SemanticStore;
use dotty_core::symbols::SymbolOrigin;
use dotty_core::types::Type;
use dotty_tasty::tasty::{
    APPLY_TAG, DEFDEF_TAG, Header, MUTABLE_TAG, NameTable, PACKAGE_TAG, PARAM_TAG, RawName,
    SHAREDTERM_TAG, SHAREDTYPE_TAG, Section, SectionTable, TEMPLATE_TAG, TERMREFDIRECT_TAG,
    TERMREFPKG_TAG, TYPEBOUNDSTPT_TAG, TYPEDEF_TAG, TYPEREFDIRECT_TAG, TYPEREFPKG_TAG, TastyFile,
    VALDEF_TAG,
};
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

const NAMES: [&str; 8] = ["ASTs", "p", "Box", "Out", "Holder", "x", "v", "f"];
const N_A: u32 = 8;

fn n(text: &str) -> u32 {
    if text == "a" {
        return N_A;
    }
    u32::try_from(NAMES.iter().position(|name| *name == text).unwrap()).unwrap()
}

const IDENT: u8 = 110;
const IDENTTPT: u8 = 111;
const QUALTHIS: u8 = 91;
const INLINED: u8 = 147;
const TRUECONST: u8 = 4;
const APPLIEDTPT: u8 = 162;

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

/// A tag, a name and one child tree (`IDENT`, `IDENTtpt`).
fn named(tag: u8, name: u32, child: &[u8]) -> Vec<u8> {
    [&[tag][..], &nat(name), child].concat()
}

/// A tag and one child tree, no length (`QUALTHIS`).
fn wrap(tag: u8, child: &[u8]) -> Vec<u8> {
    [&[tag][..], child].concat()
}

fn any_type() -> Vec<u8> {
    leaf(TYPEREFPKG_TAG, n("p"))
}

fn file_with(ast: &[u8]) -> Vec<u8> {
    let names = NameTable::from_entries(
        NAMES
            .iter()
            .copied()
            .chain(["a"])
            .map(|text| RawName::Utf8(text.to_owned()))
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

/// The definitions of the unit, in document order.
const DEFINITIONS: [&str; 7] = [
    "Box",
    "Box.Out",
    "Holder",
    "Holder.x",
    "Holder.v",
    "Holder.f",
    "Holder.f.a",
];

type Roots = Vec<(&'static str, Vec<u8>)>;

/// The addresses of the last layout round, 0 for one not known yet.
struct Addresses<'a>(&'a HashMap<&'static str, u32>);

impl Addresses<'_> {
    fn of(&self, label: &str) -> u32 {
        self.0.get(label).copied().unwrap_or(0)
    }
}

/// The loose trees, in order.
fn roots(at: &Addresses<'_>) -> Roots {
    let ident_any = || named(IDENTTPT, n("p"), &any_type());
    let box_type = || leaf(TYPEREFDIRECT_TAG, at.of("Box"));
    let ident_box = || named(IDENTTPT, n("Box"), &box_type());
    let term = |label: &str| leaf(TERMREFDIRECT_TAG, at.of(label));
    vec![
        ("ident", named(IDENT, n("x"), &box_type())),
        ("bad ident name", named(IDENT, 999, &box_type())),
        ("shared ident", leaf(SHAREDTERM_TAG, at.of("ident"))),
        ("shared chain", leaf(SHAREDTERM_TAG, at.of("shared ident"))),
        ("shared cycle", leaf(SHAREDTERM_TAG, at.of("shared cycle"))),
        ("shared bad", leaf(SHAREDTERM_TAG, 1)),
        ("direct term", term("Holder.x")),
        ("constant", vec![TRUECONST]),
        ("shared type", leaf(SHAREDTYPE_TAG, at.of("direct term"))),
        ("qualthis", wrap(QUALTHIS, &ident_box())),
        ("qualthis bare", wrap(QUALTHIS, &box_type())),
        (
            "qualthis not a class",
            wrap(
                QUALTHIS,
                &named(
                    IDENTTPT,
                    n("Out"),
                    &leaf(TYPEREFDIRECT_TAG, at.of("Box.Out")),
                ),
            ),
        ),
        ("inlined", node(INLINED, &box_type())),
        ("tpt in term position", ident_any()),
        (
            "applied cycle",
            node(
                APPLIEDTPT,
                &[ident_any(), leaf(SHAREDTERM_TAG, at.of("applied cycle"))].concat(),
            ),
        ),
    ]
}

struct Unit {
    bytes: Vec<u8>,
    /// Definition and root addresses by label.
    at: HashMap<&'static str, u32>,
}

impl Unit {
    fn at(&self, label: &str) -> u32 {
        self.at[label]
    }

    fn new() -> Self {
        // Addresses appear inside the trees, so the layout is iterated until
        // the addresses it produces are the ones it used.
        let mut at: HashMap<&'static str, u32> = HashMap::new();
        for _ in 0..8 {
            let (ast, found) = assemble(&Addresses(&at));
            let bytes = file_with(&ast);
            let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
            let mut definitions: Vec<u32> = file
                .ast_address_index()
                .unwrap()
                .iter_nodes()
                .filter(|node| {
                    matches!(node.tag, TYPEDEF_TAG | VALDEF_TAG | DEFDEF_TAG | PARAM_TAG)
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

fn assemble(at: &Addresses<'_>) -> (Vec<u8>, HashMap<&'static str, u32>) {
    let box_type = || leaf(TYPEREFDIRECT_TAG, at.of("Box"));
    let ident_box = || named(IDENTTPT, n("Box"), &box_type());
    let bounds = || node(TYPEBOUNDSTPT_TAG, &[any_type(), any_type()].concat());
    let val = |name: &str, modifiers: &[u8]| {
        node(
            VALDEF_TAG,
            &[nat(n(name)), ident_box(), modifiers.to_vec()].concat(),
        )
    };

    let out = node(TYPEDEF_TAG, &[nat(n("Out")), bounds()].concat());
    let box_class = node(
        TYPEDEF_TAG,
        &[
            nat(n("Box")),
            node(TEMPLATE_TAG, &[any_type(), out].concat()),
        ]
        .concat(),
    );
    let by_name_param = node(PARAM_TAG, &[nat(n("a")), wrap(94, &ident_box())].concat());
    let members = [
        any_type(),
        val("x", &[]),
        val("v", &[MUTABLE_TAG]),
        node(
            DEFDEF_TAG,
            &[nat(n("f")), by_name_param, any_type()].concat(),
        ),
    ];
    let holder = node(
        TYPEDEF_TAG,
        &[nat(n("Holder")), node(TEMPLATE_TAG, &members.concat())].concat(),
    );
    let package = node(
        PACKAGE_TAG,
        &[leaf(TERMREFPKG_TAG, n("p")), box_class, holder].concat(),
    );

    let loose = roots(at);
    let holder_payload: Vec<u8> = loose.iter().flat_map(|(_, tree)| tree.clone()).collect();
    let header = {
        let mut header = vec![APPLY_TAG];
        header.extend(nat(u32::try_from(holder_payload.len()).unwrap()));
        header
    };
    let mut ast = package;
    let mut next = u32::try_from(ast.len() + header.len()).unwrap();
    ast.extend(header);
    let mut found = HashMap::new();
    for (label, tree) in &loose {
        found.insert(*label, next);
        next += u32::try_from(tree.len()).unwrap();
    }
    ast.extend(holder_payload);
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

fn entered<'a>(file: &'a TastyFile<'a>, session: &'a mut Session) -> TastyUnpickler<'a, 'a, 'a> {
    let mut packages = Packages::new();
    packages.enter(&mut session.store, SymbolOrigin::Synthetic, &["p"]);
    let mut unpickler =
        TastyUnpickler::with_packages(file, &mut session.store, session.definitions, packages);
    unpickler.enter_symbols().unwrap();
    unpickler
}

fn symbol(unpickler: &TastyUnpickler<'_, '_, '_>, unit: &Unit, label: &str) -> SymbolId {
    unpickler.index().symbol_at(unit.at(label)).unwrap()
}

// Term-tree projection

#[test]
fn an_identifier_term_is_exactly_its_embedded_type() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let ty = unpickler.unpickle_term_type(unit.at("ident")).unwrap();
    // The embedded type node's own id: nothing was derived, and nothing was
    // recorded for the term.
    assert_eq!(unpickler.index().type_at(unit.at("ident") + 2), Some(ty));
    assert_eq!(unpickler.index().term_tree_type_at(unit.at("ident")), None);
    let box_class = symbol(&unpickler, &unit, "Box");
    drop(unpickler);
    assert_eq!(
        session.store.types.get(ty).reference_symbol(),
        Some(box_class)
    );
}

#[test]
fn an_identifier_name_is_validated_and_never_resolved() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    // `x` names nothing in scope here and does not have to.
    assert!(unpickler.unpickle_term_type(unit.at("ident")).is_ok());
    let types = unpickler.index().type_count();
    assert_eq!(
        unpickler.unpickle_term_type(unit.at("bad ident name")),
        Err(UnpickleError::InvalidNameReference { reference: 999 })
    );
    assert_eq!(unpickler.index().type_count(), types);
}

#[test]
fn a_shared_term_link_is_the_exact_target_projection_and_owns_nothing() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let target = unpickler.unpickle_term_type(unit.at("ident")).unwrap();
    for link in ["shared ident", "shared chain"] {
        assert_eq!(unpickler.unpickle_term_type(unit.at(link)), Ok(target));
        assert_eq!(unpickler.index().term_tree_type_at(unit.at(link)), None);
        assert_eq!(unpickler.index().type_at(unit.at(link)), None);
    }
    // Through the link first, the target is the same one.
    let mut fresh = Session::new();
    let mut other = entered(&file, &mut fresh);
    let through = other.unpickle_term_type(unit.at("shared chain")).unwrap();
    assert_eq!(other.unpickle_term_type(unit.at("ident")), Ok(through));
}

#[test]
fn a_shared_term_cycle_or_a_bad_link_is_a_typed_error_not_a_loop() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    assert_eq!(
        unpickler.unpickle_term_type(unit.at("shared cycle")),
        Err(UnpickleError::InvalidReferenceTarget {
            from: unit.at("shared cycle"),
            to: unit.at("shared cycle"),
        })
    );
    assert_eq!(
        unpickler.unpickle_term_type(unit.at("shared bad")),
        Err(UnpickleError::InvalidReferenceTarget {
            from: unit.at("shared bad"),
            to: 1,
        })
    );
}

#[test]
fn a_semantic_type_node_in_term_position_is_that_type_and_owns_no_term_entry() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let direct = unpickler
        .unpickle_term_type(unit.at("direct term"))
        .unwrap();
    assert_eq!(
        unpickler.index().type_at(unit.at("direct term")),
        Some(direct)
    );
    assert_eq!(
        unpickler.unpickle_term_type(unit.at("shared type")),
        Ok(direct)
    );
    let constant = unpickler.unpickle_term_type(unit.at("constant")).unwrap();
    assert_eq!(unpickler.index().term_tree_count(), 0);
    drop(unpickler);
    assert!(matches!(
        session.store.types.get(constant),
        Type::Constant(_)
    ));
}

#[test]
fn a_qualified_this_is_this_type_of_the_class_its_identifier_names() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let ty = unpickler.unpickle_term_type(unit.at("qualthis")).unwrap();
    assert_eq!(
        unpickler.index().term_tree_type_at(unit.at("qualthis")),
        Some(ty)
    );
    let box_class = symbol(&unpickler, &unit, "Box");
    drop(unpickler);
    assert_eq!(
        session.store.types.get(ty),
        &Type::ThisType { class: box_class }
    );
}

#[test]
fn projecting_a_derived_term_again_returns_the_cached_type_and_allocates_nothing() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let first = unpickler.unpickle_term_type(unit.at("qualthis")).unwrap();
    let types = unpickler.index().type_count();
    let terms = unpickler.index().term_tree_count();
    assert_eq!(unpickler.unpickle_term_type(unit.at("qualthis")), Ok(first));
    assert_eq!(unpickler.index().type_count(), types);
    assert_eq!(unpickler.index().term_tree_count(), terms);
}

#[test]
fn a_qualified_this_that_names_no_class_is_refused_and_leaves_nothing_behind() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    for label in ["qualthis bare", "qualthis not a class"] {
        assert!(
            unpickler.unpickle_term_type(unit.at(label)).is_err(),
            "{label}"
        );
    }
    assert_eq!(
        unpickler.unpickle_term_type(unit.at("qualthis bare")),
        Err(UnpickleError::MalformedType {
            address: unit.at("qualthis bare"),
            reason: "the qualifier of a qualified this is not a type identifier",
        })
    );
    assert_eq!(unpickler.index().term_tree_count(), 0);
}

#[test]
fn a_term_that_is_no_path_is_an_unsupported_term_tree() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    assert_eq!(
        unpickler.unpickle_term_type(unit.at("inlined")),
        Err(UnpickleError::UnsupportedTermTree {
            address: unit.at("inlined"),
            tag: INLINED,
        })
    );
}

#[test]
fn a_type_tree_in_term_position_is_its_type_tree_projection() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let through_term = unpickler
        .unpickle_term_type(unit.at("tpt in term position"))
        .unwrap();
    assert_eq!(
        unpickler.unpickle_type_tree_type(unit.at("tpt in term position")),
        Ok(through_term)
    );
    assert_eq!(unpickler.index().term_tree_count(), 0);
}

#[test]
fn a_tree_that_links_back_to_itself_is_bounded_not_a_stack_overflow() {
    // `APPLIEDtpt(ident, SHAREDterm -> itself)`: the only way a tree contains
    // itself, and the projection nests one link inside another without end.
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let types = unpickler.index().type_count();
    let trees = unpickler.index().type_tree_count();
    assert!(matches!(
        unpickler.unpickle_type_tree_type(unit.at("applied cycle")),
        Err(UnpickleError::InvalidReferenceTarget { .. })
    ));
    assert_eq!(unpickler.index().type_count(), types);
    assert_eq!(unpickler.index().type_tree_count(), trees);
}
