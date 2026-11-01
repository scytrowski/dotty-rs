use dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan;
use dotty_classfile::classfile::{ClassFile, Reader};
use dotty_classfile::tasty_link::TastyLinkId;
use dotty_core::SourceId;
use dotty_jvm::emit_class;
use dotty_jvm_tasty_contract::{
    JvmTastyBatchArtifacts, JvmTastyClassFileArtifact, JvmTastySidecarArtifact,
    validate_jvm_tasty_batch, validate_jvm_tasty_batch_artifacts,
};
use dotty_jvm_tasty_contract::{JvmTastyContractError, validate_classfile_tasty_link};
use dotty_tasty::ast::{APPLY_TAG, DEFDEF_TAG, PARAM_TAG, TERMREFDIRECT_TAG};
use dotty_tasty::tasty::{DefinitionBody, TastyFile};
use dotty_tasty::term::{AstRefKind, RawTree, TermValue};

use dotty::jvm_tasty_artifacts::{
    PairedArtifactError, PairedBatchArtifactError, emit_paired_jvm_tasty_batch,
    emit_paired_jvm_tasty_class,
};
use dotty::jvm_tasty_plan::PairedPlanError;
use dotty::jvm_tasty_plan::with_paired_jvm_tasty_batch_plan;
use dotty_classfile::attribute::Attribute;
use dotty_classfile::bytecode::CodeSequence;
use dotty_classfile::bytecode::{
    AnalysisError, CfgCodeAnalyzer, ClassReference, ConstantPoolIndexOperand, ControlFlowGraph,
    Instruction, LocalIndex, SpecialMethodReference, VirtualMethodReference,
    analyze as analyze_instructions,
};
use dotty_classfile::constant_pool::{ConstantPoolEntry, ConstantPoolIndex};
use std::process::Command;

const COUNTER: &str = include_str!("fixtures/jvm_tasty_interop/Counter.scala");
const COUNTER_GAUGE: &str = include_str!("fixtures/jvm_tasty_multiclass/CounterGauge.scala");
const COUNTER_READER: &str = include_str!("fixtures/jvm_tasty_cross_class/Reader.scala");
const COUNTER_FACTORY: &str =
    include_str!("fixtures/jvm_tasty_cross_class_factories/CounterFactory.scala");
const COUNTER_FACTORY_RUNTIME: &str =
    include_str!("fixtures/jvm_tasty_cross_class_factories/CounterFactoryRuntime.scala");
const COUNTER_OWNER: &str =
    include_str!("fixtures/jvm_tasty_object_composition/CounterOwner.scala");
const COUNTER_OWNER_ALIAS: &str =
    include_str!("fixtures/jvm_tasty_constructor_injection/CounterOwner.scala");
const REFERENCE_CONSTRUCTOR_BATCH: &str =
    include_str!("fixtures/jvm_tasty_reference_constructor_calls/CounterOwnerFactory.scala");
const NESTED_CONSTRUCTOR_BATCH: &str =
    include_str!("fixtures/jvm_tasty_nested_constructor_calls/NestedConstructorCalls.scala");
const MULTI_ARGUMENT_CONSTRUCTOR_BATCH: &str =
    include_str!("fixtures/jvm_tasty_multi_argument_constructor_calls/PairFactory.scala");
const DUAL_REFERENCE_FORWARDING_BATCH: &str =
    include_str!("fixtures/jvm_tasty_dual_reference_forwarding/PairFactory.scala");
const PAIR_FACTORY_IDENTITY_HARNESS_JAVA: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_multi_argument_constructor_calls/PairFactoryIdentityHarness.java"
);
const PAIR_FACTORY_IDENTITY_HARNESS_SHA256: &str =
    "e3f173a04d86ebc746af15e779fe0f3a4b2d9a382b291f488a5606fced5bea90";
const FACTORY_RESULT_BATCH: &str =
    include_str!("fixtures/jvm_tasty_factory_result_arguments/CounterOwnerFactory.scala");
const REFERENCE_LOCAL_BATCH: &str =
    include_str!("fixtures/jvm_tasty_reference_local_constructor_flows/CounterOwnerFactory.scala");
const COUNTER_FACTORY_IDENTITY_ONLY: &str = "package example\n\nclass Counter:\n  def value(): Int = 1\n\nclass CounterFactory:\n  def identity(counter: Counter): Counter = counter\n";
const COUNTER_FACTORY_PRIMITIVE_CREATE: &str = "package example\n\nclass Counter:\n  def value(): Int = 1\n\nclass CounterFactory:\n  def identity(counter: Counter): Counter = counter\n  def create(seed: Int): Int = seed\n";
const COUNTER_FACTORY_REFERENCE_CREATE: &str = "package example\n\nclass Counter:\n  def value(): Int = 1\n\nclass CounterFactory:\n  def create(counter: Counter): Counter = counter\n";

fn tasty_method_body<'a>(
    file: &'a TastyFile<'a>,
    name: &str,
) -> (u32, dotty_tasty::ast::DefDefBody<'a>) {
    let index = file.ast_address_index().unwrap();
    index
        .iter_nodes_with_tag(DEFDEF_TAG)
        .find_map(|node| {
            let body = index.get(node.offset as u32)?.decode_defdef_body().ok()?;
            (file.names().render(body.name).ok()? == name).then_some((node.offset as u32, body))
        })
        .unwrap_or_else(|| panic!("TASTy declares {name}"))
}

fn tasty_parameter_addresses(
    index: &dotty_tasty::ast::AstAddressIndex<'_>,
    method_address: u32,
) -> Vec<u32> {
    index
        .iter_nodes_with_tag(PARAM_TAG)
        .filter(|node| {
            let mut parent = index.parent_of(node.offset as u32);
            while let Some(item) = parent {
                if item.offset as u32 == method_address {
                    return true;
                }
                parent = index.parent_of(item.offset as u32);
            }
            false
        })
        .map(|node| node.offset as u32)
        .collect()
}

