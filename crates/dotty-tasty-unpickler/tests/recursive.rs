//! Milestone 4a: recursive types (`RECtype`, `RECthis`) and refinements
//! (`REFINEDtype`).
//!
//! Small synthetic wire files cover malformed shapes, identity and rollback;
//! the real Scala 3.9.0 fixture `tests/fixtures/semantic/RecursiveRefined.scala`
//! is used further down.
use dotty_core::Definitions;
use dotty_core::Packages;
use dotty_core::ids::TypeId;
use dotty_core::names::Namespace;
use dotty_core::names::TypeName;
use dotty_core::store::SemanticStore;
use dotty_core::symbols::SymbolOrigin;
use dotty_core::types::{
    StructuralMemberLookup, TermRefTarget, Type, TypeRefTarget, lookup_structural_member,
};
use dotty_tasty::tasty::TastyFile;
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};
use std::cell::RefCell;
use std::rc::Rc;

const AND: u8 = 165;
const FLEXIBLE: u8 = 193;
const PARAM: u8 = 172;
const RECTYPE: u8 = 100;
const SHARED: u8 = 61;
const TYPEREFPKG: u8 = 65;
/// `IMPORTED`, a one-`Nat` form the type pass does not decode.
const UNSUPPORTED: u8 = 75;

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

fn nat(value: u8) -> u8 {
    assert!(value < 128);
    0x80 | value
}

fn length_node(tag: u8, payload: &[u8]) -> Vec<u8> {
    let mut node = vec![tag, nat(u8::try_from(payload.len()).unwrap())];
    node.extend(payload);
    node
}

/// A file whose ASTs are exactly `ast`, over the names `ASTs`, `p`, `m`.
fn file_with_ast(ast: &[u8]) -> Vec<u8> {
    use dotty_tasty::tasty::{Header, NameTable, RawName, Section, SectionTable};
    let names = NameTable::from_entries(vec![
        RawName::Utf8("ASTs".to_owned()),
        RawName::Utf8("p".to_owned()),
        RawName::Utf8("m".to_owned()),
        // 3: `p` signed, which a refinement name may not be.
        RawName::Signed {
            original: 1,
            result_signature: 2,
            parameter_signatures: vec![],
        },
    ])
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

/// `TYPEREFpkg p`.
fn package_ref() -> Vec<u8> {
    vec![TYPEREFPKG, nat(1)]
}

/// `PARAMtype Length binder_ASTRef paramNum_Nat`.
fn param_type(binder: u8, number: u8) -> Vec<u8> {
    length_node(PARAM, &[nat(binder), nat(number)])
}

/// `RECtype Type`: a tag and its one child tree, no length.
fn rec_type(parent: &[u8]) -> Vec<u8> {
    let mut node = vec![RECTYPE];
    node.extend(parent);
    node
}

/// A file whose only top-level node is a `FLEXIBLEtype` wrapper (a top-level
/// node must be length-prefixed) around the one type `child`, at address 2.
fn file_with(child: &[u8]) -> Vec<u8> {
    file_with_ast(&length_node(FLEXIBLE, child))
}

/// A file whose only top-level node is an `ANDtype` over two children, the
/// first at address 2.
fn file_with_two(first: &[u8], second: &[u8]) -> Vec<u8> {
    let mut payload = first.to_vec();
    payload.extend(second);
    file_with_ast(&length_node(AND, &payload))
}

fn unpickler_for<'a>(
    file: &'a TastyFile<'a>,
    session: &'a mut Session,
) -> TastyUnpickler<'a, 'a, 'a> {
    let mut packages = Packages::new();
    packages.enter(&mut session.store, SymbolOrigin::Synthetic, &["p"]);
    TastyUnpickler::with_packages(file, &mut session.store, session.definitions, packages)
}

const REC_AT: u32 = 2;

fn recursive_parent(store: &SemanticStore, id: TypeId) -> TypeId {
    match store.types.get(id) {
        Type::Recursive { parent } => *parent,
        other => panic!("not a recursive type: {other:?}"),
    }
}

// RECtype

#[test]
fn a_recursive_type_owns_one_id_and_its_parent_is_decoded() {
    let bytes = file_with(&rec_type(&package_ref()));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let id = unpickler.unpickle_type(REC_AT).unwrap();
    assert_eq!(unpickler.index().type_at(REC_AT), Some(id));
    assert_eq!(unpickler.unpickle_type(REC_AT), Ok(id));
    drop(unpickler);

    let parent = recursive_parent(&session.store, id);
    assert!(matches!(
        session.store.types.get(parent),
        Type::TypeRef { .. }
    ));
}

#[test]
fn nested_recursive_types_have_distinct_ids() {
    let bytes = file_with(&rec_type(&rec_type(&package_ref())));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let outer = unpickler.unpickle_type(REC_AT).unwrap();
    let inner = unpickler.index().type_at(REC_AT + 1).unwrap();
    drop(unpickler);

    assert_ne!(outer, inner);
    assert_eq!(recursive_parent(&session.store, outer), inner);
}

#[test]
fn a_shared_link_to_a_recursive_type_returns_its_exact_id() {
    let rec = rec_type(&package_ref());
    let shared_at = 2 + u8::try_from(rec.len()).unwrap();
    let bytes = file_with_two(&rec, &[SHARED, nat(2)]);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let id = unpickler.unpickle_type(REC_AT).unwrap();
    assert_eq!(unpickler.unpickle_type(u32::from(shared_at)), Ok(id));
    drop(unpickler);
    assert!(matches!(
        session.store.types.get(id),
        Type::Recursive { .. }
    ));

    // A link decoded first builds the one recursive type.
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    let linked = unpickler.unpickle_type(u32::from(shared_at)).unwrap();
    assert_eq!(unpickler.unpickle_type(REC_AT), Ok(linked));
}

#[test]
fn a_recursive_parent_that_links_to_itself_is_a_cycle_not_a_panic() {
    // As with a link to a lambda still being built: Dotty registers the type
    // before reading its parent, so the link resolves to the reserved id.
    let bytes = file_with(&rec_type(&[SHARED, nat(2)]));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let id = unpickler.unpickle_type(REC_AT).unwrap();
    drop(unpickler);
    assert_eq!(recursive_parent(&session.store, id), id);
}

#[test]
fn a_parameter_type_cannot_name_a_recursive_binder() {
    // A `PARAMtype` (at 3) naming the `RECtype` (at 2) that contains it: the
    // binder is pending, and binds no parameters.
    let bytes = file_with(&rec_type(&param_type(2, 0)));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    let before = unpickler.index().type_count();

    let result = unpickler.unpickle_type(REC_AT);
    assert!(
        matches!(
            result,
            Err(UnpickleError::InvalidBinderKind { from: 3, .. })
        ),
        "{result:?}"
    );
    assert_eq!(unpickler.index().type_count(), before);
    assert_eq!(unpickler.index().type_at(REC_AT), None);
}

