use dotty_classfile::classfile::{
    ACC_PRIVATE, ACC_PUBLIC, ACC_SUPER, Attribute, ClassAccessFlags, ClassFile, ClassFileBuilder,
    ClassFileVersion, CodeAttribute, ConstantPoolEntry, FieldAccessFlags, MethodAccessFlags,
    Reader,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after the Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "dotty-classfile-jvm-{}-{timestamp}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap_or_else(|error| {
            panic!("create temporary directory {}: {error}", path.display())
        });
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn parse_tool_major_version(version_output: &str) -> Option<u32> {
    version_output.split_whitespace().find_map(|token| {
        let token = token.trim_matches('"');
        let numeric_version = token.split(['-', '+']).next()?;
        if numeric_version.is_empty()
            || !numeric_version
                .chars()
                .all(|character| character.is_ascii_digit() || character == '.')
        {
            return None;
        }

        let mut components = numeric_version.split('.');
        let first = components.next()?.parse::<u32>().ok()?;
        if first == 1 {
            components.next()?.parse::<u32>().ok()
        } else {
            Some(first)
        }
    })
}

fn executable_major_version(executable: &str) -> Option<u32> {
    let output = Command::new(executable).arg("-version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    parse_tool_major_version(&text)
}

fn require_jdk_or_skip() -> bool {
    let unavailable: Vec<String> = ["java", "javac", "javap"]
        .into_iter()
        .filter_map(|executable| match executable_major_version(executable) {
            Some(version) if version >= 21 => None,
            Some(version) => Some(format!("{executable} {version} (requires JDK 21+)")),
            None => Some(format!("{executable} (missing or version unavailable)")),
        })
        .collect();
    if unavailable.is_empty() {
        return true;
    }

    let message = format!(
        "JVM encoder gate requires java, javac, and javap from JDK 21+ on PATH; unavailable: {}",
        unavailable.join(", ")
    );
    if std::env::var("DOTTY_CLASSFILE_REQUIRE_JDK").as_deref() == Ok("1") {
        panic!("{message}; CI sets DOTTY_CLASSFILE_REQUIRE_JDK=1 to make this gate mandatory");
    }
    eprintln!("skipping JVM encoder gate: {message}");
    false
}

#[test]
fn parses_jdk_version_formats_and_rejects_older_major_versions() {
    assert_eq!(
        parse_tool_major_version("openjdk version \"17.0.18\" 2025-10-21"),
        Some(17)
    );
    assert_eq!(parse_tool_major_version("javac 21.0.12"), Some(21));
    assert_eq!(parse_tool_major_version("21-ea"), Some(21));
    assert_eq!(parse_tool_major_version("javac version unknown"), None);
}

fn run(executable: &str, args: &[&str]) -> Output {
    Command::new(executable)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("failed to start {executable}: {error}"))
}

fn decode_and_validate(bytes: &[u8]) -> ClassFile<'_> {
    let mut reader = Reader::new(bytes);
    let class_file = ClassFile::decode(&mut reader).expect("builder output decodes");
    assert!(
        reader.is_at_end(),
        "decoder consumed the complete class file"
    );
    class_file
        .validate_references()
        .expect("builder output has valid constant-pool references");
    class_file
}

fn code<'a, 'b>(method: &'a dotty_classfile::classfile::MethodInfo<'b>) -> &'a CodeAttribute<'b> {
    match method.attributes.as_slice() {
        [Attribute::Code(code)] => code,
        attributes => panic!("expected exactly one Code attribute, got {attributes:?}"),
    }
}

fn operand_index(
    code: &CodeAttribute<'_>,
    offset: usize,
) -> dotty_classfile::classfile::ConstantPoolIndex {
    dotty_classfile::classfile::ConstantPoolIndex(u16::from_be_bytes([
        code.code[offset],
        code.code[offset + 1],
    ]))
}