fn verify_dual_reference_batch_with_pinned_scala(
    batch: &dotty::jvm_tasty_artifacts::PairedJvmTastyBatchArtifacts,
) {
    const REVISION: &str = "777528f19a58e794c9954a42f433373472ec57f8";
    let scala_root = std::path::PathBuf::from(
        std::env::var_os("SCALA39_ROOT").expect("SCALA39_ROOT is required by the strict gate"),
    );
    let java_home = std::path::PathBuf::from(
        std::env::var_os("JAVA_HOME").expect("JAVA_HOME is required by the strict gate"),
    );
    let revision = Command::new("git")
        .args(["-C"])
        .arg(&scala_root)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("read pinned Scala revision");
    assert!(revision.status.success());
    assert_eq!(String::from_utf8_lossy(&revision.stdout).trim(), REVISION);
    let scalac = scala_root.join("dist/target/universal/stage/bin/scalac");
    let version = Command::new(&scalac)
        .arg("-version")
        .output()
        .expect("query pinned Scala compiler version");
    let version_text = format!(
        "{}{}",
        String::from_utf8_lossy(&version.stdout),
        String::from_utf8_lossy(&version.stderr)
    );
    assert!(version.status.success());
    assert!(
        version_text.contains("3.9.0-bin-SNAPSHOT-git-777528f"),
        "{version_text}"
    );
    let java = java_home.join("bin/java");
    let java_version = Command::new(&java)
        .arg("-version")
        .output()
        .expect("query configured JDK");
    assert!(java_version.status.success());
    let java_text = format!(
        "{}{}",
        String::from_utf8_lossy(&java_version.stdout),
        String::from_utf8_lossy(&java_version.stderr)
    );
    assert!(java_text.contains("21.0.13"), "{java_text}");

    let libraries = [
        scala_root.join("dist/target/universal/stage/maven2/org/scala-lang/scala3-library_3/3.9.0-bin-SNAPSHOT/scala3-library_3-3.9.0-bin-SNAPSHOT.jar"),
        scala_root.join("dist/target/universal/stage/maven2/org/scala-lang/scala-library/3.9.0-bin-SNAPSHOT/scala-library-3.9.0-bin-SNAPSHOT.jar"),
    ];
    for path in &libraries {
        assert!(
            path.is_file(),
            "Scala runtime jar exists: {}",
            path.display()
        );
    }
    let root = TestDirectory::new("dual-reference-forwarding-scala-reader");
    let producer = root.path().join("rust-producer");
    let reconstructed = root.path().join("scala-from-tasty");
    for artifact in batch.classes() {
        for (relative, bytes) in [
            (artifact.classfile_path(), artifact.classfile_bytes()),
            (artifact.tasty_path(), artifact.tasty_bytes()),
        ] {
            let path = producer.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
    }
    std::fs::create_dir_all(&reconstructed).unwrap();
    let sidecars = ["Counter", "Pair", "PairFactory"]
        .map(|name| producer.join(format!("example/{name}.tasty")));
    for (name, sidecar) in ["Counter", "Pair", "PairFactory"]
        .into_iter()
        .zip(&sidecars)
    {
        let decompiled = Command::new(&scalac)
            .args(["-color:never", "-decompile"])
            .arg(sidecar)
            .output()
            .expect("run pinned Scala decompiler");
        assert!(
            decompiled.status.success(),
            "Scala rejected Rust {name}.tasty: {}",
            String::from_utf8_lossy(&decompiled.stderr)
        );
        let decompiled = String::from_utf8_lossy(&decompiled.stdout);
        match name {
            "Counter" => assert!(
                decompiled.contains("def get") && decompiled.contains("def set"),
                "{decompiled}"
            ),
            "Pair" => assert!(
                decompiled.contains("class Pair(first: example.Counter, second: example.Counter)")
                    && decompiled.contains("def leftValue")
                    && decompiled.contains("def rightValue")
            ),
            "PairFactory" => assert!(
                decompiled.contains(
                    "def wrap(first: example.Counter, second: example.Counter): example.Pair"
                ) && decompiled.contains("new example.Pair(first, second)"),
                "{decompiled}"
            ),
            _ => unreachable!(),
        }
        let printed = Command::new(&scalac)
            .arg("-print-tasty")
            .arg(sidecar)
            .output()
            .expect("run pinned Scala TASTy printer");
        assert!(
            printed.status.success(),
            "Scala rejected {name}.tasty in print mode: {}",
            String::from_utf8_lossy(&printed.stderr)
        );
        assert!(String::from_utf8_lossy(&printed.stdout).contains("version: 28.9.0"));
    }
    let classpath = format!(
        "{}:{}:{}",
        producer.display(),
        libraries[0].display(),
        libraries[1].display()
    );
    let from_tasty = Command::new(&scalac)
        .args(["-release", "21", "-from-tasty", "-classpath"])
        .arg(classpath)
        .arg("-d")
        .arg(&reconstructed)
        .args(&sidecars)
        .output()
        .expect("run pinned Scala -from-tasty on the Rust batch");
    assert!(
        from_tasty.status.success(),
        "Scala -from-tasty rejected the Rust batch: {}",
        String::from_utf8_lossy(&from_tasty.stderr)
    );
    for name in ["Counter", "Pair", "PairFactory"] {
        assert!(
            reconstructed
                .join(format!("example/{name}.class"))
                .is_file()
        );
    }
}

#[test]
fn counter_emits_complete_class_and_tasty_with_the_same_real_header_uuid() {
    let pair =
        emit_paired_jvm_tasty_class(COUNTER, "Counter.scala", SourceId::from_index(310)).unwrap();

    assert_eq!(pair.classfile_path(), "example/Counter.class");
    assert_eq!(pair.tasty_path(), "example/Counter.tasty");
    assert_eq!(pair.source_member_order().len(), 3);

    let tasty = TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
    assert_eq!(
        (
            tasty.header().major_version,
            tasty.header().minor_version,
            tasty.header().experimental_version,
        ),
        (28, 9, 0)
    );
    assert_eq!(
        pair.tasty_link_id(),
        TastyLinkId::from_bytes(tasty.header().uuid)
    );

    let mut reader = Reader::new(pair.classfile_bytes());
    let class = ClassFile::decode(&mut reader).unwrap();
    assert!(reader.is_at_end());
    assert_eq!((class.version.major, class.version.minor), (65, 0));
    assert_eq!(
        class.constant_pool.class_name(class.this_class).unwrap(),
        "example/Counter"
    );
    assert_eq!(class.tasty_link_id().unwrap(), Some(pair.tasty_link_id()));
    validate_classfile_tasty_link(&class, pair.tasty_bytes()).unwrap();

    let fields = class
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
    assert_eq!(fields.len(), 1);
    assert_eq!((fields[0].0, fields[0].1), ("current", "I"));
    assert!(
        fields[0]
            .2
            .contains(dotty_classfile::access_flags::ACC_PRIVATE)
    );
    assert!(
        !fields[0]
            .2
            .contains(dotty_classfile::access_flags::ACC_FINAL)
    );

    let mut methods = class
        .methods
        .iter()
        .map(|method| {
            (
                class.constant_pool.utf8(method.name_index).unwrap(),
                class.constant_pool.utf8(method.descriptor_index).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    methods.sort();
    assert_eq!(
        methods,
        [("<init>", "(I)V"), ("get", "()I"), ("set", "(I)V")]
    );
}

#[test]
fn standalone_jvm_emission_remains_unlinked() {
    let class_bytes = with_paired_jvm_tasty_class_plan(
        COUNTER,
        "Counter.scala",
        SourceId::from_index(318),
        |plan| {
            emit_class(
                plan.jvm_unit,
                plan.class,
                plan.tasty_plan.plan().definitions,
            )
            .unwrap()
            .into_bytes()
        },
    )
    .unwrap();
    let mut reader = Reader::new(&class_bytes);
    let class = ClassFile::decode(&mut reader).unwrap();

    assert_eq!(class.tasty_link_id().unwrap(), None);
    assert!(reader.is_at_end());
}

#[test]
fn fresh_source_sessions_produce_byte_identical_linked_pairs() {
    let first =
        emit_paired_jvm_tasty_class(COUNTER, "Counter.scala", SourceId::from_index(311)).unwrap();
    let second =
        emit_paired_jvm_tasty_class(COUNTER, "Counter.scala", SourceId::from_index(312)).unwrap();

    assert_ne!(first.source_id(), second.source_id());
    assert_eq!(first.classfile_path(), second.classfile_path());
    assert_eq!(first.tasty_path(), second.tasty_path());
    assert_eq!(first.classfile_bytes(), second.classfile_bytes());
    assert_eq!(first.tasty_bytes(), second.tasty_bytes());
    assert_eq!(first.tasty_link_id(), second.tasty_link_id());
    assert_eq!(first.source_member_order(), second.source_member_order());
}

#[test]
fn batch_emits_the_complete_ordered_class_and_tasty_manifest() {
    let batch = emit_paired_jvm_tasty_batch(
        COUNTER_GAUGE,
        "CounterGauge.scala",
        SourceId::from_index(319),
    )
    .unwrap();

    assert_eq!(batch.source_id(), SourceId::from_index(319));
    assert_eq!(batch.classes().len(), 2);
    let expected = [
        (
            "example/Counter",
            "example/Counter.class",
            "example/Counter.tasty",
        ),
        (
            "example/Gauge",
            "example/Gauge.class",
            "example/Gauge.tasty",
        ),
    ];
    for (class, (internal_name, classfile_path, tasty_path)) in batch.classes().iter().zip(expected)
    {
        let pair = class;
        assert_eq!(pair.classfile_path(), classfile_path);
        assert_eq!(pair.tasty_path(), tasty_path);
        assert_eq!(pair.source_id(), batch.source_id());

        let tasty = TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
        assert_eq!(
            (
                tasty.header().major_version,
                tasty.header().minor_version,
                tasty.header().experimental_version,
            ),
            (28, 9, 0)
        );
        assert_eq!(
            pair.tasty_link_id(),
            TastyLinkId::from_bytes(tasty.header().uuid)
        );

        let mut reader = Reader::new(pair.classfile_bytes());
        let class_file = ClassFile::decode(&mut reader).unwrap();
        assert!(reader.is_at_end());
        assert_eq!(
            (class_file.version.major, class_file.version.minor),
            (65, 0)
        );
        assert_eq!(
            class_file
                .constant_pool
                .class_name(class_file.this_class)
                .unwrap(),
            internal_name
        );
        assert_eq!(
            class_file.tasty_link_id().unwrap(),
            Some(pair.tasty_link_id())
        );
        validate_classfile_tasty_link(&class_file, pair.tasty_bytes()).unwrap();
    }
}

#[test]
fn reference_local_flow_emits_a_deterministic_six_artifact_batch() {
    let first = emit_paired_jvm_tasty_batch(
        REFERENCE_LOCAL_BATCH,
        "CounterOwnerFactory.scala",
        SourceId::from_index(18530),
    )
    .expect("the original one-session reference-local source produces a paired batch");
    let second = emit_paired_jvm_tasty_batch(
        REFERENCE_LOCAL_BATCH,
        "CounterOwnerFactory.scala",
        SourceId::from_index(18531),
    )
    .expect("a fresh semantic session produces the same paired batch");

    assert_eq!(first.source_id(), SourceId::from_index(18530));
    assert_eq!(first.classes().len(), 3);
    assert_eq!(first.classes().len(), second.classes().len());
    assert_eq!(
        first
            .classes()
            .iter()
            .flat_map(|class| [class.classfile_path(), class.tasty_path()])
            .collect::<Vec<_>>(),
        [
            "example/Counter.class",
            "example/Counter.tasty",
            "example/CounterOwner.class",
            "example/CounterOwner.tasty",
            "example/CounterOwnerFactory.class",
            "example/CounterOwnerFactory.tasty",
        ]
    );
    for (left, right) in first.classes().iter().zip(second.classes()) {
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
        let mut reader = Reader::new(left.classfile_bytes());
        let class = ClassFile::decode(&mut reader).unwrap();
        assert!(reader.is_at_end());
        assert_eq!((class.version.major, class.version.minor), (65, 0));
        assert_eq!(class.tasty_link_id().unwrap(), Some(left.tasty_link_id()));
        validate_classfile_tasty_link(&class, left.tasty_bytes()).unwrap();
    }

    let factory = first
        .classes()
        .iter()
        .find(|class| class.classfile_path() == "example/CounterOwnerFactory.class")
        .unwrap();
    let mut reader = Reader::new(factory.classfile_bytes());
    let class = ClassFile::decode(&mut reader).unwrap();
    let create = class
        .methods
        .iter()
        .find(|method| {
            class.constant_pool.utf8(method.name_index).unwrap() == "create"
                && class.constant_pool.utf8(method.descriptor_index).unwrap()
                    == "(I)Lexample/CounterOwner;"
        })
        .unwrap();
    let [dotty_classfile::attribute::Attribute::Code(code)] = create.attributes.as_slice() else {
        panic!("create has one Code attribute");
    };
    assert_eq!(code.code.len(), 15);
    assert_eq!(
        [
            code.code[0],
            code.code[1],
            code.code[2],
            code.code[5],
            code.code[6],
            code.code[9],
            code.code[10],
            code.code[11],
            code.code[14]
        ],
        [0x2a, 0x1b, 0xb6, 0x4d, 0xbb, 0x59, 0x2c, 0xb7, 0xb0]
    );
    assert_eq!(
        code.code.iter().filter(|opcode| **opcode == 0xb6).count(),
        1,
        "create invokes makeCounter exactly once"
    );
    assert_eq!(
        code.code.iter().filter(|opcode| **opcode == 0xbb).count(),
        1,
        "create allocates only the outer CounterOwner"
    );
    let owner_allocation = ConstantPoolIndex(u16::from_be_bytes([code.code[7], code.code[8]]));
    assert_eq!(
        class.constant_pool.class_name(owner_allocation).unwrap(),
        "example/CounterOwner",
        "the only allocation in create is its returned Owner"
    );
    assert_eq!((code.max_stack, code.max_locals), (3, 3));
    let method_ref = |offset: usize| {
        let index = ConstantPoolIndex(u16::from_be_bytes([
            code.code[offset],
            code.code[offset + 1],
        ]));
        let Some(ConstantPoolEntry::Methodref {
            class_index,
            name_and_type_index,
        }) = class.constant_pool.get(index)
        else {
            panic!("call operand references a Methodref");
        };
        (
            class.constant_pool.class_name(*class_index).unwrap(),
            class
                .constant_pool
                .name_and_type(*name_and_type_index)
                .unwrap(),
        )
    };
    assert_eq!(
        method_ref(3),
        (
            "example/CounterOwnerFactory",
            ("makeCounter", "(I)Lexample/Counter;")
        )
    );
    assert_eq!(
        method_ref(12),
        ("example/CounterOwner", ("<init>", "(Lexample/Counter;)V"))
    );
    let operand = |offset: usize| {
        ConstantPoolIndexOperand::try_from(u16::from_be_bytes([
            code.code[offset],
            code.code[offset + 1],
        ]))
        .unwrap()
    };
    let mut create_sequence = CodeSequence::new();
    create_sequence
        .instruction(Instruction::ALoad(LocalIndex::new(0)))
        .instruction(Instruction::ILoad(LocalIndex::new(1)))
        .instruction(Instruction::InvokeVirtual(VirtualMethodReference::new(
            operand(3),
        )))
        .instruction(Instruction::AStore(LocalIndex::new(2)))
        .instruction(Instruction::New(ClassReference::new(operand(7))))
        .instruction(Instruction::Dup)
        .instruction(Instruction::ALoad(LocalIndex::new(2)))
        .instruction(Instruction::InvokeSpecial(SpecialMethodReference::new(
            operand(12),
        )))
        .instruction(Instruction::AReturn);
    let graph = ControlFlowGraph::build(&create_sequence).unwrap();
    let analyzed = CfgCodeAnalyzer::analyze_with_current_class(
        &graph,
        "create",
        "(I)Lexample/CounterOwner;",
        false,
        &class.constant_pool,
        class.this_class,
    )
    .expect("class-aware CFG analysis preserves the initialized Counter through local slot 2");
    assert_eq!(analyzed.max_stack, code.max_stack);
    assert_eq!(analyzed.max_locals, code.max_locals);

    let stage = TestDirectory::new("reference-local-six-artifacts");
    let destination = stage.path().join("classes");
    let published = publish_paired_batch(&destination, &first, None).unwrap();
    assert_eq!(published.len(), 6);
    assert_eq!(read_relative_files(&destination).unwrap().len(), 6);
    for (index, fault) in (1..=6)
        .map(BatchPublishFault::AfterStagedFiles)
        .chain([
            BatchPublishFault::BeforePromotion,
            BatchPublishFault::AfterPromotion,
        ])
        .enumerate()
    {
        let failed_destination = stage.path().join(format!("failed-{index}"));
        assert!(publish_paired_batch(&failed_destination, &first, Some(fault)).is_err());
        assert!(!failed_destination.exists(), "fault case {index}");
    }
    let sentinel_destination = stage.path().join("existing");
    std::fs::create_dir(&sentinel_destination).unwrap();
    let sentinel = sentinel_destination.join("sentinel");
    std::fs::write(&sentinel, b"leave unchanged").unwrap();
    assert!(publish_paired_batch(&sentinel_destination, &first, None).is_err());
    assert_eq!(std::fs::read(sentinel).unwrap(), b"leave unchanged");

    if require_exact_jdk_21_or_skip() {
        let runtime = TestDirectory::new("reference-local-jdk21-runtime");
        let classes = runtime.path().join("classes");
        let mut original_files = Vec::with_capacity(6);
        for class in first.classes() {
            for (relative, bytes) in [
                (class.classfile_path(), class.classfile_bytes()),
                (class.tasty_path(), class.tasty_bytes()),
            ] {
                let destination = classes.join(relative);
                std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
                std::fs::write(&destination, bytes).unwrap();
                original_files.push((destination, bytes.to_vec()));
            }
        }
        let original_hashes = original_files
            .iter()
            .map(|(path, _)| sha256sum(path))
            .collect::<Vec<_>>();
        for ((path, _), hash) in original_files.iter().zip(&original_hashes) {
            eprintln!("reference-local-original-sha256 {} {hash}", path.display());
        }

        let javap = Command::new("javap")
            .args(["-v", "-p", "-c", "-classpath"])
            .arg(&classes)
            .arg("example.CounterOwnerFactory")
            .output()
            .expect("start JDK 21 javap for original Rust-produced local-flow class");
        assert!(
            javap.status.success(),
            "javap rejected original Rust class: {}",
            String::from_utf8_lossy(&javap.stderr)
        );
        let javap = String::from_utf8_lossy(&javap.stdout);
        for expected in [
            "descriptor: (I)Lexample/CounterOwner;",
            "stack=3, locals=3, args_size=2",
            "invokevirtual #",
            "// Method makeCounter:(I)Lexample/Counter;",
            "astore_2",
            "aload_2",
            "// Method example/CounterOwner.\"<init>\":(Lexample/Counter;)V",
            "areturn",
        ] {
            assert!(
                javap.contains(expected),
                "javap omitted {expected:?}:\n{javap}"
            );
        }

        let harness = runtime.path().join("ReferenceLocalBatchHarness.java");
        std::fs::write(
            &harness,
            r#"import example.Counter;
import example.CounterOwner;
import example.CounterOwnerFactory;
import java.lang.reflect.Field;
public final class ReferenceLocalBatchHarness {
  public static void main(String[] args) throws Exception {
    CounterOwnerFactory factory = new CounterOwnerFactory();
    CounterOwner positive = factory.create(7);
    CounterOwner negative = factory.create(-9);
    CounterOwner third = factory.create(3);
    if (positive.read() != 7 || negative.read() != -9 || third.read() != 3)
      throw new AssertionError("factory result values");
    Field inner = CounterOwner.class.getDeclaredField("inner");
    inner.setAccessible(true);
    Counter first = (Counter) inner.get(positive);
    Counter second = (Counter) inner.get(negative);
    if (first == second || first == inner.get(third))
      throw new AssertionError("each local flow retains a fresh Counter");
    first.set(42);
    if (positive.read() != 42 || negative.read() != -9 || third.read() != 3)
      throw new AssertionError("first local mutation crossed Owners");
    first.set(-9);
    if (positive.read() != -9 || negative.read() != -9 || third.read() != 3)
      throw new AssertionError("second local mutation crossed Owners");
    System.out.println("paired-reference-local=7,-9,3; mutation=42,-9; mutation=-9,-9,3; identities=distinct");
  }
}"#,
        )
        .unwrap();
        let harness_classes = runtime.path().join("harness");
        std::fs::create_dir_all(&harness_classes).unwrap();
        let javac = Command::new("javac")
            .args(["--release", "21", "-classpath"])
            .arg(&classes)
            .arg("-d")
            .arg(&harness_classes)
            .arg(&harness)
            .output()
            .expect("start JDK 21 javac for the paired local batch harness");
        assert!(
            javac.status.success(),
            "javac --release 21 rejected the harness:\n{}",
            String::from_utf8_lossy(&javac.stderr)
        );
        for ((path, _), expected_hash) in original_files.iter().zip(&original_hashes) {
            assert_eq!(
                sha256sum(path),
                *expected_hash,
                "javac changed {}",
                path.display()
            );
        }
        let classpath =
            std::env::join_paths([harness_classes.as_path(), classes.as_path()]).unwrap();
        let java = Command::new("java")
            .args(["-Xverify:all", "-ea", "-classpath"])
            .arg(classpath)
            .arg("ReferenceLocalBatchHarness")
            .output()
            .expect("start strict JDK 21 verification for the paired local batch");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected the paired local batch:\n{}\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&java.stdout).trim(),
            "paired-reference-local=7,-9,3; mutation=42,-9; mutation=-9,-9,3; identities=distinct"
        );

        for ((path, bytes), expected_hash) in original_files.iter().zip(&original_hashes) {
            assert_eq!(
                sha256sum(path),
                *expected_hash,
                "JDK changed {}",
                path.display()
            );
            assert_eq!(
                std::fs::read(path).unwrap(),
                *bytes,
                "JDK changed {}",
                path.display()
            );
        }

        let code_start = code.code.as_ptr() as usize - factory.classfile_bytes().as_ptr() as usize;
        let run_local_negative = |label: &str, corrupt: &[u8]| {
            let negative_root = runtime.path().join(label);
            for artifact in first.classes() {
                let classfile = negative_root.join(artifact.classfile_path());
                std::fs::create_dir_all(classfile.parent().unwrap()).unwrap();
                std::fs::write(
                    classfile,
                    if artifact.classfile_path() == "example/CounterOwnerFactory.class" {
                        corrupt
                    } else {
                        artifact.classfile_bytes()
                    },
                )
                .unwrap();
            }
            let classpath =
                std::env::join_paths([harness_classes.as_path(), negative_root.as_path()]).unwrap();
            let result = Command::new("java")
                .args(["-Xverify:all", "-ea", "-classpath"])
                .arg(classpath)
                .arg("ReferenceLocalBatchHarness")
                .output()
                .expect("start isolated JDK 21 local-flow negative control");
            let output = format!(
                "{}{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            assert!(
                !result.status.success() && output.contains("VerifyError"),
                "{label} should fail verification before the harness runs:\n{output}"
            );
            eprintln!(
                "reference-local-negative {label}: {}",
                output
                    .lines()
                    .find(|line| line.contains("VerifyError"))
                    .unwrap_or("<VerifyError asserted; JVM emitted no matching line>")
            );
        };

        let mut wrong_store = factory.classfile_bytes().to_vec();
        wrong_store[code_start + 5] = 0x4b; // astore_0 stores the Counter into Factory this.
        run_local_negative("wrong-reference-local-store", &wrong_store);

        let mut wrong_load = factory.classfile_bytes().to_vec();
        wrong_load[code_start + 10] = 0x2b; // aload_1 loads the Int seed instead of slot 2.
        run_local_negative("wrong-reference-local-load", &wrong_load);

        let mut undersized_stack = factory.classfile_bytes().to_vec();
        undersized_stack[code_start - 8..code_start - 6].copy_from_slice(&2u16.to_be_bytes());
        run_local_negative("undersized-reference-local-stack", &undersized_stack);
    }
}

#[test]
fn reference_local_renamed_and_reordered_batches_are_independently_deterministic() {
    let renamed = REFERENCE_LOCAL_BATCH
        .replace("package example", "package sample")
        .replace("CounterOwnerFactory", "TokenFactory")
        .replace("CounterOwner", "TokenBox")
        .replace("Counter", "Token");
    let parts = renamed.split("\nclass ").collect::<Vec<_>>();
    assert_eq!(parts.len(), 4);
    let reordered = format!(
        "{}\nclass {}\nclass {}\nclass {}",
        parts[0], parts[3], parts[1], parts[2]
    );

    for (source, source_name, source_id, repeat_id, expected_names) in [
        (
            renamed.as_str(),
            "TokenFactory.scala",
            18532,
            18533,
            ["sample/Token", "sample/TokenBox", "sample/TokenFactory"],
        ),
        (
            reordered.as_str(),
            "TokenFactory.scala",
            18534,
            18535,
            ["sample/TokenFactory", "sample/Token", "sample/TokenBox"],
        ),
    ] {
        let first =
            emit_paired_jvm_tasty_batch(source, source_name, SourceId::from_index(source_id))
                .expect("bounded rename/reorder input emits a paired batch");
        let second =
            emit_paired_jvm_tasty_batch(source, source_name, SourceId::from_index(repeat_id))
                .expect("a fresh session repeats the same renamed/reordered batch");
        assert_eq!(first.classes().len(), 3);
        for (index, (left, right)) in first.classes().iter().zip(second.classes()).enumerate() {
            let internal_name = expected_names[index];
            assert_eq!(left.classfile_path(), format!("{internal_name}.class"));
            assert_eq!(left.tasty_path(), format!("{internal_name}.tasty"));
            assert_eq!(left.classfile_path(), right.classfile_path());
            assert_eq!(left.tasty_path(), right.tasty_path());
            assert_eq!(left.classfile_bytes(), right.classfile_bytes());
            assert_eq!(left.tasty_bytes(), right.tasty_bytes());
            assert_eq!(left.tasty_link_id(), right.tasty_link_id());
        }
    }
}

#[test]
fn factory_emits_four_complete_linked_artifacts_from_one_source_session() {
    let batch = emit_paired_jvm_tasty_batch(
        COUNTER_FACTORY,
        "tests/fixtures/jvm_tasty_cross_class_factories/CounterFactory.scala",
        SourceId::from_index(1694),
    )
    .unwrap();
    let second = emit_paired_jvm_tasty_batch(
        COUNTER_FACTORY,
        "tests/fixtures/jvm_tasty_cross_class_factories/CounterFactory.scala",
        SourceId::from_index(1695),
    )
    .unwrap();
    assert_eq!(batch.classes().len(), 2);
    assert_ne!(batch.source_id(), second.source_id());

    let expected = [
        (
            "example/Counter",
            "example/Counter.class",
            "example/Counter.tasty",
            [("<init>", "(I)V"), ("get", "()I"), ("set", "(I)V")].as_slice(),
        ),
        (
            "example/CounterFactory",
            "example/CounterFactory.class",
            "example/CounterFactory.tasty",
            [
                ("<init>", "()V"),
                ("create", "(I)Lexample/Counter;"),
                ("identity", "(Lexample/Counter;)Lexample/Counter;"),
            ]
            .as_slice(),
        ),
    ];
    for (index, (pair, (internal_name, classfile_path, tasty_path, expected_methods))) in
        batch.classes().iter().zip(expected).enumerate()
    {
        let second_pair = &second.classes()[index];
        assert_eq!(pair.classfile_path(), classfile_path);
        assert_eq!(pair.tasty_path(), tasty_path);
        assert_eq!(pair.source_id(), batch.source_id());
        assert_eq!(pair.classfile_bytes(), second_pair.classfile_bytes());
        assert_eq!(pair.tasty_bytes(), second_pair.tasty_bytes());
        assert_eq!(pair.tasty_link_id(), second_pair.tasty_link_id());

        let tasty = TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
        assert_eq!(
            (
                tasty.header().major_version,
                tasty.header().minor_version,
                tasty.header().experimental_version,
            ),
            (28, 9, 0)
        );
        assert_eq!(
            pair.tasty_link_id(),
            TastyLinkId::from_bytes(tasty.header().uuid)
        );

        let mut reader = Reader::new(pair.classfile_bytes());
        let class_file = ClassFile::decode(&mut reader).unwrap();
        assert!(reader.is_at_end());
        assert_eq!(
            (class_file.version.major, class_file.version.minor),
            (65, 0)
        );
        assert_eq!(
            class_file
                .constant_pool
                .class_name(class_file.this_class)
                .unwrap(),
            internal_name
        );
        assert_eq!(
            class_file.tasty_link_id().unwrap(),
            Some(pair.tasty_link_id())
        );
        validate_classfile_tasty_link(&class_file, pair.tasty_bytes()).unwrap();

        let package = tasty
            .asts()
            .unwrap()
            .get(0)
            .unwrap()
            .decode_package()
            .unwrap();
        assert_eq!(package.stats.len(), 1);
        let DefinitionBody::TypeDef { name, .. } = package
            .stats
            .get(0)
            .unwrap()
            .decode_definition_body()
            .unwrap()
        else {
            panic!("each sidecar owns its corresponding source declaration");
        };
        assert_eq!(
            tasty.names().render(name).unwrap(),
            internal_name.rsplit('/').next().unwrap()
        );

        let mut methods = class_file
            .methods
            .iter()
            .map(|method| {
                (
                    class_file.constant_pool.utf8(method.name_index).unwrap(),
                    class_file
                        .constant_pool
                        .utf8(method.descriptor_index)
                        .unwrap(),
                )
            })
            .collect::<Vec<_>>();
        methods.sort();
        let mut expected_methods = expected_methods.to_vec();
        expected_methods.sort();
        assert_eq!(methods, expected_methods);
    }

    let factory = &batch.classes()[1];
    let mut reader = Reader::new(factory.classfile_bytes());
    let class_file = ClassFile::decode(&mut reader).unwrap();
    let create = class_file
        .methods
        .iter()
        .find(|method| class_file.constant_pool.utf8(method.name_index).unwrap() == "create")
        .unwrap();
    let [Attribute::Code(code)] = create.attributes.as_slice() else {
        panic!("factory create has one Code attribute");
    };
    assert_eq!(
        code.code,
        [0xbb, 0x00, 0x0e, 0x59, 0x1b, 0xb7, 0x00, 0x11, 0xb0]
    );
}

#[test]
fn identity_only_counter_factory_keeps_the_existing_linked_batch_path() {
    let batch = emit_paired_jvm_tasty_batch(
        COUNTER_FACTORY_IDENTITY_ONLY,
        "tests/fixtures/jvm_tasty_cross_class_factories/CounterFactoryIdentityOnly.scala",
        SourceId::from_index(1696),
    )
    .unwrap();

    assert_eq!(batch.classes().len(), 2);
    assert_eq!(batch.classes()[0].classfile_path(), "example/Counter.class");
    assert_eq!(
        batch.classes()[1].classfile_path(),
        "example/CounterFactory.class"
    );
    for pair in batch.classes() {
        assert_eq!(
            pair.tasty_link_id(),
            TastyLinkId::from_bytes(
                TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes())
                    .unwrap()
                    .header()
                    .uuid
            )
        );
    }
}

#[test]
fn primitive_create_method_does_not_activate_the_counter_factory_proof() {
    let batch = emit_paired_jvm_tasty_batch(
        COUNTER_FACTORY_PRIMITIVE_CREATE,
        "tests/fixtures/jvm_tasty_cross_class_factories/CounterFactoryPrimitiveCreate.scala",
        SourceId::from_index(1697),
    )
    .unwrap();

    assert_eq!(batch.classes().len(), 2);
    let factory = &batch.classes()[1];
    let mut reader = Reader::new(factory.classfile_bytes());
    let class_file = ClassFile::decode(&mut reader).unwrap();
    let create = class_file
        .methods
        .iter()
        .find(|method| class_file.constant_pool.utf8(method.name_index).unwrap() == "create")
        .unwrap();
    assert_eq!(
        class_file
            .constant_pool
            .utf8(create.descriptor_index)
            .unwrap(),
        "(I)I"
    );
}

#[test]
fn reference_returning_create_formal_keeps_the_existing_identity_shape() {
    let batch = emit_paired_jvm_tasty_batch(
        COUNTER_FACTORY_REFERENCE_CREATE,
        "tests/fixtures/jvm_tasty_cross_class_factories/CounterFactoryReferenceCreate.scala",
        SourceId::from_index(1698),
    )
    .unwrap();

    assert_eq!(batch.classes().len(), 2);
    let factory = &batch.classes()[1];
    let mut reader = Reader::new(factory.classfile_bytes());
    let class_file = ClassFile::decode(&mut reader).unwrap();
    let create = class_file
        .methods
        .iter()
        .find(|method| class_file.constant_pool.utf8(method.name_index).unwrap() == "create")
        .unwrap();
    assert_eq!(
        class_file
            .constant_pool
            .utf8(create.descriptor_index)
            .unwrap(),
        "(Lexample/Counter;)Lexample/Counter;"
    );
}

#[test]
fn counter_reader_emits_four_complete_linked_cross_class_artifacts() {
    let batch = emit_paired_jvm_tasty_batch(
        COUNTER_READER,
        "tests/fixtures/jvm_tasty_cross_class/Reader.scala",
        SourceId::from_index(1675),
    )
    .unwrap();
    let second = emit_paired_jvm_tasty_batch(
        COUNTER_READER,
        "tests/fixtures/jvm_tasty_cross_class/Reader.scala",
        SourceId::from_index(1676),
    )
    .unwrap();
    assert_eq!(batch.classes().len(), 2);
    assert_ne!(batch.source_id(), second.source_id());
    assert_eq!(batch.classes().len(), second.classes().len());
    for (first, second) in batch.classes().iter().zip(second.classes()) {
        assert_eq!(first.classfile_path(), second.classfile_path());
        assert_eq!(first.classfile_bytes(), second.classfile_bytes());
        assert_eq!(first.tasty_path(), second.tasty_path());
        assert_eq!(first.tasty_bytes(), second.tasty_bytes());
        assert_eq!(first.tasty_link_id(), second.tasty_link_id());
    }

    let expected = [
        (
            "Counter",
            "example/Counter",
            "example/Counter.class",
            "example/Counter.tasty",
        ),
        (
            "Reader",
            "example/Reader",
            "example/Reader.class",
            "example/Reader.tasty",
        ),
    ];
    let mut decoded_classes = Vec::new();
    for (pair, (simple_name, internal_name, classfile_path, tasty_path)) in
        batch.classes().iter().zip(expected)
    {
        assert_eq!(pair.classfile_path(), classfile_path);
        assert_eq!(pair.tasty_path(), tasty_path);
        assert_eq!(pair.source_id(), batch.source_id());

        let mut class_reader = Reader::new(pair.classfile_bytes());
        let class = ClassFile::decode(&mut class_reader).unwrap();
        assert!(class_reader.is_at_end());
        assert_eq!((class.version.major, class.version.minor), (65, 0));
        assert_eq!(
            class.constant_pool.class_name(class.this_class).unwrap(),
            internal_name
        );
        assert_eq!(class.tasty_link_id().unwrap(), Some(pair.tasty_link_id()));

        let tasty = TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
        assert_eq!(
            (
                tasty.header().major_version,
                tasty.header().minor_version,
                tasty.header().experimental_version,
            ),
            (28, 9, 0)
        );
        assert_eq!(
            pair.tasty_link_id(),
            TastyLinkId::from_bytes(tasty.header().uuid)
        );
        validate_classfile_tasty_link(&class, pair.tasty_bytes()).unwrap();

        let package = tasty
            .asts()
            .unwrap()
            .get(0)
            .unwrap()
            .decode_package()
            .unwrap();
        assert_eq!(package.stats.len(), 1, "each sidecar owns one source class");
        let DefinitionBody::TypeDef { name, .. } = package
            .stats
            .get(0)
            .unwrap()
            .decode_definition_body()
            .unwrap()
        else {
            panic!("each sidecar declares its corresponding source class");
        };
        assert_eq!(tasty.names().render(name).unwrap(), simple_name);
        decoded_classes.push(class);
    }

    let counter = &decoded_classes[0];
    let mut counter_methods = counter
        .methods
        .iter()
        .map(|method| {
            (
                counter.constant_pool.utf8(method.name_index).unwrap(),
                counter.constant_pool.utf8(method.descriptor_index).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    counter_methods.sort();
    assert_eq!(
        counter_methods,
        [("<init>", "(I)V"), ("get", "()I"), ("set", "(I)V")]
    );
    assert_eq!(counter.fields.len(), 1);
    assert_eq!(
        (
            counter
                .constant_pool
                .utf8(counter.fields[0].name_index)
                .unwrap(),
            counter
                .constant_pool
                .utf8(counter.fields[0].descriptor_index)
                .unwrap(),
        ),
        ("current", "I")
    );
    assert!(
        counter.fields[0]
            .access_flags
            .contains(dotty_classfile::access_flags::ACC_PRIVATE)
    );
    assert!(
        !counter.fields[0]
            .access_flags
            .contains(dotty_classfile::access_flags::ACC_FINAL)
    );

    let reader = &decoded_classes[1];
    assert!(
        reader.fields.is_empty(),
        "Reader must not copy Counter state"
    );
    let read = reader
        .methods
        .iter()
        .find(|method| reader.constant_pool.utf8(method.name_index).unwrap() == "read")
        .expect("Reader declares read");
    let mut reader_methods = reader
        .methods
        .iter()
        .map(|method| {
            (
                reader.constant_pool.utf8(method.name_index).unwrap(),
                reader.constant_pool.utf8(method.descriptor_index).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    reader_methods.sort();
    assert_eq!(
        reader_methods,
        [("<init>", "()V"), ("read", "(Lexample/Counter;)I")]
    );
    assert_eq!(
        reader.constant_pool.utf8(read.descriptor_index).unwrap(),
        "(Lexample/Counter;)I"
    );
    let [Attribute::Code(code)] = read.attributes.as_slice() else {
        panic!("Reader.read has exactly one Code attribute");
    };
    assert_eq!(code.code.len(), 5);
    assert_eq!(&code.code[..2], &[0x2b, 0xb6], "aload_1; invokevirtual");
    assert_eq!(code.code[4], 0xac, "ireturn");
    let method_ref = ConstantPoolIndex(u16::from_be_bytes([code.code[2], code.code[3]]));
    let ConstantPoolEntry::Methodref {
        class_index,
        name_and_type_index,
    } = reader.constant_pool.get(method_ref).unwrap()
    else {
        panic!("Reader.read invokes Counter.get through CONSTANT_Methodref");
    };
    assert_eq!(
        reader.constant_pool.class_name(*class_index).unwrap(),
        "example/Counter"
    );
    assert_eq!(
        reader
            .constant_pool
            .name_and_type(*name_and_type_index)
            .unwrap(),
        ("get", "()I")
    );
}

#[test]
fn reference_field_owner_emits_a_complete_deterministic_linked_batch() {
    let first = emit_paired_jvm_tasty_batch(
        COUNTER_OWNER,
        "tests/fixtures/jvm_tasty_object_composition/CounterOwner.scala",
        SourceId::from_index(1719),
    )
    .unwrap();
    let second = emit_paired_jvm_tasty_batch(
        COUNTER_OWNER,
        "tests/fixtures/jvm_tasty_object_composition/CounterOwner.scala",
        SourceId::from_index(1720),
    )
    .unwrap();
    assert_ne!(first.source_id(), second.source_id());
    assert_eq!(first.classes().len(), 2);
    assert_eq!(second.classes().len(), 2);

    let expected = [
        (
            "example/Counter.class",
            "example/Counter.tasty",
            "example/Counter",
        ),
        (
            "example/CounterOwner.class",
            "example/CounterOwner.tasty",
            "example/CounterOwner",
        ),
    ];
    let mut classes = Vec::new();
    for ((pair, second_pair), (classfile_path, tasty_path, internal_name)) in
        first.classes().iter().zip(second.classes()).zip(expected)
    {
        assert_eq!(pair.classfile_path(), classfile_path);
        assert_eq!(pair.tasty_path(), tasty_path);
        assert_eq!(pair.source_id(), first.source_id());
        assert_eq!(pair.classfile_path(), second_pair.classfile_path());
        assert_eq!(pair.tasty_path(), second_pair.tasty_path());
        assert_eq!(pair.classfile_bytes(), second_pair.classfile_bytes());
        assert_eq!(pair.tasty_bytes(), second_pair.tasty_bytes());
        assert_eq!(pair.tasty_link_id(), second_pair.tasty_link_id());

        let tasty = TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
        assert_eq!(
            (
                tasty.header().major_version,
                tasty.header().minor_version,
                tasty.header().experimental_version,
            ),
            (28, 9, 0)
        );
        assert_eq!(
            pair.tasty_link_id(),
            TastyLinkId::from_bytes(tasty.header().uuid)
        );
        let mut reader = Reader::new(pair.classfile_bytes());
        let class = ClassFile::decode(&mut reader).unwrap();
        assert!(reader.is_at_end());
        assert_eq!((class.version.major, class.version.minor), (65, 0));
        assert_eq!(
            class.constant_pool.class_name(class.this_class).unwrap(),
            internal_name
        );
        assert_eq!(class.tasty_link_id().unwrap(), Some(pair.tasty_link_id()));
        validate_classfile_tasty_link(&class, pair.tasty_bytes()).unwrap();
        classes.push(class);
    }

    for (class, expected_name, expected_fields, expected_methods) in [
        (
            &classes[0],
            "example/Counter",
            vec![("current", "I", true, false)],
            vec![("<init>", "(I)V"), ("get", "()I"), ("set", "(I)V")],
        ),
        (
            &classes[1],
            "example/CounterOwner",
            vec![("inner", "Lexample/Counter;", true, true)],
            vec![("<init>", "(I)V"), ("read", "()I")],
        ),
    ] {
        assert_eq!(
            class.constant_pool.class_name(class.this_class).unwrap(),
            expected_name
        );
        assert_eq!(
            class
                .constant_pool
                .class_name(class.super_class.unwrap())
                .unwrap(),
            "java/lang/Object"
        );
        assert_eq!(class.fields.len(), expected_fields.len());
        for (field, (name, descriptor, private, final_field)) in
            class.fields.iter().zip(expected_fields)
        {
            assert_eq!(class.constant_pool.utf8(field.name_index).unwrap(), name);
            assert_eq!(
                class.constant_pool.utf8(field.descriptor_index).unwrap(),
                descriptor
            );
            assert_eq!(
                field
                    .access_flags
                    .contains(dotty_classfile::access_flags::ACC_PRIVATE),
                private
            );
            assert_eq!(
                field
                    .access_flags
                    .contains(dotty_classfile::access_flags::ACC_FINAL),
                final_field
            );
            assert!(
                !field
                    .access_flags
                    .contains(dotty_classfile::access_flags::ACC_STATIC),
                "source instance fields remain instance fields"
            );
        }
        assert!(class.methods.iter().all(|method| {
            method
                .access_flags
                .contains(dotty_classfile::access_flags::ACC_PUBLIC)
        }));
        assert_eq!(
            class
                .methods
                .iter()
                .map(|method| (
                    class.constant_pool.utf8(method.name_index).unwrap(),
                    class.constant_pool.utf8(method.descriptor_index).unwrap(),
                ))
                .collect::<Vec<_>>(),
            expected_methods
        );
    }

    let owner = &classes[1];
    let constructor = owner
        .methods
        .iter()
        .find(|method| owner.constant_pool.utf8(method.name_index).unwrap() == "<init>")
        .unwrap();
    let [Attribute::Code(constructor_code)] = constructor.attributes.as_slice() else {
        panic!("CounterOwner constructor has exactly one Code attribute");
    };
    assert_eq!(constructor_code.code.len(), 17);
    assert_eq!(constructor_code.max_stack, 4);
    assert_eq!(constructor_code.max_locals, 2);
    assert!(constructor_code.exception_table.is_empty());
    assert!(constructor_code.attributes.is_empty());
    assert_eq!(
        &constructor_code.code[..2],
        &[0x2a, 0xb7],
        "aload_0; Object init"
    );
    assert_eq!(
        &constructor_code.code[4..6],
        &[0x2a, 0xbb],
        "aload_0; new Counter"
    );
    assert_eq!(constructor_code.code[8], 0x59, "dup allocated Counter");
    assert_eq!(
        constructor_code.code[9], 0x1b,
        "load original seed argument"
    );
    assert_eq!(
        constructor_code.code[13], 0xb5,
        "putfield inner exactly once"
    );
    assert_eq!(constructor_code.code[16], 0xb1, "return");
    let super_ref = ConstantPoolIndex(u16::from_be_bytes([
        constructor_code.code[2],
        constructor_code.code[3],
    ]));
    let ConstantPoolEntry::Methodref {
        class_index,
        name_and_type_index,
    } = owner.constant_pool.get(super_ref).unwrap()
    else {
        panic!("owner constructor calls Object.<init> through CONSTANT_Methodref");
    };
    assert_eq!(
        owner.constant_pool.class_name(*class_index).unwrap(),
        "java/lang/Object"
    );
    assert_eq!(
        owner
            .constant_pool
            .name_and_type(*name_and_type_index)
            .unwrap(),
        ("<init>", "()V")
    );
    let new_ref = ConstantPoolIndex(u16::from_be_bytes([
        constructor_code.code[6],
        constructor_code.code[7],
    ]));
    let ConstantPoolEntry::Class { name_index } = owner.constant_pool.get(new_ref).unwrap() else {
        panic!("owner constructor allocates Counter through CONSTANT_Class");
    };
    assert_eq!(
        owner.constant_pool.utf8(*name_index).unwrap(),
        "example/Counter"
    );
    let counter_constructor_ref = ConstantPoolIndex(u16::from_be_bytes([
        constructor_code.code[11],
        constructor_code.code[12],
    ]));
    let ConstantPoolEntry::Methodref {
        class_index,
        name_and_type_index,
    } = owner.constant_pool.get(counter_constructor_ref).unwrap()
    else {
        panic!("new Counter invokes CONSTANT_Methodref <init>");
    };
    assert_eq!(
        owner.constant_pool.class_name(*class_index).unwrap(),
        "example/Counter"
    );
    assert_eq!(
        owner
            .constant_pool
            .name_and_type(*name_and_type_index)
            .unwrap(),
        ("<init>", "(I)V")
    );
    let store_ref = ConstantPoolIndex(u16::from_be_bytes([
        constructor_code.code[14],
        constructor_code.code[15],
    ]));
    let ConstantPoolEntry::Fieldref {
        class_index,
        name_and_type_index,
    } = owner.constant_pool.get(store_ref).unwrap()
    else {
        panic!("owner constructor stores through CONSTANT_Fieldref");
    };
    assert_eq!(
        owner.constant_pool.class_name(*class_index).unwrap(),
        "example/CounterOwner"
    );
    assert_eq!(
        owner
            .constant_pool
            .name_and_type(*name_and_type_index)
            .unwrap(),
        ("inner", "Lexample/Counter;")
    );
    assert_eq!(owner.fields.len(), 1);
    let field = &owner.fields[0];
    assert_eq!(owner.constant_pool.utf8(field.name_index).unwrap(), "inner");
    assert_eq!(
        owner.constant_pool.utf8(field.descriptor_index).unwrap(),
        "Lexample/Counter;"
    );
    assert!(
        field
            .access_flags
            .contains(dotty_classfile::access_flags::ACC_PRIVATE)
    );
    assert!(
        field
            .access_flags
            .contains(dotty_classfile::access_flags::ACC_FINAL)
    );
    assert!(
        !field
            .access_flags
            .contains(dotty_classfile::access_flags::ACC_STATIC)
    );
    assert_eq!(
        owner
            .methods
            .iter()
            .map(|method| (
                owner.constant_pool.utf8(method.name_index).unwrap(),
                owner.constant_pool.utf8(method.descriptor_index).unwrap(),
            ))
            .collect::<Vec<_>>(),
        [("<init>", "(I)V"), ("read", "()I")]
    );

    let read = owner
        .methods
        .iter()
        .find(|method| owner.constant_pool.utf8(method.name_index).unwrap() == "read")
        .unwrap();
    let [Attribute::Code(code)] = read.attributes.as_slice() else {
        panic!("CounterOwner.read has exactly one Code attribute");
    };
    assert_eq!(code.code.len(), 8);
    assert_eq!(code.max_stack, 1);
    assert_eq!(code.max_locals, 1);
    assert!(code.exception_table.is_empty());
    assert!(code.attributes.is_empty());
    assert_eq!(&code.code[..2], &[0x2a, 0xb4], "aload_0; getfield");
    assert_eq!(code.code[4], 0xb6, "invokevirtual Counter.get");
    assert_eq!(code.code[7], 0xac, "ireturn");
    let field_ref = ConstantPoolIndex(u16::from_be_bytes([code.code[2], code.code[3]]));
    let ConstantPoolEntry::Fieldref {
        class_index,
        name_and_type_index,
    } = owner.constant_pool.get(field_ref).unwrap()
    else {
        panic!("read loads inner through CONSTANT_Fieldref");
    };
    assert_eq!(
        owner.constant_pool.class_name(*class_index).unwrap(),
        "example/CounterOwner"
    );
    assert_eq!(
        owner
            .constant_pool
            .name_and_type(*name_and_type_index)
            .unwrap(),
        ("inner", "Lexample/Counter;")
    );
    let method_ref = ConstantPoolIndex(u16::from_be_bytes([code.code[5], code.code[6]]));
    let ConstantPoolEntry::Methodref {
        class_index,
        name_and_type_index,
    } = owner.constant_pool.get(method_ref).unwrap()
    else {
        panic!("read invokes Counter.get through CONSTANT_Methodref");
    };
    assert_eq!(
        owner.constant_pool.class_name(*class_index).unwrap(),
        "example/Counter"
    );
    assert_eq!(
        owner
            .constant_pool
            .name_and_type(*name_and_type_index)
            .unwrap(),
        ("get", "()I")
    );
}

#[test]
fn constructor_injected_reference_emits_complete_linked_batch_deterministically() {
    let first = emit_paired_jvm_tasty_batch(
        COUNTER_OWNER_ALIAS,
        "CounterOwner.scala",
        SourceId::from_index(17561),
    )
    .unwrap();
    let second = emit_paired_jvm_tasty_batch(
        COUNTER_OWNER_ALIAS,
        "CounterOwner.scala",
        SourceId::from_index(17562),
    )
    .unwrap();

    assert_ne!(first.source_id(), second.source_id());
    assert_eq!(first.classes().len(), 2);
    assert_eq!(second.classes().len(), 2);
    for (index, expected) in [
        ("example/Counter.class", "example/Counter.tasty"),
        ("example/CounterOwner.class", "example/CounterOwner.tasty"),
    ]
    .into_iter()
    .enumerate()
    {
        let first = &first.classes()[index];
        let second = &second.classes()[index];
        assert_eq!((first.classfile_path(), first.tasty_path()), expected);
        assert_eq!(first.classfile_bytes(), second.classfile_bytes());
        assert_eq!(first.tasty_bytes(), second.tasty_bytes());
        assert_eq!(first.tasty_link_id(), second.tasty_link_id());
        let mut reader = Reader::new(first.classfile_bytes());
        let class = ClassFile::decode(&mut reader).unwrap();
        assert!(reader.is_at_end());
        assert_eq!((class.version.major, class.version.minor), (65, 0));
        assert_eq!(
            class.constant_pool.class_name(class.this_class).unwrap(),
            if index == 0 {
                "example/Counter"
            } else {
                "example/CounterOwner"
            }
        );
        validate_classfile_tasty_link(&class, first.tasty_bytes()).unwrap();
    }

    assert_eq!(
        first
            .classes()
            .iter()
            .flat_map(|pair| [pair.classfile_path(), pair.tasty_path()])
            .collect::<Vec<_>>(),
        [
            "example/Counter.class",
            "example/Counter.tasty",
            "example/CounterOwner.class",
            "example/CounterOwner.tasty",
        ]
    );
    let stage = TestDirectory::new("constructor-injected-reference-batch");
    let destination = stage.path().join("classes");
    let published = publish_paired_batch(&destination, &first, None).unwrap();
    assert_eq!(
        published
            .iter()
            .map(|path| path.strip_prefix(&destination).unwrap().to_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "example/Counter.class",
            "example/Counter.tasty",
            "example/CounterOwner.class",
            "example/CounterOwner.tasty",
        ]
    );
    assert_eq!(read_relative_files(&destination).unwrap().len(), 4);
    let owner = &first.classes()[1];
    let mut reader = Reader::new(owner.classfile_bytes());
    let class = ClassFile::decode(&mut reader).unwrap();
    assert!(reader.is_at_end());
    assert_eq!(class.fields.len(), 1);
    assert_eq!(
        class
            .constant_pool
            .utf8(class.fields[0].name_index)
            .unwrap(),
        "inner"
    );
    assert_eq!(
        class
            .constant_pool
            .utf8(class.fields[0].descriptor_index)
            .unwrap(),
        "Lexample/Counter;"
    );
    assert!(
        class.fields[0]
            .access_flags
            .contains(dotty_classfile::access_flags::ACC_PRIVATE)
    );
    assert!(
        class.fields[0]
            .access_flags
            .contains(dotty_classfile::access_flags::ACC_FINAL)
    );
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
        "(Lexample/Counter;)V"
    );
    let [Attribute::Code(constructor_code)] = constructor.attributes.as_slice() else {
        panic!("injected-reference constructor has one Code attribute");
    };
    assert_eq!(constructor_code.max_locals, 2);
    assert_eq!(
        constructor_code
            .code
            .iter()
            .filter(|byte| **byte == 0xbb)
            .count(),
        0,
        "Owner constructor allocates no Counter"
    );
    assert_eq!(
        constructor_code.code,
        [
            0x2a,
            0xb7,
            constructor_code.code[2],
            constructor_code.code[3],
            0x2a,
            0x2b,
            0xb5,
            constructor_code.code[7],
            constructor_code.code[8],
            0xb1,
        ]
    );
    let field_ref = ConstantPoolIndex(u16::from_be_bytes([
        constructor_code.code[7],
        constructor_code.code[8],
    ]));
    let ConstantPoolEntry::Fieldref {
        class_index,
        name_and_type_index,
    } = class.constant_pool.get(field_ref).unwrap()
    else {
        panic!("constructor initializes inner through CONSTANT_Fieldref");
    };
    assert_eq!(
        class.constant_pool.class_name(*class_index).unwrap(),
        "example/CounterOwner"
    );
    assert_eq!(
        class
            .constant_pool
            .name_and_type(*name_and_type_index)
            .unwrap(),
        ("inner", "Lexample/Counter;")
    );
    let read = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == "read")
        .unwrap();
    assert_eq!(
        class.constant_pool.utf8(read.descriptor_index).unwrap(),
        "()I"
    );
    let [Attribute::Code(read_code)] = read.attributes.as_slice() else {
        panic!("Owner.read has one Code attribute");
    };
    assert_eq!(&read_code.code[..2], &[0x2a, 0xb4]);
    assert_eq!(read_code.code[4], 0xb6);
    assert_eq!(read_code.code[7], 0xac);
    let method_ref = ConstantPoolIndex(u16::from_be_bytes([read_code.code[5], read_code.code[6]]));
    let ConstantPoolEntry::Methodref {
        class_index,
        name_and_type_index,
    } = class.constant_pool.get(method_ref).unwrap()
    else {
        panic!("Owner.read calls Counter.get through CONSTANT_Methodref");
    };
    assert_eq!(
        class.constant_pool.class_name(*class_index).unwrap(),
        "example/Counter"
    );
    assert_eq!(
        class
            .constant_pool
            .name_and_type(*name_and_type_index)
            .unwrap(),
        ("get", "()I")
    );
}

#[test]
fn reference_constructor_graph_emits_six_complete_deterministic_linked_artifacts() {
    let first = emit_paired_jvm_tasty_batch(
        REFERENCE_CONSTRUCTOR_BATCH,
        "CounterOwnerFactory.scala",
        SourceId::from_index(17971),
    )
    .unwrap();
    let second = emit_paired_jvm_tasty_batch(
        REFERENCE_CONSTRUCTOR_BATCH,
        "CounterOwnerFactory.scala",
        SourceId::from_index(17972),
    )
    .unwrap();
    assert_ne!(first.source_id(), second.source_id());
    assert_eq!(first.classes().len(), 3);
    assert_eq!(second.classes().len(), 3);

    let expected = [
        (
            "example/Counter.class",
            "example/Counter.tasty",
            "example/Counter",
        ),
        (
            "example/CounterOwner.class",
            "example/CounterOwner.tasty",
            "example/CounterOwner",
        ),
        (
            "example/CounterOwnerFactory.class",
            "example/CounterOwnerFactory.tasty",
            "example/CounterOwnerFactory",
        ),
    ];
    let mut decoded = Vec::new();
    for ((artifact, repeat), (classfile_path, tasty_path, internal_name)) in
        first.classes().iter().zip(second.classes()).zip(expected)
    {
        assert_eq!(
            (artifact.classfile_path(), artifact.tasty_path()),
            (classfile_path, tasty_path)
        );
        assert_eq!(artifact.source_id(), first.source_id());
        assert_eq!(artifact.classfile_path(), repeat.classfile_path());
        assert_eq!(artifact.tasty_path(), repeat.tasty_path());
        assert_eq!(artifact.classfile_bytes(), repeat.classfile_bytes());
        assert_eq!(artifact.tasty_bytes(), repeat.tasty_bytes());
        assert_eq!(artifact.tasty_link_id(), repeat.tasty_link_id());

        let tasty = TastyFile::parse_and_validate_scala_3_9(artifact.tasty_bytes()).unwrap();
        assert_eq!(
            (
                tasty.header().major_version,
                tasty.header().minor_version,
                tasty.header().experimental_version,
            ),
            (28, 9, 0)
        );
        assert_eq!(
            artifact.tasty_link_id(),
            TastyLinkId::from_bytes(tasty.header().uuid)
        );

        let mut reader = Reader::new(artifact.classfile_bytes());
        let class = ClassFile::decode(&mut reader).unwrap();
        assert!(reader.is_at_end());
        assert_eq!((class.version.major, class.version.minor), (65, 0));
        assert_eq!(
            class.constant_pool.class_name(class.this_class).unwrap(),
            internal_name
        );
        assert_eq!(
            class.tasty_link_id().unwrap(),
            Some(artifact.tasty_link_id())
        );
        validate_classfile_tasty_link(&class, artifact.tasty_bytes()).unwrap();
        decoded.push(class);
    }

    assert_eq!(
        first
            .classes()
            .iter()
            .flat_map(|artifact| [artifact.classfile_path(), artifact.tasty_path()])
            .collect::<Vec<_>>(),
        [
            "example/Counter.class",
            "example/Counter.tasty",
            "example/CounterOwner.class",
            "example/CounterOwner.tasty",
            "example/CounterOwnerFactory.class",
            "example/CounterOwnerFactory.tasty",
        ]
    );

    let factory = &decoded[2];
    let wrap = factory
        .methods
        .iter()
        .find(|method| {
            factory.constant_pool.utf8(method.name_index).unwrap() == "wrap"
                && factory.constant_pool.utf8(method.descriptor_index).unwrap()
                    == "(Lexample/Counter;)Lexample/CounterOwner;"
        })
        .expect("Factory.wrap has the distinct Counter-to-CounterOwner ABI");
    let [Attribute::Code(code)] = wrap.attributes.as_slice() else {
        panic!("Factory.wrap has one Code attribute");
    };
    assert_eq!(code.code.len(), 9);
    assert_eq!(code.code[0], 0xbb, "new CounterOwner");
    assert_eq!(code.code[3], 0x59, "dup");
    assert_eq!(code.code[4], 0x2b, "aload_1 passes the existing Counter");
    assert_eq!(code.code[5], 0xb7, "invokespecial CounterOwner.<init>");
    assert_eq!(code.code[8], 0xb0, "areturn the initialized Owner");
    let constructor_ref = ConstantPoolIndex(u16::from_be_bytes([code.code[6], code.code[7]]));
    let ConstantPoolEntry::Methodref {
        class_index,
        name_and_type_index,
    } = factory.constant_pool.get(constructor_ref).unwrap()
    else {
        panic!("Factory.wrap invokes a constructor Methodref");
    };
    assert_eq!(
        factory.constant_pool.class_name(*class_index).unwrap(),
        "example/CounterOwner"
    );
    assert_eq!(
        factory
            .constant_pool
            .name_and_type(*name_and_type_index)
            .unwrap(),
        ("<init>", "(Lexample/Counter;)V")
    );
}

#[test]
fn factory_result_argument_emits_six_identical_artifacts_from_independent_sessions() {
    let first = emit_paired_jvm_tasty_batch(
        FACTORY_RESULT_BATCH,
        "CounterOwnerFactory.scala",
        SourceId::from_index(18351),
    )
    .unwrap();
    let second = emit_paired_jvm_tasty_batch(
        FACTORY_RESULT_BATCH,
        "CounterOwnerFactory.scala",
        SourceId::from_index(18352),
    )
    .unwrap();
    assert_ne!(first.source_id(), second.source_id());
    assert_eq!(first.classes().len(), 3);
    assert_eq!(second.classes().len(), 3);

    let expected = [
        ("example/Counter.class", "example/Counter.tasty"),
        ("example/CounterOwner.class", "example/CounterOwner.tasty"),
        (
            "example/CounterOwnerFactory.class",
            "example/CounterOwnerFactory.tasty",
        ),
    ];
    for ((artifact, repeat), paths) in first.classes().iter().zip(second.classes()).zip(expected) {
        assert_eq!((artifact.classfile_path(), artifact.tasty_path()), paths);
        assert_eq!(artifact.source_id(), first.source_id());
        assert_eq!(artifact.classfile_path(), repeat.classfile_path());
        assert_eq!(artifact.tasty_path(), repeat.tasty_path());
        assert_eq!(artifact.classfile_bytes(), repeat.classfile_bytes());
        assert_eq!(artifact.tasty_bytes(), repeat.tasty_bytes());
        assert_eq!(artifact.tasty_link_id(), repeat.tasty_link_id());

        let tasty = TastyFile::parse_and_validate_scala_3_9(artifact.tasty_bytes()).unwrap();
        assert_eq!(
            (
                tasty.header().major_version,
                tasty.header().minor_version,
                tasty.header().experimental_version,
            ),
            (28, 9, 0)
        );
        assert_eq!(
            artifact.tasty_link_id(),
            TastyLinkId::from_bytes(tasty.header().uuid)
        );
        let mut reader = Reader::new(artifact.classfile_bytes());
        let class = ClassFile::decode(&mut reader).unwrap();
        assert!(reader.is_at_end());
        assert_eq!((class.version.major, class.version.minor), (65, 0));
        assert_eq!(
            class.tasty_link_id().unwrap(),
            Some(artifact.tasty_link_id())
        );
        validate_classfile_tasty_link(&class, artifact.tasty_bytes()).unwrap();
    }

    assert_eq!(
        first
            .classes()
            .iter()
            .flat_map(|artifact| [artifact.classfile_path(), artifact.tasty_path()])
            .collect::<Vec<_>>(),
        [
            "example/Counter.class",
            "example/Counter.tasty",
            "example/CounterOwner.class",
            "example/CounterOwner.tasty",
            "example/CounterOwnerFactory.class",
            "example/CounterOwnerFactory.tasty",
        ]
    );

    let mut reader = Reader::new(first.classes()[2].classfile_bytes());
    let factory = ClassFile::decode(&mut reader).unwrap();
    assert!(reader.is_at_end());
    let method_descriptor = |name: &str| {
        let method = factory
            .methods
            .iter()
            .find(|method| factory.constant_pool.utf8(method.name_index).unwrap() == name)
            .unwrap();
        factory
            .constant_pool
            .utf8(method.descriptor_index)
            .unwrap()
            .to_owned()
    };
    assert_eq!(method_descriptor("makeCounter"), "(I)Lexample/Counter;");
    assert_eq!(method_descriptor("create"), "(I)Lexample/CounterOwner;");
}

#[test]
fn factory_result_batch_contract_rejects_corrupt_create_bytecode_before_publication() {
    let batch = emit_paired_jvm_tasty_batch(
        FACTORY_RESULT_BATCH,
        "CounterOwnerFactory.scala",
        SourceId::from_index(18353),
    )
    .unwrap();
    let base = TestDirectory::new("factory-result-contract-rejections");

    with_paired_jvm_tasty_batch_plan(
        FACTORY_RESULT_BATCH,
        "CounterOwnerFactory.scala",
        SourceId::from_index(18354),
        |plan| {
            let tasty_plans = plan
                .classes
                .iter()
                .map(|class| class.tasty_plan)
                .collect::<Vec<_>>();
            let validated = validate_jvm_tasty_batch(plan.jvm_unit, &tasty_plans).unwrap();
            let valid_artifacts = JvmTastyBatchArtifacts {
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
            };
            validate_jvm_tasty_batch_artifacts(&validated, valid_artifacts.clone()).unwrap();

            let factory = valid_artifacts
                .classfiles
                .iter()
                .find(|artifact| artifact.classfile_path == "example/CounterOwnerFactory.class")
                .unwrap();
            let mut reader = Reader::new(&factory.classfile_bytes);
            let decoded = ClassFile::decode(&mut reader).unwrap();
            let create = decoded
                .methods
                .iter()
                .find(|method| decoded.constant_pool.utf8(method.name_index).unwrap() == "create")
                .unwrap();
            let [Attribute::Code(code)] = create.attributes.as_slice() else {
                panic!("Factory.create has one Code attribute");
            };
            let code_start =
                code.code.as_ptr() as usize - factory.classfile_bytes.as_ptr() as usize;
            let (factory_operand, constructor_operand) =
                factory_result_call_operand_offsets(code.code).unwrap();
            let make_counter = decoded
                .methods
                .iter()
                .find(|method| {
                    decoded.constant_pool.utf8(method.name_index).unwrap() == "makeCounter"
                })
                .unwrap();
            let [Attribute::Code(helper_code)] = make_counter.attributes.as_slice() else {
                panic!("Factory.makeCounter has one Code attribute");
            };
            let helper_code_start =
                helper_code.code.as_ptr() as usize - factory.classfile_bytes.as_ptr() as usize;
            let counter_constructor_operand = constructor_operand_offset(helper_code.code).unwrap();

            let mut cases = Vec::new();
            let mut missing_invoke = valid_artifacts.clone();
            factory_classfile_mut(&mut missing_invoke).classfile_bytes
                [code_start + factory_operand - 1] = 0x00;
            cases.push(("missing-factory-invoke", missing_invoke));

            let mut wrong_factory = valid_artifacts.clone();
            let classfile = &mut factory_classfile_mut(&mut wrong_factory).classfile_bytes;
            classfile[code_start + factory_operand..code_start + factory_operand + 2]
                .copy_from_slice(
                    &factory.classfile_bytes[helper_code_start + counter_constructor_operand
                        ..helper_code_start + counter_constructor_operand + 2],
                );
            cases.push(("wrong-factory-owner-result", wrong_factory));

            let mut wrong_constructor = valid_artifacts.clone();
            let classfile = &mut factory_classfile_mut(&mut wrong_constructor).classfile_bytes;
            classfile[code_start + constructor_operand..code_start + constructor_operand + 2]
                .copy_from_slice(
                    &factory.classfile_bytes
                        [code_start + factory_operand..code_start + factory_operand + 2],
                );
            cases.push(("wrong-owner-constructor", wrong_constructor));

            let mut extra_invoke = valid_artifacts.clone();
            let classfile = factory_classfile_mut(&mut extra_invoke);
            let old_code_length = u32::from_be_bytes(
                classfile.classfile_bytes[code_start - 4..code_start]
                    .try_into()
                    .unwrap(),
            );
            let attribute_length = u32::from_be_bytes(
                classfile.classfile_bytes[code_start - 12..code_start - 8]
                    .try_into()
                    .unwrap(),
            );
            classfile.classfile_bytes.splice(
                code_start + constructor_operand - 1..code_start + constructor_operand - 1,
                [
                    0xb6,
                    factory.classfile_bytes[factory_operand + code_start],
                    factory.classfile_bytes[factory_operand + code_start + 1],
                ],
            );
            classfile.classfile_bytes[code_start - 4..code_start]
                .copy_from_slice(&(old_code_length + 3).to_be_bytes());
            classfile.classfile_bytes[code_start - 12..code_start - 8]
                .copy_from_slice(&(attribute_length + 3).to_be_bytes());
            cases.push(("additional-factory-invoke", extra_invoke));

            let mut missing_create = valid_artifacts.clone();
            let raw = factory_classfile_mut(&mut missing_create)
                .classfile_bytes
                .clone();
            let mut reader = Reader::new(&raw);
            let mut decoded = ClassFile::decode(&mut reader).unwrap();
            decoded.methods.retain(|method| {
                decoded.constant_pool.utf8(method.name_index).unwrap() != "create"
            });
            factory_classfile_mut(&mut missing_create).classfile_bytes = decoded.encode().unwrap();
            cases.push(("missing-create-method", missing_create));

            let mut wrong_result = valid_artifacts.clone();
            let raw = factory_classfile_mut(&mut wrong_result)
                .classfile_bytes
                .clone();
            let mut reader = Reader::new(&raw);
            let mut decoded = ClassFile::decode(&mut reader).unwrap();
            let create_descriptor = decoded
                .methods
                .iter()
                .find(|method| decoded.constant_pool.utf8(method.name_index).unwrap() == "create")
                .unwrap()
                .descriptor_index;
            let make_counter = decoded
                .methods
                .iter_mut()
                .find(|method| {
                    decoded.constant_pool.utf8(method.name_index).unwrap() == "makeCounter"
                })
                .unwrap();
            make_counter.descriptor_index = create_descriptor;
            factory_classfile_mut(&mut wrong_result).classfile_bytes = decoded.encode().unwrap();
            cases.push(("wrong-factory-result-class", wrong_result));

            for (label, corrupt) in cases {
                let error = validate_jvm_tasty_batch_artifacts(&validated, corrupt)
                    .expect_err("corrupt create bytecode must fail whole-batch validation");
                let expected_phase = match label {
                    "missing-create-method" | "wrong-factory-result-class" => "member ABI",
                    _ => "cross-class invocation",
                };
                assert!(
                    matches!(
                        error,
                        JvmTastyContractError::BatchArtifact {
                            class_index: 2,
                            phase,
                            ..
                        } if phase == expected_phase
                    ),
                    "{label} must fail at {expected_phase}: {error:?}"
                );
                assert!(
                    !base.path().join(label).exists(),
                    "a rejected batch has no accepted publication destination"
                );
            }

            let mut truncated = valid_artifacts;
            let classfile = &mut factory_classfile_mut(&mut truncated).classfile_bytes;
            classfile.truncate(code_start + factory_operand + 1);
            assert!(matches!(
                validate_jvm_tasty_batch_artifacts(&validated, truncated),
                Err(JvmTastyContractError::BatchArtifact {
                    class_index: 2,
                    phase: "class-file decode",
                    ..
                })
            ));
        },
    )
    .unwrap();
}

#[test]
fn factory_result_constructor_arguments_preserve_original_identity_on_jdk21() {
    if !require_exact_jdk_21_or_skip() {
        return;
    }

    let batch = emit_paired_jvm_tasty_batch(
        FACTORY_RESULT_BATCH,
        "CounterOwnerFactory.scala",
        SourceId::from_index(18360),
    )
    .expect("the unchanged factory-result source emits a complete Rust-owned batch");
    assert_eq!(batch.classes().len(), 3);

    let output = TestDirectory::new("factory-result-original-jdk21-runtime");
    let class_root = output.path().join("rust-classes");
    let harness_root = output.path().join("harness-classes");
    std::fs::create_dir_all(&class_root).unwrap();
    std::fs::create_dir_all(&harness_root).unwrap();
    let expected_paths = [
        ("example/Counter.class", "example/Counter.tasty"),
        ("example/CounterOwner.class", "example/CounterOwner.tasty"),
        (
            "example/CounterOwnerFactory.class",
            "example/CounterOwnerFactory.tasty",
        ),
    ];
    let mut original_files = Vec::with_capacity(6);
    for (artifact, (class_path, tasty_path)) in batch.classes().iter().zip(expected_paths) {
        assert_eq!(
            (artifact.classfile_path(), artifact.tasty_path()),
            (class_path, tasty_path)
        );
        for (relative, bytes) in [
            (artifact.classfile_path(), artifact.classfile_bytes()),
            (artifact.tasty_path(), artifact.tasty_bytes()),
        ] {
            let path = class_root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, bytes).unwrap();
            original_files.push((path, bytes.to_vec()));
        }
        let mut reader = Reader::new(artifact.classfile_bytes());
        let class = ClassFile::decode(&mut reader).unwrap();
        assert!(reader.is_at_end());
        assert_eq!((class.version.major, class.version.minor), (65, 0));
        let tasty = TastyFile::parse_and_validate_scala_3_9(artifact.tasty_bytes()).unwrap();
        assert_eq!(
            (
                tasty.header().major_version,
                tasty.header().minor_version,
                tasty.header().experimental_version
            ),
            (28, 9, 0)
        );
        assert_eq!(
            class.tasty_link_id().unwrap(),
            Some(TastyLinkId::from_bytes(tasty.header().uuid))
        );
    }
    assert_eq!(
        read_relative_files(&class_root).unwrap(),
        [
            "example/Counter.class",
            "example/Counter.tasty",
            "example/CounterOwner.class",
            "example/CounterOwner.tasty",
            "example/CounterOwnerFactory.class",
            "example/CounterOwnerFactory.tasty",
        ]
    );
    let original_hashes = original_files
        .iter()
        .map(|(path, _)| sha256sum(path))
        .collect::<Vec<_>>();
    for ((path, _), hash) in original_files.iter().zip(&original_hashes) {
        eprintln!("factory-result-original-sha256 {} {hash}", path.display());
    }

    let factory_artifact = &batch.classes()[2];
    let mut reader = Reader::new(factory_artifact.classfile_bytes());
    let factory = ClassFile::decode(&mut reader).unwrap();
    assert!(reader.is_at_end());
    let method = |name: &str, descriptor: &str| {
        factory
            .methods
            .iter()
            .find(|method| {
                factory.constant_pool.utf8(method.name_index) == Ok(name)
                    && factory.constant_pool.utf8(method.descriptor_index) == Ok(descriptor)
            })
            .unwrap_or_else(|| panic!("missing {name}{descriptor}"))
    };
    let create = method("create", "(I)Lexample/CounterOwner;");
    let [Attribute::Code(create_code)] = create.attributes.as_slice() else {
        panic!("Factory.create has one Code attribute");
    };
    assert_eq!(create_code.max_locals, 2);
    assert_eq!(create_code.max_stack, 4);
    assert_eq!(create_code.code.first(), Some(&0xbb));
    let owner_allocation = ConstantPoolIndex(u16::from_be_bytes([
        create_code.code[1],
        create_code.code[2],
    ]));
    assert_eq!(
        factory.constant_pool.class_name(owner_allocation).unwrap(),
        "example/CounterOwner",
        "create starts by allocating only the outer Owner"
    );
    let (factory_operand, owner_operand) =
        factory_result_call_operand_offsets(create_code.code).expect(
            "create consists of one Owner allocation, seed-preserving helper call, Owner constructor and return",
        );
    let methodref = |operand: usize| {
        let index = ConstantPoolIndex(u16::from_be_bytes([
            create_code.code[operand],
            create_code.code[operand + 1],
        ]));
        let ConstantPoolEntry::Methodref {
            class_index,
            name_and_type_index,
        } = factory.constant_pool.get(index).unwrap()
        else {
            panic!("invoke instruction has a CONSTANT_Methodref");
        };
        (
            factory
                .constant_pool
                .class_name(*class_index)
                .unwrap()
                .to_owned(),
            factory
                .constant_pool
                .name_and_type(*name_and_type_index)
                .unwrap(),
        )
    };
    assert_eq!(
        methodref(factory_operand),
        (
            "example/CounterOwnerFactory".to_owned(),
            ("makeCounter", "(I)Lexample/Counter;")
        )
    );
    assert_eq!(
        methodref(owner_operand),
        (
            "example/CounterOwner".to_owned(),
            ("<init>", "(Lexample/Counter;)V")
        )
    );
    assert_eq!(
        create_code
            .code
            .iter()
            .filter(|opcode| **opcode == 0xb6)
            .count(),
        1
    );

    let cp_operand = |index: usize| {
        ConstantPoolIndexOperand::try_from(u16::from_be_bytes([
            create_code.code[index],
            create_code.code[index + 1],
        ]))
        .unwrap()
    };
    let mut create_sequence = CodeSequence::new();
    create_sequence
        .instruction(Instruction::New(ClassReference::new(cp_operand(1))))
        .instruction(Instruction::Dup)
        .instruction(Instruction::ALoad(LocalIndex::new(0)))
        .instruction(Instruction::ILoad(LocalIndex::new(1)))
        .instruction(Instruction::InvokeVirtual(VirtualMethodReference::new(
            cp_operand(factory_operand),
        )))
        .instruction(Instruction::InvokeSpecial(SpecialMethodReference::new(
            cp_operand(owner_operand),
        )))
        .instruction(Instruction::AReturn);
    let graph = ControlFlowGraph::build(&create_sequence).unwrap();
    let analyzed = CfgCodeAnalyzer::analyze_with_current_class(
        &graph,
        "create",
        "(I)Lexample/CounterOwner;",
        false,
        &factory.constant_pool,
        factory.this_class,
    )
    .expect("class-aware CFG analysis accepts the initialized helper result below a pending Owner");
    assert_eq!(analyzed.max_stack, create_code.max_stack);
    assert_eq!(analyzed.max_locals, create_code.max_locals);
    assert_eq!(
        create_code
            .code
            .iter()
            .filter(|opcode| **opcode == 0xb7)
            .count(),
        1
    );

    let make_counter = method("makeCounter", "(I)Lexample/Counter;");
    let [Attribute::Code(helper_code)] = make_counter.attributes.as_slice() else {
        panic!("Factory.makeCounter has one Code attribute");
    };
    let helper_constructor = constructor_operand_offset(helper_code.code).unwrap();
    let helper_ref = ConstantPoolIndex(u16::from_be_bytes([
        helper_code.code[helper_constructor],
        helper_code.code[helper_constructor + 1],
    ]));
    let ConstantPoolEntry::Methodref {
        class_index,
        name_and_type_index,
    } = factory.constant_pool.get(helper_ref).unwrap()
    else {
        panic!("makeCounter invokes the Counter constructor");
    };
    assert_eq!(
        factory.constant_pool.class_name(*class_index).unwrap(),
        "example/Counter"
    );
    assert_eq!(
        factory
            .constant_pool
            .name_and_type(*name_and_type_index)
            .unwrap(),
        ("<init>", "(I)V")
    );
    assert_eq!(
        helper_code
            .code
            .iter()
            .filter(|opcode| **opcode == 0xbb)
            .count(),
        1
    );
    assert_eq!(
        create_code
            .code
            .iter()
            .filter(|opcode| **opcode == 0xbb)
            .count(),
        1,
        "create allocates only the still-uninitialized outer Owner"
    );

    let mut owner_reader = Reader::new(batch.classes()[1].classfile_bytes());
    let owner = ClassFile::decode(&mut owner_reader).unwrap();
    assert!(owner_reader.is_at_end());
    let inner = owner
        .fields
        .iter()
        .find(|field| {
            owner.constant_pool.utf8(field.name_index) == Ok("inner")
                && owner.constant_pool.utf8(field.descriptor_index) == Ok("Lexample/Counter;")
        })
        .expect("CounterOwner has its private Counter field");
    assert!(
        inner
            .access_flags
            .contains(dotty_classfile::access_flags::ACC_PRIVATE)
    );
    let owner_ctor = owner
        .methods
        .iter()
        .find(|method| {
            owner.constant_pool.utf8(method.name_index) == Ok("<init>")
                && owner.constant_pool.utf8(method.descriptor_index) == Ok("(Lexample/Counter;)V")
        })
        .unwrap();
    let [Attribute::Code(owner_ctor_code)] = owner_ctor.attributes.as_slice() else {
        panic!("CounterOwner.<init> has one Code attribute");
    };
    assert_eq!(
        owner_ctor_code
            .code
            .iter()
            .filter(|opcode| **opcode == 0xb5)
            .count(),
        1,
        "the exact initialized constructor argument is stored once"
    );
    let owner_field_operand = owner_ctor_code
        .code
        .windows(3)
        .position(|instruction| instruction[0] == 0xb5)
        .map(|offset| {
            ConstantPoolIndex(u16::from_be_bytes([
                owner_ctor_code.code[offset + 1],
                owner_ctor_code.code[offset + 2],
            ]))
        })
        .expect("Owner constructor has a putfield instruction");
    let ConstantPoolEntry::Fieldref {
        class_index,
        name_and_type_index,
    } = owner.constant_pool.get(owner_field_operand).unwrap()
    else {
        panic!("Owner constructor putfield uses a Fieldref");
    };
    assert_eq!(
        owner.constant_pool.class_name(*class_index).unwrap(),
        "example/CounterOwner"
    );
    assert_eq!(
        owner
            .constant_pool
            .name_and_type(*name_and_type_index)
            .unwrap(),
        ("inner", "Lexample/Counter;")
    );
    assert_eq!(
        owner_ctor_code
            .code
            .iter()
            .filter(|opcode| **opcode == 0xbb)
            .count(),
        0,
        "Owner stores its constructor parameter rather than constructing a replacement Counter"
    );

    for (class_name, expected) in [
        (
            "example.CounterOwnerFactory",
            &[
                "major version: 65",
                "descriptor: (I)Lexample/Counter;",
                "descriptor: (I)Lexample/CounterOwner;",
                "Method makeCounter:(I)Lexample/Counter;",
                "Method example/CounterOwner.\"<init>\":(Lexample/Counter;)V",
            ][..],
        ),
        (
            "example.CounterOwner",
            &[
                "major version: 65",
                "private final example.Counter inner;",
                "Field inner:Lexample/Counter;",
            ][..],
        ),
    ] {
        let javap = Command::new("javap")
            .args(["-v", "-p", "-c", "-classpath"])
            .arg(&class_root)
            .arg(class_name)
            .output()
            .expect("start JDK 21 javap for the original Rust classfile");
        assert!(
            javap.status.success(),
            "javap rejected {class_name}: {}",
            String::from_utf8_lossy(&javap.stderr)
        );
        let dump = String::from_utf8_lossy(&javap.stdout);
        for value in expected {
            assert!(
                dump.contains(value),
                "javap {class_name} omitted {value:?}:\n{dump}"
            );
        }
    }

    let harness = output.path().join("FactoryResultIdentityHarness.java");
    std::fs::write(&harness, r#"import java.lang.reflect.Field;

public final class FactoryResultIdentityHarness {
    public static void main(String[] args) throws ReflectiveOperationException {
        example.CounterOwnerFactory factory = new example.CounterOwnerFactory();
        example.CounterOwner positive = factory.create(7);
        example.CounterOwner negative = factory.create(-9);
        example.CounterOwner third = factory.create(3);
        example.CounterOwner independent = new example.CounterOwnerFactory().create(11);
        if (positive == negative || positive == third || negative == third || positive == independent)
            throw new AssertionError("create must return distinct Owners");
        example.Counter p = inner(positive), n = inner(negative), t = inner(third), i = inner(independent);
        if (p == n || p == t || p == i || n == t || n == i || t == i)
            throw new AssertionError("each Owner must retain a distinct helper-created Counter");
        expect(positive.read(), 7); expect(negative.read(), -9);
        expect(third.read(), 3); expect(independent.read(), 11);
        p.set(42);
        expect(positive.read(), 42); expect(negative.read(), -9);
        expect(third.read(), 3); expect(independent.read(), 11);
        p.set(-9);
        expect(positive.read(), -9); expect(negative.read(), -9);
        expect(third.read(), 3); expect(independent.read(), 11);
        n.set(100);
        expect(positive.read(), -9); expect(negative.read(), 100);
        expect(third.read(), 3); expect(independent.read(), 11);
        System.out.println("factory-result=7,-9,3,11; distinct=true; mutation=42,-9,100");
    }
    private static example.Counter inner(example.CounterOwner owner) throws ReflectiveOperationException {
        Field field = example.CounterOwner.class.getDeclaredField("inner");
        field.setAccessible(true);
        Object value = field.get(owner);
        if (!(value instanceof example.Counter)) throw new AssertionError("inner is not a Counter");
        return (example.Counter)value;
    }
    private static void expect(int actual, int expected) {
        if (actual != expected) throw new AssertionError("expected " + expected + ", got " + actual);
    }
}
"#).unwrap();
    let javac = Command::new("javac")
        .args(["--release", "21", "-classpath"])
        .arg(&class_root)
        .arg("-d")
        .arg(&harness_root)
        .arg(&harness)
        .output()
        .expect("start JDK 21 javac for the separate Java harness");
    assert!(
        javac.status.success(),
        "javac --release 21 rejected the harness:\n{}",
        String::from_utf8_lossy(&javac.stderr)
    );
    for ((path, _), expected_hash) in original_files.iter().zip(&original_hashes) {
        assert_eq!(
            sha256sum(path),
            *expected_hash,
            "javac changed an original Rust artifact"
        );
    }
    let classpath = std::env::join_paths([harness_root.as_path(), class_root.as_path()]).unwrap();
    let java = Command::new("java")
        .args(["-Xverify:all", "-ea", "-classpath"])
        .arg(&classpath)
        .arg("FactoryResultIdentityHarness")
        .output()
        .expect("start strict JDK 21 verification and Rust artifact runtime");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected or failed original Rust artifacts:\n{}\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "factory-result=7,-9,3,11; distinct=true; mutation=42,-9,100"
    );
    for ((path, bytes), expected_hash) in original_files.iter().zip(&original_hashes) {
        assert_eq!(
            sha256sum(path),
            *expected_hash,
            "JDK tools changed an original artifact"
        );
        assert_eq!(
            std::fs::read(path).unwrap(),
            *bytes,
            "runtime used unchanged Rust bytes"
        );
    }

    // Each runtime negative starts from the valid original class bytes in an isolated tree.
    let run_negative = |label: &str, factory_bytes: &[u8], expected: &str| {
        let root = output.path().join(label);
        for (index, artifact) in batch.classes().iter().enumerate() {
            let relative = artifact.classfile_path();
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let bytes = if index == 2 {
                factory_bytes
            } else {
                artifact.classfile_bytes()
            };
            std::fs::write(path, bytes).unwrap();
        }
        let classpath = std::env::join_paths([harness_root.as_path(), root.as_path()]).unwrap();
        let rejected = Command::new("java")
            .args(["-Xverify:all", "-ea", "-classpath"])
            .arg(classpath)
            .arg("FactoryResultIdentityHarness")
            .output()
            .expect("start isolated JVM negative control");
        assert!(!rejected.status.success(), "JVM accepted {label}");
        let message = format!(
            "{}{}",
            String::from_utf8_lossy(&rejected.stdout),
            String::from_utf8_lossy(&rejected.stderr)
        );
        assert!(
            message.contains(expected),
            "{label} failed at an unexpected first stage; expected {expected}:\n{message}"
        );
    };

    let code_start =
        create_code.code.as_ptr() as usize - factory_artifact.classfile_bytes().as_ptr() as usize;
    let mut wrong_receiver = factory_artifact.classfile_bytes().to_vec();
    // In `create`, replace `aload_0` with `iload_1`; the invokevirtual receiver is then an int.
    wrong_receiver[code_start + factory_operand - 3] = 0x1b;
    run_negative("wrong-reference-flow", &wrong_receiver, "VerifyError");

    let mut uninitialized_owner = factory_artifact.classfile_bytes().to_vec();
    uninitialized_owner[code_start + owner_operand - 1..code_start + owner_operand + 2]
        .copy_from_slice(&[0x57, 0x57, 0x00]);
    run_negative(
        "uninitialized-owner-return",
        &uninitialized_owner,
        "VerifyError",
    );

    let mut undersized_stack = factory_artifact.classfile_bytes().to_vec();
    undersized_stack[code_start - 8..code_start - 6].copy_from_slice(&3u16.to_be_bytes());
    run_negative("undersized-max-stack", &undersized_stack, "VerifyError");

    let mut missing_helper =
        ClassFile::decode(&mut Reader::new(factory_artifact.classfile_bytes())).unwrap();
    missing_helper.methods.retain(|method| {
        missing_helper
            .constant_pool
            .utf8(method.name_index)
            .unwrap()
            != "makeCounter"
    });
    let missing_helper = missing_helper.encode().unwrap();
    run_negative(
        "missing-helper-linkage",
        &missing_helper,
        "NoSuchMethodError",
    );
}

fn factory_classfile_mut(artifacts: &mut JvmTastyBatchArtifacts) -> &mut JvmTastyClassFileArtifact {
    artifacts
        .classfiles
        .iter_mut()
        .find(|artifact| artifact.classfile_path == "example/CounterOwnerFactory.class")
        .unwrap()
}

fn factory_result_call_operand_offsets(code: &[u8]) -> Option<(usize, usize)> {
    let mut pc = 0;
    for opcode in [0xbb, 0x59, 0x2a, 0x1b] {
        if code.get(pc) != Some(&opcode) {
            return None;
        }
        pc += 1;
        if opcode == 0xbb {
            pc = pc.checked_add(2)?;
        }
    }
    if code.get(pc) != Some(&0xb6) {
        return None;
    }
    let factory_operand = pc + 1;
    pc += 3;
    if code.get(pc) != Some(&0xb7) {
        return None;
    }
    let constructor_operand = pc + 1;
    pc += 3;
    if code.get(pc) != Some(&0xb0) || pc + 1 != code.len() {
        return None;
    }
    Some((factory_operand, constructor_operand))
}

fn constructor_operand_offset(code: &[u8]) -> Option<usize> {
    let mut pc = 0;
    if code.get(pc) != Some(&0xbb) {
        return None;
    }
    pc += 3;
    if code.get(pc) != Some(&0x59) {
        return None;
    }
    pc += 1;
    if code.get(pc) != Some(&0x1b) {
        return None;
    }
    pc += 1;
    if code.get(pc) != Some(&0xb7) {
        return None;
    }
    Some(pc + 1)
}

#[test]
fn nested_constructor_factory_emits_six_linked_artifacts_from_one_session() {
    let first = emit_paired_jvm_tasty_batch(
        NESTED_CONSTRUCTOR_BATCH,
        "NestedConstructorCalls.scala",
        SourceId::from_index(18181),
    )
    .unwrap();
    let second = emit_paired_jvm_tasty_batch(
        NESTED_CONSTRUCTOR_BATCH,
        "NestedConstructorCalls.scala",
        SourceId::from_index(18182),
    )
    .unwrap();
    assert_ne!(first.source_id(), second.source_id());
    assert_eq!(first.classes().len(), 3);
    assert_eq!(second.classes().len(), 3);

    let expected = [
        ("example/Counter.class", "example/Counter.tasty"),
        ("example/CounterOwner.class", "example/CounterOwner.tasty"),
        (
            "example/CounterOwnerFactory.class",
            "example/CounterOwnerFactory.tasty",
        ),
    ];
    for (index, (artifact, repeat)) in first.classes().iter().zip(second.classes()).enumerate() {
        assert_eq!(
            (artifact.classfile_path(), artifact.tasty_path()),
            expected[index]
        );
        assert_eq!(artifact.source_id(), first.source_id());
        assert_eq!(artifact.classfile_bytes(), repeat.classfile_bytes());
        assert_eq!(artifact.tasty_bytes(), repeat.tasty_bytes());
        assert_eq!(artifact.tasty_link_id(), repeat.tasty_link_id());
        let mut reader = Reader::new(artifact.classfile_bytes());
        let class = ClassFile::decode(&mut reader).unwrap();
        assert!(reader.is_at_end());
        assert_eq!((class.version.major, class.version.minor), (65, 0));
        assert_eq!(
            class.tasty_link_id().unwrap(),
            Some(artifact.tasty_link_id())
        );
        let tasty = TastyFile::parse_and_validate_scala_3_9(artifact.tasty_bytes()).unwrap();
        assert_eq!(
            (
                tasty.header().major_version,
                tasty.header().minor_version,
                tasty.header().experimental_version,
            ),
            (28, 9, 0)
        );
        validate_classfile_tasty_link(&class, artifact.tasty_bytes()).unwrap();
    }

    let mut reader = Reader::new(first.classes()[2].classfile_bytes());
    let factory = ClassFile::decode(&mut reader).unwrap();
    let create = factory
        .methods
        .iter()
        .find(|method| factory.constant_pool.utf8(method.name_index).unwrap() == "create")
        .unwrap();
    assert_eq!(
        factory.constant_pool.utf8(create.descriptor_index).unwrap(),
        "(I)Lexample/CounterOwner;"
    );
    let [Attribute::Code(code)] = create.attributes.as_slice() else {
        panic!("Factory.create has one Code attribute");
    };
    assert_eq!(code.code.len(), 16);
    assert_eq!(
        [
            code.code[0],
            code.code[3],
            code.code[4],
            code.code[7],
            code.code[8]
        ],
        [0xbb, 0x59, 0xbb, 0x59, 0x1b]
    );
    assert_eq!(
        (code.code[9], code.code[12], code.code[15]),
        (0xb7, 0xb7, 0xb0)
    );
    assert_eq!(code.max_stack, 5);
    assert_eq!(code.max_locals, 2);
    let method_ref = |offset: usize| {
        let index = ConstantPoolIndex(u16::from_be_bytes([
            code.code[offset],
            code.code[offset + 1],
        ]));
        let ConstantPoolEntry::Methodref {
            class_index,
            name_and_type_index,
        } = factory.constant_pool.get(index).unwrap()
        else {
            panic!("constructor invocation references a Methodref");
        };
        (
            factory.constant_pool.class_name(*class_index).unwrap(),
            factory
                .constant_pool
                .name_and_type(*name_and_type_index)
                .unwrap(),
        )
    };
    assert_eq!(method_ref(10), ("example/Counter", ("<init>", "(I)V")));
    assert_eq!(
        method_ref(13),
        ("example/CounterOwner", ("<init>", "(Lexample/Counter;)V"))
    );
}

#[test]
fn nested_constructor_factory_executes_original_rust_artifacts_on_jdk21() {
    let batch = emit_paired_jvm_tasty_batch(
        NESTED_CONSTRUCTOR_BATCH,
        "NestedConstructorCalls.scala",
        SourceId::from_index(18189),
    )
    .unwrap();
    assert_eq!(batch.classes().len(), 3);

    let output = TestDirectory::new("nested-constructor-jdk21-runtime");
    let class_root = output.path().join("rust-classes");
    std::fs::create_dir_all(&class_root).unwrap();
    let mut artifact_paths = Vec::with_capacity(6);
    let mut original_hashes = Vec::with_capacity(6);
    for artifact in batch.classes() {
        let classfile = class_root.join(artifact.classfile_path());
        let tasty = class_root.join(artifact.tasty_path());
        std::fs::create_dir_all(classfile.parent().unwrap()).unwrap();
        std::fs::write(&classfile, artifact.classfile_bytes()).unwrap();
        std::fs::write(&tasty, artifact.tasty_bytes()).unwrap();
        artifact_paths.extend([classfile, tasty]);
    }
    for path in &artifact_paths {
        let hash = sha256sum(path);
        eprintln!(
            "nested-constructor-original-sha256 {} {hash}",
            path.display()
        );
        original_hashes.push(hash);
    }
    if !require_exact_jdk_21_or_skip() {
        return;
    }

    let factory = &batch.classes()[2];
    let mut reader = Reader::new(factory.classfile_bytes());
    let factory_class = ClassFile::decode(&mut reader).unwrap();
    assert!(reader.is_at_end());
    assert_eq!(
        (factory_class.version.major, factory_class.version.minor),
        (65, 0)
    );
    let create = factory_class
        .methods
        .iter()
        .find(|method| factory_class.constant_pool.utf8(method.name_index).unwrap() == "create")
        .unwrap();
    assert_eq!(
        factory_class
            .constant_pool
            .utf8(create.descriptor_index)
            .unwrap(),
        "(I)Lexample/CounterOwner;"
    );
    let [Attribute::Code(code)] = create.attributes.as_slice() else {
        panic!("Factory.create has one Code attribute");
    };
    assert_eq!(code.max_stack, 5);
    assert_eq!(code.max_locals, 2);
    assert_eq!(code.code.len(), 16);
    assert_eq!(
        [
            code.code[0],
            code.code[3],
            code.code[4],
            code.code[7],
            code.code[8]
        ],
        [0xbb, 0x59, 0xbb, 0x59, 0x1b]
    );
    assert_eq!(
        (code.code[9], code.code[12], code.code[15]),
        (0xb7, 0xb7, 0xb0)
    );
    let constructor_target = |offset: usize| {
        let index = ConstantPoolIndex(u16::from_be_bytes([
            code.code[offset],
            code.code[offset + 1],
        ]));
        let ConstantPoolEntry::Methodref {
            class_index,
            name_and_type_index,
        } = factory_class.constant_pool.get(index).unwrap()
        else {
            panic!("constructor call uses a Methodref");
        };
        (
            factory_class
                .constant_pool
                .class_name(*class_index)
                .unwrap(),
            factory_class
                .constant_pool
                .name_and_type(*name_and_type_index)
                .unwrap(),
        )
    };
    assert_eq!(
        constructor_target(10),
        ("example/Counter", ("<init>", "(I)V"))
    );
    assert_eq!(
        constructor_target(13),
        ("example/CounterOwner", ("<init>", "(Lexample/Counter;)V"))
    );
    let pool_operand = |offset: usize| {
        ConstantPoolIndexOperand::try_from(u16::from_be_bytes([
            code.code[offset],
            code.code[offset + 1],
        ]))
        .unwrap()
    };
    let analysis = analyze_instructions(
        "(I)Lexample/CounterOwner;",
        false,
        &[
            Instruction::New(ClassReference::new(pool_operand(1))),
            Instruction::Dup,
            Instruction::New(ClassReference::new(pool_operand(5))),
            Instruction::Dup,
            Instruction::ILoad(LocalIndex::new(1)),
            Instruction::InvokeSpecial(SpecialMethodReference::new(pool_operand(10))),
            Instruction::InvokeSpecial(SpecialMethodReference::new(pool_operand(13))),
            Instruction::AReturn,
        ],
        &factory_class.constant_pool,
    )
    .expect("Rust classfile analyzer accepts the decoded nested constructor sequence");
    assert_eq!(analysis.max_stack, 5);
    assert_eq!(analysis.max_locals, 2);

    let mut reader = Reader::new(batch.classes()[1].classfile_bytes());
    let owner_class = ClassFile::decode(&mut reader).unwrap();
    assert!(reader.is_at_end());
    assert_eq!(
        (owner_class.version.major, owner_class.version.minor),
        (65, 0)
    );
    assert_eq!(owner_class.fields.len(), 1);
    let inner = &owner_class.fields[0];
    assert_eq!(
        owner_class.constant_pool.utf8(inner.name_index).unwrap(),
        "inner"
    );
    assert_eq!(
        owner_class
            .constant_pool
            .utf8(inner.descriptor_index)
            .unwrap(),
        "Lexample/Counter;"
    );
    assert!(
        inner
            .access_flags
            .contains(dotty_classfile::access_flags::ACC_PRIVATE)
    );
    assert!(
        inner
            .access_flags
            .contains(dotty_classfile::access_flags::ACC_FINAL)
    );
    assert!(owner_class.methods.iter().any(|method| {
        owner_class.constant_pool.utf8(method.name_index) == Ok("<init>")
            && owner_class.constant_pool.utf8(method.descriptor_index) == Ok("(Lexample/Counter;)V")
    }));

    let harness_root = output.path().join("harness-classes");
    std::fs::create_dir(&harness_root).unwrap();
    let harness = output.path().join("NestedConstructorIdentityHarness.java");
    std::fs::write(
        &harness,
        r#"import java.lang.reflect.Field;

public final class NestedConstructorIdentityHarness {
    public static void main(String[] args) throws ReflectiveOperationException {
        example.CounterOwnerFactory factory = new example.CounterOwnerFactory();
        example.CounterOwner positive = factory.create(7);
        example.CounterOwner negative = factory.create(-9);
        example.CounterOwner third = factory.create(3);
        if (positive == negative || positive == third || negative == third) {
            throw new AssertionError("each create must return a distinct Owner");
        }
        example.Counter positiveInner = inner(positive);
        example.Counter negativeInner = inner(negative);
        example.Counter thirdInner = inner(third);
        if (positiveInner == negativeInner || positiveInner == thirdInner || negativeInner == thirdInner) {
            throw new AssertionError("each create must allocate an independent Counter");
        }
        expect(positive.read(), 7);
        expect(negative.read(), -9);
        expect(third.read(), 3);
        positiveInner.set(42);
        expect(positive.read(), 42);
        expect(negative.read(), -9);
        expect(third.read(), 3);
        positiveInner.set(-9);
        expect(positive.read(), -9);
        expect(negative.read(), -9);
        expect(third.read(), 3);
        negativeInner.set(100);
        expect(positive.read(), -9);
        expect(negative.read(), 100);
        expect(third.read(), 3);
        System.out.println("nested-runtime=7,-9,3; independent=true; mutation=42,-9,100");
    }

    private static example.Counter inner(example.CounterOwner owner)
        throws ReflectiveOperationException {
        Field field = example.CounterOwner.class.getDeclaredField("inner");
        field.setAccessible(true);
        Object value = field.get(owner);
        if (!(value instanceof example.Counter)) {
            throw new AssertionError("Owner.inner must hold an initialized Counter");
        }
        return (example.Counter) value;
    }

    private static void expect(int actual, int expected) {
        if (actual != expected) throw new AssertionError("expected " + expected + ", got " + actual);
    }
}
"#,
    )
    .unwrap();

    let javac = Command::new("javac")
        .args([
            "-classpath",
            class_root.to_str().unwrap(),
            "-d",
            harness_root.to_str().unwrap(),
        ])
        .arg(&harness)
        .output()
        .expect("start JDK 21 javac for the original Rust classfiles");
    assert!(
        javac.status.success(),
        "javac rejected test-only harness:\n{}\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    for (path, expected_hash) in artifact_paths.iter().zip(&original_hashes) {
        assert_eq!(
            sha256sum(path),
            expected_hash.as_str(),
            "javac changed {}",
            path.display()
        );
    }

    let javap = Command::new("javap")
        .args(["-classpath", class_root.to_str().unwrap(), "-v", "-p", "-c"])
        .arg("example.CounterOwnerFactory")
        .output()
        .expect("start JDK 21 javap on the original Rust Factory classfile");
    assert!(
        javap.status.success(),
        "javap rejected original Rust Factory.class:\n{}",
        String::from_utf8_lossy(&javap.stderr)
    );
    let javap = String::from_utf8_lossy(&javap.stdout);
    for expected in [
        "major version: 65",
        "create(int)",
        "descriptor: (I)Lexample/CounterOwner;",
        "new           #",
        "example/CounterOwner",
        "example/Counter",
        "Counter.\"<init>\":(I)V",
        "CounterOwner.\"<init>\":(Lexample/Counter;)V",
        "areturn",
    ] {
        assert!(
            javap.contains(expected),
            "javap omitted {expected:?}:\n{javap}"
        );
    }

    let javap_owner = Command::new("javap")
        .args(["-classpath", class_root.to_str().unwrap(), "-v", "-p", "-c"])
        .arg("example.CounterOwner")
        .output()
        .expect("start JDK 21 javap on the original Rust Owner classfile");
    assert!(
        javap_owner.status.success(),
        "javap rejected original Rust CounterOwner.class:\n{}",
        String::from_utf8_lossy(&javap_owner.stderr)
    );
    let javap_owner = String::from_utf8_lossy(&javap_owner.stdout);
    for expected in [
        "major version: 65",
        "private final example.Counter inner;",
        "descriptor: Lexample/Counter;",
        "public example.CounterOwner(example.Counter);",
        "descriptor: (Lexample/Counter;)V",
    ] {
        assert!(
            javap_owner.contains(expected),
            "javap omitted {expected:?}:\n{javap_owner}"
        );
    }

    let classpath = std::env::join_paths([harness_root.as_path(), class_root.as_path()]).unwrap();
    let java = Command::new("java")
        .args(["-Xverify:all", "-ea", "-classpath"])
        .arg(classpath)
        .arg("NestedConstructorIdentityHarness")
        .output()
        .expect("start strict JDK 21 verification of the original Rust classfiles");
    assert!(
        java.status.success(),
        "java -Xverify:all -ea rejected or failed original Rust classfiles:\n{}\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "nested-runtime=7,-9,3; independent=true; mutation=42,-9,100"
    );

    for (path, expected_hash) in artifact_paths.iter().zip(&original_hashes) {
        assert_eq!(
            sha256sum(path),
            expected_hash.as_str(),
            "java changed {}",
            path.display()
        );
    }
}

#[test]
fn nested_constructor_factory_is_name_and_declaration_order_independent() {
    let renamed = NESTED_CONSTRUCTOR_BATCH
        .replace("package example", "package renamed")
        .replace("CounterOwnerFactory", "WidgetBuilder")
        .replace("CounterOwner", "WidgetHolder")
        .replace("Counter", "Widget");
    let reordered = NESTED_CONSTRUCTOR_BATCH
        .replace(
            "class Counter(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n\nclass CounterOwner(counter: Counter):",
            "class CounterOwner(counter: Counter):",
        )
        .replace(
            "\nclass CounterOwnerFactory:",
            "\nclass Counter(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n\nclass CounterOwnerFactory:",
        );

    for (source, source_id, expected_paths) in [
        (
            renamed.as_str(),
            18184,
            [
                ("renamed/Widget.class", "renamed/Widget.tasty"),
                ("renamed/WidgetHolder.class", "renamed/WidgetHolder.tasty"),
                ("renamed/WidgetBuilder.class", "renamed/WidgetBuilder.tasty"),
            ],
        ),
        (
            reordered.as_str(),
            18186,
            [
                ("example/CounterOwner.class", "example/CounterOwner.tasty"),
                ("example/Counter.class", "example/Counter.tasty"),
                (
                    "example/CounterOwnerFactory.class",
                    "example/CounterOwnerFactory.tasty",
                ),
            ],
        ),
    ] {
        let first = emit_paired_jvm_tasty_batch(
            source,
            "NestedConstructorCalls.scala",
            SourceId::from_index(source_id),
        )
        .unwrap();
        let second = emit_paired_jvm_tasty_batch(
            source,
            "NestedConstructorCalls.scala",
            SourceId::from_index(source_id + 1),
        )
        .unwrap();
        for ((artifact, repeat), expected_path) in first
            .classes()
            .iter()
            .zip(second.classes())
            .zip(expected_paths)
        {
            assert_eq!(
                (artifact.classfile_path(), artifact.tasty_path()),
                expected_path
            );
            assert_eq!(artifact.classfile_bytes(), repeat.classfile_bytes());
            assert_eq!(artifact.tasty_bytes(), repeat.tasty_bytes());
            assert_eq!(artifact.tasty_link_id(), repeat.tasty_link_id());
            let mut reader = Reader::new(artifact.classfile_bytes());
            let class = ClassFile::decode(&mut reader).unwrap();
            assert!(reader.is_at_end());
            validate_classfile_tasty_link(&class, artifact.tasty_bytes()).unwrap();
        }
    }
}

#[test]
fn factory_result_batch_publisher_rejects_corruption_and_all_six_write_failures() {
    let batch = emit_paired_jvm_tasty_batch(
        FACTORY_RESULT_BATCH,
        "CounterOwnerFactory.scala",
        SourceId::from_index(18355),
    )
    .expect("emit the complete Counter/Owner/Factory factory-result batch");
    let pairs = batch_publish_pairs(&batch);
    let base = TestDirectory::new("factory-result-publish-faults");

    assert_eq!(pairs.len(), 3);
    let mut missing_factory_sidecar = pairs.clone();
    missing_factory_sidecar[2].tasty_path = "";
    let missing_factory_pair = base.path().join("missing-factory-tasty");
    assert!(matches!(
        publish_batch_pairs(&missing_factory_pair, &missing_factory_sidecar, None),
        Err(BatchPublishError::InvalidManifest {
            class_index: 2,
            phase: "TASTy path",
            ..
        })
    ));
    assert!(!missing_factory_pair.exists());

    let mut missing_factory_class = pairs.clone();
    missing_factory_class[2].classfile_path = "";
    let missing_factory_class_destination = base.path().join("missing-factory-class");
    assert!(matches!(
        publish_batch_pairs(
            &missing_factory_class_destination,
            &missing_factory_class,
            None
        ),
        Err(BatchPublishError::InvalidManifest {
            class_index: 2,
            phase: "class-file path",
            ..
        })
    ));
    assert!(!missing_factory_class_destination.exists());

    let mut duplicate_factory_path = pairs.clone();
    duplicate_factory_path[2].classfile_path = pairs[1].classfile_path;
    let duplicate_factory_destination = base.path().join("duplicate-factory-path");
    assert!(matches!(
        publish_batch_pairs(
            &duplicate_factory_destination,
            &duplicate_factory_path,
            None
        ),
        Err(BatchPublishError::InvalidManifest { .. })
    ));
    assert!(!duplicate_factory_destination.exists());

    let mut unsafe_factory_path = pairs.clone();
    unsafe_factory_path[2].tasty_path = "../CounterOwnerFactory.tasty";
    let unsafe_factory_destination = base.path().join("unsafe-factory-path");
    assert!(matches!(
        publish_batch_pairs(&unsafe_factory_destination, &unsafe_factory_path, None),
        Err(BatchPublishError::InvalidManifest { .. })
    ));
    assert!(!unsafe_factory_destination.exists());

    let mut missing_owner_class = pairs.clone();
    missing_owner_class[1].classfile_path = "";
    let missing_owner_class_destination = base.path().join("missing-owner-class");
    assert!(matches!(
        publish_batch_pairs(&missing_owner_class_destination, &missing_owner_class, None),
        Err(BatchPublishError::InvalidManifest {
            class_index: 1,
            phase: "class-file path",
            ..
        })
    ));
    assert!(!missing_owner_class_destination.exists());

    let mut corrupt_factory_class = pairs.clone();
    corrupt_factory_class[2].classfile_bytes = &[];
    let corrupt_factory_class_destination = base.path().join("corrupt-factory-class");
    assert!(matches!(
        publish_batch_pairs(
            &corrupt_factory_class_destination,
            &corrupt_factory_class,
            None
        ),
        Err(BatchPublishError::InvalidManifest {
            class_index: 2,
            phase: "class-file decode",
            ..
        })
    ));
    assert!(!corrupt_factory_class_destination.exists());

    let mut corrupt_owner_class = pairs.clone();
    corrupt_owner_class[1].classfile_bytes = &[];
    let corrupt_owner_class_destination = base.path().join("corrupt-owner-class");
    assert!(matches!(
        publish_batch_pairs(&corrupt_owner_class_destination, &corrupt_owner_class, None),
        Err(BatchPublishError::InvalidManifest {
            class_index: 1,
            phase: "class-file decode",
            ..
        })
    ));
    assert!(!corrupt_owner_class_destination.exists());

    let mut forged_source = pairs.clone();
    forged_source[2].source_id = SourceId::from_index(18002);
    let forged_source_destination = base.path().join("forged-source-provenance");
    assert!(matches!(
        publish_batch_pairs(&forged_source_destination, &forged_source, None),
        Err(BatchPublishError::InvalidManifest {
            class_index: 2,
            phase: "source identity",
            ..
        })
    ));
    assert!(!forged_source_destination.exists());

    for (label, class_index) in [("owner", 1), ("factory", 2)] {
        let mut corrupt_sidecar = pairs.clone();
        corrupt_sidecar[class_index].tasty_bytes = &[];
        let destination = base.path().join(format!("corrupt-{label}-sidecar"));
        assert!(matches!(
            publish_batch_pairs(&destination, &corrupt_sidecar, None),
            Err(BatchPublishError::InvalidManifest {
                class_index: actual,
                phase: "TASTy decode",
                ..
            }) if actual == class_index
        ));
        assert!(!destination.exists());
    }

    let factory_link = batch.classes()[2].tasty_link_id();
    let mut stale_factory_class = pairs[2].classfile_bytes.to_vec();
    let link_offset = stale_factory_class
        .windows(factory_link.as_bytes().len())
        .position(|window| window == factory_link.as_bytes())
        .expect("Factory classfile contains its TASTy UUID");
    stale_factory_class[link_offset] ^= 0x80;
    let mut stale_link = pairs.clone();
    stale_link[2].classfile_bytes = &stale_factory_class;
    let stale_link_destination = base.path().join("stale-factory-link");
    assert!(matches!(
        publish_batch_pairs(&stale_link_destination, &stale_link, None),
        Err(BatchPublishError::InvalidManifest {
            class_index: 2,
            phase: "classfile/TASTy link",
            ..
        })
    ));
    assert!(!stale_link_destination.exists());

    for (index, fault) in (1..=6)
        .map(BatchPublishFault::AfterStagedFiles)
        .chain([
            BatchPublishFault::BeforePromotion,
            BatchPublishFault::AfterPromotion,
        ])
        .enumerate()
    {
        let destination = base.path().join(format!("injected-failure-{index}"));
        assert!(matches!(
            publish_paired_batch(&destination, &batch, Some(fault)),
            Err(BatchPublishError::Io { .. })
        ));
        assert!(
            !destination.exists(),
            "fault case {index} must leave no accepted partial final tree"
        );
        assert_eq!(
            std::fs::read_dir(base.path()).unwrap().count(),
            0,
            "fault case {index} must clean its private staging tree"
        );
    }

    let existing = base.path().join("existing-output");
    std::fs::create_dir(&existing).unwrap();
    let sentinel = existing.join("sentinel");
    std::fs::write(&sentinel, b"preserve unrelated destination").unwrap();
    assert!(matches!(
        publish_paired_batch(&existing, &batch, None),
        Err(BatchPublishError::DestinationExists { .. })
    ));
    assert_eq!(
        std::fs::read(&sentinel).unwrap(),
        b"preserve unrelated destination"
    );
    assert_eq!(read_relative_files(&existing).unwrap(), ["sentinel"]);
}

#[test]
fn renamed_and_reordered_reference_constructor_batches_emit_complete_links() {
    let renamed = REFERENCE_CONSTRUCTOR_BATCH
        .replace("package example", "package renamed")
        .replace("CounterOwnerFactory", "WidgetBuilder")
        .replace("CounterOwner", "WidgetHolder")
        .replace("Counter", "Widget")
        .replace("wrap(", "build(");
    let reordered = REFERENCE_CONSTRUCTOR_BATCH
        .replace(
            "class Counter(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n\nclass CounterOwner(counter: Counter):",
            "class CounterOwner(counter: Counter):",
        )
        .replace(
            "\nclass CounterOwnerFactory:",
            "\nclass Counter(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n\nclass CounterOwnerFactory:",
        );

    for (source, source_name, source_id, paths) in [
        (
            renamed.as_str(),
            "renamed-reference-constructor.scala",
            17973,
            [
                ("renamed/Widget.class", "renamed/Widget.tasty"),
                ("renamed/WidgetHolder.class", "renamed/WidgetHolder.tasty"),
                ("renamed/WidgetBuilder.class", "renamed/WidgetBuilder.tasty"),
            ],
        ),
        (
            reordered.as_str(),
            "reordered-reference-constructor.scala",
            17974,
            [
                ("example/CounterOwner.class", "example/CounterOwner.tasty"),
                ("example/Counter.class", "example/Counter.tasty"),
                (
                    "example/CounterOwnerFactory.class",
                    "example/CounterOwnerFactory.tasty",
                ),
            ],
        ),
    ] {
        let first =
            emit_paired_jvm_tasty_batch(source, source_name, SourceId::from_index(source_id))
                .expect("renamed/reordered source follows the same structural proof path");
        let second =
            emit_paired_jvm_tasty_batch(source, source_name, SourceId::from_index(source_id + 1))
                .expect("each source is repeatable in a fresh semantic session");
        assert_eq!(first.classes().len(), 3);
        assert_eq!(second.classes().len(), 3);
        for ((artifact, repeat), expected_paths) in
            first.classes().iter().zip(second.classes()).zip(paths)
        {
            assert_eq!(
                (artifact.classfile_path(), artifact.tasty_path()),
                expected_paths
            );
            assert_eq!(artifact.classfile_bytes(), repeat.classfile_bytes());
            assert_eq!(artifact.tasty_bytes(), repeat.tasty_bytes());
            assert_eq!(artifact.tasty_link_id(), repeat.tasty_link_id());
            validate_classfile_tasty_link(
                &ClassFile::decode(&mut Reader::new(artifact.classfile_bytes())).unwrap(),
                artifact.tasty_bytes(),
            )
            .unwrap();
        }
    }
}

#[test]
fn renamed_reordered_reference_constructor_graph_executes_on_verified_jvm() {
    if !require_jdk_21_or_skip() {
        return;
    }

    let renamed = REFERENCE_CONSTRUCTOR_BATCH
        .replace("package example", "package renamed")
        .replace("CounterOwnerFactory", "WidgetBuilder")
        .replace("CounterOwner", "WidgetHolder")
        .replace("Counter", "Widget")
        .replace("wrap(", "build(");
    let renamed_reordered = renamed
        .replace(
            "class Widget(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n\nclass WidgetHolder(counter: Widget):",
            "class WidgetHolder(counter: Widget):",
        )
        .replace(
            "\nclass WidgetBuilder:",
            "\nclass Widget(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n\nclass WidgetBuilder:",
        );
    let batch = emit_paired_jvm_tasty_batch(
        &renamed_reordered,
        "renamed-reference-constructor.scala",
        SourceId::from_index(17975),
    )
    .expect("renamed reference-constructor source emits a batch");
    assert_eq!(batch.classes().len(), 3);
    assert_eq!(batch.source_id(), SourceId::from_index(17975));

    let stage = TestDirectory::new("renamed-reference-constructor-runtime");
    let classes = stage.path().join("classes");
    let expected = [
        ("renamed/WidgetHolder.class", "renamed.WidgetHolder"),
        ("renamed/Widget.class", "renamed.Widget"),
        ("renamed/WidgetBuilder.class", "renamed.WidgetBuilder"),
    ];
    for (artifact, (expected_path, class_name)) in batch.classes().iter().zip(expected) {
        assert_eq!(artifact.classfile_path(), expected_path);
        let path = classes.join(artifact.classfile_path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, artifact.classfile_bytes()).unwrap();
        let decoded = ClassFile::decode(&mut Reader::new(artifact.classfile_bytes())).unwrap();
        assert_eq!(
            decoded
                .constant_pool
                .class_name(decoded.this_class)
                .unwrap(),
            expected_path.strip_suffix(".class").unwrap()
        );
        assert_eq!(decoded.version.major, 65);
        let javap = Command::new("javap")
            .args(["-classpath", classes.to_str().unwrap(), "-p", "-v"])
            .arg(class_name)
            .output()
            .expect("start javap for renamed Rust classfile");
        assert!(
            javap.status.success(),
            "javap rejected {class_name}: {}",
            String::from_utf8_lossy(&javap.stderr)
        );
        assert!(
            String::from_utf8_lossy(&javap.stdout).contains("major version: 65"),
            "javap did not report Java 21 class version for {class_name}"
        );
    }

    let harness = stage.path().join("RenamedFactoryHarness.java");
    let harness_classes = stage.path().join("harness-classes");
    std::fs::create_dir(&harness_classes).unwrap();
    std::fs::write(
        &harness,
        r#"import java.lang.reflect.Field;

public final class RenamedFactoryHarness {
    public static void main(String[] args) throws ReflectiveOperationException {
        renamed.Widget widget = new renamed.Widget(7);
        renamed.WidgetBuilder builder = new renamed.WidgetBuilder();
        renamed.WidgetHolder first = builder.build(widget);
        renamed.WidgetHolder second = builder.build(widget);
        if (first == second) throw new AssertionError("build must allocate distinct holders");
        assertIdentity(first, widget);
        assertIdentity(second, widget);
        expect(first.read(), 7);
        widget.set(42);
        expect(first.read(), 42);
        expect(second.read(), 42);

        renamed.Widget otherWidget = new renamed.Widget(3);
        renamed.WidgetHolder other = builder.build(otherWidget);
        assertIdentity(other, otherWidget);
        otherWidget.set(13);
        expect(other.read(), 13);
        expect(first.read(), 42);
        System.out.println("renamed-identity=true; same-widget-builds=distinct; values=42,13");
    }

    private static void assertIdentity(renamed.WidgetHolder holder, renamed.Widget expected)
        throws ReflectiveOperationException {
        Field inner = renamed.WidgetHolder.class.getDeclaredField("inner");
        inner.setAccessible(true);
        if (inner.get(holder) != expected) throw new AssertionError("Holder.inner lost Widget identity");
    }

    private static void expect(int actual, int expected) {
        if (actual != expected) throw new AssertionError("expected " + expected + ", got " + actual);
    }
}
"#,
    )
    .unwrap();
    let javac = Command::new("javac")
        .args([
            "-classpath",
            classes.to_str().unwrap(),
            "-d",
            harness_classes.to_str().unwrap(),
        ])
        .arg(&harness)
        .output()
        .expect("start javac against renamed Rust classes");
    assert!(
        javac.status.success(),
        "javac rejected renamed graph harness: {}",
        String::from_utf8_lossy(&javac.stderr)
    );
    let classpath = std::env::join_paths([harness_classes.as_path(), classes.as_path()]).unwrap();
    let java = Command::new("java")
        .args(["-Xverify:all", "-classpath"])
        .arg(classpath)
        .arg("RenamedFactoryHarness")
        .output()
        .expect("start strict JVM verification for renamed Rust classes");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected renamed graph:\n{}\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "renamed-identity=true; same-widget-builds=distinct; values=42,13"
    );
}

#[test]
fn reference_constructor_factory_executes_original_rust_object_graph_on_verified_jvm() {
    if !require_jdk_21_or_skip() {
        return;
    }

    let first = emit_paired_jvm_tasty_batch(
        REFERENCE_CONSTRUCTOR_BATCH,
        "CounterOwnerFactory.scala",
        SourceId::from_index(17981),
    )
    .unwrap();
    let repeated = emit_paired_jvm_tasty_batch(
        REFERENCE_CONSTRUCTOR_BATCH,
        "CounterOwnerFactory.scala",
        SourceId::from_index(17982),
    )
    .unwrap();
    assert_ne!(first.source_id(), repeated.source_id());
    assert_eq!(first.classes().len(), 3);
    for (artifact, repeated) in first.classes().iter().zip(repeated.classes()) {
        assert_eq!(artifact.classfile_bytes(), repeated.classfile_bytes());
        assert_eq!(artifact.tasty_bytes(), repeated.tasty_bytes());
        assert_eq!(artifact.tasty_link_id(), repeated.tasty_link_id());
    }

    let stage = TestDirectory::new("reference-constructor-factory-runtime");
    let malformed_third_sidecar: [u8; 0] = [];
    let mut malformed_third = batch_publish_pairs(&first);
    malformed_third[2].tasty_bytes = &malformed_third_sidecar;
    let malformed_destination = stage.path().join("malformed-third-artifact");
    assert!(matches!(
        publish_batch_pairs(&malformed_destination, &malformed_third, None),
        Err(BatchPublishError::InvalidManifest {
            class_index: 2,
            phase: "TASTy decode",
            ..
        })
    ));
    assert!(!malformed_destination.exists());

    let published = stage.path().join("published");
    let published_paths = publish_paired_batch(&published, &first, None).unwrap();
    assert_eq!(published_paths.len(), 6);
    let classes = stage.path().join("classes");
    for artifact in first.classes() {
        let path = classes.join(artifact.classfile_path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, artifact.classfile_bytes()).unwrap();
    }
    let classfile_paths = first
        .classes()
        .iter()
        .map(|artifact| classes.join(artifact.classfile_path()))
        .collect::<Vec<_>>();
    let hashes = classfile_paths
        .iter()
        .map(|path| sha256sum(path))
        .collect::<Vec<_>>();

    let decoded = first
        .classes()
        .iter()
        .map(|artifact| ClassFile::decode(&mut Reader::new(artifact.classfile_bytes())).unwrap())
        .collect::<Vec<_>>();
    let counter = &decoded[0];
    let owner = &decoded[1];
    let factory = &decoded[2];
    for (class, expected_name) in decoded.iter().zip([
        "example/Counter",
        "example/CounterOwner",
        "example/CounterOwnerFactory",
    ]) {
        assert_eq!(
            class.constant_pool.class_name(class.this_class).unwrap(),
            expected_name
        );
        assert_eq!((class.version.major, class.version.minor), (65, 0));
        assert_eq!(
            class
                .constant_pool
                .class_name(class.super_class.unwrap())
                .unwrap(),
            "java/lang/Object"
        );
        assert!(
            class
                .access_flags
                .contains(dotty_classfile::access_flags::ACC_PUBLIC)
        );
    }
    let member_pairs = |class: &ClassFile<'_>| {
        class
            .methods
            .iter()
            .map(|method| {
                (
                    class
                        .constant_pool
                        .utf8(method.name_index)
                        .unwrap()
                        .to_owned(),
                    class
                        .constant_pool
                        .utf8(method.descriptor_index)
                        .unwrap()
                        .to_owned(),
                )
            })
            .collect::<Vec<_>>()
    };
    let mut counter_methods = member_pairs(counter);
    counter_methods.sort();
    assert_eq!(
        counter_methods,
        [
            ("<init>".to_owned(), "(I)V".to_owned()),
            ("get".to_owned(), "()I".to_owned()),
            ("set".to_owned(), "(I)V".to_owned())
        ]
    );
    assert_eq!(counter.fields.len(), 1);
    assert_eq!(
        counter
            .constant_pool
            .utf8(counter.fields[0].name_index)
            .unwrap(),
        "current"
    );
    assert_eq!(
        counter
            .constant_pool
            .utf8(counter.fields[0].descriptor_index)
            .unwrap(),
        "I"
    );
    assert!(
        counter.fields[0]
            .access_flags
            .contains(dotty_classfile::access_flags::ACC_PRIVATE)
    );
    assert!(
        !counter.fields[0]
            .access_flags
            .contains(dotty_classfile::access_flags::ACC_FINAL)
    );
    let mut owner_methods = member_pairs(owner);
    owner_methods.sort();
    assert_eq!(
        owner_methods,
        [
            ("<init>".to_owned(), "(Lexample/Counter;)V".to_owned()),
            ("read".to_owned(), "()I".to_owned())
        ]
    );
    let mut factory_methods = member_pairs(factory);
    factory_methods.sort();
    assert_eq!(
        factory_methods,
        [
            ("<init>".to_owned(), "()V".to_owned()),
            (
                "wrap".to_owned(),
                "(Lexample/Counter;)Lexample/CounterOwner;".to_owned()
            )
        ]
    );
    assert!(
        factory.fields.is_empty(),
        "Factory has no synthetic storage"
    );

    assert_eq!(
        owner.fields.len(),
        1,
        "Owner stores exactly the source alias field"
    );
    let inner = &owner.fields[0];
    assert_eq!(owner.constant_pool.utf8(inner.name_index).unwrap(), "inner");
    assert_eq!(
        owner.constant_pool.utf8(inner.descriptor_index).unwrap(),
        "Lexample/Counter;"
    );
    assert!(
        inner
            .access_flags
            .contains(dotty_classfile::access_flags::ACC_PRIVATE)
    );
    assert!(
        inner
            .access_flags
            .contains(dotty_classfile::access_flags::ACC_FINAL)
    );

    fn method<'a>(
        class: &'a ClassFile<'_>,
        name: &str,
        descriptor: &str,
    ) -> &'a dotty_classfile::method::MethodInfo<'a> {
        class
            .methods
            .iter()
            .find(|method| {
                class.constant_pool.utf8(method.name_index).unwrap() == name
                    && class.constant_pool.utf8(method.descriptor_index).unwrap() == descriptor
            })
            .unwrap()
    }
    for constructor in [
        method(counter, "<init>", "(I)V"),
        method(owner, "<init>", "(Lexample/Counter;)V"),
        method(factory, "<init>", "()V"),
    ] {
        assert!(
            constructor
                .access_flags
                .contains(dotty_classfile::access_flags::ACC_PUBLIC)
        );
    }
    let [Attribute::Code(owner_ctor)] = method(owner, "<init>", "(Lexample/Counter;)V")
        .attributes
        .as_slice()
    else {
        panic!("Owner constructor has one Code attribute");
    };
    assert_eq!(&owner_ctor.code[..2], &[0x2a, 0xb7]);
    let object_ctor_index =
        ConstantPoolIndex(u16::from_be_bytes([owner_ctor.code[2], owner_ctor.code[3]]));
    let ConstantPoolEntry::Methodref {
        class_index,
        name_and_type_index,
    } = owner.constant_pool.get(object_ctor_index).unwrap()
    else {
        panic!("Owner constructor calls Object.<init> through a Methodref");
    };
    assert_eq!(
        owner.constant_pool.class_name(*class_index).unwrap(),
        "java/lang/Object"
    );
    assert_eq!(
        owner
            .constant_pool
            .name_and_type(*name_and_type_index)
            .unwrap(),
        ("<init>", "()V")
    );
    assert_eq!(
        [
            owner_ctor.code[0],
            owner_ctor.code[1],
            owner_ctor.code[4],
            owner_ctor.code[5],
            owner_ctor.code[6],
            owner_ctor.code[9]
        ],
        [0x2a, 0xb7, 0x2a, 0x2b, 0xb5, 0xb1],
        "the incoming reference is stored only after Object initialization"
    );
    let inner_field_index =
        ConstantPoolIndex(u16::from_be_bytes([owner_ctor.code[7], owner_ctor.code[8]]));
    let ConstantPoolEntry::Fieldref {
        class_index,
        name_and_type_index,
    } = owner.constant_pool.get(inner_field_index).unwrap()
    else {
        panic!("Owner constructor stores through a Fieldref");
    };
    assert_eq!(
        owner.constant_pool.class_name(*class_index).unwrap(),
        "example/CounterOwner"
    );
    assert_eq!(
        owner
            .constant_pool
            .name_and_type(*name_and_type_index)
            .unwrap(),
        ("inner", "Lexample/Counter;")
    );
    assert!(
        owner_ctor.code.contains(&0x2b),
        "Owner constructor loads the incoming Counter"
    );
    assert!(
        owner_ctor.code.contains(&0xb5),
        "Owner constructor stores the incoming Counter"
    );
    assert_eq!(
        owner_ctor
            .code
            .iter()
            .filter(|opcode| **opcode == 0xb5)
            .count(),
        1
    );
    let [Attribute::Code(owner_read)] = method(owner, "read", "()I").attributes.as_slice() else {
        panic!("Owner.read has one Code attribute");
    };
    assert_eq!(
        [
            owner_read.code[0],
            owner_read.code[1],
            owner_read.code[4],
            owner_read.code[7]
        ],
        [0x2a, 0xb4, 0xb6, 0xac],
        "read executes getfield; Counter.get; ireturn"
    );
    assert!(
        owner_read.code.contains(&0xb4),
        "Owner.read reads its alias field"
    );
    assert!(
        owner_read.code.contains(&0xb6),
        "Owner.read invokes Counter.get"
    );
    assert_eq!(owner_read.code.last(), Some(&0xac));
    assert_eq!(owner_read.code[0], 0x2a);
    let read_field_index =
        ConstantPoolIndex(u16::from_be_bytes([owner_read.code[2], owner_read.code[3]]));
    let ConstantPoolEntry::Fieldref {
        class_index,
        name_and_type_index,
    } = owner.constant_pool.get(read_field_index).unwrap()
    else {
        panic!("Owner.read loads through a Fieldref");
    };
    assert_eq!(
        owner.constant_pool.class_name(*class_index).unwrap(),
        "example/CounterOwner"
    );
    assert_eq!(
        owner
            .constant_pool
            .name_and_type(*name_and_type_index)
            .unwrap(),
        ("inner", "Lexample/Counter;")
    );
    let getter_index =
        ConstantPoolIndex(u16::from_be_bytes([owner_read.code[5], owner_read.code[6]]));
    let ConstantPoolEntry::Methodref {
        class_index,
        name_and_type_index,
    } = owner.constant_pool.get(getter_index).unwrap()
    else {
        panic!("Owner.read invokes Counter.get through a Methodref");
    };
    assert_eq!(
        owner.constant_pool.class_name(*class_index).unwrap(),
        "example/Counter"
    );
    assert_eq!(
        owner
            .constant_pool
            .name_and_type(*name_and_type_index)
            .unwrap(),
        ("get", "()I")
    );

    let [Attribute::Code(wrap)] =
        method(factory, "wrap", "(Lexample/Counter;)Lexample/CounterOwner;")
            .attributes
            .as_slice()
    else {
        panic!("Factory.wrap has one Code attribute");
    };
    assert_eq!(wrap.max_stack, 3);
    assert_eq!(wrap.max_locals, 2);
    assert_eq!(
        [
            wrap.code[0],
            wrap.code[3],
            wrap.code[4],
            wrap.code[5],
            wrap.code[8]
        ],
        [0xbb, 0x59, 0x2b, 0xb7, 0xb0]
    );
    assert_eq!(
        wrap.code.iter().filter(|opcode| **opcode == 0xbb).count(),
        1
    );
    let allocation_index = ConstantPoolIndex(u16::from_be_bytes([wrap.code[1], wrap.code[2]]));
    let ConstantPoolEntry::Class { name_index } =
        factory.constant_pool.get(allocation_index).unwrap()
    else {
        panic!("Factory allocation targets a class constant");
    };
    assert_eq!(
        factory.constant_pool.utf8(*name_index).unwrap(),
        "example/CounterOwner"
    );
    let constructor_index = ConstantPoolIndex(u16::from_be_bytes([wrap.code[6], wrap.code[7]]));
    let ConstantPoolEntry::Methodref {
        class_index,
        name_and_type_index,
    } = factory.constant_pool.get(constructor_index).unwrap()
    else {
        panic!("Factory constructor invocation targets a Methodref");
    };
    assert_eq!(
        factory.constant_pool.class_name(*class_index).unwrap(),
        "example/CounterOwner"
    );
    assert_eq!(
        factory
            .constant_pool
            .name_and_type(*name_and_type_index)
            .unwrap(),
        ("<init>", "(Lexample/Counter;)V")
    );

    for class_name in [
        "example.Counter",
        "example.CounterOwner",
        "example.CounterOwnerFactory",
    ] {
        let javap = Command::new("javap")
            .args(["-classpath", classes.to_str().unwrap(), "-p", "-v", "-c"])
            .arg(class_name)
            .output()
            .expect("start javap for original Rust classfile");
        assert!(
            javap.status.success(),
            "javap rejected {class_name}: {}",
            String::from_utf8_lossy(&javap.stderr)
        );
        let dump = String::from_utf8_lossy(&javap.stdout);
        assert!(dump.contains("major version: 65"), "{class_name}: {dump}");
        match class_name {
            "example.Counter" => {
                for expected in [
                    "public example.Counter(int);",
                    "public int get();",
                    "public void set(int);",
                ] {
                    assert!(
                        dump.contains(expected),
                        "{class_name} omitted {expected}: {dump}"
                    );
                }
            }
            "example.CounterOwner" => {
                for expected in [
                    "private final example.Counter inner;",
                    "public example.CounterOwner(example.Counter);",
                    "Field inner:Lexample/Counter;",
                    "Method example/Counter.get:()I",
                ] {
                    assert!(
                        dump.contains(expected),
                        "{class_name} omitted {expected}: {dump}"
                    );
                }
            }
            _ => {
                for expected in [
                    "public example.CounterOwner wrap(example.Counter);",
                    "new           #",
                    "aload_1",
                    "CounterOwner.\"<init>\":(Lexample/Counter;)V",
                    "areturn",
                ] {
                    assert!(
                        dump.contains(expected),
                        "{class_name} omitted {expected}: {dump}"
                    );
                }
            }
        }
    }

    let harness = stage.path().join("ReferenceFactoryHarness.java");
    let harness_classes = stage.path().join("harness-classes");
    std::fs::create_dir(&harness_classes).unwrap();
    std::fs::write(&harness, r#"import java.lang.reflect.Field;

public final class ReferenceFactoryHarness {
    public static void main(String[] args) throws ReflectiveOperationException {
        example.Counter counter = new example.Counter(7);
        example.CounterOwnerFactory factory = new example.CounterOwnerFactory();
        example.CounterOwner first = factory.wrap(counter);
        example.CounterOwner second = factory.wrap(counter);
        if (first == second) throw new AssertionError("wrap must allocate a distinct Owner each time");
        assertIdentity(first, counter);
        assertIdentity(second, counter);
        expect(first.read(), 7);
        expect(second.read(), 7);
        counter.set(42);
        expect(first.read(), 42);
        expect(second.read(), 42);
        counter.set(-9);
        expect(first.read(), -9);
        expect(second.read(), -9);

        example.Counter otherCounter = new example.Counter(3);
        example.CounterOwner other = factory.wrap(otherCounter);
        assertIdentity(other, otherCounter);
        otherCounter.set(13);
        expect(other.read(), 13);
        expect(first.read(), -9);
        counter.set(100);
        expect(first.read(), 100);
        expect(second.read(), 100);
        expect(other.read(), 13);
        System.out.println("factory-identity=true; same-counter-wraps=distinct; first=100; other=13");
    }

    private static void assertIdentity(example.CounterOwner owner, example.Counter expected)
        throws ReflectiveOperationException {
        Field inner = example.CounterOwner.class.getDeclaredField("inner");
        inner.setAccessible(true);
        if (inner.get(owner) != expected) throw new AssertionError("Owner.inner lost Counter identity");
    }

    private static void expect(int actual, int expected) {
        if (actual != expected) throw new AssertionError("expected " + expected + ", got " + actual);
    }
}
"#).unwrap();
    let javac = Command::new("javac")
        .args([
            "-classpath",
            classes.to_str().unwrap(),
            "-d",
            harness_classes.to_str().unwrap(),
        ])
        .arg(&harness)
        .output()
        .expect("start javac against the original Rust classes");
    assert!(
        javac.status.success(),
        "javac rejected harness: {}",
        String::from_utf8_lossy(&javac.stderr)
    );
    let classpath = std::env::join_paths([harness_classes.as_path(), classes.as_path()]).unwrap();
    let java = Command::new("java")
        .args(["-Xverify:all", "-classpath"])
        .arg(&classpath)
        .arg("ReferenceFactoryHarness")
        .output()
        .expect("start strict JVM verification and Rust class harness");
    assert!(
        java.status.success(),
        "java -Xverify:all failed:\n{}\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "factory-identity=true; same-counter-wraps=distinct; first=100; other=13"
    );
    for (index, path) in classfile_paths.iter().enumerate() {
        assert_eq!(
            sha256sum(path),
            hashes[index],
            "JDK tools changed original Rust classfile bytes"
        );
        assert_eq!(
            std::fs::read(path).unwrap(),
            first.classes()[index].classfile_bytes()
        );
    }

    let [Attribute::Code(factory_ctor)] = method(factory, "<init>", "()V").attributes.as_slice()
    else {
        panic!("Factory constructor has one Code attribute");
    };
    let object_constructor_index = ConstantPoolIndex(u16::from_be_bytes([
        factory_ctor.code[2],
        factory_ctor.code[3],
    ]));
    let mut wrong_constructor_target = first.classes()[2].classfile_bytes().to_vec();
    let factory_code_start = wrong_constructor_target
        .windows(wrap.code.len())
        .position(|bytes| bytes == wrap.code)
        .expect("Factory classfile contains the decoded wrap Code bytes");
    wrong_constructor_target[factory_code_start + 6..factory_code_start + 8]
        .copy_from_slice(&object_constructor_index.0.to_be_bytes());
    let wrong_constructor_root = stage.path().join("wrong-constructor-target");
    std::fs::create_dir_all(wrong_constructor_root.join("example")).unwrap();
    std::fs::write(
        wrong_constructor_root.join("example/Counter.class"),
        first.classes()[0].classfile_bytes(),
    )
    .unwrap();
    std::fs::write(
        wrong_constructor_root.join("example/CounterOwner.class"),
        first.classes()[1].classfile_bytes(),
    )
    .unwrap();
    std::fs::write(
        wrong_constructor_root.join("example/CounterOwnerFactory.class"),
        wrong_constructor_target,
    )
    .unwrap();
    let wrong_constructor_classpath =
        std::env::join_paths([harness_classes.as_path(), wrong_constructor_root.as_path()])
            .unwrap();
    let rejected_wrong_constructor = Command::new("java")
        .args(["-Xverify:all", "-classpath"])
        .arg(wrong_constructor_classpath)
        .arg("ReferenceFactoryHarness")
        .output()
        .expect("start wrong constructor target verifier negative control");
    assert!(!rejected_wrong_constructor.status.success());
    let wrong_constructor_error = format!(
        "{}{}",
        String::from_utf8_lossy(&rejected_wrong_constructor.stdout),
        String::from_utf8_lossy(&rejected_wrong_constructor.stderr)
    );
    assert!(
        wrong_constructor_error.contains("VerifyError"),
        "wrong constructor target must fail verification:\n{wrong_constructor_error}"
    );

    let mut uninitialized_owner = first.classes()[2].classfile_bytes().to_vec();
    let factory_code_offset = uninitialized_owner
        .windows(wrap.code.len())
        .position(|bytes| bytes == wrap.code)
        .expect("Factory classfile contains the decoded wrap Code bytes");
    uninitialized_owner[factory_code_offset + 5..factory_code_offset + 8]
        .copy_from_slice(&[0x57, 0x57, 0x00]);
    assert_eq!(
        &uninitialized_owner[factory_code_offset + 5..factory_code_offset + 9],
        &[0x57, 0x57, 0x00, 0xb0],
        "negative control removes the constructor call with stack-balanced pops and reaches areturn"
    );
    let corrupted = stage.path().join("uninitialized-owner");
    std::fs::create_dir_all(corrupted.join("example")).unwrap();
    std::fs::write(
        corrupted.join("example/Counter.class"),
        first.classes()[0].classfile_bytes(),
    )
    .unwrap();
    std::fs::write(
        corrupted.join("example/CounterOwner.class"),
        first.classes()[1].classfile_bytes(),
    )
    .unwrap();
    std::fs::write(
        corrupted.join("example/CounterOwnerFactory.class"),
        uninitialized_owner,
    )
    .unwrap();
    let corrupted_classpath =
        std::env::join_paths([harness_classes.as_path(), corrupted.as_path()]).unwrap();
    let rejected_uninitialized = Command::new("java")
        .args(["-Xverify:all", "-classpath"])
        .arg(corrupted_classpath)
        .arg("ReferenceFactoryHarness")
        .output()
        .expect("start uninitialized-Owner verifier negative control");
    assert!(!rejected_uninitialized.status.success());
    let verifier_error = format!(
        "{}{}",
        String::from_utf8_lossy(&rejected_uninitialized.stdout),
        String::from_utf8_lossy(&rejected_uninitialized.stderr)
    );
    assert!(
        verifier_error.contains("VerifyError"),
        "mutated Factory must fail verification, got:\n{verifier_error}"
    );
    assert!(
        verifier_error.contains("uninitialized") || verifier_error.contains("uninitializedThis"),
        "failure must be caused by the uninitialized Owner reaching areturn:\n{verifier_error}"
    );

    for (label, code_offset, replacement) in [
        ("missing-dup", 3usize, 0x00u8),
        ("integer-load-for-reference", 4, 0x1b),
        ("factory-receiver-for-counter-argument", 4, 0x2a),
        ("integer-return-for-owner", 8, 0xac),
    ] {
        let mut malformed_factory = first.classes()[2].classfile_bytes().to_vec();
        let code_start = malformed_factory
            .windows(wrap.code.len())
            .position(|bytes| bytes == wrap.code)
            .unwrap();
        malformed_factory[code_start + code_offset] = replacement;
        let malformed_root = stage.path().join(label);
        std::fs::create_dir_all(malformed_root.join("example")).unwrap();
        std::fs::write(
            malformed_root.join("example/Counter.class"),
            first.classes()[0].classfile_bytes(),
        )
        .unwrap();
        std::fs::write(
            malformed_root.join("example/CounterOwner.class"),
            first.classes()[1].classfile_bytes(),
        )
        .unwrap();
        std::fs::write(
            malformed_root.join("example/CounterOwnerFactory.class"),
            malformed_factory,
        )
        .unwrap();
        let malformed_classpath =
            std::env::join_paths([harness_classes.as_path(), malformed_root.as_path()]).unwrap();
        let rejected = Command::new("java")
            .args(["-Xverify:all", "-classpath"])
            .arg(malformed_classpath)
            .arg("ReferenceFactoryHarness")
            .output()
            .expect("start malformed Factory verifier negative control");
        assert!(!rejected.status.success(), "JVM accepted {label}");
        let output = format!(
            "{}{}",
            String::from_utf8_lossy(&rejected.stdout),
            String::from_utf8_lossy(&rejected.stderr)
        );
        assert!(
            output.contains("VerifyError"),
            "{label} failed outside verifier:\n{output}"
        );
    }

    let mut wrong_owner_receiver = first.classes()[1].classfile_bytes().to_vec();
    let owner_ctor_offset = wrong_owner_receiver
        .windows(owner_ctor.code.len())
        .position(|bytes| bytes == owner_ctor.code)
        .expect("Owner classfile contains its decoded constructor Code bytes");
    wrong_owner_receiver[owner_ctor_offset + 4] = 0x2b;
    let wrong_receiver_root = stage.path().join("wrong-owner-receiver");
    std::fs::create_dir_all(wrong_receiver_root.join("example")).unwrap();
    std::fs::write(
        wrong_receiver_root.join("example/Counter.class"),
        first.classes()[0].classfile_bytes(),
    )
    .unwrap();
    std::fs::write(
        wrong_receiver_root.join("example/CounterOwner.class"),
        wrong_owner_receiver,
    )
    .unwrap();
    std::fs::write(
        wrong_receiver_root.join("example/CounterOwnerFactory.class"),
        first.classes()[2].classfile_bytes(),
    )
    .unwrap();
    let wrong_receiver_classpath =
        std::env::join_paths([harness_classes.as_path(), wrong_receiver_root.as_path()]).unwrap();
    let wrong_receiver = Command::new("java")
        .args(["-Xverify:all", "-classpath"])
        .arg(wrong_receiver_classpath)
        .arg("ReferenceFactoryHarness")
        .output()
        .expect("start wrong Owner receiver verifier negative control");
    assert!(!wrong_receiver.status.success());
    assert!(
        String::from_utf8_lossy(&wrong_receiver.stderr).contains("VerifyError"),
        "wrong Owner receiver must fail verification: {}",
        String::from_utf8_lossy(&wrong_receiver.stderr)
    );

    let without_owner = stage.path().join("without-owner");
    std::fs::create_dir_all(without_owner.join("example")).unwrap();
    std::fs::write(
        without_owner.join("example/Counter.class"),
        first.classes()[0].classfile_bytes(),
    )
    .unwrap();
    std::fs::write(
        without_owner.join("example/CounterOwnerFactory.class"),
        first.classes()[2].classfile_bytes(),
    )
    .unwrap();
    let missing_owner_classpath =
        std::env::join_paths([harness_classes.as_path(), without_owner.as_path()]).unwrap();
    let missing_owner = Command::new("java")
        .args(["-Xverify:all", "-classpath"])
        .arg(missing_owner_classpath)
        .arg("ReferenceFactoryHarness")
        .output()
        .expect("start missing Owner linkage negative control");
    assert!(!missing_owner.status.success());
    assert!(
        String::from_utf8_lossy(&missing_owner.stderr)
            .contains("NoClassDefFoundError: example/CounterOwner")
    );

    let without_counter = stage.path().join("without-counter");
    std::fs::create_dir_all(without_counter.join("example")).unwrap();
    std::fs::write(
        without_counter.join("example/CounterOwner.class"),
        first.classes()[1].classfile_bytes(),
    )
    .unwrap();
    std::fs::write(
        without_counter.join("example/CounterOwnerFactory.class"),
        first.classes()[2].classfile_bytes(),
    )
    .unwrap();
    let missing_counter_classpath =
        std::env::join_paths([harness_classes.as_path(), without_counter.as_path()]).unwrap();
    let missing_counter = Command::new("java")
        .args(["-Xverify:all", "-classpath"])
        .arg(missing_counter_classpath)
        .arg("ReferenceFactoryHarness")
        .output()
        .expect("start missing Counter linkage negative control");
    assert!(!missing_counter.status.success());
    assert!(
        String::from_utf8_lossy(&missing_counter.stderr)
            .contains("NoClassDefFoundError: example/Counter")
    );
}

#[test]
fn constructor_injected_reference_alias_executes_on_verified_jdk21() {
    if !require_jdk_21_or_skip() {
        return;
    }

    let first = emit_paired_jvm_tasty_batch(
        COUNTER_OWNER_ALIAS,
        "tests/fixtures/jvm_tasty_constructor_injection/CounterOwner.scala",
        SourceId::from_index(17571),
    )
    .unwrap();
    let second = emit_paired_jvm_tasty_batch(
        COUNTER_OWNER_ALIAS,
        "tests/fixtures/jvm_tasty_constructor_injection/CounterOwner.scala",
        SourceId::from_index(17572),
    )
    .unwrap();
    assert_ne!(first.source_id(), second.source_id());
    assert_eq!(first.classes().len(), 2);
    for (first, second) in first.classes().iter().zip(second.classes()) {
        assert_eq!(first.classfile_bytes(), second.classfile_bytes());
        assert_eq!(first.tasty_bytes(), second.tasty_bytes());
        assert_eq!(first.tasty_link_id(), second.tasty_link_id());
    }

    let stage = TestDirectory::new("constructor-reference-alias-runtime");
    let classes = stage.path().join("rust-classes");
    let published = publish_paired_batch(&classes, &first, None).unwrap();
    assert_eq!(published.len(), 4);
    let runtime_classes = stage.path().join("runtime-classes");
    for pair in first.classes() {
        let classfile = runtime_classes.join(pair.classfile_path());
        std::fs::create_dir_all(classfile.parent().unwrap()).unwrap();
        std::fs::write(classfile, pair.classfile_bytes()).unwrap();
    }
    assert_eq!(read_relative_files(&runtime_classes).unwrap().len(), 2);
    let classfile_paths = first
        .classes()
        .iter()
        .map(|pair| runtime_classes.join(pair.classfile_path()))
        .collect::<Vec<_>>();
    let original_hashes = classfile_paths
        .iter()
        .map(|path| sha256sum(path))
        .collect::<Vec<_>>();

    let owner_pair = &first.classes()[1];

    let javap_counter = Command::new("javap")
        .args([
            "-classpath",
            runtime_classes.to_str().unwrap(),
            "-v",
            "-p",
            "-c",
        ])
        .arg("example.Counter")
        .output()
        .expect("start JDK 21 javap for the Rust Counter class");
    assert!(
        javap_counter.status.success(),
        "javap rejected original Rust Counter.class:\n{}",
        String::from_utf8_lossy(&javap_counter.stderr)
    );
    let javap_counter = String::from_utf8_lossy(&javap_counter.stdout);
    assert!(
        javap_counter.contains("major version: 65"),
        "{javap_counter}"
    );
    for expected in [
        "private int current;",
        "descriptor: I",
        "public example.Counter(int);",
        "descriptor: (I)V",
        "public int get();",
        "descriptor: ()I",
        "public void set(int);",
    ] {
        assert!(
            javap_counter.contains(expected),
            "javap Counter dump omitted {expected:?}:\n{javap_counter}"
        );
    }

    let javap = Command::new("javap")
        .args([
            "-classpath",
            runtime_classes.to_str().unwrap(),
            "-v",
            "-p",
            "-c",
        ])
        .arg("example.CounterOwner")
        .output()
        .expect("start JDK 21 javap for the Rust CounterOwner alias class");
    assert!(
        javap.status.success(),
        "javap rejected original Rust CounterOwner.class:\n{}",
        String::from_utf8_lossy(&javap.stderr)
    );
    let javap = String::from_utf8_lossy(&javap.stdout);
    assert!(javap.contains("major version: 65"), "{javap}");
    assert!(
        javap.contains("private final example.Counter inner;"),
        "{javap}"
    );
    let constructor_dump = javap
        .split("public example.CounterOwner(example.Counter);")
        .nth(1)
        .expect("javap shows the reference constructor")
        .split("public int read();")
        .next()
        .unwrap();
    for expected in [
        "descriptor: (Lexample/Counter;)V",
        "Method java/lang/Object.\"<init>\":()V",
        "Field inner:Lexample/Counter;",
        "aload_1",
        "putfield",
        "return",
    ] {
        assert!(
            constructor_dump.contains(expected),
            "javap constructor dump omitted {expected:?}:\n{constructor_dump}"
        );
    }
    assert!(!constructor_dump.contains("new "), "{constructor_dump}");
    assert_eq!(constructor_dump.matches("putfield").count(), 1);
    assert!(
        !constructor_dump.contains("Method example/Counter.\"<init>\":(I)V"),
        "{constructor_dump}"
    );
    let read_dump = javap.split("public int read();").nth(1).unwrap();
    for expected in [
        "descriptor: ()I",
        "Field inner:Lexample/Counter;",
        "Method example/Counter.get:()I",
    ] {
        assert!(
            read_dump.contains(expected),
            "javap read dump omitted {expected:?}:\n{read_dump}"
        );
    }

    let harness = stage.path().join("CounterOwnerAliasHarness.java");
    let harness_classes = stage.path().join("harness-classes");
    std::fs::create_dir(&harness_classes).unwrap();
    std::fs::write(
        &harness,
        r#"package example;

import java.lang.reflect.Field;

public final class CounterOwnerAliasHarness {
    public static void main(String[] args) throws ReflectiveOperationException {
        Counter firstCounter = new Counter(7);
        CounterOwner firstOwner = new CounterOwner(firstCounter);
        assertIdentity(firstOwner, firstCounter);
        expect(firstOwner.read(), 7);
        firstCounter.set(42);
        expect(firstOwner.read(), 42);
        firstCounter.set(-9);
        expect(firstOwner.read(), -9);

        Counter secondCounter = new Counter(3);
        CounterOwner secondOwner = new CounterOwner(secondCounter);
        assertIdentity(secondOwner, secondCounter);
        expect(secondOwner.read(), 3);
        secondCounter.set(13);
        expect(secondOwner.read(), 13);
        expect(firstOwner.read(), -9);
        firstCounter.set(100);
        expect(firstOwner.read(), 100);
        expect(secondOwner.read(), 13);
        System.out.println("identity=true; first=100; second=13");
    }

    private static void assertIdentity(CounterOwner owner, Counter expected)
        throws ReflectiveOperationException {
        Field inner = CounterOwner.class.getDeclaredField("inner");
        inner.setAccessible(true);
        if (inner.get(owner) != expected) {
            throw new AssertionError("Owner.inner is not the exact injected Counter");
        }
    }

    private static void expect(int actual, int expected) {
        if (actual != expected) {
            throw new AssertionError("expected " + expected + ", got " + actual);
        }
    }
}
"#,
    )
    .unwrap();
    let javac = Command::new("javac")
        .arg("-classpath")
        .arg(&runtime_classes)
        .arg("-d")
        .arg(&harness_classes)
        .arg(&harness)
        .output()
        .expect("start JDK 21 javac for the original Rust class harness");
    assert!(
        javac.status.success(),
        "javac rejected the alias harness:\n{}",
        String::from_utf8_lossy(&javac.stderr)
    );
    for (index, path) in classfile_paths.iter().enumerate() {
        assert_eq!(
            sha256sum(path),
            original_hashes[index],
            "javac changed the Rust class"
        );
    }

    let classpath =
        std::env::join_paths([harness_classes.as_path(), runtime_classes.as_path()]).unwrap();
    let java = Command::new("java")
        .args(["-Xverify:all", "-ea", "-classpath"])
        .arg(&classpath)
        .arg("example.CounterOwnerAliasHarness")
        .output()
        .expect("start JDK 21 verifier and original Rust class harness");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected or failed original Rust classes:\n{}\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "identity=true; first=100; second=13"
    );
    for (index, (path, pair)) in classfile_paths.iter().zip(first.classes()).enumerate() {
        assert_eq!(
            sha256sum(path),
            original_hashes[index],
            "java changed the Rust class"
        );
        assert_eq!(
            std::fs::read(path).unwrap(),
            pair.classfile_bytes(),
            "runtime must use the original Rust-produced classfile"
        );
    }

    let without_counter = stage.path().join("without-counter");
    std::fs::create_dir_all(without_counter.join("example")).unwrap();
    std::fs::write(
        without_counter.join("example/CounterOwner.class"),
        owner_pair.classfile_bytes(),
    )
    .unwrap();
    let missing_counter_classpath =
        std::env::join_paths([harness_classes.as_path(), without_counter.as_path()]).unwrap();
    let missing_counter = Command::new("java")
        .args(["-Xverify:all", "-ea", "-classpath"])
        .arg(missing_counter_classpath)
        .arg("example.CounterOwnerAliasHarness")
        .output()
        .expect("start JDK 21 missing-Counter negative control");
    assert!(!missing_counter.status.success());
    let missing_counter_error = String::from_utf8_lossy(&missing_counter.stderr);
    assert!(
        missing_counter_error.contains("NoClassDefFoundError: example/Counter"),
        "missing Counter class produced unexpected JVM error:\n{missing_counter_error}"
    );
}

#[test]
fn reference_field_owner_batch_executes_on_verified_jdk21() {
    if !require_jdk_21_or_skip() {
        return;
    }

    let batch = emit_paired_jvm_tasty_batch(
        COUNTER_OWNER,
        "tests/fixtures/jvm_tasty_object_composition/CounterOwner.scala",
        SourceId::from_index(1719),
    )
    .unwrap();
    let output = TestDirectory::new("reference-field-owner-jdk-runtime");
    let classes = output.path().join("classes");
    let published = publish_paired_batch(&classes, &batch, None).unwrap();
    assert_eq!(
        published
            .iter()
            .map(|path| path.strip_prefix(&classes).unwrap().to_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "example/Counter.class",
            "example/Counter.tasty",
            "example/CounterOwner.class",
            "example/CounterOwner.tasty",
        ]
    );
    let classfile_paths = batch
        .classes()
        .iter()
        .map(|pair| classes.join(pair.classfile_path()))
        .collect::<Vec<_>>();
    let original_hashes = classfile_paths
        .iter()
        .map(|path| sha256sum(path))
        .collect::<Vec<_>>();

    let javap = Command::new("javap")
        .args([
            "-classpath",
            classes.to_str().unwrap(),
            "-p",
            "-s",
            "-c",
            "-v",
            "example.CounterOwner",
        ])
        .output()
        .expect("start JDK 21 javap for the Rust CounterOwner class");
    assert!(
        javap.status.success(),
        "javap rejected Rust CounterOwner.class:\n{}",
        String::from_utf8_lossy(&javap.stderr)
    );
    let javap = String::from_utf8_lossy(&javap.stdout);
    assert!(javap.contains("major version: 65"), "{javap}");
    for expected in [
        "private final example.Counter inner;",
        "descriptor: Lexample/Counter;",
        "public example.CounterOwner(int);",
        "descriptor: (I)V",
        "public int read();",
        "descriptor: ()I",
        "Method java/lang/Object.\"<init>\":()V",
        "new           #",
        "Method example/Counter.\"<init>\":(I)V",
        "putfield      #",
        "getfield      #",
        "invokevirtual #",
        "Method example/Counter.get:()I",
    ] {
        assert!(
            javap.contains(expected),
            "javap output omitted {expected:?}:\n{javap}"
        );
    }

    let harness = output.path().join("CounterOwnerRuntimeHarness.java");
    let harness_classes = output.path().join("harness-classes");
    std::fs::create_dir(&harness_classes).unwrap();
    std::fs::write(
        &harness,
        r#"public final class CounterOwnerRuntimeHarness {
    public static void main(String[] args) {
        int[] seeds = {7, 42, 0, -3};
        example.CounterOwner[] owners = new example.CounterOwner[seeds.length];
        for (int index = 0; index < seeds.length; index++) {
            owners[index] = new example.CounterOwner(seeds[index]);
        }
        for (int repetition = 0; repetition < 3; repetition++) {
            for (int index = 0; index < seeds.length; index++) {
                expect(owners[index].read(), seeds[index]);
            }
        }
        System.out.println("independent owners=7,42,0,-3; repeated=3");
    }

    private static void expect(int actual, int expected) {
        if (actual != expected) {
            throw new AssertionError("expected " + expected + ", got " + actual);
        }
    }
}
"#,
    )
    .unwrap();
    let javac = Command::new("javac")
        .arg("-classpath")
        .arg(&classes)
        .arg("-d")
        .arg(&harness_classes)
        .arg(&harness)
        .output()
        .expect("start JDK 21 javac for the Rust CounterOwner harness");
    assert!(
        javac.status.success(),
        "javac rejected the Rust CounterOwner batch:\n{}",
        String::from_utf8_lossy(&javac.stderr)
    );

    let classpath = std::env::join_paths([harness_classes.as_path(), classes.as_path()]).unwrap();
    let java = Command::new("java")
        .args(["-Xverify:all", "-ea", "-classpath"])
        .arg(classpath)
        .arg("CounterOwnerRuntimeHarness")
        .output()
        .expect("start JDK 21 verifier and Rust CounterOwner harness");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected or failed the Rust CounterOwner batch:\n{}\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "independent owners=7,42,0,-3; repeated=3"
    );

    for (index, (path, pair)) in classfile_paths.iter().zip(batch.classes()).enumerate() {
        assert_eq!(sha256sum(path), original_hashes[index]);
        assert_eq!(
            std::fs::read(path).unwrap(),
            pair.classfile_bytes(),
            "javac/java must not replace the original Rust-produced classfile"
        );
    }
}

