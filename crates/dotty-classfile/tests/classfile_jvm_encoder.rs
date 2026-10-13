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

fn read_u2(bytes: &[u8], cursor: &mut usize) -> usize {
    let value = u16::from_be_bytes([bytes[*cursor], bytes[*cursor + 1]]);
    *cursor += 2;
    usize::from(value)
}

fn skip_verification_type(bytes: &[u8], cursor: &mut usize) {
    let tag = bytes[*cursor];
    *cursor += 1;
    match tag {
        0..=6 => {}
        7 | 8 => *cursor += 2,
        _ => panic!("invalid verification_type_info tag {tag}"),
    }
    assert!(*cursor <= bytes.len(), "truncated verification_type_info");
}

fn assert_stack_map_targets(
    class_file: &ClassFile<'_>,
    method: &dotty_classfile::classfile::MethodInfo<'_>,
    expected_frames: usize,
) -> Vec<usize> {
    let code = code(method);
    let [Attribute::Other(attribute)] = code.attributes.as_slice() else {
        panic!(
            "expected exactly one nested StackMapTable, got {:?}",
            code.attributes
        );
    };
    assert_eq!(
        class_file.constant_pool.utf8(attribute.name_index),
        Ok("StackMapTable")
    );

    let bytes = attribute.bytes;
    let mut cursor = 0;
    let frame_count = read_u2(bytes, &mut cursor);
    assert_eq!(frame_count, expected_frames);
    let mut frame_pcs = Vec::with_capacity(frame_count);
    for _ in 0..frame_count {
        assert_eq!(bytes[cursor], 255, "expected full_frame encoding");
        cursor += 1;
        let offset_delta = read_u2(bytes, &mut cursor);
        let pc = frame_pcs
            .last()
            .map_or(offset_delta, |previous| previous + offset_delta + 1);
        frame_pcs.push(pc);

        let locals = read_u2(bytes, &mut cursor);
        for _ in 0..locals {
            skip_verification_type(bytes, &mut cursor);
        }
        let stack = read_u2(bytes, &mut cursor);
        for _ in 0..stack {
            skip_verification_type(bytes, &mut cursor);
        }
    }
    assert_eq!(
        cursor,
        bytes.len(),
        "StackMapTable payload has trailing bytes"
    );
    assert!(frame_pcs.windows(2).all(|pair| pair[0] < pair[1]));

    let (instruction_pcs, branch_targets) = branch_targets(code.code);
    for (target, _) in &branch_targets {
        assert!(
            instruction_pcs.contains(target),
            "branch target {target} is not an instruction boundary"
        );
    }
    let mut expected_targets = branch_targets
        .iter()
        .map(|(target, _)| *target)
        .filter(|target| *target != 0)
        .collect::<Vec<_>>();
    expected_targets.sort_unstable();
    expected_targets.dedup();
    assert_eq!(frame_pcs, expected_targets);
    frame_pcs
}

