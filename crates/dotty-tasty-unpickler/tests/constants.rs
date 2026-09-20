//! Milestone 2c2: constant types (`UNITconst` .. `STRINGconst`, `CLASSconst`),
//! over the real Scala 3.9.0 `Constants.tasty`
//! (`tests/fixtures/semantic/Constants.scala`), plus small synthetic files for
//! the bit patterns the compiler cannot be made to write from source.
//!
//! A constant is written as its own node, so the initializer of each `val`
//! in the fixture is a constant type node.
use dotty_core::Definitions;
use dotty_core::store::SemanticStore;
use dotty_core::types::{Constant, Type};
use dotty_tasty::tasty::{StandardSection, TastyFile};
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

const CONSTANTS: &[u8] = include_bytes!("fixtures/semantic/Constants.tasty");

const UNIT: u32 = 115;
const TRUE: u32 = 123;
const FALSE: u32 = 129;
const NULL: u32 = 139;
const BYTE: u32 = 149;
const SHORT: u32 = 160;
const CHAR: u32 = 170;
const MAX_CHAR: u32 = 181;
const SURROGATE: u32 = 193;
const INT: u32 = 204;
const LONG: u32 = 213;
const FLOAT: u32 = 227;
const DOUBLE: u32 = 240;
const NEGATIVE_ZERO: u32 = 257;
const STRING: u32 = 275;
const UNICODE: u32 = 283;
const EMPTY: u32 = 291;
/// The unit's only `CLASSconst`, in the `VALDEF` at 67; its child (80) is a
/// `SHAREDtype` link to the `Constants` module reference at 47.
const CLASS: u32 = 79;
const CLASS_CHILD: u32 = 80;

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

/// The constant type at `at` in a freshly entered unit.
fn constant_at(at: u32) -> Constant {
    let file = TastyFile::parse_scala_3_9(CONSTANTS).unwrap();
    let mut session = Session::new();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    unpickler.enter_symbols().unwrap();
    let id = unpickler.unpickle_type(at).unwrap();
    drop(unpickler);
    let Type::Constant(constant) = session.store.types.get(id) else {
        panic!("expected a constant, got {:?}", session.store.types.get(id));
    };
    constant.clone()
}

fn string_at(at: u32) -> String {
    let file = TastyFile::parse_scala_3_9(CONSTANTS).unwrap();
    let mut session = Session::new();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    unpickler.enter_symbols().unwrap();
    let id = unpickler.unpickle_type(at).unwrap();
    drop(unpickler);
    let Type::Constant(Constant::String(name)) = session.store.types.get(id) else {
        panic!("expected a string constant");
    };
    session.store.names.resolve(*name).to_string()
}

#[test]
fn every_primitive_constant_kind_decodes_to_its_value() {
    assert_eq!(constant_at(UNIT), Constant::Unit);
    assert_eq!(constant_at(TRUE), Constant::Boolean(true));
    assert_eq!(constant_at(FALSE), Constant::Boolean(false));
    assert_eq!(constant_at(NULL), Constant::Null);
    assert_eq!(constant_at(BYTE), Constant::Byte(7));
    assert_eq!(constant_at(SHORT), Constant::Short(-300));
    assert_eq!(constant_at(INT), Constant::Int(42));
    assert_eq!(constant_at(LONG), Constant::Long(1_234_567_890_123));
}

#[test]
fn a_char_constant_is_a_sixteen_bit_code_unit() {
    assert_eq!(constant_at(CHAR), Constant::Char(u16::from(b'x')));
    assert_eq!(constant_at(MAX_CHAR), Constant::Char(0xFFFF));
    // A lone surrogate is a legal Scala `Char` and not a Rust `char`.
    let surrogate = constant_at(SURROGATE);
    assert_eq!(surrogate, Constant::Char(0xD800));
    assert_eq!(surrogate.as_char(), None);
}

