//! Serialized symbol annotations (Milestone 5e1): `complete_symbol_annotations`
//! / `complete_symbols_annotations` over the real `semantic/SymbolAnnotated*`
//! fixtures (`tests/fixtures/semantic/generate.sh`; addresses pinned the same
//! way `enter.rs`'s own unit tests pin them).

use dotty_core::Definitions;
use dotty_core::Packages;
use dotty_core::ids::{AnnotationId, TypeId};
use dotty_core::names::{Name, Namespace};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{
    Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
};
use dotty_core::types::{AnnotationArguments, AnnotationValue, Constant, Type};
use dotty_tasty::tasty::TastyFile;
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

const SYMBOL_ANNOTATED: &[u8] = include_bytes!("fixtures/semantic/SymbolAnnotated.tasty");
const SYMBOL_ANNOTATED_PARAMS: &[u8] =
    include_bytes!("fixtures/semantic/SymbolAnnotatedParams.tasty");
const SYMBOL_UNANNOTATED: &[u8] = include_bytes!("fixtures/semantic/SymbolUnannotated.tasty");

// Absolute addresses (pinned the same way `enter.rs`'s own unit tests pin
// them over these fixtures).
const SYMBOL_ANNOTATED_CLASS: u32 = 5; // `@SymbolMarker class SymbolAnnotated`
const SYMBOL_ANNOTATED_INIT: u32 = 23; // its primary constructor
const SYMBOL_ANNOTATED_X: u32 = 31; // `@SymbolMarker val x`
const SYMBOL_ANNOTATED_F: u32 = 60; // `@SymbolMarker def f(x: Int)`
const SYMBOL_ANNOTATED_F_PARAM: u32 = 63; // `f`'s unannotated parameter `x`
const SYMBOL_ANNOTATED_T: u32 = 92; // `@SymbolMarker type T`

const SYMBOL_ANNOTATED_PARAMS_CTOR_PARAM: u32 = 34; // `@SymbolMarker val ctorParam`
const SYMBOL_ANNOTATED_PARAMS_TYPE_PARAM: u32 = 67; // `[@SymbolMarker A]`
const SYMBOL_ANNOTATED_PARAMS_TERM_PARAM: u32 = 96; // `@SymbolMarker @SymbolTagged(tag = "p") param`

const SYMBOL_UNANNOTATED_CLASS: u32 = 4; // `class SymbolUnannotated` (compiler's @SourceFile only)
const SYMBOL_UNANNOTATED_PLAIN: u32 = 29; // `val plain`

/// The fixture's single top-level `PACKAGE` node: entered through
/// `enter_package`, not `enter_symbol`, so pass 1 never indexes an
/// annotation tail for it.
const PACKAGE_ADDRESS: u32 = 0;

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

