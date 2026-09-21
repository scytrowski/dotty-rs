//! Milestone 4b1: `ANNOTATEDtype`, compact and full.
//!
//! Small synthetic wire files cover classification, malformed shapes,
//! identity and rollback; real Scala 3.9.0 fixtures
//! (`tests/fixtures/semantic/{Annotations,CaptureChecking,Erased}.scala`) are
//! used further down.
use dotty_core::Definitions;
use dotty_core::Packages;
use dotty_core::ids::{AnnotationId, TypeId};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::SymbolOrigin;
use dotty_core::types::{AnnotationArguments, AnnotationValue, Constant, Type};
use dotty_tasty::tasty::TastyFile;
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

const ANNOTATED: u8 = 153;
const TYPEREFPKG: u8 = 65;

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

/// A file whose ASTs are exactly `ast`, over the names `ASTs`, `p`.
fn file_with_ast(ast: &[u8]) -> Vec<u8> {
    use dotty_tasty::tasty::{Header, NameTable, RawName, Section, SectionTable};
    let names = NameTable::from_entries(vec![
        RawName::Utf8("ASTs".to_owned()),
        RawName::Utf8("p".to_owned()),
        RawName::Utf8("<init>".to_owned()),
        RawName::Utf8("label".to_owned()),
        RawName::Utf8("hello".to_owned()),
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

/// A file whose only top-level node is an `ANNOTATEDtype` at address 0 with
/// the given children, the first at address 2.
fn annotated_file(underlying: &[u8], annotation: &[u8]) -> Vec<u8> {
    let mut payload = underlying.to_vec();
    payload.extend(annotation);
    file_with_ast(&length_node(ANNOTATED, &payload))
}

const ANNOTATED_AT: u32 = 0;
const UNDERLYING_AT: u32 = 2;

fn unpickler_for<'a>(
    file: &'a TastyFile<'a>,
    session: &'a mut Session,
) -> TastyUnpickler<'a, 'a, 'a> {
    let mut packages = Packages::new();
    packages.enter(&mut session.store, SymbolOrigin::Synthetic, &["p"]);
    TastyUnpickler::with_packages(file, &mut session.store, session.definitions, packages)
}

// Classification: the first tag of the annotation payload

#[test]
fn a_full_annotation_tree_is_a_typed_deferral_not_an_unsupported_type() {
    // `TYPEREFpkg` is type-like but not in Scala's compact set, so the payload
    // is a tree; the annotated type itself is understood.
    let annotation_at = UNDERLYING_AT + 2;
    let bytes = annotated_file(&package_ref(), &package_ref());
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let result = unpickler.unpickle_type(ANNOTATED_AT);
    assert_eq!(
        result,
        Err(UnpickleError::UnsupportedAnnotationTree {
            address: ANNOTATED_AT,
            annotation_address: annotation_at,
            tag: TYPEREFPKG,
        })
    );
}

#[test]
fn the_parent_is_decoded_first_so_its_error_wins_over_a_full_tree() {
    // As in Dotty, `readType()` for the parent runs before the annotation is
    // looked at: a parent with no decoder is reported, not the tree.
    const IMPORTED: u8 = 75;
    let bytes = annotated_file(&[IMPORTED, nat(1)], &package_ref());
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    assert_eq!(
        unpickler.unpickle_type(ANNOTATED_AT),
        Err(UnpickleError::UnsupportedType {
            tag: IMPORTED,
            address: UNDERLYING_AT,
        })
    );
}

#[test]
fn a_typerefin_annotation_is_compact_but_its_child_is_still_unsupported() {
    // `TYPEREFin` is in Scala's compact set, and is decoded in Milestone 4c:
    // the annotated type is understood, its annotation child is not (yet), so
    // the error is the child's, never `ANNOTATEDtype`'s.
    const TYPEREFIN: u8 = 175;
    let typerefin = length_node(
        TYPEREFIN,
        &[vec![nat(1)], package_ref(), package_ref()].concat(),
    );
    let bytes = annotated_file(&package_ref(), &typerefin);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    assert_eq!(
        unpickler.unpickle_type(ANNOTATED_AT),
        Err(UnpickleError::UnsupportedType {
            tag: TYPEREFIN,
            address: UNDERLYING_AT + 2,
        })
    );
}

/// The ids the next type and annotation allocations would get: equal for two
/// stores exactly when they hold the same number of each.
fn next_ids(session: &mut Session) -> (u32, u32) {
    use dotty_core::types::{Annotation, Type};
    let ty = session.store.types.alloc(Type::NoType);
    let annotation = session.store.annotations.alloc(Annotation::new(ty, None));
    (ty.index(), annotation.index())
}

#[test]
fn a_full_annotation_tree_rolls_back_the_parent_it_decoded() {
    let bytes = annotated_file(&package_ref(), &package_ref());
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();

    let mut untouched = Session::new();
    drop(unpickler_for(&file, &mut untouched));
    let expected = next_ids(&mut untouched);

    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    let before = unpickler.index().type_count();
    assert!(unpickler.unpickle_type(ANNOTATED_AT).is_err());
    assert_eq!(unpickler.index().type_count(), before);
    assert_eq!(unpickler.index().type_at(UNDERLYING_AT), None);
    drop(unpickler);

    assert_eq!(next_ids(&mut session), expected);
}

#[test]
fn an_annotated_type_with_one_child_is_a_structural_error() {
    let bytes = file_with_ast(&length_node(ANNOTATED, &package_ref()));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let result = unpickler.unpickle_type(ANNOTATED_AT);
    assert!(matches!(result, Err(UnpickleError::Ast(_))), "{result:?}");
}

#[test]
fn an_annotated_type_with_three_children_is_a_structural_error() {
    let mut payload = package_ref();
    payload.extend(package_ref());
    payload.extend(package_ref());
    let bytes = file_with_ast(&length_node(ANNOTATED, &payload));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let result = unpickler.unpickle_type(ANNOTATED_AT);
    assert!(matches!(result, Err(UnpickleError::Ast(_))), "{result:?}");
}

// Compact annotations

const SHARED: u8 = 61;
const FLEXIBLE: u8 = 193;
const APPLIED: u8 = 161;
const POLY: u8 = 169;
const TYPEBOUNDS: u8 = 163;

/// `SHAREDtype target`.
fn shared(target: u8) -> Vec<u8> {
    vec![SHARED, nat(target)]
}

fn annotated_parts(store: &SemanticStore, id: TypeId) -> (TypeId, AnnotationId) {
    match store.types.get(id) {
        Type::Annotated {
            underlying,
            annotation,
        } => (*underlying, *annotation),
        other => panic!("not an annotated type: {other:?}"),
    }
}

/// `p @p`: the annotation is a link to the underlying `TYPEREFpkg` at 2.
fn compact_file() -> Vec<u8> {
    annotated_file(&package_ref(), &shared(2))
}

#[test]
fn a_compact_annotation_type_is_stored_whole_with_no_tree() {
    let bytes = compact_file();
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let id = unpickler.unpickle_type(ANNOTATED_AT).unwrap();
    let package = unpickler.index().type_at(UNDERLYING_AT).unwrap();
    assert_eq!(unpickler.index().type_at(ANNOTATED_AT), Some(id));
    drop(unpickler);

    let (underlying, annotation) = annotated_parts(&session.store, id);
    assert_eq!(underlying, package);
    // The annotation is the type the compact payload names, and only that.
    let annotation = session.store.annotations.get(annotation);
    assert_eq!(annotation.ty, package);
    assert_eq!(annotation.tree, None);
    // Known to have no term arguments, which is not "unavailable".
    assert_eq!(annotation.arguments, AnnotationArguments::Known(Vec::new()));
}

#[test]
fn decoding_an_annotated_address_twice_allocates_one_annotation() {
    let bytes = compact_file();
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let first = unpickler.unpickle_type(ANNOTATED_AT).unwrap();
    assert_eq!(unpickler.unpickle_type(ANNOTATED_AT), Ok(first));
    drop(unpickler);

    let (_, annotation) = annotated_parts(&session.store, first);
    // One annotation and one type for the package and the annotated type each:
    // the next allocations follow directly.
    assert_eq!(annotation.index(), 0);
    let (_, next_annotation) = next_ids(&mut session);
    assert_eq!(next_annotation, 1);
}

#[test]
fn a_shared_link_to_an_annotated_type_returns_its_exact_id() {
    // The annotated type (0..7), then a `FLEXIBLEtype` wrapper at 8 whose one
    // child, at 10, is a link to it.
    let annotated = length_node(ANNOTATED, &[package_ref(), shared(2)].concat());
    let mut ast = annotated.clone();
    ast.extend(length_node(FLEXIBLE, &shared(0)));
    let link_at = u32::try_from(annotated.len() + 2).unwrap();
    let bytes = file_with_ast(&ast);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let id = unpickler.unpickle_type(ANNOTATED_AT).unwrap();
    let before = unpickler.index().type_count();
    assert_eq!(unpickler.unpickle_type(link_at), Ok(id));
    assert_eq!(unpickler.index().type_count(), before);
    drop(unpickler);
    // The link is not the underlying type, and allocated no annotation.
    assert!(matches!(
        session.store.types.get(id),
        Type::Annotated { .. }
    ));
    assert_eq!(next_ids(&mut session).1, 1);

    // A link decoded first builds the one annotated type.
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    let linked = unpickler.unpickle_type(link_at).unwrap();
    assert_eq!(unpickler.unpickle_type(ANNOTATED_AT), Ok(linked));
}

#[test]
fn nested_annotations_keep_their_nesting_order_and_are_not_deduplicated() {
    // `p @a1 @a2`: both annotations link to the one `TYPEREFpkg` (at 4), so
    // they are equal, and still two annotations, in nesting order.
    let inner = length_node(ANNOTATED, &[package_ref(), shared(4)].concat());
    let bytes = annotated_file(&inner, &shared(4));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let outer = unpickler.unpickle_type(ANNOTATED_AT).unwrap();
    let inner_id = unpickler.index().type_at(UNDERLYING_AT).unwrap();
    let package = unpickler.index().type_at(UNDERLYING_AT + 2).unwrap();
    drop(unpickler);

    let (under_outer, second) = annotated_parts(&session.store, outer);
    assert_eq!(under_outer, inner_id);
    let (under_inner, first) = annotated_parts(&session.store, inner_id);
    assert_eq!(under_inner, package);
    assert_ne!(first, second);
    assert!(first.index() < second.index());
    assert_eq!(session.store.annotations.get(first).ty, package);
    assert_eq!(session.store.annotations.get(second).ty, package);
}

#[test]
fn an_applied_annotation_type_is_stored_whole_not_reduced_to_its_constructor() {
    // The annotation payload is `APPLIEDtype tycon arg`, an `Applied` type.
    let applied = length_node(APPLIED, &[package_ref(), package_ref()].concat());
    let bytes = annotated_file(&package_ref(), &applied);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let id = unpickler.unpickle_type(ANNOTATED_AT).unwrap();
    drop(unpickler);

    let (_, annotation) = annotated_parts(&session.store, id);
    let stored = session.store.annotations.get(annotation).ty;
    let Type::Applied { tycon, args } = session.store.types.get(stored) else {
        panic!("not an applied type");
    };
    assert_eq!(args.len(), 1);
    assert_ne!(stored, *tycon);
}

#[test]
fn a_compact_link_to_a_type_that_is_not_a_reference_is_invalid() {
    // A `FLEXIBLEtype` at 0, then the annotated type at 4 whose compact
    // payload links to it: `SHAREDtype` is compact, its target is not valid.
    let flexible = length_node(FLEXIBLE, &package_ref());
    let mut ast = flexible.clone();
    let annotated_at = u32::try_from(flexible.len()).unwrap();
    ast.extend(length_node(ANNOTATED, &[package_ref(), shared(0)].concat()));
    let bytes = file_with_ast(&ast);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();

    let mut untouched = Session::new();
    {
        let mut unpickler = unpickler_for(&file, &mut untouched);
        unpickler.unpickle_type(0).unwrap();
    }
    let expected = next_ids(&mut untouched);

    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    let flexible_id = unpickler.unpickle_type(0).unwrap();
    let before = unpickler.index().type_count();

    let result = unpickler.unpickle_type(annotated_at);
    assert_eq!(
        result,
        Err(UnpickleError::InvalidCompactAnnotationType {
            address: annotated_at,
            annotation_type: flexible_id,
        })
    );
    // What was decoded before the call is intact; what the call added is not.
    assert_eq!(unpickler.index().type_at(0), Some(flexible_id));
    assert_eq!(unpickler.index().type_count(), before);
    assert_eq!(unpickler.index().type_at(annotated_at + 2), None);
    drop(unpickler);
    assert_eq!(next_ids(&mut session), expected);
}

#[test]
fn a_failed_annotated_type_can_be_retried() {
    let flexible = length_node(FLEXIBLE, &package_ref());
    let mut ast = flexible.clone();
    let bad_at = u32::try_from(flexible.len()).unwrap();
    let bad = length_node(ANNOTATED, &[package_ref(), shared(0)].concat());
    ast.extend(&bad);
    let good_at = bad_at + u32::try_from(bad.len()).unwrap();
    ast.extend(length_node(
        ANNOTATED,
        &[package_ref(), shared(u8::try_from(good_at + 2).unwrap())].concat(),
    ));
    let bytes = file_with_ast(&ast);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    assert!(unpickler.unpickle_type(bad_at).is_err());
    assert!(unpickler.unpickle_type(bad_at).is_err());
    let good = unpickler.unpickle_type(good_at).unwrap();
    drop(unpickler);
    let (_, annotation) = annotated_parts(&session.store, good);
    // The failed attempts left no annotation behind.
    assert_eq!(annotation.index(), 0);
}

#[test]
fn a_compact_link_to_a_binder_still_being_decoded_is_invalid_not_a_panic() {
    // `POLYtype` at 0 whose one parameter's alias bound is an annotated type
    // whose compact payload links back to the poly itself.
    let annotated = length_node(ANNOTATED, &[package_ref(), shared(0)].concat());
    let mut param = length_node(TYPEBOUNDS, &annotated);
    param.push(nat(1));
    let poly = length_node(POLY, &[package_ref(), param].concat());
    let bytes = file_with_ast(&poly);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let result = unpickler.unpickle_type(0);
    assert!(
        matches!(
            result,
            Err(UnpickleError::InvalidCompactAnnotationType { .. })
        ),
        "{result:?}"
    );
    assert_eq!(unpickler.index().type_at(0), None);
}

#[test]
fn a_full_new_pointing_at_a_binder_still_being_decoded_is_invalid_not_a_panic() {
    // `POLYtype` at 0 whose alias bound is an annotated type whose `NEW`
    // class tree links back to the poly itself.
    let annotated = length_node(ANNOTATED, &[package_ref(), new_of(&shared(0))].concat());
    let mut param = length_node(TYPEBOUNDS, &annotated);
    param.push(nat(1));
    let poly = length_node(POLY, &[package_ref(), param].concat());
    let bytes = file_with_ast(&poly);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let result = unpickler.unpickle_type(0);
    assert!(
        matches!(result, Err(UnpickleError::InvalidAnnotationType { .. })),
        "{result:?}"
    );
    assert_eq!(unpickler.index().type_at(0), None);
}

#[test]
fn an_annotation_that_links_to_its_own_annotated_type_is_an_error_not_a_stack_overflow() {
    let bytes = annotated_file(&package_ref(), &shared(0));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let result = unpickler.unpickle_type(ANNOTATED_AT);
    assert!(
        matches!(result, Err(UnpickleError::InvalidReferenceTarget { .. })),
        "{result:?}"
    );
    assert_eq!(unpickler.index().type_at(ANNOTATED_AT), None);
}

// Annotated as a member-lookup proxy

const TYPEREF: u8 = 117;

/// `FLEXIBLE (TYPEREF p (ANNOTATED p @p))`: a name-based reference to the
/// member `p` whose prefix is an annotated package type. The reference is at
/// address 2, its prefix at 4.
fn reference_through_annotated_prefix() -> Vec<u8> {
    let prefix = length_node(ANNOTATED, &[package_ref(), shared(6)].concat());
    let mut reference = vec![TYPEREF, nat(1)];
    reference.extend(prefix);
    file_with_ast(&length_node(FLEXIBLE, &reference))
}

#[test]
fn a_member_is_looked_up_through_an_annotated_prefix_that_stays_in_the_graph() {
    let bytes = reference_through_annotated_prefix();
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut packages = Packages::new();
    // The package `p` declares a class `p`, which the reference names.
    enter_stub_classes(&mut session, &mut packages, &["p"], &["p"]);
    let mut unpickler =
        TastyUnpickler::with_packages(&file, &mut session.store, session.definitions, packages);

    let id = unpickler.unpickle_type(2).unwrap();
    let prefix = unpickler.index().type_at(4).unwrap();
    drop(unpickler);

    let Type::TypeRef {
        prefix: found,
        symbol,
    } = session.store.types.get(id)
    else {
        panic!("not a type reference");
    };
    // The prefix is the annotated type, not its underlying package.
    assert_eq!(*found, prefix);
    assert!(matches!(
        session.store.types.get(prefix),
        Type::Annotated { .. }
    ));
    let member = session.store.symbols.get(*symbol);
    assert_eq!(session.store.names.resolve(member.name.text()), "p");
    assert_eq!(member.kind, dotty_core::SymbolKind::Class);
}

#[test]
fn an_annotated_prefix_around_a_binder_still_being_decoded_is_refused_not_read() {
    // `POLYtype` at 0 (result `p` at 2) whose one alias bound is
    // `TYPEREF p (ANNOTATED <the poly> @p)`: the reference's prefix wraps a
    // binder that has no readable slot yet.
    let prefix = length_node(ANNOTATED, &[shared(0), shared(2)].concat());
    let mut reference = vec![TYPEREF, nat(1)];
    reference.extend(prefix);
    let mut param = length_node(TYPEBOUNDS, &reference);
    param.push(nat(1));
    let poly = length_node(POLY, &[package_ref(), param].concat());
    let bytes = file_with_ast(&poly);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut packages = Packages::new();
    enter_stub_classes(&mut session, &mut packages, &["p"], &["p"]);
    let mut unpickler =
        TastyUnpickler::with_packages(&file, &mut session.store, session.definitions, packages);

    let result = unpickler.unpickle_type(0);
    assert!(
        matches!(
            result,
            Err(UnpickleError::UnsupportedResolutionPrefix { address: 6, .. })
        ),
        "{result:?}"
    );
    assert_eq!(unpickler.index().type_at(0), None);
}

// Rebinding (`TYPEBOUNDS` with a variance marker rebinds its lambda)

const LAMBDA: u8 = 170;
const PARAM: u8 = 172;
const COVARIANT: u8 = 28;

/// `PARAMtype Length binder_ASTRef paramNum_Nat`.
fn param_type(binder: u8, number: u8) -> Vec<u8> {
    length_node(PARAM, &[nat(binder), nat(number)])
}

/// `+[p] =>> (<result>)` as the alias of a `TYPEBOUNDS` with a covariant
/// marker, inside a `FLEXIBLE` wrapper: the bounds are at 2 and the lambda at
/// 4, so the lambda's result starts at 6.
fn covariant_alias_of(result: &[u8]) -> Vec<u8> {
    let mut payload = result.to_vec();
    payload.extend(length_node(TYPEBOUNDS, &package_ref()));
    payload.push(nat(1));
    let lambda = length_node(LAMBDA, &payload);
    let mut bounds = lambda;
    bounds.push(COVARIANT);
    file_with_ast(&length_node(FLEXIBLE, &length_node(TYPEBOUNDS, &bounds)))
}

const BOUNDS_AT: u32 = 2;
const LAMBDA_AT: u32 = 4;

/// Every binder a `ParamRef` reachable from `root` names, looking through the
/// annotated, applied and alias forms these tests build.
fn binders_named(session: &Session, root: TypeId) -> Vec<TypeId> {
    let store = &session.store;
    match store.types.get(root) {
        Type::ParamRef { binder, .. } => vec![*binder],
        Type::Annotated {
            underlying,
            annotation,
        } => {
            let mut named = binders_named(session, *underlying);
            named.extend(binders_named(
                session,
                store.annotations.get(*annotation).ty,
            ));
            named
        }
        Type::Applied { tycon, args } => {
            let mut named = binders_named(session, *tycon);
            for arg in args {
                named.extend(binders_named(session, *arg));
            }
            named
        }
        _ => Vec::new(),
    }
}

fn lambda_result(store: &SemanticStore, id: TypeId) -> TypeId {
    match store.types.get(id) {
        Type::TypeLambda(lambda) => lambda.result,
        other => panic!("not a type lambda: {other:?}"),
    }
}

#[test]
fn a_compact_annotation_naming_the_rebound_binder_gets_a_new_annotation() {
    // `+[p] =>> (p @Applied[p])`: parent and annotation both name the lambda.
    let annotation = length_node(APPLIED, &[package_ref(), param_type(4, 0)].concat());
    let annotated = length_node(ANNOTATED, &[param_type(4, 0), annotation].concat());
    let bytes = covariant_alias_of(&annotated);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let bounds = unpickler.unpickle_type(BOUNDS_AT).unwrap();
    let original = unpickler.index().type_at(LAMBDA_AT).unwrap();
    drop(unpickler);

    let Type::AliasingBounds { alias: derived } = *session.store.types.get(bounds) else {
        panic!("not alias bounds");
    };
    assert_ne!(derived, original);

    let (old_underlying, old_annotation) =
        annotated_parts(&session.store, lambda_result(&session.store, original));
    let (new_underlying, new_annotation) =
        annotated_parts(&session.store, lambda_result(&session.store, derived));
    // The annotation changed, so it is a new one; the stored one is intact.
    assert_ne!(new_annotation, old_annotation);
    assert_eq!(session.store.annotations.get(new_annotation).tree, None);
    assert_ne!(new_underlying, old_underlying);

    // Nothing the derived lambda reaches names the old binder, and the old
    // lambda still names only itself.
    let mut derived_names = binders_named(&session, new_underlying);
    derived_names.extend(binders_named(
        &session,
        session.store.annotations.get(new_annotation).ty,
    ));
    assert_eq!(derived_names, vec![derived, derived]);
    let mut original_names = binders_named(&session, old_underlying);
    original_names.extend(binders_named(
        &session,
        session.store.annotations.get(old_annotation).ty,
    ));
    assert_eq!(original_names, vec![original, original]);
}

#[test]
fn a_compact_annotation_that_names_no_binder_is_reused_by_the_rebound_type() {
    // `+[p] =>> (p @Applied[p, p])`: only the parent names the lambda; the
    // annotation is `Applied` over a package and is independent of it.
    let annotation = length_node(APPLIED, &[package_ref(), package_ref()].concat());
    let annotated = length_node(ANNOTATED, &[param_type(4, 0), annotation].concat());
    let bytes = covariant_alias_of(&annotated);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let bounds = unpickler.unpickle_type(BOUNDS_AT).unwrap();
    let original = unpickler.index().type_at(LAMBDA_AT).unwrap();
    drop(unpickler);

    let Type::AliasingBounds { alias: derived } = *session.store.types.get(bounds) else {
        panic!("not alias bounds");
    };
    let (old_underlying, old_annotation) =
        annotated_parts(&session.store, lambda_result(&session.store, original));
    let (new_underlying, new_annotation) =
        annotated_parts(&session.store, lambda_result(&session.store, derived));
    // The underlying names the binder, so it is rebound; the annotation does
    // not, so the same annotation is kept.
    assert_ne!(new_underlying, old_underlying);
    assert_eq!(new_annotation, old_annotation);
    assert_eq!(binders_named(&session, new_underlying), vec![derived]);
}

// Full annotation constructor applications, on synthetic wire

const NEW: u8 = 95;
const SELECTIN: u8 = 176;
const APPLY_NODE: u8 = 136;
const TYPEAPPLY: u8 = 137;
const TYPED: u8 = 138;
const NAMEDARG: u8 = 119;
const STRINGCONST: u8 = 74;
const TRUECONST: u8 = 4;
const CLASSCONST: u8 = 92;
const IDENTTPT: u8 = 111;
const SHAREDTERM: u8 = 60;

/// `NEW tpt`.
fn new_of(class: &[u8]) -> Vec<u8> {
    [&[NEW][..], class].concat()
}

/// `SELECTin <init> (NEW p) p`: the constructor of `new p`.
fn constructor() -> Vec<u8> {
    let mut payload = vec![nat(2)];
    payload.extend(new_of(&package_ref()));
    payload.extend(package_ref());
    length_node(SELECTIN, &payload)
}

fn apply_to(function: &[u8], arguments: &[Vec<u8>]) -> Vec<u8> {
    let mut payload = function.to_vec();
    for argument in arguments {
        payload.extend(argument);
    }
    length_node(APPLY_NODE, &payload)
}

/// `STRINGconst hello`.
fn string_argument() -> Vec<u8> {
    vec![STRINGCONST, nat(4)]
}

/// `TRUEconst`.
fn true_argument() -> Vec<u8> {
    vec![TRUECONST]
}

/// `NAMEDARG label value`.
fn named(value: &[u8]) -> Vec<u8> {
    [&[NAMEDARG, nat(3)][..], value].concat()
}

/// The annotation on `p`: `p @<annotation>`.
fn full_file(annotation: &[u8]) -> Vec<u8> {
    annotated_file(&package_ref(), annotation)
}

fn arguments_of(session: &Session, id: TypeId) -> Vec<(Option<String>, Constant)> {
    let (_, annotation) = annotated_parts(&session.store, id);
    let annotation = session.store.annotations.get(annotation);
    assert_eq!(annotation.tree, None);
    let AnnotationArguments::Known(arguments) = &annotation.arguments else {
        panic!("arguments unavailable");
    };
    arguments
        .iter()
        .map(|argument| {
            let name = argument.name.map(|name| {
                session
                    .store
                    .names
                    .resolve(name.as_name().text())
                    .to_string()
            });
            let AnnotationValue::Constant(constant) = &argument.value;
            (name, constant.clone())
        })
        .collect()
}

fn decode(bytes: &[u8]) -> (Session, Result<TypeId, UnpickleError>) {
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let mut session = Session::new();
    let result = {
        let mut unpickler = unpickler_for(&file, &mut session);
        unpickler.unpickle_type(ANNOTATED_AT)
    };
    (session, result)
}

#[test]
fn a_new_without_an_application_has_no_arguments() {
    let (session, id) = decode(&full_file(&new_of(&package_ref())));
    let id = id.unwrap();

    assert_eq!(arguments_of(&session, id), vec![]);
}

#[test]
fn arguments_keep_their_order_names_and_values() {
    // `new p(hello, label = true)`
    let apply = apply_to(
        &constructor(),
        &[string_argument(), named(&true_argument())],
    );
    let (session, id) = decode(&full_file(&apply));

    let arguments = arguments_of(&session, id.unwrap());
    assert_eq!(arguments.len(), 2);
    assert_eq!(arguments[0].0, None);
    assert!(matches!(arguments[0].1, Constant::String(_)));
    assert_eq!(
        arguments[1],
        (Some("label".to_owned()), Constant::Boolean(true))
    );
}

#[test]
fn duplicate_argument_names_are_kept_as_written() {
    let apply = apply_to(
        &constructor(),
        &[named(&true_argument()), named(&string_argument())],
    );
    let (session, id) = decode(&full_file(&apply));

    let names: Vec<_> = arguments_of(&session, id.unwrap())
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(names, [Some("label".to_owned()), Some("label".to_owned())]);
}

#[test]
fn the_arguments_of_every_application_layer_are_read_innermost_first() {
    // `new p(hello)(true)`: `allTermArguments(fn) ::: args`.
    let inner = apply_to(&constructor(), &[string_argument()]);
    let outer = apply_to(&inner, &[true_argument()]);
    let (session, id) = decode(&full_file(&outer));

    let arguments = arguments_of(&session, id.unwrap());
    assert!(matches!(arguments[0].1, Constant::String(_)));
    assert_eq!(arguments[1].1, Constant::Boolean(true));
}

#[test]
fn constructor_type_arguments_make_an_applied_annotation_type() {
    // `new p[p]()`: `TYPEAPPLY (SELECTin <init> (NEW p)) p`.
    let type_apply = length_node(TYPEAPPLY, &[constructor(), package_ref()].concat());
    let (session, id) = decode(&full_file(&apply_to(&type_apply, &[])));
    let id = id.unwrap();
    let (_, annotation) = annotated_parts(&session.store, id);
    let stored = session.store.annotations.get(annotation).ty;

    let Type::Applied { tycon, args } = session.store.types.get(stored) else {
        panic!("annotation type is not applied");
    };
    assert!(matches!(
        session.store.types.get(*tycon),
        Type::TypeRef { .. }
    ));
    assert_eq!(args.len(), 1);
    assert_eq!(arguments_of(&session, id), vec![]);
}

#[test]
fn an_identifier_type_tree_stands_for_its_type() {
    // `NEW (IDENTtpt p p)`: the class tree is a type tree, not a type.
    let class = [&[IDENTTPT, nat(1)][..], &package_ref()].concat();
    let (session, id) = decode(&full_file(&new_of(&class)));
    let (_, annotation) = annotated_parts(&session.store, id.unwrap());

    let ty = session.store.annotations.get(annotation).ty;
    assert!(matches!(session.store.types.get(ty), Type::TypeRef { .. }));
}

#[test]
fn a_class_literal_argument_is_a_constant_class() {
    let class_literal = [&[CLASSCONST][..], &package_ref()].concat();
    let (session, id) = decode(&full_file(&apply_to(&constructor(), &[class_literal])));

    let arguments = arguments_of(&session, id.unwrap());
    let [(None, Constant::Class(class))] = &arguments[..] else {
        panic!("not one class literal: {arguments:?}");
    };
    assert!(matches!(
        session.store.types.get(*class),
        Type::TypeRef { .. }
    ));
}

#[test]
fn an_argument_wrapper_is_not_stripped() {
    // `TYPED true p`: an ascribed literal. Not evaluated, not unwrapped.
    let typed = length_node(TYPED, &[true_argument(), package_ref()].concat());
    let apply = apply_to(&constructor(), &[typed]);
    let (_, result) = decode(&full_file(&apply));

    assert!(
        matches!(
            result,
            Err(UnpickleError::UnsupportedAnnotationArgument {
                address: ANNOTATED_AT,
                tag: TYPED,
                ..
            })
        ),
        "{result:?}"
    );
}

#[test]
fn a_selection_that_is_not_the_constructor_is_refused() {
    // `SELECTin p (NEW p) p`: the name is not `<init>`.
    let mut payload = vec![nat(1)];
    payload.extend(new_of(&package_ref()));
    payload.extend(package_ref());
    let selection = length_node(SELECTIN, &payload);
    let (_, result) = decode(&full_file(&apply_to(&selection, &[])));

    assert!(
        matches!(
            result,
            Err(UnpickleError::UnsupportedAnnotationConstructor { tag: SELECTIN, .. })
        ),
        "{result:?}"
    );
}

#[test]
fn a_class_tree_that_is_not_a_type_is_a_constructor_error() {
    const IMPORTED: u8 = 75;
    let (_, result) = decode(&full_file(&new_of(&[IMPORTED, nat(1)])));

    assert!(
        matches!(
            result,
            Err(UnpickleError::UnsupportedAnnotationConstructor { tag: IMPORTED, .. })
        ),
        "{result:?}"
    );
}

#[test]
fn a_class_that_is_not_a_reference_is_an_invalid_annotation_type() {
    let flexible = length_node(FLEXIBLE, &package_ref());
    let (_, result) = decode(&full_file(&new_of(&flexible)));

    assert!(
        matches!(result, Err(UnpickleError::InvalidAnnotationType { .. })),
        "{result:?}"
    );
}

#[test]
fn a_second_type_application_is_refused() {
    let once = length_node(TYPEAPPLY, &[constructor(), package_ref()].concat());
    let twice = length_node(TYPEAPPLY, &[once, package_ref()].concat());
    let (_, result) = decode(&full_file(&apply_to(&twice, &[])));

    assert!(
        matches!(
            result,
            Err(UnpickleError::UnsupportedAnnotationConstructor { tag: TYPEAPPLY, .. })
        ),
        "{result:?}"
    );
}

#[test]
fn a_shared_term_annotation_is_still_deferred_not_followed() {
    let (_, result) = decode(&full_file(&[SHAREDTERM, nat(2)]));

    assert_eq!(
        result,
        Err(UnpickleError::UnsupportedAnnotationTree {
            address: ANNOTATED_AT,
            annotation_address: UNDERLYING_AT + 2,
            tag: SHAREDTERM,
        })
    );
}

#[test]
fn a_failing_second_argument_rolls_back_the_first_and_the_call_can_be_retried() {
    // The first argument decodes; the second is an unsupported `TYPED`.
    let typed = length_node(TYPED, &[true_argument(), package_ref()].concat());
    let bytes = full_file(&apply_to(&constructor(), &[string_argument(), typed]));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();

    let mut untouched = Session::new();
    drop(unpickler_for(&file, &mut untouched));
    let expected = next_ids(&mut untouched);

    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);
    let before = unpickler.index().type_count();
    for _ in 0..2 {
        let result = unpickler.unpickle_type(ANNOTATED_AT);
        assert!(
            matches!(
                result,
                Err(UnpickleError::UnsupportedAnnotationArgument { tag: TYPED, .. })
            ),
            "{result:?}"
        );
        assert_eq!(unpickler.index().type_count(), before);
        assert_eq!(unpickler.index().type_at(UNDERLYING_AT), None);
    }
    drop(unpickler);
    assert_eq!(next_ids(&mut session), expected);
}

