//! Milestone 5b: term-tree `tpe` projection, and the type trees built on it
//! (`SELECTtpt`, `SINGLETONtpt`, `ANNOTATEDtpt`).
//!
//! The unit is built by hand: a package `p` with `Box` (a class with a type
//! member `Out`) and `Holder` (a `val x`, a `var v` and a method `f` with a
//! by-name parameter), then loose trees under an `APPLY` (whose arguments are
//! arbitrary trees), each with an address of its own.
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use dotty_core::Definitions;
use dotty_core::Packages;
use dotty_core::ids::SymbolId;
use dotty_core::ids::TypeId;
use dotty_core::names::Namespace;
use dotty_core::resolution::{MemberRequest, MemberSelector, ResolutionError, SymbolResolver};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolInfo, SymbolOrigin};
use dotty_core::types::{TermRefTarget, Type, TypeRefTarget};
use dotty_tasty::tasty::{
    APPLY_TAG, DEFDEF_TAG, Header, MUTABLE_TAG, NameTable, PACKAGE_TAG, PARAM_TAG, RawName,
    SHAREDTERM_TAG, SHAREDTYPE_TAG, Section, SectionTable, TEMPLATE_TAG, TERMREFDIRECT_TAG,
    TERMREFPKG_TAG, TYPEBOUNDSTPT_TAG, TYPEDEF_TAG, TYPEREFDIRECT_TAG, TYPEREFPKG_TAG, TastyFile,
    VALDEF_TAG,
};
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

const NAMES: [&str; 12] = [
    "ASTs", "p", "Box", "Out", "Holder", "x", "v", "f", "a", "Missing", "Twins", "Dup",
];

fn n(text: &str) -> u32 {
    u32::try_from(NAMES.iter().position(|name| *name == text).unwrap()).unwrap()
}

const IDENT: u8 = 110;
const IDENTTPT: u8 = 111;
const QUALTHIS: u8 = 91;
const INLINED: u8 = 147;
const TRUECONST: u8 = 4;
const APPLIEDTPT: u8 = 162;
const SELECT: u8 = 112;
const SINGLETONTPT: u8 = 101;
const SELECTTPT: u8 = 113;
const THIS: u8 = 90;
const REFINEDTYPE: u8 = 159;

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

