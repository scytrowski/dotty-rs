//! Milestone 5a: type-tree projection and simple symbol completion.
//!
//! The synthetic unit is built by hand (definitions and type trees); the real
//! Scala 3.9.0 fixtures are used further down.
use std::collections::HashMap;

use dotty_core::Definitions;
use dotty_core::Packages;
use dotty_core::ids::TypeId;
use dotty_core::names::Namespace;
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolInfo, SymbolKind, SymbolOrigin};
use dotty_core::types::Type;
use dotty_tasty::tasty::{
    APPLY_TAG, DEFDEF_TAG, Header, MUTABLE_TAG, NameTable, OPAQUE_TAG, PACKAGE_TAG, PARAM_TAG,
    RawName, SHAREDTERM_TAG, SHAREDTYPE_TAG, Section, SectionTable, TEMPLATE_TAG,
    TERMREFDIRECT_TAG, TERMREFPKG_TAG, TYPEBOUNDSTPT_TAG, TYPEDEF_TAG, TYPEPARAM_TAG, TYPEREF_TAG,
    TYPEREFDIRECT_TAG, TYPEREFPKG_TAG, TastyFile, VALDEF_TAG,
};
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

// Name table indices.
const N_P: u32 = 1;
const N_BOX: u32 = 2;
const N_OUT: u32 = 3;
const N_HOLDER: u32 = 4;
const N_A: u32 = 5;
const N_X: u32 = 6;
const N_V: u32 = 7;
const N_ALIAS: u32 = 8;
const N_ABS: u32 = 9;
const N_SAME: u32 = 10;
const N_OP: u32 = 11;
const N_BAD: u32 = 12;
const N_F: u32 = 13;
const N_PARAM: u32 = 14;
const N_S: u32 = 15;
const N_Y: u32 = 16;

const IDENTTPT: u8 = 111;
const APPLIEDTPT: u8 = 162;
const BYNAMETPT: u8 = 94;
const EXPLICITTPT: u8 = 103;
const SELECTTPT: u8 = 113;
const SINGLETONTPT: u8 = 101;

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

/// A tag, a name and one child tree (`IDENTtpt`, `SELECTtpt`, `TYPEREF`).
fn named(tag: u8, name: u32, child: &[u8]) -> Vec<u8> {
    [&[tag][..], &nat(name), child].concat()
}

/// A tag and one child tree, no length (`BYNAMEtpt`, `EXPLICITtpt`).
fn wrap(tag: u8, child: &[u8]) -> Vec<u8> {
    [&[tag][..], child].concat()
}

fn any_type() -> Vec<u8> {
    leaf(TYPEREFPKG_TAG, N_P)
}

