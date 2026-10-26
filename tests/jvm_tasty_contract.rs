use dotty::jvm_tasty_plan::with_paired_jvm_tasty_batch_plan;
use dotty_classfile::access_flags::{ACC_FINAL, ACC_PRIVATE};
use dotty_classfile::attribute::Attribute;
use dotty_classfile::classfile::{
    ClassFile, ClassFileBuilder, ClassFileVersion, Reader, TastyLinkId,
};
use dotty_classfile::constant_pool::{ConstantPoolEntry, ConstantPoolIndex};
use dotty_core::ast::TreeKind;
use dotty_core::{Definitions, Packages, SemanticStore, SourceId, SourceText, SymbolKind};
use dotty_jvm::emit_class_with_tasty_link;
use dotty_jvm::{
    JvmClassInitializationPlan, JvmClassPlan, JvmCompilationUnit, JvmFieldPlan,
    JvmFieldSourceVisibility, JvmMethodPlan, JvmPrimaryConstructorPlan,
};
use dotty_jvm_tasty_contract::{
    JvmTastyBatchArtifacts, JvmTastyClassFileArtifact, JvmTastyContractError,
    JvmTastySidecarArtifact, validate_classfile_tasty_link, validate_jvm_tasty_batch,
    validate_jvm_tasty_batch_artifacts, validate_jvm_tasty_class,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_tasty::tasty::TastyFile;
use dotty_tasty_pickler::{
    ClassFieldPicklePlan, ClassMemberPicklePlan, ClassStatePicklePlan, PrimaryConstructorInputPlan,
    SourceMetadata,
};
use dotty_typer::SourceTyper;

const COUNTER: &str = "package example\n\nclass Counter(start: Int):\n  private var current: Int = start\n\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n";
const COUNTER_GAUGE: &str = include_str!("fixtures/jvm_tasty_multiclass/CounterGauge.scala");
const COUNTER_READER: &str = include_str!("fixtures/jvm_tasty_cross_class/Reader.scala");
const COUNTER_FACTORY: &str =
    include_str!("fixtures/jvm_tasty_cross_class_factories/CounterFactory.scala");
const THREE_CLASSES: &str = "package example\n\nclass Counter(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n\nclass Gauge(start: Int):\n  private var reading: Int = start\n  def get(): Int = reading\n  def set(value: Int): Unit = reading = value\n\nclass Thermometer(start: Int):\n  private var temperature: Int = start\n  def get(): Int = temperature\n  def set(value: Int): Unit = temperature = value\n";

#[test]
fn whole_batch_proves_distinct_class_abi_and_rejects_partial_or_swapped_artifacts() {
    with_paired_jvm_tasty_batch_plan(
        COUNTER_GAUGE,
        "CounterGauge.scala",
        SourceId::from_index(70),
        |source_batch| {
            let tasty_plans = source_batch
                .classes
                .iter()
                .map(|class| class.tasty_plan)
                .collect::<Vec<_>>();
            let batch = validate_jvm_tasty_batch(source_batch.jvm_unit, &tasty_plans).unwrap();
            assert_eq!(batch.source_id(), source_batch.source_id);
            assert_eq!(
                batch
                    .classes()
                    .iter()
                    .map(|class| class.proof().internal_name())
                    .collect::<Vec<_>>(),
                ["example/Counter", "example/Gauge"]
            );
            assert_eq!(
                batch
                    .classes()
                    .iter()
                    .map(|class| class.classfile_path())
                    .collect::<Vec<_>>(),
                ["example/Counter.class", "example/Gauge.class"]
            );

            let artifacts = encode_batch_for_contract_test(&source_batch);
            let mut reversed = artifacts.clone();
            reversed.classfiles.reverse();
            reversed.sidecars.reverse();
            validate_jvm_tasty_batch_artifacts(&batch, reversed.clone()).unwrap();

            let mut missing = reversed.clone();
            missing.classfiles.pop();
            assert!(matches!(
                validate_jvm_tasty_batch_artifacts(&batch, missing),
                Err(JvmTastyContractError::BatchArtifactPaths {
                    reason: "missing expected class-file path",
                    ..
                })
            ));

            let mut missing_sidecar = reversed.clone();
            missing_sidecar.sidecars.pop();
            assert!(matches!(
                validate_jvm_tasty_batch_artifacts(&batch, missing_sidecar),
                Err(JvmTastyContractError::BatchArtifactPaths {
                    reason: "missing expected TASTy sidecar path",
                    ..
                })
            ));

            let mut duplicate = reversed.clone();
            duplicate.classfiles.push(duplicate.classfiles[0].clone());
            assert!(matches!(
                validate_jvm_tasty_batch_artifacts(&batch, duplicate),
                Err(JvmTastyContractError::BatchArtifactPaths {
                    reason: "duplicate artifact path",
                    ..
                })
            ));

            let mut swapped = artifacts;
            let tasty0 = swapped.sidecars[0].tasty_bytes.clone();
            swapped.sidecars[0].tasty_bytes = swapped.sidecars[1].tasty_bytes.clone();
            swapped.sidecars[1].tasty_bytes = tasty0;
            assert!(matches!(
                validate_jvm_tasty_batch_artifacts(&batch, swapped),
                Err(JvmTastyContractError::BatchArtifact {
                    phase: "class-file/TASTy link",
                    ..
                })
            ));

            let mut wrong_package_path = reversed.clone();
            wrong_package_path.classfiles[0].classfile_path = "wrong/Gauge.class".into();
            assert!(matches!(
                validate_jvm_tasty_batch_artifacts(&batch, wrong_package_path),
                Err(JvmTastyContractError::BatchArtifactPaths {
                    reason: "missing expected class-file path",
                    ..
                })
            ));

            let mut truncated_sidecar = reversed.clone();
            truncated_sidecar.sidecars[0].tasty_bytes.truncate(5);
            assert!(validate_jvm_tasty_batch_artifacts(&batch, truncated_sidecar).is_err());

            let mut bad_second_class = reversed.clone();
            let second = bad_second_class
                .classfiles
                .iter_mut()
                .find(|artifact| artifact.classfile_path == "example/Gauge.class")
                .unwrap();
            second.classfile_bytes.truncate(7);
            assert!(matches!(
                validate_jvm_tasty_batch_artifacts(&batch, bad_second_class),
                Err(JvmTastyContractError::BatchArtifact {
                    class_index: 1,
                    phase: "class-file decode",
                    ..
                })
            ));

            let mut wrong_version = reversed.clone();
            let counter = wrong_version
                .classfiles
                .iter_mut()
                .find(|artifact| artifact.classfile_path == "example/Counter.class")
                .unwrap();
            counter.classfile_bytes[6..8].copy_from_slice(&61u16.to_be_bytes());
            assert!(matches!(
                validate_jvm_tasty_batch_artifacts(&batch, wrong_version),
                Err(JvmTastyContractError::BatchArtifact {
                    phase: "class-file version",
                    ..
                })
            ));

            let mut wrong_field_flags = reversed.clone();
            let counter = wrong_field_flags
                .classfiles
                .iter_mut()
                .find(|artifact| artifact.classfile_path == "example/Counter.class")
                .unwrap();
            let mut reader = Reader::new(&counter.classfile_bytes);
            let mut classfile = ClassFile::decode(&mut reader).unwrap();
            classfile.fields[0].access_flags.0 |= ACC_FINAL;
            counter.classfile_bytes = classfile.encode().unwrap();
            assert!(matches!(
                validate_jvm_tasty_batch_artifacts(&batch, wrong_field_flags),
                Err(JvmTastyContractError::BatchArtifact {
                    phase: "member ABI",
                    ..
                })
            ));

            let mut unexpected_interface = reversed.clone();
            let gauge = unexpected_interface
                .classfiles
                .iter_mut()
                .find(|artifact| artifact.classfile_path == "example/Gauge.class")
                .unwrap();
            let mut reader = Reader::new(&gauge.classfile_bytes);
            let mut classfile = ClassFile::decode(&mut reader).unwrap();
            classfile.interfaces.push(classfile.this_class);
            gauge.classfile_bytes = classfile.encode().unwrap();
            assert!(matches!(
                validate_jvm_tasty_batch_artifacts(&batch, unexpected_interface),
                Err(JvmTastyContractError::BatchArtifact {
                    phase: "class ABI",
                    ..
                })
            ));

            let mut extra = reversed.clone();
            let mut unexpected = extra.classfiles[0].clone();
            unexpected.classfile_path = "example/Unexpected.class".into();
            extra.classfiles.push(unexpected);
            assert!(matches!(
                validate_jvm_tasty_batch_artifacts(&batch, extra),
                Err(JvmTastyContractError::BatchArtifactPaths {
                    reason: "unexpected extra class file",
                    ..
                })
            ));

            let mut orphan = reversed.clone();
            let mut extra_sidecar = orphan.sidecars[0].clone();
            extra_sidecar.tasty_path = "example/Unexpected.tasty".into();
            orphan.sidecars.push(extra_sidecar);
            assert!(matches!(
                validate_jvm_tasty_batch_artifacts(&batch, orphan),
                Err(JvmTastyContractError::BatchArtifactPaths {
                    reason: "unexpected extra TASTy sidecar",
                    ..
                })
            ));
        },
    )
    .unwrap();
}

#[test]
fn cross_class_call_artifact_rejects_a_different_methodref() {
    with_paired_jvm_tasty_batch_plan(
        COUNTER_READER,
        "tests/fixtures/jvm_tasty_cross_class/Reader.scala",
        SourceId::from_index(1675),
        |source_batch| {
            let plans = source_batch
                .classes
                .iter()
                .map(|class| class.tasty_plan)
                .collect::<Vec<_>>();
            let validated = validate_jvm_tasty_batch(source_batch.jvm_unit, &plans).unwrap();
            let mut artifacts = encode_batch_for_contract_test(&source_batch);
            let reader_artifact = artifacts
                .classfiles
                .iter_mut()
                .find(|artifact| artifact.classfile_path == "example/Reader.class")
                .unwrap();
            let mut reader = Reader::new(&reader_artifact.classfile_bytes);
            let class = ClassFile::decode(&mut reader).unwrap();
            assert!(reader.is_at_end());

            let read = class
                .methods
                .iter()
                .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == "read")
                .unwrap();
            let [Attribute::Code(read_code)] = read.attributes.as_slice() else {
                panic!("Reader.read has one Code attribute");
            };
            let actual_index = u16::from_be_bytes([read_code.code[2], read_code.code[3]]);

            let constructor = class
                .methods
                .iter()
                .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == "<init>")
                .unwrap();
            let [Attribute::Code(constructor_code)] = constructor.attributes.as_slice() else {
                panic!("Reader constructor has one Code attribute");
            };
            assert_eq!(constructor_code.code[1], 0xb7);
            let wrong_index =
                u16::from_be_bytes([constructor_code.code[2], constructor_code.code[3]]);
            assert_ne!(actual_index, wrong_index);
            assert!(matches!(
                class
                    .constant_pool
                    .get(ConstantPoolIndex(wrong_index))
                    .unwrap(),
                ConstantPoolEntry::Methodref { .. }
            ));

            let needle = [
                0x2b,
                0xb6,
                actual_index.to_be_bytes()[0],
                actual_index.to_be_bytes()[1],
                0xac,
            ];
            let positions = reader_artifact
                .classfile_bytes
                .windows(needle.len())
                .enumerate()
                .filter_map(|(index, bytes)| (bytes == needle).then_some(index))
                .collect::<Vec<_>>();
            let [offset] = positions.as_slice() else {
                panic!("Reader.read's exact invokevirtual byte sequence occurs once");
            };
            reader_artifact.classfile_bytes[offset + 2..offset + 4]
                .copy_from_slice(&wrong_index.to_be_bytes());

            assert!(matches!(
                validate_jvm_tasty_batch_artifacts(&validated, artifacts),
                Err(JvmTastyContractError::BatchArtifact {
                    class_index: 1,
                    phase: "cross-class invocation",
                    reason,
                    ..
                }) if reason.contains("expected example/Counter.get()I")
            ));

            let mut wrong_descriptor = encode_batch_for_contract_test(&source_batch);
            let reader_artifact = wrong_descriptor
                .classfiles
                .iter_mut()
                .find(|artifact| artifact.classfile_path == "example/Reader.class")
                .unwrap();
            let mut reader = Reader::new(&reader_artifact.classfile_bytes);
            let mut class = ClassFile::decode(&mut reader).unwrap();
            let constructor_descriptor = class
                .methods
                .iter()
                .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == "<init>")
                .unwrap()
                .descriptor_index;
            let read_index = class
                .methods
                .iter()
                .position(|method| class.constant_pool.utf8(method.name_index).unwrap() == "read")
                .unwrap();
            class.methods[read_index].descriptor_index = constructor_descriptor;
            reader_artifact.classfile_bytes = class.encode().unwrap();
            assert!(matches!(
                validate_jvm_tasty_batch_artifacts(&validated, wrong_descriptor),
                Err(JvmTastyContractError::BatchArtifact {
                    class_index: 1,
                    phase: "member ABI",
                    ..
                })
            ));

            let mut missing_reader = encode_batch_for_contract_test(&source_batch);
            missing_reader
                .classfiles
                .retain(|artifact| artifact.classfile_path != "example/Reader.class");
            missing_reader
                .sidecars
                .retain(|artifact| artifact.tasty_path != "example/Reader.tasty");
            assert!(matches!(
                validate_jvm_tasty_batch_artifacts(&validated, missing_reader),
                Err(JvmTastyContractError::BatchArtifactPaths {
                    reason: "missing expected class-file path",
                    path,
                }) if path == "example/Reader.class"
            ));
        },
    )
    .unwrap();
}

