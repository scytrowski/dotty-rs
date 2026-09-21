//! Milestone 5d1: template parent projection (`type_of_parent`), on synthetic
//! units, so the wrapper shapes and what is deliberately not read can be
//! written exactly. `package p; class C extends <parents>`, where every type
//! a parent mentions is the package `p` itself (a type that always resolves).
use dotty_core::ids::TypeId;
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolInfo, SymbolKind, SymbolOrigin};
use dotty_core::types::Type;
use dotty_core::{Definitions, Packages};
use dotty_tasty::tasty::{
    APPLIEDTPT_TAG, Header, NameTable, PACKAGE_TAG, RawName, Section, SectionTable, TEMPLATE_TAG,
    TERMREFDIRECT_TAG, TERMREFPKG_TAG, TYPEDEF_TAG, TYPEREFDIRECT_TAG, TYPEREFPKG_TAG, TastyFile,
};
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

const NAMES: [&str; 6] = ["ASTs", "p", "C", "<init>", "other", "x"];

const IDENTTPT: u8 = 111;
const APPLY: u8 = 136;
const TYPEAPPLY: u8 = 137;
const BLOCK: u8 = 140;
const SELECTIN: u8 = 176;
const NEW: u8 = 95;
const SHAREDTERM: u8 = 60;
const SELFDEF: u8 = 118;

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

/// The type every parent here names: the package `p`.
fn p_type() -> Vec<u8> {
    leaf(TYPEREFPKG_TAG, n("p"))
}

/// `IDENTtpt p`: a type tree whose type is `p`.
fn ident() -> Vec<u8> {
    [&[IDENTTPT][..], &nat(n("p")), &p_type()].concat()
}

/// A term whose address does not exist, so reading it is an error.
fn poison_term() -> Vec<u8> {
    leaf(TERMREFDIRECT_TAG, 0x3fff)
}

/// A type whose address does not exist, so reading it is an error.
fn poison_type() -> Vec<u8> {
    leaf(TYPEREFDIRECT_TAG, 0x3fff)
}

/// `SELECTin <init> (NEW tpt) owner`.
fn constructor(tpt: Vec<u8>) -> Vec<u8> {
    node(
        SELECTIN,
        &[nat(n("<init>")), [&[NEW][..], &tpt].concat(), p_type()].concat(),
    )
}

fn applied_tpt(tycon: Vec<u8>, args: &[Vec<u8>]) -> Vec<u8> {
    node(APPLIEDTPT_TAG, &[tycon, args.concat()].concat())
}

fn class_unit(parents: &[Vec<u8>], self_def: Option<Vec<u8>>) -> Vec<u8> {
    let template = node(
        TEMPLATE_TAG,
        &[parents.concat(), self_def.unwrap_or_default()].concat(),
    );
    let class = node(TYPEDEF_TAG, &[nat(n("C")), template].concat());
    let ast = node(PACKAGE_TAG, &[leaf(TERMREFPKG_TAG, n("p")), class].concat());
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
        SectionTable::from_sections(vec![Section::new(0, &ast)]),
    )
    .unwrap()
    .encode()
    .unwrap()
}

/// The unit's file, the class definition's address and the session that
/// completed it (or the error).
struct Completed {
    store: SemanticStore,
    result: Result<TypeId, UnpickleError>,
    class_state: SymbolInfo,
}

fn complete(bytes: &[u8]) -> Completed {
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    packages.enter(&mut store, SymbolOrigin::Synthetic, &["p"]);
    let mut unpickler = TastyUnpickler::with_packages(&file, &mut store, definitions, packages);
    unpickler.enter_symbols().unwrap();
    let index = file.ast_address_index().unwrap();
    let class = index
        .iter_nodes()
        .find(|node| node.tag == TYPEDEF_TAG)
        .map(|node| u32::try_from(node.offset).unwrap())
        .unwrap();
    assert_eq!(
        unpickler.symbol_state_at(class).unwrap().0,
        SymbolKind::Class
    );
    let result = unpickler.complete_symbol(class);
    let class_state = unpickler.symbol_state_at(class).unwrap().1;
    drop(unpickler);
    Completed {
        store,
        result,
        class_state,
    }
}

impl Completed {
    fn parents(&self) -> Vec<TypeId> {
        match self
            .store
            .types
            .get(self.result.clone().expect("completes"))
        {
            Type::ClassInfo(info) => info.parents.clone(),
            other => panic!("not a ClassInfo: {other:?}"),
        }
    }

    fn error(&self) -> &UnpickleError {
        self.result.as_ref().expect_err("fails")
    }
}