fn file_with(ast: &[u8]) -> Vec<u8> {
    let names = NameTable::from_entries(
        [
            "ASTs", "p", "Box", "Out", "Holder", "A", "x", "v", "Alias", "Abs", "Same", "Op",
            "Bad", "f", "a", "s", "y", "m",
        ]
        .into_iter()
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
const DEFINITIONS: [&str; 15] = [
    "Box",
    "Box.Out",
    "Holder",
    "Holder.A",
    "Holder.x",
    "Holder.v",
    "Holder.Alias",
    "Holder.Abs",
    "Holder.Same",
    "Holder.Op",
    "Holder.Bad",
    "Holder.f",
    "Holder.f.a",
    "Holder.s",
    "Holder.y",
];

/// The loose type trees and references, in order, appended after the package.
const ROOTS: [&str; 22] = [
    "ident",
    "applied",
    "applied again",
    "by-name",
    "explicit",
    "bounds1",
    "bounds2",
    "bounds3",
    "shared",
    "shared chain",
    "shared cycle",
    "direct",
    "shared type",
    "select",
    "singleton",
    "bad applied",
    "x.Out",
    "y.Out",
    "v.Out",
    "a.Out",
    "f.Out",
    "s.Out",
];

struct Unit {
    bytes: Vec<u8>,
    /// Definition and root addresses by label.
    at: HashMap<&'static str, u32>,
}

impl Unit {
    fn at(&self, label: &str) -> u32 {
        self.at[label]
    }
}

/// The package and the roots, given the addresses of the last round.
fn assemble(at: &HashMap<&'static str, u32>) -> (Vec<u8>, HashMap<&'static str, u32>) {
    let def = |label: &str| at.get(label).copied().unwrap_or(0);
    let ident = |name: u32, ty: Vec<u8>| named(IDENTTPT, name, &ty);
    let ident_any = || ident(N_P, any_type());
    let ident_box = || ident(N_BOX, leaf(TYPEREFDIRECT_TAG, def("Box")));
    let bounds = |children: &[Vec<u8>]| node(TYPEBOUNDSTPT_TAG, &children.concat());

    let out = node(
        TYPEDEF_TAG,
        &[nat(N_OUT), bounds(&[any_type(), any_type()])].concat(),
    );
    let box_class = node(
        TYPEDEF_TAG,
        &[nat(N_BOX), node(TEMPLATE_TAG, &[any_type(), out].concat())].concat(),
    );

    let val = |name: u32, tpt: Vec<u8>, modifiers: &[u8]| {
        node(VALDEF_TAG, &[nat(name), tpt, modifiers.to_vec()].concat())
    };
    let alias = |name: u32, rhs: Vec<u8>, modifiers: &[u8]| {
        node(TYPEDEF_TAG, &[nat(name), rhs, modifiers.to_vec()].concat())
    };
    let by_name_param = node(
        PARAM_TAG,
        &[nat(N_PARAM), wrap(BYNAMETPT, &ident_box())].concat(),
    );
    let members = [
        node(
            TYPEPARAM_TAG,
            &[nat(N_A), bounds(&[any_type(), any_type()])].concat(),
        ),
        any_type(),
        val(N_X, ident_box(), &[]),
        val(N_V, ident_box(), &[MUTABLE_TAG]),
        alias(N_ALIAS, ident_any(), &[]),
        alias(N_ABS, bounds(&[any_type(), any_type()]), &[]),
        alias(N_SAME, bounds(&[any_type()]), &[]),
        alias(N_OP, ident_any(), &[OPAQUE_TAG]),
        alias(N_BAD, named(SELECTTPT, N_OUT, &any_type()), &[]),
        node(DEFDEF_TAG, &[nat(N_F), by_name_param, any_type()].concat()),
        val(
            N_S,
            ident(N_S, leaf(TERMREFDIRECT_TAG, def("Holder.s"))),
            &[],
        ),
        val(
            N_Y,
            node(APPLIEDTPT, &[ident_box(), ident_any()].concat()),
            &[],
        ),
    ];
    let holder = node(
        TYPEDEF_TAG,
        &[nat(N_HOLDER), node(TEMPLATE_TAG, &members.concat())].concat(),
    );
    let package = node(
        PACKAGE_TAG,
        &[leaf(TERMREFPKG_TAG, N_P), box_class, holder].concat(),
    );

    // The loose roots, under an `APPLY` (whose arguments are arbitrary
    // trees), so each has an address of its own.
    let root = |label: &str| at.get(label).copied().unwrap_or(0);
    let term = |label: &str| leaf(TERMREFDIRECT_TAG, def(label));
    let out_of = |prefix: Vec<u8>| named(TYPEREF_TAG, N_OUT, &prefix);
    let applied = || node(APPLIEDTPT, &[ident_box(), ident_any()].concat());
    let roots: Vec<Vec<u8>> = vec![
        ident_any(),
        applied(),
        applied(),
        wrap(BYNAMETPT, &ident_any()),
        wrap(EXPLICITTPT, &ident_any()),
        bounds(&[any_type()]),
        bounds(&[any_type(), any_type()]),
        bounds(&[any_type(), any_type(), ident_any()]),
        leaf(SHAREDTERM_TAG, root("applied")),
        leaf(SHAREDTERM_TAG, root("shared")),
        leaf(SHAREDTERM_TAG, root("shared cycle")),
        any_type(),
        leaf(SHAREDTYPE_TAG, root("direct")),
        named(SELECTTPT, N_OUT, &any_type()),
        wrap(SINGLETONTPT, &leaf(TERMREFPKG_TAG, N_P)),
        node(
            APPLIEDTPT,
            &[ident_any(), named(SELECTTPT, N_OUT, &any_type())].concat(),
        ),
        out_of(term("Holder.x")),
        out_of(term("Holder.y")),
        out_of(term("Holder.v")),
        out_of(term("Holder.f.a")),
        out_of(term("Holder.f")),
        out_of(term("Holder.s")),
    ];
    assert_eq!(roots.len(), ROOTS.len());
    let holder_payload = roots.concat();
    let header = {
        let mut header = vec![APPLY_TAG];
        header.extend(nat(u32::try_from(holder_payload.len()).unwrap()));
        header
    };
    let mut ast = package;
    let base = u32::try_from(ast.len() + header.len()).unwrap();
    ast.extend(header);
    let mut found = HashMap::new();
    let mut next = base;
    for (label, tree) in ROOTS.iter().zip(&roots) {
        found.insert(*label, next);
        next += u32::try_from(tree.len()).unwrap();
    }
    ast.extend(holder_payload);
    (ast, found)
}

impl Unit {
    fn new() -> Self {
        // Addresses appear inside the trees, so the layout is iterated until
        // the addresses it produces are the ones it used.
        let mut at: HashMap<&'static str, u32> = HashMap::new();
        for _ in 0..6 {
            let (ast, roots) = assemble(&at);
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
            assert_eq!(definitions.len(), DEFINITIONS.len());
            let mut next: HashMap<&'static str, u32> =
                DEFINITIONS.iter().copied().zip(definitions).collect();
            next.extend(roots);
            if next == at {
                return Self { bytes, at };
            }
            at = next;
        }
        panic!("the layout did not settle");
    }
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

/// The ids the next type allocation would get.
fn next_type(session: &mut Session) -> u32 {
    session.store.types.alloc(Type::NoType).index()
}

/// The symbol entered for each definition label.
fn symbols_of(
    unpickler: &TastyUnpickler<'_, '_, '_>,
    unit: &Unit,
) -> HashMap<&'static str, dotty_core::ids::SymbolId> {
    DEFINITIONS
        .iter()
        .map(|label| (*label, unpickler.index().symbol_at(unit.at(label)).unwrap()))
        .collect()
}

fn info(session: &Session, symbol: dotty_core::ids::SymbolId) -> SymbolInfo {
    session.store.symbols.get(symbol).info
}

fn complete_info(session: &Session, symbol: dotty_core::ids::SymbolId) -> TypeId {
    match info(session, symbol) {
        SymbolInfo::Complete(ty) => ty,
        other => panic!("not complete: {other:?}"),
    }
}

// Type-tree projection

#[test]
fn an_identifier_type_tree_projects_exactly_its_embedded_type() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let projected = unpickler.unpickle_type_tree_type(unit.at("ident")).unwrap();
    // The embedded type is the tree's last child: two bytes past its name.
    let embedded = unpickler.index().type_at(unit.at("ident") + 2).unwrap();
    assert_eq!(projected, embedded);
    assert_eq!(
        unpickler.index().type_tree_type_at(unit.at("ident")),
        Some(projected)
    );
    // The tree address is not a type-node address.
    assert_eq!(unpickler.index().type_at(unit.at("ident")), None);
}

#[test]
fn an_applied_type_tree_keeps_its_parts_in_order_and_is_owned_by_its_address() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let first = unpickler
        .unpickle_type_tree_type(unit.at("applied"))
        .unwrap();
    let again = unpickler
        .unpickle_type_tree_type(unit.at("applied"))
        .unwrap();
    let trees = unpickler.index().type_tree_count();
    let second = unpickler
        .unpickle_type_tree_type(unit.at("applied again"))
        .unwrap();
    assert_eq!(first, again);
    // Equal structure at another address is another type: no interning.
    assert_ne!(first, second);
    assert!(unpickler.index().type_tree_count() > trees);
    drop(unpickler);

    let Type::Applied { tycon, args } = session.store.types.get(first) else {
        panic!("not applied");
    };
    assert_eq!(args.len(), 1);
    assert!(matches!(
        session.store.types.get(*tycon),
        Type::TypeRef { .. }
    ));
}

