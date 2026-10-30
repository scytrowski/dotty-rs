use dotty::jvm_tasty_artifacts::{
    PairedBatchArtifactError, PairedJvmTastyBatchArtifacts, emit_paired_jvm_tasty_batch,
};
use dotty::jvm_tasty_plan::{PairedPlanError, with_paired_jvm_tasty_batch_plan};
use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::{ConstantPoolEntry, ConstantPoolIndex};
use dotty_classfile::reader::Reader;
use dotty_classfile::tasty_link::TastyLinkId;
use dotty_core::SourceId;
use dotty_jvm_tasty_contract::{
    JvmTastyBatchArtifacts, JvmTastyClassFileArtifact, JvmTastyContractError,
    JvmTastySidecarArtifact, validate_jvm_tasty_batch, validate_jvm_tasty_batch_artifacts,
};
use dotty_tasty::tasty::TastyFile;
use std::path::{Path, PathBuf};
use std::process::Command;

#[allow(dead_code)]
#[path = "support/jvm_tasty_batch_publisher.rs"]
mod batch_publisher;

const PAIR_SOURCE: &str = include_str!("fixtures/jvm_tasty_dual_reference_constructors/Pair.scala");
const RENAMED_REORDERED_SOURCE: &str = "package sample\n\nclass Envelope(firstInput: Token, secondInput: Token):\n  private val firstAlias: Token = firstInput\n  private val secondAlias: Token = secondInput\n  def firstValue(): Int = firstAlias.get()\n  def secondValue(): Int = secondAlias.get()\n\nclass Token(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n";

fn emit_pair(source_id: u32) -> PairedJvmTastyBatchArtifacts {
    emit_paired_jvm_tasty_batch(PAIR_SOURCE, "Pair.scala", SourceId::from_index(source_id))
        .expect("the bounded dual-reference source emits a validated four-file batch")
}

fn pair_artifacts(batch: &PairedJvmTastyBatchArtifacts) -> JvmTastyBatchArtifacts {
    JvmTastyBatchArtifacts {
        classfiles: batch
            .classes()
            .iter()
            .map(|artifact| JvmTastyClassFileArtifact {
                classfile_path: artifact.classfile_path().to_owned(),
                classfile_bytes: artifact.classfile_bytes().to_vec(),
            })
            .collect(),
        sidecars: batch
            .classes()
            .iter()
            .map(|artifact| JvmTastySidecarArtifact {
                tasty_path: artifact.tasty_path().to_owned(),
                tasty_bytes: artifact.tasty_bytes().to_vec(),
            })
            .collect(),
    }
}

fn validate_pair_artifacts(
    source_id: u32,
    artifacts: JvmTastyBatchArtifacts,
) -> Result<(), JvmTastyContractError> {
    with_paired_jvm_tasty_batch_plan(
        PAIR_SOURCE,
        "Pair.scala",
        SourceId::from_index(source_id),
        |plan| {
            let tasty_plans = plan
                .classes
                .iter()
                .map(|class| class.tasty_plan)
                .collect::<Vec<_>>();
            let validated = validate_jvm_tasty_batch(plan.jvm_unit, &tasty_plans)
                .expect("the original source and both class plans validate together");
            validate_jvm_tasty_batch_artifacts(&validated, artifacts)
        },
    )
    .expect("the original source plans successfully")
}

fn decoded_class(bytes: &[u8]) -> ClassFile<'_> {
    let mut reader = Reader::new(bytes);
    let class = ClassFile::decode(&mut reader).unwrap();
    assert!(reader.is_at_end());
    class
}

fn code_offset(bytes: &[u8], method_name: &str) -> usize {
    let class = decoded_class(bytes);
    let method = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == method_name)
        .unwrap();
    let [Attribute::Code(code)] = method.attributes.as_slice() else {
        panic!("{method_name} has a single Code attribute")
    };
    code.code.as_ptr() as usize - bytes.as_ptr() as usize
}