/// The definitions of the unit, in document order.
const DEFINITIONS: [&str; 10] = [
    "Box",
    "Box.Out",
    "Holder",
    "Holder.x",
    "Holder.v",
    "Holder.f",
    "Holder.f.a",
    "Twins",
    "Twins.Dup1",
    "Twins.Dup2",
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
        ("select class", named(SELECTTPT, n("Out"), &box_type())),
        (
            "select class again",
            named(SELECTTPT, n("Out"), &box_type()),
        ),
        ("shared select", leaf(SHAREDTERM_TAG, at.of("select class"))),
        (
            "select applied",
            named(
                SELECTTPT,
                n("Out"),
                &node(APPLIEDTPT, &[ident_box(), ident_any()].concat()),
            ),
        ),
        (
            "select stable",
            named(SELECTTPT, n("Out"), &term("Holder.x")),
        ),
        (
            "select mutable",
            named(SELECTTPT, n("Out"), &term("Holder.v")),
        ),
        (
            "select method",
            named(SELECTTPT, n("Out"), &term("Holder.f")),
        ),
        (
            "select by-name",
            named(SELECTTPT, n("Out"), &term("Holder.f.a")),
        ),
        (
            "select missing",
            named(SELECTTPT, n("Missing"), &box_type()),
        ),
        (
            "select structural",
            named(
                SELECTTPT,
                n("Missing"),
                &node(
                    REFINEDTYPE,
                    &[nat(n("Missing")), box_type(), any_type()].concat(),
                ),
            ),
        ),
        (
            "select dup",
            named(
                SELECTTPT,
                n("Dup"),
                &leaf(TYPEREFDIRECT_TAG, at.of("Twins")),
            ),
        ),
        (
            "select term",
            named(
                SELECT,
                n("x"),
                &wrap(THIS, &leaf(TYPEREFDIRECT_TAG, at.of("Holder"))),
            ),
        ),
        (
            "select over select",
            named(
                SELECTTPT,
                n("Out"),
                &named(
                    SELECT,
                    n("x"),
                    &wrap(THIS, &leaf(TYPEREFDIRECT_TAG, at.of("Holder"))),
                ),
            ),
        ),
        (
            "select over shared",
            named(
                SELECTTPT,
                n("Out"),
                &leaf(SHAREDTERM_TAG, at.of("select term")),
            ),
        ),
        (
            "select rollback",
            named(SELECTTPT, n("Missing"), &wrap(QUALTHIS, &ident_box())),
        ),
        ("select bad name", named(SELECTTPT, 999, &box_type())),
        ("singleton stable", wrap(SINGLETONTPT, &term("Holder.x"))),
        (
            "singleton stable again",
            wrap(SINGLETONTPT, &term("Holder.x")),
        ),
        (
            "shared singleton",
            leaf(SHAREDTERM_TAG, at.of("singleton stable")),
        ),
        (
            "singleton package",
            wrap(SINGLETONTPT, &leaf(TERMREFPKG_TAG, n("p"))),
        ),
        ("singleton constant", wrap(SINGLETONTPT, &[TRUECONST])),
        (
            "singleton this",
            wrap(
                SINGLETONTPT,
                &wrap(THIS, &leaf(TYPEREFDIRECT_TAG, at.of("Holder"))),
            ),
        ),
        (
            "singleton qualthis",
            wrap(SINGLETONTPT, &wrap(QUALTHIS, &ident_box())),
        ),
        (
            "singleton select",
            wrap(
                SINGLETONTPT,
                &named(
                    SELECT,
                    n("x"),
                    &wrap(THIS, &leaf(TYPEREFDIRECT_TAG, at.of("Holder"))),
                ),
            ),
        ),
        (
            "singleton select var",
            wrap(
                SINGLETONTPT,
                &named(
                    SELECT,
                    n("v"),
                    &wrap(THIS, &leaf(TYPEREFDIRECT_TAG, at.of("Holder"))),
                ),
            ),
        ),
        ("singleton mutable", wrap(SINGLETONTPT, &term("Holder.v"))),
        ("singleton method", wrap(SINGLETONTPT, &term("Holder.f"))),
        ("singleton by-name", wrap(SINGLETONTPT, &term("Holder.f.a"))),
        ("singleton class", wrap(SINGLETONTPT, &box_type())),
        (
            "singleton inlined",
            wrap(SINGLETONTPT, &node(INLINED, &box_type())),
        ),
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
    let dup = || node(TYPEDEF_TAG, &[nat(n("Dup")), bounds()].concat());
    let twins = node(
        TYPEDEF_TAG,
        &[
            nat(n("Twins")),
            node(TEMPLATE_TAG, &[any_type(), dup(), dup()].concat()),
        ]
        .concat(),
    );
    let package = node(
        PACKAGE_TAG,
        &[leaf(TERMREFPKG_TAG, n("p")), box_class, holder, twins].concat(),
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

// SELECTtpt and SELECT

fn complete(unpickler: &mut TastyUnpickler<'_, '_, '_>, unit: &Unit, label: &str) {
    unpickler.complete_symbol(unit.at(label)).unwrap();
}

/// `(prefix, member symbol)` of a `TypeRef` / `TermRef` to a symbol.
fn member_of(session: &Session, ty: TypeId) -> (TypeId, SymbolId) {
    match session.store.types.get(ty) {
        Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(symbol),
        }
        | Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(symbol),
        } => (*prefix, *symbol),
        other => panic!("not a reference to a symbol: {other:?}"),
    }
}

#[test]
fn a_selection_on_a_class_type_is_a_type_ref_to_the_member_with_that_prefix() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let selected = unpickler
        .unpickle_type_tree_type(unit.at("select class"))
        .unwrap();
    let out = symbol(&unpickler, &unit, "Box.Out");
    let box_class = symbol(&unpickler, &unit, "Box");
    drop(unpickler);
    let (prefix, member) = member_of(&session, selected);
    assert_eq!(member, out);
    assert_eq!(
        session.store.types.get(prefix).reference_symbol(),
        Some(box_class)
    );
}

#[test]
fn a_selection_on_an_applied_qualifier_keeps_the_applied_prefix() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let selected = unpickler
        .unpickle_type_tree_type(unit.at("select applied"))
        .unwrap();
    drop(unpickler);
    let (prefix, _) = member_of(&session, selected);
    assert!(matches!(
        session.store.types.get(prefix),
        Type::Applied { .. }
    ));
}