#[test]
fn projecting_the_same_tree_again_allocates_nothing() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let first = unpickler
        .unpickle_type_tree_type(unit.at("applied"))
        .unwrap();
    drop(unpickler);
    let after_first = next_type(&mut session);

    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    unpickler
        .unpickle_type_tree_type(unit.at("applied"))
        .unwrap();
    assert_eq!(
        unpickler.unpickle_type_tree_type(unit.at("applied")),
        Ok(first)
    );
    drop(unpickler);
    assert_eq!(next_type(&mut session), after_first);
}

#[test]
fn a_by_name_tree_is_a_by_name_type_and_an_explicit_tree_is_transparent() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let by_name = unpickler
        .unpickle_type_tree_type(unit.at("by-name"))
        .unwrap();
    let explicit = unpickler
        .unpickle_type_tree_type(unit.at("explicit"))
        .unwrap();
    // The explicit tree's child is the `IDENTtpt` right after the tag.
    let child = unpickler
        .index()
        .type_tree_type_at(unit.at("explicit") + 1)
        .unwrap();
    assert_eq!(explicit, child);
    assert_eq!(
        unpickler.index().type_tree_type_at(unit.at("explicit")),
        Some(child)
    );
    drop(unpickler);

    let Type::ByName { result } = session.store.types.get(by_name) else {
        panic!("not by-name");
    };
    assert!(matches!(
        session.store.types.get(*result),
        Type::TypeRef { .. }
    ));
}

