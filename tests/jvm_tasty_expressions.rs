use dotty::core::SourceId;
use dotty::jvm_tasty_artifacts::{PairedArtifactError, emit_paired_jvm_tasty_class};
use dotty::jvm_tasty_plan::PairedPlanError;
use dotty_jvm_tasty_contract::JvmTastyContractError;

#[test]
fn pending_expression_forms_reject_before_returning_a_partial_pair() {
    for method in [
        "def value(x: Int): Int = x & 255",
        "def value(x: Int, y: Int): Boolean = x == y",
        "def value(x: Int): Int = x match { case 0 => 10; case _ => 20 }",
    ] {
        let source = format!("package example\nclass Expressions {{ {method} }}");
        let error =
            emit_paired_jvm_tasty_class(&source, "Expressions.scala", SourceId::from_index(2470))
                .expect_err("current expression gap must not yield either artifact");
        assert!(
            matches!(
                error,
                PairedArtifactError::Plan(
                    PairedPlanError::Tasty(_)
                        | PairedPlanError::Contract(JvmTastyContractError::TastyPlan(_))
                )
            ),
            "{method}: {error:?}"
        );
    }
}

fn emit(methods: &str) -> dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts {
    let source = format!("package example\nclass Expressions {{ {methods} }}");
    emit_paired_jvm_tasty_class(&source, "Expressions.scala", SourceId::from_index(2470))
        .unwrap_or_else(|error| panic!("{methods}: {error:?}"))
}

#[test]
fn primitive_ascriptions_emit_typed_tasty_and_no_runtime_conversion() {
    let pair = emit(
        "def intValue(x: Int): Int = (x: Int); def booleanValue(x: Boolean): Boolean = (x: Boolean); def unitValue(): Unit = ((): Unit)",
    );
    let file =
        dotty_tasty::tasty::TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
    let index = file.ast_address_index().unwrap();
    assert_eq!(
        index
            .iter()
            .filter(|node| node.tag == dotty_tasty::tasty::TYPED_TAG)
            .count(),
        3
    );
    let class = dotty_classfile::class_file::ClassFile::decode(
        &mut dotty_classfile::reader::Reader::new(pair.classfile_bytes()),
    )
    .unwrap();
    for (name, expected) in [
        ("intValue", &[0x1b, 0xac][..]),
        ("booleanValue", &[0x1b, 0xac][..]),
        ("unitValue", &[0xb1][..]),
    ] {
        let method = class
            .methods
            .iter()
            .find(|m| class.constant_pool.utf8(m.name_index) == Ok(name))
            .unwrap();
        let [dotty_classfile::attribute::Attribute::Code(code)] = method.attributes.as_slice()
        else {
            panic!("method code")
        };
        assert_eq!(code.code, expected);
    }
}

#[test]
fn official_scala_oracle_preserves_ascriptions_matches_and_operator_names() {
    let oracle = include_str!("fixtures/jvm_tasty_expressions/oracle.txt");
    for shape in [
        "TYPED",
        "MATCH",
        "CASEDEF",
        "intValue",
        "mask",
        "equal",
        "different",
    ] {
        assert!(
            oracle.contains(shape),
            "missing official Scala shape {shape}"
        );
    }
}