fn constant_pool_index(bytes: &[u8], operand_offset: usize) -> ConstantPoolIndex {
    ConstantPoolIndex(u16::from_be_bytes([
        bytes[operand_offset],
        bytes[operand_offset + 1],
    ]))
}

fn class_method_ref<'a>(
    class: &'a ClassFile<'a>,
    index: ConstantPoolIndex,
) -> (&'a str, (&'a str, &'a str)) {
    let Some(ConstantPoolEntry::Methodref {
        class_index,
        name_and_type_index,
    }) = class.constant_pool.get(index)
    else {
        panic!("constant-pool entry is Methodref")
    };
    (
        class.constant_pool.class_name(*class_index).unwrap(),
        class
            .constant_pool
            .name_and_type(*name_and_type_index)
            .unwrap(),
    )
}

fn class_field_ref<'a>(
    class: &'a ClassFile<'a>,
    index: ConstantPoolIndex,
) -> (&'a str, (&'a str, &'a str)) {
    let Some(ConstantPoolEntry::Fieldref {
        class_index,
        name_and_type_index,
    }) = class.constant_pool.get(index)
    else {
        panic!("constant-pool entry is Fieldref")
    };
    (
        class.constant_pool.class_name(*class_index).unwrap(),
        class
            .constant_pool
            .name_and_type(*name_and_type_index)
            .unwrap(),
    )
}