#[test]
fn factory_batch_contract_rejects_wrong_result_abi_target_and_partial_pairs() {
    with_paired_jvm_tasty_batch_plan(
        COUNTER_FACTORY,
        "tests/fixtures/jvm_tasty_cross_class_factories/CounterFactory.scala",
        SourceId::from_index(1694),
        |source_batch| {
            let plans = source_batch
                .classes
                .iter()
                .map(|class| class.tasty_plan)
                .collect::<Vec<_>>();
            let validated = validate_jvm_tasty_batch(source_batch.jvm_unit, &plans).unwrap();
            assert!(source_batch.factory_proof.is_some());
            let valid = encode_batch_for_contract_test(&source_batch);
            validate_jvm_tasty_batch_artifacts(&validated, valid.clone()).unwrap();

            let mut wrong_result = valid.clone();
            let factory = wrong_result
                .classfiles
                .iter_mut()
                .find(|artifact| artifact.classfile_path == "example/CounterFactory.class")
                .unwrap();
            let mut reader = Reader::new(&factory.classfile_bytes);
            let mut class = ClassFile::decode(&mut reader).unwrap();
            let constructor_descriptor = class
                .methods
                .iter()
                .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == "<init>")
                .unwrap()
                .descriptor_index;
            let create_index = class
                .methods
                .iter()
                .position(|method| class.constant_pool.utf8(method.name_index).unwrap() == "create")
                .unwrap();
            class.methods[create_index].descriptor_index = constructor_descriptor;
            factory.classfile_bytes = class.encode().unwrap();
            assert!(matches!(
                validate_jvm_tasty_batch_artifacts(&validated, wrong_result),
                Err(JvmTastyContractError::BatchArtifact {
                    class_index: 1,
                    phase: "member ABI",
                    ..
                })
            ));

            let mut wrong_allocation_owner = valid.clone();
            let factory = wrong_allocation_owner
                .classfiles
                .iter_mut()
                .find(|artifact| artifact.classfile_path == "example/CounterFactory.class")
                .unwrap();
            let (code_bytes, wrong_owner) = {
                let mut reader = Reader::new(&factory.classfile_bytes);
                let class = ClassFile::decode(&mut reader).unwrap();
                let create = class
                    .methods
                    .iter()
                    .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == "create")
                    .unwrap();
                let [Attribute::Code(code)] = create.attributes.as_slice() else {
                    panic!("factory create has one Code attribute");
                };
                (code.code.to_vec(), class.this_class.0)
            };
            let allocation = factory
                .classfile_bytes
                .windows(code_bytes.len())
                .position(|window| window == code_bytes)
                .expect("the factory create instructions appear in the class bytes");
            factory.classfile_bytes[allocation + 1..allocation + 3]
                .copy_from_slice(&wrong_owner.to_be_bytes());
            assert!(matches!(
                validate_jvm_tasty_batch_artifacts(&validated, wrong_allocation_owner),
                Err(JvmTastyContractError::BatchArtifact {
                    class_index: 1,
                    phase: "cross-class invocation",
                    ..
                })
            ));

            let mut missing_counter = valid.clone();
            missing_counter
                .classfiles
                .retain(|artifact| artifact.classfile_path != "example/Counter.class");
            missing_counter
                .sidecars
                .retain(|artifact| artifact.tasty_path != "example/Counter.tasty");
            assert!(matches!(
                validate_jvm_tasty_batch_artifacts(&validated, missing_counter),
                Err(JvmTastyContractError::BatchArtifactPaths {
                    reason: "missing expected class-file path",
                    path,
                }) if path == "example/Counter.class"
            ));
        },
    )
    .unwrap();
}