fn branch_targets(code: &[u8]) -> (Vec<usize>, Vec<(usize, &'static str)>) {
    let mut instruction_pcs = Vec::new();
    let mut targets = Vec::new();
    let mut pc = 0;
    while pc < code.len() {
        instruction_pcs.push(pc);
        let opcode = code[pc];
        let (length, mnemonic) = match opcode {
            0x99 => (3, Some("ifeq")),
            0x9a => (3, Some("ifne")),
            0x9f => (3, Some("if_icmpeq")),
            0xa0 => (3, Some("if_icmpne")),
            0xa1 => (3, Some("if_icmplt")),
            0xa2 => (3, Some("if_icmpge")),
            0xa3 => (3, Some("if_icmpgt")),
            0xa4 => (3, Some("if_icmple")),
            0xa7 => (3, Some("goto")),
            0xc8 => (5, Some("goto_w")),
            0x10 | 0x12 | 0x15 | 0x36 => (2, None),
            0x11 | 0x13 | 0xb4 | 0xb5 | 0xb6 | 0xb7 | 0xb8 | 0xbb => (3, None),
            0x00..=0x0f | 0x1a..=0x2d | 0x3b..=0x4e | 0x57 | 0x59 | 0x60 | 0xac | 0xb0 | 0xb1 => {
                (1, None)
            }
            _ => panic!("unexpected opcode 0x{opcode:02x} at PC {pc}"),
        };
        assert!(pc + length <= code.len(), "truncated opcode at PC {pc}");
        if let Some(mnemonic) = mnemonic {
            let displacement = if opcode == 0xc8 {
                i32::from_be_bytes(code[pc + 1..pc + 5].try_into().unwrap()) as i64
            } else {
                i64::from(i16::from_be_bytes([code[pc + 1], code[pc + 2]]))
            };
            let target = usize::try_from((pc as i64) + displacement)
                .expect("branch target is a nonnegative PC");
            targets.push((target, mnemonic));
        }
        pc += length;
    }
    (instruction_pcs, targets)
}

fn javap_contains_branch(output: &str, mnemonic: &str, target: usize) -> bool {
    let target = target.to_string();
    output.lines().any(|line| {
        let mut tokens = line.split_whitespace();
        let is_instruction = tokens
            .next()
            .and_then(|token| token.strip_suffix(':'))
            .and_then(|pc| pc.parse::<usize>().ok())
            .is_some();
        is_instruction && tokens.next() == Some(mnemonic) && tokens.next() == Some(target.as_str())
    })
}

#[test]
fn matches_javap_branch_columns_independent_of_whitespace() {
    let javap_output = "  2: if_icmpge     7\n\t22: goto\t4\n 30: if_icmpge 17\n";
    assert!(javap_contains_branch(javap_output, "if_icmpge", 7));
    assert!(javap_contains_branch(javap_output, "goto", 4));
    assert!(!javap_contains_branch(javap_output, "if_icmpge", 1));
    assert!(!javap_contains_branch(javap_output, "iconst_0", 7));
}

fn build_calculator(version: ClassFileVersion) -> Vec<u8> {
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
    calculator
        .add_static_method("nested", "(II)I", |code| {
            let not_less = code.new_label()?;
            let nonzero = code.new_label()?;
            let equal = code.new_label()?;
            let joined = code.new_label()?;
            code.iload(0)?;
            code.iload(1)?;
            code.if_icmpge(not_less.clone())?;
            code.iload(0)?;
            code.ifne(nonzero.clone())?;
            code.iconst(100)?;
            code.istore(2)?;
            code.goto(joined.clone())?;
            code.mark(nonzero)?;
            code.iconst(10)?;
            code.istore(2)?;
            code.goto(joined.clone())?;
            code.mark(not_less)?;
            code.iload(0)?;
            code.iload(1)?;
            code.if_icmpeq(equal.clone())?;
            code.iconst(0)?;
            code.istore(2)?;
            code.goto(joined.clone())?;
            code.mark(equal)?;
            code.iconst(1)?;
            code.istore(2)?;
            code.mark(joined)?;
            code.iload(2)?;
            code.ireturn()?;
            Ok(())
        })
        .expect("branched nested method construction succeeds");
    calculator
        .encode()
        .expect("Calculator is generated through the typed builder")
}

#[test]
fn public_builders_emit_executable_calculator_box_and_caller_classes() {
    let directory = TempDir::new();
    let version = ClassFileVersion {
        major: 65,
        minor: 0,
    };

    let calculator_bytes = build_calculator(version);
    assert_eq!(
        build_calculator(version),
        calculator_bytes,
        "repeated builds must produce identical class, code, and frame bytes"
    );
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
    assert_eq!(calculator.methods.len(), 4);
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
    assert_eq!(
        assert_stack_map_targets(&calculator, &calculator.methods[1], 1),
        [7]
    );

    let sum_to = code(&calculator.methods[2]);
    assert_eq!((sum_to.max_stack, sum_to.max_locals), (2, 3));
    assert_eq!(sum_to.code.len(), 22);
    assert_eq!(sum_to.attributes.len(), 1);
    assert!(
        matches!(sum_to.attributes.as_slice(), [Attribute::Other(attribute)]
        if calculator.constant_pool.utf8(attribute.name_index) == Ok("StackMapTable"))
    );

    let sum_to_frames = assert_stack_map_targets(&calculator, &calculator.methods[2], 2);
    assert_eq!(sum_to_frames.len(), 2);
    let nested = code(&calculator.methods[3]);
    assert_eq!((nested.max_stack, nested.max_locals), (2, 3));
    assert!(nested.exception_table.is_empty());
    let nested_frames = assert_stack_map_targets(&calculator, &calculator.methods[3], 4);
    assert_eq!(nested_frames.len(), 4);

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
        "public final class GeneratedClassesHarness {\n    public static void main(String[] args) {\n        int calculator = Calculator.add(20, 22);\n        if (calculator != 42) throw new AssertionError(\"Calculator.add: \" + calculator);\n        int maxForward = Calculator.max(20, 22);\n        int maxReverse = Calculator.max(22, 20);\n        int maxEqual = Calculator.max(22, 22);\n        if (maxForward != 22 || maxReverse != 22 || maxEqual != 22) throw new AssertionError(\"Calculator.max: \" + maxForward + \", \" + maxReverse + \", \" + maxEqual);\n        int sumTen = Calculator.sumTo(10);\n        int sumZero = Calculator.sumTo(0);\n        int sumOne = Calculator.sumTo(1);\n        if (sumTen != 55 || sumZero != 0 || sumOne != 1) throw new AssertionError(\"Calculator.sumTo: \" + sumTen + \", \" + sumZero + \", \" + sumOne);\n        int nestedZero = Calculator.nested(0, 1);\n        int nestedLess = Calculator.nested(1, 2);\n        int nestedEqual = Calculator.nested(2, 2);\n        int nestedGreater = Calculator.nested(3, 2);\n        if (nestedZero != 100 || nestedLess != 10 || nestedEqual != 1 || nestedGreater != 0) throw new AssertionError(\"Calculator.nested: \" + nestedZero + \", \" + nestedLess + \", \" + nestedEqual + \", \" + nestedGreater);\n        int boxed = new Box(42).get();\n        if (boxed != 42) throw new AssertionError(\"Box.get: \" + boxed);\n        int caller = Caller.answer();\n        if (caller != 42) throw new AssertionError(\"Caller.answer: \" + caller);\n        int generated = Caller.makeBox(42).get();\n        if (generated != 42) throw new AssertionError(\"Caller.makeBox: \" + generated);\n        System.out.println(\"Calculator.add=\" + calculator);\n        System.out.println(\"Calculator.max=\" + maxForward + \",\" + maxReverse + \",\" + maxEqual);\n        System.out.println(\"Calculator.sumTo=\" + sumTen + \",\" + sumZero + \",\" + sumOne);\n        System.out.println(\"Calculator.nested=\" + nestedZero + \",\" + nestedLess + \",\" + nestedEqual + \",\" + nestedGreater);\n        System.out.println(\"Box.get=\" + boxed);\n        System.out.println(\"Caller.answer=\" + caller);\n        System.out.println(\"Caller.makeBox=\" + generated);\n    }\n}\n",
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
                "public static int nested(int, int);",
                "StackMapTable: number_of_entries = 1",
                "StackMapTable: number_of_entries = 2",
                "StackMapTable: number_of_entries = 4",
                "stack=2, locals=2",
                "stack=2, locals=3",
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
        if class_name == "Calculator" {
            for method in [
                &calculator.methods[1],
                &calculator.methods[2],
                &calculator.methods[3],
            ] {
                for (target, mnemonic) in branch_targets(code(method).code).1 {
                    assert!(
                        javap_contains_branch(&javap_output, mnemonic, target),
                        "javap output is missing branch {mnemonic} to {target}:\n{javap_output}"
                    );
                }
            }
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
        "Calculator.add=42\nCalculator.max=22,22,22\nCalculator.sumTo=55,0,1\nCalculator.nested=100,10,1,0\nBox.get=42\nCaller.answer=42\nCaller.makeBox=42"
    );
}