#[test]
fn a_parameter_type_cannot_name_a_completed_recursive_type_either() {
    // The recursive type is decoded first; a later `PARAMtype` naming it is
    // refused by kind (it is neither a lambda, a poly nor a method).
    let rec = rec_type(&package_ref());
    let param_at = 2 + u8::try_from(rec.len()).unwrap();
    let bytes = file_with_two(&rec, &param_type(2, 0));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    unpickler.unpickle_type(REC_AT).unwrap();

    let result = unpickler.unpickle_type(u32::from(param_at));
    assert!(
        matches!(result, Err(UnpickleError::InvalidBinderKind { .. })),
        "{result:?}"
    );
}

#[test]
fn a_failure_after_the_id_is_published_leaves_nothing_behind() {
    let bytes = file_with(&rec_type(&[UNSUPPORTED, nat(1)]));
    let next_id_after = |fail: bool| {
        let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
        let mut session = Session::new();
        let mut unpickler = unpickler_for(&file, &mut session);
        if fail {
            let before = unpickler.index().type_count();
            let result = unpickler.unpickle_type(REC_AT);
            assert!(
                matches!(
                    result,
                    Err(UnpickleError::UnsupportedType {
                        tag: UNSUPPORTED,
                        ..
                    })
                ),
                "{result:?}"
            );
            assert_eq!(unpickler.index().type_count(), before);
            assert_eq!(unpickler.index().type_at(REC_AT), None);
        }
        drop(unpickler);
        session.store.types.alloc(Type::NoType)
    };

    // The reserved slot went back with the rest.
    assert_eq!(next_id_after(true), next_id_after(false));
}

#[test]
fn a_member_lookup_through_a_recursive_type_still_being_decoded_is_an_error_not_a_panic() {
    // `TYPEREF p <link to the RECtype>`: a member selected by name from the
    // binder whose slot is reserved but unfilled.
    let bytes = file_with(&rec_type(&[117, nat(1), SHARED, nat(2)]));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let result = unpickler.unpickle_type(REC_AT);
    assert!(
        matches!(
            result,
            Err(UnpickleError::UnsupportedResolutionPrefix { .. })
        ),
        "{result:?}"
    );
    assert_eq!(unpickler.index().type_at(REC_AT), None);
}

// RECthis

const RECTHIS: u8 = 66;
const LAMBDA: u8 = 170;

/// `RECthis ASTRef`.
fn rec_this(binder: u8) -> Vec<u8> {
    vec![RECTHIS, nat(binder)]
}

fn rec_this_binder(store: &SemanticStore, id: TypeId) -> TypeId {
    match store.types.get(id) {
        Type::RecThis { binder } => *binder,
        other => panic!("not a RecThis: {other:?}"),
    }
}

#[test]
fn a_rec_this_in_the_parent_names_the_pending_recursive_type() {
    // `RECtype` at 2 whose parent is a `RECthis` (at 3) naming it.
    let bytes = file_with(&rec_type(&rec_this(2)));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let id = unpickler.unpickle_type(REC_AT).unwrap();
    let this = unpickler.index().type_at(3).unwrap();
    drop(unpickler);

    // The parent is the canonical `RecThis`, whose binder is the exact id.
    assert_eq!(recursive_parent(&session.store, id), this);
    assert_eq!(rec_this_binder(&session.store, this), id);
}

#[test]
fn a_rec_this_decoded_first_builds_its_recursive_type_once() {
    let bytes = file_with(&rec_type(&rec_this(2)));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    assert_eq!(unpickler.index().type_at(REC_AT), None);

    // The `RECthis` is the root; its binder has no type yet, so it is decoded
    // on demand, and that decode reaches this very `RECthis` again.
    let this = unpickler.unpickle_type(3).unwrap();
    let binder = unpickler.index().type_at(REC_AT).unwrap();
    let count = unpickler.index().type_count();

    assert_eq!(unpickler.unpickle_type(REC_AT), Ok(binder));
    assert_eq!(unpickler.unpickle_type(3), Ok(this));
    assert_eq!(unpickler.index().type_count(), count);
    drop(unpickler);
    assert_eq!(rec_this_binder(&session.store, this), binder);
    assert_eq!(recursive_parent(&session.store, binder), this);
}

#[test]
fn two_rec_this_addresses_for_one_binder_share_one_type_id() {
    // `RECtype` at 2 over `ANDtype` at 3 over two `RECthis` (at 5 and 7).
    let mut both = rec_this(2);
    both.extend(rec_this(2));
    let bytes = file_with(&rec_type(&length_node(AND, &both)));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let id = unpickler.unpickle_type(REC_AT).unwrap();
    let (first, second) = (
        unpickler.index().type_at(5).unwrap(),
        unpickler.index().type_at(7).unwrap(),
    );
    drop(unpickler);

    // Two addresses, one semantic `RecThis`, as Dotty's one `recThis`.
    assert_eq!(first, second);
    assert_eq!(rec_this_binder(&session.store, first), id);
    let parent = recursive_parent(&session.store, id);
    assert_eq!(
        session.store.types.get(parent),
        &Type::And {
            left: first,
            right: first
        }
    );
}

#[test]
fn rec_this_values_of_different_recursive_binders_are_never_shared() {
    // Outer `RECtype` at 2, inner at 3, whose parent is an `ANDtype` (at 4) of
    // a `RECthis` to the outer (at 6) and one to the inner (at 8).
    let mut both = rec_this(2);
    both.extend(rec_this(3));
    let bytes = file_with(&rec_type(&rec_type(&length_node(AND, &both))));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let outer = unpickler.unpickle_type(REC_AT).unwrap();
    let inner = unpickler.index().type_at(3).unwrap();
    let to_outer = unpickler.index().type_at(6).unwrap();
    let to_inner = unpickler.index().type_at(8).unwrap();
    drop(unpickler);

    assert_ne!(to_outer, to_inner);
    assert_eq!(rec_this_binder(&session.store, to_outer), outer);
    assert_eq!(rec_this_binder(&session.store, to_inner), inner);
}

#[test]
fn a_shared_link_to_a_rec_this_returns_the_exact_rec_this() {
    // `ANDtype` over a `RECtype` (at 2, with a `RECthis` at 3) and a
    // `SHAREDtype` link to that `RECthis`.
    let rec = rec_type(&rec_this(2));
    let bytes = file_with_two(&rec, &[SHARED, nat(3)]);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    let link = 2 + u32::try_from(rec.len()).unwrap();

    let this = unpickler.unpickle_type(3).unwrap();
    assert_eq!(unpickler.unpickle_type(link), Ok(this));
}

#[test]
fn a_rec_this_naming_no_node_is_an_invalid_reference() {
    // Address 1 is the length byte of the wrapper, not the start of a node.
    for target in [1u8, 100] {
        let bytes = file_with(&rec_this(target));
        let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
        let mut session = Session::new();
        let mut unpickler = unpickler_for(&file, &mut session);

        assert_eq!(
            unpickler.unpickle_type(REC_AT),
            Err(UnpickleError::InvalidReferenceTarget {
                from: 2,
                to: u32::from(target)
            })
        );
    }
}