#[test]
fn whole_source_sequence_rejects_joint_truncation_and_joint_reordering() {
    with_paired_jvm_tasty_batch_plan(
        THREE_CLASSES,
        "CounterGauge.scala",
        SourceId::from_index(72),
        |source_batch| {
            let plans = source_batch
                .classes
                .iter()
                .map(|class| class.tasty_plan)
                .collect::<Vec<_>>();
            assert_eq!(plans.len(), 3);

            let truncated_unit = JvmCompilationUnit {
                store: source_batch.jvm_unit.store,
                typed_ast: source_batch.jvm_unit.typed_ast,
                classes: source_batch.jvm_unit.classes[..2].to_vec(),
            };
            assert!(matches!(
                validate_jvm_tasty_batch(&truncated_unit, &plans[..2]),
                Err(JvmTastyContractError::BatchShape(
                    "JVM and TASTy class lists must exactly match source package declarations in order"
                ))
            ));

            let mut reversed_classes = source_batch.jvm_unit.classes.clone();
            reversed_classes.reverse();
            let reversed_unit = JvmCompilationUnit {
                store: source_batch.jvm_unit.store,
                typed_ast: source_batch.jvm_unit.typed_ast,
                classes: reversed_classes,
            };
            let reversed_plans = [plans[2], plans[1], plans[0]];
            assert!(matches!(
                validate_jvm_tasty_batch(&reversed_unit, &reversed_plans),
                Err(JvmTastyContractError::BatchShape(
                    "JVM and TASTy class lists must exactly match source package declarations in order"
                ))
            ));
        },
    )
    .unwrap();
}