fn sha256sum(path: &std::path::Path) -> String {
    let output = Command::new("sha256sum")
        .arg(path)
        .output()
        .expect("start sha256sum for original Rust classfile");
    assert!(
        output.status.success(),
        "sha256sum failed for {}: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .expect("sha256sum prints a digest")
        .to_owned()
}

fn classfile_constant_pool_entry_offset(classfile: &[u8], wanted: ConstantPoolIndex) -> usize {
    let count = usize::from(u16::from_be_bytes([classfile[8], classfile[9]]));
    let mut index = 1usize;
    let mut offset = 10usize;
    while index < count {
        let entry_offset = offset;
        let tag = classfile[offset];
        offset += 1;
        let wide_slots = match tag {
            1 => {
                let length = usize::from(u16::from_be_bytes([
                    classfile[offset],
                    classfile[offset + 1],
                ]));
                offset += 2 + length;
                1
            }
            3 | 4 => {
                offset += 4;
                1
            }
            5 | 6 => {
                offset += 8;
                2
            }
            7 | 8 | 16 | 19 | 20 => {
                offset += 2;
                1
            }
            9 | 10 | 11 | 12 | 17 | 18 => {
                offset += 4;
                1
            }
            15 => {
                offset += 3;
                1
            }
            other => panic!("unexpected constant-pool tag {other} at slot {index}"),
        };
        if index == usize::from(wanted.0) {
            return entry_offset;
        }
        index += wide_slots;
    }
    panic!("constant-pool entry {} is absent", wanted.0)
}

#[test]
fn reference_field_batch_publication_rejects_partial_and_faulted_results() {
    let batch = emit_paired_jvm_tasty_batch(
        COUNTER_OWNER,
        "tests/fixtures/jvm_tasty_object_composition/CounterOwner.scala",
        SourceId::from_index(1722),
    )
    .unwrap();
    let pairs = batch_publish_pairs(&batch);
    let base = TestDirectory::new("object-composition-publish-faults");

    let missing_pair = base.path().join("missing-pair");
    assert!(matches!(
        publish_batch_pairs(&missing_pair, &pairs[..1], None),
        Err(BatchPublishError::InvalidManifest {
            phase: "manifest shape",
            ..
        })
    ));
    assert!(!missing_pair.exists());

    let malformed_sidecar = [0u8; 0];
    let mut malformed_pairs = pairs.clone();
    malformed_pairs[1].tasty_bytes = &malformed_sidecar;
    let malformed_destination = base.path().join("malformed-owner-sidecar");
    assert!(matches!(
        publish_batch_pairs(&malformed_destination, &malformed_pairs, None),
        Err(BatchPublishError::InvalidManifest {
            class_index: 1,
            phase: "TASTy decode",
            ..
        })
    ));
    assert!(!malformed_destination.exists());

    let mut duplicate_pairs = pairs.clone();
    duplicate_pairs[1].classfile_path = duplicate_pairs[0].classfile_path;
    let duplicate_destination = base.path().join("duplicate-path");
    assert!(matches!(
        publish_batch_pairs(&duplicate_destination, &duplicate_pairs, None),
        Err(BatchPublishError::InvalidManifest {
            phase: "artifact paths",
            ..
        })
    ));
    assert!(!duplicate_destination.exists());

    let owner_link = batch.classes()[1].tasty_link_id();
    let mut mismatched_classfile = pairs[1].classfile_bytes.to_vec();
    let link_offset = mismatched_classfile
        .windows(owner_link.as_bytes().len())
        .position(|window| window == owner_link.as_bytes())
        .expect("CounterOwner classfile contains its TASTy link UUID");
    mismatched_classfile[link_offset] ^= 0x80;
    let mut mismatched_pairs = pairs.clone();
    mismatched_pairs[1].classfile_bytes = &mismatched_classfile;
    let mismatched_destination = base.path().join("mismatched-owner-link");
    assert!(matches!(
        publish_batch_pairs(&mismatched_destination, &mismatched_pairs, None),
        Err(BatchPublishError::InvalidManifest {
            class_index: 1,
            phase: "classfile/TASTy link",
            ..
        })
    ));
    assert!(!mismatched_destination.exists());

    for (index, fault) in [
        BatchPublishFault::AfterStagedFiles(1),
        BatchPublishFault::AfterStagedFiles(2),
        BatchPublishFault::AfterStagedFiles(3),
        BatchPublishFault::AfterStagedFiles(4),
        BatchPublishFault::BeforePromotion,
        BatchPublishFault::AfterPromotion,
    ]
    .into_iter()
    .enumerate()
    {
        let destination = base.path().join(format!("injected-failure-{index}"));
        assert!(matches!(
            publish_paired_batch(&destination, &batch, Some(fault)),
            Err(BatchPublishError::Io { .. })
        ));
        assert!(!destination.exists(), "fault case {index}");
        assert_eq!(
            std::fs::read_dir(base.path()).unwrap().count(),
            0,
            "fault case {index} must remove its private staging tree"
        );
    }

    let existing_destination = base.path().join("existing-output");
    std::fs::create_dir(&existing_destination).unwrap();
    let sentinel = existing_destination.join("sentinel");
    std::fs::write(&sentinel, b"preserve existing output").unwrap();
    assert!(matches!(
        publish_paired_batch(&existing_destination, &batch, None),
        Err(BatchPublishError::DestinationExists { .. })
    ));
    assert_eq!(
        std::fs::read(&sentinel).unwrap(),
        b"preserve existing output"
    );
    assert_eq!(
        read_relative_files(&existing_destination).unwrap(),
        ["sentinel"]
    );
    assert_eq!(std::fs::read_dir(base.path()).unwrap().count(), 1);
}

#[test]
fn constructor_alias_batch_publication_rejects_invalid_pairs_and_partial_results() {
    let batch = emit_paired_jvm_tasty_batch(
        COUNTER_OWNER_ALIAS,
        "tests/fixtures/jvm_tasty_constructor_injection/CounterOwner.scala",
        SourceId::from_index(17591),
    )
    .unwrap();
    let pairs = batch_publish_pairs(&batch);
    let base = TestDirectory::new("constructor-alias-publish-faults");

    let missing_pair = base.path().join("missing-pair");
    assert!(matches!(
        publish_batch_pairs(&missing_pair, &pairs[..1], None),
        Err(BatchPublishError::InvalidManifest {
            phase: "manifest shape",
            ..
        })
    ));
    assert!(!missing_pair.exists());

    let mut foreign_source = pairs.clone();
    foreign_source[1].source_id = SourceId::from_index(17592);
    let foreign_source_destination = base.path().join("foreign-source");
    assert!(matches!(
        publish_batch_pairs(&foreign_source_destination, &foreign_source, None),
        Err(BatchPublishError::InvalidManifest {
            class_index: 1,
            phase: "source identity",
            ..
        })
    ));
    assert!(!foreign_source_destination.exists());

    let mut duplicate_class = pairs.clone();
    duplicate_class[1].class = duplicate_class[0].class;
    let duplicate_class_destination = base.path().join("duplicate-class");
    assert!(matches!(
        publish_batch_pairs(&duplicate_class_destination, &duplicate_class, None),
        Err(BatchPublishError::InvalidManifest {
            class_index: 1,
            phase: "class identity",
            ..
        })
    ));
    assert!(!duplicate_class_destination.exists());

    let mut duplicate_path = pairs.clone();
    duplicate_path[1].classfile_path = duplicate_path[0].classfile_path;
    let duplicate_path_destination = base.path().join("duplicate-path");
    assert!(matches!(
        publish_batch_pairs(&duplicate_path_destination, &duplicate_path, None),
        Err(BatchPublishError::InvalidManifest {
            phase: "artifact paths",
            ..
        })
    ));
    assert!(!duplicate_path_destination.exists());

    let mut unsafe_path = pairs.clone();
    unsafe_path[1].classfile_path = "../CounterOwner.class";
    let unsafe_path_destination = base.path().join("unsafe-path");
    assert!(matches!(
        publish_batch_pairs(&unsafe_path_destination, &unsafe_path, None),
        Err(BatchPublishError::InvalidManifest {
            phase: "class-file path",
            ..
        })
    ));
    assert!(!unsafe_path_destination.exists());

    let malformed_owner_class = [];
    let mut malformed_class_pair = pairs.clone();
    malformed_class_pair[1].classfile_bytes = &malformed_owner_class;
    let malformed_class_destination = base.path().join("malformed-owner-class");
    assert!(matches!(
        publish_batch_pairs(&malformed_class_destination, &malformed_class_pair, None),
        Err(BatchPublishError::InvalidManifest {
            class_index: 1,
            phase: "class-file decode",
            ..
        })
    ));
    assert!(!malformed_class_destination.exists());

    let malformed_owner_sidecar = [];
    let mut malformed_sidecar_pair = pairs.clone();
    malformed_sidecar_pair[1].tasty_bytes = &malformed_owner_sidecar;
    let malformed_sidecar_destination = base.path().join("malformed-owner-sidecar");
    assert!(matches!(
        publish_batch_pairs(
            &malformed_sidecar_destination,
            &malformed_sidecar_pair,
            None
        ),
        Err(BatchPublishError::InvalidManifest {
            class_index: 1,
            phase: "TASTy decode",
            ..
        })
    ));
    assert!(!malformed_sidecar_destination.exists());

    let owner_link = batch.classes()[1].tasty_link_id();
    let mut stale_owner_class = pairs[1].classfile_bytes.to_vec();
    let link_offset = stale_owner_class
        .windows(owner_link.as_bytes().len())
        .position(|window| window == owner_link.as_bytes())
        .expect("CounterOwner classfile contains its TASTy link UUID");
    stale_owner_class[link_offset] ^= 0x80;
    let mut stale_link_pair = pairs.clone();
    stale_link_pair[1].classfile_bytes = &stale_owner_class;
    let stale_link_destination = base.path().join("stale-owner-link");
    assert!(matches!(
        publish_batch_pairs(&stale_link_destination, &stale_link_pair, None),
        Err(BatchPublishError::InvalidManifest {
            class_index: 1,
            phase: "classfile/TASTy link",
            ..
        })
    ));
    assert!(!stale_link_destination.exists());

    for (index, fault) in [
        BatchPublishFault::AfterStagedFiles(1),
        BatchPublishFault::AfterStagedFiles(2),
        BatchPublishFault::AfterStagedFiles(3),
        BatchPublishFault::AfterStagedFiles(4),
        BatchPublishFault::BeforePromotion,
        BatchPublishFault::AfterPromotion,
    ]
    .into_iter()
    .enumerate()
    {
        let destination = base.path().join(format!("injected-failure-{index}"));
        assert!(matches!(
            publish_paired_batch(&destination, &batch, Some(fault)),
            Err(BatchPublishError::Io { .. })
        ));
        assert!(!destination.exists(), "fault case {index}");
        assert_eq!(std::fs::read_dir(base.path()).unwrap().count(), 0);
    }

    let existing_destination = base.path().join("existing-output");
    std::fs::create_dir(&existing_destination).unwrap();
    let sentinel = existing_destination.join("sentinel");
    std::fs::write(&sentinel, b"preserve constructor alias output").unwrap();
    assert!(matches!(
        publish_paired_batch(&existing_destination, &batch, None),
        Err(BatchPublishError::DestinationExists { .. })
    ));
    assert_eq!(
        std::fs::read(&sentinel).unwrap(),
        b"preserve constructor alias output"
    );
    assert_eq!(
        read_relative_files(&existing_destination).unwrap(),
        ["sentinel"]
    );
    assert_eq!(std::fs::read_dir(base.path()).unwrap().count(), 1);
}

#[test]
fn nested_reference_field_call_rejects_the_complete_batch() {
    let source = COUNTER_OWNER.replace("inner.get()", "inner.get() + 1");
    assert!(matches!(
        emit_paired_jvm_tasty_batch(
            &source,
            "tests/fixtures/jvm_tasty_object_composition/CounterOwner.scala",
            SourceId::from_index(1719),
        ),
        Err(PairedBatchArtifactError::Contract(
            JvmTastyContractError::BatchArtifact {
                class_index: 1,
                phase: "cross-class invocation",
                ..
            }
        ))
    ));
}

fn take_batch_opcode(code: &[u8], pc: &mut usize, expected: u8) {
    assert_eq!(code.get(*pc), Some(&expected));
    *pc += 1;
}

fn take_batch_cp_operand(code: &[u8], pc: &mut usize, opcode: u8) -> ConstantPoolIndex {
    take_batch_opcode(code, pc, opcode);
    let index = ConstantPoolIndex(u16::from_be_bytes([code[*pc], code[*pc + 1]]));
    *pc += 2;
    index
}

fn batch_class_name<'a>(class: &'a ClassFile<'a>, index: ConstantPoolIndex) -> &'a str {
    let ConstantPoolEntry::Class { name_index } = class.constant_pool.get(index).unwrap() else {
        panic!("allocation operand references CONSTANT_Class");
    };
    class.constant_pool.utf8(*name_index).unwrap()
}

fn batch_method_ref<'a>(
    class: &'a ClassFile<'a>,
    index: ConstantPoolIndex,
) -> (&'a str, (&'a str, &'a str)) {
    let ConstantPoolEntry::Methodref {
        class_index,
        name_and_type_index,
    } = class.constant_pool.get(index).unwrap()
    else {
        panic!("invokespecial operand references CONSTANT_Methodref");
    };
    (
        class.constant_pool.class_name(*class_index).unwrap(),
        class
            .constant_pool
            .name_and_type(*name_and_type_index)
            .unwrap(),
    )
}