#[test]
fn a_shared_link_to_a_full_annotated_type_returns_its_exact_id() {
    let annotated = length_node(
        ANNOTATED,
        &[
            package_ref(),
            apply_to(&constructor(), &[string_argument()]),
        ]
        .concat(),
    );
    let mut ast = annotated.clone();
    ast.extend(length_node(FLEXIBLE, &shared(0)));
    let link_at = u32::try_from(annotated.len() + 2).unwrap();
    let bytes = file_with_ast(&ast);
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = unpickler_for(&file, &mut session);

    let id = unpickler.unpickle_type(ANNOTATED_AT).unwrap();
    let before = unpickler.index().type_count();
    assert_eq!(unpickler.unpickle_type(link_at), Ok(id));
    assert_eq!(unpickler.index().type_count(), before);
    drop(unpickler);
    // One annotation, with its one argument.
    assert_eq!(next_ids(&mut session).1, 1);
    assert_eq!(arguments_of(&session, id).len(), 1);
}

// Real Scala 3.9.0 output.

const ANNOTATIONS: &[u8] = include_bytes!("fixtures/semantic/Annotated.tasty");
const ERASED: &[u8] = include_bytes!("fixtures/semantic/ErasedParams.tasty");

/// A session with the library classes the fixtures reference but do not
/// define (`scala.Int`, `java.lang.String`), and the unit's own symbols.
fn enter_stub_classes(
    session: &mut Session,
    packages: &mut Packages,
    path: &[&str],
    names: &[&str],
) {
    use dotty_core::names::{Name, Namespace};
    use dotty_core::{Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, Visibility};
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
            links: SymbolLinks::default(),
        });
        session
            .store
            .scopes
            .get_mut(package.scope)
            .enter(name, symbol);
    }
}

