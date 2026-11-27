use dotty::core::SourceId;
use dotty::jvm_tasty_artifacts::{PairedJvmTastyArtifacts, emit_paired_jvm_tasty_class};

const SOURCE: &str =
    include_str!("../fixtures/jvm_tasty_numeric_ascriptions/NumericAscriptions.scala");

fn emit(methods: &str) -> PairedJvmTastyArtifacts {
    emit_paired_jvm_tasty_class(
        &format!("package example; class NumericAscriptions {{ {methods} }}"),
        "NumericAscriptions.scala",
        SourceId::from_index(2479),
    )
    .unwrap_or_else(|e| panic!("{methods}: {e:?}"))
}

#[test]
fn numeric_ascriptions_retain_each_typed_wrapper_and_primitive_type_syntax() {
    use dotty_tasty::tasty::*;
    for (ty, literal, expected) in [
        (
            "Long",
            "9223372036854775807L",
            ConstantValue::Long(i64::MAX),
        ),
        ("Float", "-0.0f", ConstantValue::FloatBits(0x80000000)),
        ("Double", "4.9e-324", ConstantValue::DoubleBits(1)),
    ] {
        let pair = emit(&format!("def f(): {ty} = (({literal}: {ty}): {ty})"));
        let file = TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
        let index = file.ast_address_index().unwrap();
        let wrappers: Vec<_> = index.iter().filter(|n| n.tag == TYPED_TAG).collect();
        assert_eq!(wrappers.len(), 2);
        for wrapper in &wrappers {
            let typed = wrapper.decode_typed().unwrap();
            let RawTree::NatAst { tag, value, .. } = typed.type_tree else {
                panic!("explicit type syntax")
            };
            assert_eq!(tag, IDENTTPT_TAG);
            assert_eq!(file.render_name(value).unwrap(), ty);
        }
        let typed = wrappers[1].decode_typed().unwrap();
        let RawTree::Leaf(literal) = typed.expression else {
            panic!("literal")
        };
        assert_eq!(literal.constant_value().unwrap(), Some(expected));
    }
}

#[test]
fn numeric_ascription_fixture_keeps_original_wrappers_and_is_deterministic() {
    let first = emit_paired_jvm_tasty_class(
        SOURCE,
        "NumericAscriptions.scala",
        SourceId::from_index(2479),
    )
    .unwrap();
    let second = emit_paired_jvm_tasty_class(
        SOURCE,
        "NumericAscriptions.scala",
        SourceId::from_index(2480),
    )
    .unwrap();
    assert_eq!(first.classfile_bytes(), second.classfile_bytes());
    assert_eq!(first.tasty_bytes(), second.tasty_bytes());
    let file =
        dotty_tasty::tasty::TastyFile::parse_and_validate_scala_3_9(first.tasty_bytes()).unwrap();
    let index = file.ast_address_index().unwrap();
    assert_eq!(
        index
            .iter_nodes()
            .filter(|n| n.tag == dotty_tasty::tasty::TYPED_TAG)
            .count(),
        // The source has 83 explicit wrappers. Scala also synthesizes widening
        // wrappers around some local reads; Rust preserves its canonical trees.
        83
    );
}