#[test]
fn a_rec_this_naming_a_node_that_is_not_a_recursive_type_is_an_invalid_reference() {
    // The `RECthis` (at 4) names the `TYPEREFpkg` at 2, never decoded.
    let mut both = package_ref();
    both.extend(rec_this(2));
    let bytes = file_with_ast(&length_node(AND, &both));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    let before = unpickler.index().type_count();

    assert_eq!(
        unpickler.unpickle_type(4),
        Err(UnpickleError::InvalidReferenceTarget { from: 4, to: 2 })
    );
    assert_eq!(unpickler.index().type_count(), before);
}

#[test]
fn a_rec_this_naming_an_already_decoded_type_that_is_not_recursive_is_refused() {
    let mut both = package_ref();
    both.extend(rec_this(2));
    let bytes = file_with_ast(&length_node(AND, &both));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    unpickler.unpickle_type(2).unwrap();

    assert_eq!(
        unpickler.unpickle_type(4),
        Err(UnpickleError::InvalidReferenceTarget { from: 4, to: 2 })
    );
}

#[test]
fn a_rec_this_naming_itself_is_an_error_not_a_loop() {
    // The `RECthis` at 2 names address 2, which is itself: no `RECtype`.
    let bytes = file_with(&rec_this(2));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    assert_eq!(
        unpickler.unpickle_type(REC_AT),
        Err(UnpickleError::InvalidReferenceTarget { from: 2, to: 2 })
    );
}

#[test]
fn a_rec_this_naming_a_binder_that_is_not_recursive_is_refused_while_it_is_pending() {
    // `TYPELAMBDAtype` at 2 whose result is a `RECthis` (at 4) naming it.
    let mut payload = rec_this(2);
    payload.extend(length_node(163, &package_ref()));
    payload.push(nat(1));
    let bytes = file_with(&length_node(LAMBDA, &payload));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    assert_eq!(
        unpickler.unpickle_type(REC_AT),
        Err(UnpickleError::InvalidReferenceTarget { from: 4, to: 2 })
    );
    assert_eq!(unpickler.index().type_at(REC_AT), None);
}

/// A chain of `count` sibling `RECtype`s under an `APPLIEDtype`, each one's
/// parent a `RECthis` naming the next, the last one's parent a package. The
/// first is at address 4.
fn rec_chain(count: u8) -> Vec<u8> {
    let mut payload = package_ref();
    for position in 0..count {
        payload.push(RECTYPE);
        if position + 1 == count {
            payload.extend(package_ref());
        } else {
            // The next `RECtype` is 3 bytes on.
            payload.extend(rec_this(4 + 3 * (position + 1)));
        }
    }
    file_with_ast(&length_node(161, &payload))
}

#[test]
fn a_short_chain_of_on_demand_binders_decodes() {
    let bytes = rec_chain(5);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    assert!(unpickler.unpickle_type(4).is_ok());
}

#[test]
fn a_long_chain_of_on_demand_binders_is_bounded_like_a_shared_chain() {
    let bytes = rec_chain(30);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    let before = unpickler.index().type_count();

    let result = unpickler.unpickle_type(4);
    assert!(
        matches!(result, Err(UnpickleError::InvalidReferenceTarget { .. })),
        "{result:?}"
    );
    assert_eq!(unpickler.index().type_count(), before);
}

// The canonical `RecThis` and rollback

#[test]
fn a_failed_call_does_not_leave_a_stale_canonical_rec_this_behind() {
    // Top-level 1 (address 0): `FLEXIBLE(AND(RECtype@4 over RECthis@5, IMPORTED))`.
    // Top-level 2 (address 9): `FLEXIBLE(RECtype@11 over AND(TYPEREFpkg, RECthis@16))`.
    let mut first = rec_type(&rec_this(4));
    first.extend([UNSUPPORTED, nat(1)]);
    let first = length_node(FLEXIBLE, &length_node(AND, &first));
    assert_eq!(first.len(), 9);
    let mut both = package_ref();
    both.extend(rec_this(11));
    let second = length_node(FLEXIBLE, &rec_type(&length_node(AND, &both)));
    let mut ast = first;
    ast.extend(second);
    let bytes = file_with_ast(&ast);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    // The first call builds a `RecThis` for its binder and then fails.
    assert!(matches!(
        unpickler.unpickle_type(2),
        Err(UnpickleError::UnsupportedType { .. })
    ));
    // The second call's binder reuses the id the failed call's had, but its
    // `RecThis` is allocated at a different position. A surviving entry would
    // hand back the failed call's `RecThis` id, now some other type.
    let binder = unpickler.unpickle_type(11).unwrap();
    let this = unpickler.index().type_at(16).unwrap();
    drop(unpickler);

    assert_eq!(rec_this_binder(&session.store, this), binder);
}

#[test]
fn a_recursive_type_and_its_rec_this_from_an_earlier_call_survive_a_later_failure() {
    // `RECtype` at 4 over a `RECthis`; an `ANDtype` (at 2) of it and an
    // unsupported form. Then a second `RECthis` for the same binder in
    // another top-level node.
    let mut inner = rec_type(&rec_this(4));
    inner.extend([UNSUPPORTED, nat(1)]);
    let first = length_node(FLEXIBLE, &length_node(AND, &inner));
    // A second top-level `FLEXIBLE` around a `RECthis` to the same `RECtype`.
    let second = length_node(FLEXIBLE, &rec_this(4));
    let mut ast = first;
    let second_at = u32::try_from(ast.len()).unwrap() + 2;
    ast.extend(second);
    let bytes = file_with_ast(&ast);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    // The recursive type decodes on its own first.
    let binder = unpickler.unpickle_type(4).unwrap();
    let this = unpickler.index().type_at(5).unwrap();
    let count = unpickler.index().type_count();
    // A call that includes it fails on the unsupported sibling.
    assert!(unpickler.unpickle_type(2).is_err());

    // What existed before the failed call is intact.
    assert_eq!(unpickler.index().type_at(4), Some(binder));
    assert_eq!(unpickler.index().type_count(), count);
    // And the canonical `RecThis` is still the one, for a new address.
    assert_eq!(unpickler.unpickle_type(second_at), Ok(this));
}

// REFINEDtype

const REFINED: u8 = 159;
const TYPEBOUNDS: u8 = 163;
const APPLIED: u8 = 161;

/// `REFINEDtype Length name_NameRef parent info`.
fn refined(name: u8, parent: &[u8], info: &[u8]) -> Vec<u8> {
    let mut payload = vec![nat(name)];
    payload.extend(parent);
    payload.extend(info);
    length_node(REFINED, &payload)
}

/// `TYPEBOUNDS` over one alias child: 4 bytes.
fn alias_bounds() -> Vec<u8> {
    length_node(TYPEBOUNDS, &package_ref())
}