#[test]
fn public_builders_emit_executable_calculator_box_and_caller_classes() {
    let directory = TempDir::new();
    let version = ClassFileVersion {
        major: 65,
        minor: 0,
    };

    let mut calculator =
        ClassFileBuilder::new("Calculator", version).expect("Java 21 class builder is supported");
    calculator
        .set_access_flags(ClassAccessFlags(ACC_PUBLIC | ACC_SUPER))
        .unwrap();
    calculator
        .add_static_method("add", "(II)I", |code| {
            code.iload(0)?;
            code.iload(1)?;
            code.iadd()?;
            code.ireturn()?;
            Ok(())
        })
        .expect("typed method construction succeeds");
    calculator
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
        .expect("branched max method construction succeeds");
    calculator
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
        .expect("branched sumTo method construction succeeds");
    let calculator_bytes = calculator
        .encode()
        .expect("Calculator is generated through the typed builder");
    let calculator_path = directory.path().join("Calculator.class");
    fs::write(&calculator_path, &calculator_bytes)
        .unwrap_or_else(|error| panic!("write {}: {error}", calculator_path.display()));

    let mut box_class =
        ClassFileBuilder::new("Box", version).expect("Java 21 class builder is supported");
    box_class
        .set_access_flags(ClassAccessFlags(ACC_PUBLIC | ACC_SUPER))
        .unwrap();
    box_class
        .add_field("value", "I", FieldAccessFlags(ACC_PRIVATE))
        .unwrap();
    box_class
        .add_code_method("<init>", "(I)V", MethodAccessFlags(ACC_PUBLIC), |code| {
            code.aload(0)?;
            code.invokespecial("java/lang/Object", "<init>", "()V")?;
            code.aload(0)?;
            code.iload(1)?;
            code.putfield("Box", "value", "I")?;
            code.return_void()?;
            Ok(())
        })
        .expect("typed constructor construction succeeds");
    box_class
        .add_instance_method("get", "()I", |code| {
            code.aload(0)?;
            code.getfield("Box", "value", "I")?;
            code.ireturn()?;
            Ok(())
        })
        .expect("typed getter construction succeeds");
    let box_bytes = box_class
        .encode()
        .expect("Box is generated through the typed builder");
    let box_path = directory.path().join("Box.class");
    fs::write(&box_path, &box_bytes)
        .unwrap_or_else(|error| panic!("write {}: {error}", box_path.display()));

    let mut caller =
        ClassFileBuilder::new("Caller", version).expect("Java 21 class builder is supported");
    caller
        .set_access_flags(ClassAccessFlags(ACC_PUBLIC | ACC_SUPER))
        .unwrap();
    caller
        .add_static_method("answer", "()I", |code| {
            code.iconst(20)?;
            code.iconst(22)?;
            code.invokestatic("Calculator", "add", "(II)I")?;
            code.ireturn()?;
            Ok(())
        })
        .expect("typed caller method construction succeeds");
    caller
        .add_static_method("makeBox", "(I)LBox;", |code| {
            code.new_object("Box")?;
            code.dup()?;
            code.iload(0)?;
            code.invokespecial("Box", "<init>", "(I)V")?;
            code.areturn()?;
            Ok(())
        })
        .expect("typed factory construction succeeds");
    let caller_bytes = caller
        .encode()
        .expect("Caller is generated through the typed builder");
    let caller_path = directory.path().join("Caller.class");
    fs::write(&caller_path, &caller_bytes)
        .unwrap_or_else(|error| panic!("write {}: {error}", caller_path.display()));

    let calculator = decode_and_validate(&calculator_bytes);
    assert_eq!(calculator.version, version);
    assert_eq!(calculator.access_flags.0, ACC_PUBLIC | ACC_SUPER);
    assert_eq!(calculator.methods.len(), 3);
    let add = &calculator.methods[0];
    assert_eq!(calculator.constant_pool.utf8(add.name_index), Ok("add"));
    assert_eq!(
        calculator.constant_pool.utf8(add.descriptor_index),
        Ok("(II)I")
    );
    assert_eq!(
        add.access_flags.0,
        ACC_PUBLIC | dotty_classfile::classfile::ACC_STATIC
    );
    let add_code = code(add);
    assert_eq!((add_code.max_stack, add_code.max_locals), (2, 2));
    assert_eq!(add_code.code, [0x1a, 0x1b, 0x60, 0xac]);
    assert!(add_code.exception_table.is_empty());
    assert!(add_code.attributes.is_empty());

    let max = code(&calculator.methods[1]);
    assert_eq!((max.max_stack, max.max_locals), (2, 2));
    assert_eq!(
        max.code,
        [0x1a, 0x1b, 0xa2, 0x00, 0x05, 0x1b, 0xac, 0x1a, 0xac]
    );
    assert!(
        matches!(max.attributes.as_slice(), [Attribute::Other(attribute)]
        if calculator.constant_pool.utf8(attribute.name_index) == Ok("StackMapTable"))
    );

    let sum_to = code(&calculator.methods[2]);
    assert_eq!((sum_to.max_stack, sum_to.max_locals), (2, 3));
    assert_eq!(sum_to.code.len(), 22);
    assert_eq!(sum_to.attributes.len(), 1);
    assert!(
        matches!(sum_to.attributes.as_slice(), [Attribute::Other(attribute)]
        if calculator.constant_pool.utf8(attribute.name_index) == Ok("StackMapTable"))
    );

    let box_file = decode_and_validate(&box_bytes);
    assert_eq!(box_file.version, version);
    assert_eq!(box_file.access_flags.0, ACC_PUBLIC | ACC_SUPER);
    assert_eq!(box_file.fields.len(), 1);
    assert_eq!(
        box_file.constant_pool.utf8(box_file.fields[0].name_index),
        Ok("value")
    );
    assert_eq!(
        box_file
            .constant_pool
            .utf8(box_file.fields[0].descriptor_index),
        Ok("I")
    );
    assert_eq!(box_file.fields[0].access_flags.0, ACC_PRIVATE);
    assert_eq!(box_file.methods.len(), 2);
    assert_eq!(
        box_file.constant_pool.utf8(box_file.methods[0].name_index),
        Ok("<init>")
    );
    assert_eq!(box_file.methods[0].access_flags.0, ACC_PUBLIC);
    let constructor = code(&box_file.methods[0]);
    assert_eq!((constructor.max_stack, constructor.max_locals), (2, 2));
    assert_eq!(constructor.code.len(), 10);
    assert_eq!(constructor.code[0], 0x2a);
    assert_eq!(constructor.code[1], 0xb7);
    assert!(matches!(
        box_file.constant_pool.get(operand_index(constructor, 2)),
        Some(ConstantPoolEntry::Methodref { .. })
    ));
    assert_eq!(&constructor.code[4..6], &[0x2a, 0x1b]);
    assert_eq!(constructor.code[6], 0xb5);
    assert_eq!(constructor.code[9], 0xb1);
    assert!(constructor.exception_table.is_empty());
    assert!(constructor.attributes.is_empty());
    assert_eq!(
        box_file.constant_pool.utf8(box_file.methods[1].name_index),
        Ok("get")
    );
    assert_eq!(box_file.methods[1].access_flags.0, ACC_PUBLIC);
    let getter = code(&box_file.methods[1]);
    assert_eq!((getter.max_stack, getter.max_locals), (1, 1));
    assert_eq!(getter.code.len(), 5);
    assert_eq!(&getter.code[..1], &[0x2a]);
    assert_eq!(getter.code[1], 0xb4);
    assert!(matches!(
        box_file.constant_pool.get(operand_index(getter, 2)),
        Some(ConstantPoolEntry::Fieldref { .. })
    ));
    assert_eq!(getter.code[4], 0xac);
    assert!(getter.exception_table.is_empty());
    assert!(getter.attributes.is_empty());

    let caller = decode_and_validate(&caller_bytes);
    assert_eq!(caller.version, version);
    assert_eq!(caller.access_flags.0, ACC_PUBLIC | ACC_SUPER);
    assert_eq!(caller.methods.len(), 2);
    let answer = &caller.methods[0];
    assert_eq!(caller.constant_pool.utf8(answer.name_index), Ok("answer"));
    assert_eq!(
        caller.constant_pool.utf8(answer.descriptor_index),
        Ok("()I")
    );
    assert_eq!(
        answer.access_flags.0,
        ACC_PUBLIC | dotty_classfile::classfile::ACC_STATIC
    );
    let answer_code = code(answer);
    assert_eq!((answer_code.max_stack, answer_code.max_locals), (2, 0));
    assert_eq!(answer_code.code.len(), 8);
    assert_eq!(&answer_code.code[..4], &[0x10, 20, 0x10, 22]);
    assert_eq!(answer_code.code[4], 0xb8);
    assert!(matches!(
        caller.constant_pool.get(operand_index(answer_code, 5)),
        Some(ConstantPoolEntry::Methodref { .. })
    ));
    assert_eq!(answer_code.code[7], 0xac);
    assert!(answer_code.exception_table.is_empty());
    assert!(answer_code.attributes.is_empty());

    let factory = &caller.methods[1];
    assert_eq!(caller.constant_pool.utf8(factory.name_index), Ok("makeBox"));
    assert_eq!(
        caller.constant_pool.utf8(factory.descriptor_index),
        Ok("(I)LBox;")
    );
    assert_eq!(
        factory.access_flags.0,
        ACC_PUBLIC | dotty_classfile::classfile::ACC_STATIC
    );
    let factory_code = code(factory);
    assert_eq!((factory_code.max_stack, factory_code.max_locals), (3, 1));
    assert_eq!(factory_code.code.len(), 9);
    assert_eq!(factory_code.code[0], 0xbb);
    assert!(matches!(
        caller.constant_pool.get(operand_index(factory_code, 1)),
        Some(ConstantPoolEntry::Class { .. })
    ));
    assert_eq!(factory_code.code[3], 0x59);
    assert_eq!(factory_code.code[4], 0x1a);
    assert_eq!(factory_code.code[5], 0xb7);
    assert!(matches!(
        caller.constant_pool.get(operand_index(factory_code, 6)),
        Some(ConstantPoolEntry::Methodref { .. })
    ));
    assert_eq!(factory_code.code[8], 0xb0);
    assert!(factory_code.exception_table.is_empty());
    assert!(factory_code.attributes.is_empty());

    if !require_jdk_or_skip() {
        return;
    }

    let harness_path = directory.path().join("GeneratedClassesHarness.java");
    fs::write(
        &harness_path,
        "public final class GeneratedClassesHarness {\n    public static void main(String[] args) {\n        int calculator = Calculator.add(20, 22);\n        if (calculator != 42) throw new AssertionError(\"Calculator.add: \" + calculator);\n        int maximum = Calculator.max(12, 42);\n        if (maximum != 42) throw new AssertionError(\"Calculator.max: \" + maximum);\n        int sum = Calculator.sumTo(10);\n        if (sum != 55) throw new AssertionError(\"Calculator.sumTo: \" + sum);\n        int boxed = new Box(42).get();\n        if (boxed != 42) throw new AssertionError(\"Box.get: \" + boxed);\n        int caller = Caller.answer();\n        if (caller != 42) throw new AssertionError(\"Caller.answer: \" + caller);\n        int generated = Caller.makeBox(42).get();\n        if (generated != 42) throw new AssertionError(\"Caller.makeBox: \" + generated);\n        System.out.println(\"Calculator.add=\" + calculator);\n        System.out.println(\"Calculator.max=\" + maximum);\n        System.out.println(\"Calculator.sumTo=\" + sum);\n        System.out.println(\"Box.get=\" + boxed);\n        System.out.println(\"Caller.answer=\" + caller);\n        System.out.println(\"Caller.makeBox=\" + generated);\n    }\n}\n",
    )
    .unwrap_or_else(|error| panic!("write {}: {error}", harness_path.display()));

    let directory_arg = directory.path().to_string_lossy().into_owned();
    for (class_name, expected) in [
        (
            "Calculator",
            &[
                "public static int add(int, int);",
                "public static int max(int, int);",
                "public static int sumTo(int);",
                "StackMapTable: number_of_entries =",
                "stack=2, locals=2",
                "iload_0",
                "iload_1",
                "iadd",
            ][..],
        ),
        (
            "Box",
            &[
                "public Box(int);",
                "stack=2, locals=2",
                "stack=1, locals=1",
                "putfield",
                "getfield",
            ][..],
        ),
        (
            "Caller",
            &[
                "public static int answer();",
                "public static Box makeBox(int);",
                "invokestatic",
                "stack=2, locals=0",
                "bipush        20",
                "bipush        22",
                "new           #",
                "dup",
                "invokespecial",
                "stack=3, locals=1",
            ][..],
        ),
    ] {
        let javap = run("javap", &["-v", "-classpath", &directory_arg, class_name]);
        assert!(
            javap.status.success(),
            "javap -v rejected generated {class_name}.class\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javap.stdout),
            String::from_utf8_lossy(&javap.stderr)
        );
        let javap_output = String::from_utf8_lossy(&javap.stdout);
        for item in expected {
            assert!(
                javap_output.contains(item),
                "javap output for {class_name} did not contain {item:?}:\n{javap_output}"
            );
        }
    }

    let javac = run(
        "javac",
        &[
            "-classpath",
            &directory_arg,
            "-d",
            &directory_arg,
            harness_path.to_str().unwrap(),
        ],
    );
    assert!(
        javac.status.success(),
        "javac failed to compile the generated-class harness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );

    let java = run(
        "java",
        &[
            "-Xverify:all",
            "-classpath",
            &directory_arg,
            "GeneratedClassesHarness",
        ],
    );
    assert!(
        java.status.success(),
        "java -Xverify:all rejected or failed to execute generated classes\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "Calculator.add=42\nCalculator.max=42\nCalculator.sumTo=55\nBox.get=42\nCaller.answer=42\nCaller.makeBox=42"
    );
}