macro_rules! real_unit {
    ($bytes:expr, $file:ident, $session:ident, $unpickler:ident) => {
        let $file = TastyFile::parse_scala_3_9($bytes).unwrap();
        let mut $session = Session::new();
        let mut packages = Packages::new();
        enter_stub_classes(&mut $session, &mut packages, &["scala"], &["Int"]);
        enter_stub_classes(&mut $session, &mut packages, &["java", "lang"], &["String"]);
        enter_stub_classes(
            &mut $session,
            &mut packages,
            &["me", "cytrowski", "tastyfixtures", "semantic"],
            &["Tag", "Label"],
        );
        enter_stub_classes(
            &mut $session,
            &mut packages,
            &["scala", "annotation", "internal"],
            &["ErasedParam"],
        );
        let mut $unpickler = TastyUnpickler::with_packages(
            &$file,
            &mut $session.store,
            $session.definitions,
            packages,
        );
        $unpickler.enter_symbols().unwrap();
    };
}

/// `def inferred = (1: Int @Tag)`: an `ANNOTATEDtype` over `Int` whose
/// annotation is a `Tag` constructor call (`APPLY`, no arguments).
const FULL: u32 = 35;
/// `(1: Int @Tag @Label("second"))`: the outer node (86) wraps the inner (88).
const NESTED_OUTER: u32 = 86;
const NESTED_INNER: u32 = 88;

