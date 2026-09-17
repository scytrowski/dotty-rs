//! Decodes and validates real `.class` files extracted from a local JDK 25
//! runtime image (`tests/fixtures/jdk_corpus/`, see that directory's
//! `manifest.toml`/`generate.sh`). Corpus membership comes from
//! `classes.txt` rather than being hardcoded here, per
//! `docs/testing-strategy.md`'s baseline-corpus convention. Unlike this
//! crate's other fixtures, individual file paths aren't known at compile
//! time, so this reads fixture bytes via `std::fs::read` instead of
//! `include_bytes!`.

use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::{ConstantPool, ConstantPoolEntry, ConstantPoolIndex};
use dotty_classfile::reader::Reader;
use std::path::Path;

const CLASSES_TXT: &str = include_str!("fixtures/jdk_corpus/classes.txt");

fn corpus_classes() -> Vec<&'static str> {
    CLASSES_TXT
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

fn read_corpus_class(name: &str) -> Vec<u8> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jdk_corpus");
    let path = root.join(format!("{name}.class"));
    std::fs::read(&path)
        .unwrap_or_else(|error| panic!("failed to read corpus fixture {}: {error}", path.display()))
}

fn decode<'a>(name: &str, bytes: &'a [u8]) -> ClassFile<'a> {
    let mut reader = Reader::new(bytes);
    let class_file = ClassFile::decode(&mut reader)
        .unwrap_or_else(|error| panic!("{name} failed to decode: {error}"));
    assert!(
        reader.is_at_end(),
        "{name} decoded but left unread trailing bytes"
    );
    class_file
}

fn utf8_name(pool: &ConstantPool, index: ConstantPoolIndex) -> &str {
    match pool.get(index) {
        Some(ConstantPoolEntry::Utf8(name)) => name.as_str(),
        other => panic!("expected Utf8 at {index:?}, got {other:?}"),
    }
}

fn class_name(pool: &ConstantPool, index: ConstantPoolIndex) -> &str {
    match pool.get(index) {
        Some(ConstantPoolEntry::Class { name_index }) => utf8_name(pool, *name_index),
        other => panic!("expected Class at {index:?}, got {other:?}"),
    }
}

#[test]
fn decodes_and_validates_every_corpus_class() {
    for name in corpus_classes() {
        let bytes = read_corpus_class(name);
        let class_file = decode(name, &bytes);

        assert_eq!(
            class_file.validate_references(),
            Ok(()),
            "{name} failed reference validation"
        );
    }
}

#[test]
fn constant_desc_is_a_sealed_interface_with_its_real_permitted_subclasses() {
    let bytes = read_corpus_class("java/lang/constant/ConstantDesc");
    let class_file = decode("ConstantDesc", &bytes);
    let pool = &class_file.constant_pool;

    assert!(class_file.access_flags.is_interface());

    let permitted = class_file
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::PermittedSubclasses(classes) => Some(classes.clone()),
            _ => None,
        })
        .expect("expected a PermittedSubclasses attribute");

    let mut names: Vec<&str> = permitted
        .iter()
        .map(|index| class_name(pool, *index))
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec![
            "java/lang/Double",
            "java/lang/Float",
            "java/lang/Integer",
            "java/lang/Long",
            "java/lang/String",
            "java/lang/constant/ClassDesc",
            "java/lang/constant/DynamicConstantDesc",
            "java/lang/constant/MethodHandleDesc",
            "java/lang/constant/MethodTypeDesc",
        ]
    );
}

#[test]
fn day_of_week_is_a_real_enum() {
    let bytes = read_corpus_class("java/time/DayOfWeek");
    let class_file = decode("DayOfWeek", &bytes);

    assert!(class_file.access_flags.is_enum());
}

#[test]
fn array_list_has_a_real_inner_classes_attribute_naming_itr() {
    let bytes = read_corpus_class("java/util/ArrayList");
    let class_file = decode("ArrayList", &bytes);
    let pool = &class_file.constant_pool;

    let inner_classes = class_file
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::InnerClasses(entries) => Some(entries),
            _ => None,
        })
        .expect("expected an InnerClasses attribute");

    let has_itr = inner_classes.iter().any(|entry| {
        entry
            .inner_name_index
            .is_some_and(|index| utf8_name(pool, index) == "Itr")
    });
    assert!(has_itr, "expected an InnerClasses entry named Itr");
}

#[test]
fn list_and_stream_are_real_interfaces() {
    for name in ["java/util/List", "java/util/stream/Stream"] {
        let bytes = read_corpus_class(name);
        let class_file = decode(name, &bytes);

        assert!(
            class_file.access_flags.is_interface(),
            "{name} should be an interface"
        );
    }
}

#[test]
fn string_implements_its_five_real_interfaces() {
    let bytes = read_corpus_class("java/lang/String");
    let class_file = decode("String", &bytes);
    let pool = &class_file.constant_pool;

    let mut names: Vec<&str> = class_file
        .interfaces
        .iter()
        .map(|index| class_name(pool, *index))
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec![
            "java/io/Serializable",
            "java/lang/CharSequence",
            "java/lang/Comparable",
            "java/lang/constant/Constable",
            "java/lang/constant/ConstantDesc",
        ]
    );
}