fn sibling_factory_classfile_mut(
    artifacts: &mut JvmTastyBatchArtifacts,
) -> &mut JvmTastyClassFileArtifact {
    artifacts
        .classfiles
        .iter_mut()
        .find(|artifact| artifact.classfile_path == "example/PairFactory.class")
        .unwrap()
}

#[test]
fn sibling_constructor_batch_emits_six_deterministic_linked_artifacts() {
    let first = emit_paired_jvm_tasty_batch(
        MULTI_ARGUMENT_CONSTRUCTOR_BATCH,
        "PairFactory.scala",
        SourceId::from_index(19170),
    )
    .expect("the complete source batch satisfies its cross-producer contract");
    let second = emit_paired_jvm_tasty_batch(
        MULTI_ARGUMENT_CONSTRUCTOR_BATCH,
        "PairFactory.scala",
        SourceId::from_index(19171),
    )
    .unwrap();
    assert_ne!(first.source_id(), second.source_id());
    assert_eq!(first.classes().len(), 3);
    assert_eq!(second.classes().len(), 3);

    let expected = [
        ("example/Counter.class", "example/Counter.tasty"),
        ("example/Pair.class", "example/Pair.tasty"),
        ("example/PairFactory.class", "example/PairFactory.tasty"),
    ];
    let mut decoded = Vec::new();
    for (index, (artifact, repeated)) in first.classes().iter().zip(second.classes()).enumerate() {
        assert_eq!(
            (artifact.classfile_path(), artifact.tasty_path()),
            expected[index]
        );
        assert_eq!(artifact.source_id(), first.source_id());
        assert_eq!(artifact.classfile_bytes(), repeated.classfile_bytes());
        assert_eq!(artifact.tasty_bytes(), repeated.tasty_bytes());
        assert_eq!(artifact.tasty_link_id(), repeated.tasty_link_id());
        let tasty = TastyFile::parse_and_validate_scala_3_9(artifact.tasty_bytes()).unwrap();
        assert_eq!(
            (
                tasty.header().major_version,
                tasty.header().minor_version,
                tasty.header().experimental_version,
            ),
            (28, 9, 0)
        );
        assert_eq!(
            artifact.tasty_link_id(),
            TastyLinkId::from_bytes(tasty.header().uuid)
        );
        let mut reader = Reader::new(artifact.classfile_bytes());
        let class = ClassFile::decode(&mut reader).unwrap();
        assert!(reader.is_at_end());
        assert_eq!((class.version.major, class.version.minor), (65, 0));
        assert_eq!(
            class.tasty_link_id().unwrap(),
            Some(artifact.tasty_link_id())
        );
        validate_classfile_tasty_link(&class, artifact.tasty_bytes()).unwrap();
        decoded.push(class);
    }
    assert_eq!(
        first
            .classes()
            .iter()
            .flat_map(|artifact| [artifact.classfile_path(), artifact.tasty_path()])
            .collect::<Vec<_>>(),
        [
            "example/Counter.class",
            "example/Counter.tasty",
            "example/Pair.class",
            "example/Pair.tasty",
            "example/PairFactory.class",
            "example/PairFactory.tasty",
        ]
    );

    let pair = &decoded[1];
    let pair_constructor = pair
        .methods
        .iter()
        .find(|method| pair.constant_pool.utf8(method.name_index).unwrap() == "<init>")
        .unwrap();
    assert_eq!(
        pair.constant_pool
            .utf8(pair_constructor.descriptor_index)
            .unwrap(),
        "(Lexample/Counter;Lexample/Counter;)V"
    );
    assert_eq!(
        pair.fields
            .iter()
            .map(|field| (
                pair.constant_pool.utf8(field.name_index).unwrap(),
                pair.constant_pool.utf8(field.descriptor_index).unwrap(),
            ))
            .collect::<Vec<_>>(),
        [
            ("left", "Lexample/Counter;"),
            ("right", "Lexample/Counter;"),
        ]
    );

    let factory = &decoded[2];
    let create = factory
        .methods
        .iter()
        .find(|method| factory.constant_pool.utf8(method.name_index).unwrap() == "create")
        .unwrap();
    assert_eq!(
        factory.constant_pool.utf8(create.descriptor_index).unwrap(),
        "(II)Lexample/Pair;"
    );
    let [Attribute::Code(code)] = create.attributes.as_slice() else {
        panic!("PairFactory.create has one Code attribute");
    };
    assert_eq!((code.max_stack, code.max_locals), (6, 3));
    let mut pc = 0;
    let outer_allocation_pc = pc;
    let outer_allocation = take_batch_cp_operand(code.code, &mut pc, 0xbb);
    assert_eq!(batch_class_name(factory, outer_allocation), "example/Pair");
    take_batch_opcode(code.code, &mut pc, 0x59);
    let first_allocation_pc = pc;
    let first_allocation = take_batch_cp_operand(code.code, &mut pc, 0xbb);
    assert_eq!(
        batch_class_name(factory, first_allocation),
        "example/Counter"
    );
    take_batch_opcode(code.code, &mut pc, 0x59);
    take_batch_opcode(code.code, &mut pc, 0x1b);
    let first_constructor = take_batch_cp_operand(code.code, &mut pc, 0xb7);
    assert_eq!(
        batch_method_ref(factory, first_constructor),
        ("example/Counter", ("<init>", "(I)V"))
    );
    let second_allocation_pc = pc;
    let second_allocation = take_batch_cp_operand(code.code, &mut pc, 0xbb);
    assert_eq!(
        batch_class_name(factory, second_allocation),
        "example/Counter"
    );
    take_batch_opcode(code.code, &mut pc, 0x59);
    take_batch_opcode(code.code, &mut pc, 0x1c);
    let second_constructor = take_batch_cp_operand(code.code, &mut pc, 0xb7);
    assert_eq!(
        batch_method_ref(factory, second_constructor),
        ("example/Counter", ("<init>", "(I)V"))
    );
    let pair_constructor_pc = pc;
    let pair_constructor = take_batch_cp_operand(code.code, &mut pc, 0xb7);
    assert_eq!(
        batch_method_ref(factory, pair_constructor),
        (
            "example/Pair",
            ("<init>", "(Lexample/Counter;Lexample/Counter;)V"),
        )
    );
    take_batch_opcode(code.code, &mut pc, 0xb0);
    assert_eq!(pc, code.code.len(), "the checker consumes all instructions");
    assert_ne!(first_allocation_pc, second_allocation_pc);
    assert_eq!(first_constructor, second_constructor);
    eprintln!(
        "sibling-allocation-pcs outer={outer_allocation_pc} first_counter={first_allocation_pc} second_counter={second_allocation_pc} pair_init={pair_constructor_pc}"
    );

    let mut sequence = CodeSequence::new();
    sequence
        .instruction(Instruction::New(ClassReference::new(
            ConstantPoolIndexOperand::try_from(outer_allocation.0).unwrap(),
        )))
        .instruction(Instruction::Dup)
        .instruction(Instruction::New(ClassReference::new(
            ConstantPoolIndexOperand::try_from(first_allocation.0).unwrap(),
        )))
        .instruction(Instruction::Dup)
        .instruction(Instruction::ILoad(LocalIndex::new(1)))
        .instruction(Instruction::InvokeSpecial(SpecialMethodReference::new(
            ConstantPoolIndexOperand::try_from(first_constructor.0).unwrap(),
        )))
        .instruction(Instruction::New(ClassReference::new(
            ConstantPoolIndexOperand::try_from(second_allocation.0).unwrap(),
        )))
        .instruction(Instruction::Dup)
        .instruction(Instruction::ILoad(LocalIndex::new(2)))
        .instruction(Instruction::InvokeSpecial(SpecialMethodReference::new(
            ConstantPoolIndexOperand::try_from(second_constructor.0).unwrap(),
        )))
        .instruction(Instruction::InvokeSpecial(SpecialMethodReference::new(
            ConstantPoolIndexOperand::try_from(pair_constructor.0).unwrap(),
        )))
        .instruction(Instruction::AReturn);
    let graph = ControlFlowGraph::build(&sequence).unwrap();
    let analysis = CfgCodeAnalyzer::analyze_with_current_class(
        &graph,
        "create",
        "(II)Lexample/Pair;",
        false,
        &factory.constant_pool,
        factory.this_class,
    )
    .expect("decoded sibling allocations pass class-aware uninitialized-site analysis");
    assert_eq!(
        (analysis.max_stack, analysis.max_locals),
        (code.max_stack, code.max_locals)
    );
}