/// A file whose only top-level node is an `APPLIEDtype` over the given
/// children (any count), the first at address 2.
fn file_with_many(children: &[&[u8]]) -> Vec<u8> {
    let mut payload = Vec::new();
    for child in children {
        payload.extend(*child);
    }
    file_with_ast(&length_node(APPLIED, &payload))
}

fn refined_parts(store: &SemanticStore, id: TypeId) -> (TypeId, dotty_core::Name, TypeId) {
    match store.types.get(id) {
        Type::Refined { parent, name, info } => (*parent, *name, *info),
        other => panic!("not a refined type: {other:?}"),
    }
}

fn namespace_of(bytes: &[u8], at: u32) -> Namespace {
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    let id = unpickler.unpickle_type(at).unwrap();
    drop(unpickler);
    let (_, name, _) = refined_parts(&session.store, id);
    assert_eq!(session.store.names.resolve(name.text()), "m");
    name.namespace()
}

#[test]
fn a_refinement_whose_info_is_not_bounds_has_a_term_name() {
    let bytes = file_with(&refined(2, &package_ref(), &package_ref()));
    assert_eq!(namespace_of(&bytes, 2), Namespace::Term);
}

#[test]
fn a_refinement_whose_info_is_bounds_has_a_type_name() {
    let bytes = file_with(&refined(2, &package_ref(), &alias_bounds()));
    assert_eq!(namespace_of(&bytes, 2), Namespace::Type);
}

#[test]
fn the_namespace_follows_a_shared_link_to_bounds() {
    // The bounds at 2 (4 bytes); a refinement at 6 whose info is a link to it.
    let bounds = alias_bounds();
    let link = [SHARED, nat(2)];
    let bytes = file_with_many(&[&bounds, &refined(2, &package_ref(), &link)]);
    assert_eq!(namespace_of(&bytes, 6), Namespace::Type);
}

#[test]
fn the_namespace_follows_a_chain_of_shared_links() {
    // Bounds at 2, a link to them at 6, and a refinement at 8 whose info is a
    // link to that link.
    let bounds = alias_bounds();
    let first = [SHARED, nat(2)];
    let second = [SHARED, nat(6)];
    let bytes = file_with_many(&[&bounds, &first, &refined(2, &package_ref(), &second)]);
    assert_eq!(namespace_of(&bytes, 8), Namespace::Type);
}

#[test]
fn a_shared_link_to_something_that_is_not_bounds_leaves_a_term_name() {
    // A `TYPEREFpkg` at 2 and a refinement at 4 whose info is a link to it.
    let pkg = package_ref();
    let link = [SHARED, nat(2)];
    let bytes = file_with_many(&[&pkg, &refined(2, &package_ref(), &link)]);
    assert_eq!(namespace_of(&bytes, 4), Namespace::Term);
}

#[test]
fn a_refinement_info_that_links_to_itself_is_an_error_not_a_loop() {
    // Refinement at 2: name at 4, parent at 5, info at 7 is a link to 7.
    let bytes = file_with(&refined(2, &package_ref(), &[SHARED, nat(7)]));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    assert!(matches!(
        unpickler.unpickle_type(2),
        Err(UnpickleError::InvalidReferenceTarget { from: 2, .. })
    ));
    assert_eq!(unpickler.index().type_at(2), None);
}

#[test]
fn a_refinement_keeps_its_parent_and_info_from_their_absolute_addresses() {
    // Parent at 5 (a `TYPEREFpkg`), info at 7 (another one): distinct nodes.
    let bytes = file_with(&refined(2, &package_ref(), &package_ref()));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let id = unpickler.unpickle_type(2).unwrap();
    let (parent, info) = (
        unpickler.index().type_at(5).unwrap(),
        unpickler.index().type_at(7).unwrap(),
    );
    drop(unpickler);

    let (found_parent, _, found_info) = refined_parts(&session.store, id);
    assert_eq!((found_parent, found_info), (parent, info));
}

#[test]
fn nested_refinements_keep_the_order_and_nesting_tasty_writes() {
    // `Base { m1 } { m2 }`: the inner refinement is the outer one's parent.
    // Names: `p` for the inner member, `m` for the outer one.
    let inner = refined(1, &package_ref(), &package_ref());
    let bytes = file_with(&refined(2, &inner, &alias_bounds()));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let outer = unpickler.unpickle_type(2).unwrap();
    let inner_id = unpickler.index().type_at(5).unwrap();
    drop(unpickler);

    let (parent, name, _) = refined_parts(&session.store, outer);
    assert_eq!(parent, inner_id);
    assert_eq!(session.store.names.resolve(name.text()), "m");
    assert_eq!(name.namespace(), Namespace::Type);
    let (base, inner_name, _) = refined_parts(&session.store, inner_id);
    assert_eq!(session.store.names.resolve(inner_name.text()), "p");
    assert_eq!(inner_name.namespace(), Namespace::Term);
    assert!(matches!(
        session.store.types.get(base),
        Type::TypeRef { .. }
    ));
}

#[test]
fn a_refinement_is_decoded_once_and_a_shared_link_returns_its_id() {
    let refinement = refined(2, &package_ref(), &package_ref());
    let link_at = 2 + u8::try_from(refinement.len()).unwrap();
    let bytes = file_with_two(&refinement, &[SHARED, nat(2)]);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let id = unpickler.unpickle_type(2).unwrap();
    assert_eq!(unpickler.unpickle_type(2), Ok(id));
    assert_eq!(unpickler.unpickle_type(u32::from(link_at)), Ok(id));
}

#[test]
fn a_signed_refinement_name_is_refused_not_stripped() {
    let bytes = file_with(&refined(3, &package_ref(), &package_ref()));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    assert!(matches!(
        unpickler.unpickle_type(2),
        Err(UnpickleError::UnsupportedSignedReference { address: 2, .. })
    ));
}

#[test]
fn a_failing_refinement_child_leaves_nothing_behind() {
    // The parent decodes, the info does not.
    let bytes = file_with(&refined(2, &package_ref(), &[UNSUPPORTED, nat(1)]));
    let next_id_after = |fail: bool| {
        let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
        let mut session = Session::new();
        let mut unpickler = unpickler_for(&file, &mut session);
        if fail {
            let before = unpickler.index().type_count();
            assert!(matches!(
                unpickler.unpickle_type(2),
                Err(UnpickleError::UnsupportedType { .. })
            ));
            assert_eq!(unpickler.index().type_count(), before);
            assert_eq!(unpickler.index().type_at(2), None);
            assert_eq!(unpickler.index().type_at(5), None);
        }
        drop(unpickler);
        session.store.types.alloc(Type::NoType)
    };

    assert_eq!(next_id_after(true), next_id_after(false));
}

// Real Scala 3.9.0 output: `tests/fixtures/semantic/RecursiveRefined.scala`.

const REAL: &[u8] = include_bytes!("fixtures/semantic/RecursiveRefined.tasty");