fn class_name(session: &Session, ty: TypeId) -> String {
    let Type::TypeRef { symbol, .. } = session.store.types.get(ty) else {
        panic!("not a type reference: {:?}", session.store.types.get(ty));
    };
    let name = session.store.symbols.get(*symbol).name.text();
    session.store.names.resolve(name).to_string()
}

#[test]
fn a_real_zero_argument_annotation_is_known_to_have_no_arguments() {
    real_unit!(ANNOTATIONS, file, session, unpickler);
    let id = unpickler.unpickle_type(FULL).unwrap();
    // Identity: the same annotated type, and no second annotation.
    assert_eq!(unpickler.unpickle_type(FULL), Ok(id));
    drop(unpickler);

    let (underlying, annotation) = annotated_parts(&session.store, id);
    assert_eq!(class_name(&session, underlying), "Int");
    let annotation = session.store.annotations.get(annotation);
    assert_eq!(class_name(&session, annotation.ty), "Tag");
    // `@Tag()` written: no arguments, which is not "unavailable"; and no typed
    // tree is attached.
    assert_eq!(annotation.arguments, AnnotationArguments::Known(Vec::new()));
    assert_eq!(annotation.tree, None);
    assert_eq!(next_ids(&mut session).1, 1);
}

#[test]
fn a_real_annotation_with_an_argument_keeps_it_and_the_nesting_order() {
    real_unit!(ANNOTATIONS, file, session, unpickler);
    let outer = unpickler.unpickle_type(NESTED_OUTER).unwrap();
    let inner = unpickler.index().type_at(NESTED_INNER).unwrap();
    drop(unpickler);

    // `Int @Tag @Label("second")`: the later annotation is the outer one.
    let (under_outer, label) = annotated_parts(&session.store, outer);
    assert_eq!(under_outer, inner);
    let (_, tag) = annotated_parts(&session.store, inner);
    assert_eq!(
        class_name(&session, session.store.annotations.get(tag).ty),
        "Tag"
    );
    let label = session.store.annotations.get(label);
    assert_eq!(class_name(&session, label.ty), "Label");
    let AnnotationArguments::Known(arguments) = &label.arguments else {
        panic!("arguments unavailable");
    };
    assert_eq!(arguments.len(), 1);
    assert_eq!(arguments[0].name, None);
    let AnnotationValue::Constant(Constant::String(text)) = &arguments[0].value else {
        panic!("not a string argument");
    };
    assert_eq!(session.store.names.resolve(*text), "second");
}