#[test]
fn dual_reference_forwarding_emits_six_linked_artifacts_with_ordered_formal_loads() {
    let first = emit_paired_jvm_tasty_batch(
        DUAL_REFERENCE_FORWARDING_BATCH,
        "PairFactory.scala",
        SourceId::from_index(19660),
    )
    .expect("the one-session source batch satisfies the linked JVM/TASTy contract");
    let second = emit_paired_jvm_tasty_batch(
        DUAL_REFERENCE_FORWARDING_BATCH,
        "PairFactory.scala",
        SourceId::from_index(19661),
    )
    .unwrap();
    assert_ne!(first.source_id(), second.source_id());
    match std::env::var("DOTTY_REQUIRE_SCALA_TASTY_DUAL_REFERENCE_FORWARDING_BATCH").as_deref() {
        Ok("1") => verify_dual_reference_batch_with_pinned_scala(&first),
        Ok(value) => panic!(
            "DOTTY_REQUIRE_SCALA_TASTY_DUAL_REFERENCE_FORWARDING_BATCH must equal 1, got {value:?}"
        ),
        Err(std::env::VarError::NotPresent) => {}
        Err(error) => panic!("read strict Scala reader gate: {error}"),
    }
    assert_eq!(first.classes().len(), 3);
    assert_eq!(second.classes().len(), 3);
    let expected = [
        ("example/Counter.class", "example/Counter.tasty"),
        ("example/Pair.class", "example/Pair.tasty"),
        ("example/PairFactory.class", "example/PairFactory.tasty"),
    ];
    let mut decoded = Vec::new();
    for (index, (artifact, repeated)) in first.classes().iter().zip(second.classes()).enumerate() {
        assert_eq!(
            (artifact.classfile_path(), artifact.tasty_path()),
            expected[index]
        );
        assert_eq!(artifact.source_id(), first.source_id());
        assert_eq!(artifact.classfile_bytes(), repeated.classfile_bytes());
        assert_eq!(artifact.tasty_bytes(), repeated.tasty_bytes());
        assert_eq!(artifact.tasty_link_id(), repeated.tasty_link_id());
        let tasty = TastyFile::parse_and_validate_scala_3_9(artifact.tasty_bytes()).unwrap();
        assert_eq!(
            (
                tasty.header().major_version,
                tasty.header().minor_version,
                tasty.header().experimental_version
            ),
            (28, 9, 0)
        );
        assert_eq!(
            artifact.tasty_link_id(),
            TastyLinkId::from_bytes(tasty.header().uuid)
        );
        if index == 2 {
            let (wrap_address, wrap) = tasty_method_body(&tasty, "wrap");
            assert_eq!(wrap.parameters.len(), 2);
            assert_eq!(
                wrap.parameters
                    .iter()
                    .map(|parameter| tasty.names().render(parameter.name()).unwrap())
                    .collect::<Vec<_>>(),
                ["first", "second"]
            );
            let address_index = tasty.ast_address_index().unwrap();
            let formal_addresses = tasty_parameter_addresses(&address_index, wrap_address);
            assert_eq!(formal_addresses.len(), 2);
            let RawTree::LengthNode(outer_node) = wrap.rhs.expect("wrap has a body") else {
                panic!("wrap body is an APPLY node");
            };
            assert_eq!(outer_node.tag, APPLY_TAG);
            let outer = outer_node.decode_apply().unwrap();
            assert_eq!(outer.arguments.len(), 2);
            for (argument, formal_address) in outer.arguments.iter().zip(formal_addresses) {
                assert_eq!(argument.ast_refs().len(), 1);
                assert_eq!(argument.ast_refs()[0].kind, AstRefKind::TermRefDirect);
                assert_eq!(argument.ast_refs()[0].address, formal_address);
                let RawTree::Leaf(term) = argument else {
                    panic!("forwarded formal is a direct TASTy reference leaf");
                };
                assert_eq!(term.tag, TERMREFDIRECT_TAG);
                assert!(
                    matches!(term.value, TermValue::AstRef(address) if address == formal_address)
                );
            }
        }
        let mut reader = Reader::new(artifact.classfile_bytes());
        let class = ClassFile::decode(&mut reader).unwrap();
        assert!(reader.is_at_end());
        assert_eq!((class.version.major, class.version.minor), (65, 0));
        assert_eq!(
            class.tasty_link_id().unwrap(),
            Some(artifact.tasty_link_id())
        );
        validate_classfile_tasty_link(&class, artifact.tasty_bytes()).unwrap();
        decoded.push(class);
    }
    assert_eq!(
        first
            .classes()
            .iter()
            .flat_map(|artifact| [artifact.classfile_path(), artifact.tasty_path()])
            .collect::<Vec<_>>(),
        [
            "example/Counter.class",
            "example/Counter.tasty",
            "example/Pair.class",
            "example/Pair.tasty",
            "example/PairFactory.class",
            "example/PairFactory.tasty",
        ]
    );
    let pair = &decoded[1];
    assert_eq!(
        pair.fields
            .iter()
            .map(|field| (
                pair.constant_pool.utf8(field.name_index).unwrap(),
                pair.constant_pool.utf8(field.descriptor_index).unwrap(),
            ))
            .collect::<Vec<_>>(),
        [
            ("left", "Lexample/Counter;"),
            ("right", "Lexample/Counter;")
        ]
    );
    let factory = &decoded[2];
    let wrap = factory
        .methods
        .iter()
        .find(|method| factory.constant_pool.utf8(method.name_index).unwrap() == "wrap")
        .unwrap();
    assert_eq!(
        factory.constant_pool.utf8(wrap.descriptor_index).unwrap(),
        "(Lexample/Counter;Lexample/Counter;)Lexample/Pair;"
    );
    let [Attribute::Code(code)] = wrap.attributes.as_slice() else {
        panic!("wrap has one Code attribute");
    };
    assert_eq!((code.max_stack, code.max_locals), (4, 3));
    let mut pc = 0;
    let allocation = take_batch_cp_operand(code.code, &mut pc, 0xbb);
    assert_eq!(batch_class_name(factory, allocation), "example/Pair");
    take_batch_opcode(code.code, &mut pc, 0x59);
    take_batch_opcode(code.code, &mut pc, 0x2b);
    take_batch_opcode(code.code, &mut pc, 0x2c);
    let constructor = take_batch_cp_operand(code.code, &mut pc, 0xb7);
    assert_eq!(
        batch_method_ref(factory, constructor),
        (
            "example/Pair",
            ("<init>", "(Lexample/Counter;Lexample/Counter;)V")
        )
    );
    take_batch_opcode(code.code, &mut pc, 0xb0);
    assert_eq!(pc, code.code.len());
    assert!(!factory.methods.iter().any(|method| {
        let name = factory.constant_pool.utf8(method.name_index).unwrap();
        name == "<init>"
            && factory
                .constant_pool
                .utf8(method.descriptor_index)
                .unwrap()
                .starts_with("(Lexample/Counter;")
    }));
}