#[test]
fn floating_point_constants_keep_their_bits() {
    assert_eq!(constant_at(FLOAT), Constant::float(1.5));
    assert_eq!(constant_at(DOUBLE), Constant::double(2.25));

    let negative_zero = constant_at(NEGATIVE_ZERO);
    assert_eq!(negative_zero, Constant::DoubleBits(0x8000_0000_0000_0000));
    assert_ne!(negative_zero, Constant::double(0.0));
}

#[test]
fn string_constants_carry_their_text() {
    assert_eq!(string_at(STRING), "hello");
    assert_eq!(string_at(UNICODE), "héllo ✓");
    assert_eq!(string_at(EMPTY), "");
}

#[test]
fn a_class_constant_holds_the_type_of_its_child() {
    let file = TastyFile::parse_scala_3_9(CONSTANTS).unwrap();
    let mut session = Session::new();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    unpickler.enter_symbols().unwrap();
    let id = unpickler.unpickle_type(CLASS).unwrap();
    let child = unpickler.unpickle_type(CLASS_CHILD).unwrap();
    drop(unpickler);

    // The stored id is the child's own, not a fresh type and not a wrapper.
    assert_eq!(
        session.store.types.get(id),
        &Type::Constant(Constant::Class(child))
    );
    let Type::TermRef { symbol, .. } = session.store.types.get(child) else {
        panic!(
            "expected a term reference, got {:?}",
            session.store.types.get(child)
        );
    };
    let name = session.store.symbols.get(*symbol).name.text();
    assert_eq!(session.store.names.resolve(name), "Constants");
}

#[test]
fn a_class_constant_child_goes_through_the_normal_pipeline() {
    // `CLASSconst` over an inline `INTconst`: not a class symbol reference.
    let bytes = file_with_ast(&length_node(165, &[92, 70, 0x81, 66, 0x80]));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    // The second operand is a leaf the decoder does not support: only the
    // class constant is decoded here.
    let id = unpickler.unpickle_type(2).unwrap();
    let child = unpickler.unpickle_type(3).unwrap();
    drop(unpickler);

    assert_eq!(
        session.store.types.get(id),
        &Type::Constant(Constant::Class(child))
    );
    assert_eq!(
        session.store.types.get(child),
        &Type::Constant(Constant::Int(1))
    );
}

#[test]
fn decoding_a_constant_twice_returns_the_same_type_id() {
    let file = TastyFile::parse_scala_3_9(CONSTANTS).unwrap();
    let mut session = Session::new();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    unpickler.enter_symbols().unwrap();

    for at in [INT, STRING, CLASS, FLOAT] {
        let first = unpickler.unpickle_type(at).unwrap();
        assert_eq!(unpickler.unpickle_type(at), Ok(first));
        assert_eq!(unpickler.index().type_at(at), Some(first));
    }
}

#[test]
fn equal_constants_at_different_addresses_are_not_interned() {
    // Two `Int(1)` operands of an intersection, at different addresses.
    let bytes = file_with_ast(&length_node(165, &[70, 0x81, 70, 0x81]));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    let first = unpickler.unpickle_type(2).unwrap();
    let second = unpickler.unpickle_type(4).unwrap();

    assert_ne!(first, second);
    drop(unpickler);
    assert_eq!(
        session.store.types.get(first),
        session.store.types.get(second)
    );
}

#[test]
fn a_failing_class_child_is_reported_and_leaves_no_trace() {
    let patched = retagged(CONSTANTS, CLASS_CHILD as usize, 61, 66);
    let file = TastyFile::parse_scala_3_9(&patched).unwrap();
    let mut session = Session::new();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    unpickler.enter_symbols().unwrap();
    let before = unpickler.index().type_count();

    assert!(matches!(
        unpickler.unpickle_type(CLASS),
        Err(UnpickleError::UnsupportedType { tag: 66, .. })
    ));
    assert_eq!(unpickler.index().type_count(), before);
    assert_eq!(unpickler.index().type_at(CLASS), None);
}