#[test]
fn bounds_trees_follow_the_one_two_and_three_child_shapes() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let one = unpickler
        .unpickle_type_tree_type(unit.at("bounds1"))
        .unwrap();
    let two = unpickler
        .unpickle_type_tree_type(unit.at("bounds2"))
        .unwrap();
    let three = unpickler
        .unpickle_type_tree_type(unit.at("bounds3"))
        .unwrap();
    drop(unpickler);

    // One child: `lo eq hi`, so an alias, however equal the two would be.
    assert!(matches!(
        session.store.types.get(one),
        Type::AliasingBounds { .. }
    ));
    // Two: bounds, even when both are the same type.
    assert!(matches!(session.store.types.get(two), Type::Bounds { .. }));
    // Three: the alias' own type, not a bounds object.
    assert!(matches!(
        session.store.types.get(three),
        Type::TypeRef { .. }
    ));
}

#[test]
fn a_shared_term_link_projects_its_target_and_owns_nothing() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let target = unpickler
        .unpickle_type_tree_type(unit.at("applied"))
        .unwrap();
    let count = unpickler.index().type_tree_count();
    assert_eq!(
        unpickler.unpickle_type_tree_type(unit.at("shared")),
        Ok(target)
    );
    assert_eq!(
        unpickler.unpickle_type_tree_type(unit.at("shared chain")),
        Ok(target)
    );
    assert_eq!(unpickler.index().type_tree_count(), count);
    assert_eq!(unpickler.index().type_tree_type_at(unit.at("shared")), None);
}

#[test]
fn a_shared_term_link_first_builds_the_target_once() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let through = unpickler
        .unpickle_type_tree_type(unit.at("shared"))
        .unwrap();
    assert_eq!(
        unpickler.unpickle_type_tree_type(unit.at("applied")),
        Ok(through)
    );
}

#[test]
fn a_shared_term_cycle_is_an_invalid_reference_not_a_loop() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    assert_eq!(
        unpickler.unpickle_type_tree_type(unit.at("shared cycle")),
        Err(UnpickleError::InvalidReferenceTarget {
            from: unit.at("shared cycle"),
            to: unit.at("shared cycle"),
        })
    );
}

#[test]
fn a_semantic_type_node_in_tree_position_is_that_type() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let direct = unpickler
        .unpickle_type_tree_type(unit.at("direct"))
        .unwrap();
    assert_eq!(unpickler.index().type_at(unit.at("direct")), Some(direct));
    // A `SHAREDtype` there is the type it names.
    assert_eq!(
        unpickler.unpickle_type_tree_type(unit.at("shared type")),
        Ok(direct)
    );
}

#[test]
fn a_tree_form_that_is_not_projected_yet_is_a_typed_error() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    assert_eq!(
        unpickler.unpickle_type_tree_type(unit.at("select")),
        Err(UnpickleError::UnsupportedTypeTree {
            address: unit.at("select"),
            tag: SELECTTPT
        })
    );
    assert_eq!(
        unpickler.unpickle_type_tree_type(unit.at("singleton")),
        Err(UnpickleError::UnsupportedTypeTree {
            address: unit.at("singleton"),
            tag: SINGLETONTPT
        })
    );
}