#[test]
fn batch_rejects_a_foreign_jvm_store_without_panicking() {
    with_paired_jvm_tasty_batch_plan(
        COUNTER_GAUGE,
        "CounterGauge.scala",
        SourceId::from_index(73),
        |source_batch| {
            let plans = source_batch
                .classes
                .iter()
                .map(|class| class.tasty_plan)
                .collect::<Vec<_>>();
            let foreign_store = SemanticStore::new();
            let foreign_unit = JvmCompilationUnit {
                store: &foreign_store,
                typed_ast: source_batch.jvm_unit.typed_ast,
                classes: source_batch.jvm_unit.classes.clone(),
            };
            assert!(matches!(
                validate_jvm_tasty_batch(&foreign_unit, &plans),
                Err(JvmTastyContractError::DifferentSession("semantic store"))
            ));
        },
    )
    .unwrap();
}

#[test]
fn whole_batch_rejects_wrong_tasty_order_and_duplicate_source_plan() {
    with_paired_jvm_tasty_batch_plan(
        COUNTER_GAUGE,
        "CounterGauge.scala",
        SourceId::from_index(71),
        |source_batch| {
            let plans = source_batch
                .classes
                .iter()
                .map(|class| class.tasty_plan)
                .collect::<Vec<_>>();
            let swapped = [plans[1], plans[0]];
            assert!(matches!(
                validate_jvm_tasty_batch(source_batch.jvm_unit, &swapped),
                Err(JvmTastyContractError::BatchShape(
                    "JVM and TASTy class lists must exactly match source package declarations in order"
                ))
            ));
            let duplicate = [plans[0], plans[0]];
            assert!(matches!(
                validate_jvm_tasty_batch(source_batch.jvm_unit, &duplicate),
                Err(JvmTastyContractError::BatchShape(
                    "JVM and TASTy class lists must exactly match source package declarations in order"
                ))
            ));
        },
    )
    .unwrap();
}

