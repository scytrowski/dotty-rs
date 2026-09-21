//! Milestone 4b1: `ANNOTATEDtype`, compact and full.
//!
//! Small synthetic wire files cover classification, malformed shapes,
//! identity and rollback; real Scala 3.9.0 fixtures
//! (`tests/fixtures/semantic/{Annotations,CaptureChecking,Erased}.scala`) are
//! used further down.
use dotty_core::Definitions;
use dotty_core::Packages;
use dotty_core::store::SemanticStore;
use dotty_core::symbols::SymbolOrigin;
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