#[test]
fn a_failing_argument_rolls_back_the_tree_types_built_before_it() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();

    let mut untouched = Session::new();
    drop(entered(&file, &mut untouched));
    let expected = next_type(&mut untouched);

    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let types = unpickler.index().type_count();
    let trees = unpickler.index().type_tree_count();
    for _ in 0..2 {
        let result = unpickler.unpickle_type_tree_type(unit.at("bad applied"));
        assert!(
            matches!(
                result,
                Err(UnpickleError::UnsupportedTypeTree { tag: SELECTTPT, .. })
            ),
            "{result:?}"
        );
        assert_eq!(unpickler.index().type_count(), types);
        assert_eq!(unpickler.index().type_tree_count(), trees);
    }
    drop(unpickler);
    assert_eq!(next_type(&mut session), expected);
}

// Symbol completion

#[test]
fn a_val_and_a_parameter_take_their_declared_type_tree() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let symbols = symbols_of(&unpickler, &unit);

    let x = unpickler.complete_symbol(unit.at("Holder.x")).unwrap();
    let a = unpickler.complete_symbol(unit.at("Holder.f.a")).unwrap();
    // `IDENTtpt`: the embedded type, exactly.
    let embedded = unpickler.index().type_at(unit.at("Holder.x") + 5).unwrap();
    assert_eq!(x, embedded);
    drop(unpickler);

    assert_eq!(complete_info(&session, symbols["Holder.x"]), x);
    // The by-name parameter's info stays by-name.
    assert!(matches!(session.store.types.get(a), Type::ByName { .. }));
    assert_eq!(complete_info(&session, symbols["Holder.f.a"]), a);
}

#[test]
fn entered_symbols_start_missing_and_completion_makes_no_symbol() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let symbols = symbols_of(&unpickler, &unit);
    let count = unpickler.index().symbol_count();
    unpickler.complete_symbol(unit.at("Holder.x")).unwrap();
    assert_eq!(unpickler.index().symbol_count(), count);
    // The same symbol, completed in place.
    assert_eq!(
        symbols["Holder.x"],
        unpickler.index().symbol_at(unit.at("Holder.x")).unwrap()
    );
    drop(unpickler);

    for (label, symbol) in &symbols {
        if *label != "Holder.x" {
            assert_eq!(info(&session, *symbol), SymbolInfo::Missing, "{label}");
        }
    }
}

#[test]
fn a_type_parameter_and_a_type_definition_get_bounds() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let parameter = unpickler.complete_symbol(unit.at("Holder.A")).unwrap();
    let alias = unpickler.complete_symbol(unit.at("Holder.Alias")).unwrap();
    let abstract_ = unpickler.complete_symbol(unit.at("Holder.Abs")).unwrap();
    let same = unpickler.complete_symbol(unit.at("Holder.Same")).unwrap();
    // The right-hand side of `type Alias = p` still projects to `p`.
    let rhs = unpickler
        .index()
        .type_tree_type_at(unit.at("Holder.Alias") + 3)
        .unwrap();
    // The bounds tree itself is the projection, reused as the info.
    let abstract_tree = unpickler
        .index()
        .type_tree_type_at(unit.at("Holder.Abs") + 3)
        .unwrap();
    let same_tree = unpickler
        .index()
        .type_tree_type_at(unit.at("Holder.Same") + 3)
        .unwrap();
    drop(unpickler);

    assert!(matches!(
        session.store.types.get(parameter),
        Type::Bounds { .. }
    ));
    // `toBounds` of an ordinary type: a fresh alias, the type itself untouched.
    let Type::AliasingBounds { alias: aliased } = session.store.types.get(alias) else {
        panic!("not alias bounds");
    };
    assert_eq!(*aliased, rhs);
    assert!(matches!(session.store.types.get(rhs), Type::TypeRef { .. }));
    assert_eq!(abstract_, abstract_tree);
    assert!(matches!(
        session.store.types.get(abstract_),
        Type::Bounds { .. }
    ));
    // A one-child bounds tree is already an alias: not wrapped a second time.
    assert_eq!(same, same_tree);
    assert!(matches!(
        session.store.types.get(same),
        Type::AliasingBounds { .. }
    ));
}