#[test]
fn dual_reference_pair_emits_four_linked_artifacts_with_ordered_bytecode_and_deterministic_bytes() {
    let first = emit_pair(1879);
    let second = emit_pair(18790);
    assert_ne!(first.source_id(), second.source_id());
    assert_eq!(first.classes().len(), 2);
    assert_eq!(second.classes().len(), 2);
    assert_eq!(
        first
            .classes()
            .iter()
            .flat_map(|class| [class.classfile_path(), class.tasty_path()])
            .collect::<Vec<_>>(),
        [
            "example/Counter.class",
            "example/Counter.tasty",
            "example/Pair.class",
            "example/Pair.tasty",
        ]
    );

    for (left, right) in first.classes().iter().zip(second.classes()) {
        assert_eq!(left.source_id(), first.source_id());
        assert_eq!(left.classfile_path(), right.classfile_path());
        assert_eq!(left.tasty_path(), right.tasty_path());
        assert_eq!(left.classfile_bytes(), right.classfile_bytes());
        assert_eq!(left.tasty_bytes(), right.tasty_bytes());
        assert_eq!(left.tasty_link_id(), right.tasty_link_id());
        let tasty = TastyFile::parse_and_validate_scala_3_9(left.tasty_bytes()).unwrap();
        assert_eq!(
            (
                tasty.header().major_version,
                tasty.header().minor_version,
                tasty.header().experimental_version,
            ),
            (28, 9, 0)
        );
        assert_eq!(
            left.tasty_link_id(),
            TastyLinkId::from_bytes(tasty.header().uuid)
        );
        let class = decoded_class(left.classfile_bytes());
        assert_eq!((class.version.major, class.version.minor), (65, 0));
        assert_eq!(class.tasty_link_id().unwrap(), Some(left.tasty_link_id()));
    }

    let pair = &first.classes()[1];
    assert_eq!(pair.classfile_path(), "example/Pair.class");
    assert_eq!(pair.tasty_path(), "example/Pair.tasty");
    assert_eq!(pair.source_member_order().len(), 4);
    let class = decoded_class(pair.classfile_bytes());
    assert_eq!(
        class.constant_pool.class_name(class.this_class).unwrap(),
        "example/Pair"
    );
    let mut fields = class
        .fields
        .iter()
        .map(|field| {
            (
                class.constant_pool.utf8(field.name_index).unwrap(),
                class.constant_pool.utf8(field.descriptor_index).unwrap(),
                field.access_flags,
            )
        })
        .collect::<Vec<_>>();
    fields.sort_by_key(|field| field.0);
    assert_eq!(fields.len(), 2);
    assert_eq!(
        fields
            .iter()
            .map(|field| (field.0, field.1))
            .collect::<Vec<_>>(),
        [
            ("left", "Lexample/Counter;"),
            ("right", "Lexample/Counter;"),
        ]
    );
    for (_, _, flags) in fields {
        assert!(flags.contains(dotty_classfile::access_flags::ACC_PRIVATE));
        assert!(flags.contains(dotty_classfile::access_flags::ACC_FINAL));
    }

    let constructor = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == "<init>")
        .unwrap();
    assert_eq!(
        class
            .constant_pool
            .utf8(constructor.descriptor_index)
            .unwrap(),
        "(Lexample/Counter;Lexample/Counter;)V"
    );
    let [Attribute::Code(constructor_code)] = constructor.attributes.as_slice() else {
        panic!("Pair primary constructor has one Code attribute")
    };
    assert_eq!(
        constructor_code.code,
        [
            0x2a, 0xb7, 0x00, 0x0f, 0x2a, 0x2b, 0xb5, 0x00, 0x11, 0x2a, 0x2c, 0xb5, 0x00, 0x13,
            0xb1,
        ]
    );
    assert_eq!(
        class_method_ref(&class, constant_pool_index(constructor_code.code, 2)),
        ("java/lang/Object", ("<init>", "()V"))
    );
    assert_eq!(
        class_field_ref(&class, constant_pool_index(constructor_code.code, 7)),
        ("example/Pair", ("left", "Lexample/Counter;"))
    );
    assert_eq!(
        class_field_ref(&class, constant_pool_index(constructor_code.code, 12)),
        ("example/Pair", ("right", "Lexample/Counter;"))
    );

    for (method_name, expected_field, operand_offset) in
        [("leftValue", "left", 2), ("rightValue", "right", 2)]
    {
        let method = class
            .methods
            .iter()
            .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == method_name)
            .unwrap();
        assert_eq!(
            class.constant_pool.utf8(method.descriptor_index).unwrap(),
            "()I"
        );
        let [Attribute::Code(code)] = method.attributes.as_slice() else {
            panic!("{method_name} has one Code attribute")
        };
        assert_eq!(code.code.len(), 8);
        assert_eq!(&code.code[..2], &[0x2a, 0xb4]);
        assert_eq!(&code.code[4..6], &[0xb6, 0x00]);
        assert_eq!(code.code[7], 0xac);
        assert_eq!(
            class_field_ref(&class, constant_pool_index(code.code, operand_offset)),
            ("example/Pair", (expected_field, "Lexample/Counter;"))
        );
        assert_eq!(
            class_method_ref(&class, constant_pool_index(code.code, 5)),
            ("example/Counter", ("get", "()I"))
        );
    }
}