/// `Base { type T = Int }`: a type member, alias bounds.
const TYPE_MEMBER: u32 = 203;
const TYPE_MEMBER_INFO: u32 = 209;
/// `Base { type T <: Any }`: two-sided bounds.
const UPPER_MEMBER: u32 = 244;
/// `Base { def run(x: Int): Int }`: a term member with a method info.
const METHOD_MEMBER: u32 = 289;
const METHOD_MEMBER_INFO: u32 = 295;
/// `Base { type T = Int; def run(x: Int): Int }`: the parent is a link to
/// [`TYPE_MEMBER`].
const TWO_MEMBERS: u32 = 344;
/// `Base { type T = Int; type U = Int }`: the parent links to [`TYPE_MEMBER`]
/// and the info links to [`TYPE_MEMBER_INFO`].
const SHARED_BOUNDS: u32 = 489;
/// `C { type T1; type T2 = T1 }`: a `RECtype` (399) over two refinements whose
/// `T2` names `T1` by name through the `RECthis` at 419.
const RECURSIVE: u32 = 399;
const RECURSIVE_THIS: u32 = 419;
/// `Base { def me: this.type }`: a `RECtype` (446) over a refinement whose info
/// is a by-name type over the `RECthis` at 454.
const SELF_TYPE: u32 = 446;
const SELF_TYPE_REFINED: u32 = 447;
const SELF_TYPE_THIS: u32 = 454;

/// A session holding the `scala` classes the fixture references but does not
/// define, with its own symbols entered.
macro_rules! real_unit {
    ($file:ident, $session:ident, $unpickler:ident) => {
        let $file = TastyFile::parse_scala_3_9(REAL).unwrap();
        let mut $session = Session::new();
        let mut packages = Packages::new();
        let scala = packages
            .enter(&mut $session.store, SymbolOrigin::Synthetic, &["scala"])
            .pop()
            .unwrap();
        for class in ["Int", "Any", "Nothing"] {
            let name = dotty_core::Name::new($session.store.names.intern(class), Namespace::Type);
            let symbol = $session.store.symbols.alloc(dotty_core::Symbol {
                name,
                owner: Some(scala.symbol),
                kind: dotty_core::SymbolKind::Class,
                flags: dotty_core::SymbolFlags::EMPTY,
                visibility: dotty_core::Visibility::Public,
                info: dotty_core::SymbolInfo::Missing,
                origin: SymbolOrigin::Synthetic,
                annotations: Vec::new(),
                position: None,
                links: dotty_core::SymbolLinks::default(),
            });
            $session
                .store
                .scopes
                .get_mut(scala.scope)
                .enter(name, symbol);
        }
        let mut $unpickler = TastyUnpickler::with_packages(
            &$file,
            &mut $session.store,
            $session.definitions,
            packages,
        );
        $unpickler.enter_symbols().unwrap();
    };
}

fn member_name(store: &SemanticStore, name: dotty_core::Name) -> String {
    store.names.resolve(name.text()).to_string()
}

#[test]
fn a_real_type_refinement_has_a_type_name_and_alias_bounds() {
    real_unit!(file, session, unpickler);
    let id = unpickler.unpickle_type(TYPE_MEMBER).unwrap();
    let info = unpickler.index().type_at(TYPE_MEMBER_INFO).unwrap();
    drop(unpickler);

    let (parent, name, found) = refined_parts(&session.store, id);
    assert_eq!(member_name(&session.store, name), "T");
    assert_eq!(name.namespace(), Namespace::Type);
    assert_eq!(found, info);
    assert!(matches!(
        session.store.types.get(found),
        Type::AliasingBounds { .. }
    ));
    assert!(matches!(
        session.store.types.get(parent),
        Type::TypeRef { .. }
    ));
}

#[test]
fn a_real_upper_bound_refinement_has_a_type_name_and_two_sided_bounds() {
    real_unit!(file, session, unpickler);
    let id = unpickler.unpickle_type(UPPER_MEMBER).unwrap();
    drop(unpickler);

    let (_, name, info) = refined_parts(&session.store, id);
    assert_eq!(name.namespace(), Namespace::Type);
    assert!(matches!(session.store.types.get(info), Type::Bounds { .. }));
}

#[test]
fn a_real_method_refinement_has_a_term_name_and_a_method_info() {
    real_unit!(file, session, unpickler);
    let id = unpickler.unpickle_type(METHOD_MEMBER).unwrap();
    let info = unpickler.index().type_at(METHOD_MEMBER_INFO).unwrap();
    drop(unpickler);

    let (_, name, found) = refined_parts(&session.store, id);
    assert_eq!(member_name(&session.store, name), "run");
    assert_eq!(name.namespace(), Namespace::Term);
    assert_eq!(found, info);
    assert!(matches!(session.store.types.get(found), Type::Method(_)));
}

#[test]
fn real_members_are_nested_in_the_order_they_are_written() {
    real_unit!(file, session, unpickler);
    let outer = unpickler.unpickle_type(TWO_MEMBERS).unwrap();
    let inner = unpickler.unpickle_type(TYPE_MEMBER).unwrap();
    drop(unpickler);

    // `{ type T = Int; def run }`: the type member is the inner refinement,
    // written first, and it is the very one at its own address.
    let (parent, name, _) = refined_parts(&session.store, outer);
    assert_eq!(parent, inner);
    assert_eq!(member_name(&session.store, name), "run");
    let (_, inner_name, _) = refined_parts(&session.store, inner);
    assert_eq!(member_name(&session.store, inner_name), "T");
}

#[test]
fn a_real_refinement_whose_info_is_a_shared_link_still_has_a_type_name() {
    real_unit!(file, session, unpickler);
    let id = unpickler.unpickle_type(SHARED_BOUNDS).unwrap();
    let bounds = unpickler.index().type_at(TYPE_MEMBER_INFO).unwrap();
    let parent = unpickler.index().type_at(TYPE_MEMBER).unwrap();
    drop(unpickler);

    // Both children are `SHAREDtype` links: the namespace comes from the
    // bounds they lead to, and each resolves to the exact earlier type.
    let (found_parent, name, info) = refined_parts(&session.store, id);
    assert_eq!(member_name(&session.store, name), "U");
    assert_eq!(name.namespace(), Namespace::Type);
    assert_eq!(info, bounds);
    assert_eq!(found_parent, parent);
}

#[test]
fn a_real_recursive_refinement_ties_recursive_refined_and_rec_this() {
    real_unit!(file, session, unpickler);
    let recursive = unpickler.unpickle_type(SELF_TYPE).unwrap();
    let refined_id = unpickler.index().type_at(SELF_TYPE_REFINED).unwrap();
    let this = unpickler.index().type_at(SELF_TYPE_THIS).unwrap();
    drop(unpickler);

    // Recursive -> Refined -> ByName -> RecThis, the last naming the very
    // `Recursive` id.
    assert_eq!(recursive_parent(&session.store, recursive), refined_id);
    let (_, name, info) = refined_parts(&session.store, refined_id);
    assert_eq!(member_name(&session.store, name), "me");
    assert_eq!(name.namespace(), Namespace::Term);
    assert_eq!(
        session.store.types.get(info),
        &Type::ByName { result: this }
    );
    assert_eq!(rec_this_binder(&session.store, this), recursive);
}