// Synthetic files

/// TASTy's signed variable-length integer: big-endian base-128 groups, the
/// last one flagged with the high bit.
fn int_bytes(value: i64) -> Vec<u8> {
    fn go(value: i64, out: &mut Vec<u8>) {
        let rest = value >> 7;
        if rest != -((value >> 6) & 1) {
            go(rest, out);
        }
        out.push((value & 0x7f) as u8);
    }
    let mut out = Vec::new();
    go(value, &mut out);
    *out.last_mut().unwrap() |= 0x80;
    out
}

fn nat_bytes(mut value: u64) -> Vec<u8> {
    let mut groups = vec![(value & 0x7f) as u8 | 0x80];
    value >>= 7;
    while value != 0 {
        groups.push((value & 0x7f) as u8);
        value >>= 7;
    }
    groups.reverse();
    groups
}

fn file_with_ast(ast: &[u8]) -> Vec<u8> {
    let names = dotty_tasty::tasty::NameTable::from_entries(vec![
        dotty_tasty::tasty::RawName::Utf8("ASTs".to_owned()),
        dotty_tasty::tasty::RawName::Utf8("p".to_owned()),
    ])
    .unwrap();
    let sections =
        dotty_tasty::tasty::SectionTable::from_sections(vec![dotty_tasty::tasty::Section::new(
            0, ast,
        )]);
    TastyFile::from_parts(
        dotty_tasty::tasty::Header {
            major_version: 28,
            minor_version: 9,
            experimental_version: 0,
            tooling_version: "Scala 3.9.0".to_owned(),
            uuid: [0; 16],
        },
        names,
        sections,
    )
    .unwrap()
    .encode()
    .unwrap()
}

fn retagged(bytes: &[u8], at: usize, from: u8, to: u8) -> Vec<u8> {
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let payload = file.section(StandardSection::Asts).unwrap().payload;
    assert_eq!(
        payload[at], from,
        "the fixture no longer has {from} at {at}"
    );
    let start = payload.as_ptr() as usize - bytes.as_ptr() as usize;
    let mut patched = bytes.to_vec();
    patched[start + at] = to;
    patched
}

/// `tag length payload`, the only shape a top-level AST node may have.
fn length_node(tag: u8, payload: &[u8]) -> Vec<u8> {
    let mut node = vec![tag];
    node.extend(nat_bytes(payload.len() as u64));
    node.extend(payload);
    node
}

/// Decodes the constant node `ast`, written as the only child of a
/// `FLEXIBLEtype` (a top-level node must be length-prefixed) and so at
/// address 2.
fn decode_single(ast: &[u8]) -> Result<Type, UnpickleError> {
    let bytes = file_with_ast(&length_node(193, ast));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    let id = unpickler.unpickle_type(2)?;
    drop(unpickler);
    Ok(session.store.types.get(id).clone())
}

#[test]
fn a_float_nan_payload_and_signed_zero_survive_exactly() {
    for bits in [0x7fc0_1234_u32, 0xffc0_0001, 0x8000_0000, 0x7f80_0001] {
        let mut ast = vec![72];
        ast.extend(int_bytes(i64::from(bits as i32)));
        assert_eq!(
            decode_single(&ast),
            Ok(Type::Constant(Constant::FloatBits(bits))),
            "{bits:#x}"
        );
    }
}

#[test]
fn a_double_nan_payload_and_signed_zero_survive_exactly() {
    for bits in [
        0x7ff8_0000_0000_1234_u64,
        0xfff8_0000_0000_0001,
        0x8000_0000_0000_0000,
        0x7ff0_0000_0000_0001,
    ] {
        let mut ast = vec![73];
        ast.extend(int_bytes(bits as i64));
        assert_eq!(
            decode_single(&ast),
            Ok(Type::Constant(Constant::DoubleBits(bits))),
            "{bits:#x}"
        );
    }
}

