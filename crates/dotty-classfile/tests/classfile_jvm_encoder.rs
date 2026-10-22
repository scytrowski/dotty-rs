use dotty_classfile::bytecode::ClassHierarchy;
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

fn method_named<'a, 'b>(
    class_file: &'a ClassFile<'b>,
    name: &str,
) -> &'a dotty_classfile::classfile::MethodInfo<'b> {
    class_file
        .methods
        .iter()
        .find(|method| class_file.constant_pool.utf8(method.name_index) == Ok(name))
        .unwrap_or_else(|| panic!("missing method {name}"))
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

fn stack_map_references_class(
    class_file: &ClassFile<'_>,
    method: &dotty_classfile::classfile::MethodInfo<'_>,
    expected_class: &str,
) -> bool {
    let code = code(method);
    let [Attribute::Other(attribute)] = code.attributes.as_slice() else {
        return false;
    };
    if class_file.constant_pool.utf8(attribute.name_index) != Ok("StackMapTable") {
        return false;
    }
    let bytes = attribute.bytes;
    let mut cursor = 0;
    let frame_count = read_u2(bytes, &mut cursor);
    for _ in 0..frame_count {
        if bytes[cursor] != 255 {
            return false;
        }
        cursor += 1;
        cursor += 2; // offset_delta
        for _ in 0..read_u2(bytes, &mut cursor) {
            if verification_type_references_class(class_file, bytes, &mut cursor, expected_class) {
                return true;
            }
        }
        for _ in 0..read_u2(bytes, &mut cursor) {
            if verification_type_references_class(class_file, bytes, &mut cursor, expected_class) {
                return true;
            }
        }
    }
    false
}

fn verification_type_references_class(
    class_file: &ClassFile<'_>,
    bytes: &[u8],
    cursor: &mut usize,
    expected_class: &str,
) -> bool {
    let tag = bytes[*cursor];
    *cursor += 1;
    match tag {
        7 => {
            let index =
                dotty_classfile::classfile::ConstantPoolIndex(read_u2(bytes, cursor) as u16);
            class_file.constant_pool.class_name(index) == Ok(expected_class)
        }
        8 => {
            *cursor += 2;
            false
        }
        0..=6 => false,
        _ => panic!("invalid verification_type_info tag {tag}"),
    }
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
        .add_static_method("discard", "()V", |code| {
            code.iconst(1)?;
            code.pop()?;
            code.return_void()?;
            Ok(())
        })
        .expect("category-one discard method construction succeeds");
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
    box_class
        .add_instance_method("combine", "(ILBox;Z)LBox;", |code| {
            code.aload(2)?;
            code.areturn()?;
            Ok(())
        })
        .expect("typed reference-argument and result method construction succeeds");
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
    caller
        .add_static_method("branchBox", "(I)V", |code| {
            let target = code.new_label()?;
            code.new_object("Box")?;
            code.dup()?;
            code.iconst(42)?;
            code.invokespecial("Box", "<init>", "(I)V")?;
            code.astore(1)?;
            code.iload(0)?;
            code.ifeq(target.clone())?;
            code.return_void()?;
            code.mark(target)?;
            code.aload(1)?;
            code.pop()?;
            code.return_void()?;
            Ok(())
        })
        .expect("branched reference-local construction succeeds");
    caller
        .add_static_method("echoBox", "(LBox;)LBox;", |code| {
            code.aload(0)?;
            code.areturn()?;
            Ok(())
        })
        .expect("linear exact reference parameter construction succeeds");
    caller
        .add_static_method("chooseBox", "(ZLBox;LBox;)LBox;", |code| {
            let otherwise = code.new_label()?;
            code.iload(0)?;
            code.ifeq(otherwise.clone())?;
            code.aload(1)?;
            code.areturn()?;
            code.mark(otherwise)?;
            code.aload(2)?;
            code.areturn()?;
            Ok(())
        })
        .expect("branched exact reference parameters construction succeeds");
    caller
        .add_static_method("loopBox", "(ZLBox;)LBox;", |code| {
            let head = code.new_label()?;
            let done = code.new_label()?;
            code.iconst(0)?;
            code.istore(2)?;
            code.mark(head.clone())?;
            code.iload(0)?;
            code.ifeq(done.clone())?;
            code.iconst(0)?;
            code.istore(0)?;
            code.goto(head)?;
            code.mark(done)?;
            code.aload(1)?;
            code.areturn()?;
            Ok(())
        })
        .expect("loop exact reference parameter construction succeeds");
    caller
        .add_static_method("callMixed", "(ILBox;Z)LBox;", |code| {
            code.aload(1)?;
            code.iload(0)?;
            code.aload(1)?;
            code.iload(2)?;
            code.invokevirtual("Box", "combine", "(ILBox;Z)LBox;")?;
            code.areturn()?;
            Ok(())
        })
        .expect("linear exact virtual reference argument and result construction succeeds");
    caller
        .add_static_method("chainMixed", "(ILBox;Z)I", |code| {
            code.aload(1)?;
            code.iload(0)?;
            code.aload(1)?;
            code.iload(2)?;
            code.invokevirtual("Box", "combine", "(ILBox;Z)LBox;")?;
            code.invokevirtual("Box", "get", "()I")?;
            code.ireturn()?;
            Ok(())
        })
        .expect("chained exact virtual reference results construction succeeds");
    caller
        .add_static_method("loopMixed", "(ZLBox;)LBox;", |code| {
            let head = code.new_label()?;
            let done = code.new_label()?;
            code.aload(1)?;
            code.iconst(1)?;
            code.aload(1)?;
            code.iload(0)?;
            code.invokevirtual("Box", "combine", "(ILBox;Z)LBox;")?;
            code.mark(head.clone())?;
            code.iload(0)?;
            code.ifeq(done.clone())?;
            code.iconst(0)?;
            code.istore(0)?;
            code.goto(head)?;
            code.mark(done)?;
            code.areturn()?;
            Ok(())
        })
        .expect("virtual reference result survives loop frame construction");
    caller
        .add_code_method("<init>", "()V", MethodAccessFlags(ACC_PUBLIC), |code| {
            code.aload(0)?;
            code.invokespecial("java/lang/Object", "<init>", "()V")?;
            code.return_void()?;
            Ok(())
        })
        .unwrap();
    caller
        .add_instance_method("self", "()LCaller;", |code| {
            code.aload(0)?;
            code.areturn()?;
            Ok(())
        })
        .expect("instance receiver has the exact current class type");
    let caller_bytes = caller
        .encode()
        .expect("Caller is generated through the typed builder");
    let caller_path = directory.path().join("Caller.class");
    fs::write(&caller_path, &caller_bytes)
        .unwrap_or_else(|error| panic!("write {}: {error}", caller_path.display()));

    let calculator = decode_and_validate(&calculator_bytes);
    assert_eq!(calculator.version, version);
    assert_eq!(calculator.access_flags.0, ACC_PUBLIC | ACC_SUPER);
    assert_eq!(calculator.methods.len(), 5);
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
    let discard = &calculator.methods[4];
    assert_eq!(
        calculator.constant_pool.utf8(discard.name_index),
        Ok("discard")
    );
    let discard_code = code(discard);
    assert_eq!((discard_code.max_stack, discard_code.max_locals), (1, 0));
    assert_eq!(discard_code.code, [0x04, 0x57, 0xb1]);
    assert!(discard_code.attributes.is_empty());

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
    assert_eq!(box_file.methods.len(), 3);
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
    assert_eq!(caller.methods.len(), 11);
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

    let branch_factory = &caller.methods[2];
    assert_eq!(
        caller.constant_pool.utf8(branch_factory.name_index),
        Ok("branchBox")
    );
    let branch_factory_code = code(branch_factory);
    assert_eq!(
        (
            branch_factory_code.max_stack,
            branch_factory_code.max_locals
        ),
        (3, 2)
    );
    assert_eq!(
        assert_stack_map_targets(&caller, branch_factory, 1).len(),
        1
    );
    let [Attribute::Other(branch_stack_map)] = branch_factory_code.attributes.as_slice() else {
        panic!("expected StackMapTable on branched reference factory");
    };
    let frame = branch_stack_map.bytes;
    assert_eq!(&frame[..2], &[0, 1]);
    assert_eq!(frame[2], 255);
    assert_eq!(&frame[5..7], &[0, 2]); // int parameter and Box local
    assert_eq!(frame[7], 1);
    assert_eq!(frame[8], 7);
    let box_class = dotty_classfile::constant_pool::ConstantPoolIndex(u16::from_be_bytes([
        frame[9], frame[10],
    ]));
    assert_eq!(caller.constant_pool.class_name(box_class), Ok("Box"));
    assert_eq!(&frame[11..13], &[0, 0]); // empty operand stack

    if !require_jdk_or_skip() {
        return;
    }

    let harness_path = directory.path().join("GeneratedClassesHarness.java");
    fs::write(
        &harness_path,
        r#"public final class GeneratedClassesHarness {
    public static void main(String[] args) {
        int calculator = Calculator.add(20, 22);
        if (calculator != 42) throw new AssertionError("Calculator.add: " + calculator);
        Calculator.discard();
        int maxForward = Calculator.max(20, 22);
        int maxReverse = Calculator.max(22, 20);
        int maxEqual = Calculator.max(22, 22);
        if (maxForward != 22 || maxReverse != 22 || maxEqual != 22) throw new AssertionError("Calculator.max: " + maxForward + ", " + maxReverse + ", " + maxEqual);
        int sumTen = Calculator.sumTo(10);
        int sumZero = Calculator.sumTo(0);
        int sumOne = Calculator.sumTo(1);
        if (sumTen != 55 || sumZero != 0 || sumOne != 1) throw new AssertionError("Calculator.sumTo: " + sumTen + ", " + sumZero + ", " + sumOne);
        int nestedZero = Calculator.nested(0, 1);
        int nestedLess = Calculator.nested(1, 2);
        int nestedEqual = Calculator.nested(2, 2);
        int nestedGreater = Calculator.nested(3, 2);
        if (nestedZero != 100 || nestedLess != 10 || nestedEqual != 1 || nestedGreater != 0) throw new AssertionError("Calculator.nested: " + nestedZero + ", " + nestedLess + ", " + nestedEqual + ", " + nestedGreater);
        int boxed = new Box(42).get();
        if (boxed != 42) throw new AssertionError("Box.get: " + boxed);
        int caller = Caller.answer();
        if (caller != 42) throw new AssertionError("Caller.answer: " + caller);
        int generated = Caller.makeBox(42).get();
        if (generated != 42) throw new AssertionError("Caller.makeBox: " + generated);
        Box first = new Box(10);
        Box second = new Box(20);
        if (Caller.callMixed(7, first, true) != first) throw new AssertionError("Caller.callMixed");
        if (Caller.chainMixed(7, first, true) != 10) throw new AssertionError("Caller.chainMixed");
        if (Caller.loopMixed(false, second) != second || Caller.loopMixed(true, first) != first) throw new AssertionError("Caller.loopMixed");
        if (Caller.echoBox(first) != first) throw new AssertionError("Caller.echoBox");
        if (Caller.chooseBox(true, first, second) != first || Caller.chooseBox(false, first, second) != second) throw new AssertionError("Caller.chooseBox");
        if (Caller.loopBox(false, second) != second || Caller.loopBox(true, first) != first) throw new AssertionError("Caller.loopBox");
        Caller instance = new Caller();
        if (instance.self() != instance) throw new AssertionError("Caller.self");
        Caller.branchBox(0);
        Caller.branchBox(1);
        System.out.println("Calculator.add=" + calculator);
        System.out.println("Calculator.max=" + maxForward + "," + maxReverse + "," + maxEqual);
        System.out.println("Calculator.sumTo=" + sumTen + "," + sumZero + "," + sumOne);
        System.out.println("Calculator.nested=" + nestedZero + "," + nestedLess + "," + nestedEqual + "," + nestedGreater);
        System.out.println("Box.get=" + boxed);
        System.out.println("Caller.answer=" + caller);
        System.out.println("Caller.makeBox=" + generated);
        System.out.println("Caller.branchBox=ok");
    }
}
"#
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
                "public static void discard();",
                "StackMapTable: number_of_entries = 1",
                "StackMapTable: number_of_entries = 2",
                "StackMapTable: number_of_entries = 4",
                "stack=2, locals=2",
                "stack=2, locals=3",
                "iload_0",
                "iload_1",
                "iadd",
                "pop",
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
                "public Box combine(int, Box, boolean);",
            ][..],
        ),
        (
            "Caller",
            &[
                "public static int answer();",
                "public static Box makeBox(int);",
                "public static void branchBox(int);",
                "public static Box callMixed(int, Box, boolean);",
                "public static int chainMixed(int, Box, boolean);",
                "public static Box loopMixed(boolean, Box);",
                "invokevirtual",
                "invokestatic",
                "stack=2, locals=0",
                "bipush        20",
                "bipush        22",
                "new           #",
                "dup",
                "invokespecial",
                "stack=3, locals=1",
                "StackMapTable: number_of_entries = 1",
                "class Box",
                "locals = [ int, class Box ]",
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
        "Calculator.add=42\nCalculator.max=22,22,22\nCalculator.sumTo=55,0,1\nCalculator.nested=100,10,1,0\nBox.get=42\nCaller.answer=42\nCaller.makeBox=42\nCaller.branchBox=ok"
    );
}