/// The address of the first parent tree of the class in `bytes`.
fn first_parent_address(bytes: &[u8]) -> u32 {
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let index = file.ast_address_index().unwrap();
    let template = index
        .iter_nodes()
        .find(|node| node.tag == TEMPLATE_TAG)
        .unwrap()
        .offset;
    index
        .iter_tree_edges()
        .find(|edge| edge.parent.offset == template)
        .map(|edge| u32::try_from(edge.child.offset).unwrap())
        .unwrap()
}

#[test]
fn a_constructor_call_parent_is_its_new_type_and_reads_no_arguments() {
    // `extends p(<poison>)`: the argument cannot be evaluated, so it is not.
    let call = node(APPLY, &[constructor(ident()), poison_term()].concat());
    let completed = complete(&class_unit(&[call], None));

    let parents = completed.parents();
    assert_eq!(parents.len(), 1);
    assert!(matches!(
        completed.store.types.get(parents[0]),
        Type::TypeRef { .. }
    ));
}

#[test]
fn curried_constructor_calls_and_blocks_are_read_through_their_function() {
    let inner = node(APPLY, &[constructor(ident()), poison_term()].concat());
    let curried = node(APPLY, &[inner, poison_term()].concat());
    let block = node(BLOCK, &[curried.clone(), poison_term()].concat());
    let completed = complete(&class_unit(&[curried, block], None));

    let parents = completed.parents();
    assert_eq!(parents.len(), 2);
    for parent in parents {
        assert!(matches!(
            completed.store.types.get(parent),
            Type::TypeRef { .. }
        ));
    }
}

#[test]
fn a_type_application_over_an_already_applied_new_type_skips_its_arguments() {
    // `New(p[p])` then `TypeApply(..., <poison>)`: the compiler repeats the
    // arguments, and upstream reads them only when the type is not applied.
    let new = applied_tpt(ident(), &[ident()]);
    let call = node(
        APPLY,
        &[node(TYPEAPPLY, &[constructor(new), poison_type()].concat())].concat(),
    );
    let completed = complete(&class_unit(&[call], None));

    let parents = completed.parents();
    assert!(matches!(
        completed.store.types.get(parents[0]),
        Type::Applied { args, .. } if args.len() == 1
    ));
}

#[test]
fn a_type_application_over_a_bare_type_applies_its_arguments() {
    let call = node(
        TYPEAPPLY,
        &[constructor(ident()), ident(), ident()].concat(),
    );
    let completed = complete(&class_unit(&[call], None));

    let parents = completed.parents();
    assert!(matches!(
        completed.store.types.get(parents[0]),
        Type::Applied { args, .. } if args.len() == 2
    ));
}

#[test]
fn a_shared_term_parent_is_the_tree_it_names() {
    let plain = ident();
    let build = |target: u32| class_unit(&[plain.clone(), leaf(SHAREDTERM, target)], None);
    let bytes = build(0);
    let target = first_parent_address(&bytes);
    let completed = complete(&build(target));

    let parents = completed.parents();
    assert_eq!(parents.len(), 2);
    assert_eq!(parents[0], parents[1]);
}

#[test]
fn a_constructor_selection_of_anything_but_init_is_malformed() {
    let selection = node(
        SELECTIN,
        &[nat(n("other")), [&[NEW][..], &ident()].concat(), p_type()].concat(),
    );
    let completed = complete(&class_unit(&[node(APPLY, &selection)], None));

    assert!(matches!(
        completed.error(),
        UnpickleError::MalformedParentTree { .. }
    ));
    assert_eq!(completed.class_state, SymbolInfo::Missing);
}

#[test]
fn a_constructor_selection_that_is_not_on_new_is_malformed() {
    let selection = node(SELECTIN, &[nat(n("<init>")), ident(), p_type()].concat());
    let completed = complete(&class_unit(&[node(APPLY, &selection)], None));

    assert!(matches!(
        completed.error(),
        UnpickleError::MalformedParentTree { .. }
    ));
}

#[test]
fn a_parent_that_cannot_be_projected_leaves_the_class_missing() {
    let completed = complete(&class_unit(&[ident(), poison_type()], None));

    assert!(completed.result.is_err());
    assert_eq!(completed.class_state, SymbolInfo::Missing);
}

#[test]
fn a_self_definition_is_the_self_type() {
    let self_def = [&[SELFDEF][..], &nat(n("x")), &ident()].concat();
    let completed = complete(&class_unit(&[ident()], Some(self_def)));

    let ty = completed.result.clone().unwrap();
    let Type::ClassInfo(info) = completed.store.types.get(ty) else {
        panic!("class info")
    };
    assert!(info.self_type.is_some());
}