#[test]
fn a_real_rec_this_decoded_first_builds_the_same_graph() {
    real_unit!(file, session, unpickler);
    let this = unpickler.unpickle_type(SELF_TYPE_THIS).unwrap();
    let recursive = unpickler.index().type_at(SELF_TYPE).unwrap();
    let count = unpickler.index().type_count();

    assert_eq!(unpickler.unpickle_type(SELF_TYPE), Ok(recursive));
    assert_eq!(unpickler.unpickle_type(SELF_TYPE_THIS), Ok(this));
    assert_eq!(unpickler.index().type_count(), count);
    drop(unpickler);
    assert_eq!(rec_this_binder(&session.store, this), recursive);
}

/// The address and type of every node of the real unit decoded so far.
fn decoded_types(unpickler: &TastyUnpickler<'_, '_, '_>) -> Vec<(u32, TypeId)> {
    let file = TastyFile::parse_scala_3_9(REAL).unwrap();
    let index = file.ast_address_index().unwrap();
    index
        .iter_nodes()
        .filter_map(|node| {
            let at = u32::try_from(node.offset).unwrap();
            Some((at, unpickler.index().type_at(at)?))
        })
        .collect()
}

/// Those of `decoded` whose reference is name-designated.
fn name_designated(store: &SemanticStore, decoded: &[(u32, TypeId)]) -> Vec<(u32, TypeId)> {
    decoded
        .iter()
        .copied()
        .filter(|(_, id)| {
            matches!(
                store.types.get(*id),
                Type::TypeRef {
                    target: TypeRefTarget::Name(_),
                    ..
                }
            )
        })
        .collect()
}

#[test]
fn a_real_member_selected_by_name_from_a_rec_this_is_a_name_designated_reference() {
    // `C { type T1; type T2 = T1 }`: the `T1` on the right is a name-based
    // reference whose prefix is the `RecThis`. A refinement member has no
    // symbol, so the reference is `prefix + Name`, and no symbol is invented.
    real_unit!(file, session, unpickler);
    let symbols = unpickler.index().symbol_count();

    let recursive = unpickler.unpickle_type(RECURSIVE).unwrap();
    let this = unpickler.index().type_at(RECURSIVE_THIS).unwrap();
    let decoded = decoded_types(&unpickler);
    let symbols_after = unpickler.index().symbol_count();
    drop(unpickler);
    let store = &session.store;
    let refs = name_designated(store, &decoded);

    assert_eq!(symbols_after, symbols);
    // Recursive R { Refined T2 (Refined T1 C, bounds) = Alias(T1 ref) }
    let t2 = recursive_parent(store, recursive);
    let (t1_refined, t2_name, t2_info) = refined_parts(store, t2);
    assert_eq!(member_name(store, t2_name), "T2");
    let (class, t1_name, t1_info) = refined_parts(store, t1_refined);
    assert_eq!(member_name(store, t1_name), "T1");
    assert!(matches!(store.types.get(class), Type::TypeRef { .. }));
    let Type::AliasingBounds { alias } = *store.types.get(t2_info) else {
        panic!("alias bounds expected, got {:?}", store.types.get(t2_info));
    };
    // The reference: RecThis(R) prefix, Name(T1) target, no symbol.
    let Type::TypeRef { prefix, target } = *store.types.get(alias) else {
        panic!("a type ref expected, got {:?}", store.types.get(alias));
    };
    assert_eq!(prefix, this);
    assert_eq!(rec_this_binder(store, prefix), recursive);
    let TypeRefTarget::Name(name) = target else {
        panic!("a name designator expected");
    };
    assert_eq!(name.as_name(), &t1_name);
    assert_eq!(store.types.get(alias).reference_symbol(), None);
    // The single name-designated reference of the unit is this one.
    assert_eq!(refs.iter().map(|(_, id)| *id).collect::<Vec<_>>(), [alias]);
    // Not replaced by the bounds it names.
    assert_ne!(alias, t1_info);

    // After construction, structural lookup recovers the T1 refinement's info.
    assert_eq!(
        lookup_structural_member(store, prefix, t1_name),
        Ok(StructuralMemberLookup::Found {
            name: t1_name,
            info: t1_info
        })
    );
}

#[test]
fn both_decode_orders_of_the_real_recursive_refinement_build_one_graph() {
    real_unit!(file, session, unpickler);
    let recursive = unpickler.unpickle_type(RECURSIVE).unwrap();
    let this = unpickler.index().type_at(RECURSIVE_THIS).unwrap();
    let count = unpickler.index().type_count();
    let decoded = decoded_types(&unpickler);
    drop(unpickler);
    let refs = name_designated(&session.store, &decoded);

    real_unit!(file, session2, unpickler2);
    // The `RECthis` first: its binder is decoded on demand.
    let this2 = unpickler2.unpickle_type(RECURSIVE_THIS).unwrap();
    let recursive2 = unpickler2.index().type_at(RECURSIVE).unwrap();
    assert_eq!(unpickler2.unpickle_type(RECURSIVE), Ok(recursive2));
    assert_eq!(unpickler2.unpickle_type(RECURSIVE_THIS), Ok(this2));
    let decoded2 = decoded_types(&unpickler2);
    let count2 = unpickler2.index().type_count();
    drop(unpickler2);
    let refs2 = name_designated(&session2.store, &decoded2);

    // The same ids, in the same numbering, whichever entry point built it.
    assert_eq!((recursive2, this2, count2), (recursive, this, count));
    assert_eq!(refs2, refs);
    assert_eq!(rec_this_binder(&session2.store, this2), recursive2);
}

#[test]
fn a_rec_this_naming_a_shared_link_to_a_recursive_type_is_refused_in_either_order() {
    // `RECtype` at 2 (over a package, 4 bytes on to 6), a `SHAREDtype` link to
    // it at 6, and a `RECthis` naming the LINK at 8. Dotty's `RECthis` is
    // `typeAtAddr(readAddr())` on the address itself, and only nodes that
    // register themselves (`RECtype`, lambdas) or targets of a link are in
    // that map: a link's own address never is, so it is a lookup failure
    // there and an `InvalidReferenceTarget` here, whatever was decoded before.
    let rec = rec_type(&package_ref());
    let link_at = 2 + u8::try_from(rec.len()).unwrap();
    let link = [SHARED, nat(2)];
    let this = rec_this(link_at);
    let bytes = file_with_many(&[&rec, &link, &this]);
    let this_at = u32::from(link_at) + 2;
    let expected = Err(UnpickleError::InvalidReferenceTarget {
        from: this_at,
        to: u32::from(link_at),
    });

    // Nothing decoded before.
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    assert_eq!(unpickler.unpickle_type(this_at), expected);

    // The `RECtype` and the link decoded first.
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    unpickler.unpickle_type(2).unwrap();
    unpickler.unpickle_type(u32::from(link_at)).unwrap();
    assert_eq!(unpickler.unpickle_type(this_at), expected);
}

