use dotty_classfile::classfile::{
    ACC_ABSTRACT, ACC_FINAL, ACC_INTERFACE, ACC_NATIVE, ACC_PRIVATE, ACC_PUBLIC, ACC_STATIC,
    ACC_SUPER, Attribute, ClassAccessFlags, ClassFile, ClassFileBuildError, ClassFileBuilder,
    ClassFileVersion, CodeBuilderError, ConstantPoolEntry, FieldAccessFlags, MethodAccessFlags,
    Reader,
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

fn code_attribute<'a, 'b>(
    method: &'a dotty_classfile::classfile::MethodInfo<'b>,
) -> &'a dotty_classfile::classfile::CodeAttribute<'b> {
    let [Attribute::Code(code)] = method.attributes.as_slice() else {
        panic!(
            "expected exactly one Code attribute, got {:?}",
            method.attributes
        );
    };
    code
}

fn method_named<'a, 'b>(
    class_file: &'a ClassFile<'b>,
    name: &str,
) -> &'a dotty_classfile::classfile::MethodInfo<'b> {
    class_file
        .methods
        .iter()
        .find(|method| utf8(class_file, method.name_index) == name)
        .unwrap_or_else(|| panic!("missing method {name}"))
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
fn builds_static_arithmetic_method_from_typed_instructions() {
    let mut builder = ClassFileBuilder::new(
        "example/TypedCalculator",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    builder
        .add_static_method("add", "(II)I", |code| {
            code.iload(0)?;
            code.iload(1)?;
            code.iadd()?;
            code.ireturn()?;
            Ok(())
        })
        .unwrap();

    let bytes = builder.encode().unwrap();
    let class_file = decode(&bytes);
    assert_eq!(class_file.methods.len(), 1);
    assert!(matches!(
        class_file.methods[0].attributes.as_slice(),
        [Attribute::Code(code)]
            if code.max_stack == 2
                && code.max_locals == 2
                && code.code == [0x1a, 0x1b, 0x60, 0xac]
    ));
}

#[test]
fn builds_static_method_that_discards_one_category_one_value() {
    let mut builder = ClassFileBuilder::new(
        "example/PopValue",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    builder
        .add_static_method("discard", "()V", |code| {
            code.iconst(1)?;
            code.pop()?;
            code.return_void()?;
            Ok(())
        })
        .unwrap();

    let bytes = builder.encode().unwrap();
    let class_file = decode(&bytes);
    assert_eq!(class_file.methods.len(), 1);
    assert!(matches!(
        class_file.methods[0].attributes.as_slice(),
        [Attribute::Code(code)]
            if code.max_stack == 1
                && code.max_locals == 0
                && code.code == [0x04, 0x57, 0xb1]
    ));
}

#[test]
fn builds_branched_integer_methods_with_one_stack_map_table_each() {
    let mut builder = ClassFileBuilder::new(
        "example/Branched",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    builder
        .add_static_method("max", "(II)I", |code| {
            let left = code.new_label()?;
            code.iload(0)?;
            code.iload(1)?;
            code.if_icmpge(left.clone())?;
            code.iload(1)?;
            code.ireturn()?;
            code.mark(left)?;
            code.iload(0)?;
            code.ireturn()?;
            Ok(())
        })
        .unwrap();
    builder
        .add_static_method("sumTo", "(I)I", |code| {
            let loop_head = code.new_label()?;
            let done = code.new_label()?;
            code.iconst(0)?;
            code.istore(1)?;
            code.iconst(1)?;
            code.istore(2)?;
            code.mark(loop_head.clone())?;
            code.iload(2)?;
            code.iload(0)?;
            code.if_icmpgt(done.clone())?;
            code.iload(1)?;
            code.iload(2)?;
            code.iadd()?;
            code.istore(1)?;
            code.iload(2)?;
            code.iconst(1)?;
            code.iadd()?;
            code.istore(2)?;
            code.goto(loop_head)?;
            code.mark(done)?;
            code.iload(1)?;
            code.ireturn()?;
            Ok(())
        })
        .unwrap();

    let bytes = builder.encode().unwrap();
    let class_file = decode(&bytes);
    assert_eq!(class_file.methods.len(), 2);
    let max = code_attribute(method_named(&class_file, "max"));
    assert_eq!((max.max_stack, max.max_locals), (2, 2));
    assert_eq!(
        max.code,
        [0x1a, 0x1b, 0xa2, 0x00, 0x05, 0x1b, 0xac, 0x1a, 0xac]
    );
    let [Attribute::Other(max_stack_map)] = max.attributes.as_slice() else {
        panic!("branched method should contain exactly one StackMapTable");
    };
    assert_eq!(utf8(&class_file, max_stack_map.name_index), "StackMapTable");
    assert_eq!(
        max_stack_map.bytes,
        [
            0x00, 0x01, 0xff, 0x00, 0x07, 0x00, 0x02, 0x01, 0x01, 0x00, 0x00
        ]
    );

    let sum_to = code_attribute(method_named(&class_file, "sumTo"));
    assert_eq!((sum_to.max_stack, sum_to.max_locals), (2, 3));
    assert_eq!(sum_to.code.len(), 22);
    assert_eq!(
        &sum_to.code[..9],
        &[0x03, 0x3c, 0x04, 0x3d, 0x1c, 0x1a, 0xa3, 0x00, 0x0e]
    );
    assert_eq!(&sum_to.code[17..], &[0xa7, 0xff, 0xf3, 0x1b, 0xac]);
    let [Attribute::Other(sum_to_stack_map)] = sum_to.attributes.as_slice() else {
        panic!("branched loop should contain exactly one StackMapTable");
    };
    assert_eq!(
        utf8(&class_file, sum_to_stack_map.name_index),
        "StackMapTable"
    );
    assert_eq!(
        sum_to_stack_map.bytes,
        [
            0x00, 0x02, 0xff, 0x00, 0x04, 0x00, 0x03, 0x01, 0x01, 0x01, 0x00, 0x00, 0xff, 0x00,
            0x0f, 0x00, 0x03, 0x01, 0x01, 0x01, 0x00, 0x00,
        ]
    );
}

#[test]
fn class_builder_accepts_prebuilt_symbolic_branch_code() {
    let mut builder = ClassFileBuilder::new(
        "example/Prebuilt",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    let mut code = dotty_classfile::bytecode::CodeBuilder::new();
    let otherwise = code.new_label().unwrap();
    let join = code.new_label().unwrap();
    code.iconst(1).unwrap();
    code.ifeq(otherwise.clone()).unwrap();
    code.iconst(2).unwrap();
    code.goto(join.clone()).unwrap();
    code.mark(otherwise).unwrap();
    code.iconst(3).unwrap();
    code.mark(join).unwrap();
    code.ireturn().unwrap();
    builder
        .add_prebuilt_code_method(
            "choose",
            "()I",
            MethodAccessFlags(ACC_PUBLIC | ACC_STATIC),
            code,
        )
        .unwrap();

    let bytes = builder.encode().unwrap();
    let class_file = decode(&bytes);
    let method = method_named(&class_file, "choose");
    let code = code_attribute(method);
    assert_eq!(
        code.code,
        [0x04, 0x99, 0x00, 0x07, 0x05, 0xa7, 0x00, 0x04, 0x06, 0xac]
    );
    assert!(matches!(
        code.attributes.as_slice(),
        [Attribute::Other(attribute)] if utf8(&class_file, attribute.name_index) == "StackMapTable"
    ));
}

#[test]
fn stack_map_frames_preserve_exact_objects_in_locals_and_operand_stack() {
    let mut builder = ClassFileBuilder::new(
        "example/ReferenceFrames",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    builder
        .add_static_method("branch", "(I)V", |code| {
            let target = code.new_label()?;
            code.new_object("example/Alpha")?;
            code.dup()?;
            code.invokespecial("example/Alpha", "<init>", "()V")?;
            code.astore(1)?;
            code.new_object("example/Beta")?;
            code.dup()?;
            code.invokespecial("example/Beta", "<init>", "()V")?;
            code.astore(3)?;
            code.aload(1)?;
            code.iload(0)?;
            code.ifeq(target.clone())?;
            code.pop()?;
            code.return_void()?;
            code.mark(target)?;
            code.pop()?;
            code.return_void()?;
            Ok(())
        })
        .unwrap();

    let bytes = builder.encode().unwrap();
    let class_file = decode(&bytes);
    let method = method_named(&class_file, "branch");
    let code = code_attribute(method);
    assert_eq!((code.max_stack, code.max_locals), (2, 4));
    let [Attribute::Other(stack_map)] = code.attributes.as_slice() else {
        panic!("expected one StackMapTable, got {:?}", code.attributes);
    };
    let bytes = stack_map.bytes;
    let mut cursor = 0;
    assert_eq!(u16::from_be_bytes([bytes[cursor], bytes[cursor + 1]]), 1);
    cursor += 2;
    assert_eq!(bytes[cursor], 255);
    cursor += 1;
    cursor += 2; // offset_delta
    assert_eq!(u16::from_be_bytes([bytes[cursor], bytes[cursor + 1]]), 4);
    cursor += 2;
    assert_eq!(bytes[cursor], 1); // Integer parameter
    cursor += 1;
    let alpha_index = dotty_classfile::constant_pool::ConstantPoolIndex(u16::from_be_bytes([
        bytes[cursor + 1],
        bytes[cursor + 2],
    ]));
    assert_eq!(bytes[cursor], 7);
    cursor += 3;
    assert_eq!(bytes[cursor], 0); // interior Top hole
    cursor += 1;
    let beta_index = dotty_classfile::constant_pool::ConstantPoolIndex(u16::from_be_bytes([
        bytes[cursor + 1],
        bytes[cursor + 2],
    ]));
    assert_eq!(bytes[cursor], 7);
    cursor += 3;
    assert_eq!(u16::from_be_bytes([bytes[cursor], bytes[cursor + 1]]), 1);
    cursor += 2;
    assert_eq!(bytes[cursor], 7); // exact object live on operand stack
    let stack_alpha_index =
        dotty_classfile::constant_pool::ConstantPoolIndex(u16::from_be_bytes([
            bytes[cursor + 1],
            bytes[cursor + 2],
        ]));
    assert_eq!(alpha_index, stack_alpha_index);
    assert_eq!(
        class_file.constant_pool.class_name(alpha_index),
        Ok("example/Alpha")
    );
    assert_eq!(
        class_file.constant_pool.class_name(beta_index),
        Ok("example/Beta")
    );
}

#[test]
fn rejects_unreachable_blocks_before_automatic_stack_map_emission() {
    let mut builder = ClassFileBuilder::new(
        "example/UnreachableIntegerBlock",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    builder
        .add_static_method("branch", "()I", |code| {
            let target = code.new_label()?;
            code.goto(target.clone())?;
            code.iconst(0)?;
            code.ireturn()?;
            code.mark(target)?;
            code.iconst(1)?;
            code.ireturn()?;
            Ok(())
        })
        .unwrap();

    assert!(matches!(
        builder.encode(),
        Err(ClassFileBuildError::Analysis(
            dotty_classfile::bytecode::AnalysisError::UnreachableBlock { block }
        )) if block.index() == 1
    ));
}

#[test]
fn rejects_foreign_unbound_duplicate_and_old_version_branch_labels() {
    let mut foreign_builder = dotty_classfile::classfile::CodeBuilder::new();
    let foreign = foreign_builder.new_label().unwrap();
    let mut cloned_source = dotty_classfile::classfile::CodeBuilder::new();
    let cloned_source_label = cloned_source.new_label().unwrap();
    let mut cloned_builder = cloned_source.clone();
    assert_eq!(
        cloned_builder.goto(cloned_source_label).err(),
        Some(CodeBuilderError::ForeignLabel { label: 0 })
    );
    let mut builder = ClassFileBuilder::new(
        "example/BranchErrors",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();

    assert_eq!(
        builder
            .add_static_method("foreign", "()V", |code| {
                code.goto(foreign.clone())?;
                Ok(())
            })
            .err(),
        Some(ClassFileBuildError::CodeBuilder(
            CodeBuilderError::ForeignLabel { label: 0 }
        ))
    );
    assert_eq!(
        builder
            .add_static_method("unbound", "()V", |code| {
                let label = code.new_label()?;
                code.goto(label)?;
                Ok(())
            })
            .err(),
        Some(ClassFileBuildError::CodeBuilder(
            CodeBuilderError::UnboundLabel { label: 0 }
        ))
    );
    assert_eq!(
        builder
            .add_static_method("duplicate", "()V", |code| {
                let label = code.new_label()?;
                code.mark(label.clone())?;
                code.mark(label)?;
                Ok(())
            })
            .err(),
        Some(ClassFileBuildError::CodeBuilder(
            CodeBuilderError::DuplicateLabel { label: 0 }
        ))
    );
    // Failed closures do not publish method declarations.
    builder
        .add_static_method("unbound", "()V", |code| {
            code.return_void()?;
            Ok(())
        })
        .unwrap();

    let mut old_version = ClassFileBuilder::new(
        "example/OldBranchVersion",
        ClassFileVersion {
            major: 61,
            minor: 0,
        },
    )
    .unwrap();
    assert_eq!(
        old_version
            .add_static_method("branch", "()V", |code| {
                let label = code.new_label()?;
                code.goto(label.clone())?;
                code.mark(label)?;
                code.return_void()?;
                Ok(())
            })
            .err(),
        Some(ClassFileBuildError::BranchingRequiresVersion { major: 61 })
    );
}

#[test]
fn branched_layout_uses_wide_locals_and_ldc_w_before_frame_targets() {
    let mut wide_builder = ClassFileBuilder::new(
        "example/WideBranch",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    wide_builder
        .add_static_method("wide", "()V", |code| {
            let target = code.new_label()?;
            code.iconst(1)?;
            code.istore(256)?;
            code.iconst(0)?;
            code.ifeq(target.clone())?;
            code.return_void()?;
            code.mark(target)?;
            code.return_void()?;
            Ok(())
        })
        .unwrap();
    let wide_bytes = wide_builder.encode().unwrap();
    let wide_class = decode(&wide_bytes);
    let wide = code_attribute(method_named(&wide_class, "wide"));
    assert_eq!(wide.max_locals, 257);
    assert_eq!(&wide.code[..6], &[0x04, 0xc4, 0x36, 0x01, 0x00, 0x03]);
    assert_eq!(&wide.code[6..], &[0x99, 0x00, 0x04, 0xb1, 0xb1]);
    let [Attribute::Other(wide_stack_map)] = wide.attributes.as_slice() else {
        panic!("wide branched code should contain one StackMapTable");
    };
    assert_eq!(wide_stack_map.bytes.len(), 266);
    assert_eq!(
        &wide_stack_map.bytes[..7],
        &[0x00, 0x01, 0xff, 0x00, 0x0a, 0x01, 0x01]
    );
    assert!(wide_stack_map.bytes[7..263].iter().all(|entry| *entry == 0));
    assert_eq!(&wide_stack_map.bytes[263..], &[0x01, 0x00, 0x00]);

    let mut ldc_builder = ClassFileBuilder::new(
        "example/LdcWideBranch",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    for index in 0..260 {
        ldc_builder
            .add_field(format!("field{index}"), "I", FieldAccessFlags(ACC_PRIVATE))
            .unwrap();
    }
    ldc_builder
        .add_static_method("ldcWide", "()V", |code| {
            let target = code.new_label()?;
            code.iconst(123_456)?;
            code.ifeq(target.clone())?;
            code.return_void()?;
            code.mark(target)?;
            code.return_void()?;
            Ok(())
        })
        .unwrap();
    let ldc_bytes = ldc_builder.encode().unwrap();
    let ldc_class = decode(&ldc_bytes);
    let ldc = code_attribute(method_named(&ldc_class, "ldcWide"));
    assert_eq!(ldc.code.len(), 8);
    assert_eq!(ldc.code[0], 0x13);
    let integer_index = dotty_classfile::constant_pool::ConstantPoolIndex(u16::from_be_bytes([
        ldc.code[1],
        ldc.code[2],
    ]));
    assert!(matches!(
        ldc_class.constant_pool.get(integer_index),
        Some(ConstantPoolEntry::Integer(123_456))
    ));
    assert_eq!(&ldc.code[3..], &[0x99, 0x00, 0x04, 0xb1, 0xb1]);
    let [Attribute::Other(ldc_stack_map)] = ldc.attributes.as_slice() else {
        panic!("ldc_w branch should contain one StackMapTable");
    };
    assert_eq!(
        ldc_stack_map.bytes,
        [0x00, 0x01, 0xff, 0x00, 0x07, 0x00, 0x00, 0x00, 0x00]
    );
}

#[test]
fn rejects_unsupported_branch_frames_stack_merges_uninitialized_locals_and_terminal_labels() {
    let version = ClassFileVersion {
        major: 65,
        minor: 0,
    };
    let mut references = ClassFileBuilder::new("example/ReferenceBranch", version).unwrap();
    references
        .add_static_method("unsupported", "()Ljava/lang/Object;", |code| {
            let other = code.new_label()?;
            let join = code.new_label()?;
            code.iconst(0)?;
            code.ifeq(other.clone())?;
            code.aconst_null()?;
            code.goto(join.clone())?;
            code.mark(other)?;
            code.aconst_null()?;
            code.mark(join)?;
            code.areturn()?;
            Ok(())
        })
        .unwrap();
    assert!(matches!(
        references.encode(),
        Err(ClassFileBuildError::Analysis(
            dotty_classfile::bytecode::AnalysisError::UnsupportedFlowInstruction {
                kind: dotty_classfile::bytecode::UnsupportedFlowInstructionKind::Reference,
                ..
            }
        ))
    ));

    let mut unreachable_reference =
        ClassFileBuilder::new("example/UnreachableReferenceBranch", version).unwrap();
    unreachable_reference
        .add_static_method("unsupported", "()V", |code| {
            let target = code.new_label()?;
            code.iconst(0)?;
            code.ifeq(target.clone())?;
            code.return_void()?;
            code.aconst_null()?;
            code.return_void()?;
            code.mark(target)?;
            code.return_void()?;
            Ok(())
        })
        .unwrap();
    assert!(matches!(
        unreachable_reference.encode(),
        Err(ClassFileBuildError::Analysis(
            dotty_classfile::bytecode::AnalysisError::UnsupportedFlowInstruction {
                kind: dotty_classfile::bytecode::UnsupportedFlowInstructionKind::Reference,
                ..
            }
        ))
    ));

    let mut category_two = ClassFileBuilder::new("example/CategoryTwoBranch", version).unwrap();
    category_two
        .add_static_method("unsupported", "()V", |code| {
            let target = code.new_label()?;
            code.lconst(1)?;
            code.ifeq(target.clone())?;
            code.return_void()?;
            code.mark(target)?;
            code.return_void()?;
            Ok(())
        })
        .unwrap();
    assert!(matches!(
        category_two.encode(),
        Err(ClassFileBuildError::Analysis(
            dotty_classfile::bytecode::AnalysisError::UnsupportedFlowInstruction {
                kind: dotty_classfile::bytecode::UnsupportedFlowInstructionKind::CategoryTwo,
                ..
            }
        ))
    ));

    let mut stack_mismatch = ClassFileBuilder::new("example/StackMismatchBranch", version).unwrap();
    stack_mismatch
        .add_static_method("invalid", "()V", |code| {
            let other = code.new_label()?;
            let join = code.new_label()?;
            code.iconst(0)?;
            code.ifeq(other.clone())?;
            code.iconst(1)?;
            code.goto(join.clone())?;
            code.mark(other)?;
            code.mark(join)?;
            code.return_void()?;
            Ok(())
        })
        .unwrap();
    assert!(matches!(
        stack_mismatch.encode(),
        Err(ClassFileBuildError::Analysis(
            dotty_classfile::bytecode::AnalysisError::IncompatibleStackMerge { .. }
        ))
    ));

    let mut uninitialized = ClassFileBuilder::new("example/UninitializedBranch", version).unwrap();
    uninitialized
        .add_static_method("invalid", "()I", |code| {
            let target = code.new_label()?;
            code.iconst(0)?;
            code.ifeq(target.clone())?;
            code.iload(0)?;
            code.ireturn()?;
            code.mark(target)?;
            code.iconst(0)?;
            code.ireturn()?;
            Ok(())
        })
        .unwrap();
    assert!(matches!(
        uninitialized.encode(),
        Err(ClassFileBuildError::Analysis(
            dotty_classfile::bytecode::AnalysisError::UninitializedLocal { index: 0, .. }
        ))
    ));

    let mut terminal_label = ClassFileBuilder::new("example/TerminalLabelBranch", version).unwrap();
    assert_eq!(
        terminal_label
            .add_static_method("invalid", "()V", |code| {
                let end = code.new_label()?;
                code.goto(end.clone())?;
                code.mark(end)?;
                Ok(())
            })
            .err(),
        Some(ClassFileBuildError::CodeBuilder(
            CodeBuilderError::BranchToTerminalEnd { label: 0 }
        ))
    );

    let mut wrong_return = ClassFileBuilder::new("example/WrongBranchReturn", version).unwrap();
    wrong_return
        .add_static_method("invalid", "()V", |code| {
            code.iconst(0)?;
            code.ireturn()?;
            Ok(())
        })
        .unwrap();
    assert!(matches!(
        wrong_return.encode(),
        Err(ClassFileBuildError::Analysis(
            dotty_classfile::bytecode::AnalysisError::WrongReturnOpcode {
                expected: None,
                actual: Some(dotty_classfile::bytecode::ValueKind::Int),
                ..
            }
        ))
    ));
}

#[test]
fn rejects_branched_reference_return_until_result_type_is_checked() {
    let mut builder = ClassFileBuilder::new(
        "example/InvalidReferenceReturn",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    builder
        .add_static_method("invalid", "()Ljava/lang/String;", |code| {
            let target = code.new_label()?;
            code.new_object("java/lang/Object")?;
            code.dup()?;
            code.invokespecial("java/lang/Object", "<init>", "()V")?;
            code.goto(target.clone())?;
            code.mark(target)?;
            code.areturn()?;
            Ok(())
        })
        .unwrap();

    assert!(matches!(
        builder.encode(),
        Err(ClassFileBuildError::Analysis(
            dotty_classfile::bytecode::AnalysisError::UnsupportedFlowInstruction {
                kind: dotty_classfile::bytecode::UnsupportedFlowInstructionKind::Reference,
                ..
            }
        ))
    ));
}

#[test]
fn class_builder_rejects_overflowing_short_branch_displacement() {
    let version = ClassFileVersion {
        major: 65,
        minor: 0,
    };
    let mut builder = ClassFileBuilder::new("example/BranchOverflow", version).unwrap();
    builder
        .add_static_method("overflow", "()I", |code| {
            let target = code.new_label()?;
            code.iconst(0)?;
            code.ifeq(target.clone())?;
            for _ in 0..16_384 {
                code.iconst(0)?;
                code.istore(0)?;
            }
            code.mark(target)?;
            code.iconst(1)?;
            code.ireturn()?;
            Ok(())
        })
        .unwrap();

    assert!(matches!(
        builder.encode(),
        Err(ClassFileBuildError::Assemble(
            dotty_classfile::bytecode::AssembleError::BranchOffsetOutOfRange { .. }
        ))
    ));
}

#[test]
fn builds_constructor_and_instance_field_access_from_symbolic_code() {
    let mut builder = ClassFileBuilder::new(
        "example/Counter",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    builder
        .add_field("value", "I", FieldAccessFlags(ACC_PRIVATE))
        .unwrap();
    builder
        .add_instance_method("<init>", "()V", |code| {
            code.aload(0)?;
            code.invokespecial("java/lang/Object", "<init>", "()V")?;
            code.return_void()?;
            Ok(())
        })
        .unwrap();
    builder
        .add_instance_method("getValue", "()I", |code| {
            code.aload(0)?;
            code.getfield("example/Counter", "value", "I")?;
            code.ireturn()?;
            Ok(())
        })
        .unwrap();
    builder
        .add_instance_method("setValue", "(I)V", |code| {
            code.aload(0)?;
            code.iload(1)?;
            code.putfield("example/Counter", "value", "I")?;
            code.return_void()?;
            Ok(())
        })
        .unwrap();

    let bytes = builder.encode().unwrap();
    let class_file = decode(&bytes);
    assert_eq!(class_file.fields.len(), 1);
    assert_eq!(class_file.methods.len(), 3);
    assert!(matches!(
        class_file.methods[0].attributes.as_slice(),
        [Attribute::Code(code)]
            if code.max_stack == 1 && code.max_locals == 1
                && code.code.len() == 5 && code.code[0] == 0x2a
                && code.code[1] == 0xb7 && code.code[4] == 0xb1
    ));
    assert!(matches!(
        class_file.methods[1].attributes.as_slice(),
        [Attribute::Code(code)]
            if code.max_stack == 1 && code.max_locals == 1
                && code.code[0] == 0x2a && code.code[1] == 0xb4 && code.code[4] == 0xac
    ));
    assert!(matches!(
        class_file.methods[2].attributes.as_slice(),
        [Attribute::Code(code)]
            if code.max_stack == 2 && code.max_locals == 2
                && code.code[0] == 0x2a && code.code[1] == 0x1b
                && code.code[2] == 0xb5 && code.code[5] == 0xb1
    ));
}

#[test]
fn rejects_branched_constructors_without_uninitialized_this_flow() {
    let mut builder = ClassFileBuilder::new(
        "example/BranchedConstructor",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    builder
        .add_instance_method("<init>", "()V", |code| {
            let done = code.new_label()?;
            code.aload(0)?;
            code.invokespecial("java/lang/Object", "<init>", "()V")?;
            code.iconst(0)?;
            code.ifeq(done.clone())?;
            code.return_void()?;
            code.mark(done)?;
            code.return_void()?;
            Ok(())
        })
        .unwrap();

    assert!(matches!(
        builder.encode(),
        Err(ClassFileBuildError::StackMapFrames(
            dotty_classfile::bytecode::StackMapFrameGenerationError::UnsupportedFlow {
                state: dotty_classfile::bytecode::UnsupportedStackMapState::InstanceReceiver,
                ..
            }
        ))
    ));
}

#[test]
fn builds_new_dup_and_constructor_invocation_from_symbolic_code() {
    let mut builder = ClassFileBuilder::new(
        "example/Factory",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    builder
        .add_static_method("create", "()Lexample/Widget;", |code| {
            code.new_object("example/Widget")?;
            code.dup()?;
            code.invokespecial("example/Widget", "<init>", "()V")?;
            code.areturn()?;
            Ok(())
        })
        .unwrap();

    let bytes = builder.encode().unwrap();
    let class_file = decode(&bytes);
    assert!(matches!(
        class_file.methods[0].attributes.as_slice(),
        [Attribute::Code(code)]
            if code.max_stack == 2 && code.max_locals == 0
                && code.code[0] == 0xbb && code.code[3] == 0x59
                && code.code[4] == 0xb7 && code.code[7] == 0xb0
    ));
}

#[test]
fn rejects_interface_constructor_invocation_without_committing_the_method() {
    let mut builder = ClassFileBuilder::new(
        "example/InterfaceConstructor",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();

    let error = match builder.add_static_method("candidate", "()V", |code| {
        code.aconst_null()?;
        code.invokespecial_interface("example/Api", "<init>", "()V")?;
        code.return_void()?;
        Ok(())
    }) {
        Err(error) => error,
        Ok(_) => panic!("interface constructor invocation should fail"),
    };
    assert!(matches!(
        error,
        ClassFileBuildError::CodeBuilder(CodeBuilderError::InvalidName)
    ));

    // The rejected closure leaves no partial declaration behind.
    builder
        .add_static_method("candidate", "()V", |code| {
            code.return_void()?;
            Ok(())
        })
        .unwrap();
    assert_eq!(decode(&builder.encode().unwrap()).methods.len(), 1);
}

#[test]
fn interface_method_reference_versions_are_checked_but_raw_code_remains_opaque() {
    for is_special in [false, true] {
        let mut builder = ClassFileBuilder::new(
            "example/OldInterfaceCall",
            ClassFileVersion {
                major: 51,
                minor: 0,
            },
        )
        .unwrap();
        if is_special {
            builder
                .add_code_method("call", "()V", MethodAccessFlags(ACC_PUBLIC), |code| {
                    code.aload(0)?;
                    code.invokespecial_interface("example/Api", "touch", "()V")?;
                    code.return_void()?;
                    Ok(())
                })
                .unwrap();
        } else {
            builder
                .add_static_method("call", "()V", |code| {
                    code.invokestatic_interface("example/Api", "touch", "()V")?;
                    code.return_void()?;
                    Ok(())
                })
                .unwrap();
        }
        let error = builder.encode().unwrap_err();
        assert_eq!(
            error,
            ClassFileBuildError::InterfaceMethodReferenceRequiresVersion { major: 51 }
        );
    }

    let mut old_raw = ClassFileBuilder::new(
        "example/OldRawCode",
        ClassFileVersion {
            major: 51,
            minor: 0,
        },
    )
    .unwrap();
    old_raw
        .add_method("run", "()V", MethodAccessFlags(ACC_PUBLIC))
        .unwrap()
        .code(0, 1, [0xb1])
        .unwrap();
    assert_eq!(decode(&old_raw.encode().unwrap()).version.major, 51);

    let mut modern = ClassFileBuilder::new(
        "example/ModernInterfaceCall",
        ClassFileVersion {
            major: 52,
            minor: 0,
        },
    )
    .unwrap();
    modern
        .add_static_method("staticCall", "()V", |code| {
            code.invokestatic_interface("example/Api", "touch", "()V")?;
            code.return_void()?;
            Ok(())
        })
        .unwrap();
    modern
        .add_code_method(
            "specialCall",
            "()V",
            MethodAccessFlags(ACC_PUBLIC),
            |code| {
                code.aload(0)?;
                code.invokespecial_interface("example/Api", "touch", "()V")?;
                code.return_void()?;
                Ok(())
            },
        )
        .unwrap();
    let bytes = modern.encode().unwrap();
    let modern = decode(&bytes);
    assert_eq!(modern.version.major, 52);
    for (method, (opcode, index_offset)) in modern
        .methods
        .iter()
        .zip([(0xb8_u8, 1_usize), (0xb7_u8, 2_usize)])
    {
        let code = match &method.attributes[..] {
            [Attribute::Code(code)] => code,
            _ => panic!("typed methods should have one Code attribute each"),
        };
        assert_eq!(code.code[index_offset - 1], opcode);
        let index = u16::from_be_bytes([code.code[index_offset], code.code[index_offset + 1]]);
        assert!(matches!(
            modern
                .constant_pool
                .get(dotty_classfile::constant_pool::ConstantPoolIndex(index)),
            Some(ConstantPoolEntry::InterfaceMethodref { .. })
        ));
    }
}

#[test]
fn assembles_many_typed_methods_against_one_finalized_pool() {
    let mut builder = ClassFileBuilder::new(
        "example/ManyMethods",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    for index in 0..256 {
        builder
            .add_static_method(format!("method{index}"), "()I", |code| {
                code.iconst(7)?;
                code.ireturn()?;
                Ok(())
            })
            .unwrap();
    }

    let bytes = builder.encode().unwrap();
    let class_file = decode(&bytes);
    assert_eq!(class_file.methods.len(), 256);
    assert!(class_file.methods.iter().all(|method| matches!(
        method.attributes.as_slice(),
        [Attribute::Code(code)] if code.code == [0x10, 7, 0xac] && code.max_stack == 1
    )));
}

#[test]
fn interns_wide_integer_and_long_constants_for_typed_methods() {
    let mut builder = ClassFileBuilder::new(
        "example/Constants",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    builder
        .add_static_method("integer", "()I", |code| {
            code.iconst(70_000)?;
            code.ireturn()?;
            Ok(())
        })
        .unwrap();
    builder
        .add_static_method("long", "()J", |code| {
            code.lconst(2)?;
            code.lreturn()?;
            Ok(())
        })
        .unwrap();

    let bytes = builder.encode().unwrap();
    let class_file = decode(&bytes);
    let integer = match &class_file.methods[0].attributes[..] {
        [Attribute::Code(code)] => code,
        _ => panic!("typed methods should have one Code attribute each"),
    };
    assert_eq!(integer.code[0], 0x12);
    assert_eq!(integer.code[2], 0xac);
    let integer_index = u16::from(integer.code[1]);
    assert!(matches!(
        class_file
            .constant_pool
            .get(dotty_classfile::constant_pool::ConstantPoolIndex(
                integer_index
            )),
        Some(ConstantPoolEntry::Integer(70_000))
    ));

    let long = match &class_file.methods[1].attributes[..] {
        [Attribute::Code(code)] => code,
        _ => panic!("typed methods should have one Code attribute each"),
    };
    assert_eq!(long.code[0], 0x14);
    assert_eq!(long.code[3], 0xad);
    let long_index = u16::from_be_bytes([long.code[1], long.code[2]]);
    assert!(matches!(
        class_file
            .constant_pool
            .get(dotty_classfile::constant_pool::ConstantPoolIndex(
                long_index
            )),
        Some(ConstantPoolEntry::Long(2))
    ));
}

#[test]
fn interns_symbolic_calls_once_and_reuses_stable_instruction_indices() {
    let mut builder = ClassFileBuilder::new(
        "example/Calls",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    for name in ["first", "second"] {
        builder
            .add_static_method(name, "(I)I", |code| {
                code.iload(0)?;
                code.invokestatic("example/Math", "abs", "(I)I")?;
                code.ireturn()?;
                Ok(())
            })
            .unwrap();
    }
    builder
        .add_static_method("stringLength", "(Ljava/lang/String;)I", |code| {
            code.aload(0)?;
            code.invokevirtual("java/lang/String", "length", "()I")?;
            code.ireturn()?;
            Ok(())
        })
        .unwrap();
    builder
        .add_static_method("normalize", "(I)I", |code| {
            code.iload(0)?;
            code.invokestatic_interface("example/Normalizer", "normalize", "(I)I")?;
            code.ireturn()?;
            Ok(())
        })
        .unwrap();

    let bytes = builder.encode().unwrap();
    let class_file = decode(&bytes);
    let (first, second) = match (
        &class_file.methods[0].attributes[0],
        &class_file.methods[1].attributes[0],
    ) {
        (Attribute::Code(first), Attribute::Code(second)) => (first, second),
        _ => panic!("typed methods should have one Code attribute each"),
    };
    assert_eq!(first.code[1], 0xb8);
    assert_eq!(first.code[2..4], second.code[2..4]);
    let method_index = u16::from_be_bytes([first.code[2], first.code[3]]);
    assert!(matches!(
        class_file
            .constant_pool
            .get(dotty_classfile::constant_pool::ConstantPoolIndex(
                method_index
            )),
        Some(ConstantPoolEntry::Methodref { .. })
    ));
    assert!(matches!(
        class_file.methods[2].attributes.as_slice(),
        [Attribute::Code(code)] if code.code[1] == 0xb6 && code.max_locals == 1
    ));
    let interface_call = match &class_file.methods[3].attributes[..] {
        [Attribute::Code(code)] => code,
        _ => panic!("typed methods should have one Code attribute each"),
    };
    assert_eq!(interface_call.code[1], 0xb8);
    let interface_index = u16::from_be_bytes([interface_call.code[2], interface_call.code[3]]);
    assert!(matches!(
        class_file
            .constant_pool
            .get(dotty_classfile::constant_pool::ConstantPoolIndex(
                interface_index
            )),
        Some(ConstantPoolEntry::InterfaceMethodref { .. })
    ));
}

#[test]
fn failed_typed_method_construction_does_not_publish_a_partial_method() {
    let mut builder = ClassFileBuilder::new(
        "example/Failure",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    assert_eq!(
        builder
            .add_code_method(
                "answer",
                "()I",
                MethodAccessFlags(ACC_PUBLIC | ACC_STATIC),
                |code| {
                    code.iconst(1)?;
                    code.iload(usize::MAX).map(|_| ())
                }
            )
            .err(),
        Some(ClassFileBuildError::CodeBuilder(
            CodeBuilderError::LocalIndexOutOfRange { index: usize::MAX }
        ))
    );
    builder
        .add_static_method("answer", "()I", |code| {
            code.iconst(42)?;
            code.ireturn()?;
            Ok(())
        })
        .unwrap();
    builder
        .add_static_method("invalid", "()I", |code| {
            code.ireturn()?;
            Ok(())
        })
        .unwrap();

    assert!(matches!(
        builder.encode(),
        Err(ClassFileBuildError::Analysis(
            dotty_classfile::bytecode::AnalysisError::StackUnderflow {
                instruction: 0,
                needed: 1,
                available: 0,
            }
        ))
    ));
}

#[test]
fn rejects_invalid_symbolic_references_and_descriptors() {
    let mut builder = ClassFileBuilder::new(
        "example/InvalidReference",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    assert!(matches!(
        builder.add_static_method("badField", "()I", |code| {
            code.getstatic("example/Owner", "value", "V")?;
            code.ireturn()?;
            Ok(())
        }),
        Err(ClassFileBuildError::CodeBuilder(
            CodeBuilderError::InvalidDescriptor(_)
        ))
    ));
    builder
        .add_static_method("badField", "()I", |code| {
            code.iconst(0)?;
            code.ireturn()?;
            Ok(())
        })
        .unwrap();
    assert!(builder.encode().is_ok());
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
fn validates_jvm_angle_brackets_by_member_kind() {
    let mut builder = ClassFileBuilder::new(
        "example/Names",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();

    for field_name in ["<init>", "<clinit>"] {
        builder
            .add_field(field_name, "I", FieldAccessFlags(ACC_PRIVATE))
            .unwrap();
    }
    for method_name in ["m<suffix", "m>suffix", ">m", "<m", "m>"] {
        assert_eq!(
            builder
                .add_method(method_name, "()V", MethodAccessFlags(ACC_PUBLIC))
                .err(),
            Some(ClassFileBuildError::InvalidMemberName)
        );
    }

    builder
        .add_method("<init>", "()V", MethodAccessFlags(ACC_PUBLIC))
        .unwrap()
        .code(0, 1, [0xb1])
        .unwrap();
    builder
        .add_method("<clinit>", "()V", MethodAccessFlags(ACC_STATIC))
        .unwrap()
        .code(0, 0, [0xb1])
        .unwrap();
    assert!(builder.encode().is_ok());
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
fn enforces_versioned_interface_class_and_method_flags() {
    let mut rejected_super = ClassFileBuilder::new(
        "example/RejectedSuper",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    assert_eq!(
        rejected_super
            .set_access_flags(ClassAccessFlags(
                ACC_PUBLIC | ACC_INTERFACE | ACC_ABSTRACT | ACC_SUPER,
            ))
            .err(),
        Some(ClassFileBuildError::InvalidAccessFlags)
    );
    assert!(rejected_super.encode().is_ok());

    let mut interface = ClassFileBuilder::new(
        "example/ApiFlags",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    assert_eq!(
        interface
            .set_access_flags(ClassAccessFlags(
                ACC_PUBLIC | ACC_INTERFACE | ACC_ABSTRACT | ACC_SUPER,
            ))
            .err(),
        Some(ClassFileBuildError::InvalidAccessFlags)
    );
    interface
        .set_access_flags(ClassAccessFlags(ACC_PUBLIC | ACC_INTERFACE | ACC_ABSTRACT))
        .unwrap();
    assert_eq!(
        interface
            .add_method("packagePrivate", "()V", MethodAccessFlags(ACC_ABSTRACT))
            .err(),
        Some(ClassFileBuildError::InvalidAccessFlags)
    );
    interface
        .add_method(
            "publicAbstract",
            "()V",
            MethodAccessFlags(ACC_PUBLIC | ACC_ABSTRACT),
        )
        .unwrap();
    interface
        .add_method("privateConcrete", "()V", MethodAccessFlags(ACC_PRIVATE))
        .unwrap()
        .code(0, 1, [0xb1])
        .unwrap();
    assert!(interface.encode().is_ok());

    let mut old_interface = ClassFileBuilder::new(
        "example/OldApi",
        ClassFileVersion {
            major: 51,
            minor: 0,
        },
    )
    .unwrap();
    old_interface
        .set_access_flags(ClassAccessFlags(ACC_PUBLIC | ACC_INTERFACE | ACC_ABSTRACT))
        .unwrap();
    assert_eq!(
        old_interface
            .add_method(
                "staticConcrete",
                "()V",
                MethodAccessFlags(ACC_PUBLIC | ACC_STATIC),
            )
            .err(),
        Some(ClassFileBuildError::InvalidAccessFlags)
    );
    assert_eq!(
        old_interface
            .add_method("packagePrivate", "()V", MethodAccessFlags(ACC_ABSTRACT))
            .err(),
        Some(ClassFileBuildError::InvalidAccessFlags)
    );
    old_interface
        .add_method(
            "abstractMethod",
            "()V",
            MethodAccessFlags(ACC_PUBLIC | ACC_ABSTRACT),
        )
        .unwrap();
    assert!(old_interface.encode().is_ok());

    let mut late_interface = ClassFileBuilder::new(
        "example/LateApiFlags",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    late_interface
        .add_method("packagePrivate", "()V", MethodAccessFlags(0))
        .unwrap()
        .code(0, 1, [0xb1])
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
