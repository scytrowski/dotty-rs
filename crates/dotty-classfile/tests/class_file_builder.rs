use dotty_classfile::classfile::{
    ACC_ABSTRACT, ACC_FINAL, ACC_INTERFACE, ACC_NATIVE, ACC_PRIVATE, ACC_PUBLIC, ACC_STATIC,
    ACC_SUPER, Attribute, ClassAccessFlags, ClassFile, ClassFileBuildError, ClassFileBuilder,
    ClassFileVersion, ConstantPoolEntry, FieldAccessFlags, MethodAccessFlags, Reader,
};
use std::fs;
use std::process::Command;

fn decode(bytes: &[u8]) -> ClassFile<'_> {
    let mut reader = Reader::new(bytes);
    let class_file = ClassFile::decode(&mut reader).unwrap();
    assert!(reader.is_at_end());
    class_file
}

fn utf8<'a>(
    class_file: &'a ClassFile<'_>,
    index: dotty_classfile::constant_pool::ConstantPoolIndex,
) -> &'a str {
    class_file.constant_pool.utf8(index).unwrap()
}

#[test]
fn builds_and_roundtrips_owned_class_metadata_and_raw_code() {
    let mut builder = ClassFileBuilder::new(
        "example/Calculator",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    builder
        .set_access_flags(ClassAccessFlags(ACC_PUBLIC | ACC_ABSTRACT | ACC_SUPER))
        .unwrap();
    builder.set_super_class(Some("java/lang/Object")).unwrap();
    builder.add_interface("java/lang/Runnable").unwrap();
    builder.source_file("Calculator.java").unwrap();
    builder
        .signature("Ljava/lang/Object;Ljava/lang/Runnable;")
        .unwrap();
    builder
        .add_field(
            "values",
            "Ljava/util/List;",
            FieldAccessFlags(ACC_PRIVATE | ACC_FINAL),
        )
        .unwrap()
        .signature("Ljava/util/List<Ljava/lang/String;>;")
        .unwrap();
    builder
        .add_method(
            "compute",
            "()I",
            MethodAccessFlags(ACC_PUBLIC | ACC_ABSTRACT),
        )
        .unwrap()
        .signature("()I")
        .unwrap();
    builder
        .add_method(
            "answerFromConstant",
            "()I",
            MethodAccessFlags(ACC_PUBLIC | ACC_STATIC),
        )
        .unwrap()
        .code(1, 0, [0x10, 42, 0xac])
        .unwrap();

    let bytes = builder.encode().unwrap();
    let class_file = decode(&bytes);
    assert_eq!(
        class_file.version,
        ClassFileVersion {
            major: 65,
            minor: 0
        }
    );
    assert_eq!(
        class_file.access_flags,
        ClassAccessFlags(ACC_PUBLIC | ACC_ABSTRACT | ACC_SUPER)
    );
    assert_eq!(
        class_file.constant_pool.class_name(class_file.this_class),
        Ok("example/Calculator")
    );
    assert_eq!(
        class_file
            .constant_pool
            .class_name(class_file.super_class.unwrap()),
        Ok("java/lang/Object")
    );
    assert_eq!(class_file.interfaces.len(), 1);
    assert_eq!(
        class_file
            .constant_pool
            .class_name(class_file.interfaces[0]),
        Ok("java/lang/Runnable")
    );
    assert_eq!(class_file.fields.len(), 1);
    assert_eq!(utf8(&class_file, class_file.fields[0].name_index), "values");
    assert_eq!(
        utf8(&class_file, class_file.fields[0].descriptor_index),
        "Ljava/util/List;"
    );
    assert_eq!(
        class_file.fields[0].access_flags,
        FieldAccessFlags(ACC_PRIVATE | ACC_FINAL)
    );
    assert!(matches!(
        class_file.fields[0].attributes.as_slice(),
        [Attribute::Signature(_)]
    ));
    assert_eq!(class_file.methods.len(), 2);
    assert_eq!(
        utf8(&class_file, class_file.methods[0].name_index),
        "compute"
    );
    assert_eq!(
        class_file.methods[0].access_flags,
        MethodAccessFlags(ACC_PUBLIC | ACC_ABSTRACT)
    );
    assert!(matches!(
        class_file.methods[0].attributes.as_slice(),
        [Attribute::Signature(_)]
    ));
    assert_eq!(
        utf8(&class_file, class_file.methods[1].name_index),
        "answerFromConstant"
    );
    assert!(matches!(
        class_file.methods[1].attributes.as_slice(),
        [Attribute::Code(code)] if code.max_stack == 1 && code.max_locals == 0 && code.code == [0x10, 42, 0xac]
    ));
    assert!(matches!(
        class_file.attributes.as_slice(),
        [Attribute::SourceFile(_), Attribute::Signature(_)]
    ));
    assert_eq!(class_file.encode().unwrap(), bytes);
    assert!(
        class_file
            .constant_pool
            .encode()
            .unwrap()
            .windows(4)
            .any(|bytes| bytes == b"Code")
    );

    // javap checks that the generated Java 21 classfile and its attributes
    // are inspectable when a JDK is available. The raw code remains opaque to
    // this builder and is not executed here.
    if Command::new("javap").arg("-version").output().is_ok() {
        let unique = format!("dotty-classfile-builder-{}", std::process::id());
        let directory = std::env::temp_dir().join(unique).join("example");
        fs::create_dir_all(&directory).unwrap();
        let class_path = directory.join("Calculator.class");
        fs::write(&class_path, bytes).unwrap();
        let output = Command::new("javap")
            .args(["-v", "-classpath"])
            .arg(directory.parent().unwrap())
            .arg("example.Calculator")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "javap failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("Calculator.java"));
        fs::remove_dir_all(directory.parent().unwrap()).unwrap();
        let _ = class_path;
    }
}

#[test]
fn builds_constructor_and_static_method_using_raw_code() {
    let mut builder = ClassFileBuilder::new(
        "example/WithConstructor",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    builder
        .add_method("<init>", "()V", MethodAccessFlags(ACC_PUBLIC))
        .unwrap()
        .code(0, 1, [0xb1])
        .unwrap();
    builder
        .add_method("answer", "()I", MethodAccessFlags(ACC_PUBLIC | ACC_STATIC))
        .unwrap()
        .code(1, 0, [0x10, 42, 0xac])
        .unwrap();
    let bytes = builder.encode().unwrap();
    let class_file = decode(&bytes);
    assert_eq!(class_file.methods.len(), 2);
    assert_eq!(
        utf8(&class_file, class_file.methods[0].name_index),
        "<init>"
    );
    assert_eq!(
        utf8(&class_file, class_file.methods[1].name_index),
        "answer"
    );
    assert!(class_file.methods.iter().all(|method| {
        method
            .attributes
            .iter()
            .any(|attribute| matches!(attribute, Attribute::Code(_)))
    }));
    if Command::new("javap").arg("-version").output().is_ok() {
        let directory = std::env::temp_dir().join(format!(
            "dotty-classfile-builder-ctor-{}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        let class_path = directory.join("WithConstructor.class");
        fs::write(&class_path, bytes).unwrap();
        let output = Command::new("javap")
            .arg("-v")
            .arg(&class_path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "javap failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("<init>"));
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn rejects_invalid_names_descriptors_flags_versions_and_duplicates() {
    assert_eq!(
        ClassFileBuilder::new(
            "example//Broken",
            ClassFileVersion {
                major: 65,
                minor: 0
            }
        )
        .err(),
        Some(ClassFileBuildError::InvalidInternalName)
    );
    assert_eq!(
        ClassFileBuilder::new(
            "example/C",
            ClassFileVersion {
                major: 71,
                minor: 0
            }
        )
        .err(),
        Some(ClassFileBuildError::UnsupportedVersion {
            major: 71,
            minor: 0
        })
    );

    let mut builder = ClassFileBuilder::new(
        "example/C",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    assert_eq!(
        builder
            .add_field("x", "V", FieldAccessFlags(ACC_PUBLIC))
            .err(),
        Some(ClassFileBuildError::InvalidDescriptor(
            dotty_classfile::descriptor::DescriptorError::UnknownTypeChar {
                offset: 0,
                found: 'V'
            }
        ))
    );
    assert_eq!(
        builder
            .add_field("x", "I", FieldAccessFlags(ACC_PUBLIC | ACC_PRIVATE))
            .err(),
        Some(ClassFileBuildError::InvalidAccessFlags)
    );
    assert_eq!(
        builder
            .add_field("badFlags", "I", FieldAccessFlags(ACC_ABSTRACT))
            .err(),
        Some(ClassFileBuildError::InvalidAccessFlags)
    );
    builder
        .add_field("x", "I", FieldAccessFlags(ACC_PUBLIC))
        .unwrap();
    assert_eq!(
        builder
            .add_field("x", "I", FieldAccessFlags(ACC_PUBLIC))
            .err(),
        Some(ClassFileBuildError::DuplicateField)
    );
    assert_eq!(
        builder
            .add_field("bad/name", "I", FieldAccessFlags(ACC_PUBLIC))
            .err(),
        Some(ClassFileBuildError::InvalidMemberName)
    );
    assert_eq!(
        builder
            .add_method("run", "(V)V", MethodAccessFlags(ACC_PUBLIC))
            .err(),
        Some(ClassFileBuildError::InvalidDescriptor(
            dotty_classfile::descriptor::DescriptorError::UnknownTypeChar {
                offset: 1,
                found: 'V'
            }
        ))
    );
    assert_eq!(
        builder
            .add_method("<init>", "()I", MethodAccessFlags(ACC_PUBLIC))
            .err(),
        Some(ClassFileBuildError::InvalidConstructor)
    );
    assert_eq!(
        builder
            .add_method("bad/name", "()V", MethodAccessFlags(ACC_PUBLIC))
            .err(),
        Some(ClassFileBuildError::InvalidMemberName)
    );
    assert_eq!(
        builder
            .add_method("run", "()V", MethodAccessFlags(ACC_ABSTRACT | ACC_STATIC))
            .err(),
        Some(ClassFileBuildError::InvalidAccessFlags)
    );
    assert_eq!(
        builder
            .add_method(
                "badFlags",
                "()V",
                MethodAccessFlags(dotty_classfile::classfile::ACC_ENUM),
            )
            .err(),
        Some(ClassFileBuildError::InvalidAccessFlags)
    );
    assert_eq!(
        builder
            .add_method(
                "strictMethod",
                "()V",
                MethodAccessFlags(ACC_PUBLIC | dotty_classfile::classfile::ACC_STRICT),
            )
            .err(),
        Some(ClassFileBuildError::InvalidAccessFlags)
    );
}

#[test]
fn code_rules_duplicates_and_failed_mutations_are_observable() {
    let mut builder = ClassFileBuilder::new(
        "example/C",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    builder
        .set_access_flags(ClassAccessFlags(ACC_PUBLIC | ACC_ABSTRACT | ACC_SUPER))
        .unwrap();
    assert_eq!(
        builder
            .add_method(
                "abstractMethod",
                "()V",
                MethodAccessFlags(ACC_ABSTRACT | ACC_PUBLIC)
            )
            .unwrap()
            .code(0, 0, [0xb1])
            .err(),
        Some(ClassFileBuildError::CodeNotAllowed)
    );
    assert_eq!(
        builder
            .add_method(
                "nativeMethod",
                "()V",
                MethodAccessFlags(ACC_NATIVE | ACC_PUBLIC)
            )
            .unwrap()
            .code(0, 0, [0xb1])
            .err(),
        Some(ClassFileBuildError::CodeNotAllowed)
    );
    {
        let mut method = builder
            .add_method("concrete", "()V", MethodAccessFlags(ACC_PUBLIC))
            .unwrap();
        assert_eq!(
            method.code(0, 0, [] as [u8; 0]).err(),
            Some(ClassFileBuildError::InvalidCodeLength { length: 0 })
        );
        method.code(0, 0, [0xb1]).unwrap();
        assert_eq!(
            method.code(0, 0, [0xb1]).err(),
            Some(ClassFileBuildError::DuplicateCode)
        );
    }
    assert_eq!(
        builder
            .add_method("concrete", "()V", MethodAccessFlags(ACC_PUBLIC))
            .err(),
        Some(ClassFileBuildError::DuplicateMethod)
    );
    assert!(builder.encode().is_ok());

    let missing_code = ClassFileBuilder::new(
        "example/MissingCode",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    let mut missing_code = missing_code;
    missing_code
        .add_method("run", "()V", MethodAccessFlags(ACC_PUBLIC))
        .unwrap();
    assert_eq!(missing_code.encode(), Err(ClassFileBuildError::MissingCode));

    let mut native = ClassFileBuilder::new(
        "example/Native",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    native
        .add_method("run", "()V", MethodAccessFlags(ACC_NATIVE | ACC_PUBLIC))
        .unwrap();
    assert!(native.encode().is_ok());
}

#[test]
fn enforces_unambiguous_interface_and_abstract_class_rules() {
    let mut interface = ClassFileBuilder::new(
        "example/Api",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    interface
        .set_access_flags(ClassAccessFlags(ACC_PUBLIC | ACC_INTERFACE | ACC_ABSTRACT))
        .unwrap();
    assert_eq!(
        interface.set_super_class(Some("example/Base")).err(),
        Some(ClassFileBuildError::InvalidSuperclass)
    );
    assert_eq!(
        interface.add_interface("example/Api").err(),
        Some(ClassFileBuildError::SelfInterface)
    );
    assert_eq!(
        interface
            .add_field("VALUE", "I", FieldAccessFlags(ACC_PUBLIC))
            .err(),
        Some(ClassFileBuildError::InvalidAccessFlags)
    );
    assert_eq!(
        interface
            .add_method("<init>", "()V", MethodAccessFlags(ACC_PUBLIC))
            .err(),
        Some(ClassFileBuildError::InvalidAccessFlags)
    );
    interface
        .add_field(
            "VALUE",
            "I",
            FieldAccessFlags(ACC_PUBLIC | ACC_STATIC | ACC_FINAL),
        )
        .unwrap();
    interface
        .add_method("run", "()V", MethodAccessFlags(ACC_PUBLIC | ACC_ABSTRACT))
        .unwrap();
    assert!(interface.encode().is_ok());

    let mut class = ClassFileBuilder::new(
        "example/Concrete",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    class
        .add_method("run", "()V", MethodAccessFlags(ACC_ABSTRACT | ACC_PUBLIC))
        .unwrap();
    assert_eq!(class.encode(), Err(ClassFileBuildError::InvalidAccessFlags));

    let mut contradictory = ClassFileBuilder::new(
        "example/Contradictory",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    assert_eq!(
        contradictory
            .set_access_flags(ClassAccessFlags(ACC_FINAL | ACC_ABSTRACT))
            .err(),
        Some(ClassFileBuildError::InvalidAccessFlags)
    );

    let mut late_interface = ClassFileBuilder::new(
        "example/LateInterface",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    late_interface
        .add_field("value", "I", FieldAccessFlags(ACC_PUBLIC))
        .unwrap();
    late_interface
        .set_access_flags(ClassAccessFlags(ACC_PUBLIC | ACC_INTERFACE | ACC_ABSTRACT))
        .unwrap();
    assert_eq!(
        late_interface.encode(),
        Err(ClassFileBuildError::InvalidAccessFlags)
    );
}

#[test]
fn reuses_constant_pool_entries_for_repeated_symbols() {
    let mut builder = ClassFileBuilder::new(
        "example/C",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    builder
        .add_field("first", "I", FieldAccessFlags(ACC_PRIVATE))
        .unwrap();
    builder
        .add_field("second", "I", FieldAccessFlags(ACC_PRIVATE))
        .unwrap();
    builder
        .add_method("one", "()V", MethodAccessFlags(ACC_PUBLIC))
        .unwrap()
        .code(0, 1, [0xb1])
        .unwrap();
    builder
        .add_method("two", "()V", MethodAccessFlags(ACC_PUBLIC))
        .unwrap()
        .code(0, 1, [0xb1])
        .unwrap();
    let bytes = builder.encode().unwrap();
    let class_file = decode(&bytes);
    assert_eq!(
        class_file.fields[0].descriptor_index,
        class_file.fields[1].descriptor_index
    );
    assert_eq!(
        class_file.methods[0].descriptor_index,
        class_file.methods[1].descriptor_index
    );
    assert!(
        class_file
            .constant_pool
            .encode()
            .unwrap()
            .windows(4)
            .any(|bytes| bytes == b"Code")
    );
    assert!(
        class_file
            .constant_pool
            .get(class_file.fields[0].descriptor_index)
            .is_some_and(|entry| matches!(entry, ConstantPoolEntry::Utf8(value) if value == "I"))
    );
}
