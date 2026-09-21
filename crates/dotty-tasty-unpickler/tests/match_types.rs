//! Milestone 4d: `MATCHtype` and `MATCHCASEtype`.
//!
//! Small synthetic wire files: no real Scala 3.9.0 output holds either tag
//! (`MATCHtpt`, the source syntax, is a tree and is not decoded here).
use dotty_core::Definitions;
use dotty_core::Packages;
use dotty_core::ids::TypeId;
use dotty_core::store::SemanticStore;
use dotty_core::symbols::SymbolOrigin;
use dotty_core::types::{MatchType, Type};
use dotty_tasty::tasty::TastyFile;
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

const MATCH: u8 = 190;
const MATCHCASE: u8 = 192;
const APPLIED: u8 = 161;
const SHARED: u8 = 61;
const TYPEREFPKG: u8 = 65;
const LAMBDA: u8 = 170;
const PARAM: u8 = 172;
const TYPEBOUNDS: u8 = 163;
const COVARIANT: u8 = 28;
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

fn file_with_ast(ast: &[u8]) -> Vec<u8> {
    use dotty_tasty::tasty::{Header, NameTable, RawName, Section, SectionTable};
    let names = NameTable::from_entries(vec![
        RawName::Utf8("ASTs".to_owned()),
        RawName::Utf8("p".to_owned()),
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

/// `TYPEREFpkg p`, two bytes: every distinct address is a distinct type.
fn package_ref() -> Vec<u8> {
    vec![TYPEREFPKG, nat(1)]
}

fn shared(target: u32) -> Vec<u8> {
    vec![SHARED, nat(u8::try_from(target).unwrap())]
}

/// `PARAMtype Length binder_ASTRef paramNum_Nat`, four bytes.
fn param_type(binder: u32, number: u8) -> Vec<u8> {
    length_node(PARAM, &[nat(u8::try_from(binder).unwrap()), nat(number)])
}

fn match_case(pattern: &[u8], result: &[u8]) -> Vec<u8> {
    length_node(MATCHCASE, &[pattern, result].concat())
}

fn match_type(bound: &[u8], scrutinee: &[u8], cases: &[Vec<u8>]) -> Vec<u8> {
    let mut payload = [bound, scrutinee].concat();
    for case in cases {
        payload.extend(case);
    }
    length_node(MATCH, &payload)
}

/// `[p] =>> result` with one unbounded parameter.
fn lambda(result: &[u8]) -> Vec<u8> {
    let mut payload = result.to_vec();
    payload.extend(length_node(TYPEBOUNDS, &package_ref()));
    payload.push(nat(1));
    length_node(LAMBDA, &payload)
}

/// A file with a `APPLIEDtype` holder whose children are `children`; the
/// first child is at address 2.
fn holder(children: &[Vec<u8>]) -> Vec<u8> {
    file_with_ast(&length_node(APPLIED, &children.concat()))
}

fn unpickler_for<'a>(
    file: &'a TastyFile<'a>,
    session: &'a mut Session,
) -> TastyUnpickler<'a, 'a, 'a> {
    let mut packages = Packages::new();
    packages.enter(&mut session.store, SymbolOrigin::Synthetic, &["p"]);
    TastyUnpickler::with_packages(file, &mut session.store, session.definitions, packages)
}

fn decode(bytes: &[u8], at: u32) -> (Session, Result<TypeId, UnpickleError>) {
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let mut session = Session::new();
    let result = {
        let mut unpickler = unpickler_for(&file, &mut session);
        unpickler.unpickle_type(at)
    };
    (session, result)
}

fn match_of(store: &SemanticStore, id: TypeId) -> MatchType {
    match store.types.get(id) {
        Type::Match(m) => m.clone(),
        other => panic!("not a match type: {other:?}"),
    }
}

fn case_of(store: &SemanticStore, id: TypeId) -> (TypeId, TypeId) {
    match store.types.get(id) {
        Type::MatchCase { pattern, result } => (*pattern, *result),
        other => panic!("not a match case: {other:?}"),
    }
}

/// The ids the next type allocation would get.
fn next_type(session: &mut Session) -> u32 {
    session.store.types.alloc(Type::NoType).index()
}

// `holder` children start at 2. Each `package_ref` is 2 bytes.

#[test]
fn a_direct_case_keeps_its_pattern_and_result_in_wire_order() {
    // Pattern at 4, result at 6.
    let bytes = holder(&[match_case(&package_ref(), &package_ref())]);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    let id = unpickler.unpickle_type(2).unwrap();
    let pattern = unpickler.index().type_at(4).unwrap();
    let result = unpickler.index().type_at(6).unwrap();
    drop(unpickler);

    assert_ne!(pattern, result);
    assert_eq!(case_of(&session.store, id), (pattern, result));
}

#[test]
fn a_match_type_keeps_bound_scrutinee_and_case_order() {
    let cases = [
        match_case(&package_ref(), &package_ref()),
        match_case(&package_ref(), &package_ref()),
    ];
    // Match at 2; bound 4, scrutinee 6, cases 8 and 14.
    let bytes = holder(&[match_type(&package_ref(), &package_ref(), &cases)]);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    let id = unpickler.unpickle_type(2).unwrap();
    let (bound, scrutinee, first, second) = (
        unpickler.index().type_at(4).unwrap(),
        unpickler.index().type_at(6).unwrap(),
        unpickler.index().type_at(8).unwrap(),
        unpickler.index().type_at(14).unwrap(),
    );
    drop(unpickler);

    let decoded = match_of(&session.store, id);
    // Bound and scrutinee are different fields, and a case is the exact
    // `MatchCase` node, never re-derived.
    assert_ne!(bound, scrutinee);
    assert_eq!(decoded.bound, bound);
    assert_eq!(decoded.scrutinee, scrutinee);
    assert_eq!(decoded.cases, vec![first, second]);
    assert!(matches!(
        session.store.types.get(first),
        Type::MatchCase { .. }
    ));
}

#[test]
fn a_match_type_with_no_case_is_accepted() {
    // The grammar is `CaseType*` and upstream passes the list on as it is.
    let bytes = holder(&[match_type(&package_ref(), &package_ref(), &[])]);
    let (session, result) = decode(&bytes, 2);

    assert!(match_of(&session.store, result.unwrap()).cases.is_empty());
}

#[test]
fn a_match_type_short_of_its_scrutinee_is_a_structural_error() {
    let bytes = holder(&[length_node(MATCH, &package_ref())]);
    let (_, result) = decode(&bytes, 2);

    assert!(result.is_err(), "{result:?}");
}

#[test]
fn a_case_with_one_child_is_a_structural_error() {
    let bytes = holder(&[length_node(MATCHCASE, &package_ref())]);
    let (_, result) = decode(&bytes, 2);

    assert!(result.is_err(), "{result:?}");
}

#[test]
fn a_case_is_not_looked_up_as_a_match_case_class() {
    // No `MatchCase` symbol exists in these stores; decoding needs none.
    let bytes = holder(&[match_case(&package_ref(), &package_ref())]);
    let (session, result) = decode(&bytes, 2);

    assert!(matches!(
        session.store.types.get(result.unwrap()),
        Type::MatchCase { .. }
    ));
}

// Captured cases

/// `X match { case [p] =>> MatchCase(p, p) }`: the match at 2 (bound 4,
/// scrutinee 6), the case lambda at 8, its result at 10, both parts of which
/// are parameter types naming address 8.
fn captured_file() -> Vec<u8> {
    let case = match_case(&param_type(8, 0), &param_type(8, 0));
    holder(&[match_type(&package_ref(), &package_ref(), &[lambda(&case)])])
}

#[test]
fn a_captured_case_is_a_type_lambda_whose_result_is_a_match_case() {
    let bytes = captured_file();
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    let id = unpickler.unpickle_type(2).unwrap();
    let lambda_id = unpickler.index().type_at(8).unwrap();
    let case_id = unpickler.index().type_at(10).unwrap();
    drop(unpickler);

    assert_eq!(match_of(&session.store, id).cases, vec![lambda_id]);
    let Type::TypeLambda(lambda) = session.store.types.get(lambda_id) else {
        panic!("not a type lambda");
    };
    assert_eq!(lambda.result, case_id);
    let (pattern, result) = case_of(&session.store, case_id);
    // Both captures name that exact lambda.
    for part in [pattern, result] {
        assert_eq!(
            session.store.types.get(part),
            &Type::ParamRef {
                binder: lambda_id,
                index: 0
            }
        );
    }
}

#[test]
fn a_captured_case_decoded_from_its_lambda_first_builds_one_graph() {
    let bytes = captured_file();
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    let lambda_id = unpickler.unpickle_type(8).unwrap();
    let id = unpickler.unpickle_type(2).unwrap();
    assert_eq!(unpickler.index().type_at(8), Some(lambda_id));
    drop(unpickler);

    assert_eq!(match_of(&session.store, id).cases, vec![lambda_id]);
}

#[test]
fn decoding_the_same_match_again_allocates_nothing() {
    let bytes = captured_file();
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    let id = unpickler.unpickle_type(2).unwrap();
    let count = unpickler.index().type_count();
    assert_eq!(unpickler.unpickle_type(2), Ok(id));
    assert_eq!(unpickler.index().type_count(), count);
    drop(unpickler);
    let after = next_type(&mut session);
    let mut again = Session::new();
    let mut unpickler = unpickler_for(&file, &mut again);
    unpickler.unpickle_type(2).unwrap();
    drop(unpickler);
    assert_eq!(after, next_type(&mut again));
}

// Sharing

#[test]
fn a_shared_link_to_a_match_or_a_case_returns_the_exact_id_in_both_orders() {
    // The match at 2 (length 2 + 6 + case), then a link to it; a case, and a
    // link to it.
    let case = match_case(&package_ref(), &package_ref());
    let the_match = match_type(&package_ref(), &package_ref(), std::slice::from_ref(&case));
    let match_len = u32::try_from(the_match.len()).unwrap();
    let link_to_match = 2 + match_len;
    let case_at = link_to_match + 2;
    let case_len = u32::try_from(case.len()).unwrap();
    let link_to_case = case_at + case_len;
    let bytes = holder(&[the_match, shared(2), case, shared(case_at)]);

    for (target, link) in [(2, link_to_match), (case_at, link_to_case)] {
        // The target first, then the link.
        let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
        let mut session = Session::new();
        let mut unpickler = unpickler_for(&file, &mut session);
        let id = unpickler.unpickle_type(target).unwrap();
        let count = unpickler.index().type_count();
        assert_eq!(unpickler.unpickle_type(link), Ok(id));
        assert_eq!(unpickler.index().type_count(), count);
        drop(unpickler);

        // The link first, then the target.
        let mut session = Session::new();
        let mut unpickler = unpickler_for(&file, &mut session);
        let linked = unpickler.unpickle_type(link).unwrap();
        assert_eq!(unpickler.unpickle_type(target), Ok(linked));
    }
}

#[test]
fn a_case_entry_may_be_a_shared_link_to_an_existing_case() {
    // A case at 2, then a match at 8 whose only case is a link to it.
    let case = match_case(&package_ref(), &package_ref());
    let case_len = u32::try_from(case.len()).unwrap();
    let the_match = match_type(&package_ref(), &package_ref(), &[shared(2)]);
    let bytes = holder(&[case, the_match]);
    let match_at = 2 + case_len;

    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    let linked = unpickler.unpickle_type(match_at).unwrap();
    let case_id = unpickler.index().type_at(2).unwrap();
    drop(unpickler);
    assert_eq!(match_of(&session.store, linked).cases, vec![case_id]);

    // The case first: still one node.
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    let case_id = unpickler.unpickle_type(2).unwrap();
    let linked = unpickler.unpickle_type(match_at).unwrap();
    drop(unpickler);
    assert_eq!(match_of(&session.store, linked).cases, vec![case_id]);
}

// Rollback

#[test]
fn a_failing_later_case_rolls_back_the_earlier_ones_and_their_binders() {
    // The first case is captured (a lambda, a binder); the second fails.
    let good = lambda(&match_case(&param_type(8, 0), &param_type(8, 0)));
    let bad = vec![UNSUPPORTED, nat(1)];
    let bytes = holder(&[match_type(&package_ref(), &package_ref(), &[good, bad])]);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();

    let mut untouched = Session::new();
    drop(unpickler_for(&file, &mut untouched));
    let expected = next_type(&mut untouched);

    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    let before = unpickler.index().type_count();
    for _ in 0..2 {
        let result = unpickler.unpickle_type(2);
        assert!(
            matches!(result, Err(UnpickleError::UnsupportedType { .. })),
            "{result:?}"
        );
        assert_eq!(unpickler.index().type_count(), before);
        for at in [4, 6, 8, 10] {
            assert_eq!(unpickler.index().type_at(at), None, "{at}");
        }
    }
    drop(unpickler);
    assert_eq!(next_type(&mut session), expected);
}

#[test]
fn a_case_whose_result_fails_rolls_back_its_pattern() {
    let bytes = holder(&[match_case(&package_ref(), &[UNSUPPORTED, nat(1)])]);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();

    let mut untouched = Session::new();
    drop(unpickler_for(&file, &mut untouched));
    let expected = next_type(&mut untouched);

    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    let before = unpickler.index().type_count();
    assert!(unpickler.unpickle_type(2).is_err());
    assert_eq!(unpickler.index().type_count(), before);
    assert_eq!(unpickler.index().type_at(4), None);
    drop(unpickler);
    assert_eq!(next_type(&mut session), expected);
}

// Rebinding

#[test]
fn a_rebound_outer_lambda_remaps_a_match_type_and_keeps_a_nested_case_binder() {
    // `+[p] =>> (p match { case [q] =>> MatchCase(q, p) })`, the outer lambda
    // the alias of a covariant `TYPEBOUNDS` (so it is rebound). Layout: bounds
    // at 2, outer lambda at 4, match at 6 (bound 8, scrutinee 10, case lambda
    // at 14, its case at 16).
    let case = match_case(&param_type(14, 0), &param_type(4, 0));
    let the_match = match_type(&package_ref(), &param_type(4, 0), &[lambda(&case)]);
    let mut payload = the_match;
    payload.extend(length_node(TYPEBOUNDS, &package_ref()));
    payload.push(nat(1));
    let mut bounds = length_node(LAMBDA, &payload);
    bounds.push(COVARIANT);
    let bytes = file_with_ast(&length_node(APPLIED, &length_node(TYPEBOUNDS, &bounds)));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let bounds = unpickler.unpickle_type(2).unwrap();
    let original = unpickler.index().type_at(4).unwrap();
    drop(unpickler);

    let Type::AliasingBounds { alias: derived } = *session.store.types.get(bounds) else {
        panic!("not alias bounds");
    };
    assert_ne!(derived, original);
    let outer = |id: TypeId| match session.store.types.get(id) {
        Type::TypeLambda(lambda) => lambda.result,
        other => panic!("not a type lambda: {other:?}"),
    };
    let param_binder = |id: TypeId| match session.store.types.get(id) {
        Type::ParamRef { binder, .. } => *binder,
        other => panic!("not a parameter reference: {other:?}"),
    };

    // The original still names only itself and its nested binder.
    let old = match_of(&session.store, outer(original));
    assert_eq!(param_binder(old.scrutinee), original);
    // The rebound match names the new outer binder.
    let new = match_of(&session.store, outer(derived));
    assert_eq!(param_binder(new.scrutinee), derived);
    let [new_case_lambda] = new.cases[..] else {
        panic!("one case");
    };
    let [old_case_lambda] = old.cases[..] else {
        panic!("one case");
    };
    assert_ne!(new_case_lambda, old_case_lambda);
    let (pattern, result) = case_of(&session.store, outer(new_case_lambda));
    // The capture names its own (new) case lambda; the outer reference names
    // the new outer binder, never the old one.
    assert_eq!(param_binder(pattern), new_case_lambda);
    assert_eq!(param_binder(result), derived);
    let (old_pattern, old_result) = case_of(&session.store, outer(old_case_lambda));
    assert_eq!(param_binder(old_pattern), old_case_lambda);
    assert_eq!(param_binder(old_result), original);
}