#[test]
fn a_real_annotation_whose_class_is_not_entered_rolls_back_everything() {
    // Without the `Tag` class the annotation type cannot resolve: the call
    // fails after `Int` (the parent) was decoded, and nothing survives.
    let file = TastyFile::parse_scala_3_9(ANNOTATIONS).unwrap();
    let mut session = Session::new();
    let mut packages = Packages::new();
    enter_stub_classes(&mut session, &mut packages, &["scala"], &["Int"]);
    let mut unpickler =
        TastyUnpickler::with_packages(&file, &mut session.store, session.definitions, packages);
    unpickler.enter_symbols().unwrap();
    let before = unpickler.index().type_count();

    let result = unpickler.unpickle_type(FULL);
    assert!(
        matches!(result, Err(UnpickleError::UnresolvedMember { .. })),
        "{result:?}"
    );
    assert_eq!(unpickler.index().type_count(), before);
    assert_eq!(unpickler.index().type_at(FULL), None);
    assert_eq!(unpickler.index().type_at(FULL + 2), None);
}

// Erased method parameters (Milestone 4b2a)
//
// `MethodParam.erased` is derived from the parameter type's outer chain of
// `Annotated` wrappers, matching `scala.annotation.internal.ErasedParam` by
// its exact package path.

/// The `METHODtype` of `(erased z: Int, w: Int) => w`.
const ERASED_METHOD: u32 = 136;

