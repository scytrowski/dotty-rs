//! Pins JDK 21 and tiny javac/jmod module descriptors without implementing a
//! production JPMS decoder. JVMS Module* attributes intentionally stay opaque.

use dotty_classfile::access_flags::ACC_MODULE;
use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::{ConstantPoolEntry, ConstantPoolIndex};
use dotty_classfile::reader::Reader;

struct Fixture {
    name: &'static str,
    bytes: &'static [u8],
    module_name: &'static str,
    opaque_attributes: &'static [&'static str],
}

const FIXTURES: &[Fixture] = &[
    Fixture {
        name: "JDK 21 java.base",
        bytes: include_bytes!("fixtures/jdk_module_info/jdk21/java.base-module-info.class"),
        module_name: "java.base",
        opaque_attributes: &["Module", "ModulePackages", "ModuleTarget", "ModuleHashes"],
    },
    Fixture {
        name: "JDK 21 java.net.http",
        bytes: include_bytes!("fixtures/jdk_module_info/jdk21/java.net.http-module-info.class"),
        module_name: "java.net.http",
        opaque_attributes: &["Module", "ModulePackages", "ModuleTarget"],
    },
    Fixture {
        name: "JDK 21 jdk.jshell",
        bytes: include_bytes!("fixtures/jdk_module_info/jdk21/jdk.jshell-module-info.class"),
        module_name: "jdk.jshell",
        opaque_attributes: &["Module", "ModulePackages", "ModuleTarget"],
    },
    Fixture {
        name: "directed exports, opens, uses and provides",
        bytes: include_bytes!("fixtures/jdk_module_info/generated/directives-module-info.class"),
        module_name: "fixture.directives",
        opaque_attributes: &["Module", "ModulePackages"],
    },
    Fixture {
        name: "optional module main class present",
        bytes: include_bytes!("fixtures/jdk_module_info/generated/main-class-module-info.class"),
        module_name: "fixture.main",
        opaque_attributes: &["Module", "ModuleMainClass", "ModulePackages"],
    },
    Fixture {
        name: "minimal descriptor with optional attributes absent",
        bytes: include_bytes!("fixtures/jdk_module_info/generated/minimal-module-info.class"),
        module_name: "fixture.minimal",
        opaque_attributes: &["Module"],
    },
    Fixture {
        name: "zero module and requires version indices",
        bytes: include_bytes!(
            "fixtures/jdk_module_info/generated/zero-optional-versions-module-info.class"
        ),
        module_name: "fixture.minimal",
        opaque_attributes: &["Module"],
    },
];

fn attribute_name(class_file: &ClassFile<'_>, attribute: &Attribute<'_>) -> String {
    match attribute {
        Attribute::Other(raw) => class_file
            .constant_pool
            .utf8(raw.name_index)
            .expect("opaque attribute name resolves to Utf8")
            .to_owned(),
        Attribute::SourceFile(_) => "SourceFile".to_owned(),
        Attribute::InnerClasses(_) => "InnerClasses".to_owned(),
        other => panic!("unexpected structured module-info attribute: {other:?}"),
    }
}

fn u2(bytes: &[u8]) -> u16 {
    u16::from_be_bytes([bytes[0], bytes[1]])
}

#[test]
fn jdk_and_minimal_module_info_fixtures_decode_and_roundtrip_opaquely() {
    for fixture in FIXTURES {
        let mut reader = Reader::new(fixture.bytes);
        let class_file = ClassFile::decode(&mut reader)
            .unwrap_or_else(|error| panic!("{} failed to decode: {error}", fixture.name));
        assert!(reader.is_at_end(), "{} has no trailing bytes", fixture.name);
        assert_eq!(
            class_file.version.major, 65,
            "{} uses Java 21",
            fixture.name
        );
        assert_eq!(
            class_file.access_flags.0, ACC_MODULE,
            "{} is ACC_MODULE",
            fixture.name
        );
        assert_eq!(
            class_file.super_class, None,
            "{} has no superclass",
            fixture.name
        );
        assert!(
            class_file.interfaces.is_empty(),
            "{} has no interfaces",
            fixture.name
        );
        assert!(
            class_file.fields.is_empty(),
            "{} has no fields",
            fixture.name
        );
        assert!(
            class_file.methods.is_empty(),
            "{} has no methods",
            fixture.name
        );
        class_file.validate_references().unwrap_or_else(|error| {
            panic!("{} has invalid modeled references: {error}", fixture.name)
        });

        let names: Vec<_> = class_file
            .attributes
            .iter()
            .map(|attribute| attribute_name(&class_file, attribute))
            .collect();
        let opaque: Vec<_> = class_file
            .attributes
            .iter()
            .filter_map(|attribute| match attribute {
                Attribute::Other(raw) => Some(
                    class_file
                        .constant_pool
                        .utf8(raw.name_index)
                        .expect("opaque attribute name resolves to Utf8"),
                ),
                _ => None,
            })
            .collect();
        assert!(
            names.contains(&"Module".to_owned()),
            "{} has Module",
            fixture.name
        );
        assert_eq!(
            opaque, fixture.opaque_attributes,
            "{} preserves expected Module* attributes opaquely",
            fixture.name
        );

        let module = class_file
            .attributes
            .iter()
            .find_map(|attribute| match attribute {
                Attribute::Other(raw)
                    if class_file.constant_pool.utf8(raw.name_index).ok() == Some("Module") =>
                {
                    Some(raw)
                }
                _ => None,
            })
            .expect("Module attribute is preserved as Other");
        let module_index = ConstantPoolIndex(u2(module.bytes));
        let module_name_index = match class_file.constant_pool.get(module_index) {
            Some(ConstantPoolEntry::Module { name_index }) => *name_index,
            other => panic!(
                "{} module_name_index has wrong constant-pool kind: {other:?}",
                fixture.name
            ),
        };
        assert_eq!(
            class_file.constant_pool.utf8(module_name_index).unwrap(),
            fixture.module_name,
            "{} names the expected module",
            fixture.name
        );

        assert_eq!(
            class_file.encode().unwrap(),
            fixture.bytes,
            "{} roundtrips byte-for-byte while Module* remains opaque",
            fixture.name
        );
    }
}

#[test]
fn zero_module_and_requires_version_indices_mean_absent_versions() {
    let fixture = FIXTURES
        .iter()
        .find(|fixture| fixture.name == "zero module and requires version indices")
        .unwrap();
    let mut reader = Reader::new(fixture.bytes);
    let class_file = ClassFile::decode(&mut reader).unwrap();
    assert!(reader.is_at_end());
    let module = class_file
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::Other(raw)
                if class_file.constant_pool.utf8(raw.name_index).ok() == Some("Module") =>
            {
                Some(raw)
            }
            _ => None,
        })
        .unwrap();

    assert_eq!(u2(&module.bytes[4..6]), 0, "module version is absent");
    assert_eq!(u2(&module.bytes[6..8]), 1, "fixture has one requires entry");
    assert_eq!(
        u2(&module.bytes[12..14]),
        0,
        "required module version is absent"
    );
    assert_eq!(class_file.encode().unwrap(), fixture.bytes);
}