// Name-designated references (Milestone 4c2), over synthetic wire

const TYPEREF: u8 = 117;
const TERMREF: u8 = 115;
const TYPEREFIN: u8 = 175;
const NAME_M: u8 = 2;
const NAME_SIGNED: u8 = 3;

/// `TYPEREF m prefix` or `TERMREF m prefix`.
fn named(tag: u8, name: u8, prefix: &[u8]) -> Vec<u8> {
    let mut node = vec![tag, nat(name)];
    node.extend(prefix);
    node
}

/// `RECtype (REFINED m package {info: TYPEBOUNDS reference})`, with the
/// reference to build, in a file whose `RECtype` is at 2 and whose reference
/// is at [`REFERENCE_AT`]. The reference names the `RECtype` through a
/// `RECthis` while that type is still being decoded.
fn recursive_holding(reference: &[u8]) -> Vec<u8> {
    let info = length_node(TYPEBOUNDS, reference);
    rec_type(&refined(NAME_M, &package_ref(), &info))
}

/// `RECtype` at 2, `REFINED` at 3, its info `TYPEBOUNDS` at 8, the reference
/// at 10.
const REFERENCE_AT: u32 = 10;

fn this_of_rec() -> Vec<u8> {
    rec_this(u8::try_from(REC_AT).unwrap())
}

/// The reference inside the one refinement of `recursive`.
fn reference_of(store: &SemanticStore, recursive: TypeId) -> TypeId {
    let (_, _, info) = refined_parts(store, recursive_parent(store, recursive));
    match store.types.get(info) {
        Type::AliasingBounds { alias } => *alias,
        // A term member's info is the type itself.
        _ => info,
    }
}

#[test]
fn a_name_designated_reference_is_built_while_its_recursive_type_is_still_pending() {
    let bytes = file_with(&recursive_holding(&named(TYPEREF, NAME_M, &this_of_rec())));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    let symbols = unpickler.index().symbol_count();

    let recursive = unpickler.unpickle_type(REC_AT).unwrap();
    let reference = unpickler.index().type_at(REFERENCE_AT).unwrap();
    assert_eq!(unpickler.index().symbol_count(), symbols);
    drop(unpickler);

    let store = &session.store;
    let (_, name, _) = refined_parts(store, recursive_parent(store, recursive));
    assert_eq!(reference_of(store, recursive), reference);
    let Type::TypeRef { prefix, target } = *store.types.get(reference) else {
        panic!("a type ref expected");
    };
    assert_eq!(rec_this_binder(store, prefix), recursive);
    assert_eq!(target, TypeRefTarget::Name(TypeName::new(name.text())));
    assert_eq!(name.namespace(), Namespace::Type);
}

#[test]
fn a_shared_link_to_a_name_designated_reference_is_the_same_type_in_either_order() {
    let reference = named(TYPEREF, NAME_M, &this_of_rec());
    let rec = recursive_holding(&reference);
    let link_at = 2 + u8::try_from(rec.len()).unwrap();
    let bytes = file_with_two(&rec, &[SHARED, nat(u8::try_from(REFERENCE_AT).unwrap())]);
    let ids = |link_first: bool| {
        let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
        let mut session = Session::new();
        let mut unpickler = unpickler_for(&file, &mut session);
        let (recursive, linked) = if link_first {
            let linked = unpickler.unpickle_type(u32::from(link_at)).unwrap();
            (unpickler.unpickle_type(REC_AT).unwrap(), linked)
        } else {
            let recursive = unpickler.unpickle_type(REC_AT).unwrap();
            (
                recursive,
                unpickler.unpickle_type(u32::from(link_at)).unwrap(),
            )
        };
        let direct = unpickler.index().type_at(REFERENCE_AT).unwrap();
        let count = unpickler.index().type_count();
        drop(unpickler);
        assert_eq!(linked, direct, "link first: {link_first}");
        assert_eq!(reference_of(&session.store, recursive), direct);
        (recursive, direct, count)
    };

    assert_eq!(ids(false), ids(true));
}

#[test]
fn a_term_reference_through_a_rec_this_is_name_designated_too() {
    let bytes = file_with(&recursive_holding(&named(TERMREF, NAME_M, &this_of_rec())));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let recursive = unpickler.unpickle_type(REC_AT).unwrap();
    drop(unpickler);

    let store = &session.store;
    let reference = reference_of(store, recursive);
    let Type::TermRef { prefix, target } = *store.types.get(reference) else {
        panic!("a term ref expected");
    };
    assert_eq!(rec_this_binder(store, prefix), recursive);
    let TermRefTarget::Name(name) = target else {
        panic!("a name designator expected");
    };
    assert_eq!(name.as_name().namespace(), Namespace::Term);
    assert_eq!(store.names.resolve(name.as_name().text()), "m");
    // A `TypeRef` to the same text would be a different reference.
    assert_eq!(store.types.get(reference).reference_symbol(), None);
}

#[test]
fn a_refined_prefix_designates_a_member_by_name_as_well() {
    // `(p { type m }).m`.
    let prefix = refined(NAME_M, &package_ref(), &alias_bounds());
    let bytes = file_with(&named(TYPEREF, NAME_M, &prefix));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let id = unpickler.unpickle_type(REC_AT).unwrap();
    drop(unpickler);

    let Type::TypeRef { prefix, target } = *session.store.types.get(id) else {
        panic!("a type ref expected");
    };
    assert!(matches!(
        session.store.types.get(prefix),
        Type::Refined { .. }
    ));
    assert!(matches!(target, TypeRefTarget::Name(_)));
}

#[test]
fn a_signed_term_reference_through_a_rec_this_is_still_deferred() {
    let bytes = file_with(&recursive_holding(&named(
        TERMREF,
        NAME_SIGNED,
        &this_of_rec(),
    )));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    assert!(matches!(
        unpickler.unpickle_type(REC_AT),
        Err(UnpickleError::UnsupportedSignedReference { .. })
    ));
}

#[test]
fn an_owner_space_reference_never_falls_back_to_a_name_designator() {
    // `TYPEREFin m (RECthis) p`: the declaration is looked for in the package
    // `p`, which has none, so it is unresolved, not `RecThis.m`.
    let reference = length_node(
        TYPEREFIN,
        &[vec![nat(NAME_M)], this_of_rec(), package_ref()].concat(),
    );
    let bytes = file_with(&recursive_holding(&reference));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    assert!(matches!(
        unpickler.unpickle_type(REC_AT),
        Err(UnpickleError::UnresolvedMember { space: Some(_), .. })
    ));
}

#[test]
fn an_owner_space_without_declarations_is_a_space_error_even_under_a_structural_prefix() {
    // `TYPEREFin m (RECthis) (RECthis)`: a `RecThis` is not a declaration
    // space, and the structural prefix must not turn that into a name.
    let reference = length_node(
        TYPEREFIN,
        &[vec![nat(NAME_M)], this_of_rec(), this_of_rec()].concat(),
    );
    let bytes = file_with(&recursive_holding(&reference));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    assert!(matches!(
        unpickler.unpickle_type(REC_AT),
        Err(UnpickleError::UnsupportedResolutionSpace { .. })
    ));
}