fn encode_batch_for_contract_test(
    source_batch: &dotty::jvm_tasty_plan::PairedJvmTastyBatchPlan<'_>,
) -> JvmTastyBatchArtifacts {
    let entries = source_batch
        .classes
        .iter()
        .map(|class| {
            let pickled = class.tasty_plan.pickle().unwrap();
            let tasty_file = TastyFile::parse_and_validate_scala_3_9(pickled.bytes()).unwrap();
            let link = TastyLinkId::from_bytes(tasty_file.header().uuid);
            let generated = emit_class_with_tasty_link(
                source_batch.jvm_unit,
                class.class,
                class.tasty_plan.plan().definitions,
                link,
            )
            .unwrap();
            (
                JvmTastyClassFileArtifact {
                    classfile_path: class.classfile_path.clone(),
                    classfile_bytes: generated.into_bytes(),
                },
                JvmTastySidecarArtifact {
                    tasty_path: class.tasty_path.clone(),
                    tasty_bytes: pickled.bytes().to_vec(),
                },
            )
        })
        .collect::<Vec<_>>();
    let (classfiles, sidecars) = entries.into_iter().unzip();
    JvmTastyBatchArtifacts {
        classfiles,
        sidecars,
    }
}

#[test]
fn one_session_counter_proves_source_tasty_and_jvm_abi_agreement() {
    with_counter_pair(COUNTER, SourceId::from_index(40), |unit, tasty| {
        let proof = validate_jvm_tasty_class(unit, tasty).unwrap();
        assert_eq!(proof.internal_name(), "example/Counter");
        assert_eq!(proof.tasty_path(), "example/Counter.tasty");
        let metadata = proof.jvm_metadata();
        assert_eq!(
            metadata.constructor().unwrap().descriptor().as_str(),
            "(I)V"
        );
        assert_eq!(metadata.fields().len(), 1);
        assert_eq!(metadata.fields()[0].descriptor(), "I");
        assert!(metadata.fields()[0].access_flags().contains(ACC_PRIVATE));
        assert!(!metadata.fields()[0].access_flags().contains(ACC_FINAL));
        assert_eq!(
            metadata
                .methods()
                .iter()
                .map(|method| (method.name(), method.descriptor().as_str()))
                .collect::<Vec<_>>(),
            [("get", "()I"), ("set", "(I)V")]
        );
    });
}

