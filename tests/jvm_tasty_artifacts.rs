use dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan;
use dotty_classfile::classfile::{ClassFile, Reader};
use dotty_classfile::tasty_link::TastyLinkId;
use dotty_core::SourceId;
use dotty_jvm::emit_class;
use dotty_jvm_tasty_contract::{JvmTastyContractError, validate_classfile_tasty_link};
use dotty_tasty::tasty::{DefinitionBody, TastyFile};

use dotty::jvm_tasty_artifacts::{
    PairedArtifactError, PairedBatchArtifactError, emit_paired_jvm_tasty_batch,
    emit_paired_jvm_tasty_class,
};
use dotty::jvm_tasty_plan::PairedPlanError;
use dotty_classfile::attribute::Attribute;
use dotty_classfile::constant_pool::{ConstantPoolEntry, ConstantPoolIndex};
use std::process::Command;

const COUNTER: &str = include_str!("fixtures/jvm_tasty_interop/Counter.scala");
const COUNTER_GAUGE: &str = include_str!("fixtures/jvm_tasty_multiclass/CounterGauge.scala");
const COUNTER_READER: &str = include_str!("fixtures/jvm_tasty_cross_class/Reader.scala");
const COUNTER_FACTORY: &str =
    include_str!("fixtures/jvm_tasty_cross_class_factories/CounterFactory.scala");

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
        "linked Counter/Reader runtime requires java, javac, and javap from JDK 21+ on PATH; unavailable: {}",
        unavailable.join(", ")
    );
    if std::env::var("DOTTY_CLASSFILE_REQUIRE_JDK").as_deref() == Ok("1") {
        panic!("{message}");
    }
    eprintln!("skipping optional linked Counter/Reader JDK runtime: {message}");
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
    version_text
        .split(|character: char| !character.is_ascii_digit())
        .find_map(|component| component.parse::<u32>().ok())
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
