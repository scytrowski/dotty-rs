use std::fs;
use std::path::Path;

use dotty_classfile::access_flags::{ACC_PRIVATE, ACC_PUBLIC};
use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::reader::Reader;
use dotty_tasty::tasty::TastyFile;

const ORACLE_DIR: &str = "tests/fixtures/jvm_tasty_interop/scala39/example";

fn fixture(name: &str) -> Vec<u8> {
    fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(ORACLE_DIR)
            .join(name),
    )
    .unwrap()
}

#[test]
fn scala_390_counter_classfile_links_to_its_tasty_header() {
    let class_bytes = fixture("Counter.class");
    let tasty_bytes = fixture("Counter.tasty");
    let mut reader = Reader::new(&class_bytes);
    let class = ClassFile::decode(&mut reader).unwrap();

    assert!(reader.is_at_end());
    assert_eq!(class.version.major, 61, "pinned scalac default target");
    assert_eq!(
        class.constant_pool.class_name(class.this_class).unwrap(),
        "example/Counter"
    );
    assert_eq!(class.fields.len(), 1);
    assert_eq!(class.methods.len(), 3);

    let field = &class.fields[0];
    assert_eq!(
        class.constant_pool.utf8(field.name_index).unwrap(),
        "current"
    );
    assert_eq!(
        class.constant_pool.utf8(field.descriptor_index).unwrap(),
        "I"
    );
    assert!(field.access_flags.contains(ACC_PRIVATE));

    let methods = class
        .methods
        .iter()
        .map(|method| {
            (
                class.constant_pool.utf8(method.name_index).unwrap(),
                class.constant_pool.utf8(method.descriptor_index).unwrap(),
                method.access_flags.0,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(methods[0].0, "<init>");
    assert_eq!(methods[0].1, "(I)V");
    assert_eq!(methods[1].0, "get");
    assert_eq!(methods[1].1, "()I");
    assert_eq!(methods[2].0, "set");
    assert_eq!(methods[2].1, "(I)V", "Scala Unit lowers to JVM void");
    assert!(methods.iter().all(|method| method.2 & ACC_PUBLIC != 0));

    let attribute_names = class
        .attributes
        .iter()
        .map(|attribute| match attribute {
            Attribute::Other(raw) => class.constant_pool.utf8(raw.name_index).unwrap(),
            Attribute::Signature(_) => "Signature",
            Attribute::SourceFile(_) => "SourceFile",
            other => panic!("unexpected modeled attribute on the pinned Scala fixture: {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        attribute_names,
        [
            "Signature",
            "SourceFile",
            "TASTY",
            "Scala3InlineInfo",
            "Scala"
        ]
    );

    let tasty_links = class
        .attributes
        .iter()
        .filter_map(|attribute| match attribute {
            Attribute::Other(raw)
                if class.constant_pool.utf8(raw.name_index).unwrap() == "TASTY" =>
            {
                Some(raw.bytes)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(tasty_links.len(), 1, "TASTY is one class-level attribute");
    assert_eq!(tasty_links[0].len(), 16);

    let tasty = TastyFile::parse_and_validate_scala_3_9(&tasty_bytes).unwrap();
    assert_eq!(tasty.header().major_version, 28);
    assert_eq!(tasty.header().minor_version, 9);
    assert_eq!(tasty.header().experimental_version, 0);
    assert_eq!(tasty_links[0], tasty.header().uuid);

    // Pin the observed UUID bytes from this Scala 3.9.0 oracle fixture.
    assert_eq!(
        tasty_links[0],
        [
            0x00, 0x9d, 0x71, 0x1b, 0xcc, 0x01, 0x99, 0xe8, 0x00, 0xce, 0x94, 0x0d, 0xd0, 0x72,
            0x1c, 0x1b,
        ]
    );
}
