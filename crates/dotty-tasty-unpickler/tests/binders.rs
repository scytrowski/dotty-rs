//! Milestone 3a: binder identity (`TYPELAMBDAtype`, `PARAMtype`).
//!
//! Small synthetic wire files cover what real compiler output cannot: a
//! `PARAMtype` that names a wrong or missing binder, and cycles.
use dotty_core::Definitions;
use dotty_core::ids::TypeId;
use dotty_core::store::SemanticStore;
use dotty_core::{Packages, symbols::SymbolOrigin};
use dotty_tasty::tasty::TastyFile;
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

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

/// A Nat holding a small value: the value with the stop bit.
fn nat(value: u8) -> u8 {
    assert!(value < 128);
    0x80 | value
}

/// `tag length payload`, the only shape a top-level AST node may have.
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

/// `PARAMtype Length binder_ASTRef paramNum_Nat`.
fn param_type(binder: u8, number: u8) -> Vec<u8> {
    length_node(172, &[nat(binder), nat(number)])
}

/// An `ANDtype` wrapper (a top-level node must be length-prefixed) around
/// `TYPEREFpkg p` at address 2 and `children`, which start at address 4.
fn file_with(children: &[u8]) -> Vec<u8> {
    let mut payload = vec![65, nat(1)];
    payload.extend(children);
    file_with_ast(&length_node(165, &payload))
}

fn decode(bytes: &[u8], at: u32) -> Result<TypeId, UnpickleError> {
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let mut session = Session::new();
    let mut packages = Packages::new();
    packages.enter(&mut session.store, SymbolOrigin::Synthetic, &["p"]);
    let mut unpickler =
        TastyUnpickler::with_packages(&file, &mut session.store, session.definitions, packages);
    unpickler.unpickle_type(at)
}

#[test]
fn a_parameter_type_naming_no_node_is_an_invalid_binder_reference() {
    // Address 1 is the length byte of the wrapper, not the start of a node.
    let bytes = file_with(&param_type(1, 0));
    assert_eq!(
        decode(&bytes, 4),
        Err(UnpickleError::InvalidBinderReference { from: 4, binder: 1 })
    );
    let far = file_with(&param_type(100, 0));
    assert_eq!(
        decode(&far, 4),
        Err(UnpickleError::InvalidBinderReference {
            from: 4,
            binder: 100
        })
    );
}

#[test]
fn a_parameter_type_naming_a_non_binder_is_an_invalid_binder_kind() {
    // Address 2 is `TYPEREFpkg p`, a perfectly good type that binds nothing.
    let bytes = file_with(&param_type(2, 0));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut packages = Packages::new();
    packages.enter(&mut session.store, SymbolOrigin::Synthetic, &["p"]);
    let mut unpickler =
        TastyUnpickler::with_packages(&file, &mut session.store, session.definitions, packages);
    let before = unpickler.index().type_count();

    let result = unpickler.unpickle_type(4);
    assert!(
        matches!(
            result,
            Err(UnpickleError::InvalidBinderKind { from: 4, .. })
        ),
        "{result:?}"
    );
    // Nothing of the failed call survives, not even the package reference.
    assert_eq!(unpickler.index().type_count(), before);
    assert_eq!(unpickler.index().type_at(2), None);
    assert_eq!(unpickler.index().type_at(4), None);
}

#[test]
fn a_parameter_type_naming_itself_is_an_error_not_a_stack_overflow() {
    let bytes = file_with(&param_type(4, 0));
    assert!(matches!(
        decode(&bytes, 4),
        Err(UnpickleError::InvalidReferenceTarget { from: 4, .. })
    ));
}

#[test]
fn two_parameter_types_naming_each_other_are_an_error_not_a_stack_overflow() {
    // PARAMtype at 4 names the one at 8 and vice versa.
    let mut children = param_type(8, 0);
    children.extend(param_type(4, 0));
    let bytes = file_with(&children);
    assert!(matches!(
        decode(&bytes, 4),
        Err(UnpickleError::InvalidReferenceTarget { .. })
    ));
}

#[test]
fn a_malformed_parameter_type_payload_is_a_structural_error() {
    // A trailing byte after the two Nats.
    let bytes = file_with(&length_node(172, &[nat(2), nat(0), nat(0)]));
    assert!(matches!(decode(&bytes, 4), Err(UnpickleError::Ast(_))));
}