#[test]
fn a_prefix_that_is_not_structural_stays_an_unresolved_member() {
    // `p.m`: the package `p` is entered and has no `m`; a name is not a
    // fallback for missing resolution.
    let bytes = file_with(&named(TYPEREF, NAME_M, &package_ref()));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    assert!(matches!(
        unpickler.unpickle_type(REC_AT),
        Err(UnpickleError::UnresolvedMember { space: None, .. })
    ));
}

/// A resolver that answers every request with `answer`.
struct Answering(Option<dotty_core::SymbolId>);

impl dotty_core::SymbolResolver for Answering {
    fn checkpoint(&self) -> dotty_core::ResolverCheckpoint {
        dotty_core::ResolverCheckpoint::new(0)
    }

    fn rollback_to(
        &mut self,
        _store: &mut SemanticStore,
        _checkpoint: dotty_core::ResolverCheckpoint,
    ) {
    }

    fn resolve_member(
        &mut self,
        _store: &mut SemanticStore,
        _request: &dotty_core::MemberRequest,
    ) -> Result<Option<dotty_core::SymbolId>, dotty_core::ResolutionError> {
        Ok(self.0)
    }

    fn resolve_package(
        &mut self,
        _store: &mut SemanticStore,
        _path: &[&str],
    ) -> Result<Option<dotty_core::SymbolId>, dotty_core::ResolutionError> {
        Ok(None)
    }
}

#[test]
fn a_symbol_the_resolver_knows_wins_over_a_name_designator() {
    let bytes = file_with(&recursive_holding(&named(TYPEREF, NAME_M, &this_of_rec())));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let name = dotty_core::Name::new(session.store.names.intern("m"), Namespace::Type);
    let answer = session.store.symbols.alloc(dotty_core::Symbol {
        name,
        owner: None,
        kind: dotty_core::SymbolKind::Class,
        flags: dotty_core::SymbolFlags::EMPTY,
        visibility: dotty_core::Visibility::Public,
        info: dotty_core::SymbolInfo::Missing,
        origin: SymbolOrigin::Synthetic,
        annotations: Vec::new(),
        position: None,
        links: dotty_core::SymbolLinks::default(),
    });
    let mut unpickler =
        unpickler_for(&file, &mut session).with_resolver(Box::new(Answering(Some(answer))));

    let recursive = unpickler.unpickle_type(REC_AT).unwrap();
    drop(unpickler);

    let reference = reference_of(&session.store, recursive);
    assert_eq!(
        session.store.types.get(reference).reference_symbol(),
        Some(answer)
    );
}

#[test]
fn a_failure_after_a_name_designated_reference_was_built_leaves_nothing_behind() {
    // The left operand builds the recursive type, its `RecThis` and the
    // name-designated reference; the right one fails.
    let rec = recursive_holding(&named(TYPEREF, NAME_M, &this_of_rec()));
    let bytes = file_with_two(&rec, &[UNSUPPORTED, nat(1)]);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    let (types, symbols) = (
        unpickler.index().type_count(),
        unpickler.index().symbol_count(),
    );
    let checkpoint_before = unpickler.index().type_count();

    // The failing operand is decoded after the recursive one.
    assert!(unpickler.unpickle_type(0).is_err());

    assert_eq!(unpickler.index().type_count(), checkpoint_before);
    assert_eq!(unpickler.index().type_count(), types);
    assert_eq!(unpickler.index().symbol_count(), symbols);
    assert_eq!(unpickler.index().type_at(REC_AT), None);
    assert_eq!(unpickler.index().type_at(REFERENCE_AT), None);

    // The retry builds a clean graph with its own `RecThis`.
    let recursive = unpickler.unpickle_type(REC_AT).unwrap();
    let reference = unpickler.index().type_at(REFERENCE_AT).unwrap();
    drop(unpickler);
    let store = &session.store;
    assert_eq!(reference_of(store, recursive), reference);
    let Type::TypeRef { prefix, .. } = *store.types.get(reference) else {
        panic!("a type ref expected");
    };
    assert_eq!(rec_this_binder(store, prefix), recursive);
}

#[derive(Default)]
struct MaterializedState {
    member: Option<dotty_core::SymbolId>,
    journal: Vec<Option<dotty_core::SymbolId>>,
}

struct Materializing(Rc<RefCell<MaterializedState>>);

impl dotty_core::SymbolResolver for Materializing {
    fn checkpoint(&self) -> dotty_core::ResolverCheckpoint {
        dotty_core::ResolverCheckpoint::new(self.0.borrow().journal.len() as u64)
    }

    fn rollback_to(
        &mut self,
        _store: &mut SemanticStore,
        checkpoint: dotty_core::ResolverCheckpoint,
    ) {
        let mut state = self.0.borrow_mut();
        while state.journal.len() > checkpoint.token() as usize {
            state.member = state.journal.pop().unwrap_or(None);
        }
    }

    fn resolve_member(
        &mut self,
        store: &mut SemanticStore,
        request: &dotty_core::MemberRequest,
    ) -> Result<Option<dotty_core::SymbolId>, dotty_core::ResolutionError> {
        if let Some(member) = self
            .0
            .borrow()
            .member
            .filter(|member| store.symbols.contains(*member))
        {
            return Ok(Some(member));
        }
        let member = store.symbols.alloc(dotty_core::Symbol {
            name: request.name,
            owner: None,
            kind: dotty_core::SymbolKind::Class,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: dotty_core::SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        });
        let mut state = self.0.borrow_mut();
        let previous = state.member.replace(member);
        state.journal.push(previous);
        Ok(Some(member))
    }

    fn resolve_package(
        &mut self,
        _store: &mut SemanticStore,
        _path: &[&str],
    ) -> Result<Option<dotty_core::SymbolId>, dotty_core::ResolutionError> {
        Ok(None)
    }
}

#[test]
fn a_later_tasty_failure_rolls_back_materialized_resolver_state() {
    let first = named(TYPEREF, NAME_M, &package_ref());
    let bytes = file_with_two(&first, &[UNSUPPORTED, nat(1)]);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut packages = Packages::new();
    packages.enter(&mut session.store, SymbolOrigin::Synthetic, &["p"]);
    let before = session.store.checkpoint();
    let state = Rc::new(RefCell::new(MaterializedState::default()));
    let mut unpickler =
        TastyUnpickler::with_packages(&file, &mut session.store, session.definitions, packages)
            .with_resolver(Box::new(Materializing(state.clone())));

    assert!(unpickler.unpickle_type(0).is_err());

    assert_eq!(state.borrow().member, None);
    assert!(state.borrow().journal.is_empty());
    drop(unpickler);
    assert_eq!(session.store.checkpoint(), before);
}