#[test]
fn unsupported_tasty_method_body_is_rejected_before_pair_proof() {
    let counter_with_loop = "package example\n\nclass Counter(start: Int):\n  private var current: Int = start\n\n  def get(): Int = current\n  def set(value: Int): Unit = while current < value do current = current + 1\n";
    with_counter_pair(
        counter_with_loop,
        SourceId::from_index(46),
        |unit, tasty| {
            assert!(tasty.pickle().is_err());
            assert!(matches!(
                validate_jvm_tasty_class(unit, tasty),
                Err(JvmTastyContractError::TastyPlan(_))
            ));
        },
    );
}

#[test]
fn independent_numeric_arena_ids_do_not_prove_shared_session_provenance() {
    with_counter_pair(COUNTER, SourceId::from_index(41), |first_unit, _| {
        with_counter_pair(COUNTER, SourceId::from_index(41), |_, second_tasty| {
            let result = validate_jvm_tasty_class(first_unit, second_tasty);
            assert!(matches!(
                result,
                Err(JvmTastyContractError::DifferentSession("semantic store"))
            ));
        });
    });
}

#[test]
fn mismatched_typed_initializer_root_is_rejected_before_a_pair_proof() {
    with_counter_pair(COUNTER, SourceId::from_index(42), |unit, tasty| {
        let class_plan = &unit.classes[0];
        let field = class_plan.fields[0];
        let mut changed_classes = unit.classes.clone();
        changed_classes[0].fields[0] = JvmFieldPlan {
            initializer: class_plan.methods[0].body,
            ..field
        };
        let changed = JvmCompilationUnit {
            store: unit.store,
            typed_ast: unit.typed_ast,
            classes: changed_classes,
        };
        let result = validate_jvm_tasty_class(&changed, tasty);
        let error = result.err().expect("mismatched root must fail");
        assert!(
            matches!(error, JvmTastyContractError::JvmPlan(_))
                || matches!(error, JvmTastyContractError::TypedRootMismatch { .. })
        );
    });
}

