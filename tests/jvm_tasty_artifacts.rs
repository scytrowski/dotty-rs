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
const COUNTER_FACTORY_RUNTIME: &str =
    include_str!("fixtures/jvm_tasty_cross_class_factories/CounterFactoryRuntime.scala");
const COUNTER_OWNER: &str =
    include_str!("fixtures/jvm_tasty_object_composition/CounterOwner.scala");
const COUNTER_OWNER_ALIAS: &str =
    include_str!("fixtures/jvm_tasty_constructor_injection/CounterOwner.scala");
const REFERENCE_CONSTRUCTOR_BATCH: &str =
    include_str!("fixtures/jvm_tasty_reference_constructor_calls/CounterOwnerFactory.scala");
const COUNTER_FACTORY_IDENTITY_ONLY: &str = "package example\n\nclass Counter:\n  def value(): Int = 1\n\nclass CounterFactory:\n  def identity(counter: Counter): Counter = counter\n";
const COUNTER_FACTORY_PRIMITIVE_CREATE: &str = "package example\n\nclass Counter:\n  def value(): Int = 1\n\nclass CounterFactory:\n  def identity(counter: Counter): Counter = counter\n  def create(seed: Int): Int = seed\n";
const COUNTER_FACTORY_REFERENCE_CREATE: &str = "package example\n\nclass Counter:\n  def value(): Int = 1\n\nclass CounterFactory:\n  def create(counter: Counter): Counter = counter\n";

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
    uninitialized_owner[factory_code_offset + 5] = 0x00;
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