#[test]
fn dual_reference_forwarding_renamed_and_reordered_batches_are_deterministic() {
    let renamed = DUAL_REFERENCE_FORWARDING_BATCH
        .replace("package example", "package renamed.example")
        .replace("PairFactory", "Assembler")
        .replace("Pair", "Envelope")
        .replace("Counter", "Gauge");
    let source = DUAL_REFERENCE_FORWARDING_BATCH;
    let counter_start = source.find("class Counter(start").unwrap();
    let pair_start = source.find("class Pair(first").unwrap();
    let factory_start = source.find("class PairFactory:").unwrap();
    let counter = &source[counter_start..pair_start];
    let pair = &source[pair_start..factory_start];
    let factory = &source[factory_start..];
    let reordered = format!("package example\n\n{factory}\n{pair}\n{counter}");

    for (variant, source_name, first_id, second_id, expected_paths) in [
        (
            renamed.as_str(),
            "Assembler.scala",
            19662,
            19663,
            [
                "renamed/example/Gauge.class",
                "renamed/example/Envelope.class",
                "renamed/example/Assembler.class",
            ],
        ),
        (
            reordered.as_str(),
            "PairFactory.scala",
            19664,
            19665,
            [
                "example/PairFactory.class",
                "example/Pair.class",
                "example/Counter.class",
            ],
        ),
    ] {
        let first =
            emit_paired_jvm_tasty_batch(variant, source_name, SourceId::from_index(first_id))
                .expect("the supported source variant emits a complete batch");
        let second =
            emit_paired_jvm_tasty_batch(variant, source_name, SourceId::from_index(second_id))
                .unwrap();
        assert_ne!(first.source_id(), second.source_id());
        assert_eq!(first.classes().len(), 3);
        assert_eq!(second.classes().len(), 3);
        for (index, (left, right)) in first.classes().iter().zip(second.classes()).enumerate() {
            assert_eq!(left.classfile_path(), expected_paths[index]);
            assert_eq!(left.classfile_bytes(), right.classfile_bytes());
            assert_eq!(left.tasty_path(), right.tasty_path());
            assert_eq!(left.tasty_bytes(), right.tasty_bytes());
            assert_eq!(left.tasty_link_id(), right.tasty_link_id());
        }
    }
}

