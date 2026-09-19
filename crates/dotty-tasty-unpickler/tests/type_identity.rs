//! Pass 2a: type identity and reference types, over the real Scala 3.9.0
//! `Distinct.tasty` (see `tests/fixtures/semantic/TypeRefs.scala`).
//!
//! The unit holds two classes with a member class of the same name (`Left`
//! and `Right`, each with `Inner`), a class with a type parameter, a stable
//! `val`, and a local class. Several tests patch bytes of the AST to reach
//! shapes the compiler does not write, such as a `SHAREDtype` cycle; every
//! patch first asserts what it overwrites, so a regenerated fixture fails
//! loudly instead of testing something else.
//!
//! Addresses used below (absolute AST addresses):
//!
//! | address | node                                                            |
//! |---------|-----------------------------------------------------------------|
//! | 10      | `TYPEREFsymbol`, the first type reference                       |
//! | 22      | `SHAREDtype(10)`                                                |
//! | 47      | `TERMREFsymbol`, a visible node with no symbol                  |
//! | 49      | `SHAREDtype(12)`                                                |
//! | 133     | `TYPEREFsymbol` to `Inner` whose prefix is `THIS`               |
//! | 312     | `TYPEREFsymbol` with a `THIS` prefix, in `Box`                  |
//! | 351     | `TYPEREFdirect` to the type parameter `T` of `Box`              |
//! | 434     | `TYPEREFdirect` to the local class `Hidden`, which has no symbol |
//! | 445     | `SHAREDtype(434)`, a two-byte target                            |

use dotty_core::Definitions;
use dotty_core::ids::{SymbolId, TypeId};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::SymbolKind;
use dotty_core::types::Type;
use dotty_tasty::tasty::{StandardSection, TastyFile};
use dotty_tasty_unpickler::tasty_unpickler::{TastySemanticIndex, TastyUnpickler, UnpickleError};

const DISTINCT: &[u8] = include_bytes!("fixtures/semantic/Distinct.tasty");

const SHARED_TAG: u8 = 61;
const TYPEREFSYMBOL_TAG: u8 = 116;
const TERMREFSYMBOL_TAG: u8 = 114;
const TYPEREFDIRECT_TAG: u8 = 63;
const TERMREFDIRECT_TAG: u8 = 62;
const TERMREFPKG_TAG: u8 = 64;
const TYPEREFPKG_TAG: u8 = 65;

const FIRST_REFERENCE: u32 = 10;
const SHARED_TO_FIRST: u32 = 22;
const NODE_WITHOUT_SYMBOL: u32 = 47;
const SHARED_SHORT: u32 = 49;
const REFERENCE_WITH_THIS_PREFIX: u32 = 133;
const TYPE_PARAMETER_REFERENCE: u32 = 351;
const LATER_REFERENCE: u32 = 312;
const LOCAL_CLASS_REFERENCE: u32 = 434;
const SHARED_LONG: u32 = 445;

/// Overwrites `from` at AST address `at` with `to` (same length), after
/// checking the bytes are what the test expects.
fn patched(bytes: &[u8], at: usize, from: &[u8], to: &[u8]) -> Vec<u8> {
    assert_eq!(from.len(), to.len());
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let payload = file.section(StandardSection::Asts).unwrap().payload;
    assert_eq!(
        &payload[at..at + from.len()],
        from,
        "the fixture no longer has the expected bytes at {at}"
    );
    let start = payload.as_ptr() as usize - bytes.as_ptr() as usize;
    let mut patched = bytes.to_vec();
    patched[start + at..start + at + to.len()].copy_from_slice(to);
    patched
}

