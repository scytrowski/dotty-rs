use dotty::core::SourceId;
use dotty::jvm_tasty_artifacts::{PairedArtifactError, emit_paired_jvm_tasty_class};
use dotty::jvm_tasty_plan::PairedPlanError;
use dotty_jvm_tasty_contract::JvmTastyContractError;

#[test]
fn pending_expression_forms_reject_before_returning_a_partial_pair() {
    for method in ["def value(x: Int): Int = x match { case 0 => 10; case _ => 20 }"] {
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

#[test]
fn integer_bitwise_and_emits_iand_and_the_signed_scala_operator() {
    let pair = emit("def mask(x: Int): Int = x & 255");
    let class = dotty_classfile::class_file::ClassFile::decode(
        &mut dotty_classfile::reader::Reader::new(pair.classfile_bytes()),
    )
    .unwrap();
    let method = class
        .methods
        .iter()
        .find(|m| class.constant_pool.utf8(m.name_index) == Ok("mask"))
        .unwrap();
    let [dotty_classfile::attribute::Attribute::Code(code)] = method.attributes.as_slice() else {
        panic!("method code")
    };
    assert_eq!(code.code, &[0x1b, 0x11, 0x00, 0xff, 0x7e, 0xac]);
    let file =
        dotty_tasty::tasty::TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
    let index = file.ast_address_index().unwrap();
    let selections: Vec<_> = index
        .iter()
        .filter(|n| n.tag == dotty_tasty::tasty::SELECTIN_TAG)
        .map(|n| {
            file.names()
                .render_signed_name(n.decode_select_in().unwrap().name)
                .unwrap()
                .unwrap()
        })
        .collect();
    assert!(
        selections.contains(&dotty_tasty::tasty::RenderedSignedName::Signed {
            original: "&".into(),
            signature: dotty_tasty::tasty::RenderedNameSignature {
                result: "scala.Int".into(),
                parameters: vec![dotty_tasty::tasty::RenderedParamSig::TermParameter(
                    "scala.Int".into()
                )],
            },
        }),
        "{selections:?}"
    );
}

#[test]
fn integer_equality_emits_int_branches_and_specialized_tasty_signatures() {
    let pair = emit(
        "def equal(x: Int, y: Int): Boolean = x == y; def different(x: Int, y: Int): Boolean = x != y",
    );
    let class = dotty_classfile::class_file::ClassFile::decode(
        &mut dotty_classfile::reader::Reader::new(pair.classfile_bytes()),
    )
    .unwrap();
    for (name, branch) in [("equal", 0x9f), ("different", 0xa0)] {
        let method = class
            .methods
            .iter()
            .find(|m| class.constant_pool.utf8(m.name_index) == Ok(name))
            .unwrap();
        let [dotty_classfile::attribute::Attribute::Code(code)] = method.attributes.as_slice()
        else {
            panic!("method code")
        };
        assert_eq!(&code.code[..3], &[0x1b, 0x1c, branch]);
    }
    let file =
        dotty_tasty::tasty::TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
    for spelling in ["==", "!="] {
        assert!(
            file.ast_address_index()
                .unwrap()
                .iter()
                .filter(|n| n.tag == dotty_tasty::tasty::SELECTIN_TAG)
                .any(|n| {
                    file.names()
                        .render_signed_name(n.decode_select_in().unwrap().name)
                        .unwrap()
                        == Some(dotty_tasty::tasty::RenderedSignedName::Signed {
                            original: spelling.into(),
                            signature: dotty_tasty::tasty::RenderedNameSignature {
                                result: "scala.Boolean".into(),
                                parameters: vec![
                                    dotty_tasty::tasty::RenderedParamSig::TermParameter(
                                        "scala.Int".into(),
                                    ),
                                ],
                            },
                        })
                })
        );
    }
}