#[test]
fn an_opaque_alias_a_class_a_method_and_a_package_are_explicit_deferrals() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let symbols = symbols_of(&unpickler, &unit);

    assert_eq!(
        unpickler.complete_symbol(unit.at("Holder.Op")),
        Err(UnpickleError::OpaqueAliasDeferred {
            address: unit.at("Holder.Op")
        })
    );
    for (label, kind) in [
        ("Box", SymbolKind::Class),
        ("Holder", SymbolKind::Class),
        ("Holder.f", SymbolKind::Method),
    ] {
        assert_eq!(
            unpickler.complete_symbol(unit.at(label)),
            Err(UnpickleError::UnsupportedSymbolCompletion {
                address: unit.at(label),
                kind
            }),
            "{label}"
        );
    }
    assert_eq!(
        unpickler.complete_symbol(0),
        Err(UnpickleError::UnsupportedSymbolCompletion {
            address: 0,
            kind: SymbolKind::Package
        })
    );
    assert_eq!(
        unpickler.complete_symbol(1),
        Err(UnpickleError::MissingEnteredSymbol { address: 1 })
    );
    drop(unpickler);
    for label in ["Holder.Op", "Box", "Holder", "Holder.f"] {
        // No empty class info or placeholder is written.
        assert_eq!(
            info(&session, symbols[label]),
            SymbolInfo::Missing,
            "{label}"
        );
    }
}

#[test]
fn completing_twice_returns_the_existing_type_and_allocates_nothing() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let first = unpickler.complete_symbol(unit.at("Holder.Alias")).unwrap();
    drop(unpickler);
    let after = next_type(&mut session);

    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    unpickler.complete_symbol(unit.at("Holder.Alias")).unwrap();
    assert_eq!(
        unpickler.complete_symbol(unit.at("Holder.Alias")),
        Ok(first)
    );
    drop(unpickler);
    assert_eq!(next_type(&mut session), after);
}

#[test]
fn an_unsupported_definition_leaves_an_earlier_completion_alone() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let symbols = symbols_of(&unpickler, &unit);

    let x = unpickler.complete_symbol(unit.at("Holder.x")).unwrap();
    assert_eq!(
        unpickler.complete_symbol(unit.at("Holder.Bad")),
        Err(UnpickleError::UnsupportedTypeTree {
            address: unit.at("Holder.Bad") + 3,
            tag: SELECTTPT
        })
    );
    drop(unpickler);

    assert_eq!(complete_info(&session, symbols["Holder.x"]), x);
    assert_eq!(info(&session, symbols["Holder.Bad"]), SymbolInfo::Missing);
}

#[test]
fn a_failed_batch_restores_every_symbol_info_it_had_set_and_can_be_retried() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();

    let mut untouched = Session::new();
    drop(entered(&file, &mut untouched));
    let expected = next_type(&mut untouched);

    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let symbols = symbols_of(&unpickler, &unit);
    let types = unpickler.index().type_count();
    let trees = unpickler.index().type_tree_count();
    // `x` and `Alias` complete inside the batch; `Bad` fails after them.
    let batch = [
        unit.at("Holder.x"),
        unit.at("Holder.Alias"),
        unit.at("Holder.Bad"),
    ];
    for _ in 0..2 {
        assert!(matches!(
            unpickler.complete_symbols(&batch),
            Err(UnpickleError::UnsupportedTypeTree { tag: SELECTTPT, .. })
        ));
        assert_eq!(unpickler.index().type_count(), types);
        assert_eq!(unpickler.index().type_tree_count(), trees);
    }
    drop(unpickler);
    for label in ["Holder.x", "Holder.Alias", "Holder.Bad"] {
        assert_eq!(
            info(&session, symbols[label]),
            SymbolInfo::Missing,
            "{label}"
        );
    }
    assert_eq!(next_type(&mut session), expected);

    // The same unpickler, retried without the failing one.
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    assert!(unpickler.complete_symbols(&batch).is_err());
    let done = unpickler
        .complete_symbols(&[unit.at("Holder.x"), unit.at("Holder.Alias")])
        .unwrap();
    assert_eq!(done.len(), 2);
}

#[test]
fn a_symbol_completed_by_an_earlier_call_survives_a_later_failed_batch() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let symbols = symbols_of(&unpickler, &unit);

    let x = unpickler.complete_symbol(unit.at("Holder.x")).unwrap();
    // The batch completes `x` again (nothing to do), `Alias`, then fails.
    assert!(
        unpickler
            .complete_symbols(&[
                unit.at("Holder.x"),
                unit.at("Holder.Alias"),
                unit.at("Holder.Bad")
            ])
            .is_err()
    );
    drop(unpickler);

    assert_eq!(complete_info(&session, symbols["Holder.x"]), x);
    assert_eq!(info(&session, symbols["Holder.Alias"]), SymbolInfo::Missing);
}