#[test]
fn the_real_erased_method_type_decodes_with_the_first_parameter_erased() {
    real_unit!(ERASED, file, session, unpickler);
    let method = unpickler.unpickle_type(ERASED_METHOD).unwrap();
    let annotated = unpickler.index().type_at(ERASED_PARAM).unwrap();
    drop(unpickler);

    let Type::Method(method) = session.store.types.get(method) else {
        panic!("not a method type");
    };
    let [erased, ordinary] = &method.params[..] else {
        panic!("not two parameters");
    };
    assert!(erased.erased);
    assert!(!ordinary.erased);
    // The annotation is not stripped from the type.
    assert_eq!(erased.ty, annotated);
    assert!(matches!(
        session.store.types.get(erased.ty),
        Type::Annotated { .. }
    ));
    assert_eq!(class_name(&session, ordinary.ty), "Int");
    // `varargs` stays the JVM distinction, which a `METHODtype` does not carry.
    assert!(!erased.varargs && !ordinary.varargs);
}

/// `METHODtype p (x: <param>)`: a method at 0 with result `p` (at 2) and one
/// parameter, named `p`, whose type is `param`.
fn method_with(param: &[u8]) -> Vec<u8> {
    const METHOD: u8 = 180;
    let mut payload = package_ref();
    payload.extend(param);
    payload.push(nat(1));
    file_with_ast(&length_node(METHOD, &payload))
}