#[test]
fn dual_reference_alias_byte_contract_rejects_wrong_slot_swapped_targets_and_reader_refs() {
    let batch = emit_pair(18791);
    let valid = pair_artifacts(&batch);
    validate_pair_artifacts(18792, valid.clone()).unwrap();
    let pair_index = valid
        .classfiles
        .iter()
        .position(|class| class.classfile_path == "example/Pair.class")
        .unwrap();
    let constructor_offset = code_offset(&valid.classfiles[pair_index].classfile_bytes, "<init>");

    let mut wrong_second_slot = valid.clone();
    wrong_second_slot.classfiles[pair_index].classfile_bytes[constructor_offset + 10] = 0x2b;
    assert!(matches!(
        validate_pair_artifacts(18793, wrong_second_slot),
        Err(JvmTastyContractError::BatchArtifact {
            class_index: 1,
            phase: "reference-field initializer",
            ..
        })
    ));

    let mut swapped_aliases = valid.clone();
    let bytes = &mut swapped_aliases.classfiles[pair_index].classfile_bytes;
    for offset in 0..2 {
        bytes.swap(
            constructor_offset + 7 + offset,
            constructor_offset + 12 + offset,
        );
    }
    assert!(matches!(
        validate_pair_artifacts(18794, swapped_aliases),
        Err(JvmTastyContractError::BatchArtifact {
            class_index: 1,
            phase: "reference-field initializer",
            ..
        })
    ));

    let mut wrong_getfield = valid.clone();
    let left_reader_offset = code_offset(
        &wrong_getfield.classfiles[pair_index].classfile_bytes,
        "leftValue",
    );
    let right_reader_offset = code_offset(
        &wrong_getfield.classfiles[pair_index].classfile_bytes,
        "rightValue",
    );
    let left_reference = wrong_getfield.classfiles[pair_index].classfile_bytes
        [left_reader_offset + 2..left_reader_offset + 4]
        .to_vec();
    wrong_getfield.classfiles[pair_index].classfile_bytes
        [right_reader_offset + 2..right_reader_offset + 4]
        .copy_from_slice(&left_reference);
    assert!(matches!(
        validate_pair_artifacts(18795, wrong_getfield),
        Err(JvmTastyContractError::BatchArtifact {
            class_index: 1,
            phase: "cross-class invocation",
            ..
        })
    ));

    let mut wrong_methodref = valid;
    let bytes = &mut wrong_methodref.classfiles[pair_index].classfile_bytes;
    let constructor_ref = bytes[constructor_offset + 2..constructor_offset + 4].to_vec();
    let right_reader_offset = code_offset(bytes, "rightValue");
    bytes[right_reader_offset + 5..right_reader_offset + 7].copy_from_slice(&constructor_ref);
    assert!(matches!(
        validate_pair_artifacts(18796, wrong_methodref),
        Err(JvmTastyContractError::BatchArtifact {
            class_index: 1,
            phase: "cross-class invocation",
            ..
        })
    ));
}

#[test]
fn dual_reference_batch_artifact_path_link_and_decode_failures_are_rejected() {
    let batch = emit_pair(18797);
    let valid = pair_artifacts(&batch);

    let mut missing_classfile = valid.clone();
    missing_classfile
        .classfiles
        .retain(|classfile| classfile.classfile_path != "example/Pair.class");
    assert!(matches!(
        validate_pair_artifacts(18798, missing_classfile),
        Err(JvmTastyContractError::BatchArtifactPaths {
            reason: "missing expected class-file path",
            ..
        })
    ));

    let mut invalid_path = valid.clone();
    invalid_path.classfiles[1].classfile_path = "../Pair.class".into();
    assert!(matches!(
        validate_pair_artifacts(18798, invalid_path),
        Err(JvmTastyContractError::BatchArtifactPaths {
            reason: "missing expected class-file path",
            ..
        })
    ));

    let mut missing_sidecar = valid.clone();
    missing_sidecar
        .sidecars
        .retain(|sidecar| sidecar.tasty_path != "example/Pair.tasty");
    assert!(matches!(
        validate_pair_artifacts(18798, missing_sidecar),
        Err(JvmTastyContractError::BatchArtifactPaths {
            reason: "missing expected TASTy sidecar path",
            ..
        })
    ));

    let mut corrupt_class = valid.clone();
    corrupt_class.classfiles[1].classfile_bytes.clear();
    assert!(matches!(
        validate_pair_artifacts(18799, corrupt_class),
        Err(JvmTastyContractError::BatchArtifact {
            class_index: 1,
            phase: "class-file decode",
            ..
        })
    ));

    let mut corrupt_sidecar = valid.clone();
    corrupt_sidecar.sidecars[1].tasty_bytes.clear();
    assert!(matches!(
        validate_pair_artifacts(18800, corrupt_sidecar),
        Err(JvmTastyContractError::BatchArtifact {
            class_index: 1,
            phase: "class-file/TASTy link",
            ..
        })
    ));

    let mut stale_uuid = valid;
    let pair = &mut stale_uuid.classfiles[1].classfile_bytes;
    let link = batch.classes()[1].tasty_link_id();
    let offset = pair
        .windows(link.as_bytes().len())
        .position(|window| window == link.as_bytes())
        .expect("Pair class carries its TASTy UUID");
    pair[offset] ^= 0x80;
    assert!(matches!(
        validate_pair_artifacts(18801, stale_uuid),
        Err(JvmTastyContractError::BatchArtifact {
            class_index: 1,
            phase: "class-file/TASTy link",
            ..
        })
    ));
}