// Member lookup through a completed stable term

// Member lookup through a completed stable term

#[test]
fn a_stable_val_selects_a_member_only_once_it_is_completed() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let out = unpickler.index().symbol_at(unit.at("Box.Out")).unwrap();

    // `x.Out`, with `x` still `Missing`: no declared type to search.
    let before = unpickler.unpickle_type(unit.at("x.Out"));
    assert!(
        matches!(
            before,
            Err(UnpickleError::UnsupportedResolutionPrefix { .. })
        ),
        "{before:?}"
    );
    unpickler.complete_symbol(unit.at("Holder.x")).unwrap();
    let id = unpickler.unpickle_type(unit.at("x.Out")).unwrap();
    drop(unpickler);

    assert_eq!(session.store.types.get(id).reference_symbol(), Some(out));
}

#[test]
fn a_generic_stable_val_finds_the_member_through_its_constructor_and_keeps_the_application() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let out = unpickler.index().symbol_at(unit.at("Box.Out")).unwrap();
    let y = unpickler.complete_symbol(unit.at("Holder.y")).unwrap();

    let id = unpickler.unpickle_type(unit.at("y.Out")).unwrap();
    drop(unpickler);

    assert_eq!(session.store.types.get(id).reference_symbol(), Some(out));
    // The application is the declared type still; only its constructor was
    // used to find the scope.
    assert!(matches!(session.store.types.get(y), Type::Applied { .. }));
}

#[test]
fn a_mutable_a_by_name_and_a_method_prefix_are_not_widened() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    for label in ["Holder.v", "Holder.f.a"] {
        unpickler.complete_symbol(unit.at(label)).unwrap();
    }

    // `var v: Box`: not a stable path, completed or not (the prefix is
    // refused as illegal only for an owner-space reference).
    let mutable = unpickler.unpickle_type(unit.at("v.Out"));
    assert!(
        matches!(
            mutable,
            Err(UnpickleError::UnsupportedResolutionPrefix { .. })
        ),
        "{mutable:?}"
    );
    // `a: => Box`: completed, but not a path.
    let by_name = unpickler.unpickle_type(unit.at("a.Out"));
    assert!(
        matches!(
            by_name,
            Err(UnpickleError::UnsupportedResolutionPrefix { .. })
        ),
        "{by_name:?}"
    );
    // A method is not a stable prefix.
    let method = unpickler.unpickle_type(unit.at("f.Out"));
    assert!(
        matches!(
            method,
            Err(UnpickleError::UnsupportedResolutionPrefix { .. })
        ),
        "{method:?}"
    );
}

#[test]
fn a_completed_info_that_names_its_own_term_is_a_bounded_unsupported_prefix() {
    let unit = Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    // `val s: s.type`.
    let s = unpickler.complete_symbol(unit.at("Holder.s")).unwrap();
    let result = unpickler.unpickle_type(unit.at("s.Out"));
    drop(unpickler);

    assert!(matches!(session.store.types.get(s), Type::TermRef { .. }));
    assert!(
        matches!(
            result,
            Err(UnpickleError::UnsupportedResolutionPrefix { .. })
        ),
        "{result:?}"
    );
}

// Real Scala 3.9.0 output: `fixtures/semantic/Completion.scala`.

const HOLDER: &[u8] = include_bytes!("fixtures/semantic/CompletionHolder.tasty");

/// Every `VALDEF`, `TYPEDEF` and `DEFDEF` of the unit by name.
fn definitions_by_name(file: &TastyFile<'_>) -> HashMap<String, u32> {
    use dotty_tasty::tasty::{DefinitionBody, StructuredNode};
    let index = file.ast_address_index().unwrap();
    let mut found = HashMap::new();
    for node in index.iter_nodes() {
        let Some(raw) = index.get(u32::try_from(node.offset).unwrap()) else {
            continue;
        };
        let name = match raw.decode_structured() {
            Ok(StructuredNode::ValDef(DefinitionBody::ValDef { name, .. }))
            | Ok(StructuredNode::TypeDef(DefinitionBody::TypeDef { name, .. })) => name,
            Ok(StructuredNode::DefDef(body)) => body.name,
            _ => continue,
        };
        let Some(text) = file.names().get_utf8(name).map(str::to_owned) else {
            continue;
        };
        found.insert(text, u32::try_from(node.offset).unwrap());
    }
    found
}