#[test]
fn missing_jvm_method_is_rejected_by_canonical_identity() {
    with_counter_pair(COUNTER, SourceId::from_index(44), |unit, tasty| {
        let missing = unit.classes[0].methods[0].method;
        let mut classes = unit.classes.clone();
        classes[0].methods.remove(0);
        let changed = JvmCompilationUnit {
            store: unit.store,
            typed_ast: unit.typed_ast,
            classes,
        };
        let result = validate_jvm_tasty_class(&changed, tasty);
        assert!(matches!(
            result,
            Err(JvmTastyContractError::MemberMissing {
                member,
                side: "JVM method plan"
            }) if member == missing
        ));
    });
}

#[test]
fn immutable_source_field_is_not_mistaken_for_the_audited_mutable_backing_field() {
    let immutable_counter = "package example\nclass Counter(start: Int):\n  private val current: Int = start\n  def get(): Int = current\n";
    with_counter_pair(
        immutable_counter,
        SourceId::from_index(45),
        |unit, tasty| {
            let result = validate_jvm_tasty_class(unit, tasty);
            assert!(matches!(
                result,
                Err(JvmTastyContractError::FieldShapeMismatch { .. })
            ));
        },
    );
}

#[test]
fn same_simple_class_name_keeps_distinct_package_artifact_paths() {
    for (source, expected) in [
        (
            "package left\nclass Counter(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n",
            "left/Counter.tasty",
        ),
        (
            "package right\nclass Counter(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n",
            "right/Counter.tasty",
        ),
    ] {
        with_counter_pair(source, SourceId::from_index(43), |unit, tasty| {
            let proof = validate_jvm_tasty_class(unit, tasty).unwrap();
            assert_eq!(proof.tasty_path(), expected);
        });
    }
}

#[test]
fn stale_classfile_link_uuid_is_rejected_against_validated_tasty_header() {
    let fixture_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/jvm_tasty_interop/scala39/example");
    let class_bytes = std::fs::read(fixture_dir.join("Counter.class")).unwrap();
    let tasty_bytes = std::fs::read(fixture_dir.join("Counter.tasty")).unwrap();
    let mut reader = Reader::new(&class_bytes);
    let class = ClassFile::decode(&mut reader).unwrap();
    validate_classfile_tasty_link(&class, &tasty_bytes).unwrap();

    let mut builder = ClassFileBuilder::new(
        "example/Counter",
        ClassFileVersion {
            major: 61,
            minor: 0,
        },
    )
    .unwrap();
    builder
        .tasty_link(TastyLinkId::from_bytes([0; TastyLinkId::LENGTH]))
        .unwrap();
    let stale_bytes = builder.encode().unwrap();
    let mut stale_reader = Reader::new(&stale_bytes);
    let stale = ClassFile::decode(&mut stale_reader).unwrap();
    assert!(matches!(
        validate_classfile_tasty_link(&stale, &tasty_bytes),
        Err(JvmTastyContractError::StaleTastyLink { .. })
    ));
}

