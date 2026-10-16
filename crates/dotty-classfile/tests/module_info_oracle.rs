//! Pins JDK 21 and tiny javac/jmod descriptors as typed JPMS codec and
//! descriptor-validation oracles.

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
        opaque_attributes: &["ModuleTarget", "ModuleHashes"],
    },
    Fixture {
        name: "JDK 21 java.net.http",
        bytes: include_bytes!("fixtures/jdk_module_info/jdk21/java.net.http-module-info.class"),
        module_name: "java.net.http",
        opaque_attributes: &["ModuleTarget"],
    },
    Fixture {
        name: "JDK 21 jdk.jshell",
        bytes: include_bytes!("fixtures/jdk_module_info/jdk21/jdk.jshell-module-info.class"),
        module_name: "jdk.jshell",
        opaque_attributes: &["ModuleTarget"],
    },
    Fixture {
        name: "directed exports, opens, uses and provides",
        bytes: include_bytes!("fixtures/jdk_module_info/generated/directives-module-info.class"),
        module_name: "fixture.directives",
        opaque_attributes: &[],
    },
    Fixture {
        name: "optional module main class present",
        bytes: include_bytes!("fixtures/jdk_module_info/generated/main-class-module-info.class"),
        module_name: "fixture.main",
        opaque_attributes: &[],
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
        Attribute::ModulePackages(_) => "ModulePackages".to_owned(),
        Attribute::ModuleMainClass(_) => "ModuleMainClass".to_owned(),
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
        class_file
            .validate_module_descriptor()
            .unwrap_or_else(|error| {
                panic!("{} is not a valid module descriptor: {error}", fixture.name)
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
            "{} preserves expected opaque attributes",
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
            "{} roundtrips byte-for-byte with typed JPMS attributes",
            fixture.name
        );
    }
}

#[test]
fn module_companion_attributes_are_typed_and_presence_is_preserved() {
    let mut counts = Vec::new();
    for fixture in FIXTURES {
        let mut reader = Reader::new(fixture.bytes);
        let class_file = ClassFile::decode(&mut reader).unwrap();
        let packages = class_file
            .attributes
            .iter()
            .find_map(|attribute| match attribute {
                Attribute::ModulePackages(value) => Some(value.package_indices.len()),
                _ => None,
            });
        let main_class = class_file
            .attributes
            .iter()
            .find_map(|attribute| match attribute {
                Attribute::ModuleMainClass(index) => Some(
                    class_file
                        .constant_pool
                        .class_name(*index)
                        .unwrap()
                        .to_owned(),
                ),
                _ => None,
            });
        counts.push((fixture.module_name, packages, main_class));
    }
    assert_eq!(counts[0], ("java.base", Some(196), None));
    assert_eq!(counts[1], ("java.net.http", Some(6), None));
    assert_eq!(counts[3], ("fixture.directives", Some(2), None));
    assert_eq!(
        counts[4],
        ("fixture.main", Some(1), Some("fixture/app/Main".to_owned()))
    );
    assert_eq!(counts[5], ("fixture.minimal", None, None));
}