fn decoded_method(bytes: &[u8]) -> (Session, dotty_core::types::MethodType) {
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let mut session = Session::new();
    let id = unpickler_for(&file, &mut session).unpickle_type(0).unwrap();
    let Type::Method(method) = session.store.types.get(id).clone() else {
        panic!("not a method type");
    };
    (session, method)
}

#[test]
fn an_annotation_that_is_not_erased_param_does_not_erase_the_parameter() {
    // `p @new p`: annotated with the class `p`, by its name text an
    // unrelated annotation.
    let annotated = length_node(ANNOTATED, &[package_ref(), new_of(&package_ref())].concat());
    let (session, method) = decoded_method(&method_with(&annotated));

    assert!(!method.params[0].erased);
    assert!(matches!(
        session.store.types.get(method.params[0].ty),
        Type::Annotated { .. }
    ));
}

#[test]
fn an_unannotated_parameter_is_not_erased() {
    let (_, method) = decoded_method(&method_with(&package_ref()));

    assert!(!method.params[0].erased);
}

// The erased-parameter wire audit (Milestone 4b1, question 12).
//
// `MethodParam.erased` stays `false`: Dotty derives it from an
// `ErasedParamAnnot` (`scala.annotation.internal.ErasedParam`) on the parameter *type*, and the type is pickled with a
// full annotation tree, not a compact one.