#[test]
fn a_selection_on_a_completed_stable_term_keeps_the_path_dependent_prefix() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    // The term has no completed type yet: no member is guessed, and nothing
    // is completed on the way.
    let types = unpickler.index().type_count();
    let result = unpickler.unpickle_type_tree_type(unit.at("select stable"));
    assert!(
        matches!(
            result,
            Err(UnpickleError::UnsupportedResolutionPrefix { .. })
        ),
        "{result:?}"
    );
    assert_eq!(unpickler.index().type_count(), types);
    assert_eq!(
        unpickler
            .symbol_state_at(unit.at("Holder.x"))
            .map(|state| state.1),
        Some(SymbolInfo::Missing)
    );

    complete(&mut unpickler, &unit, "Holder.x");
    let selected = unpickler
        .unpickle_type_tree_type(unit.at("select stable"))
        .unwrap();
    let x = symbol(&unpickler, &unit, "Holder.x");
    let out = symbol(&unpickler, &unit, "Box.Out");
    drop(unpickler);
    let (prefix, member) = member_of(&session, selected);
    assert_eq!(member, out);
    // The prefix is the singleton `x.type`, not its widened class type.
    assert!(matches!(
        session.store.types.get(prefix),
        Type::TermRef { .. }
    ));
    assert_eq!(session.store.types.get(prefix).reference_symbol(), Some(x));
}

#[test]
fn an_unstable_qualifier_is_refused_and_never_selected_from() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    // Completed, so that only stability can refuse them.
    for label in ["Holder.v", "Holder.f.a"] {
        complete(&mut unpickler, &unit, label);
    }

    for label in ["select mutable", "select method", "select by-name"] {
        let types = unpickler.index().type_count();
        let result = unpickler.unpickle_type_tree_type(unit.at(label));
        assert!(
            matches!(result, Err(UnpickleError::UnstableSelectQualifier { address, .. }) if address == unit.at(label)),
            "{label}: {result:?}"
        );
        assert_eq!(unpickler.index().type_count(), types, "{label}");
    }
}

#[test]
fn a_member_that_is_not_there_is_unresolved_not_a_guess() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let result = unpickler.unpickle_type_tree_type(unit.at("select missing"));
    assert!(
        matches!(
            &result,
            Err(UnpickleError::UnresolvedMember { name, namespace: Namespace::Type, .. }) if name == "Missing"
        ),
        "{result:?}"
    );
}

#[test]
fn a_member_of_a_structural_qualifier_is_selected_by_name_with_no_symbol() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let selected = unpickler
        .unpickle_type_tree_type(unit.at("select structural"))
        .unwrap();
    drop(unpickler);
    let Type::TypeRef { prefix, target } = session.store.types.get(selected) else {
        panic!("not a type reference");
    };
    assert!(matches!(
        session.store.types.get(*prefix),
        Type::Refined { .. }
    ));
    let TypeRefTarget::Name(name) = target else {
        panic!("a structural member has no symbol");
    };
    assert_eq!(
        session.store.names.resolve(name.as_name().text()),
        "Missing"
    );
}

#[test]
fn overloaded_members_are_ambiguous_and_the_first_is_never_taken() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let result = unpickler.unpickle_type_tree_type(unit.at("select dup"));
    assert!(
        matches!(
            &result,
            Err(UnpickleError::AmbiguousMember { candidates: 2, name, .. }) if name == "Dup"
        ),
        "{result:?}"
    );
}

/// Answers every member request with one fixed symbol and logs the requests.
struct Fixed {
    answer: Option<SymbolId>,
    log: Rc<RefCell<Vec<MemberRequest>>>,
}

impl SymbolResolver for Fixed {
    fn resolve_member(
        &mut self,
        _store: &SemanticStore,
        request: &MemberRequest,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        self.log.borrow_mut().push(request.clone());
        Ok(self.answer)
    }

    fn resolve_package(
        &mut self,
        _store: &SemanticStore,
        _path: &[&str],
    ) -> Result<Option<SymbolId>, ResolutionError> {
        Ok(None)
    }
}

#[test]
fn the_resolver_is_asked_for_a_member_the_local_scope_does_not_have() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let unpickler = entered(&file, &mut session);
    let holder = symbol(&unpickler, &unit, "Holder");
    let box_class = symbol(&unpickler, &unit, "Box");
    let log = Rc::default();
    let mut unpickler = unpickler.with_resolver(Box::new(Fixed {
        answer: Some(holder),
        log: Rc::clone(&log),
    }));

    let selected = unpickler
        .unpickle_type_tree_type(unit.at("select missing"))
        .unwrap();
    drop(unpickler);
    let (prefix, member) = member_of(&session, selected);
    assert_eq!(member, holder);
    assert_eq!(
        session.store.types.get(prefix).reference_symbol(),
        Some(box_class)
    );
    let requests = log.borrow();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].prefix, prefix);
    assert_eq!(
        session.store.names.resolve(requests[0].name.text()),
        "Missing"
    );
    assert_eq!(requests[0].name.namespace(), Namespace::Type);
    assert_eq!(requests[0].selector, MemberSelector::Unique);
}