#[test]
fn descriptor_validation_returns_specific_structural_errors() {
    use dotty_classfile::class_file::ModuleDescriptorError as Error;

    let fixture = FIXTURES
        .iter()
        .find(|fixture| fixture.name == "minimal descriptor with optional attributes absent")
        .unwrap();
    let decode = || {
        let mut reader = Reader::new(fixture.bytes);
        ClassFile::decode(&mut reader).unwrap()
    };

    let mut missing_module = decode();
    missing_module
        .attributes
        .retain(|attribute| !matches!(attribute, Attribute::Module(_)));
    assert_eq!(
        missing_module.validate_module_descriptor(),
        Err(Error::MissingModuleAttribute)
    );

    let mut duplicate_module = decode();
    let module = duplicate_module
        .attributes
        .iter()
        .find(|attribute| matches!(attribute, Attribute::Module(_)))
        .unwrap()
        .clone();
    duplicate_module.attributes.push(module);
    assert_eq!(
        duplicate_module.validate_module_descriptor(),
        Err(Error::DuplicateAttribute { name: "Module" })
    );

    let mut invalid_version = decode();
    invalid_version.version.major = 52;
    assert_eq!(
        invalid_version.validate_module_descriptor(),
        Err(Error::UnsupportedVersion {
            major: 52,
            minor: 0
        })
    );

    let mut invalid_shape = decode();
    invalid_shape.super_class = Some(invalid_shape.this_class);
    assert_eq!(
        invalid_shape.validate_module_descriptor(),
        Err(Error::HasSuperclass)
    );

    let module_name_index = invalid_shape
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::Module(module) => Some(module.module_name_index),
            _ => None,
        })
        .unwrap();
    let utf8_index = match invalid_shape.constant_pool.get(module_name_index).unwrap() {
        dotty_classfile::constant_pool::ConstantPoolEntry::Module { name_index } => *name_index,
        _ => unreachable!(),
    };
    let mut with_field = decode();
    with_field.fields.push(dotty_classfile::field::FieldInfo {
        access_flags: dotty_classfile::access_flags::FieldAccessFlags(0),
        name_index: utf8_index,
        descriptor_index: utf8_index,
        attributes: Vec::new(),
    });
    assert_eq!(
        with_field.validate_module_descriptor(),
        Err(Error::HasFields { count: 1 })
    );

    let mut with_method = decode();
    with_method
        .methods
        .push(dotty_classfile::method::MethodInfo {
            access_flags: dotty_classfile::access_flags::MethodAccessFlags(0),
            name_index: utf8_index,
            descriptor_index: utf8_index,
            attributes: Vec::new(),
        });
    assert_eq!(
        with_method.validate_module_descriptor(),
        Err(Error::HasMethods { count: 1 })
    );

    let mut invalid_flags = decode();
    invalid_flags.access_flags.0 |= dotty_classfile::access_flags::ACC_PUBLIC;
    assert_eq!(
        invalid_flags.validate_module_descriptor(),
        Err(Error::InvalidAccessFlags {
            flags: ACC_MODULE | dotty_classfile::access_flags::ACC_PUBLIC
        })
    );

    let mut ordinary_class = decode();
    ordinary_class.access_flags.0 = 0;
    assert_eq!(
        ordinary_class.validate_module_descriptor(),
        Err(Error::NotModuleDescriptor)
    );
}

#[test]
fn descriptor_validation_rejects_duplicate_optional_attributes_and_wrong_tags() {
    use dotty_classfile::class_file::ModuleDescriptorError as Error;
    use dotty_classfile::constant_pool::{EntryKind, PoolRefError};

    let fixture = FIXTURES
        .iter()
        .find(|fixture| fixture.name == "optional module main class present")
        .unwrap();
    let mut reader = Reader::new(fixture.bytes);
    let mut class_file = ClassFile::decode(&mut reader).unwrap();

    let main_class = class_file
        .attributes
        .iter()
        .find(|attribute| matches!(attribute, Attribute::ModuleMainClass(_)))
        .unwrap()
        .clone();
    class_file.attributes.push(main_class);
    assert_eq!(
        class_file.validate_module_descriptor(),
        Err(Error::DuplicateAttribute {
            name: "ModuleMainClass"
        })
    );

    class_file.attributes.pop();
    let packages = class_file
        .attributes
        .iter()
        .find(|attribute| matches!(attribute, Attribute::ModulePackages(_)))
        .unwrap()
        .clone();
    class_file.attributes.push(packages);
    assert_eq!(
        class_file.validate_module_descriptor(),
        Err(Error::DuplicateAttribute {
            name: "ModulePackages"
        })
    );

    class_file.attributes.pop();
    let module_index = class_file
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::Module(module) => Some(module.module_name_index),
            _ => None,
        })
        .unwrap();
    for attribute in &mut class_file.attributes {
        if let Attribute::ModuleMainClass(index) = attribute {
            *index = module_index;
        }
    }
    assert_eq!(
        class_file.validate_module_descriptor(),
        Err(Error::ConstantPool(PoolRefError::WrongKind {
            index: module_index,
            expected: EntryKind::Class
        }))
    );

    let fixture = FIXTURES
        .iter()
        .find(|fixture| fixture.name == "optional module main class present")
        .unwrap();
    let mut reader = Reader::new(fixture.bytes);
    let mut wrong_name = ClassFile::decode(&mut reader).unwrap();
    wrong_name.this_class = wrong_name
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::ModuleMainClass(index) => Some(*index),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        wrong_name.validate_module_descriptor(),
        Err(Error::WrongThisClass {
            name: "fixture/app/Main".to_owned()
        })
    );
}