#[test]
fn dual_reference_forwarding_contract_rejects_same_typed_swapped_formal_loads() {
    let artifacts = emit_paired_jvm_tasty_batch(
        DUAL_REFERENCE_FORWARDING_BATCH,
        "PairFactory.scala",
        SourceId::from_index(19666),
    )
    .unwrap();
    let mut batch_artifacts = JvmTastyBatchArtifacts {
        classfiles: artifacts
            .classes()
            .iter()
            .map(|artifact| JvmTastyClassFileArtifact {
                classfile_path: artifact.classfile_path().to_owned(),
                classfile_bytes: artifact.classfile_bytes().to_vec(),
            })
            .collect(),
        sidecars: artifacts
            .classes()
            .iter()
            .map(|artifact| JvmTastySidecarArtifact {
                tasty_path: artifact.tasty_path().to_owned(),
                tasty_bytes: artifact.tasty_bytes().to_vec(),
            })
            .collect(),
    };
    let factory = batch_artifacts
        .classfiles
        .iter_mut()
        .find(|artifact| artifact.classfile_path == "example/PairFactory.class")
        .unwrap();
    let mut reader = Reader::new(&factory.classfile_bytes);
    let class = ClassFile::decode(&mut reader).unwrap();
    let wrap = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == "wrap")
        .unwrap();
    let [Attribute::Code(code)] = wrap.attributes.as_slice() else {
        panic!("wrap has one Code attribute");
    };
    assert_eq!(&code.code[4..6], &[0x2b, 0x2c]);
    let code_start = code.code.as_ptr() as usize - factory.classfile_bytes.as_ptr() as usize;
    factory.classfile_bytes[code_start + 4] = 0x2c;
    factory.classfile_bytes[code_start + 5] = 0x2b;

    let error = with_paired_jvm_tasty_batch_plan(
        DUAL_REFERENCE_FORWARDING_BATCH,
        "PairFactory.scala",
        SourceId::from_index(19667),
        |plan| {
            let tasty_plans = plan
                .classes
                .iter()
                .map(|class| class.tasty_plan)
                .collect::<Vec<_>>();
            let validated = validate_jvm_tasty_batch(plan.jvm_unit, &tasty_plans).unwrap();
            validate_jvm_tasty_batch_artifacts(&validated, batch_artifacts).unwrap_err()
        },
    )
    .unwrap();
    assert!(
        matches!(
            error,
            JvmTastyContractError::BatchArtifact {
                phase: "cross-class invocation",
                ref reason,
                ..
        } if reason.contains("expected aload_1 in bounded call bytecode")
        ),
        "unexpected first rejection: {error:?}"
    );
}

#[test]
fn dual_reference_forwarding_publisher_rejects_each_six_file_fault_without_partial_output() {
    let batch = emit_paired_jvm_tasty_batch(
        DUAL_REFERENCE_FORWARDING_BATCH,
        "PairFactory.scala",
        SourceId::from_index(19668),
    )
    .unwrap();
    let pairs = batch_publish_pairs(&batch);
    assert_eq!(pairs.len(), 3);
    let root = TestDirectory::new("dual-reference-forwarding-publish-faults");
    for (index, fault) in (1..=6)
        .map(BatchPublishFault::AfterStagedFiles)
        .chain([
            BatchPublishFault::BeforePromotion,
            BatchPublishFault::AfterPromotion,
        ])
        .enumerate()
    {
        let destination = root.path().join(format!("failed-{index}"));
        assert!(matches!(
            publish_batch_pairs(&destination, &pairs, Some(fault)),
            Err(BatchPublishError::Io { .. })
        ));
        assert!(!destination.exists(), "fault case {index}");
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }

    let existing = root.path().join("existing-output");
    std::fs::create_dir(&existing).unwrap();
    let sentinel = existing.join("sentinel");
    std::fs::write(&sentinel, b"preserve prior output").unwrap();
    assert!(matches!(
        publish_batch_pairs(&existing, &pairs, None),
        Err(BatchPublishError::DestinationExists { .. })
    ));
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"preserve prior output");
    assert_eq!(read_relative_files(&existing).unwrap(), ["sentinel"]);
}

#[test]
fn sibling_constructor_batch_publication_contains_all_handled_failures() {
    let batch = emit_paired_jvm_tasty_batch(
        MULTI_ARGUMENT_CONSTRUCTOR_BATCH,
        "PairFactory.scala",
        SourceId::from_index(19172),
    )
    .unwrap();
    let pairs = batch_publish_pairs(&batch);
    let base = TestDirectory::new("sibling-constructor-publish-faults");
    assert_eq!(pairs.len(), 3);

    for (index, fault) in (1..=6)
        .map(BatchPublishFault::AfterStagedFiles)
        .chain([
            BatchPublishFault::BeforePromotion,
            BatchPublishFault::AfterPromotion,
        ])
        .enumerate()
    {
        let destination = base.path().join(format!("injected-failure-{index}"));
        assert!(matches!(
            publish_batch_pairs(&destination, &pairs, Some(fault)),
            Err(BatchPublishError::Io { .. })
        ));
        assert!(!destination.exists());
        assert_eq!(std::fs::read_dir(base.path()).unwrap().count(), 0);
    }

    let existing = base.path().join("existing-output");
    std::fs::create_dir(&existing).unwrap();
    let sentinel = existing.join("sentinel");
    std::fs::write(&sentinel, b"preserve existing destination").unwrap();
    assert!(matches!(
        publish_batch_pairs(&existing, &pairs, None),
        Err(BatchPublishError::DestinationExists { .. })
    ));
    assert_eq!(
        std::fs::read(&sentinel).unwrap(),
        b"preserve existing destination"
    );
    assert_eq!(read_relative_files(&existing).unwrap(), ["sentinel"]);
}

#[test]
fn sibling_constructor_batches_are_deterministic_after_renaming_and_reordering() {
    let source = MULTI_ARGUMENT_CONSTRUCTOR_BATCH;
    let counter_start = source.find("class Counter(start").unwrap();
    let pair_start = source.find("class Pair(first").unwrap();
    let factory_start = source.find("class PairFactory:").unwrap();
    let counter = &source[counter_start..pair_start];
    let pair = &source[pair_start..factory_start];
    let factory = &source[factory_start..];
    let reordered = format!("package example\n\n{factory}\n{pair}\n{counter}");
    let repeated_formal = source.replace(
        "new Counter(first), new Counter(second)",
        "new Counter(first), new Counter(first)",
    );
    let renamed = source
        .replace("package example", "package renamed.example")
        .replace("Counter", "Token")
        .replace("Pair", "Envelope");

    for (index, (variant, source_name, paths)) in [
        (
            renamed.as_str(),
            "EnvelopeFactory.scala",
            [
                "renamed/example/Token.class",
                "renamed/example/Envelope.class",
                "renamed/example/EnvelopeFactory.class",
            ],
        ),
        (
            reordered.as_str(),
            "PairFactory.scala",
            [
                "example/PairFactory.class",
                "example/Pair.class",
                "example/Counter.class",
            ],
        ),
        (
            repeated_formal.as_str(),
            "PairFactory.scala",
            [
                "example/Counter.class",
                "example/Pair.class",
                "example/PairFactory.class",
            ],
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let first = emit_paired_jvm_tasty_batch(
            variant,
            source_name,
            SourceId::from_index(19180 + index as u32 * 2),
        )
        .unwrap();
        let second = emit_paired_jvm_tasty_batch(
            variant,
            source_name,
            SourceId::from_index(19181 + index as u32 * 2),
        )
        .unwrap();
        assert_eq!(first.classes().len(), 3);
        for ((left, right), expected_classfile) in
            first.classes().iter().zip(second.classes()).zip(paths)
        {
            assert_eq!(left.classfile_path(), expected_classfile);
            assert_eq!(left.classfile_bytes(), right.classfile_bytes());
            assert_eq!(left.tasty_path(), right.tasty_path());
            assert_eq!(left.tasty_bytes(), right.tasty_bytes());
            assert_eq!(left.tasty_link_id(), right.tasty_link_id());
        }
    }
}

#[test]
fn sibling_constructor_batch_executes_original_rust_artifacts_with_independent_pair_identity() {
    if !require_exact_jdk_21_or_skip() {
        return;
    }

    let batch = emit_paired_jvm_tasty_batch(
        MULTI_ARGUMENT_CONSTRUCTOR_BATCH,
        "PairFactory.scala",
        SourceId::from_index(19190),
    )
    .expect("the original single-session three-class source emits six linked artifacts");
    assert_eq!(batch.classes().len(), 3);
    let stage = TestDirectory::new("sibling-constructor-runtime-identity");
    let classes = stage.path().join("original-rust-classes");
    let harness_sources = stage.path().join("harness-sources");
    let harness_classes = stage.path().join("harness-classes");
    std::fs::create_dir(&harness_sources).unwrap();
    std::fs::create_dir(&harness_classes).unwrap();

    let mut artifact_paths = Vec::with_capacity(6);
    for artifact in batch.classes() {
        for (relative_path, bytes) in [
            (artifact.classfile_path(), artifact.classfile_bytes()),
            (artifact.tasty_path(), artifact.tasty_bytes()),
        ] {
            let path = classes.join(relative_path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, bytes).unwrap();
            artifact_paths.push((path, relative_path.to_owned()));
        }
    }
    let original_hashes = artifact_paths
        .iter()
        .map(|(path, relative_path)| {
            let hash = sha256sum(path);
            eprintln!(
                "sibling-pair-original-sha256 {relative_path} {hash} tasty-link={:?}",
                batch
                    .classes()
                    .iter()
                    .find(|artifact| {
                        artifact.classfile_path() == relative_path
                            || artifact.tasty_path() == relative_path
                    })
                    .unwrap()
                    .tasty_link_id()
            );
            hash
        })
        .collect::<Vec<_>>();

    let harness = harness_sources.join("PairFactoryIdentityHarness.java");
    std::fs::write(&harness, PAIR_FACTORY_IDENTITY_HARNESS_JAVA).unwrap();
    let harness_source_sha256 = sha256sum(&harness);
    assert_eq!(harness_source_sha256, PAIR_FACTORY_IDENTITY_HARNESS_SHA256);
    eprintln!(
        "sibling-pair-independent-java-source-sha256 tests/fixtures/jvm_tasty_multi_argument_constructor_calls/PairFactoryIdentityHarness.java {harness_source_sha256}"
    );
    let javac = Command::new("javac")
        .args(["-classpath", classes.to_str().unwrap(), "-d"])
        .arg(&harness_classes)
        .arg(&harness)
        .output()
        .expect("start JDK 21 javac for the separate test-only Java harness");
    assert!(
        javac.status.success(),
        "javac rejected the separate identity harness:\n{}\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    for ((path, relative_path), expected_hash) in artifact_paths.iter().zip(&original_hashes) {
        assert_eq!(
            sha256sum(path),
            *expected_hash,
            "javac changed {relative_path}"
        );
    }

    let javap = Command::new("javap")
        .args(["-v", "-p", "-c", "-classpath", classes.to_str().unwrap()])
        .arg("example.PairFactory")
        .output()
        .expect("start JDK 21 javap on the original Rust PairFactory.class");
    assert!(
        javap.status.success(),
        "javap rejected original Rust PairFactory.class:\n{}",
        String::from_utf8_lossy(&javap.stderr)
    );
    let dump = String::from_utf8_lossy(&javap.stdout);
    for expected in [
        "major version: 65",
        "create(int, int)",
        "descriptor: (II)Lexample/Pair;",
        "new           #",
        "example/Counter",
        "example/Pair",
        "invokespecial",
    ] {
        assert!(
            dump.contains(expected),
            "javap omitted {expected:?}:\n{dump}"
        );
    }

    let classpath = std::env::join_paths([harness_classes.as_path(), classes.as_path()]).unwrap();
    let java = Command::new("java")
        .args(["-Xverify:all", "-ea", "-classpath"])
        .arg(&classpath)
        .arg("PairFactoryIdentityHarness")
        .output()
        .expect("start strict JDK 21 verification for the original Rust classfiles");
    assert!(
        java.status.success(),
        "java -Xverify:all -ea rejected the original Rust graph:\n{}\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "values=7/3,3/7,-9/42; independent=true; mutations=100/-5,-100/7"
    );
    for ((path, relative_path), expected_hash) in artifact_paths.iter().zip(&original_hashes) {
        assert_eq!(
            sha256sum(path),
            *expected_hash,
            "java changed {relative_path}"
        );
        let source_bytes = batch
            .classes()
            .iter()
            .find(|artifact| {
                artifact.classfile_path() == relative_path || artifact.tasty_path() == relative_path
            })
            .map(|artifact| {
                if artifact.classfile_path() == relative_path {
                    artifact.classfile_bytes()
                } else {
                    artifact.tasty_bytes()
                }
            })
            .unwrap();
        assert_eq!(
            std::fs::read(path).unwrap(),
            source_bytes,
            "JDK tools must not replace original Rust artifact {relative_path}"
        );
    }

    let factory = batch
        .classes()
        .iter()
        .find(|artifact| artifact.classfile_path() == "example/PairFactory.class")
        .unwrap();
    let mut decoded = Reader::new(factory.classfile_bytes());
    let class = ClassFile::decode(&mut decoded).unwrap();
    let create = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == "create")
        .unwrap();
    let [Attribute::Code(code)] = create.attributes.as_slice() else {
        panic!("PairFactory.create has one Code attribute");
    };
    let code_start = code.code.as_ptr() as usize - factory.classfile_bytes().as_ptr() as usize;
    let mut pc = 0;
    take_batch_cp_operand(code.code, &mut pc, 0xbb);
    take_batch_opcode(code.code, &mut pc, 0x59);
    take_batch_cp_operand(code.code, &mut pc, 0xbb);
    take_batch_opcode(code.code, &mut pc, 0x59);
    let first_load_pc = pc;
    take_batch_opcode(code.code, &mut pc, 0x1b);
    take_batch_cp_operand(code.code, &mut pc, 0xb7);
    take_batch_cp_operand(code.code, &mut pc, 0xbb);
    take_batch_opcode(code.code, &mut pc, 0x59);
    let second_load_pc = pc;
    take_batch_opcode(code.code, &mut pc, 0x1c);
    let second_constructor_pc = pc;
    take_batch_cp_operand(code.code, &mut pc, 0xb7);

    let swapped_classes = stage.path().join("swapped-order-test-copy");
    for artifact in batch.classes() {
        let path = swapped_classes.join(artifact.classfile_path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bytes = if artifact.classfile_path() == "example/PairFactory.class" {
            let mut bytes = artifact.classfile_bytes().to_vec();
            bytes[code_start + first_load_pc] = 0x1c;
            bytes[code_start + second_load_pc] = 0x1b;
            bytes
        } else {
            artifact.classfile_bytes().to_vec()
        };
        std::fs::write(path, bytes).unwrap();
    }
    let swapped_classpath =
        std::env::join_paths([harness_classes.as_path(), swapped_classes.as_path()]).unwrap();
    let swapped = Command::new("java")
        .args(["-Xverify:all", "-ea", "-classpath"])
        .arg(swapped_classpath)
        .arg("PairFactoryIdentityHarness")
        .output()
        .expect("start verifier-valid swapped-load negative control");
    let swapped_error = String::from_utf8_lossy(&swapped.stderr);
    assert!(
        !swapped.status.success(),
        "swapped loads preserve the wrong runtime values"
    );
    assert!(
        swapped_error.contains("AssertionError: expected 7, got 3"),
        "swapped Int loads must fail a runtime value assertion, not verification:\n{swapped_error}"
    );
    assert!(
        !swapped_error.contains("VerifyError") && !swapped_error.contains("LinkageError"),
        "swapped Int loads are JVM-valid and fail only semantic assertions:\n{swapped_error}"
    );

    let verifier_error_classes = stage.path().join("verify-error-test-copy");
    for artifact in batch.classes() {
        let path = verifier_error_classes.join(artifact.classfile_path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bytes = if artifact.classfile_path() == "example/PairFactory.class" {
            let mut bytes = artifact.classfile_bytes().to_vec();
            bytes[code_start + second_constructor_pc..code_start + second_constructor_pc + 3]
                .fill(0x00);
            bytes
        } else {
            artifact.classfile_bytes().to_vec()
        };
        std::fs::write(path, bytes).unwrap();
    }
    let verifier_error_classpath =
        std::env::join_paths([harness_classes.as_path(), verifier_error_classes.as_path()])
            .unwrap();
    let verifier_error = Command::new("java")
        .args(["-Xverify:all", "-ea", "-classpath"])
        .arg(verifier_error_classpath)
        .arg("PairFactoryIdentityHarness")
        .output()
        .expect("start strict JVM verification for the missing-init test copy");
    let verifier_error_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&verifier_error.stdout),
        String::from_utf8_lossy(&verifier_error.stderr)
    );
    assert!(!verifier_error.status.success());
    assert!(
        verifier_error_text.contains("VerifyError"),
        "missing inner initialization fails at JVM verification:\n{verifier_error_text}"
    );
    assert!(
        !verifier_error_text.contains("LinkageError")
            && !verifier_error_text.contains("AssertionError"),
        "missing inner initialization is a VerifyError, not linkage or a value assertion:\n{verifier_error_text}"
    );

    let pair_artifact = batch
        .classes()
        .iter()
        .find(|artifact| artifact.classfile_path() == "example/Pair.class")
        .unwrap();
    let pair_class = ClassFile::decode(&mut Reader::new(pair_artifact.classfile_bytes())).unwrap();
    let left_value = pair_class
        .methods
        .iter()
        .find(|method| pair_class.constant_pool.utf8(method.name_index).unwrap() == "leftValue")
        .unwrap();
    let [Attribute::Code(left_value_code)] = left_value.attributes.as_slice() else {
        panic!("Pair.leftValue has one Code attribute");
    };
    let mut left_value_pc = 0;
    take_batch_opcode(left_value_code.code, &mut left_value_pc, 0x2a);
    take_batch_cp_operand(left_value_code.code, &mut left_value_pc, 0xb4);
    let get_method_ref = take_batch_cp_operand(left_value_code.code, &mut left_value_pc, 0xb6);
    let Some(ConstantPoolEntry::Methodref {
        class_index: get_owner,
        name_and_type_index: get_name_and_type,
    }) = pair_class.constant_pool.get(get_method_ref)
    else {
        panic!("Pair.leftValue invokes a class Methodref");
    };
    assert_eq!(
        pair_class.constant_pool.class_name(*get_owner).unwrap(),
        "example/Counter"
    );
    assert_eq!(
        pair_class
            .constant_pool
            .name_and_type(*get_name_and_type)
            .unwrap(),
        ("get", "()I")
    );
    let pair_object_class = pair_class.super_class.unwrap();
    let get_method_ref_offset =
        classfile_constant_pool_entry_offset(pair_artifact.classfile_bytes(), get_method_ref);
    let linkage_error_classes = stage.path().join("linkage-error-test-copy");
    for artifact in batch.classes() {
        let path = linkage_error_classes.join(artifact.classfile_path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bytes = if artifact.classfile_path() == "example/Pair.class" {
            let mut bytes = artifact.classfile_bytes().to_vec();
            bytes[get_method_ref_offset + 1..get_method_ref_offset + 3]
                .copy_from_slice(&pair_object_class.0.to_be_bytes());
            bytes
        } else {
            artifact.classfile_bytes().to_vec()
        };
        std::fs::write(path, bytes).unwrap();
    }
    let linkage_error_classpath =
        std::env::join_paths([harness_classes.as_path(), linkage_error_classes.as_path()]).unwrap();
    let linkage_error = Command::new("java")
        .args(["-Xverify:all", "-ea", "-classpath"])
        .arg(linkage_error_classpath)
        .arg("PairFactoryIdentityHarness")
        .output()
        .expect("start JVM on the unresolved Methodref test copy");
    let linkage_error_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&linkage_error.stdout),
        String::from_utf8_lossy(&linkage_error.stderr)
    );
    assert!(!linkage_error.status.success());
    assert!(
        linkage_error_text.contains("NoSuchMethodError"),
        "a verifier-valid unresolved Methodref fails during JVM linkage:\n{linkage_error_text}"
    );
    assert!(
        !linkage_error_text.contains("VerifyError")
            && !linkage_error_text.contains("AssertionError"),
        "the unresolved Methodref is a linkage failure, not verification or a semantic assertion:\n{linkage_error_text}"
    );

    for ((path, relative_path), expected_hash) in artifact_paths.iter().zip(&original_hashes) {
        assert_eq!(
            sha256sum(path),
            *expected_hash,
            "negative test copies must not alter original {relative_path}"
        );
    }
}

#[test]
fn sibling_constructor_batch_contract_rejects_bytecode_and_link_corruption() {
    let batch = emit_paired_jvm_tasty_batch(
        MULTI_ARGUMENT_CONSTRUCTOR_BATCH,
        "PairFactory.scala",
        SourceId::from_index(19173),
    )
    .unwrap();
    let valid_artifacts = JvmTastyBatchArtifacts {
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
    };

    with_paired_jvm_tasty_batch_plan(
        MULTI_ARGUMENT_CONSTRUCTOR_BATCH,
        "PairFactory.scala",
        SourceId::from_index(19174),
        |plan| {
            let tasty_plans = plan
                .classes
                .iter()
                .map(|class| class.tasty_plan)
                .collect::<Vec<_>>();
            let validated = validate_jvm_tasty_batch(plan.jvm_unit, &tasty_plans).unwrap();
            validate_jvm_tasty_batch_artifacts(&validated, valid_artifacts.clone()).unwrap();

            let factory = valid_artifacts
                .classfiles
                .iter()
                .find(|artifact| artifact.classfile_path == "example/PairFactory.class")
                .unwrap();
            let mut reader = Reader::new(&factory.classfile_bytes);
            let decoded = ClassFile::decode(&mut reader).unwrap();
            let create = decoded
                .methods
                .iter()
                .find(|method| decoded.constant_pool.utf8(method.name_index).unwrap() == "create")
                .unwrap();
            let [Attribute::Code(code)] = create.attributes.as_slice() else {
                panic!("PairFactory.create has one Code attribute");
            };
            let code_start =
                code.code.as_ptr() as usize - factory.classfile_bytes.as_ptr() as usize;
            let mut pc = 0;
            let outer_allocation = take_batch_cp_operand(code.code, &mut pc, 0xbb);
            take_batch_opcode(code.code, &mut pc, 0x59);
            let first_allocation_opcode = pc;
            let first_allocation = take_batch_cp_operand(code.code, &mut pc, 0xbb);
            take_batch_opcode(code.code, &mut pc, 0x59);
            let first_load_opcode = pc;
            take_batch_opcode(code.code, &mut pc, 0x1b);
            let first_constructor_opcode = pc;
            take_batch_cp_operand(code.code, &mut pc, 0xb7);
            let second_allocation_opcode = pc;
            let second_allocation = take_batch_cp_operand(code.code, &mut pc, 0xbb);
            take_batch_opcode(code.code, &mut pc, 0x59);
            let second_load_opcode = pc;
            take_batch_opcode(code.code, &mut pc, 0x1c);
            let second_constructor_opcode = pc;
            let second_constructor_ref = take_batch_cp_operand(code.code, &mut pc, 0xb7);
            let outer_constructor_opcode = pc;
            let outer_constructor_ref = take_batch_cp_operand(code.code, &mut pc, 0xb7);

            let mut cases = Vec::new();
            let mut missing_first_new = valid_artifacts.clone();
            sibling_factory_classfile_mut(&mut missing_first_new).classfile_bytes
                [code_start + first_allocation_opcode] = 0x00;
            cases.push(("missing-first-inner-new", missing_first_new));

            let mut missing_second_new = valid_artifacts.clone();
            sibling_factory_classfile_mut(&mut missing_second_new).classfile_bytes
                [code_start + second_allocation_opcode] = 0x00;
            cases.push(("missing-second-inner-new", missing_second_new));

            let mut missing_inner_init = valid_artifacts.clone();
            sibling_factory_classfile_mut(&mut missing_inner_init).classfile_bytes
                [code_start + second_constructor_opcode] = 0x00;
            cases.push(("missing-second-inner-init", missing_inner_init));

            let mut missing_first_inner_init = valid_artifacts.clone();
            sibling_factory_classfile_mut(&mut missing_first_inner_init).classfile_bytes
                [code_start + first_constructor_opcode] = 0x00;
            cases.push(("missing-first-inner-init", missing_first_inner_init));

            let mut premature_outer_init = valid_artifacts.clone();
            let classfile =
                &mut sibling_factory_classfile_mut(&mut premature_outer_init).classfile_bytes;
            classfile[code_start + second_constructor_opcode + 1
                ..code_start + second_constructor_opcode + 3]
                .copy_from_slice(&outer_constructor_ref.0.to_be_bytes());
            cases.push(("premature-outer-init", premature_outer_init));

            let mut duplicate_inner_init_reference = valid_artifacts.clone();
            let classfile = &mut sibling_factory_classfile_mut(&mut duplicate_inner_init_reference)
                .classfile_bytes;
            classfile[code_start + outer_constructor_opcode + 1
                ..code_start + outer_constructor_opcode + 3]
                .copy_from_slice(&second_constructor_ref.0.to_be_bytes());
            cases.push((
                "outer-call-reuses-inner-methodref",
                duplicate_inner_init_reference,
            ));

            let mut swapped_loads = valid_artifacts.clone();
            let classfile = &mut sibling_factory_classfile_mut(&mut swapped_loads).classfile_bytes;
            classfile[code_start + first_load_opcode] = 0x1c;
            classfile[code_start + second_load_opcode] = 0x1b;
            cases.push(("swapped-second-formal-load", swapped_loads));

            let mut duplicated_load = valid_artifacts.clone();
            sibling_factory_classfile_mut(&mut duplicated_load).classfile_bytes
                [code_start + second_load_opcode] = 0x1b;
            cases.push(("duplicated-second-formal-load", duplicated_load));

            let mut wrong_inner_owner = valid_artifacts.clone();
            let classfile =
                &mut sibling_factory_classfile_mut(&mut wrong_inner_owner).classfile_bytes;
            classfile[code_start + second_constructor_opcode + 1
                ..code_start + second_constructor_opcode + 3]
                .copy_from_slice(&outer_constructor_ref.0.to_be_bytes());
            cases.push(("wrong-second-inner-constructor", wrong_inner_owner));

            let mut wrong_outer_descriptor = valid_artifacts.clone();
            let classfile =
                &mut sibling_factory_classfile_mut(&mut wrong_outer_descriptor).classfile_bytes;
            classfile[code_start + outer_constructor_opcode + 1
                ..code_start + outer_constructor_opcode + 3]
                .copy_from_slice(&second_constructor_ref.0.to_be_bytes());
            cases.push(("wrong-outer-constructor-descriptor", wrong_outer_descriptor));

            let mut wrong_stack = valid_artifacts.clone();
            sibling_factory_classfile_mut(&mut wrong_stack).classfile_bytes
                [code_start - 8..code_start - 6]
                .copy_from_slice(&[0, 5]);
            cases.push(("wrong-max-stack", wrong_stack));

            let mut wrong_locals = valid_artifacts.clone();
            sibling_factory_classfile_mut(&mut wrong_locals).classfile_bytes
                [code_start - 6..code_start - 4]
                .copy_from_slice(&[0, 2]);
            cases.push(("wrong-max-locals", wrong_locals));

            for (label, corrupted) in cases {
                let error = validate_jvm_tasty_batch_artifacts(&validated, corrupted)
                    .expect_err("corrupt paired-constructor code fails batch validation");
                assert!(
                    matches!(
                        error,
                        JvmTastyContractError::BatchArtifact {
                            class_index: 2,
                            phase: "cross-class invocation",
                            ..
                        }
                    ),
                    "{label} must fail at the paired call bytecode stage: {error:?}"
                );
            }

            let operand =
                |index: ConstantPoolIndex| ConstantPoolIndexOperand::try_from(index.0).unwrap();
            let mut third_live_allocation = CodeSequence::new();
            third_live_allocation
                .instruction(Instruction::New(ClassReference::new(operand(
                    outer_allocation,
                ))))
                .instruction(Instruction::Dup)
                .instruction(Instruction::New(ClassReference::new(operand(
                    first_allocation,
                ))))
                .instruction(Instruction::Dup)
                .instruction(Instruction::New(ClassReference::new(operand(
                    second_allocation,
                ))))
                .instruction(Instruction::Dup)
                .instruction(Instruction::Return);
            let graph = ControlFlowGraph::build(&third_live_allocation).unwrap();
            assert!(
                matches!(
                    CfgCodeAnalyzer::analyze_with_current_class(
                        &graph,
                        "create",
                        "(II)Lexample/Pair;",
                        false,
                        &decoded.constant_pool,
                        decoded.this_class,
                    ),
                    Err(AnalysisError::TooManyLiveAllocations {
                        instruction: 4,
                        limit: 2,
                        ..
                    })
                ),
                "third simultaneously uninitialized object fails in CFG analysis"
            );

            let mut escaped_uninitialized = CodeSequence::new();
            escaped_uninitialized
                .instruction(Instruction::New(ClassReference::new(operand(
                    first_allocation,
                ))))
                .instruction(Instruction::AStore(LocalIndex::new(3)))
                .instruction(Instruction::Return);
            let graph = ControlFlowGraph::build(&escaped_uninitialized).unwrap();
            assert!(
                matches!(
                    CfgCodeAnalyzer::analyze_with_current_class(
                        &graph,
                        "create",
                        "(II)Lexample/Pair;",
                        false,
                        &decoded.constant_pool,
                        decoded.this_class,
                    ),
                    Err(AnalysisError::UninitializedReferenceUse {
                        instruction: 1,
                        allocation_site: 0,
                    })
                ),
                "uninitialized allocation cannot escape to a local"
            );

            let mut truncated = valid_artifacts.clone();
            let classfile = &mut sibling_factory_classfile_mut(&mut truncated).classfile_bytes;
            classfile.truncate(code_start + second_constructor_opcode + 1);
            assert!(matches!(
                validate_jvm_tasty_batch_artifacts(&validated, truncated),
                Err(JvmTastyContractError::BatchArtifact {
                    class_index: 2,
                    phase: "class-file decode",
                    ..
                })
            ));

            let mut missing_sidecar = valid_artifacts.clone();
            missing_sidecar
                .sidecars
                .retain(|sidecar| sidecar.tasty_path != "example/PairFactory.tasty");
            assert!(matches!(
                validate_jvm_tasty_batch_artifacts(&validated, missing_sidecar),
                Err(JvmTastyContractError::BatchArtifactPaths {
                    reason: "missing expected TASTy sidecar path",
                    ..
                })
            ));

            let mut missing_classfile = valid_artifacts.clone();
            missing_classfile
                .classfiles
                .retain(|classfile| classfile.classfile_path != "example/PairFactory.class");
            assert!(matches!(
                validate_jvm_tasty_batch_artifacts(&validated, missing_classfile),
                Err(JvmTastyContractError::BatchArtifactPaths {
                    reason: "missing expected class-file path",
                    ..
                })
            ));

            let mut duplicate_path = valid_artifacts.clone();
            duplicate_path
                .classfiles
                .iter_mut()
                .find(|classfile| classfile.classfile_path == "example/PairFactory.class")
                .unwrap()
                .classfile_path = "example/Pair.class".to_owned();
            assert!(matches!(
                validate_jvm_tasty_batch_artifacts(&validated, duplicate_path),
                Err(JvmTastyContractError::BatchArtifactPaths {
                    reason: "duplicate artifact path",
                    ..
                })
            ));

            let mut unsafe_path = valid_artifacts.clone();
            unsafe_path
                .classfiles
                .iter_mut()
                .find(|classfile| classfile.classfile_path == "example/PairFactory.class")
                .unwrap()
                .classfile_path = "../PairFactory.class".to_owned();
            assert!(matches!(
                validate_jvm_tasty_batch_artifacts(&validated, unsafe_path),
                Err(JvmTastyContractError::BatchArtifactPaths { .. })
            ));

            let mut corrupt_sidecar = valid_artifacts.clone();
            corrupt_sidecar
                .sidecars
                .iter_mut()
                .find(|sidecar| sidecar.tasty_path == "example/PairFactory.tasty")
                .unwrap()
                .tasty_bytes = vec![0, 1, 2];
            assert!(matches!(
                validate_jvm_tasty_batch_artifacts(&validated, corrupt_sidecar),
                Err(JvmTastyContractError::BatchArtifact {
                    class_index: 2,
                    phase: "class-file/TASTy link",
                    ..
                })
            ));

            let mut stale_link = valid_artifacts;
            let classfile = &mut sibling_factory_classfile_mut(&mut stale_link).classfile_bytes;
            let link_id = batch.classes()[2].tasty_link_id();
            let uuid = link_id.as_bytes();
            let uuid_offset = classfile
                .windows(uuid.len())
                .position(|window| window == uuid)
                .unwrap();
            classfile[uuid_offset] ^= 0x80;
            assert!(matches!(
                validate_jvm_tasty_batch_artifacts(&validated, stale_link),
                Err(JvmTastyContractError::BatchArtifact {
                    class_index: 2,
                    phase: "class-file/TASTy link",
                    ..
                })
            ));
            Ok::<_, JvmTastyContractError>(())
        },
    )
    .unwrap()
    .unwrap();
}