#[test]
fn reference_fields_and_constructor_arguments_verify_with_jdk() {
    if !require_jdk_or_skip() {
        return;
    }
    let directory = TempDir::new();
    let version = ClassFileVersion {
        major: 65,
        minor: 0,
    };

    let mut counter = ClassFileBuilder::new("example/Counter", version).unwrap();
    counter
        .add_instance_method("<init>", "()V", |code| {
            code.aload(0)?;
            code.invokespecial("java/lang/Object", "<init>", "()V")?;
            code.return_void()?;
            Ok(())
        })
        .unwrap();
    let counter_bytes = counter.encode().unwrap();
    fs::create_dir_all(directory.path().join("example")).unwrap();
    fs::write(
        directory.path().join("example/Counter.class"),
        counter_bytes,
    )
    .unwrap();

    let mut holder = ClassFileBuilder::new("example/Holder", version).unwrap();
    holder
        .add_field("ref", "Lexample/Counter;", FieldAccessFlags(ACC_PRIVATE))
        .unwrap();
    holder
        .add_instance_method("<init>", "(Lexample/Counter;)V", |code| {
            code.aload(0)?;
            code.invokespecial("java/lang/Object", "<init>", "()V")?;
            code.aload(0)?;
            code.aload(1)?;
            code.putfield("example/Holder", "ref", "Lexample/Counter;")?;
            code.return_void()?;
            Ok(())
        })
        .unwrap();
    holder
        .add_instance_method("get", "()Lexample/Counter;", |code| {
            code.aload(0)?;
            code.getfield("example/Holder", "ref", "Lexample/Counter;")?;
            code.areturn()?;
            Ok(())
        })
        .unwrap();
    holder
        .add_instance_method("setIf", "(Lexample/Counter;Z)V", |code| {
            let alternate = code.new_label()?;
            let done = code.new_label()?;
            code.iload(2)?;
            code.ifeq(alternate.clone())?;
            code.aload(0)?;
            code.aload(1)?;
            code.putfield("example/Holder", "ref", "Lexample/Counter;")?;
            code.goto(done.clone())?;
            code.mark(alternate)?;
            code.aload(0)?;
            code.aload(1)?;
            code.putfield("example/Holder", "ref", "Lexample/Counter;")?;
            code.mark(done)?;
            code.return_void()?;
            Ok(())
        })
        .unwrap();
    let holder_bytes = holder.encode().unwrap();
    let holder_file = decode_and_validate(&holder_bytes);
    assert!(stack_map_references_class(
        &holder_file,
        method_named(&holder_file, "setIf"),
        "example/Counter"
    ));
    fs::write(directory.path().join("example/Holder.class"), holder_bytes).unwrap();

    let mut client = ClassFileBuilder::new("example/ReferenceFieldClient", version).unwrap();
    client
        .set_access_flags(ClassAccessFlags(ACC_PUBLIC | ACC_SUPER))
        .unwrap();
    client
        .add_static_method("create", "(Lexample/Counter;)Lexample/Holder;", |code| {
            code.new_object("example/Holder")?;
            code.dup()?;
            code.aload(0)?;
            code.invokespecial("example/Holder", "<init>", "(Lexample/Counter;)V")?;
            code.areturn()?;
            Ok(())
        })
        .unwrap();
    client
        .add_static_method("run", "()V", |code| {
            code.new_object("example/Counter")?;
            code.dup()?;
            code.invokespecial("example/Counter", "<init>", "()V")?;
            code.astore(0)?;
            code.new_object("example/Holder")?;
            code.dup()?;
            code.aload(0)?;
            code.invokespecial("example/Holder", "<init>", "(Lexample/Counter;)V")?;
            code.astore(1)?;
            code.aload(1)?;
            code.aload(0)?;
            code.iconst(1)?;
            code.invokevirtual("example/Holder", "setIf", "(Lexample/Counter;Z)V")?;
            code.aload(1)?;
            code.invokevirtual("example/Holder", "get", "()Lexample/Counter;")?;
            code.pop()?;
            code.return_void()?;
            Ok(())
        })
        .unwrap();
    let client_bytes = client.encode().unwrap();
    fs::write(
        directory.path().join("example/ReferenceFieldClient.class"),
        client_bytes,
    )
    .unwrap();

    let harness = directory.path().join("ReferenceFieldHarness.java");
    fs::write(
        &harness,
        "public class ReferenceFieldHarness { public static void main(String[] args) { example.ReferenceFieldClient.run(); System.out.println(\"reference-field=ok\"); } }\n",
    )
    .unwrap();
    let directory_arg = directory.path().to_str().unwrap();
    let javac = run(
        "javac",
        &[
            "-classpath",
            directory_arg,
            "-d",
            directory_arg,
            harness.to_str().unwrap(),
        ],
    );
    assert!(
        javac.status.success(),
        "javac failed for reference-field harness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = run(
        "java",
        &[
            "-Xverify:all",
            "-classpath",
            directory_arg,
            "ReferenceFieldHarness",
        ],
    );
    assert!(
        java.status.success(),
        "java -Xverify:all rejected reference fields or constructor arguments\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "reference-field=ok"
    );
}

