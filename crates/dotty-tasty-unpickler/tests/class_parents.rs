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

const NAMES: [&str; 9] = ["ASTs", "p", "C", "<init>", "other", "x", "D", "E", "T"];

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
    classes_unit(&[("C", parents.to_vec(), self_def)])
}

/// A class of a synthetic unit: its name, parents and self definition.
type ClassSpec<'a> = (&'a str, Vec<Vec<u8>>, Option<Vec<u8>>);

/// One unit with a class per entry: its name, parents and self definition.
fn classes_unit(classes: &[ClassSpec<'_>]) -> Vec<u8> {
    let classes: Vec<u8> = classes
        .iter()
        .flat_map(|(name, parents, self_def)| {
            let template = node(
                TEMPLATE_TAG,
                &[parents.concat(), self_def.clone().unwrap_or_default()].concat(),
            );
            node(TYPEDEF_TAG, &[nat(n(name)), template].concat())
        })
        .collect();
    let ast = node(
        PACKAGE_TAG,
        &[leaf(TERMREFPKG_TAG, n("p")), classes].concat(),
    );
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
    complete_nth(bytes, 0)
}

/// Completes the `nth` class (in address order) of the unit.
fn complete_nth(bytes: &[u8], nth: usize) -> Completed {
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    packages.enter(&mut store, SymbolOrigin::Synthetic, &["p"]);
    let mut unpickler = TastyUnpickler::with_packages(&file, &mut store, definitions, packages);
    unpickler.enter_symbols().unwrap();
    let index = file.ast_address_index().unwrap();
    let mut classes: Vec<u32> = index
        .iter_nodes()
        .filter(|node| node.tag == TYPEDEF_TAG)
        .map(|node| u32::try_from(node.offset).unwrap())
        .collect();
    classes.sort_unstable();
    let class = classes[nth];
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
fn a_parent_naming_an_address_that_does_not_exist_fails_enter_symbols() {
    // `poison_type()` is a bare `TYPEREFdirect` parent naming an
    // out-of-range address. Milestone 5d2c's broadened pass-1 discovery
    // scans every parent through `discover_identities` (`TypeTree` mode's
    // fallback to `SemanticType` for a bare reference tag), which validates
    // a direct/symbol reference's target address (`reference_target`) — so
    // this is caught eagerly, during `enter_symbols`, rather than deferred
    // to `type_of_parent`/completion the way the pre-5d2c narrow whitelist
    // (which never inspected a `TYPEREFdirect` parent at all) left it.
    let bytes = class_unit(&[ident(), poison_type()], None);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    packages.enter(&mut store, SymbolOrigin::Synthetic, &["p"]);
    let mut unpickler = TastyUnpickler::with_packages(&file, &mut store, definitions, packages);

    assert_eq!(
        unpickler.enter_symbols().map(|_| ()),
        Err(UnpickleError::InvalidReferenceTarget {
            from: 13,
            to: 0x3fff,
        })
    );
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

/// `LAMBDAtpt [T] =>> p`: a lambda whose one parameter is unbounded.
fn lambda_tpt() -> Vec<u8> {
    let bounds = node(
        dotty_tasty::tasty::TYPEBOUNDSTPT_TAG,
        &[p_type(), p_type()].concat(),
    );
    let parameter = node(
        dotty_tasty::tasty::TYPEPARAM_TAG,
        &[nat(n("T")), bounds].concat(),
    );
    node(
        dotty_tasty::tasty::LAMBDATPT_TAG,
        &[parameter, ident()].concat(),
    )
}

#[test]
fn a_lambda_shared_by_two_classes_is_refused_for_both() {
    // D's parent is the lambda; E's parent is a link to it.
    let build = |target: u32| {
        classes_unit(&[
            ("D", vec![lambda_tpt()], None),
            ("E", vec![leaf(SHAREDTERM, target)], None),
        ])
    };
    let target = first_parent_address(&build(0));
    let bytes = build(target);

    // The conflict belongs to the lambda's address, as in 5c: neither owner
    // gets a lambda whose parameters it cannot own alone.
    for nth in [0, 1] {
        let completed = complete_nth(&bytes, nth);
        assert!(
            matches!(
                completed.error(),
                UnpickleError::SharedLambdaOwnerConflict { .. }
            ),
            "class {nth}: {:?}",
            completed.result
        );
        assert_eq!(completed.class_state, SymbolInfo::Missing);
    }
}

#[test]
fn a_term_that_is_not_a_constructor_call_is_an_unsupported_parent() {
    // `IDENT p p`: a term identifier where a parent is expected.
    let ident_term = [&[110u8][..], &nat(n("p")), &p_type()].concat();
    let completed = complete(&class_unit(&[ident_term], None));

    assert!(matches!(
        completed.error(),
        UnpickleError::UnsupportedParentTree { tag: 110, .. }
    ));
    assert_eq!(completed.class_state, SymbolInfo::Missing);
}

#[test]
fn a_self_definition_with_an_invalid_name_reference_is_a_typed_error() {
    // The name index is far outside the name table.
    let self_def = [&[SELFDEF][..], &nat(0x3fff), &ident()].concat();
    let completed = complete(&class_unit(&[ident()], Some(self_def)));

    assert!(
        matches!(
            completed.error(),
            UnpickleError::InvalidNameReference { .. } | UnpickleError::UnsupportedName { .. }
        ),
        "{:?}",
        completed.result
    );
    assert_eq!(completed.class_state, SymbolInfo::Missing);
}

#[test]
fn a_lambda_in_a_skipped_type_argument_does_not_take_ownership_from_a_read_parent() {
    // D's parent is the lambda. E's parent is `TypeApply(New(p[p]), <link to
    // the lambda>)`: the constructor type is already applied, so the argument is
    // never read, and must not claim or conflict with D's lambda.
    let build = |target: u32| {
        let call = node(
            TYPEAPPLY,
            &[
                constructor(applied_tpt(ident(), &[ident()])),
                leaf(SHAREDTERM, target),
            ]
            .concat(),
        );
        classes_unit(&[
            ("D", vec![lambda_tpt()], None),
            ("E", vec![node(APPLY, &call)], None),
        ])
    };
    let target = first_parent_address(&build(0));
    let bytes = build(target);

    let read = complete_nth(&bytes, 0);
    assert!(read.result.is_ok(), "{:?}", read.result);
    let skipping = complete_nth(&bytes, 1);
    assert!(skipping.result.is_ok(), "{:?}", skipping.result);
}