#[test]
fn a_local_member_never_asks_the_resolver() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let unpickler = entered(&file, &mut session);
    let log = Rc::default();
    let mut unpickler = unpickler.with_resolver(Box::new(Fixed {
        answer: None,
        log: Rc::clone(&log),
    }));

    unpickler
        .unpickle_type_tree_type(unit.at("select class"))
        .unwrap();
    assert!(log.borrow().is_empty());
}

#[test]
fn a_term_selection_is_a_term_ref_and_can_qualify_a_type_selection() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let term = unpickler
        .unpickle_term_type(unit.at("select term"))
        .unwrap();
    assert_eq!(
        unpickler.index().term_tree_type_at(unit.at("select term")),
        Some(term)
    );
    complete(&mut unpickler, &unit, "Holder.x");
    let through = unpickler
        .unpickle_type_tree_type(unit.at("select over select"))
        .unwrap();
    let x = symbol(&unpickler, &unit, "Holder.x");
    let holder = symbol(&unpickler, &unit, "Holder");
    drop(unpickler);
    let (prefix, member) = member_of(&session, term);
    assert_eq!(member, x);
    assert!(
        matches!(session.store.types.get(prefix), Type::ThisType { class } if *class == holder)
    );
    // `this.x.Out`: the qualifier is the `TermRef` of the term selection.
    let (qualifier, _) = member_of(&session, through);
    assert!(matches!(
        session.store.types.get(qualifier),
        Type::TermRef { .. }
    ));
    assert_eq!(
        session.store.types.get(qualifier).reference_symbol(),
        Some(x)
    );
}

#[test]
fn projecting_a_selection_again_is_the_same_type_and_two_written_ones_are_two_types() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let first = unpickler
        .unpickle_type_tree_type(unit.at("select class"))
        .unwrap();
    let types = unpickler.index().type_count();
    let trees = unpickler.index().type_tree_count();
    assert_eq!(
        unpickler.unpickle_type_tree_type(unit.at("select class")),
        Ok(first)
    );
    assert_eq!(unpickler.index().type_count(), types);
    assert_eq!(unpickler.index().type_tree_count(), trees);
    // Written twice: two trees, two types, no structural interning.
    let again = unpickler
        .unpickle_type_tree_type(unit.at("select class again"))
        .unwrap();
    assert_ne!(again, first);
    // A link owns nothing and is the target's own type.
    assert_eq!(
        unpickler.unpickle_type_tree_type(unit.at("shared select")),
        Ok(first)
    );
    assert_eq!(
        unpickler
            .index()
            .type_tree_type_at(unit.at("shared select")),
        None
    );
}

#[test]
fn a_selection_through_a_shared_term_qualifier_is_the_qualifier_the_link_names() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    complete(&mut unpickler, &unit, "Holder.x");

    let direct = unpickler
        .unpickle_term_type(unit.at("select term"))
        .unwrap();
    let selected = unpickler
        .unpickle_type_tree_type(unit.at("select over shared"))
        .unwrap();
    drop(unpickler);
    let (qualifier, _) = member_of(&session, selected);
    assert_eq!(qualifier, direct);
}

#[test]
fn a_selection_that_fails_after_its_qualifier_projected_leaves_nothing_behind() {
    // `this.Missing` of `Box`: the qualifier (a `QUALTHIS`, which allocates a
    // `ThisType` and a term-cache entry) projects, then the member is not
    // found.
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let types = unpickler.index().type_count();
    let arena = {
        drop(unpickler);
        let count = session.store.types.alloc(Type::NoType).index();
        unpickler = entered(&file, &mut session);
        count
    };
    let _ = arena;
    let result = unpickler.unpickle_type_tree_type(unit.at("select rollback"));
    assert!(
        matches!(result, Err(UnpickleError::UnresolvedMember { .. })),
        "{result:?}"
    );
    assert_eq!(unpickler.index().type_count(), types);
    assert_eq!(unpickler.index().term_tree_count(), 0);
    assert_eq!(unpickler.index().type_tree_count(), 0);
    // Retrying behaves as if the failed call never happened.
    let again = unpickler.unpickle_type_tree_type(unit.at("select rollback"));
    assert_eq!(again, result);
}