#[test]
fn direct_super_invokespecial_verifies_and_executes_with_jdk() {
    if !require_jdk_or_skip() {
        return;
    }
    let directory = TempDir::new();
    let version = ClassFileVersion {
        major: 65,
        minor: 0,
    };
    let package_dir = directory.path().join("example");
    fs::create_dir_all(&package_dir).unwrap();

    let mut base = ClassFileBuilder::new("example/Base", version).unwrap();
    base.add_instance_method("<init>", "()V", |code| {
        code.aload(0)?;
        code.invokespecial("java/lang/Object", "<init>", "()V")?;
        code.return_void()?;
        Ok(())
    })
    .unwrap()
    .add_instance_method("value", "()I", |code| {
        code.iconst(41)?;
        code.ireturn()?;
        Ok(())
    })
    .unwrap();
    fs::write(package_dir.join("Base.class"), base.encode().unwrap()).unwrap();

    let mut derived = ClassFileBuilder::new("example/Derived", version).unwrap();
    derived
        .set_super_class(Some("example/Base"))
        .unwrap()
        .add_instance_method("<init>", "()V", |code| {
            code.aload(0)?;
            code.invokespecial("example/Base", "<init>", "()V")?;
            code.return_void()?;
            Ok(())
        })
        .unwrap()
        .add_instance_method("superValue", "()I", |code| {
            code.aload(0)?;
            code.invokespecial("example/Base", "value", "()I")?;
            code.ireturn()?;
            Ok(())
        })
        .unwrap()
        .add_instance_method("superValueOnBranch", "(Z)I", |code| {
            let otherwise = code.new_label()?;
            code.iload(1)?;
            code.ifeq(otherwise.clone())?;
            code.aload(0)?;
            code.invokespecial("example/Base", "value", "()I")?;
            code.ireturn()?;
            code.mark(otherwise)?;
            code.iconst(0)?;
            code.ireturn()?;
            Ok(())
        })
        .unwrap();
    let derived_bytes = derived.encode().unwrap();
    let decoded_derived = decode_and_validate(&derived_bytes);
    let branch_code = code(method_named(&decoded_derived, "superValueOnBranch"));
    assert!(
        branch_code
            .attributes
            .iter()
            .any(|attribute| matches!(attribute, Attribute::Other(attribute)
                if decoded_derived.constant_pool.utf8(attribute.name_index) == Ok("StackMapTable")))
    );
    assert!(
        !code(method_named(&decoded_derived, "superValue"))
            .attributes
            .iter()
            .any(|attribute| matches!(attribute, Attribute::Other(attribute)
                if decoded_derived.constant_pool.utf8(attribute.name_index) == Ok("StackMapTable")))
    );
    fs::write(package_dir.join("Derived.class"), derived_bytes).unwrap();

    let harness_path = package_dir.join("DirectSuperHarness.java");
    fs::write(
        &harness_path,
        "package example; public class DirectSuperHarness { public static void main(String[] args) { Derived value = new Derived(); if (value.superValue() != 41 || value.superValueOnBranch(true) != 41 || value.superValueOnBranch(false) != 0) throw new AssertionError(); System.out.print(\"ok\"); } }",
    )
    .unwrap();
    let directory_arg = directory.path().to_string_lossy().into_owned();
    let harness_arg = harness_path.to_string_lossy().into_owned();
    let javac = run(
        "javac",
        &[
            "-classpath",
            &directory_arg,
            "-d",
            &directory_arg,
            &harness_arg,
        ],
    );
    assert!(
        javac.status.success(),
        "javac failed for direct-super harness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = run(
        "java",
        &[
            "-Xverify:all",
            "-classpath",
            &directory_arg,
            "example.DirectSuperHarness",
        ],
    );
    assert!(
        java.status.success(),
        "java -Xverify:all rejected direct-super invokespecial\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&java.stdout).trim(), "ok");
}

#[test]
fn parameterized_direct_super_constructor_prologue_verifies_with_jdk() {
    if !require_jdk_or_skip() {
        return;
    }
    let directory = TempDir::new();
    let version = ClassFileVersion {
        major: 65,
        minor: 0,
    };
    let hierarchy =
        ClassHierarchy::from_edges([("example/Derived".to_owned(), "example/Base".to_owned())])
            .unwrap();

    let mut dep = ClassFileBuilder::new("example/Dep", version).unwrap();
    dep.set_access_flags(ClassAccessFlags(ACC_PUBLIC | ACC_SUPER))
        .unwrap();
    dep.add_instance_method("<init>", "()V", |code| {
        code.aload(0)?;
        code.invokespecial("java/lang/Object", "<init>", "()V")?;
        code.return_void()?;
        Ok(())
    })
    .unwrap();
    dep.add_instance_method("value", "()I", |code| {
        code.iconst(7)?;
        code.ireturn()?;
        Ok(())
    })
    .unwrap();

    let mut base = ClassFileBuilder::new("example/Base", version).unwrap();
    base.set_access_flags(ClassAccessFlags(ACC_PUBLIC | ACC_SUPER))
        .unwrap();
    base.add_field("seed", "I", FieldAccessFlags(ACC_PRIVATE))
        .unwrap();
    base.add_field("enabled", "Z", FieldAccessFlags(ACC_PRIVATE))
        .unwrap();
    base.add_field("dep", "Lexample/Dep;", FieldAccessFlags(ACC_PRIVATE))
        .unwrap();
    base.add_instance_method("<init>", "(IZLexample/Dep;)V", |code| {
        code.aload(0)?;
        code.invokespecial("java/lang/Object", "<init>", "()V")?;
        code.aload(0)?;
        code.iload(1)?;
        code.putfield("example/Base", "seed", "I")?;
        code.aload(0)?;
        code.iload(2)?;
        code.putfield("example/Base", "enabled", "Z")?;
        code.aload(0)?;
        code.aload(3)?;
        code.putfield("example/Base", "dep", "Lexample/Dep;")?;
        code.return_void()?;
        Ok(())
    })
    .unwrap();
    base.add_instance_method("<init>", "(I)V", |code| {
        code.aload(0)?;
        code.invokespecial("java/lang/Object", "<init>", "()V")?;
        code.aload(0)?;
        code.iload(1)?;
        code.putfield("example/Base", "seed", "I")?;
        code.return_void()?;
        Ok(())
    })
    .unwrap();
    base.add_instance_method("seed", "()I", |code| {
        code.aload(0)?;
        code.getfield("example/Base", "seed", "I")?;
        code.ireturn()?;
        Ok(())
    })
    .unwrap();
    base.add_instance_method("enabled", "()Z", |code| {
        code.aload(0)?;
        code.getfield("example/Base", "enabled", "Z")?;
        code.ireturn()?;
        Ok(())
    })
    .unwrap();
    base.add_instance_method("dep", "()Lexample/Dep;", |code| {
        code.aload(0)?;
        code.getfield("example/Base", "dep", "Lexample/Dep;")?;
        code.areturn()?;
        Ok(())
    })
    .unwrap();

    let mut derived = ClassFileBuilder::new("example/Derived", version).unwrap();
    derived
        .set_access_flags(ClassAccessFlags(ACC_PUBLIC | ACC_SUPER))
        .unwrap();
    derived.set_super_class(Some("example/Base")).unwrap();
    derived.set_verification_hierarchy(hierarchy);
    derived
        .add_instance_method("<init>", "(IZLexample/Dep;)V", |code| {
            code.aload(0)?;
            code.iload(1)?;
            code.iload(2)?;
            code.aload(3)?;
            code.invokespecial("example/Base", "<init>", "(IZLexample/Dep;)V")?;
            code.return_void()?;
            Ok(())
        })
        .unwrap();
    derived
        .add_instance_method("<init>", "()V", |code| {
            code.aload(0)?;
            code.iconst(123_456)?;
            code.invokespecial("example/Base", "<init>", "(I)V")?;
            code.return_void()?;
            Ok(())
        })
        .unwrap();

    fs::create_dir_all(directory.path().join("example")).unwrap();
    for (name, bytes) in [
        ("Dep", dep.encode().unwrap()),
        ("Base", base.encode().unwrap()),
        ("Derived", derived.encode().unwrap()),
    ] {
        fs::write(
            directory.path().join(format!("example/{name}.class")),
            bytes,
        )
        .unwrap();
    }

    let harness = directory.path().join("ParameterizedSuperHarness.java");
    fs::write(
        &harness,
        "public class ParameterizedSuperHarness { public static void main(String[] args) { example.Dep dep = new example.Dep(); example.Derived value = new example.Derived(42, true, dep); if (value.seed() != 42 || !value.enabled() || value.dep() != dep) throw new AssertionError(); if (new example.Derived().seed() != 123456) throw new AssertionError(); System.out.println(\"parameterized-super=ok\"); } }\n",
    )
    .unwrap();
    let directory_arg = directory.path().to_str().unwrap();
    let javac = run(
        "javac",
        &[
            "-classpath",
            directory_arg,
            "-d",
            directory_arg,
            harness.to_str().unwrap(),
        ],
    );
    assert!(
        javac.status.success(),
        "javac failed for parameterized-super harness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = run(
        "java",
        &[
            "-Xverify:all",
            "-classpath",
            directory_arg,
            "ParameterizedSuperHarness",
        ],
    );
    assert!(
        java.status.success(),
        "java -Xverify:all rejected parameterized direct-super initialization\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "parameterized-super=ok"
    );
}

#[test]
fn ancestor_reference_stack_join_frames_verify_with_jdk() {
    if !require_jdk_or_skip() {
        return;
    }
    let directory = TempDir::new();
    let version = ClassFileVersion {
        major: 65,
        minor: 0,
    };
    fs::create_dir_all(directory.path().join("example")).unwrap();

    for (class_name, parent) in [
        ("Base", "java/lang/Object"),
        ("Middle", "example/Base"),
        ("Leaf", "example/Middle"),
    ] {
        let internal_name = format!("example/{class_name}");
        let mut class = ClassFileBuilder::new(&internal_name, version).unwrap();
        class
            .set_access_flags(ClassAccessFlags(ACC_PUBLIC | ACC_SUPER))
            .unwrap();
        class.set_super_class(Some(parent)).unwrap();
        class
            .add_instance_method("<init>", "()V", move |code| {
                code.aload(0)?;
                code.invokespecial(parent, "<init>", "()V")?;
                code.return_void()?;
                Ok(())
            })
            .unwrap();
        fs::write(
            directory.path().join(format!("example/{class_name}.class")),
            class.encode().unwrap(),
        )
        .unwrap();
    }

    let hierarchy = ClassHierarchy::from_edges([
        ("example/Leaf".to_owned(), "example/Middle".to_owned()),
        ("example/Middle".to_owned(), "example/Base".to_owned()),
    ])
    .unwrap();
    let mut joins = ClassFileBuilder::new("example/AncestorJoin", version).unwrap();
    joins
        .set_access_flags(ClassAccessFlags(ACC_PUBLIC | ACC_SUPER))
        .unwrap();
    joins.set_super_class(Some("java/lang/Object")).unwrap();
    joins.set_verification_hierarchy(hierarchy);
    for (name, middle_on_fallthrough) in [("choose", true), ("chooseReversed", false)] {
        joins
            .add_static_method(
                name,
                "(ZLexample/Middle;Lexample/Leaf;)Lexample/Base;",
                move |code| {
                    let otherwise = code.new_label()?;
                    let join = code.new_label()?;
                    code.iload(0)?;
                    code.ifeq(otherwise.clone())?;
                    code.aload(if middle_on_fallthrough { 1 } else { 2 })?;
                    code.goto(join.clone())?;
                    code.mark(otherwise)?;
                    code.aload(if middle_on_fallthrough { 2 } else { 1 })?;
                    code.mark(join)?;
                    code.areturn()?;
                    Ok(())
                },
            )
            .unwrap();
    }
    fs::write(
        directory.path().join("example/AncestorJoin.class"),
        joins.encode().unwrap(),
    )
    .unwrap();

    let harness = directory.path().join("AncestorJoinHarness.java");
    fs::write(
        &harness,
        "import example.*; public class AncestorJoinHarness { public static void main(String[] args) { Middle middle = new Middle(); Leaf leaf = new Leaf(); if (AncestorJoin.choose(true, middle, leaf) != middle || AncestorJoin.choose(false, middle, leaf) != leaf) throw new AssertionError(\"choose identity\"); if (AncestorJoin.chooseReversed(true, middle, leaf) != leaf || AncestorJoin.chooseReversed(false, middle, leaf) != middle) throw new AssertionError(\"reversed identity\"); System.out.println(\"ancestor-stack-join=ok\"); } }\n",
    )
    .unwrap();
    let directory_arg = directory.path().to_str().unwrap();
    let javac = run(
        "javac",
        &[
            "-classpath",
            directory_arg,
            "-d",
            directory_arg,
            harness.to_str().unwrap(),
        ],
    );
    assert!(
        javac.status.success(),
        "javac failed for ancestor-join harness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = run(
        "java",
        &[
            "-Xverify:all",
            "-classpath",
            directory_arg,
            "AncestorJoinHarness",
        ],
    );
    assert!(
        java.status.success(),
        "java -Xverify:all rejected ancestor stack-join frames\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "ancestor-stack-join=ok"
    );
}

#[test]
fn sibling_reference_stack_join_frames_verify_with_jdk() {
    if !require_jdk_or_skip() {
        return;
    }
    let directory = TempDir::new();
    let version = ClassFileVersion {
        major: 65,
        minor: 0,
    };
    fs::create_dir_all(directory.path().join("example")).unwrap();
    for (name, parent) in [
        ("Base", "java/lang/Object"),
        ("Left", "example/Base"),
        ("Right", "example/Base"),
    ] {
        let internal_name = format!("example/{name}");
        let mut class = ClassFileBuilder::new(&internal_name, version).unwrap();
        class
            .set_access_flags(ClassAccessFlags(ACC_PUBLIC | ACC_SUPER))
            .unwrap();
        class.set_super_class(Some(parent)).unwrap();
        class
            .add_instance_method("<init>", "()V", move |code| {
                code.aload(0)?;
                code.invokespecial(parent, "<init>", "()V")?;
                code.return_void()?;
                Ok(())
            })
            .unwrap();
        fs::write(
            directory.path().join(format!("example/{name}.class")),
            class.encode().unwrap(),
        )
        .unwrap();
    }
    let hierarchy = ClassHierarchy::from_edges([
        ("example/Left".to_owned(), "example/Base".to_owned()),
        ("example/Right".to_owned(), "example/Base".to_owned()),
        ("example/Base".to_owned(), "java/lang/Object".to_owned()),
    ])
    .unwrap();
    let mut joins = ClassFileBuilder::new("example/SiblingJoin", version).unwrap();
    joins
        .set_access_flags(ClassAccessFlags(ACC_PUBLIC | ACC_SUPER))
        .unwrap();
    joins.set_super_class(Some("java/lang/Object")).unwrap();
    joins.set_verification_hierarchy(hierarchy);
    for (name, left_first) in [("choose", true), ("chooseReversed", false)] {
        joins
            .add_static_method(
                name,
                "(ZLexample/Left;Lexample/Right;)Lexample/Base;",
                move |code| {
                    let otherwise = code.new_label()?;
                    let join = code.new_label()?;
                    code.iload(0)?;
                    code.ifeq(otherwise.clone())?;
                    code.aload(if left_first { 1 } else { 2 })?;
                    code.goto(join.clone())?;
                    code.mark(otherwise)?;
                    code.aload(if left_first { 2 } else { 1 })?;
                    code.mark(join)?;
                    code.areturn()?;
                    Ok(())
                },
            )
            .unwrap();
        joins
            .add_static_method(
                if left_first {
                    "chooseLocal"
                } else {
                    "chooseLocalReversed"
                },
                "(ZLexample/Left;Lexample/Right;)Lexample/Base;",
                move |code| {
                    let otherwise = code.new_label()?;
                    let join = code.new_label()?;
                    code.iload(0)?;
                    code.ifeq(otherwise.clone())?;
                    code.aload(if left_first { 1 } else { 2 })?;
                    code.astore(3)?;
                    code.goto(join.clone())?;
                    code.mark(otherwise)?;
                    code.aload(if left_first { 2 } else { 1 })?;
                    code.astore(3)?;
                    code.mark(join)?;
                    code.aload(3)?;
                    code.areturn()?;
                    Ok(())
                },
            )
            .unwrap();
    }
    fs::write(
        directory.path().join("example/SiblingJoin.class"),
        joins.encode().unwrap(),
    )
    .unwrap();
    let harness = directory.path().join("SiblingJoinHarness.java");
    fs::write(&harness, "import example.*; public class SiblingJoinHarness { public static void main(String[] args) { Left left = new Left(); Right right = new Right(); if (SiblingJoin.choose(true, left, right) != left || SiblingJoin.choose(false, left, right) != right || SiblingJoin.chooseReversed(true, left, right) != right || SiblingJoin.chooseReversed(false, left, right) != left || SiblingJoin.chooseLocal(true, left, right) != left || SiblingJoin.chooseLocal(false, left, right) != right || SiblingJoin.chooseLocalReversed(true, left, right) != right || SiblingJoin.chooseLocalReversed(false, left, right) != left) throw new AssertionError(); System.out.println(\"sibling-reference-joins=ok\"); } }\n").unwrap();
    let dir = directory.path().to_str().unwrap();
    let javac = run(
        "javac",
        &["-classpath", dir, "-d", dir, harness.to_str().unwrap()],
    );
    assert!(
        javac.status.success(),
        "javac failed: {}",
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = run(
        "java",
        &["-Xverify:all", "-classpath", dir, "SiblingJoinHarness"],
    );
    assert!(
        java.status.success(),
        "java -Xverify:all failed: {}",
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "sibling-reference-joins=ok"
    );
    let javap = run(
        "javap",
        &["-classpath", dir, "-c", "-v", "example.SiblingJoin"],
    );
    assert!(javap.status.success());
    assert!(String::from_utf8_lossy(&javap.stdout).contains("example/Base"));
}

#[test]
fn integer_arithmetic_instructions_execute_with_jvm_signed_semantics() {
    let directory = TempDir::new();
    let version = ClassFileVersion {
        major: 65,
        minor: 0,
    };
    let mut arithmetic = ClassFileBuilder::new("IntegerArithmetic", version)
        .expect("Java 21 class builder is supported");
    arithmetic
        .set_access_flags(ClassAccessFlags(ACC_PUBLIC | ACC_SUPER))
        .unwrap();
    for (name, operation, opcode) in [
        ("subtract", 0u8, 0x64u8),
        ("multiply", 1, 0x68),
        ("divide", 2, 0x6c),
        ("remainder", 3, 0x70),
    ] {
        arithmetic
            .add_static_method(name, "(II)I", move |code| {
                code.iload(0)?;
                code.iload(1)?;
                match operation {
                    0 => code.isub()?,
                    1 => code.imul()?,
                    2 => code.idiv()?,
                    3 => code.irem()?,
                    _ => unreachable!(),
                };
                code.ireturn()?;
                Ok(())
            })
            .unwrap();
        assert_eq!(opcode, [0x64, 0x68, 0x6c, 0x70][operation as usize]);
    }
    arithmetic
        .add_static_method("divideByZero", "()I", |code| {
            code.iconst(1)?;
            code.iconst(0)?;
            code.idiv()?;
            code.ireturn()?;
            Ok(())
        })
        .unwrap();
    arithmetic
        .add_static_method("remainderByZero", "()I", |code| {
            code.iconst(1)?;
            code.iconst(0)?;
            code.irem()?;
            code.ireturn()?;
            Ok(())
        })
        .unwrap();
    let bytes = arithmetic
        .encode()
        .expect("integer arithmetic class is generated through typed code builder");
    let class = decode_and_validate(&bytes);
    assert_eq!(class.methods.len(), 6);
    for (index, opcode) in [0x64, 0x68, 0x6c, 0x70].into_iter().enumerate() {
        let method_code = code(&class.methods[index]);
        assert_eq!((method_code.max_stack, method_code.max_locals), (2, 2));
        assert_eq!(method_code.code, [0x1a, 0x1b, opcode, 0xac]);
    }
    assert_eq!(code(&class.methods[4]).code, [0x04, 0x03, 0x6c, 0xac]);
    assert_eq!(code(&class.methods[5]).code, [0x04, 0x03, 0x70, 0xac]);

    if !require_jdk_or_skip() {
        return;
    }
    let class_path = directory.path().join("IntegerArithmetic.class");
    fs::write(&class_path, bytes)
        .unwrap_or_else(|error| panic!("write {}: {error}", class_path.display()));
    let harness_path = directory.path().join("IntegerArithmeticHarness.java");
    fs::write(
        &harness_path,
        "public final class IntegerArithmeticHarness {\n    public static void main(String[] args) {\n        if (IntegerArithmetic.subtract(2, 5) != -3 || IntegerArithmetic.subtract(-2, -5) != 3) throw new AssertionError(\"isub\");\n        if (IntegerArithmetic.multiply(-7, 3) != -21) throw new AssertionError(\"imul\");\n        if (IntegerArithmetic.divide(-7, 3) != -2 || IntegerArithmetic.divide(7, -3) != -2) throw new AssertionError(\"idiv signed truncation\");\n        if (IntegerArithmetic.divide(Integer.MIN_VALUE, -1) != Integer.MIN_VALUE) throw new AssertionError(\"idiv overflow case\");\n        if (IntegerArithmetic.remainder(-7, 3) != -1 || IntegerArithmetic.remainder(7, -3) != 1) throw new AssertionError(\"irem signed remainder\");\n        if (IntegerArithmetic.remainder(Integer.MIN_VALUE, -1) != 0) throw new AssertionError(\"irem overflow case\");\n        try { IntegerArithmetic.divideByZero(); throw new AssertionError(\"idiv by zero returned\"); } catch (ArithmeticException expected) { }\n        try { IntegerArithmetic.remainderByZero(); throw new AssertionError(\"irem by zero returned\"); } catch (ArithmeticException expected) { }\n        System.out.println(\"integer arithmetic semantics ok\");\n    }\n}\n",
    )
    .unwrap_or_else(|error| panic!("write {}: {error}", harness_path.display()));
    let directory_arg = directory.path().to_string_lossy().into_owned();
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
        "javac failed for arithmetic harness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = run(
        "java",
        &[
            "-Xverify:all",
            "-classpath",
            &directory_arg,
            "IntegerArithmeticHarness",
        ],
    );
    assert!(
        java.status.success(),
        "java -Xverify:all rejected or failed to execute arithmetic class\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "integer arithmetic semantics ok"
    );
}

#[test]
fn int_bitwise_instructions_execute_and_verify_with_jvm_semantics() {
    let directory = TempDir::new();
    let version = ClassFileVersion {
        major: 65,
        minor: 0,
    };
    let mut class =
        ClassFileBuilder::new("IntBitwise", version).expect("Java 21 class builder is supported");
    class
        .set_access_flags(ClassAccessFlags(ACC_PUBLIC | ACC_SUPER))
        .unwrap();
    for (name, operation, opcode) in [("and", 0u8, 0x7e), ("or", 1, 0x80), ("xor", 2, 0x82)] {
        class
            .add_static_method(name, "(II)I", move |code| {
                code.iload(0)?;
                code.iload(1)?;
                match operation {
                    0 => code.iand()?,
                    1 => code.ior()?,
                    2 => code.ixor()?,
                    _ => unreachable!(),
                };
                code.ireturn()?;
                Ok(())
            })
            .unwrap();
        assert_eq!(opcode, [0x7e, 0x80, 0x82][operation as usize]);
    }
    class
        .add_static_method("branched", "()I", |code| {
            let otherwise = code.new_label()?;
            let join = code.new_label()?;
            code.iconst(1)?;
            code.iconst(1)?;
            code.ixor()?;
            code.ifeq(otherwise.clone())?;
            code.iconst(10)?;
            code.goto(join.clone())?;
            code.mark(otherwise)?;
            code.iconst(20)?;
            code.mark(join)?;
            code.ireturn()?;
            Ok(())
        })
        .unwrap();

    let bytes = class.encode().expect("typed bitwise bytecode encodes");
    let decoded = decode_and_validate(&bytes);
    assert_eq!(decoded.methods.len(), 4);
    for (index, opcode) in [0x7e, 0x80, 0x82].into_iter().enumerate() {
        assert_eq!(
            code(&decoded.methods[index]).code,
            [0x1a, 0x1b, opcode, 0xac]
        );
        assert_eq!(
            (
                code(&decoded.methods[index]).max_stack,
                code(&decoded.methods[index]).max_locals
            ),
            (2, 2)
        );
    }
    let branched = code(&decoded.methods[3]);
    assert_eq!(
        branched.code,
        [
            0x04, 0x04, 0x82, 0x99, 0x00, 0x08, 0x10, 10, 0xa7, 0x00, 0x05, 0x10, 20, 0xac,
        ]
    );
    assert_eq!(branched.max_stack, 2);
    assert_eq!(branched.max_locals, 0);
    assert_eq!(
        branched
            .attributes
            .iter()
            .filter_map(|attribute| match attribute {
                Attribute::Other(attribute)
                    if decoded.constant_pool.utf8(attribute.name_index) == Ok("StackMapTable") =>
                {
                    Some(attribute)
                }
                _ => None,
            })
            .count(),
        1
    );

    if !require_jdk_or_skip() {
        return;
    }
    let class_path = directory.path().join("IntBitwise.class");
    fs::write(&class_path, bytes)
        .unwrap_or_else(|error| panic!("write {}: {error}", class_path.display()));
    let harness_path = directory.path().join("IntBitwiseHarness.java");
    fs::write(
        &harness_path,
        "public final class IntBitwiseHarness {\n    public static void main(String[] args) {\n        if (IntBitwise.and(1, 0) != 0 || IntBitwise.or(1, 0) != 1 || IntBitwise.xor(1, 1) != 0) throw new AssertionError(\"boolean encodings\");\n        if (IntBitwise.and(0x5a5a, 0x0ff0) != 0x0a50 || IntBitwise.or(0x5a5a, 0x0ff0) != 0x5ffa || IntBitwise.xor(0x5a5a, 0x0ff0) != 0x55aa) throw new AssertionError(\"integer masks\");\n        if (IntBitwise.branched() != 20) throw new AssertionError(\"branched StackMapTable\");\n        System.out.println(\"int bitwise semantics ok\");\n    }\n}\n",
    )
    .unwrap_or_else(|error| panic!("write {}: {error}", harness_path.display()));
    let directory_arg = directory.path().to_string_lossy().into_owned();
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
        "javac failed for bitwise harness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = run(
        "java",
        &[
            "-Xverify:all",
            "-classpath",
            &directory_arg,
            "IntBitwiseHarness",
        ],
    );
    assert!(
        java.status.success(),
        "java -Xverify:all rejected or failed to execute bitwise class\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "int bitwise semantics ok"
    );
}