#[test]
fn java_base_requires_flags_follow_each_supported_release_rule() {
    use dotty_classfile::attribute::{MODULE_ACC_STATIC_PHASE, MODULE_ACC_TRANSITIVE};
    use dotty_classfile::class_file::ModuleDescriptorError as Error;

    let fixture = FIXTURES
        .iter()
        .find(|fixture| fixture.name == "minimal descriptor with optional attributes absent")
        .unwrap();
    for major in [65, 66, 69, 70] {
        let mut reader = Reader::new(fixture.bytes);
        let mut class_file = ClassFile::decode(&mut reader).unwrap();
        class_file.version.major = major;
        for attribute in &mut class_file.attributes {
            if let Attribute::Module(module) = attribute {
                module.requires[0].requires_flags |= MODULE_ACC_STATIC_PHASE;
            }
        }
        assert_eq!(
            class_file.validate_module_descriptor(),
            Err(Error::InvalidDirective {
                reason: "java.base requires row has forbidden static-phase flag"
            }),
            "ACC_STATIC_PHASE remains prohibited at major {major}"
        );
    }

    for major in [65, 66] {
        let mut reader = Reader::new(fixture.bytes);
        let mut class_file = ClassFile::decode(&mut reader).unwrap();
        class_file.version.major = major;
        for attribute in &mut class_file.attributes {
            if let Attribute::Module(module) = attribute {
                module.requires[0].requires_flags |= MODULE_ACC_TRANSITIVE;
            }
        }
        assert!(
            matches!(
                class_file.validate_module_descriptor(),
                Err(Error::InvalidDirective {
                    reason: "java.base requires row has forbidden transitive flag"
                })
            ),
            "ACC_TRANSITIVE remains prohibited at major {major}"
        );
    }

    for major in [69, 70] {
        let mut reader = Reader::new(fixture.bytes);
        let mut java_25_or_later = ClassFile::decode(&mut reader).unwrap();
        java_25_or_later.version.major = major;
        for attribute in &mut java_25_or_later.attributes {
            if let Attribute::Module(module) = attribute {
                module.requires[0].requires_flags |= MODULE_ACC_TRANSITIVE;
            }
        }
        assert_eq!(
            java_25_or_later.validate_module_descriptor(),
            Ok(()),
            "ACC_TRANSITIVE is permitted on java.base at major {major}"
        );
    }
}

#[test]
fn java_25_javac_allows_transitive_java_base_requirement() {
    // Compiled from jdk25/fixture.transitive/module-info.java with Amazon Corretto 25.0.4.
    let bytes =
        include_bytes!("fixtures/jdk_module_info/jdk25/transitive-java-base-module-info.class");
    let mut reader = Reader::new(bytes);
    let class_file = ClassFile::decode(&mut reader).unwrap();

    assert_eq!(class_file.version.major, 69);
    assert_eq!(class_file.validate_module_descriptor(), Ok(()));
}

#[test]
fn opaque_attributes_with_known_jpms_names_cannot_bypass_validation() {
    use dotty_classfile::class_file::ModuleDescriptorError as Error;

    for (fixture_name, target_name) in [
        (
            "minimal descriptor with optional attributes absent",
            "Module",
        ),
        ("JDK 21 java.base", "ModulePackages"),
        ("optional module main class present", "ModuleMainClass"),
    ] {
        let fixture = FIXTURES
            .iter()
            .find(|fixture| fixture.name == fixture_name)
            .unwrap();
        let mut reader = Reader::new(fixture.bytes);
        let mut class_file = ClassFile::decode(&mut reader).unwrap();
        let typed = class_file
            .attributes
            .iter()
            .find(|attribute| attribute_name(&class_file, attribute) == target_name)
            .unwrap()
            .clone();
        let encoded = typed.encode(&class_file.constant_pool).unwrap();
        let name_index = u16::from_be_bytes([encoded[0], encoded[1]]);
        class_file
            .attributes
            .push(Attribute::Other(dotty_classfile::attribute::RawAttribute {
                name_index: dotty_classfile::constant_pool::ConstantPoolIndex(name_index),
                bytes: &encoded[6..],
            }));
        assert_eq!(
            class_file.validate_module_descriptor(),
            Err(Error::OpaqueKnownModuleAttribute {
                name: target_name.to_owned()
            }),
            "raw {target_name} is rejected before encoding"
        );

        let encoded_class = class_file.encode().unwrap();
        let mut reader = Reader::new(&encoded_class);
        let decoded = ClassFile::decode(&mut reader).unwrap();
        let expected_name = match target_name {
            "Module" => "Module",
            "ModulePackages" => "ModulePackages",
            "ModuleMainClass" => "ModuleMainClass",
            _ => unreachable!(),
        };
        assert_eq!(
            decoded.validate_module_descriptor(),
            Err(Error::DuplicateAttribute {
                name: expected_name
            }),
            "encoded/decoded duplicate {target_name} remains invalid"
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