/// The first parameter type of the `METHODtype` of
/// `(erased z: Int, w: Int) => w`: an `ANNOTATEDtype`.
const ERASED_PARAM: u32 = 140;
const ERASED_ANNOTATION: u32 = 144;

#[test]
fn an_erased_parameter_type_is_annotated_with_a_real_erased_param_annotation() {
    real_unit!(ERASED, file, session, unpickler);
    let id = unpickler.unpickle_type(ERASED_PARAM).unwrap();
    drop(unpickler);

    let (underlying, annotation) = annotated_parts(&session.store, id);
    assert_eq!(class_name(&session, underlying), "Int");
    let annotation = session.store.annotations.get(annotation);
    assert_eq!(class_name(&session, annotation.ty), "ErasedParam");
    assert_eq!(annotation.arguments, AnnotationArguments::Known(Vec::new()));
}

#[test]
fn the_erased_parameter_annotation_is_a_constructor_call_of_erased_param() {
    use dotty_tasty::tasty::{RawName, RawTree, Reader, StandardSection};
    let file = TastyFile::parse_scala_3_9(ERASED).unwrap();
    let index = file.ast_address_index().unwrap();
    let payload = file.section(StandardSection::Asts).unwrap().payload;
    let end = ERASED_ANNOTATION as usize + index.get(ERASED_ANNOTATION).unwrap().payload.len();

    // The class named by the annotation's `new`: the one `TYPEREF` inside it.
    let mut classes = Vec::new();
    for node in index.iter_nodes_with_tag(117) {
        if node.offset > ERASED_ANNOTATION as usize && node.offset < end {
            let mut reader = Reader::with_range(payload, node.offset, payload.len()).unwrap();
            let tree = RawTree::decode_with_base_offset(&mut reader, 0).unwrap();
            let RawTree::NatAst { value, .. } = tree else {
                panic!("not a named type reference");
            };
            classes.push(file.names().entries()[usize::try_from(value).unwrap()].clone());
        }
    }
    assert_eq!(classes, [RawName::Utf8("ErasedParam".to_owned())]);
}

const CAPTURING: &[u8] = include_bytes!("fixtures/semantic/CapturingHolder.tasty");

/// The inferred type of `val local = () => c` under capture checking: an
/// `ANNOTATEDtype` whose parent and whose compact annotation are both
/// `SHAREDtype` links (the annotation is `scala.annotation.retainsCap`).
const CAPTURING_ANNOTATED: u32 = 92;
const CAPTURING_PARENT: u32 = 94;
const CAPTURING_ANNOTATION: u32 = 96;

macro_rules! capturing_unit {
    ($file:ident, $session:ident, $unpickler:ident) => {
        let $file = TastyFile::parse_scala_3_9(CAPTURING).unwrap();
        let mut $session = Session::new();
        let mut packages = Packages::new();
        enter_stub_classes(
            &mut $session,
            &mut packages,
            &["scala"],
            &["Int", "Function0"],
        );
        enter_stub_classes(
            &mut $session,
            &mut packages,
            &["scala", "annotation"],
            &["retainsCap"],
        );
        enter_stub_classes(
            &mut $session,
            &mut packages,
            &["me", "cytrowski", "tastyfixtures", "semantic"],
            &["Cap"],
        );
        let mut $unpickler = TastyUnpickler::with_packages(
            &$file,
            &mut $session.store,
            $session.definitions,
            packages,
        );
        $unpickler.enter_symbols().unwrap();
    };
}

#[test]
fn a_real_compact_annotation_decodes_to_an_annotation_without_a_tree() {
    capturing_unit!(file, session, unpickler);
    let id = unpickler.unpickle_type(CAPTURING_ANNOTATED).unwrap();
    // A link has no entry of its own; decoding it names its target's id.
    let parent = unpickler.unpickle_type(CAPTURING_PARENT).unwrap();
    let annotation_type = unpickler.unpickle_type(CAPTURING_ANNOTATION).unwrap();
    // Identity: again, and through the address index.
    assert_eq!(unpickler.unpickle_type(CAPTURING_ANNOTATED), Ok(id));
    drop(unpickler);

    let (underlying, annotation) = annotated_parts(&session.store, id);
    assert_eq!(underlying, parent);
    let annotation = session.store.annotations.get(annotation);
    assert_eq!(annotation.ty, annotation_type);
    assert_eq!(annotation.tree, None);
    let Type::TypeRef { symbol, .. } = session.store.types.get(annotation.ty) else {
        panic!("the annotation is not a type reference");
    };
    let class = session.store.symbols.get(*symbol).name.text();
    assert_eq!(session.store.names.resolve(class), "retainsCap");
}