fn with_counter_pair<R>(
    source_text: &str,
    source: SourceId,
    check: impl FnOnce(
        &JvmCompilationUnit<'_>,
        &dotty_tasty_pickler::ValidatedClassStatePlan<'_, '_>,
    ) -> R,
) -> R {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(source_text).unwrap();
    assert!(scanner.diagnostics().is_empty());
    let parsed = parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source,
        "Counter.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        outcome => panic!("namer rejected Counter: {outcome:?}"),
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("expected package")
    };
    let [class_tree] = package.stats.as_slice() else {
        panic!("expected one class")
    };
    let class = index.symbol_at(source, *class_tree).unwrap();
    let TreeKind::TypeDef(class_definition) = &parsed.ast.get(*class_tree).kind else {
        panic!("expected class definition")
    };
    let TreeKind::Template(template) = &parsed.ast.get(class_definition.rhs).kind else {
        panic!("expected template")
    };
    let constructor_tree = template.constructor;
    let constructor = index.symbol_at(source, constructor_tree).unwrap();
    let TreeKind::DefDef(constructor_definition) = &parsed.ast.get(constructor_tree).kind else {
        panic!("expected constructor")
    };
    let constructor_parameters = constructor_definition
        .value_param_clauses
        .iter()
        .map(|clause| {
            clause
                .iter()
                .map(|tree| index.derived_symbol_at(constructor, source, *tree).unwrap())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer.complete_symbol(class).unwrap();
    typer.complete_symbol(constructor).unwrap();
    for parameter in constructor_parameters.iter().flatten().copied() {
        typer.complete_symbol(parameter).unwrap();
    }
    let mut tasty_members = Vec::new();
    let mut jvm_fields = Vec::new();
    let mut jvm_methods = Vec::new();
    for &tree in &template.body {
        match &parsed.ast.get(tree).kind {
            TreeKind::ValDef(definition) => {
                let symbol = index.symbol_at(source, tree).unwrap();
                assert_eq!(typer.store().symbols.get(symbol).kind, SymbolKind::Field);
                typer.complete_symbol(symbol).unwrap();
                let declared_type = match typer.store().symbols.get(symbol).info {
                    dotty_core::SymbolInfo::Complete(ty) => ty,
                    info => panic!("field type incomplete: {info:?}"),
                };
                let rhs = definition.rhs.expect("Counter field initializer");
                let context = typer.field_initializer_context_for(symbol).unwrap();
                let typed_initializer = typer
                    .type_expression_expected(rhs, context, declared_type)
                    .unwrap();
                tasty_members.push(ClassMemberPicklePlan::Field(ClassFieldPicklePlan {
                    symbol,
                    source_tree: tree,
                    typed_initializer,
                    declared_type,
                }));
                jvm_fields.push(JvmFieldPlan {
                    field: symbol,
                    source_visibility: JvmFieldSourceVisibility::PlainPrivate,
                    initializer: Some(typed_initializer),
                });
            }
            TreeKind::DefDef(definition) => {
                let symbol = index.symbol_at(source, tree).unwrap();
                if symbol == constructor {
                    continue;
                }
                typer.complete_symbol(symbol).unwrap();
                let params = definition
                    .value_param_clauses
                    .iter()
                    .flatten()
                    .map(|tree| index.symbol_at(source, *tree).unwrap())
                    .collect::<Vec<_>>();
                let rhs = definition.rhs.expect("method RHS");
                let context = typer.expression_context_for(symbol).unwrap();
                let body = typer.type_expression(rhs, context).unwrap();
                tasty_members.push(ClassMemberPicklePlan::Method {
                    symbol,
                    source_tree: tree,
                });
                jvm_methods.push(JvmMethodPlan {
                    method: symbol,
                    parameters: params,
                    locals: Vec::new(),
                    body: Some(body),
                });
            }
            _ => panic!("unsupported Counter member"),
        }
    }
    let jvm_constructor = JvmPrimaryConstructorPlan::from_source(
        class,
        constructor,
        constructor_parameters.iter().flatten().copied().collect(),
        &parsed.ast,
        &index,
    )
    .unwrap();
    let typed_bindings = |source, tree| typer.source_typed_index().get(source, tree);
    let tasty_input = ClassStatePicklePlan {
        store: typer.store(),
        definitions: &definitions,
        source_ast: &parsed.ast,
        typed_ast: typer.typed_ast(),
        source_index: &index,
        typed_bindings: &typed_bindings,
        source: SourceMetadata {
            id: source,
            name: "Counter.scala".into(),
        },
        class,
        class_tree: *class_tree,
        constructor: PrimaryConstructorInputPlan {
            symbol: constructor,
            source_tree: constructor_tree,
            parameter_clauses: constructor_parameters,
        },
        members: &tasty_members,
    };
    let validated_tasty = tasty_input.validate().unwrap();
    let unit = JvmCompilationUnit {
        store: typer.store(),
        typed_ast: typer.typed_ast(),
        classes: vec![JvmClassPlan {
            class,
            methods: jvm_methods,
            fields: jvm_fields,
            initialization: JvmClassInitializationPlan::PrimaryConstructor(jvm_constructor),
        }],
    };
    check(&unit, &validated_tasty)
}
