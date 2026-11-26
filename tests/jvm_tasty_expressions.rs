use dotty::core::SourceId;
use dotty::jvm_tasty_artifacts::{PairedArtifactError, emit_paired_jvm_tasty_class};
use dotty::jvm_tasty_plan::PairedPlanError;
use dotty_jvm_tasty_contract::JvmTastyContractError;

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

#[test]
fn primitive_matches_emit_ordered_cases_for_all_three_result_types() {
    let pair = emit(
        "def classify(x: Int): Int = x match { case 0 => 10; case 1 => 20; case _ => 30 }; def flag(x: Int): Boolean = x match { case 0 => false; case _ => true }; def nothing(x: Int): Unit = x match { case 0 => (); case _ => () }",
    );
    let file =
        dotty_tasty::tasty::TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
    let index = file.ast_address_index().unwrap();
    let matches: Vec<_> = index
        .iter()
        .filter(|n| n.tag == dotty_tasty::tasty::MATCH_TAG)
        .map(|n| n.decode_match().unwrap())
        .collect();
    assert_eq!(
        matches.iter().map(|m| m.cases.len()).collect::<Vec<_>>(),
        [3, 2, 2]
    );
    assert_eq!(
        index
            .iter()
            .filter(|n| n.tag == dotty_tasty::tasty::CASEDEF_TAG)
            .count(),
        7
    );
}

#[test]
fn unsupported_matches_reject_the_pair_before_artifacts_escape() {
    for body in [
        "x match { case 0 => 10 }",
        "x match { case 0 | 1 => 1; case _ => 2 }",
    ] {
        let source =
            format!("package example\nclass Expressions {{ def value(x: Int): Int = {body} }}");
        assert!(
            matches!(
                emit_paired_jvm_tasty_class(
                    &source,
                    "Expressions.scala",
                    SourceId::from_index(2470)
                ),
                Err(PairedArtifactError::Plan(
                    PairedPlanError::Tasty(_)
                        | PairedPlanError::Contract(JvmTastyContractError::TastyPlan(_))
                ))
            ),
            "{body}"
        );
    }
}

#[test]
fn pinned_scala_reads_and_recompiles_rust_expression_tasty() {
    if std::env::var("DOTTY_REQUIRE_SCALA_TASTY_CONSUMER").as_deref() != Ok("1") {
        return;
    }
    let pair = emit_paired_jvm_tasty_class(
        include_str!("fixtures/jvm_tasty_expressions/Expressions.scala"),
        "Expressions.scala",
        SourceId::from_index(2470),
    )
    .unwrap();
    let directory =
        std::env::temp_dir().join(format!("dotty-expressions-scala-{}", std::process::id()));
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
            "/tests/fixtures/jvm_tasty_expressions/run.py"
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

#[test]
fn composed_expression_fixture_emits_a_deterministic_linked_pair() {
    let source = include_str!("fixtures/jvm_tasty_expressions/Expressions.scala");
    let first =
        emit_paired_jvm_tasty_class(source, "Expressions.scala", SourceId::from_index(2470))
            .unwrap();
    let second =
        emit_paired_jvm_tasty_class(source, "Expressions.scala", SourceId::from_index(2471))
            .unwrap();
    assert_eq!(first.classfile_bytes(), second.classfile_bytes());
    assert_eq!(first.tasty_bytes(), second.tasty_bytes());
    assert_eq!(first.tasty_link_id(), second.tasty_link_id());
}

#[test]
fn paired_contract_rejects_missing_canonical_primitive_local_evidence() {
    let source = include_str!("fixtures/jvm_tasty_expressions/Expressions.scala");
    dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(source, "Expressions.scala", SourceId::from_index(2470), |plan| {
        let before = plan.jvm_unit.store.checkpoint();
        let mut classes = plan.jvm_unit.classes.clone();
        let method = classes[0].methods.iter_mut().find(|method| !method.locals.is_empty()).unwrap();
        let symbol = method.method;
        method.locals.clear();
        let forged = dotty::jvm::JvmCompilationUnit { store: plan.jvm_unit.store, typed_ast: plan.jvm_unit.typed_ast, classes };
        let error = dotty_jvm_tasty_contract::validate_jvm_tasty_class(&forged, plan.tasty_plan).err().expect("missing local evidence must fail");
        assert!(matches!(error, JvmTastyContractError::LocalBindingMismatch { method } if method == symbol), "{error:?}");
        assert_eq!(before, plan.jvm_unit.store.checkpoint());
    }).unwrap();
}

fn jdk_available() -> bool {
    let version = std::process::Command::new("java")
        .arg("-version")
        .output()
        .ok();
    let major = version.as_ref().and_then(|output| {
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        text.split('"')
            .nth(1)?
            .split('.')
            .next()?
            .parse::<u32>()
            .ok()
    });
    let available = major.is_some_and(|major| major >= 21);
    if std::env::var("DOTTY_CLASSFILE_REQUIRE_JDK").as_deref() == Ok("1") {
        assert!(
            available,
            "mandatory expression runtime gate requires JDK 21+"
        );
    }
    available
}

fn run_java(pair: &dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts, name: &str, source: &str) {
    let directory =
        std::env::temp_dir().join(format!("dotty-expressions-{}-{name}", std::process::id()));
    std::fs::create_dir_all(directory.join("example")).unwrap();
    std::fs::write(
        directory.join(pair.classfile_path()),
        pair.classfile_bytes(),
    )
    .unwrap();
    std::fs::write(directory.join(pair.tasty_path()), pair.tasty_bytes()).unwrap();
    let client = directory.join(format!("{name}.java"));
    std::fs::write(&client, source).unwrap();
    let compile = std::process::Command::new("javac")
        .args(["--release", "21", "-cp"])
        .arg(&directory)
        .arg("-d")
        .arg(&directory)
        .arg(&client)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = std::process::Command::new("java")
        .args(["-Xverify:all", "-ea", "-cp"])
        .arg(&directory)
        .arg(name)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "expression parity ok"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn composed_expressions_and_effectful_operands_execute_on_jdk21() {
    if !jdk_available() {
        return;
    }
    let pair = emit_paired_jvm_tasty_class(
        include_str!("fixtures/jvm_tasty_expressions/Expressions.scala"),
        "Expressions.scala",
        SourceId::from_index(2470),
    )
    .unwrap();
    run_java(
        &pair,
        "JavaConsumer",
        include_str!("fixtures/jvm_tasty_expressions/JavaConsumer.java"),
    );
    let effects = emit(
        "private var count: Int = 0; def left(): Int = { count = count * 10 + 1; 7 }; def right(): Int = { count = count * 10 + 2; 3 }; def read(): Int = count; def mask(): Int = left() & right(); def same(): Boolean = left() == right(); def different(): Boolean = left() != right()",
    );
    run_java(
        &effects,
        "JavaEffects",
        r#"public class JavaEffects {
      public static void main(String[] args) {
        example.Expressions x = new example.Expressions();
        if (x.mask() != 3 || x.read() != 12) throw new AssertionError("and operand order/count");
        if (x.same() || x.read() != 1212) throw new AssertionError("equal operand order/count");
        if (!x.different() || x.read() != 121212) throw new AssertionError("not-equal operand order/count");
        System.out.println("expression parity ok");
      }
    }"#,
    );
}
