use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_batch;
use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::ConstantPoolEntry;
use dotty_classfile::reader::Reader as ClassReader;
use dotty_core::SourceId;
use dotty_tasty::ast::{APPLY_TAG, NEW_TAG, SELECTIN_TAG};
use dotty_tasty::file::TastyFile;

const FACTORY_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_cross_class_factories/CounterFactory.scala");
const IDENTITY_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_cross_class_factories/CounterIdentitySource.scala");

const COUNTER_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_cross_class_factories/scala39/example/Counter.class");
const COUNTER_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_cross_class_factories/scala39/example/Counter.tasty");
const FACTORY_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_cross_class_factories/scala39/example/CounterFactory.class");
const FACTORY_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_cross_class_factories/scala39/example/CounterFactory.tasty");
const IDENTITY_CLASS: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_cross_class_factories/scala39/example/CounterIdentity.class"
);
const IDENTITY_TASTY: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_cross_class_factories/scala39/example/CounterIdentity.tasty"
);

fn decode_class(bytes: &[u8]) -> ClassFile<'_> {
    let mut reader = ClassReader::new(bytes);
    let class = ClassFile::decode(&mut reader).unwrap();
    assert!(reader.is_at_end());
    class
}

fn tasty_uuid(class: &ClassFile<'_>) -> [u8; 16] {
    *class
        .tasty_link_id()
        .unwrap()
        .expect("Scala oracle class has one TASTY link")
        .as_bytes()
}

fn assert_pinned_tasty(bytes: &[u8]) -> TastyFile<'_> {
    let tasty = TastyFile::parse_and_validate_scala_3_9(bytes).unwrap();
    assert_eq!(
        (
            tasty.header().major_version,
            tasty.header().minor_version,
            tasty.header().experimental_version,
        ),
        (28, 9, 0)
    );
    tasty
}

fn method_code<'a>(class: &'a ClassFile<'_>, name: &str) -> &'a [u8] {
    let method = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == name)
        .unwrap_or_else(|| panic!("class declares {name}"));
    method
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::Code(code) => Some(code.code),
            _ => None,
        })
        .expect("concrete method has Code")
}

#[test]
fn scala_39_factory_oracle_pins_descriptors_constructor_wire_and_sidecar_links() {
    let counter = decode_class(COUNTER_CLASS);
    let factory = decode_class(FACTORY_CLASS);
    let identity = decode_class(IDENTITY_CLASS);
    assert_eq!((counter.version.major, factory.version.major), (65, 65));
    assert_eq!(
        counter.constant_pool.class_name(counter.this_class),
        Ok("example/Counter")
    );
    assert_eq!(
        factory.constant_pool.class_name(factory.this_class),
        Ok("example/CounterFactory")
    );
    assert_eq!(
        identity.constant_pool.class_name(identity.this_class),
        Ok("example/CounterIdentity")
    );

    let factory_tasty = assert_pinned_tasty(FACTORY_TASTY);
    let counter_tasty = assert_pinned_tasty(COUNTER_TASTY);
    let identity_tasty = assert_pinned_tasty(IDENTITY_TASTY);
    assert_eq!(tasty_uuid(&factory), factory_tasty.header().uuid);
    assert_eq!(tasty_uuid(&counter), counter_tasty.header().uuid);
    assert_eq!(tasty_uuid(&identity), identity_tasty.header().uuid);
    for name in ["create", "seed", "Counter", "<init>"] {
        assert!(
            (0..factory_tasty.names().len())
                .any(|reference| factory_tasty.names().get_utf8(reference as u32) == Some(name)),
            "factory sidecar retains the {name} wire name"
        );
    }

    let create = factory
        .methods
        .iter()
        .find(|method| factory.constant_pool.utf8(method.name_index).unwrap() == "create")
        .expect("CounterFactory declares create");
    assert_eq!(
        factory.constant_pool.utf8(create.descriptor_index),
        Ok("(I)Lexample/Counter;")
    );
    assert!(create.access_flags.is_public());
    for tag in [APPLY_TAG, SELECTIN_TAG, NEW_TAG] {
        assert!(
            !factory_tasty.ast_nodes_with_tag(tag).unwrap().is_empty(),
            "factory sidecar retains the TASTy tag {tag} used by the constructor call"
        );
    }
    assert!(
        (0..factory_tasty.names().len()).any(|reference| {
            matches!(
                factory_tasty.names().get(reference as u32),
                Some(dotty_tasty::RawName::Qualified { prefix, selector })
                    if factory_tasty.names().get_utf8(*prefix) == Some("example")
                        && factory_tasty.names().get_utf8(*selector) == Some("Counter")
            )
        }),
        "factory's TASTy signature has a qualified cross-class Counter reference"
    );
    assert_eq!(
        method_code(&factory, "create"),
        [0xbb, 0x00, 0x11, 0x59, 0x1b, 0xb7, 0x00, 0x14, 0xb0]
    );

    let identity_method = identity
        .methods
        .iter()
        .find(|method| identity.constant_pool.utf8(method.name_index).unwrap() == "identity")
        .expect("CounterIdentity declares the independent identity control");
    assert_eq!(
        identity
            .constant_pool
            .utf8(identity_method.descriptor_index),
        Ok("(Lexample/Counter;)Lexample/Counter;")
    );
    assert_eq!(method_code(&identity, "identity"), [0x2b, 0xb0]);

    assert!((1..=u16::MAX).any(|index| matches!(
        factory.constant_pool.get(dotty_classfile::constant_pool::ConstantPoolIndex(index)),
        Some(ConstantPoolEntry::Methodref { class_index, name_and_type_index })
            if factory.constant_pool.class_name(*class_index) == Ok("example/Counter")
                && factory.constant_pool.name_and_type(*name_and_type_index)
                    .is_ok_and(|(name, descriptor)| name == "<init>" && descriptor == "(I)V")
    )));
}

#[test]
fn paired_tasty_planning_rejects_identity_and_factory_at_method_result_validation() {
    for (source, source_name, source_id, method) in [
        (
            FACTORY_SOURCE,
            "CounterFactory.scala",
            SourceId::from_index(1690),
            "create",
        ),
        (
            IDENTITY_SOURCE,
            "CounterIdentity.scala",
            SourceId::from_index(1691),
            "identity",
        ),
    ] {
        let error = emit_paired_jvm_tasty_batch(source, source_name, source_id)
            .expect_err("reference-result methods exceed the current TASTy subset");
        let evidence = format!("{error:?}");
        assert!(
            evidence.contains("phase: \"JVM/TASTy cross-contract validation\""),
            "{method} must pass source typing and JVM unit validation before the cross-contract rejects it: {evidence}"
        );
        assert!(
            evidence.contains("TastyPlan(MalformedType") && evidence.contains("role: MethodResult"),
            "{method} must fail specifically at the pickler's reference result contract: {evidence}"
        );
    }
}