#[test]
fn linked_counter_reader_batch_executes_on_verified_jdk21() {
    if !require_jdk_21_or_skip() {
        return;
    }

    let batch = emit_paired_jvm_tasty_batch(
        COUNTER_READER,
        "tests/fixtures/jvm_tasty_cross_class/Reader.scala",
        SourceId::from_index(1676),
    )
    .unwrap();
    let output = TestDirectory::new("cross-class-jdk-runtime");
    for pair in batch.classes() {
        let classfile = output.path().join(pair.classfile_path());
        let tasty = output.path().join(pair.tasty_path());
        std::fs::create_dir_all(classfile.parent().unwrap()).unwrap();
        std::fs::write(&classfile, pair.classfile_bytes()).unwrap();
        std::fs::write(&tasty, pair.tasty_bytes()).unwrap();
    }

    let harness = output.path().join("CrossClassRuntimeHarness.java");
    std::fs::write(
        &harness,
        r#"public final class CrossClassRuntimeHarness {
    public static void main(String[] args) {
        example.Counter first = new example.Counter(7);
        example.Counter second = new example.Counter(-5);
        example.Reader reader = new example.Reader();
        expect(reader.read(first), 7);
        first.set(13);
        expect(reader.read(first), 13);
        first.set(-42);
        expect(reader.read(first), -42);
        expect(reader.read(second), -5);
        second.set(99);
        expect(reader.read(first), -42);
        expect(reader.read(second), 99);
        System.out.println("7,13,-42; independent=-5,99");
    }

    private static void expect(int actual, int expected) {
        if (actual != expected) {
            throw new AssertionError("expected " + expected + ", got " + actual);
        }
    }
}
"#,
    )
    .unwrap();
    let javac = Command::new("javac")
        .arg("-classpath")
        .arg(output.path())
        .arg("-d")
        .arg(output.path())
        .arg(&harness)
        .output()
        .expect("start JDK 21 javac for the Rust artifact harness");
    assert!(
        javac.status.success(),
        "javac rejected the Rust Counter/Reader batch:\n{}",
        String::from_utf8_lossy(&javac.stderr)
    );

    let javap = Command::new("javap")
        .args([
            "-classpath",
            output.path().to_str().unwrap(),
            "-p",
            "-s",
            "-c",
            "-v",
        ])
        .arg("example.Reader")
        .output()
        .expect("start JDK 21 javap for the Rust Reader class");
    assert!(
        javap.status.success(),
        "javap rejected Rust Reader.class:\n{}",
        String::from_utf8_lossy(&javap.stderr)
    );
    let javap = String::from_utf8_lossy(&javap.stdout);
    assert!(javap.contains("major version: 65"), "{javap}");
    let read_details = javap
        .split("public int read(example.Counter);")
        .nth(1)
        .expect("javap shows Reader.read as a public instance method");
    for expected in [
        "descriptor: (Lexample/Counter;)I",
        "flags: (0x0001) ACC_PUBLIC",
        "invokevirtual",
        "Method example/Counter.get:()I",
    ] {
        assert!(
            read_details.contains(expected),
            "javap Reader.read section omitted {expected:?}:\n{read_details}"
        );
    }

    let java = Command::new("java")
        .args(["-Xverify:all", "-classpath"])
        .arg(output.path())
        .arg("CrossClassRuntimeHarness")
        .output()
        .expect("start JDK 21 verifier and Rust artifact harness");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected or failed the Rust Counter/Reader batch:\n{}\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "7,13,-42; independent=-5,99"
    );

    for pair in batch.classes() {
        assert_eq!(
            std::fs::read(output.path().join(pair.classfile_path())).unwrap(),
            pair.classfile_bytes(),
            "JDK tools must not replace or mutate Rust-produced classfiles"
        );
    }
}

fn require_jdk_21_or_skip() -> bool {
    let unavailable = ["java", "javac", "javap"]
        .into_iter()
        .filter_map(|tool| match jdk_tool_major_version(tool) {
            Some(version) if version >= 21 => None,
            Some(version) => Some(format!("{tool} {version} (requires JDK 21+)")),
            None => Some(format!("{tool} (missing or version unavailable)")),
        })
        .collect::<Vec<_>>();
    if unavailable.is_empty() {
        return true;
    }
    let message = format!(
        "linked JVM batch runtime requires java, javac, and javap from JDK 21+ on PATH; unavailable: {}",
        unavailable.join(", ")
    );
    if std::env::var("DOTTY_CLASSFILE_REQUIRE_JDK").as_deref() == Ok("1") {
        panic!("{message}");
    }
    eprintln!("skipping optional linked JVM batch JDK runtime: {message}");
    false
}

fn require_exact_jdk_21_or_skip() -> bool {
    let unavailable = ["java", "javac", "javap"]
        .into_iter()
        .filter_map(|tool| match jdk_tool_major_version(tool) {
            Some(21) => None,
            Some(version) => Some(format!("{tool} {version} (requires JDK 21 exactly)")),
            None => Some(format!("{tool} (missing or version unavailable)")),
        })
        .collect::<Vec<_>>();
    if unavailable.is_empty() {
        return true;
    }
    let message = format!(
        "nested constructor acceptance requires java, javac, and javap from JDK 21 exactly on PATH; unavailable: {}",
        unavailable.join(", ")
    );
    if std::env::var("DOTTY_CLASSFILE_REQUIRE_JDK").as_deref() == Ok("1") {
        panic!("{message}");
    }
    eprintln!("skipping optional nested constructor JDK 21 acceptance: {message}");
    false
}

fn jdk_tool_major_version(tool: &str) -> Option<u32> {
    let output = Command::new(tool).arg("-version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let version_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let version_line = version_text.lines().find_map(|line| {
        let line = line.trim();
        if line.starts_with("openjdk version ") || line.starts_with("java version ") {
            line.split('"').nth(1)
        } else if let Some(version) = line.strip_prefix("javac ") {
            Some(version)
        } else if tool == "javap"
            && line
                .chars()
                .all(|character| character.is_ascii_digit() || character == '.')
        {
            Some(line)
        } else {
            None
        }
    })?;
    let components = version_line.split('.').collect::<Vec<_>>();
    let major = if components.first() == Some(&"1") {
        *components.get(1)?
    } else {
        *components.first()?
    };
    major.parse().ok()
}

#[test]
fn fresh_batch_sessions_produce_byte_identical_complete_manifests() {
    let first = emit_paired_jvm_tasty_batch(
        COUNTER_GAUGE,
        "CounterGauge.scala",
        SourceId::from_index(320),
    )
    .unwrap();
    let second = emit_paired_jvm_tasty_batch(
        COUNTER_GAUGE,
        "CounterGauge.scala",
        SourceId::from_index(321),
    )
    .unwrap();

    assert_ne!(first.source_id(), second.source_id());
    assert_eq!(first.classes().len(), second.classes().len());
    for (left, right) in first.classes().iter().zip(second.classes()) {
        assert_eq!(left.classfile_path(), right.classfile_path());
        assert_eq!(left.classfile_bytes(), right.classfile_bytes());
        assert_eq!(left.tasty_path(), right.tasty_path());
        assert_eq!(left.tasty_bytes(), right.tasty_bytes());
        assert_eq!(left.tasty_link_id(), right.tasty_link_id());
    }
}

#[test]
fn unsupported_second_class_rejects_the_entire_batch() {
    let source = "package example\n\nclass Counter(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n\nclass Gauge(start: Int):\n  private var reading: Int = start\n  def get(): Int = reading\n  def set(value: Int): Unit = while reading < value do reading = reading + 1\n";
    assert!(matches!(
        emit_paired_jvm_tasty_batch(source, "CounterGauge.scala", SourceId::from_index(322)),
        Err(PairedBatchArtifactError::Plan(
            PairedPlanError::BatchClass { class_index: 1, .. }
        ))
    ));
}

#[test]
fn primitive_root_call_does_not_masquerade_as_a_cross_class_batch_call() {
    let source = COUNTER_GAUGE.replacen(
        "def get(): Int = current",
        "def get(): Int = current + 1",
        1,
    );
    let batch =
        emit_paired_jvm_tasty_batch(&source, "CounterGauge.scala", SourceId::from_index(1677))
            .expect("primitive arithmetic in a batched source class remains supported");

    assert_eq!(batch.classes().len(), 2);
    assert_eq!(batch.classes()[0].classfile_path(), "example/Counter.class");
    assert_eq!(batch.classes()[1].classfile_path(), "example/Gauge.class");
}

#[test]
fn nested_cross_class_call_is_rejected_outside_the_bounded_invocation_shape() {
    let source = COUNTER_READER.replace("counter.get()", "counter.get() + 1");
    let error = emit_paired_jvm_tasty_batch(
        &source,
        "tests/fixtures/jvm_tasty_cross_class/Reader.scala",
        SourceId::from_index(1678),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        PairedBatchArtifactError::Contract(JvmTastyContractError::BatchArtifact {
            class_index: 1,
            phase: "cross-class invocation",
            ..
        })
    ));
}

#[test]
fn unsupported_method_body_rejects_before_returning_either_artifact() {
    let source = "package example\nclass Counter(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = while current < value do current = current + 1\n";
    assert!(matches!(
        emit_paired_jvm_tasty_class(source, "Counter.scala", SourceId::from_index(313)),
        Err(PairedArtifactError::Plan(_))
    ));
}

#[test]
fn pair_publisher_stages_both_artifacts_before_exposing_them() {
    let pair =
        emit_paired_jvm_tasty_class(COUNTER, "Counter.scala", SourceId::from_index(314)).unwrap();
    let output = TestDirectory::new("publish-success");
    publish_pair(output.path(), &pair, None).unwrap();

    assert_eq!(
        std::fs::read(output.path().join(pair.classfile_path())).unwrap(),
        pair.classfile_bytes()
    );
    assert_eq!(
        std::fs::read(output.path().join(pair.tasty_path())).unwrap(),
        pair.tasty_bytes()
    );
    assert_eq!(std::fs::read_dir(output.path()).unwrap().count(), 1);
}

#[test]
fn pair_publisher_cleans_staging_after_failure_following_first_staged_artifact() {
    let pair =
        emit_paired_jvm_tasty_class(COUNTER, "Counter.scala", SourceId::from_index(315)).unwrap();
    let output = TestDirectory::new("publish-failure");

    assert!(publish_pair(output.path(), &pair, Some(1)).is_err());
    assert_eq!(std::fs::read_dir(output.path()).unwrap().count(), 0);
}

#[test]
fn batch_publisher_promotes_the_complete_manifest_and_preserves_every_byte() {
    let batch = emitted_batch(323);
    let base = TestDirectory::new("batch-publish-success");
    let destination = base.path().join("classes");

    let published = publish_paired_batch(&destination, &batch, None).unwrap();
    let expected = [
        "example/Counter.class",
        "example/Counter.tasty",
        "example/Gauge.class",
        "example/Gauge.tasty",
    ];
    assert_eq!(
        published
            .iter()
            .map(|path| path.strip_prefix(&destination).unwrap().to_str().unwrap())
            .collect::<Vec<_>>(),
        expected
    );
    let inventory = read_relative_files(&destination).unwrap();
    assert_eq!(inventory, expected.map(str::to_owned));
    for pair in batch.classes() {
        assert_eq!(
            std::fs::read(destination.join(pair.classfile_path())).unwrap(),
            pair.classfile_bytes()
        );
        assert_eq!(
            std::fs::read(destination.join(pair.tasty_path())).unwrap(),
            pair.tasty_bytes()
        );
    }

    let sentinel = destination.join("example/Counter.class");
    let original = std::fs::read(&sentinel).unwrap();
    assert!(matches!(
        publish_paired_batch(&destination, &batch, None),
        Err(BatchPublishError::DestinationExists { .. })
    ));
    assert_eq!(std::fs::read(sentinel).unwrap(), original);
}

#[test]
fn malformed_second_sidecar_fails_before_creating_staging_or_destination() {
    let batch = emitted_batch(324);
    let mut pairs = batch_publish_pairs(&batch);
    let malformed: [u8; 0] = [];
    pairs[1].tasty_bytes = &malformed;
    let base = TestDirectory::new("batch-publish-invalid");
    let destination = base.path().join("classes");

    assert!(matches!(
        publish_batch_pairs(&destination, &pairs, None),
        Err(BatchPublishError::InvalidManifest {
            class_index: 1,
            phase: "TASTy decode",
            ..
        })
    ));
    assert!(!destination.exists());
    assert_eq!(std::fs::read_dir(base.path()).unwrap().count(), 0);
}

#[test]
fn batch_publisher_enforces_class_and_byte_bounds_before_decoding_or_writing() {
    let batch = emitted_batch(329);
    let base = TestDirectory::new("batch-publish-bounds");
    let original_pairs = batch_publish_pairs(&batch);

    let mut too_many = original_pairs.iter().take(1).cloned().collect::<Vec<_>>();
    too_many.resize_with(MAX_BATCH_CLASSES + 1, || BatchPublishPair {
        class: original_pairs[0].class,
        source_id: original_pairs[0].source_id,
        classfile_path: original_pairs[0].classfile_path,
        classfile_bytes: original_pairs[0].classfile_bytes,
        tasty_path: original_pairs[0].tasty_path,
        tasty_bytes: original_pairs[0].tasty_bytes,
    });
    let too_many_destination = base.path().join("too-many");
    assert!(matches!(
        publish_batch_pairs(&too_many_destination, &too_many, None),
        Err(BatchPublishError::InvalidManifest {
            phase: "manifest shape",
            ..
        })
    ));
    assert!(!too_many_destination.exists());

    let oversized = vec![0; MAX_BATCH_ARTIFACT_BYTES + 1];
    let mut too_large = original_pairs;
    too_large[0].classfile_bytes = &oversized;
    let too_large_destination = base.path().join("too-large");
    assert!(matches!(
        publish_batch_pairs(&too_large_destination, &too_large, None),
        Err(BatchPublishError::InvalidManifest {
            phase: "manifest size",
            ..
        })
    ));
    assert!(!too_large_destination.exists());
    assert_eq!(std::fs::read_dir(base.path()).unwrap().count(), 0);
}

#[test]
fn batch_publisher_cleans_every_staged_artifact_on_handled_failures() {
    let batch = emitted_batch(325);
    let base = TestDirectory::new("batch-publish-faults");
    for (index, fault) in [
        BatchPublishFault::AfterStagedFiles(1),
        BatchPublishFault::AfterStagedFiles(2),
        BatchPublishFault::AfterStagedFiles(3),
        BatchPublishFault::AfterStagedFiles(4),
        BatchPublishFault::BeforePromotion,
        BatchPublishFault::AfterPromotion,
    ]
    .into_iter()
    .enumerate()
    {
        let destination = base.path().join(format!("classes-{index}"));
        let error = publish_paired_batch(&destination, &batch, Some(fault)).unwrap_err();
        assert!(matches!(error, BatchPublishError::Io { .. }));
        assert!(!destination.exists(), "fault case {index}: {error:?}");
        assert_eq!(std::fs::read_dir(base.path()).unwrap().count(), 0);
    }
}

#[test]
fn batch_publisher_rejects_unsafe_or_conflicting_paths_before_writing() {
    let batch = emitted_batch(326);
    let base = TestDirectory::new("batch-publish-paths");
    let bad_paths = [
        "../Gauge.class",
        "/absolute/Gauge.class",
        "C:/drive/Gauge.class",
        "example\\Gauge.class",
        "example//Gauge.class",
        "example/./Gauge.class",
        "example/Gauge.class/",
    ];
    for (index, path) in bad_paths.into_iter().enumerate() {
        let mut pairs = batch_publish_pairs(&batch);
        pairs[1].classfile_path = path;
        let destination = base.path().join(format!("unsafe-{index}"));
        assert!(matches!(
            publish_batch_pairs(&destination, &pairs, None),
            Err(BatchPublishError::InvalidManifest { .. })
        ));
        assert!(!destination.exists());
    }

    let mut duplicates = batch_publish_pairs(&batch);
    duplicates[1].classfile_path = duplicates[0].classfile_path;
    let duplicate_destination = base.path().join("duplicate");
    assert!(matches!(
        publish_batch_pairs(&duplicate_destination, &duplicates, None),
        Err(BatchPublishError::InvalidManifest {
            phase: "artifact paths",
            ..
        })
    ));
    assert!(!duplicate_destination.exists());

    let mut case_collisions = batch_publish_pairs(&batch);
    case_collisions[1].classfile_path = "example/counter.class";
    let case_collision_destination = base.path().join("case-collision");
    assert!(matches!(
        publish_batch_pairs(&case_collision_destination, &case_collisions, None),
        Err(BatchPublishError::InvalidManifest {
            phase: "artifact paths",
            ..
        })
    ));
    assert!(!case_collision_destination.exists());
    assert_eq!(std::fs::read_dir(base.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn batch_publisher_rejects_symlinked_output_parent_without_following_it() {
    use std::os::unix::fs::symlink;

    let batch = emitted_batch(327);
    let base = TestDirectory::new("batch-publish-symlink");
    let real_parent = base.path().join("real");
    std::fs::create_dir(&real_parent).unwrap();
    let linked_parent = base.path().join("linked");
    symlink(&real_parent, &linked_parent).unwrap();
    let destination = linked_parent.join("classes");

    assert!(matches!(
        publish_paired_batch(&destination, &batch, None),
        Err(BatchPublishError::InvalidDestination { .. })
    ));
    assert!(!destination.exists());
    assert_eq!(std::fs::read_dir(&real_parent).unwrap().count(), 0);
}

#[test]
fn batch_publisher_leaves_a_preexisting_output_tree_untouched() {
    let batch = emitted_batch(328);
    let base = TestDirectory::new("batch-publish-existing");
    let destination = base.path().join("classes");
    std::fs::create_dir(&destination).unwrap();
    let sentinel = destination.join("sentinel");
    std::fs::write(&sentinel, b"keep this file").unwrap();

    assert!(matches!(
        publish_paired_batch(&destination, &batch, None),
        Err(BatchPublishError::DestinationExists { .. })
    ));
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"keep this file");
    assert_eq!(read_relative_files(&destination).unwrap(), ["sentinel"]);
}

#[test]
fn equal_simple_names_in_different_packages_keep_distinct_pair_paths() {
    let left = COUNTER.replace("package example", "package left");
    let right = COUNTER.replace("package example", "package right");
    let left =
        emit_paired_jvm_tasty_class(&left, "Counter.scala", SourceId::from_index(316)).unwrap();
    let right =
        emit_paired_jvm_tasty_class(&right, "Counter.scala", SourceId::from_index(317)).unwrap();

    assert_eq!(left.classfile_path(), "left/Counter.class");
    assert_eq!(left.tasty_path(), "left/Counter.tasty");
    assert_eq!(right.classfile_path(), "right/Counter.class");
    assert_eq!(right.tasty_path(), "right/Counter.tasty");
}

#[test]
fn rust_factory_counter_batch_executes_with_verified_reference_returns_on_jdk21() {
    if !require_jdk_21_or_skip() {
        return;
    }

    let first = emit_paired_jvm_tasty_batch(
        COUNTER_FACTORY_RUNTIME,
        "tests/fixtures/jvm_tasty_cross_class_factories/CounterFactoryRuntime.scala",
        SourceId::from_index(1695),
    )
    .unwrap();
    let second = emit_paired_jvm_tasty_batch(
        COUNTER_FACTORY_RUNTIME,
        "tests/fixtures/jvm_tasty_cross_class_factories/CounterFactoryRuntime.scala",
        SourceId::from_index(1696),
    )
    .unwrap();
    assert_ne!(first.source_id(), second.source_id());
    assert_eq!(first.classes().len(), 2);
    assert_eq!(second.classes().len(), 2);
    for (left, right) in first.classes().iter().zip(second.classes()) {
        assert_eq!(left.classfile_path(), right.classfile_path());
        assert_eq!(left.classfile_bytes(), right.classfile_bytes());
        assert_eq!(left.tasty_path(), right.tasty_path());
        assert_eq!(left.tasty_bytes(), right.tasty_bytes());
    }

    let output = TestDirectory::new("factory-counter-jdk-runtime");
    let class_root = output.path().join("rust-classes");
    let harness_root = output.path().join("harness-classes");
    std::fs::create_dir(&class_root).unwrap();
    std::fs::create_dir(&harness_root).unwrap();
    for (pair, expected_path) in first
        .classes()
        .iter()
        .zip(["example/Counter.class", "example/CounterFactory.class"])
    {
        assert_eq!(pair.classfile_path(), expected_path);
        let classfile = class_root.join(pair.classfile_path());
        let tasty = class_root.join(pair.tasty_path());
        std::fs::create_dir_all(classfile.parent().unwrap()).unwrap();
        std::fs::write(&classfile, pair.classfile_bytes()).unwrap();
        std::fs::write(&tasty, pair.tasty_bytes()).unwrap();
    }
    assert_eq!(
        read_relative_files(&class_root).unwrap(),
        [
            "example/Counter.class",
            "example/Counter.tasty",
            "example/CounterFactory.class",
            "example/CounterFactory.tasty",
        ]
    );

    let counter_pair = &first.classes()[0];
    let mut reader = Reader::new(counter_pair.classfile_bytes());
    let counter = ClassFile::decode(&mut reader).unwrap();
    assert!(reader.is_at_end());
    assert_eq!((counter.version.major, counter.version.minor), (65, 0));
    assert!(counter.access_flags.is_public());
    assert_eq!(
        counter
            .constant_pool
            .class_name(counter.this_class)
            .unwrap(),
        "example/Counter"
    );
    assert_eq!(counter.fields.len(), 1);
    assert_eq!(
        (
            counter
                .constant_pool
                .utf8(counter.fields[0].name_index)
                .unwrap(),
            counter
                .constant_pool
                .utf8(counter.fields[0].descriptor_index)
                .unwrap()
        ),
        ("current", "I")
    );
    assert!(counter.fields[0].access_flags.is_private());
    let mut counter_methods = counter
        .methods
        .iter()
        .map(|method| {
            (
                counter.constant_pool.utf8(method.name_index).unwrap(),
                counter.constant_pool.utf8(method.descriptor_index).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    counter_methods.sort_unstable();
    assert_eq!(
        counter_methods,
        [("<init>", "(I)V"), ("get", "()I"), ("set", "(I)V")]
    );
    assert!(
        counter
            .methods
            .iter()
            .find(|method| counter.constant_pool.utf8(method.name_index).unwrap() == "<init>")
            .unwrap()
            .access_flags
            .is_public()
    );
    assert!(
        counter
            .methods
            .iter()
            .all(|method| method.access_flags.is_public())
    );

    let factory_pair = &first.classes()[1];
    let mut reader = Reader::new(factory_pair.classfile_bytes());
    let factory = ClassFile::decode(&mut reader).unwrap();
    assert!(reader.is_at_end());
    assert_eq!((factory.version.major, factory.version.minor), (65, 0));
    assert!(factory.access_flags.is_public());
    assert_eq!(
        factory
            .constant_pool
            .class_name(factory.this_class)
            .unwrap(),
        "example/CounterFactory"
    );
    assert!(
        factory.fields.is_empty(),
        "Factory has no copied Counter storage"
    );
    let mut factory_methods = factory
        .methods
        .iter()
        .map(|method| {
            (
                factory.constant_pool.utf8(method.name_index).unwrap(),
                factory.constant_pool.utf8(method.descriptor_index).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    factory_methods.sort_unstable();
    assert_eq!(
        factory_methods,
        [("<init>", "()V"), ("create", "(I)Lexample/Counter;")]
    );
    assert!(
        factory
            .methods
            .iter()
            .all(|method| method.access_flags.is_public())
    );
    let create = factory
        .methods
        .iter()
        .find(|method| factory.constant_pool.utf8(method.name_index).unwrap() == "create")
        .unwrap();
    assert!(create.access_flags.is_public());
    let [Attribute::Code(code)] = create.attributes.as_slice() else {
        panic!("Factory.create has exactly one Code attribute");
    };
    assert_eq!(code.max_stack, 3);
    assert_eq!(code.max_locals, 2, "this is local 0 and seed is local 1");

    // Check instruction flow and canonical references without pinning bytecode
    // offsets or constant-pool indices.
    let mut cursor = 0;
    assert_eq!(code.code.get(cursor), Some(&0xbb), "new Counter");
    cursor += 1;
    let new_operand = code
        .code
        .get(cursor..cursor + 2)
        .expect("new has a u2 class operand");
    let new_index = ConstantPoolIndex(u16::from_be_bytes([new_operand[0], new_operand[1]]));
    cursor += 2;
    assert!(matches!(
        factory.constant_pool.get(new_index),
        Some(ConstantPoolEntry::Class { .. })
    ));
    assert_eq!(
        factory.constant_pool.class_name(new_index).unwrap(),
        "example/Counter"
    );
    assert_eq!(
        code.code.get(cursor),
        Some(&0x59),
        "dup preserves the return value"
    );
    cursor += 1;
    assert_eq!(
        code.code.get(cursor),
        Some(&0x1b),
        "iload_1 loads seed once"
    );
    cursor += 1;
    assert_eq!(
        code.code.get(cursor),
        Some(&0xb7),
        "invokespecial initializes the allocation"
    );
    cursor += 1;
    let method_operand = code
        .code
        .get(cursor..cursor + 2)
        .expect("invokespecial has a u2 method operand");
    let method_index =
        ConstantPoolIndex(u16::from_be_bytes([method_operand[0], method_operand[1]]));
    cursor += 2;
    let ConstantPoolEntry::Methodref {
        class_index,
        name_and_type_index,
    } = factory.constant_pool.get(method_index).unwrap()
    else {
        panic!("invokespecial references a CONSTANT_Methodref");
    };
    assert_eq!(
        factory.constant_pool.class_name(*class_index).unwrap(),
        "example/Counter"
    );
    assert_eq!(
        factory
            .constant_pool
            .name_and_type(*name_and_type_index)
            .unwrap(),
        ("<init>", "(I)V")
    );
    assert_eq!(
        code.code.get(cursor),
        Some(&0xb0),
        "areturn returns initialized Counter"
    );
    cursor += 1;
    assert_eq!(
        cursor,
        code.code.len(),
        "no uninitialized reference escapes"
    );

    let javap = Command::new("javap")
        .args([
            "-classpath",
            class_root.to_str().unwrap(),
            "-p",
            "-s",
            "-c",
            "-v",
            "example.CounterFactory",
        ])
        .output()
        .expect("start JDK 21 javap for the original Rust Factory.class");
    assert!(
        javap.status.success(),
        "javap rejected Rust CounterFactory.class:\n{}",
        String::from_utf8_lossy(&javap.stderr)
    );
    let javap = String::from_utf8_lossy(&javap.stdout);
    for expected in [
        "major version: 65",
        "public example.Counter create(int)",
        "descriptor: (I)Lexample/Counter;",
        "new           #",
        "class example/Counter",
        "dup",
        "iload_1",
        "invokespecial #",
        "Method example/Counter.\"<init>\":(I)V",
        "areturn",
    ] {
        assert!(
            javap.contains(expected),
            "javap omitted {expected:?}:\n{javap}"
        );
    }
    assert!(!javap.contains("CounterFactory.get"), "{javap}");
    assert!(
        !javap.contains("java/lang/Object;"),
        "Factory result is not widened:\n{javap}"
    );

    let class_paths = [
        class_root.join("example/Counter.class"),
        class_root.join("example/CounterFactory.class"),
    ];
    let original_hashes = class_paths
        .iter()
        .map(|path| {
            let output = Command::new("sha256sum")
                .arg(path)
                .output()
                .expect("start sha256sum for the original Rust classfile");
            assert!(
                output.status.success(),
                "sha256sum failed for {}",
                path.display()
            );
            String::from_utf8_lossy(&output.stdout)
                .split_whitespace()
                .next()
                .expect("sha256sum prints a digest")
                .to_owned()
        })
        .collect::<Vec<_>>();

    let harness = output.path().join("FactoryRuntimeHarness.java");
    std::fs::write(
        &harness,
        r#"public final class FactoryRuntimeHarness {
    public static void main(String[] args) {
        example.CounterFactory firstFactory = new example.CounterFactory();
        example.Counter first = firstFactory.create(7);
        example.Counter second = new example.CounterFactory().create(2);
        expect(first.get(), 7);
        expect(second.get(), 2);
        first.set(13);
        expect(first.get(), 13);
        expect(second.get(), 2);
        first.set(-42);
        expect(first.get(), -42);
        expect(second.get(), 2);
        example.Counter zero = firstFactory.create(0);
        expect(zero.get(), 0);
        expect(first.get(), -42);
        expect(second.get(), 2);
        System.out.println("factory runtime: 7,2,13,-42; fresh=0");
    }

    private static void expect(int actual, int expected) {
        if (actual != expected) {
            throw new AssertionError("expected " + expected + ", got " + actual);
        }
    }
}
"#,
    )
    .unwrap();
    let javac = Command::new("javac")
        .arg("-classpath")
        .arg(&class_root)
        .arg("-d")
        .arg(&harness_root)
        .arg(&harness)
        .output()
        .expect("start JDK 21 javac for the Rust factory runtime harness");
    assert!(
        javac.status.success(),
        "javac rejected the original Rust Counter/CounterFactory classes:\n{}",
        String::from_utf8_lossy(&javac.stderr)
    );

    let java = Command::new("java")
        .args(["-Xverify:all", "-classpath"])
        .arg(format!(
            "{}:{}",
            harness_root.display(),
            class_root.display()
        ))
        .arg("FactoryRuntimeHarness")
        .output()
        .expect("start JDK 21 verifier and Rust factory runtime harness");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected or failed the Rust factory batch:\n{}\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "factory runtime: 7,2,13,-42; fresh=0"
    );
    for (index, (path, pair)) in class_paths.iter().zip(first.classes()).enumerate() {
        let output = Command::new("sha256sum")
            .arg(path)
            .output()
            .expect("recompute SHA-256 for the original Rust classfile");
        assert!(
            output.status.success(),
            "sha256sum failed for {}",
            path.display()
        );
        let final_hash = String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .next()
            .expect("sha256sum prints a digest")
            .to_owned();
        assert_eq!(final_hash, original_hashes[index]);
        assert_eq!(
            std::fs::read(path).unwrap(),
            pair.classfile_bytes(),
            "javac/java retain original Rust-produced bytes"
        );
    }
}

struct TestDirectory(std::path::PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);

        let path = std::env::temp_dir().join(format!(
            "dotty-jvm-tasty-pair-{}-{label}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn publish_pair(
    output: &std::path::Path,
    pair: &dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts,
    fail_after_staged_files: Option<usize>,
) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT_STAGING_ID: AtomicUsize = AtomicUsize::new(0);

    if std::fs::read_dir(output)?.next().is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "paired output directory must be empty",
        ));
    }
    let staging = output.join(format!(
        ".dotty-jvm-tasty-pair-{}-{}",
        std::process::id(),
        NEXT_STAGING_ID.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&staging)?;

    let artifacts = [
        (pair.classfile_path(), pair.classfile_bytes()),
        (pair.tasty_path(), pair.tasty_bytes()),
    ];
    let stage_result = (|| {
        for (index, (relative, bytes)) in artifacts.iter().enumerate() {
            let relative = std::path::Path::new(relative);
            if relative.as_os_str().is_empty()
                || !relative
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_)))
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "unsafe paired artifact path",
                ));
            }
            let staged = staging.join("new").join(relative);
            if let Some(parent) = staged.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(staged, bytes)?;
            if fail_after_staged_files == Some(index + 1) {
                return Err(std::io::Error::other("injected staging write failure"));
            }
        }
        Ok(())
    })();
    if let Err(error) = stage_result {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }

    let mut published = Vec::new();
    let publish_result = (|| {
        for (relative, _) in artifacts {
            let relative = std::path::Path::new(relative);
            let destination = output.join(relative);
            let staged = staging.join("new").join(relative);
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(staged, &destination)?;
            published.push(destination);
        }
        Ok(())
    })();
    if let Err(error) = publish_result {
        for path in published {
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }
    let _ = std::fs::remove_dir_all(staging);
    Ok(())
}

#[path = "support/jvm_tasty_batch_publisher.rs"]
mod batch_publisher;
use batch_publisher::{
    BatchPublishError, BatchPublishFault, BatchPublishPair, MAX_BATCH_ARTIFACT_BYTES,
    MAX_BATCH_CLASSES, batch_publish_pairs, emitted_batch, publish_batch_pairs,
    publish_paired_batch, read_relative_files,
};