fn with_unpickler<R>(
    bytes: &[u8],
    check: impl FnOnce(&mut TastyUnpickler<'_, '_, '_>) -> R,
) -> (R, SemanticStore, TastySemanticIndex) {
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut unpickler = TastyUnpickler::new(&file, &mut store, definitions);
    unpickler.enter_symbols().unwrap();
    let result = check(&mut unpickler);
    let index = unpickler.into_index();
    (result, store, index)
}

fn name(store: &SemanticStore, symbol: SymbolId) -> &str {
    store.names.resolve(store.symbols.get(symbol).name.text())
}

/// The `(prefix, symbol)` of a `TypeRef` or `TermRef`.
fn reference(store: &SemanticStore, ty: TypeId) -> (TypeId, SymbolId, bool) {
    match store.types.get(ty) {
        Type::TypeRef { prefix, symbol } => (*prefix, *symbol, true),
        Type::TermRef { prefix, symbol } => (*prefix, *symbol, false),
        other => panic!("not a reference: {other:?}"),
    }
}

/// The target a reference node names: its `ASTRef`, a natural number that
/// follows the tag byte.
fn target_of(bytes: &[u8], at: usize) -> u32 {
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let payload = file.section(StandardSection::Asts).unwrap().payload;
    let mut value = 0u32;
    for &byte in &payload[at + 1..] {
        value = (value << 7) | u32::from(byte & 0x7f);
        if byte & 0x80 != 0 {
            break;
        }
    }
    value
}

fn nodes_with_tag(bytes: &[u8], tag: u8) -> Vec<u32> {
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    file.ast_address_index()
        .unwrap()
        .iter_nodes_with_tag(tag)
        .map(|node| u32::try_from(node.offset).unwrap())
        .collect()
}

#[test]
fn a_direct_reference_resolves_to_the_symbol_at_its_target_address() {
    assert_eq!(
        nodes_with_tag(DISTINCT, TYPEREFDIRECT_TAG)[0],
        TYPE_PARAMETER_REFERENCE
    );
    let target = target_of(DISTINCT, TYPE_PARAMETER_REFERENCE as usize);

    let (ty, store, index) = with_unpickler(DISTINCT, |unpickler| {
        unpickler.unpickle_type(TYPE_PARAMETER_REFERENCE).unwrap()
    });

    let (prefix, symbol, is_type) = reference(&store, ty);
    assert!(is_type);
    assert_eq!(Some(symbol), index.symbol_at(target));
    assert_eq!(name(&store, symbol), "T");
    assert_eq!(store.symbols.get(symbol).kind, SymbolKind::TypeParameter);
    // A direct reference names a local symbol and has no prefix on the wire.
    assert_eq!(store.types.get(prefix), &Type::NoPrefix);
}

/// The address of a `VALDEF` or `DEFDEF`, which pass 1 enters as a
/// term-namespace symbol, below 128 so that it is a one-byte natural number.
fn small_term_definition() -> u32 {
    let mut candidates = nodes_with_tag(DISTINCT, 129);
    candidates.extend(nodes_with_tag(DISTINCT, 130));
    candidates
        .into_iter()
        .filter(|&at| at < 128)
        .min()
        .expect("a term definition below address 128")
}

/// The two-byte natural number for `address` (below 16384).
fn nat2(address: u32) -> [u8; 2] {
    [(address >> 7) as u8, 0x80 | (address & 0x7f) as u8]
}

#[test]
fn a_direct_term_reference_is_a_term_ref_with_no_prefix() {
    // TERMREFdirect and TYPEREFdirect have the same wire shape. Point the
    // compiler's reference at a term definition and change the tag.
    let term = small_term_definition();
    let [hi, lo] = nat2(term);
    let bytes = patched(
        DISTINCT,
        TYPE_PARAMETER_REFERENCE as usize,
        &[TYPEREFDIRECT_TAG, 2, 207],
        &[TERMREFDIRECT_TAG, hi, lo],
    );

    let (ty, store, index) = with_unpickler(&bytes, |unpickler| {
        unpickler.unpickle_type(TYPE_PARAMETER_REFERENCE).unwrap()
    });

    let (prefix, symbol, is_type) = reference(&store, ty);
    assert!(!is_type);
    assert_eq!(Some(symbol), index.symbol_at(term));
    assert_eq!(store.types.get(prefix), &Type::NoPrefix);
}

#[test]
fn a_type_reference_to_a_term_symbol_is_rejected() {
    let term = small_term_definition();
    let [hi, lo] = nat2(term);
    let bytes = patched(
        DISTINCT,
        TYPE_PARAMETER_REFERENCE as usize,
        &[TYPEREFDIRECT_TAG, 2, 207],
        &[TYPEREFDIRECT_TAG, hi, lo],
    );

    let (result, _, index) = with_unpickler(&bytes, |unpickler| {
        unpickler.unpickle_type(TYPE_PARAMETER_REFERENCE)
    });

    assert_eq!(
        result,
        Err(UnpickleError::InvalidReferenceKind {
            from: TYPE_PARAMETER_REFERENCE,
            to: term
        })
    );
    assert_eq!(index.type_count(), 0);
}

#[test]
fn a_term_reference_to_a_type_symbol_is_rejected() {
    // The reference to the type parameter `T`, retagged as a term reference.
    let bytes = patched(
        DISTINCT,
        TYPE_PARAMETER_REFERENCE as usize,
        &[TYPEREFDIRECT_TAG],
        &[TERMREFDIRECT_TAG],
    );

    let (result, _, _) = with_unpickler(&bytes, |unpickler| {
        unpickler.unpickle_type(TYPE_PARAMETER_REFERENCE)
    });

    assert!(matches!(
        result,
        Err(UnpickleError::InvalidReferenceKind {
            from: TYPE_PARAMETER_REFERENCE,
            ..
        })
    ));
}

#[test]
fn a_type_symbol_reference_to_a_term_symbol_is_rejected() {
    let term = small_term_definition();
    let bytes = patched(
        DISTINCT,
        REFERENCE_WITH_THIS_PREFIX as usize,
        &[TYPEREFSYMBOL_TAG, 234],
        &[TYPEREFSYMBOL_TAG, 0x80 | term as u8],
    );

    let (result, _, index) = with_unpickler(&bytes, |unpickler| {
        unpickler.unpickle_type(REFERENCE_WITH_THIS_PREFIX)
    });

    assert_eq!(
        result,
        Err(UnpickleError::InvalidReferenceKind {
            from: REFERENCE_WITH_THIS_PREFIX,
            to: term
        })
    );
    // Its `THIS` prefix was decoded first and is taken back.
    assert_eq!(index.type_count(), 0);
}

#[test]
fn a_term_symbol_reference_to_a_type_symbol_is_rejected() {
    // A reference to `one` (TERMREFsymbol) retargeted at a class.
    let refs = nodes_with_tag(DISTINCT, TERMREFSYMBOL_TAG);
    let class = *nodes_with_tag(DISTINCT, 131)
        .iter()
        .find(|&&at| at < 128)
        .expect("a type definition below address 128");
    let at = refs
        .into_iter()
        .find(|&at| {
            let file = TastyFile::parse_scala_3_9(DISTINCT).unwrap();
            let payload = file.section(StandardSection::Asts).unwrap().payload;
            payload[at as usize + 1] & 0x80 != 0
        })
        .expect("a TERMREFsymbol with a one-byte target");
    let target = target_of(DISTINCT, at as usize);
    let bytes = patched(
        DISTINCT,
        at as usize,
        &[TERMREFSYMBOL_TAG, 0x80 | target as u8],
        &[TERMREFSYMBOL_TAG, 0x80 | class as u8],
    );

    let (result, _, _) = with_unpickler(&bytes, |unpickler| unpickler.unpickle_type(at));

    assert!(matches!(
        result,
        Err(UnpickleError::InvalidReferenceKind { from, to }) if from == at && to == class
    ));
}

#[test]
fn this_of_a_term_symbol_is_rejected() {
    // 133 is `TYPEREFsymbol(Inner, THIS(TYPEREFsymbol(Left)))`; the `THIS`
    // argument at 136 is retargeted at a term definition.
    let term = small_term_definition();
    let bytes = patched(
        DISTINCT,
        136,
        &[TYPEREFSYMBOL_TAG, 212],
        &[TYPEREFSYMBOL_TAG, 0x80 | term as u8],
    );

    let (result, _, _) = with_unpickler(&bytes, |unpickler| {
        unpickler.unpickle_type(REFERENCE_WITH_THIS_PREFIX)
    });

    assert_eq!(
        result,
        Err(UnpickleError::InvalidReferenceKind {
            from: 136,
            to: term
        })
    );
}

#[test]
fn a_symbol_reference_to_a_stable_val_is_a_term_ref_with_its_decoded_prefix() {
    // `one.type` in `def use: one.type` is a `TERMREFsymbol` to `val one`.
    let refs = nodes_with_tag(DISTINCT, TERMREFSYMBOL_TAG);
    let (decoded, store, index) = with_unpickler(DISTINCT, |unpickler| {
        refs.iter()
            .filter_map(|&at| Some((at, unpickler.unpickle_type(at).ok()?)))
            .collect::<Vec<_>>()
    });

    let (at, ty) = decoded
        .into_iter()
        .find(|&(_, ty)| name(&store, reference(&store, ty).1) == "one")
        .expect("a reference to `one` decodes");
    let target = target_of(DISTINCT, at as usize);
    let (prefix, symbol, is_type) = reference(&store, ty);
    assert!(!is_type);
    assert_eq!(Some(symbol), index.symbol_at(target));
    assert_eq!(store.symbols.get(symbol).kind, SymbolKind::Field);
    // The prefix is `Distinct.this` (the module class), decoded from the tree rather than
    // dropped or replaced by `NoPrefix`.
    let Type::ThisType { class } = store.types.get(prefix) else {
        panic!("the prefix of `one` is `Distinct.this`");
    };
    assert_eq!(name(&store, *class), "Distinct$");
    assert_eq!(store.symbols.get(*class).kind, SymbolKind::ModuleClass);
    assert_eq!(Some(*class), store.symbols.get(symbol).owner);
}

#[test]
fn references_to_same_named_classes_resolve_by_address_not_by_name() {
    // Every `TYPEREFsymbol` to an `Inner` has a prefix naming the class that
    // owns that `Inner`. Resolving `Inner` by name would send some of them to
    // the wrong class.
    let refs = nodes_with_tag(DISTINCT, TYPEREFSYMBOL_TAG);
    let (inners, store, _) = with_unpickler(DISTINCT, |unpickler| {
        refs.iter()
            .filter_map(|&at| unpickler.unpickle_type(at).ok())
            .collect::<Vec<_>>()
    });

    let mut owners_seen = Vec::new();
    for ty in inners {
        let (prefix, symbol, _) = reference(&store, ty);
        if name(&store, symbol) != "Inner" {
            continue;
        }
        let owner = store.symbols.get(symbol).owner.expect("Inner has an owner");
        let Type::ThisType { class } = store.types.get(prefix) else {
            panic!("the prefix of Inner is `Owner.this`");
        };
        assert_eq!(*class, owner, "prefix must be the owner of Inner");
        owners_seen.push(name(&store, owner).to_owned());
    }
    owners_seen.sort();
    owners_seen.dedup();
    assert_eq!(owners_seen, ["Left", "Right"]);
}

#[test]
fn decoding_an_address_twice_returns_the_same_type_id_and_allocates_once() {
    let ((first, second, count), _, index) = with_unpickler(DISTINCT, |unpickler| {
        let first = unpickler.unpickle_type(FIRST_REFERENCE).unwrap();
        let before = unpickler.index().type_count();
        let second = unpickler.unpickle_type(FIRST_REFERENCE).unwrap();
        (first, second, (before, unpickler.index().type_count()))
    });

    assert_eq!(first, second);
    assert_eq!(count.0, count.1);
    assert_eq!(index.type_at(FIRST_REFERENCE), Some(first));
}

#[test]
fn a_shared_type_is_the_exact_type_id_of_its_target() {
    assert_eq!(
        target_of(DISTINCT, SHARED_TO_FIRST as usize),
        FIRST_REFERENCE
    );

    let ((original, shared, count), _, index) = with_unpickler(DISTINCT, |unpickler| {
        let original = unpickler.unpickle_type(FIRST_REFERENCE).unwrap();
        let before = unpickler.index().type_count();
        let shared = unpickler.unpickle_type(SHARED_TO_FIRST).unwrap();
        (original, shared, (before, unpickler.index().type_count()))
    });

    assert_eq!(shared, original);
    // The link allocated nothing and has no entry of its own.
    assert_eq!(count.0, count.1);
    assert_eq!(index.type_at(SHARED_TO_FIRST), None);
}

#[test]
fn a_shared_type_decoded_before_its_target_still_shares_the_target_id() {
    let ((shared, original), _, _) = with_unpickler(DISTINCT, |unpickler| {
        let shared = unpickler.unpickle_type(SHARED_TO_FIRST).unwrap();
        let original = unpickler.unpickle_type(FIRST_REFERENCE).unwrap();
        (shared, original)
    });

    assert_eq!(shared, original);
}

#[test]
fn every_shared_type_that_decodes_is_the_id_of_its_target() {
    let links = nodes_with_tag(DISTINCT, SHARED_TAG);
    assert!(links.len() > 20, "the unit has many SHAREDtype nodes");

    let (decoded, _, _) = with_unpickler(DISTINCT, |unpickler| {
        let mut decoded = 0;
        for &link in &links {
            let target = target_of(DISTINCT, link as usize);
            if let Ok(shared) = unpickler.unpickle_type(link) {
                assert_eq!(
                    unpickler.unpickle_type(target),
                    Ok(shared),
                    "link at {link}"
                );
                decoded += 1;
            }
        }
        decoded
    });
    assert!(decoded > 5);
}

#[test]
fn a_chain_of_shared_links_resolves_to_the_final_target() {
    // 49 -> 22 -> 10.
    let bytes = patched(
        DISTINCT,
        SHARED_SHORT as usize,
        &[SHARED_TAG, 0x80 | 12],
        &[SHARED_TAG, 0x80 | 22],
    );

    let ((chain, original), _, _) = with_unpickler(&bytes, |unpickler| {
        let chain = unpickler.unpickle_type(SHARED_SHORT).unwrap();
        let original = unpickler.unpickle_type(FIRST_REFERENCE).unwrap();
        (chain, original)
    });

    assert_eq!(chain, original);
}

#[test]
fn a_shared_link_to_itself_is_an_error_not_a_hang() {
    let bytes = patched(
        DISTINCT,
        SHARED_SHORT as usize,
        &[SHARED_TAG, 0x80 | 12],
        &[SHARED_TAG, 0x80 | 49],
    );

    let (result, _, index) =
        with_unpickler(&bytes, |unpickler| unpickler.unpickle_type(SHARED_SHORT));

    assert_eq!(
        result,
        Err(UnpickleError::InvalidReferenceTarget { from: 49, to: 49 })
    );
    assert_eq!(index.type_count(), 0);
}

#[test]
fn two_shared_links_to_each_other_are_an_error_not_a_hang() {
    // 49 -> 22 -> 49.
    let cycle = patched(
        DISTINCT,
        SHARED_SHORT as usize,
        &[SHARED_TAG, 0x80 | 12],
        &[SHARED_TAG, 0x80 | 22],
    );
    let cycle = patched(
        &cycle,
        SHARED_TO_FIRST as usize,
        &[SHARED_TAG, 0x80 | 10],
        &[SHARED_TAG, 0x80 | 49],
    );

    let (result, _, _) = with_unpickler(&cycle, |unpickler| unpickler.unpickle_type(22));

    assert!(matches!(
        result,
        Err(UnpickleError::InvalidReferenceTarget { .. })
    ));
}

#[test]
fn a_shared_link_into_the_middle_of_a_node_is_rejected() {
    // Address 11 is the target field of the node at 10, not a node.
    let bytes = patched(
        DISTINCT,
        SHARED_SHORT as usize,
        &[SHARED_TAG, 0x80 | 12],
        &[SHARED_TAG, 0x80 | 11],
    );

    let (result, _, _) = with_unpickler(&bytes, |unpickler| unpickler.unpickle_type(SHARED_SHORT));

    assert_eq!(
        result,
        Err(UnpickleError::InvalidReferenceTarget { from: 49, to: 11 })
    );
}

#[test]
fn a_shared_link_past_the_end_of_the_section_is_rejected() {
    let bytes = patched(
        DISTINCT,
        SHARED_LONG as usize,
        &[SHARED_TAG, 3, 0x80 | 50],
        &[SHARED_TAG, 0x7f, 0xff],
    );

    let (result, _, _) = with_unpickler(&bytes, |unpickler| unpickler.unpickle_type(SHARED_LONG));

    assert_eq!(
        result,
        Err(UnpickleError::InvalidReferenceTarget {
            from: SHARED_LONG,
            to: 16383
        })
    );
}

#[test]
fn a_type_address_that_is_not_a_node_is_rejected() {
    let (inside, _, _) = with_unpickler(DISTINCT, |unpickler| unpickler.unpickle_type(11));
    let (outside, _, _) = with_unpickler(DISTINCT, |unpickler| unpickler.unpickle_type(100_000));

    assert_eq!(
        inside,
        Err(UnpickleError::InvalidReferenceTarget { from: 11, to: 11 })
    );
    assert_eq!(
        outside,
        Err(UnpickleError::InvalidReferenceTarget {
            from: 100_000,
            to: 100_000
        })
    );
}

#[test]
fn a_reference_to_a_visible_node_without_a_symbol_is_a_missing_symbol_error() {
    // `Hidden` is a local class, which pass 1 does not enter.
    let (result, _, _) = with_unpickler(DISTINCT, |unpickler| {
        unpickler.unpickle_type(LOCAL_CLASS_REFERENCE)
    });

    assert!(matches!(
        result,
        Err(UnpickleError::MissingReferencedSymbol {
            from: LOCAL_CLASS_REFERENCE,
            ..
        })
    ));
}

#[test]
fn a_reference_to_an_address_that_is_not_a_node_is_an_invalid_target() {
    // [63, 2, 207] names 335; [63, 0, 139] names address 11, the target
    // field of the node at 10.
    let bytes = patched(
        DISTINCT,
        TYPE_PARAMETER_REFERENCE as usize,
        &[TYPEREFDIRECT_TAG, 2, 207],
        &[TYPEREFDIRECT_TAG, 0, 0x80 | 11],
    );

    let (result, _, _) = with_unpickler(&bytes, |unpickler| {
        unpickler.unpickle_type(TYPE_PARAMETER_REFERENCE)
    });

    assert_eq!(
        result,
        Err(UnpickleError::InvalidReferenceTarget {
            from: TYPE_PARAMETER_REFERENCE,
            to: 11
        })
    );
}

#[test]
fn name_based_and_unmodelled_types_are_explicitly_unsupported() {
    // The compiler writes a reference to `java.lang.Object` (in another
    // unit) with the name-based TYPEREF (117), which needs the resolver.
    let by_name = nodes_with_tag(DISTINCT, 117);
    let annotation = nodes_with_tag(DISTINCT, 173);
    assert!(!by_name.is_empty() && !annotation.is_empty());

    let (results, _, index) = with_unpickler(DISTINCT, |unpickler| {
        let mut results = vec![
            unpickler.unpickle_type(by_name[0]),
            unpickler.unpickle_type(annotation[0]),
        ];
        // A definition is not a type.
        results.push(unpickler.unpickle_type(0));
        results
    });

    assert_eq!(
        results[0],
        Err(UnpickleError::UnsupportedType {
            tag: 117,
            address: by_name[0]
        })
    );
    assert_eq!(
        results[1],
        Err(UnpickleError::UnsupportedType {
            tag: 173,
            address: annotation[0]
        })
    );
    assert!(matches!(
        results[2],
        Err(UnpickleError::UnsupportedType {
            tag: 128,
            address: 0
        })
    ));
    assert_eq!(index.type_count(), 0);
}

/// The `TERMREFpkg` nodes of the unit that name an entered package, and
/// those that do not.
fn package_references() -> (Vec<u32>, Vec<u32>) {
    let (mut resolved, mut unresolved) = (Vec::new(), Vec::new());
    for at in nodes_with_tag(DISTINCT, 64) {
        let (result, _, _) = with_unpickler(DISTINCT, |unpickler| unpickler.unpickle_type(at));
        match result {
            Ok(_) => resolved.push(at),
            Err(UnpickleError::UnresolvedPackage { .. }) => unresolved.push(at),
            Err(other) => panic!("unexpected error at {at}: {other:?}"),
        }
    }
    (resolved, unresolved)
}

#[test]
fn a_term_package_reference_resolves_to_the_entered_package_symbol() {
    let (resolved, _) = package_references();
    assert!(!resolved.is_empty());

    for at in resolved {
        let (ty, store, _) =
            with_unpickler(DISTINCT, |unpickler| unpickler.unpickle_type(at).unwrap());
        let (prefix, symbol, is_type) = reference(&store, ty);
        assert!(!is_type, "TERMREFpkg at {at}");
        assert_eq!(store.symbols.get(symbol).kind, SymbolKind::Package);
        assert_eq!(store.types.get(prefix), &Type::NoPrefix);
    }
}

#[test]
fn a_type_package_reference_resolves_to_the_entered_package_symbol() {
    // TYPEREFpkg (65) has the wire shape of TERMREFpkg (64).
    let (resolved, _) = package_references();
    // Not the path of the unit's own `PACKAGE`, which pass 1 must still read.
    let at = *resolved.last().unwrap();
    assert!(at > 10);
    let bytes = patched(DISTINCT, at as usize, &[TERMREFPKG_TAG], &[TYPEREFPKG_TAG]);

    let (ty, store, _) = with_unpickler(&bytes, |unpickler| unpickler.unpickle_type(at).unwrap());

    let (prefix, symbol, is_type) = reference(&store, ty);
    assert!(is_type);
    assert_eq!(store.symbols.get(symbol).kind, SymbolKind::Package);
    assert_eq!(store.types.get(prefix), &Type::NoPrefix);
}

#[test]
fn an_unentered_term_package_is_unresolved_not_created() {
    let (_, unresolved) = package_references();
    assert!(!unresolved.is_empty());

    let ((result, symbols), _, _) = with_unpickler(DISTINCT, |unpickler| {
        let symbols = unpickler.index().symbol_count();
        (unpickler.unpickle_type(unresolved[0]), symbols)
    });

    assert!(matches!(
        result,
        Err(UnpickleError::UnresolvedPackage { address, ref package })
            if address == unresolved[0] && !package.is_empty()
    ));
    assert!(symbols > 0);
}

#[test]
fn an_unentered_type_package_is_unresolved_not_created() {
    let (_, unresolved) = package_references();
    let at = unresolved[0];
    let bytes = patched(DISTINCT, at as usize, &[TERMREFPKG_TAG], &[TYPEREFPKG_TAG]);

    let (result, _, index) = with_unpickler(&bytes, |unpickler| unpickler.unpickle_type(at));

    assert!(matches!(
        result,
        Err(UnpickleError::UnresolvedPackage { address, .. }) if address == at
    ));
    assert_eq!(index.type_count(), 0);
}

#[test]
fn a_failed_decode_takes_back_everything_it_allocated() {
    // The node at 182 decodes its `THIS` prefix (allocating a type and
    // recording its address) and only then fails to resolve its target, which
    // is patched to the symbol-less node at 47.
    let bytes = patched(
        DISTINCT,
        REFERENCE_WITH_THIS_PREFIX as usize,
        &[TYPEREFSYMBOL_TAG, 234],
        &[TYPEREFSYMBOL_TAG, 0x80 | 47],
    );
    assert_eq!(NODE_WITHOUT_SYMBOL, 47);

    // A run that never sees the failure, for comparison.
    let (control, control_store, _) = with_unpickler(&bytes, |unpickler| {
        let existing = unpickler.unpickle_type(FIRST_REFERENCE).unwrap();
        let later = unpickler.unpickle_type(LATER_REFERENCE).unwrap();
        (existing, later)
    });

    let ((existing, later), store, index) = with_unpickler(&bytes, |unpickler| {
        let existing = unpickler.unpickle_type(FIRST_REFERENCE).unwrap();
        let types_before = unpickler.index().type_count();
        let symbols_before = unpickler.index().symbol_count();

        let failure = unpickler.unpickle_type(REFERENCE_WITH_THIS_PREFIX);
        assert!(matches!(
            failure,
            Err(UnpickleError::MissingReferencedSymbol {
                from: REFERENCE_WITH_THIS_PREFIX,
                to: 47
            })
        ));

        // The prefix decoded before the failure is not reachable any more.
        assert_eq!(unpickler.index().type_count(), types_before);
        assert_eq!(unpickler.index().symbol_count(), symbols_before);
        assert_eq!(unpickler.index().type_at(184), None);
        assert_eq!(unpickler.index().type_at(REFERENCE_WITH_THIS_PREFIX), None);

        // Ids freed by the rollback are handed out again, exactly as in the
        // run that never failed.
        let later = unpickler.unpickle_type(LATER_REFERENCE).unwrap();
        (existing, later)
    });

    assert_eq!((existing, later), control);
    assert_eq!(store.checkpoint(), control_store.checkpoint());
    assert_eq!(index.type_at(FIRST_REFERENCE), Some(existing));
}

#[test]
fn type_decoding_does_not_disturb_entered_symbols_or_scopes() {
    let file = TastyFile::parse_scala_3_9(DISTINCT).unwrap();
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut unpickler = TastyUnpickler::new(&file, &mut store, definitions);
    unpickler.enter_symbols().unwrap();
    let symbols = unpickler.index().symbol_count();

    let _ = unpickler.unpickle_type(FIRST_REFERENCE);
    let _ = unpickler.unpickle_type(LOCAL_CLASS_REFERENCE);

    assert_eq!(unpickler.index().symbol_count(), symbols);
}

#[test]
fn every_reference_without_a_prefix_reuses_the_sessions_canonical_no_prefix() {
    let (resolved, _) = package_references();
    let type_package_at = *resolved.last().unwrap();
    let term_package_at = *resolved.first().unwrap();
    assert_ne!(type_package_at, term_package_at);
    let bytes = patched(
        DISTINCT,
        type_package_at as usize,
        &[TERMREFPKG_TAG],
        &[TYPEREFPKG_TAG],
    );

    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut unpickler = TastyUnpickler::new(&file, &mut store, definitions);
    unpickler.enter_symbols().unwrap();

    // TYPEREFdirect, TYPEREFpkg and TERMREFpkg: three wire forms, one session.
    let decoded = [
        unpickler.unpickle_type(TYPE_PARAMETER_REFERENCE).unwrap(),
        unpickler.unpickle_type(type_package_at).unwrap(),
        unpickler.unpickle_type(term_package_at).unwrap(),
    ];
    drop(unpickler);

    for ty in decoded {
        let (prefix, _, _) = reference(&store, ty);
        // The `TypeId` itself, not just an equal-looking `NoPrefix`.
        assert_eq!(prefix, definitions.no_prefix);
    }
}
