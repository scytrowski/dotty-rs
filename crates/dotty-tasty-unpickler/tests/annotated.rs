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
use dotty_core::types::Type;
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
/// annotation is a `Tag` constructor call (`APPLY`).
const FULL: u32 = 35;
const FULL_ANNOTATION: u32 = 41;
/// `(1: Int @Tag @Label("second"))`: the outer node (86) wraps the inner (88).
const NESTED_OUTER: u32 = 86;
const NESTED_INNER: u32 = 88;
const APPLY: u8 = 136;

#[test]
fn a_real_full_annotation_is_deferred_with_its_root_tag() {
    real_unit!(ANNOTATIONS, file, session, unpickler);
    assert_eq!(
        unpickler.unpickle_type(FULL),
        Err(UnpickleError::UnsupportedAnnotationTree {
            address: FULL,
            annotation_address: FULL_ANNOTATION,
            tag: APPLY,
        })
    );
}

#[test]
fn a_real_full_annotation_is_atomic() {
    real_unit!(ANNOTATIONS, file, session, unpickler);
    let before = unpickler.index().type_count();

    assert!(unpickler.unpickle_type(FULL).is_err());
    assert_eq!(unpickler.index().type_count(), before);
    // Nothing of the parent (`Int`) survives either.
    assert_eq!(unpickler.index().type_at(FULL + 2), None);
    assert_eq!(unpickler.index().type_at(FULL), None);
}

#[test]
fn a_nested_real_annotation_reports_the_first_full_tree_it_meets() {
    real_unit!(ANNOTATIONS, file, session, unpickler);
    // The parent is decoded first, so the inner annotation is reached first.
    assert!(matches!(
        unpickler.unpickle_type(NESTED_OUTER),
        Err(UnpickleError::UnsupportedAnnotationTree { address, tag: APPLY, .. })
            if address == NESTED_INNER
    ));
}

// The erased-parameter wire audit (Milestone 4b1, question 12).
//
// `MethodParam.erased` stays `false`: Dotty derives it from an
// `ErasedParamAnnot` (`scala.annotation.internal.ErasedParam`) on the parameter *type*, and the type is pickled with a
// full annotation tree, not a compact one.

/// The `METHODtype` of `(erased z: Int, w: Int) => w` (136), whose first
/// parameter type is the `ANNOTATEDtype` at 140.
const ERASED_METHOD: u32 = 136;
const ERASED_PARAM: u32 = 140;
const ERASED_ANNOTATION: u32 = 144;

#[test]
fn an_erased_parameter_type_is_annotated_with_a_full_tree() {
    real_unit!(ERASED, file, session, unpickler);
    assert_eq!(
        unpickler.unpickle_type(ERASED_PARAM),
        Err(UnpickleError::UnsupportedAnnotationTree {
            address: ERASED_PARAM,
            annotation_address: ERASED_ANNOTATION,
            tag: APPLY,
        })
    );
    // So the method type holding it is deferred, not decoded with the
    // annotation dropped.
    assert!(matches!(
        unpickler.unpickle_type(ERASED_METHOD),
        Err(UnpickleError::UnsupportedAnnotationTree { .. })
    ));
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
