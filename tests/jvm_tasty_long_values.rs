use dotty::core::SourceId;
use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_class;

#[test]
fn semantic_oracle_retains_full_signed_long_values_and_control_flow() {
    let oracle = include_str!("fixtures/jvm_tasty_long_values/oracle.txt");
    for shape in [
        "scala.Long",
        "9000000000L",
        "-9223372036854775808L",
        "choose",
        "return",
    ] {
        assert!(oracle.contains(shape), "missing {shape}");
    }
}

fn emit(methods: &str) -> dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts {
    emit_paired_jvm_tasty_class(
        &format!("package example; class LongValues {{ {methods} }}"),
        "LongValues.scala",
        SourceId::from_index(2473),
    )
    .unwrap_or_else(|e| panic!("{methods}: {e:?}"))
}

#[test]
fn long_literal_wire_values_preserve_all_64_bits() {
    use dotty_tasty::tasty::*;
    let pair = emit(
        "def big(): Long = 9_000_000_000L; def min(): Long = -9223372036854775808L; def max(): Long = 9223372036854775807L; def hex(): Long = 0xffffffffffffffffL; def bin(): Long = 0b101010L",
    );
    let file = TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
    let payload = file.section(StandardSection::Asts).unwrap().payload;
    let index = file.ast_address_index().unwrap();
    let values: Vec<_> = index
        .iter_nodes_with_tag(LONGCONST_TAG)
        .map(|node| {
            let RawTree::Leaf(term) =
                RawTree::decode(&mut Reader::new(&payload[node.offset..])).unwrap()
            else {
                panic!()
            };
            let TermValue::LongInt(value) = term.value else {
                panic!()
            };
            value
        })
        .collect();
    assert_eq!(values, [9_000_000_000, i64::MIN, i64::MAX, -1, 42]);
}

#[test]
fn long_parameters_and_results_have_exact_j_descriptors_and_slots() {
    use dotty_classfile::{attribute::Attribute, class_file::ClassFile, reader::Reader};
    let pair = emit(
        "def direct(x: Long): Long = x; def first(a: Int, x: Long, flag: Boolean, y: Long): Long = x; def last(a: Int, x: Long, flag: Boolean, y: Long): Long = y; def intResult(x: Long): Int = 7",
    );
    let file = ClassFile::decode(&mut Reader::new(pair.classfile_bytes())).unwrap();
    for (name, descriptor, slots, expected) in [
        ("direct", "(J)J", 3, &[0x1f, 0xad][..]),
        ("first", "(IJZJ)J", 7, &[0x20, 0xad][..]),
        ("last", "(IJZJ)J", 7, &[0x16, 5, 0xad][..]),
        ("intResult", "(J)I", 3, &[0x10, 7, 0xac][..]),
    ] {
        let method = file
            .methods
            .iter()
            .find(|m| file.constant_pool.utf8(m.name_index) == Ok(name))
            .unwrap();
        assert_eq!(
            file.constant_pool.utf8(method.descriptor_index),
            Ok(descriptor)
        );
        let [Attribute::Code(code)] = method.attributes.as_slice() else {
            panic!()
        };
        assert_eq!(code.max_locals, slots);
        assert_eq!(code.code, expected);
    }
}

#[test]
fn long_locals_if_while_and_local_returns_compose_in_the_real_pipeline() {
    for method in [
        "def f(x: Long): Long = { val saved: Long = x; saved }",
        "def f(x: Long): Long = { val saved = x; saved }",
        "def f(): Long = { val saved = 9000000000L; saved }",
        "def f(a: Long, b: Long): Long = { var saved: Long = a; saved = b; saved }",
        "def f(flag: Boolean, a: Long, b: Long): Long = if flag then a else b",
        "def f(flag: Boolean): Long = if flag then 1L else 9000000000L",
        "def f(n: Int, a: Long, b: Long): Long = { var saved: Long = a; var i: Int = n; while i > 0 do { saved = b; i = i - 1 }; saved }",
        "def f(flag: Boolean, a: Long, b: Long): Long = { if flag then return a; b }",
        "def f(flag: Boolean, a: Long, b: Long): Long = if flag then return a else return b",
        "def f(n: Int, a: Long, b: Long): Long = { var saved: Long = a; var i: Int = n; while i > 0 do { if i == 3 then return a; saved = b; i = i - 1 }; saved }",
        "def f(flag: Boolean, a: Long, b: Long): Long = { val saved: Long = if flag then a else b; saved }",
        "def f(flag: Boolean, a: Long, b: Long): Long = { var saved: Long = a; saved = if flag then b else a; saved }",
    ] {
        emit(method);
    }
}

#[test]
fn long_matches_and_proved_abrupt_long_tails_emit_without_a_placeholder_value() {
    for method in [
        "def f(x: Int, a: Long, b: Long): Long = x match { case 0 => a; case _ => b }",
        "def f(x: Int, a: Long, b: Long): Long = x match { case n if n < 0 => a; case _ => b }",
        "def f(x: Int): Long = x match { case 0 => 1L; case _ => 9000000000L }",
        "def f(x: Int, a: Long, b: Long): Long = x match { case 0 => return a; case _ => b }",
        "def f(x: Int, a: Long, b: Long): Long = x match { case 0 => return a; case _ => return b }",
        "def f(flag: Boolean, a: Long, b: Long): Long = if flag then { return a; b } else b",
        "def f(x: Int, a: Long, b: Long): Long = x match { case 0 => { return a; b }; case _ => b }",
        "def f(flag: Boolean, a: Long, b: Long): Long = if flag then { val saved: Long = b; return a; saved } else b",
    ] {
        emit(method);
    }
}

#[test]
fn the_full_long_fixture_is_deterministic_across_fresh_sessions() {
    let source = include_str!("fixtures/jvm_tasty_long_values/LongValues.scala");
    let first = emit_paired_jvm_tasty_class(source, "LongValues.scala", SourceId::from_index(2473))
        .unwrap();
    let second =
        emit_paired_jvm_tasty_class(source, "LongValues.scala", SourceId::from_index(2474))
            .unwrap();
    assert_eq!(first.classfile_bytes(), second.classfile_bytes());
    assert_eq!(first.tasty_bytes(), second.tasty_bytes());
}

#[test]
fn pinned_scala_consumes_and_recompiles_original_rust_long_tasty() {
    if std::env::var("DOTTY_REQUIRE_SCALA_TASTY_CONSUMER").as_deref() != Ok("1") {
        return;
    }
    let pair = emit_paired_jvm_tasty_class(
        include_str!("fixtures/jvm_tasty_long_values/LongValues.scala"),
        "LongValues.scala",
        SourceId::from_index(2473),
    )
    .unwrap();
    let directory =
        std::env::temp_dir().join(format!("dotty-long-value-scala-{}", std::process::id()));
    std::fs::create_dir_all(directory.join("example")).unwrap();
    std::fs::write(
        directory.join(pair.classfile_path()),
        pair.classfile_bytes(),
    )
    .unwrap();
    std::fs::write(directory.join(pair.tasty_path()), pair.tasty_bytes()).unwrap();
    let output = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/jvm_tasty_long_values/run.py"
        ))
        .arg(&directory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    println!("{}", String::from_utf8_lossy(&output.stdout));
    std::fs::remove_dir_all(directory).unwrap();
}