#[test]
fn dual_reference_batch_rejects_unsupported_alias_counts_and_swaps_before_artifacts() {
    let swapped = PAIR_SOURCE
        .replace(
            "private val left: Counter = first",
            "private val left: Counter = second",
        )
        .replace(
            "private val right: Counter = second",
            "private val right: Counter = first",
        );
    let one_field = PAIR_SOURCE.replace("  private val right: Counter = second\n", "");
    let third_field = PAIR_SOURCE.replace(
        "  def leftValue(): Int = left.get()",
        "  private val extra: Counter = first\n  def leftValue(): Int = left.get()",
    );
    for (label, source, expected_phase, expected_reason) in [
        (
            "swapped aliases",
            swapped.as_str(),
            "TASTy plan validation",
            "UnsafeFieldInitializer",
        ),
        (
            "missing alias",
            one_field.as_str(),
            "method body typing",
            "TermNameNotFound",
        ),
        (
            "extra alias",
            third_field.as_str(),
            "TASTy plan validation",
            "ClassMembers",
        ),
    ] {
        let error =
            emit_paired_jvm_tasty_batch(source, "PairNegative.scala", SourceId::from_index(18802))
                .expect_err(
                    "unsupported or reordered aliases are rejected before batch artifacts escape",
                );
        let rendered = format!("{error:?}");
        eprintln!("{label} first rejection: {rendered}");
        let PairedBatchArtifactError::Plan(PairedPlanError::BatchClass { phase, reason, .. }) =
            error
        else {
            panic!("{label} must fail at a class plan stage: {rendered}");
        };
        assert_eq!(phase, expected_phase, "{label}: {reason}");
        assert!(reason.contains(expected_reason), "{label}: {reason}");
    }
}

#[test]
fn dual_reference_four_artifact_publication_cleans_every_staged_write_failure() {
    let batch = emit_pair(18803);
    let stage = TestDirectory::new("dual-reference-four-artifact-publication");
    let destination = stage.path().join("classes");
    let paths = batch_publisher::publish_paired_batch(&destination, &batch, None).unwrap();
    assert_eq!(paths.len(), 4);
    assert_eq!(
        relative_files(&destination),
        [
            "example/Counter.class",
            "example/Counter.tasty",
            "example/Pair.class",
            "example/Pair.tasty",
        ]
    );

    for (index, fault) in (1..=4)
        .map(batch_publisher::BatchPublishFault::AfterStagedFiles)
        .chain([
            batch_publisher::BatchPublishFault::BeforePromotion,
            batch_publisher::BatchPublishFault::AfterPromotion,
        ])
        .enumerate()
    {
        let failed = stage.path().join(format!("failure-{index}"));
        assert!(batch_publisher::publish_paired_batch(&failed, &batch, Some(fault)).is_err());
        assert!(
            !failed.exists(),
            "fault {index} leaves no accepted destination"
        );
        assert_eq!(std::fs::read_dir(stage.path()).unwrap().count(), 1);
    }

    let pairs = batch_publisher::batch_publish_pairs(&batch);
    let mut invalid_path = pairs.clone();
    invalid_path[1].classfile_path = "../Pair.class";
    let bad_path_destination = stage.path().join("invalid-path");
    assert!(matches!(
        batch_publisher::publish_batch_pairs(&bad_path_destination, &invalid_path, None),
        Err(batch_publisher::BatchPublishError::InvalidManifest {
            phase: "class-file path",
            ..
        })
    ));
    assert!(!bad_path_destination.exists());

    let mut forged_source = pairs.clone();
    forged_source[1].source_id = SourceId::from_index(18809);
    let forged_source_destination = stage.path().join("forged-source");
    assert!(matches!(
        batch_publisher::publish_batch_pairs(&forged_source_destination, &forged_source, None),
        Err(batch_publisher::BatchPublishError::InvalidManifest {
            phase: "source identity",
            ..
        })
    ));
    assert!(!forged_source_destination.exists());

    let sentinel_destination = stage.path().join("sentinel");
    std::fs::create_dir(&sentinel_destination).unwrap();
    let sentinel = sentinel_destination.join("sentinel.txt");
    std::fs::write(&sentinel, b"preserve existing destination").unwrap();
    assert!(batch_publisher::publish_paired_batch(&sentinel_destination, &batch, None).is_err());
    assert_eq!(
        std::fs::read(&sentinel).unwrap(),
        b"preserve existing destination"
    );
    assert_eq!(relative_files(&sentinel_destination), ["sentinel.txt"]);
}