fn stub_classes(session: &mut Session, packages: &mut Packages, path: &[&str], names: &[&str]) {
    use dotty_core::names::Name;
    use dotty_core::{Symbol, SymbolFlags, Visibility};
    let package = packages
        .enter(&mut session.store, SymbolOrigin::Synthetic, path)
        .pop()
        .unwrap();
    for class in names {
        let name = Name::new(session.store.names.intern(class), Namespace::Type);
        let symbol = session.store.symbols.alloc(Symbol {
            name,
            owner: Some(package.symbol),
            kind: SymbolKind::Class,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        });
        session
            .store
            .scopes
            .get_mut(package.scope)
            .enter(name, symbol);
    }
}

#[test]
fn real_simple_definitions_complete_and_methods_and_classes_stay_missing() {
    let file = TastyFile::parse_scala_3_9(HOLDER).unwrap();
    let defs = definitions_by_name(&file);
    let mut session = Session::new();
    let mut packages = Packages::new();
    stub_classes(
        &mut session,
        &mut packages,
        &["scala"],
        &["Int", "Any", "Nothing", "Unit"],
    );
    stub_classes(&mut session, &mut packages, &["java", "lang"], &["String"]);
    let mut unpickler =
        TastyUnpickler::with_packages(&file, &mut session.store, session.definitions, packages);
    unpickler.enter_symbols().unwrap();
    let symbol_count = unpickler.index().symbol_count();
    let symbols: HashMap<String, dotty_core::ids::SymbolId> = defs
        .iter()
        .map(|(name, at)| (name.clone(), unpickler.index().symbol_at(*at).unwrap()))
        .collect();

    let plain = unpickler.complete_symbol(defs["plain"]).unwrap();
    let alias = unpickler.complete_symbol(defs["Alias"]).unwrap();
    let abstract_ = unpickler.complete_symbol(defs["Abstract"]).unwrap();
    let mutable = unpickler.complete_symbol(defs["mutable"]).unwrap();
    // Methods, constructors and classes are later milestones.
    for name in ["use", "byName", "CompletionHolder", "CBox"] {
        let Some(at) = defs.get(name) else { continue };
        assert!(
            matches!(
                unpickler.complete_symbol(*at),
                Err(UnpickleError::UnsupportedSymbolCompletion { .. })
            ),
            "{name}"
        );
    }
    // A type tree is not a symbol.
    assert_eq!(unpickler.index().symbol_count(), symbol_count);
    drop(unpickler);

    assert!(matches!(
        session.store.types.get(plain),
        Type::TypeRef { .. }
    ));
    assert!(matches!(
        session.store.types.get(alias),
        Type::AliasingBounds { .. }
    ));
    assert!(matches!(
        session.store.types.get(abstract_),
        Type::Bounds { .. }
    ));
    assert!(matches!(
        session.store.types.get(mutable),
        Type::TypeRef { .. }
    ));
    for name in ["use", "byName", "CompletionHolder", "CBox"] {
        if let Some(symbol) = symbols.get(name) {
            assert_eq!(info(&session, *symbol), SymbolInfo::Missing, "{name}");
        }
    }
    assert_eq!(complete_info(&session, symbols["plain"]), plain);
}

#[test]
fn a_real_definition_with_an_unsupported_tree_does_not_stop_the_others() {
    // `type Selected = stable.Out` is a `SELECTtpt`.
    let file = TastyFile::parse_scala_3_9(HOLDER).unwrap();
    let defs = definitions_by_name(&file);
    let mut session = Session::new();
    let mut packages = Packages::new();
    stub_classes(
        &mut session,
        &mut packages,
        &["scala"],
        &["Int", "Any", "Nothing"],
    );
    stub_classes(&mut session, &mut packages, &["java", "lang"], &["String"]);
    let mut unpickler =
        TastyUnpickler::with_packages(&file, &mut session.store, session.definitions, packages);
    unpickler.enter_symbols().unwrap();

    let result = unpickler.complete_symbol(defs["Selected"]);
    assert!(
        matches!(
            result,
            Err(UnpickleError::UnsupportedTypeTree { tag: 113, .. })
        ),
        "{result:?}"
    );
    assert!(unpickler.complete_symbol(defs["plain"]).is_ok());
}