/// Enters a synthetic class symbol for every name in `names`, directly
/// declared under the package at `path`, standing in for a real class
/// defined in a unit this test does not enter — mirroring
/// `tests/annotated.rs`'s own `enter_stub_classes`.
fn enter_stub_classes(
    session: &mut Session,
    packages: &mut Packages,
    path: &[&str],
    names: &[&str],
) {
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

/// The stub classes every fixture in this file needs: `Int` and `String`
/// (declared types), `SymbolMarker`/`SymbolTagged` (the fixture's own
/// annotation classes, defined in sibling units this test does not enter)
/// and `SourceFile` (the compiler's own synthetic annotation on every
/// top-level class).
fn stub_packages(session: &mut Session) -> Packages {
    let mut packages = Packages::new();
    enter_stub_classes(session, &mut packages, &["scala"], &["Int"]);
    enter_stub_classes(session, &mut packages, &["java", "lang"], &["String"]);
    enter_stub_classes(
        session,
        &mut packages,
        &["me", "cytrowski", "tastyfixtures", "semantic"],
        &["SymbolMarker", "SymbolTagged"],
    );
    enter_stub_classes(
        session,
        &mut packages,
        &["scala", "annotation", "internal"],
        &["SourceFile"],
    );
    packages
}

fn class_name(session: &Session, ty: TypeId) -> String {
    let Some(symbol) = session.store.types.get(ty).reference_symbol() else {
        panic!("not a type reference: {:?}", session.store.types.get(ty));
    };
    let name = session.store.symbols.get(symbol).name.text();
    session.store.names.resolve(name).to_string()
}

fn class_names(session: &Session, ids: &[AnnotationId]) -> Vec<String> {
    ids.iter()
        .map(|id| class_name(session, session.store.annotations.get(*id).ty))
        .collect()
}

macro_rules! real_unit {
    ($bytes:expr, $file:ident, $session:ident, $unpickler:ident) => {
        let $file = TastyFile::parse_scala_3_9($bytes).unwrap();
        let mut $session = Session::new();
        let packages = stub_packages(&mut $session);
        let mut $unpickler = TastyUnpickler::with_packages(
            &$file,
            &mut $session.store,
            $session.definitions,
            packages,
        );
        $unpickler.enter_symbols().unwrap();
    };
}

// --- basic completion, over every fixture shape ---

#[test]
fn an_annotated_val_completes_to_one_annotation_of_the_right_class() {
    real_unit!(SYMBOL_ANNOTATED, file, session, unpickler);

    let ids = unpickler
        .complete_symbol_annotations(SYMBOL_ANNOTATED_X)
        .unwrap();
    drop(unpickler);

    assert_eq!(class_names(&session, &ids), ["SymbolMarker"]);
    let annotation = session.store.annotations.get(ids[0]);
    assert_eq!(annotation.arguments, AnnotationArguments::Known(Vec::new()));
    assert_eq!(annotation.tree, None);
}

#[test]
fn an_annotated_def_completes_to_one_annotation() {
    real_unit!(SYMBOL_ANNOTATED, file, session, unpickler);

    let ids = unpickler
        .complete_symbol_annotations(SYMBOL_ANNOTATED_F)
        .unwrap();
    drop(unpickler);

    assert_eq!(class_names(&session, &ids), ["SymbolMarker"]);
}

#[test]
fn an_annotated_type_alias_completes_to_one_annotation() {
    real_unit!(SYMBOL_ANNOTATED, file, session, unpickler);

    let ids = unpickler
        .complete_symbol_annotations(SYMBOL_ANNOTATED_T)
        .unwrap();
    drop(unpickler);

    assert_eq!(class_names(&session, &ids), ["SymbolMarker"]);
}

#[test]
fn an_unannotated_parameter_completes_to_no_annotations() {
    real_unit!(SYMBOL_ANNOTATED, file, session, unpickler);

    let ids = unpickler
        .complete_symbol_annotations(SYMBOL_ANNOTATED_F_PARAM)
        .unwrap();

    assert_eq!(ids, Vec::new());
}

#[test]
fn a_synthesized_constructor_completes_to_no_annotations() {
    real_unit!(SYMBOL_ANNOTATED, file, session, unpickler);

    let ids = unpickler
        .complete_symbol_annotations(SYMBOL_ANNOTATED_INIT)
        .unwrap();

    assert_eq!(ids, Vec::new());
}

#[test]
fn a_class_completes_with_its_annotation_and_the_compilers_source_file() {
    real_unit!(SYMBOL_ANNOTATED, file, session, unpickler);

    let ids = unpickler
        .complete_symbol_annotations(SYMBOL_ANNOTATED_CLASS)
        .unwrap();
    drop(unpickler);

    let mut names = class_names(&session, &ids);
    names.sort();
    assert_eq!(names, ["SourceFile", "SymbolMarker"]);
}

#[test]
fn an_unannotated_class_still_completes_with_the_compilers_source_file() {
    real_unit!(SYMBOL_UNANNOTATED, file, session, unpickler);

    let ids = unpickler
        .complete_symbol_annotations(SYMBOL_UNANNOTATED_CLASS)
        .unwrap();
    drop(unpickler);

    assert_eq!(class_names(&session, &ids), ["SourceFile"]);
}

#[test]
fn a_definition_with_no_annotations_at_all_completes_idempotently_to_none() {
    real_unit!(SYMBOL_UNANNOTATED, file, session, unpickler);

    let first = unpickler
        .complete_symbol_annotations(SYMBOL_UNANNOTATED_PLAIN)
        .unwrap();
    let second = unpickler
        .complete_symbol_annotations(SYMBOL_UNANNOTATED_PLAIN)
        .unwrap();

    assert_eq!(first, Vec::new());
    assert_eq!(second, Vec::new());
}

#[test]
fn an_annotated_constructor_value_parameter_completes() {
    real_unit!(SYMBOL_ANNOTATED_PARAMS, file, session, unpickler);

    let ids = unpickler
        .complete_symbol_annotations(SYMBOL_ANNOTATED_PARAMS_CTOR_PARAM)
        .unwrap();
    drop(unpickler);

    assert_eq!(class_names(&session, &ids), ["SymbolMarker"]);
}

#[test]
fn an_annotated_type_parameter_completes() {
    real_unit!(SYMBOL_ANNOTATED_PARAMS, file, session, unpickler);

    let ids = unpickler
        .complete_symbol_annotations(SYMBOL_ANNOTATED_PARAMS_TYPE_PARAM)
        .unwrap();
    drop(unpickler);

    assert_eq!(class_names(&session, &ids), ["SymbolMarker"]);
}

#[test]
fn a_term_parameter_with_two_annotations_keeps_wire_order_and_the_named_argument() {
    real_unit!(SYMBOL_ANNOTATED_PARAMS, file, session, unpickler);

    let ids = unpickler
        .complete_symbol_annotations(SYMBOL_ANNOTATED_PARAMS_TERM_PARAM)
        .unwrap();
    drop(unpickler);

    // `@SymbolMarker @SymbolTagged(tag = "p") param` in source; the wire's
    // own `ANNOTATION` tail order (which `enter.rs`'s
    // `a_term_parameter_with_two_annotations_keeps_both_in_wire_order` also
    // pins over this exact fixture) has `SymbolTagged` first.
    assert_eq!(
        class_names(&session, &ids),
        ["SymbolTagged", "SymbolMarker"]
    );
    let tagged = session.store.annotations.get(ids[0]);
    let AnnotationArguments::Known(arguments) = &tagged.arguments else {
        panic!("arguments unavailable");
    };
    assert_eq!(arguments.len(), 1);
    assert_eq!(
        session
            .store
            .names
            .resolve(arguments[0].name.unwrap().as_name().text()),
        "tag"
    );
    let AnnotationValue::Constant(Constant::String(text)) = &arguments[0].value else {
        panic!("not a string argument");
    };
    assert_eq!(session.store.names.resolve(*text), "p");
}

// --- identity, idempotence ---

#[test]
fn each_annotation_occurrence_owns_a_distinct_annotation_id_even_with_equal_payloads() {
    real_unit!(SYMBOL_ANNOTATED_PARAMS, file, session, unpickler);

    let ctor_param = unpickler
        .complete_symbol_annotations(SYMBOL_ANNOTATED_PARAMS_CTOR_PARAM)
        .unwrap();
    let type_param = unpickler
        .complete_symbol_annotations(SYMBOL_ANNOTATED_PARAMS_TYPE_PARAM)
        .unwrap();

    // Both are a lone, argument-free `@SymbolMarker`: equal payloads, but two
    // distinct occurrences never share one id.
    assert_ne!(ctor_param[0], type_param[0]);
}

#[test]
fn repeated_completion_of_the_same_symbol_returns_the_same_ids() {
    real_unit!(SYMBOL_ANNOTATED, file, session, unpickler);

    let first = unpickler
        .complete_symbol_annotations(SYMBOL_ANNOTATED_X)
        .unwrap();
    let second = unpickler
        .complete_symbol_annotations(SYMBOL_ANNOTATED_X)
        .unwrap();

    assert_eq!(first, second);
}

// --- independence from `SymbolInfo` completion ---

#[test]
fn annotation_completion_succeeds_while_symbol_info_is_still_missing() {
    real_unit!(SYMBOL_ANNOTATED, file, session, unpickler);

    assert_eq!(
        unpickler.symbol_state_at(SYMBOL_ANNOTATED_X).unwrap().1,
        SymbolInfo::Missing
    );
    let ids = unpickler
        .complete_symbol_annotations(SYMBOL_ANNOTATED_X)
        .unwrap();
    assert_eq!(ids.len(), 1);
    // Annotation completion never touches `SymbolInfo`.
    assert_eq!(
        unpickler.symbol_state_at(SYMBOL_ANNOTATED_X).unwrap().1,
        SymbolInfo::Missing
    );
}

#[test]
fn completing_symbol_info_does_not_force_or_populate_annotations() {
    real_unit!(SYMBOL_ANNOTATED, file, session, unpickler);

    let ty = unpickler.complete_symbol(SYMBOL_ANNOTATED_X).unwrap();
    let symbol = unpickler.index().symbol_at(SYMBOL_ANNOTATED_X).unwrap();
    drop(unpickler);

    assert!(matches!(session.store.types.get(ty), Type::TypeRef { .. }));
    assert_eq!(session.store.symbols.get(symbol).annotations, Vec::new());
}

#[test]
fn an_annotation_completion_failure_does_not_disturb_a_prior_successful_symbol_completion() {
    // `x`'s own type (`Int`) resolves; its annotation class does not, because
    // this session never enters `SymbolMarker`.
    let file = TastyFile::parse_scala_3_9(SYMBOL_ANNOTATED).unwrap();
    let mut session = Session::new();
    let mut packages = Packages::new();
    enter_stub_classes(&mut session, &mut packages, &["scala"], &["Int"]);
    let mut unpickler =
        TastyUnpickler::with_packages(&file, &mut session.store, session.definitions, packages);
    unpickler.enter_symbols().unwrap();

    let ty = unpickler.complete_symbol(SYMBOL_ANNOTATED_X).unwrap();
    let result = unpickler.complete_symbol_annotations(SYMBOL_ANNOTATED_X);

    assert!(result.is_err(), "{result:?}");
    assert_eq!(
        unpickler.symbol_state_at(SYMBOL_ANNOTATED_X).unwrap().1,
        SymbolInfo::Complete(ty)
    );
}

// --- atomicity ---

#[test]
fn a_partial_multi_annotation_failure_leaves_no_annotation_attached() {
    // The class has two annotation occurrences (`@SymbolMarker` and the
    // compiler's own `@SourceFile`); this session enters neither class, so
    // both decode attempts fail and nothing about the symbol changes.
    let file = TastyFile::parse_scala_3_9(SYMBOL_ANNOTATED).unwrap();
    let mut session = Session::new();
    let mut packages = Packages::new();
    enter_stub_classes(&mut session, &mut packages, &["scala"], &["Int"]);
    let mut unpickler =
        TastyUnpickler::with_packages(&file, &mut session.store, session.definitions, packages);
    unpickler.enter_symbols().unwrap();

    let result = unpickler.complete_symbol_annotations(SYMBOL_ANNOTATED_CLASS);
    assert!(result.is_err(), "{result:?}");
    // A retry behaves like the first attempt, not like a completed symbol.
    let retry = unpickler.complete_symbol_annotations(SYMBOL_ANNOTATED_CLASS);
    assert!(retry.is_err(), "{retry:?}");
    let symbol = unpickler.index().symbol_at(SYMBOL_ANNOTATED_CLASS).unwrap();
    drop(unpickler);

    assert_eq!(session.store.symbols.get(symbol).annotations, Vec::new());
}

#[test]
fn a_symbol_previously_completed_survives_a_later_failed_call_for_another_symbol() {
    real_unit!(SYMBOL_ANNOTATED, file, session, unpickler);

    let x = unpickler
        .complete_symbol_annotations(SYMBOL_ANNOTATED_X)
        .unwrap();

    // An address with no entered symbol: the second call fails outright.
    let bogus_address = u32::MAX;
    assert!(matches!(
        unpickler.complete_symbol_annotations(bogus_address),
        Err(UnpickleError::MissingEnteredSymbol { address }) if address == bogus_address
    ));

    // `x`'s own completion, from before, is untouched.
    assert_eq!(
        unpickler
            .complete_symbol_annotations(SYMBOL_ANNOTATED_X)
            .unwrap(),
        x
    );
}

// --- batch semantics ---

#[test]
fn a_batch_of_valid_addresses_completes_all_of_them() {
    real_unit!(SYMBOL_ANNOTATED, file, session, unpickler);

    let result = unpickler
        .complete_symbols_annotations(&[
            SYMBOL_ANNOTATED_X,
            SYMBOL_ANNOTATED_F,
            SYMBOL_ANNOTATED_F_PARAM,
        ])
        .unwrap();

    assert_eq!(result[0].len(), 1);
    assert_eq!(result[1].len(), 1);
    assert_eq!(result[2].len(), 0);
}

#[test]
fn a_batch_with_one_failing_address_leaves_none_of_it_completed() {
    real_unit!(SYMBOL_ANNOTATED, file, session, unpickler);

    let result = unpickler.complete_symbols_annotations(&[
        SYMBOL_ANNOTATED_X,
        SYMBOL_ANNOTATED_F_PARAM,
        u32::MAX,
    ]);
    assert!(result.is_err(), "{result:?}");
    let x = unpickler.index().symbol_at(SYMBOL_ANNOTATED_X).unwrap();

    assert_eq!(session.store.symbols.get(x).annotations, Vec::new());
}

// --- a package address is a typed error, not a panic ---

#[test]
fn a_package_address_is_a_typed_error_not_a_panic() {
    // A package is entered through `enter_package`, which never indexes an
    // annotation tail for it (a `PACKAGE` node has none in the wire format).
    // `complete_symbol_annotations` must not `.expect()` its way into a
    // panic on this input: it is a real, well-formed address with a real
    // entered symbol, just not one this completion supports.
    real_unit!(SYMBOL_ANNOTATED, file, session, unpickler);

    let result = unpickler.complete_symbol_annotations(PACKAGE_ADDRESS);

    assert_eq!(
        result,
        Err(UnpickleError::UnsupportedAnnotationCompletion {
            address: PACKAGE_ADDRESS,
            kind: SymbolKind::Package,
        })
    );
}

#[test]
fn a_package_address_in_a_batch_fails_the_whole_batch_without_panicking() {
    real_unit!(SYMBOL_ANNOTATED, file, session, unpickler);

    let result = unpickler.complete_symbols_annotations(&[SYMBOL_ANNOTATED_X, PACKAGE_ADDRESS]);

    assert_eq!(
        result,
        Err(UnpickleError::UnsupportedAnnotationCompletion {
            address: PACKAGE_ADDRESS,
            kind: SymbolKind::Package,
        })
    );
}