#[test]
fn renamed_reordered_dual_reference_batch_is_deterministic_per_source_variant() {
    let class_start = RENAMED_REORDERED_SOURCE.find("class Envelope").unwrap();
    let token_start = RENAMED_REORDERED_SOURCE.find("\nclass Token").unwrap() + 1;
    let reordered = format!(
        "{}{}\n{}",
        &RENAMED_REORDERED_SOURCE[..class_start],
        &RENAMED_REORDERED_SOURCE[token_start..],
        &RENAMED_REORDERED_SOURCE[class_start..token_start]
    );
    for (source, source_id, repeat_id, expected) in [
        (
            RENAMED_REORDERED_SOURCE,
            18804,
            18805,
            ["sample/Envelope.class", "sample/Token.class"],
        ),
        (
            reordered.as_str(),
            18806,
            18807,
            ["sample/Token.class", "sample/Envelope.class"],
        ),
    ] {
        let batch =
            emit_paired_jvm_tasty_batch(source, "Envelope.scala", SourceId::from_index(source_id))
                .expect("renamed/reordered dual-reference class declarations are supported");
        let repeated =
            emit_paired_jvm_tasty_batch(source, "Envelope.scala", SourceId::from_index(repeat_id))
                .expect("a fresh session repeats the same source variant deterministically");
        assert_eq!(batch.classes().len(), 2);
        for (index, (left, right)) in batch.classes().iter().zip(repeated.classes()).enumerate() {
            assert_eq!(left.classfile_path(), expected[index]);
            assert_eq!(
                left.tasty_path(),
                expected[index].replace(".class", ".tasty")
            );
            assert_eq!(left.classfile_path(), right.classfile_path());
            assert_eq!(left.tasty_path(), right.tasty_path());
            assert_eq!(left.classfile_bytes(), right.classfile_bytes());
            assert_eq!(left.tasty_bytes(), right.tasty_bytes());
            assert_eq!(left.tasty_link_id(), right.tasty_link_id());
        }
    }
}