#[test]
fn every_char_edge_code_unit_is_kept() {
    for unit in [0u16, 0x7f, 0xD7FF, 0xD800, 0xDBFF, 0xDC00, 0xDFFF, 0xFFFF] {
        let mut ast = vec![69];
        ast.extend(nat_bytes(u64::from(unit)));
        assert_eq!(
            decode_single(&ast),
            Ok(Type::Constant(Constant::Char(unit))),
            "{unit:#x}"
        );
    }
}

#[test]
fn narrow_and_integer_edges_are_exact() {
    let cases: [(u8, i64, Constant); 6] = [
        (67, -128, Constant::Byte(i8::MIN)),
        (67, 127, Constant::Byte(i8::MAX)),
        (68, -32768, Constant::Short(i16::MIN)),
        (68, 32767, Constant::Short(i16::MAX)),
        (70, i64::from(i32::MIN), Constant::Int(i32::MIN)),
        (71, i64::MIN, Constant::Long(i64::MIN)),
    ];
    for (tag, value, expected) in cases {
        let mut ast = vec![tag];
        ast.extend(int_bytes(value));
        assert_eq!(decode_single(&ast), Ok(Type::Constant(expected)), "{tag}");
    }
}

#[test]
fn an_out_of_range_constant_is_an_error_not_a_wrap() {
    let mut byte = vec![67];
    byte.extend(int_bytes(200));
    let mut char = vec![69];
    char.extend(nat_bytes(0x1_0000));

    assert!(matches!(decode_single(&byte), Err(UnpickleError::Ast(_))));
    assert!(matches!(decode_single(&char), Err(UnpickleError::Ast(_))));
}

#[test]
fn a_failure_after_a_constant_decoded_forgets_the_constant() {
    // `ANDtype` over an `INTconst` and a tag the decoder does not support.
    let mut payload = vec![70, 0x81];
    payload.push(66);
    payload.extend(nat_bytes(0));
    let bytes = file_with_ast(&length_node(165, &payload));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    let before = unpickler.index().type_count();

    assert!(matches!(
        unpickler.unpickle_type(0),
        Err(UnpickleError::UnsupportedType { tag: 66, .. })
    ));
    assert_eq!(unpickler.index().type_count(), before);
    assert_eq!(unpickler.index().type_at(2), None);
}

/// A file over the names `ASTs`, `a`, `b`, `a.b` (qualified, entry 3).
fn file_with_qualified_name(ast: &[u8]) -> Vec<u8> {
    use dotty_tasty::tasty::{Header, NameTable, RawName, Section, SectionTable};
    let names = NameTable::from_entries(vec![
        RawName::Utf8("ASTs".to_owned()),
        RawName::Utf8("a".to_owned()),
        RawName::Utf8("b".to_owned()),
        RawName::Qualified {
            prefix: 1,
            selector: 2,
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

fn string_constant_over(name_ref: u8) -> Result<String, UnpickleError> {
    let bytes = file_with_qualified_name(&length_node(193, &[74, 0x80 | name_ref]));
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    let id = unpickler.unpickle_type(2)?;
    drop(unpickler);
    let Type::Constant(Constant::String(name)) = session.store.types.get(id) else {
        panic!("expected a string constant");
    };
    Ok(session.store.names.resolve(*name).to_string())
}

#[test]
fn a_string_constant_may_name_any_valid_name_entry() {
    // `STRINGconst` carries a `NameRef`, which Dotty reads as
    // `readName().toString`: a qualified entry is a string, spelled `a.b`.
    assert_eq!(string_constant_over(1).as_deref(), Ok("a"));
    assert_eq!(string_constant_over(3).as_deref(), Ok("a.b"));
}

#[test]
fn a_string_constant_with_a_missing_name_is_an_error() {
    assert_eq!(
        string_constant_over(9),
        Err(UnpickleError::InvalidNameReference { reference: 9 })
    );
}
