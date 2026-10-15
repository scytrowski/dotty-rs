//! Pins JDK 21 and tiny javac/jmod descriptors as a typed Module codec oracle;
//! companion Module* attributes remain opaque.

use dotty_classfile::access_flags::ACC_MODULE;
use dotty_classfile::attribute::{Attribute, MODULE_ACC_TRANSITIVE};
use dotty_classfile::class_file::ClassFile;
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
        opaque_attributes: &["ModulePackages", "ModuleTarget", "ModuleHashes"],
    },
    Fixture {
        name: "JDK 21 java.net.http",
        bytes: include_bytes!("fixtures/jdk_module_info/jdk21/java.net.http-module-info.class"),
        module_name: "java.net.http",
        opaque_attributes: &["ModulePackages", "ModuleTarget"],
    },
    Fixture {
        name: "JDK 21 jdk.jshell",
        bytes: include_bytes!("fixtures/jdk_module_info/jdk21/jdk.jshell-module-info.class"),
        module_name: "jdk.jshell",
        opaque_attributes: &["ModulePackages", "ModuleTarget"],
    },
    Fixture {
        name: "directed exports, opens, uses and provides",
        bytes: include_bytes!("fixtures/jdk_module_info/generated/directives-module-info.class"),
        module_name: "fixture.directives",
        opaque_attributes: &["ModulePackages"],
    },
    Fixture {
        name: "optional module main class present",
        bytes: include_bytes!("fixtures/jdk_module_info/generated/main-class-module-info.class"),
        module_name: "fixture.main",
        opaque_attributes: &["ModuleMainClass", "ModulePackages"],
    },
    Fixture {
        name: "minimal descriptor with optional attributes absent",
        bytes: include_bytes!("fixtures/jdk_module_info/generated/minimal-module-info.class"),
        module_name: "fixture.minimal",
        opaque_attributes: &[],
    },
    Fixture {
        name: "zero module and requires version indices",
        bytes: include_bytes!(
            "fixtures/jdk_module_info/generated/zero-optional-versions-module-info.class"
        ),
        module_name: "fixture.minimal",
        opaque_attributes: &[],
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
        Attribute::Module(_) => "Module".to_owned(),
        other => panic!("unexpected structured module-info attribute: {other:?}"),
    }
}

#[test]
fn jdk_and_minimal_module_info_fixtures_decode_and_roundtrip() {
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
                Attribute::Module(module) => Some(module),
                _ => None,
            })
            .expect("Module attribute is decoded structurally");
        assert_eq!(
            class_file
                .constant_pool
                .module_name(module.module_name_index)
                .unwrap(),
            fixture.module_name,
            "{} names the expected module",
            fixture.name
        );

        assert_eq!(
            class_file.encode().unwrap(),
            fixture.bytes,
            "{} roundtrips byte-for-byte with typed Module and opaque companion attributes",
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
            Attribute::Module(module) => Some(module),
            _ => None,
        })
        .unwrap();

    assert_eq!(
        module.module_version_index, None,
        "module version is absent"
    );
    assert_eq!(module.requires.len(), 1, "fixture has one requires entry");
    assert_eq!(
        module.requires[0].requires_version_index, None,
        "required module version is absent"
    );
    assert_eq!(class_file.encode().unwrap(), fixture.bytes);
}

#[test]
fn javac_directive_fixture_exposes_each_module_table_in_wire_order() {
    let fixture = FIXTURES
        .iter()
        .find(|fixture| fixture.name == "directed exports, opens, uses and provides")
        .unwrap();
    let mut reader = Reader::new(fixture.bytes);
    let class_file = ClassFile::decode(&mut reader).unwrap();
    let module = class_file
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::Module(module) => Some(module),
            _ => None,
        })
        .unwrap();
    let pool = &class_file.constant_pool;

    assert_eq!(
        pool.utf8(module.module_version_index.unwrap()).unwrap(),
        "1.0"
    );
    assert_eq!(module.module_flags, 0);
    assert_eq!(module.requires.len(), 2);
    assert_eq!(
        pool.module_name(module.requires[0].requires_index).unwrap(),
        "java.base"
    );
    assert_eq!(module.requires[0].requires_flags, 0x8000);
    assert_eq!(
        pool.utf8(module.requires[0].requires_version_index.unwrap())
            .unwrap(),
        "21.0.8"
    );
    assert_eq!(
        pool.module_name(module.requires[1].requires_index).unwrap(),
        "java.logging"
    );
    assert_eq!(module.requires[1].requires_flags, 0x0040);
    assert_eq!(
        pool.utf8(module.requires[1].requires_version_index.unwrap())
            .unwrap(),
        "21.0.8"
    );

    assert_eq!(module.exports.len(), 1);
    assert_eq!(
        pool.package_name(module.exports[0].exports_index).unwrap(),
        "fixture/api"
    );
    assert_eq!(module.exports[0].exports_flags, 0);
    assert_eq!(
        module.exports[0]
            .exports_to
            .iter()
            .map(|index| pool.module_name(*index).unwrap())
            .collect::<Vec<_>>(),
        ["fixture.consumer.one", "fixture.consumer.two"]
    );

    assert_eq!(module.opens.len(), 1);
    assert_eq!(
        pool.package_name(module.opens[0].opens_index).unwrap(),
        "fixture/impl"
    );
    assert_eq!(module.opens[0].opens_flags, 0);
    assert_eq!(
        pool.module_name(module.opens[0].opens_to[0]).unwrap(),
        "fixture.consumer.one"
    );
    assert_eq!(module.uses.len(), 1);
    assert_eq!(
        pool.class_name(module.uses[0]).unwrap(),
        "fixture/api/Greeting"
    );
    assert_eq!(module.provides.len(), 1);
    assert_eq!(
        pool.class_name(module.provides[0].provides_index).unwrap(),
        "fixture/api/Greeting"
    );
    assert_eq!(
        module.provides[0]
            .provides_with
            .iter()
            .map(|index| pool.class_name(*index).unwrap())
            .collect::<Vec<_>>(),
        ["fixture/impl/GreetingProvider"]
    );
}

#[test]
fn jdk_module_fixture_preserves_transitive_requires_flags() {
    let fixture = FIXTURES
        .iter()
        .find(|fixture| fixture.name == "JDK 21 jdk.jshell")
        .unwrap();
    let mut reader = Reader::new(fixture.bytes);
    let class_file = ClassFile::decode(&mut reader).unwrap();
    let module = class_file
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::Module(module) => Some(module),
            _ => None,
        })
        .unwrap();
    let transitive = module
        .requires
        .iter()
        .find(|requires| {
            class_file
                .constant_pool
                .module_name(requires.requires_index)
                == Ok("java.compiler")
        })
        .unwrap();

    assert_eq!(transitive.requires_flags, MODULE_ACC_TRANSITIVE);
}