#[test]
fn pinned_scala39_reads_the_dual_reference_batch_from_rust_artifacts() {
    if std::env::var_os("DOTTY_REQUIRE_SCALA_TASTY_DUAL_REFERENCE_BATCH").is_none() {
        eprintln!("skipping strict Scala dual-reference batch check");
        return;
    }
    let scala_root = PathBuf::from(std::env::var_os("SCALA39_ROOT").expect("SCALA39_ROOT"));
    let java_home = PathBuf::from(std::env::var_os("JAVA_HOME").expect("JAVA_HOME"));
    let scalac = scala_root.join("dist/target/universal/stage/bin/scalac");
    let revision = Command::new("git")
        .arg("-C")
        .arg(&scala_root)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("read pinned Scala revision");
    assert!(revision.status.success());
    assert_eq!(
        String::from_utf8_lossy(&revision.stdout).trim(),
        "777528f19a58e794c9954a42f433373472ec57f8"
    );
    let version = Command::new(&scalac)
        .arg("-version")
        .output()
        .expect("run pinned Scala 3.9.0 compiler");
    assert!(version.status.success());
    let version_text = String::from_utf8_lossy(&version.stderr).to_string()
        + &String::from_utf8_lossy(&version.stdout);
    assert!(
        version_text.contains("3.9.0-bin-SNAPSHOT-git-777528f"),
        "{version_text}"
    );
    let java_version = Command::new(java_home.join("bin/java"))
        .arg("-version")
        .output()
        .expect("run SDKMAN Temurin JDK");
    assert!(java_version.status.success());
    let java_text = String::from_utf8_lossy(&java_version.stderr).to_string()
        + &String::from_utf8_lossy(&java_version.stdout);
    assert!(java_text.contains("21.0.13"), "{java_text}");

    let batch = emit_pair(18808);
    let temporary = std::env::temp_dir().join(format!(
        "dotty-issue-1879-scala-batch-{}",
        std::process::id()
    ));
    let candidate = temporary.join("candidate");
    let rebuilt = temporary.join("rebuilt");
    let _ = std::fs::remove_dir_all(&temporary);
    std::fs::create_dir_all(&candidate).unwrap();
    std::fs::create_dir_all(&rebuilt).unwrap();
    for artifact in batch.classes() {
        for (relative, bytes) in [
            (artifact.classfile_path(), artifact.classfile_bytes()),
            (artifact.tasty_path(), artifact.tasty_bytes()),
        ] {
            let path = candidate.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
    }

    let pair_tasty = candidate.join("example/Pair.tasty");
    let decompile = Command::new(&scalac)
        .arg("-decompile")
        .arg(&pair_tasty)
        .output()
        .expect("Scala -decompile Rust Pair.tasty");
    assert!(
        decompile.status.success(),
        "{}",
        String::from_utf8_lossy(&decompile.stderr)
    );
    let decompiled = String::from_utf8_lossy(&decompile.stdout);
    for name in [
        "first",
        "second",
        "left",
        "right",
        "leftValue",
        "rightValue",
        "get",
    ] {
        assert!(
            decompiled.contains(name),
            "missing {name} in Scala output:\n{decompiled}"
        );
    }

    let from_tasty = Command::new(&scalac)
        .args(["-from-tasty", "-classpath"])
        .arg(&candidate)
        .arg("-d")
        .arg(&rebuilt)
        .arg(candidate.join("example/Counter.tasty"))
        .arg(&pair_tasty)
        .output()
        .expect("Scala -from-tasty Rust Counter and Pair sidecars");
    assert!(
        from_tasty.status.success(),
        "{}",
        String::from_utf8_lossy(&from_tasty.stderr)
    );
    let javap = Command::new(java_home.join("bin/javap"))
        .args(["-p", "-s", "-classpath"])
        .arg(&rebuilt)
        .arg("example.Pair")
        .output()
        .expect("inspect Scala-rebuilt Pair ABI");
    assert!(
        javap.status.success(),
        "{}",
        String::from_utf8_lossy(&javap.stderr)
    );
    let javap_text = String::from_utf8_lossy(&javap.stdout);
    assert!(
        javap_text.contains("(Lexample/Counter;Lexample/Counter;)V"),
        "{javap_text}"
    );
    assert!(
        javap_text.contains("private final example.Counter left"),
        "{javap_text}"
    );
    assert!(
        javap_text.contains("private final example.Counter right"),
        "{javap_text}"
    );
    std::fs::remove_dir_all(temporary).unwrap();
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "dotty-jvm-tasty-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn relative_files(root: &Path) -> Vec<String> {
    fn visit(root: &Path, directory: &Path, files: &mut Vec<String>) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                visit(root, &path, files);
            } else {
                files.push(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    let mut files = Vec::new();
    visit(root, root, &mut files);
    files.sort();
    files
}