#[test]
fn a_selection_name_is_validated_before_anything_is_recorded() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    assert_eq!(
        unpickler.unpickle_type_tree_type(unit.at("select bad name")),
        Err(UnpickleError::InvalidNameReference { reference: 999 })
    );
    assert_eq!(unpickler.index().type_tree_count(), 0);
    assert_eq!(unpickler.index().term_tree_count(), 0);
}

// SINGLETONtpt

#[test]
fn a_singleton_type_tree_is_exactly_the_tpe_of_its_reference() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let singleton = unpickler
        .unpickle_type_tree_type(unit.at("singleton stable"))
        .unwrap();
    // The reference is the child written right after the `SINGLETONtpt` tag:
    // its own type node's id, with no wrapper allocated.
    assert_eq!(
        unpickler.index().type_at(unit.at("singleton stable") + 1),
        Some(singleton)
    );
    let x = symbol(&unpickler, &unit, "Holder.x");
    let types = unpickler.index().type_count();
    assert_eq!(
        unpickler.unpickle_type_tree_type(unit.at("singleton stable")),
        Ok(singleton)
    );
    assert_eq!(unpickler.index().type_count(), types);
    // Written twice, both are the reference's tpe of their own address.
    let again = unpickler
        .unpickle_type_tree_type(unit.at("singleton stable again"))
        .unwrap();
    assert_ne!(again, singleton);
    assert_eq!(
        unpickler.unpickle_type_tree_type(unit.at("shared singleton")),
        Ok(singleton)
    );
    assert_eq!(
        unpickler
            .index()
            .type_tree_type_at(unit.at("shared singleton")),
        None
    );
    drop(unpickler);
    assert_eq!(
        session.store.types.get(singleton).reference_symbol(),
        Some(x)
    );
}

#[test]
fn a_singleton_of_a_package_a_constant_a_this_and_a_qualified_this_projects() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let mut projected = Vec::new();
    for label in [
        "singleton package",
        "singleton constant",
        "singleton this",
        "singleton qualthis",
        "singleton select",
    ] {
        projected.push(unpickler.unpickle_type_tree_type(unit.at(label)).unwrap());
    }
    drop(unpickler);
    let kinds: Vec<&Type> = projected
        .iter()
        .map(|ty| session.store.types.get(*ty))
        .collect();
    assert!(matches!(kinds[0], Type::TermRef { .. }));
    assert!(matches!(kinds[1], Type::Constant(_)));
    assert!(matches!(kinds[2], Type::ThisType { .. }));
    assert!(matches!(kinds[3], Type::ThisType { .. }));
    assert!(matches!(kinds[4], Type::TermRef { .. }));
}

#[test]
fn a_singleton_of_something_that_is_not_a_stable_singleton_is_invalid() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    complete(&mut unpickler, &unit, "Holder.f.a");

    for label in [
        "singleton mutable",
        "singleton method",
        "singleton by-name",
        "singleton class",
    ] {
        let types = unpickler.index().type_count();
        let result = unpickler.unpickle_type_tree_type(unit.at(label));
        assert!(
            matches!(result, Err(UnpickleError::InvalidSingletonTypeTree { address, .. }) if address == unit.at(label)),
            "{label}: {result:?}"
        );
        assert_eq!(unpickler.index().type_count(), types, "{label}");
        assert_eq!(unpickler.index().type_tree_type_at(unit.at(label)), None);
    }
}

#[test]
fn a_singleton_that_is_refused_after_its_reference_projected_leaves_nothing_behind() {
    // `this.v.type` for a `var v`: the qualifier and the selection allocate
    // and are cached, then the singleton check refuses.
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let types = unpickler.index().type_count();
    let result = unpickler.unpickle_type_tree_type(unit.at("singleton select var"));
    assert!(
        matches!(result, Err(UnpickleError::InvalidSingletonTypeTree { .. })),
        "{result:?}"
    );
    assert_eq!(unpickler.index().type_count(), types);
    assert_eq!(unpickler.index().term_tree_count(), 0);
    assert_eq!(unpickler.index().type_tree_count(), 0);
    assert_eq!(
        unpickler.unpickle_type_tree_type(unit.at("singleton select var")),
        result
    );
}

#[test]
fn a_singleton_of_a_term_that_is_no_path_names_the_term_tree() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    assert_eq!(
        unpickler.unpickle_type_tree_type(unit.at("singleton inlined")),
        Err(UnpickleError::UnsupportedTermTree {
            address: unit.at("singleton inlined") + 1,
            tag: INLINED,
        })
    );
}
