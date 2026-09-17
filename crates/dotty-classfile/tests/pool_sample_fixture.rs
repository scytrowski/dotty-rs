//! Decodes a real JDK 25-compiled class file
//! (`tests/fixtures/pool_sample/PoolSample.class`, source alongside it) and
//! spot-checks known constant pool entries. Indices below are taken from
//! `javap -v PoolSample.class` for this exact checked-in `.class` file.

use dotty_classfile::class_file::ClassFileVersion;
use dotty_classfile::constant_pool::{
    BootstrapMethodIndex, ConstantPool, ConstantPoolEntry, ConstantPoolIndex, MethodHandleKind,
};
use dotty_classfile::reader::Reader;

const POOL_SAMPLE_CLASS: &[u8] = include_bytes!("fixtures/pool_sample/PoolSample.class");

fn decode_version_and_pool() -> (ClassFileVersion, ConstantPool) {
    let mut reader = Reader::new(POOL_SAMPLE_CLASS);
    let version = ClassFileVersion::decode(&mut reader).unwrap();
    let pool = ConstantPool::decode(&mut reader).unwrap();
    (version, pool)
}

#[test]
fn decodes_the_jdk_25_version() {
    let (version, _pool) = decode_version_and_pool();

    assert_eq!(version.major, 69);
    assert_eq!(version.minor, 0);
}

#[test]
fn decodes_the_class_name_and_super_class_utf8_entries() {
    let (_version, pool) = decode_version_and_pool();

    assert_eq!(
        pool.get(ConstantPoolIndex(10)),
        Some(&ConstantPoolEntry::Utf8("PoolSample".to_owned()))
    );
    assert_eq!(
        pool.get(ConstantPoolIndex(4)),
        Some(&ConstantPoolEntry::Utf8("java/lang/Object".to_owned()))
    );
}

#[test]
#[allow(clippy::approx_constant)] // 3.14159 is PoolSample.java's actual PI field value.
fn decodes_the_numeric_constant_value_entries() {
    let (_version, pool) = decode_version_and_pool();

    assert_eq!(
        pool.get(ConstantPoolIndex(37)),
        Some(&ConstantPoolEntry::Integer(42))
    );
    assert_eq!(
        pool.get(ConstantPoolIndex(40)),
        Some(&ConstantPoolEntry::Long(42_000_000_000))
    );
    assert_eq!(pool.get(ConstantPoolIndex(41)), None, "reserved long slot");
    assert_eq!(
        pool.get(ConstantPoolIndex(44)),
        Some(&ConstantPoolEntry::Float(0.5))
    );
    assert_eq!(
        pool.get(ConstantPoolIndex(47)),
        Some(&ConstantPoolEntry::Double(3.14159))
    );
    assert_eq!(
        pool.get(ConstantPoolIndex(48)),
        None,
        "reserved double slot"
    );
}

#[test]
fn decodes_the_string_constant_value_entry() {
    let (_version, pool) = decode_version_and_pool();

    assert_eq!(
        pool.get(ConstantPoolIndex(51)),
        Some(&ConstantPoolEntry::String {
            string_index: ConstantPoolIndex(52),
        })
    );
    assert_eq!(
        pool.get(ConstantPoolIndex(52)),
        Some(&ConstantPoolEntry::Utf8("hello".to_owned()))
    );
}

#[test]
fn decodes_the_interface_methodref_entry_for_the_recursive_run_call() {
    let (_version, pool) = decode_version_and_pool();

    assert_eq!(
        pool.get(ConstantPoolIndex(29)),
        Some(&ConstantPoolEntry::InterfaceMethodref {
            class_index: ConstantPoolIndex(30),
            name_and_type_index: ConstantPoolIndex(31),
        })
    );
}

#[test]
fn decodes_the_invoke_dynamic_entry_for_the_string_concatenation() {
    let (_version, pool) = decode_version_and_pool();

    assert_eq!(
        pool.get(ConstantPoolIndex(13)),
        Some(&ConstantPoolEntry::InvokeDynamic {
            bootstrap_method_attr_index: BootstrapMethodIndex(0),
            name_and_type_index: ConstantPoolIndex(14),
        })
    );
}

#[test]
fn decodes_the_method_handle_entry_for_the_string_concat_bootstrap() {
    let (_version, pool) = decode_version_and_pool();

    assert_eq!(
        pool.get(ConstantPoolIndex(60)),
        Some(&ConstantPoolEntry::MethodHandle {
            reference_kind: MethodHandleKind::InvokeStatic,
            reference_index: ConstantPoolIndex(61),
        })
    );
}

#[test]
fn decodes_exactly_the_expected_number_of_entries() {
    let (_version, pool) = decode_version_and_pool();

    assert_eq!(
        pool.get(ConstantPoolIndex(71)),
        Some(&ConstantPoolEntry::Utf8("Lookup".to_owned()))
    );
    assert_eq!(pool.get(ConstantPoolIndex(72)), None);
}

#[test]
fn leaves_the_reader_positioned_after_the_constant_pool() {
    let mut reader = Reader::new(POOL_SAMPLE_CLASS);
    ClassFileVersion::decode(&mut reader).unwrap();
    ConstantPool::decode(&mut reader).unwrap();

    // access_flags/this_class/super_class/interfaces/fields/methods/
    // attributes still remain; decoding them is future work.
    assert!(reader.remaining() > 0);
}
