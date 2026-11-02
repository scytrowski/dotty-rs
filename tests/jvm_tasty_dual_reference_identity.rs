use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_batch;
use dotty_classfile::attribute::Attribute;
use dotty_classfile::bytecode::{
    CfgCodeAnalyzer, ClassReference, CodeSequence, ConstantPoolIndexOperand, ControlFlowGraph,
    FieldReference, Instruction, LocalIndex, SpecialMethodReference, VirtualMethodReference,
};
use dotty_classfile::classfile::{ClassFile, Reader};
use dotty_classfile::constant_pool::{ConstantPoolEntry, ConstantPoolIndex};
use dotty_core::SourceId;
use std::process::Command;

const SOURCE: &str = include_str!("fixtures/jvm_tasty_dual_reference_constructors/Pair.scala");
const FACTORY_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_dual_reference_forwarding/PairFactory.scala");

#[test]
fn dual_reference_factory_preserves_forwarded_argument_identity_on_strict_jdk21() {
    if !require_jdk_21() {
        return;
    }

    let batch = emit_paired_jvm_tasty_batch(
        FACTORY_SOURCE,
        "tests/fixtures/jvm_tasty_dual_reference_forwarding/PairFactory.scala",
        SourceId::from_index(19671),
    )
    .expect("one original Rust frontend session emits the complete factory batch");
    assert_eq!(
        batch
            .classes()
            .iter()
            .map(|artifact| artifact.classfile_path())
            .collect::<Vec<_>>(),
        [
            "example/Counter.class",
            "example/Pair.class",
            "example/PairFactory.class"
        ]
    );

    let scratch = TestDirectory::new("dual-reference-factory-identity");
    let original = scratch.path().join("original-rust-artifacts");
    write_classes(&original, &batch, None);
    let files = batch
        .classes()
        .iter()
        .flat_map(|artifact| [artifact.classfile_path(), artifact.tasty_path()])
        .map(|relative| original.join(relative))
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 6);
    let hashes = files.iter().map(|path| sha256sum(path)).collect::<Vec<_>>();
    eprintln!("dual-reference-factory-original-sha256-before {hashes:?}");

    let factory_bytes = batch.classes()[2].classfile_bytes();
    let factory = decode(factory_bytes);
    assert_eq!((factory.version.major, factory.version.minor), (65, 0));
    let wrap = method(
        &factory,
        "wrap",
        "(Lexample/Counter;Lexample/Counter;)Lexample/Pair;",
    );
    let wrap_code = code(wrap);
    assert_eq!((wrap_code.max_stack, wrap_code.max_locals), (4, 3));
    let pair_class = find_class(&factory, factory_bytes, "example/Pair");
    let constructor = find_methodref(
        &factory,
        factory_bytes,
        "example/Pair",
        "<init>",
        "(Lexample/Counter;Lexample/Counter;)V",
    );
    let expected_wrap = [
        0xbb,
        (pair_class.0 >> 8) as u8,
        pair_class.0 as u8,
        0x59,
        0x2b,
        0x2c,
        0xb7,
        (constructor.0 >> 8) as u8,
        constructor.0 as u8,
        0xb0,
    ];
    assert_eq!(wrap_code.code, expected_wrap);
    assert_eq!(
        wrap_code
            .code
            .iter()
            .filter(|opcode| **opcode == 0xbb)
            .count(),
        1,
        "wrap allocates only Pair"
    );
    let cp_operand =
        |index: ConstantPoolIndex| ConstantPoolIndexOperand::try_from(index.0).unwrap();
    let mut sequence = CodeSequence::new();
    sequence
        .instruction(Instruction::New(ClassReference::new(cp_operand(
            pair_class,
        ))))
        .instruction(Instruction::Dup)
        .instruction(Instruction::ALoad(LocalIndex::new(1)))
        .instruction(Instruction::ALoad(LocalIndex::new(2)))
        .instruction(Instruction::InvokeSpecial(SpecialMethodReference::new(
            cp_operand(constructor),
        )))
        .instruction(Instruction::AReturn);
    let graph = ControlFlowGraph::build(&sequence).expect("wrap bytecode decodes");
    let analysis = CfgCodeAnalyzer::analyze_with_current_class(
        &graph,
        "wrap",
        "(Lexample/Counter;Lexample/Counter;)Lexample/Pair;",
        false,
        &factory.constant_pool,
        factory.this_class,
    )
    .expect("Rust bytecode analysis accepts ordered initialized Pair return");
    assert_eq!((analysis.max_stack, analysis.max_locals), (4, 3));
    let dump = javap(&original, "example.PairFactory");
    for expected in [
        "major version: 65",
        "descriptor: (Lexample/Counter;Lexample/Counter;)Lexample/Pair;",
        "stack=4, locals=3",
        "aload_1",
        "aload_2",
        "Method example/Pair.\"<init>\":(Lexample/Counter;Lexample/Counter;)V",
        "areturn",
    ] {
        assert!(
            dump.contains(expected),
            "javap omitted {expected:?}:\n{dump}"
        );
    }
    let wrap_dump = dump
        .split("public example.Pair wrap(example.Counter, example.Counter);")
        .nth(1)
        .expect("javap prints the forwarding method")
        .split("  public ")
        .next()
        .unwrap();
    let pcs = wrap_dump
        .lines()
        .filter_map(|line| {
            let instruction = line.trim();
            let (pc, opcode) = instruction.split_once(": ")?;
            Some((
                pc.parse::<usize>().ok()?,
                opcode.split_whitespace().next()?.to_owned(),
            ))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        pcs,
        [
            (0, "new".to_owned()),
            (3, "dup".to_owned()),
            (4, "aload_1".to_owned()),
            (5, "aload_2".to_owned()),
            (6, "invokespecial".to_owned()),
            (9, "areturn".to_owned()),
        ],
        "assert program counters reported by javap for the original classfile"
    );

    let harness = scratch.path().join("PairFactoryForwardingHarness.java");
    let harness_classes = scratch.path().join("harness-classes");
    std::fs::create_dir(&harness_classes).unwrap();
    std::fs::write(&harness, FACTORY_HARNESS).unwrap();
    let javac = Command::new("javac")
        .args(["--release", "21", "-classpath"])
        .arg(&original)
        .arg("-d")
        .arg(&harness_classes)
        .arg(&harness)
        .output()
        .expect("start JDK 21 javac for the independent Java harness");
    assert!(
        javac.status.success(),
        "javac --release 21 rejected harness:\n{}",
        String::from_utf8_lossy(&javac.stderr)
    );
    assert_hashes(&files, &hashes, "factory javac");
    let classpath = std::env::join_paths([harness_classes.as_path(), original.as_path()]).unwrap();
    let java = run_java(&classpath, "PairFactoryForwardingHarness");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected original Rust artifacts:\n{}\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "forward=7/3,42/3,42/-9;reverse=-9/42;repeat=true;alias=100/100"
    );
    assert_hashes(&files, &hashes, "factory runtime");

    for (path, expected_hash) in files.iter().zip(&hashes) {
        assert_eq!(sha256sum(path), *expected_hash);
    }

    let run_factory_negative = |label: &str,
                                changed_factory: Option<&[u8]>,
                                omitted_class: Option<&str>,
                                expected: &str| {
        let root = scratch.path().join(label);
        for (index, artifact) in batch.classes().iter().enumerate() {
            if omitted_class == Some(artifact.classfile_path()) {
                continue;
            }
            let destination = root.join(artifact.classfile_path());
            std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
            let class_bytes = if index == 2 {
                changed_factory.unwrap_or(artifact.classfile_bytes())
            } else {
                artifact.classfile_bytes()
            };
            std::fs::write(destination, class_bytes).unwrap();
            let tasty_destination = root.join(artifact.tasty_path());
            std::fs::create_dir_all(tasty_destination.parent().unwrap()).unwrap();
            std::fs::write(tasty_destination, artifact.tasty_bytes()).unwrap();
        }
        let negative_cp =
            std::env::join_paths([harness_classes.as_path(), root.as_path()]).unwrap();
        let rejected = run_java(&negative_cp, "PairFactoryForwardingHarness");
        assert!(!rejected.status.success(), "JVM/harness accepted {label}");
        let output = format!(
            "{}{}",
            String::from_utf8_lossy(&rejected.stdout),
            String::from_utf8_lossy(&rejected.stderr)
        );
        assert!(
            output.contains(expected),
            "{label} failed at an unexpected first layer; expected {expected:?}:\n{output}"
        );
        let layer = if expected.starts_with("AssertionError") {
            "runtime identity assertion"
        } else if expected.contains("NoClassDefFoundError") {
            "JVM linkage"
        } else {
            "JVM verifier/classfile"
        };
        let evidence = output
            .lines()
            .find(|line| line.contains(expected.split(':').next().unwrap()))
            .unwrap_or("expected rejection observed");
        eprintln!(
            "dual-reference-factory-negative {label} first-layer={layer} evidence={evidence}"
        );
    };

    let wrap_start = code_offset(factory_bytes, wrap_code.code);
    let mut swapped_loads = factory_bytes.to_vec();
    swapped_loads.swap(wrap_start + 4, wrap_start + 5);
    run_factory_negative(
        "factory-swapped-reference-loads",
        Some(&swapped_loads),
        None,
        "AssertionError: left must preserve the original argument object",
    );
    let mut repeated_first_load = factory_bytes.to_vec();
    repeated_first_load[wrap_start + 5] = 0x2b;
    run_factory_negative(
        "factory-load-first-formal-twice",
        Some(&repeated_first_load),
        None,
        "AssertionError: right must preserve the original argument object",
    );

    let mut wrong_owner = factory.clone();
    let wrong_constructor_owner = find_class(&factory, factory_bytes, "java/lang/Object");
    replace_methodref_class(
        &mut wrong_owner,
        factory_bytes,
        constructor,
        wrong_constructor_owner,
    );
    run_factory_negative(
        "factory-wrong-pair-constructor-owner",
        Some(&wrong_owner.encode().unwrap()),
        None,
        "VerifyError",
    );
    let no_args_void = find_utf8(&factory, factory_bytes, "()V");
    let mut wrong_descriptor = factory.clone();
    replace_methodref_descriptor(
        &mut wrong_descriptor,
        factory_bytes,
        constructor,
        no_args_void,
    );
    run_factory_negative(
        "factory-wrong-pair-constructor-descriptor",
        Some(&wrong_descriptor.encode().unwrap()),
        None,
        "VerifyError",
    );

    let mut missing_allocation = factory_bytes.to_vec();
    missing_allocation[wrap_start] = 0x00;
    run_factory_negative(
        "factory-missing-pair-new",
        Some(&missing_allocation),
        None,
        "VerifyError",
    );
    let mut duplicate_allocation = factory_bytes.to_vec();
    duplicate_allocation.splice(
        wrap_start + 9..wrap_start + 9,
        [0xbb, (pair_class.0 >> 8) as u8, pair_class.0 as u8],
    );
    let old_code_length = u32_at(&duplicate_allocation, wrap_start - 4);
    set_u32(
        &mut duplicate_allocation,
        wrap_start - 4,
        old_code_length + 3,
    );
    let old_attribute_length = u32_at(&duplicate_allocation, wrap_start - 12);
    set_u32(
        &mut duplicate_allocation,
        wrap_start - 12,
        old_attribute_length + 3,
    );
    run_factory_negative(
        "factory-duplicate-pair-new",
        Some(&duplicate_allocation),
        None,
        "VerifyError",
    );
    let mut missing_constructor_call = factory_bytes.to_vec();
    missing_constructor_call[wrap_start + 6] = 0x00;
    run_factory_negative(
        "factory-missing-pair-invokespecial",
        Some(&missing_constructor_call),
        None,
        "VerifyError",
    );
    let mut duplicate_constructor_call = factory_bytes.to_vec();
    duplicate_constructor_call.splice(
        wrap_start + 9..wrap_start + 9,
        [0xb7, (constructor.0 >> 8) as u8, constructor.0 as u8],
    );
    let old_code_length = u32_at(&duplicate_constructor_call, wrap_start - 4);
    set_u32(
        &mut duplicate_constructor_call,
        wrap_start - 4,
        old_code_length + 3,
    );
    let old_attribute_length = u32_at(&duplicate_constructor_call, wrap_start - 12);
    set_u32(
        &mut duplicate_constructor_call,
        wrap_start - 12,
        old_attribute_length + 3,
    );
    run_factory_negative(
        "factory-duplicate-pair-invokespecial",
        Some(&duplicate_constructor_call),
        None,
        "VerifyError",
    );
    let mut illegal_uninitialized_flow = factory_bytes.to_vec();
    illegal_uninitialized_flow[wrap_start + 3] = 0x00;
    run_factory_negative(
        "factory-missing-dup-before-pair-init",
        Some(&illegal_uninitialized_flow),
        None,
        "VerifyError",
    );
    let mut insufficient_stack = factory_bytes.to_vec();
    set_u16(&mut insufficient_stack, wrap_start - 8, 3);
    run_factory_negative(
        "factory-insufficient-max-stack",
        Some(&insufficient_stack),
        None,
        "VerifyError",
    );
    let mut insufficient_locals = factory_bytes.to_vec();
    set_u16(&mut insufficient_locals, wrap_start - 6, 2);
    run_factory_negative(
        "factory-insufficient-max-locals",
        Some(&insufficient_locals),
        None,
        "ClassFormatError",
    );
    run_factory_negative(
        "factory-missing-counter-class",
        None,
        Some("example/Counter.class"),
        "NoClassDefFoundError: example/Counter",
    );
    run_factory_negative(
        "factory-missing-pair-class",
        None,
        Some("example/Pair.class"),
        "NoClassDefFoundError: example/Pair",
    );
}

const FACTORY_HARNESS: &str = r#"
import java.lang.reflect.Field;

public final class PairFactoryForwardingHarness {
    public static void main(String[] args) throws Exception {
        example.Counter a = new example.Counter(7);
        example.Counter b = new example.Counter(3);
        example.PairFactory firstFactory = new example.PairFactory();
        example.PairFactory secondFactory = new example.PairFactory();
        example.Pair pair = firstFactory.wrap(a, b);
        identity(pair, "left", a); identity(pair, "right", b);
        expect(pair.leftValue(), 7); expect(pair.rightValue(), 3);
        a.set(42); expect(pair.leftValue(), 42); expect(pair.rightValue(), 3);
        b.set(-9); expect(pair.leftValue(), 42); expect(pair.rightValue(), -9);

        example.Pair reverse = secondFactory.wrap(b, a);
        identity(reverse, "left", b); identity(reverse, "right", a);
        expect(reverse.leftValue(), -9); expect(reverse.rightValue(), 42);
        example.Pair repeated = new example.PairFactory().wrap(a, b);
        identity(repeated, "left", a); identity(repeated, "right", b);
        if (repeated == pair || firstFactory == secondFactory)
            throw new AssertionError("separate calls and factory instances remain independent");

        example.Pair alias = secondFactory.wrap(a, a);
        identity(alias, "left", a); identity(alias, "right", a);
        a.set(100); expect(alias.leftValue(), 100); expect(alias.rightValue(), 100);
        expect(pair.leftValue(), 100); expect(pair.rightValue(), -9);
        expect(reverse.leftValue(), -9); expect(reverse.rightValue(), 100);
        System.out.println("forward=7/3,42/3,42/-9;reverse=-9/42;repeat=true;alias=100/100");
    }
    private static void identity(example.Pair pair, String fieldName, example.Counter expected)
        throws Exception {
        Field field = example.Pair.class.getDeclaredField(fieldName);
        field.setAccessible(true);
        if (field.get(pair) != expected)
            throw new AssertionError(fieldName + " must preserve the original argument object");
    }
    private static void expect(int actual, int expected) {
        if (actual != expected) throw new AssertionError("expected " + expected + ", got " + actual);
    }
}
"#;

#[test]
fn dual_reference_pair_preserves_original_argument_identity_on_strict_jdk21() {
    if !require_jdk_21() {
        return;
    }

    let batch = emit_paired_jvm_tasty_batch(
        SOURCE,
        "tests/fixtures/jvm_tasty_dual_reference_constructors/Pair.scala",
        SourceId::from_index(18801),
    )
    .expect("the original one-session Rust frontend emits the complete Pair batch");
    assert_eq!(batch.classes().len(), 2);
    let names = batch
        .classes()
        .iter()
        .map(|pair| pair.classfile_path())
        .collect::<Vec<_>>();
    assert_eq!(names, ["example/Counter.class", "example/Pair.class"]);

    let scratch = TestDirectory::new("dual-reference-identity");
    let original = scratch.path().join("original");
    write_classes(&original, &batch, None);
    let files = batch
        .classes()
        .iter()
        .flat_map(|pair| {
            [
                (pair.classfile_path(), pair.classfile_bytes()),
                (pair.tasty_path(), pair.tasty_bytes()),
            ]
        })
        .map(|(relative, _)| original.join(relative))
        .collect::<Vec<_>>();
    let hashes = files.iter().map(|path| sha256sum(path)).collect::<Vec<_>>();
    eprintln!("dual-reference-original-hashes-before {hashes:?}");

    let pair_bytes = batch.classes()[1].classfile_bytes();
    let pair = decode(pair_bytes);
    assert_eq!((pair.version.major, pair.version.minor), (65, 0));
    assert_eq!(
        pair.constant_pool.class_name(pair.this_class).unwrap(),
        "example/Pair"
    );
    assert_eq!(pair.fields.len(), 2);
    let fields = pair
        .fields
        .iter()
        .map(|field| {
            (
                pair.constant_pool.utf8(field.name_index).unwrap(),
                pair.constant_pool.utf8(field.descriptor_index).unwrap(),
                field.access_flags,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(fields.len(), 2);
    for (name, descriptor, flags) in fields {
        assert!(matches!(name, "left" | "right"));
        assert_eq!(descriptor, "Lexample/Counter;");
        assert!(flags.contains(dotty_classfile::access_flags::ACC_PRIVATE));
        assert!(flags.contains(dotty_classfile::access_flags::ACC_FINAL));
    }
    let ctor = method(&pair, "<init>", "(Lexample/Counter;Lexample/Counter;)V");
    let ctor_code = code(ctor);
    assert_eq!((ctor_code.max_stack, ctor_code.max_locals), (2, 3));
    let stores = opcode_offsets(ctor_code.code, 0xb5);
    assert_eq!(stores.len(), 2);
    let loads = ctor_code
        .code
        .iter()
        .enumerate()
        .filter(|(_, opcode)| matches!(opcode, 0x2a..=0x2d))
        .map(|(pc, opcode)| (pc, opcode - 0x2a))
        .collect::<Vec<_>>();
    assert!(
        loads.contains(&(0, 0)),
        "receiver is loaded from local 0: {loads:?}"
    );
    assert!(
        loads.contains(&(stores[0] - 1, 1)),
        "first constructor parameter is loaded from local 1: {loads:?}"
    );
    assert!(
        loads.contains(&(stores[1] - 1, 2)),
        "second constructor parameter is loaded from local 2: {loads:?}"
    );
    let constructor_dump = javap(&original, "example.Pair");
    assert!(
        constructor_dump.contains("major version: 65"),
        "{constructor_dump}"
    );
    let ctor_dump = constructor_dump
        .split("public example.Pair(example.Counter, example.Counter);")
        .nth(1)
        .expect("javap prints the primary constructor")
        .split("public int leftValue();")
        .next()
        .unwrap();
    for expected in [
        "descriptor: (Lexample/Counter;Lexample/Counter;)V",
        "stack=2, locals=3",
        "aload_0",
        "Method java/lang/Object.\"<init>\":()V",
        "aload_1",
        "Field left:Lexample/Counter;",
        "aload_2",
        "Field right:Lexample/Counter;",
    ] {
        assert!(
            ctor_dump.contains(expected),
            "javap omitted {expected:?}:\n{ctor_dump}"
        );
    }
    assert!(
        !ctor_dump.contains("new "),
        "constructor allocates a replacement Counter:\n{ctor_dump}"
    );
    assert!(
        !ctor_dump.contains("Counter.\"<init>\""),
        "constructor invokes a Counter constructor:\n{ctor_dump}"
    );
    let left_read = method(&pair, "leftValue", "()I");
    let right_read = method(&pair, "rightValue", "()I");
    for (label, reader, field) in [
        ("leftValue", left_read, "left"),
        ("rightValue", right_read, "right"),
    ] {
        let code = code(reader);
        let dump = if label == "leftValue" {
            constructor_dump
                .split("public int leftValue();")
                .nth(1)
                .unwrap()
                .split("public int rightValue();")
                .next()
                .unwrap()
        } else {
            constructor_dump
                .split("public int rightValue();")
                .nth(1)
                .unwrap()
        };
        assert!(
            dump.contains(&format!("Field {field}:Lexample/Counter;")),
            "{dump}"
        );
        assert!(dump.contains("Method example/Counter.get:()I"), "{dump}");
        assert_eq!(code.max_locals, 1, "{label} uses receiver slot 0");
        let fieldref =
            ConstantPoolIndex(u16_at(pair_bytes, code_offset(pair_bytes, code.code) + 2));
        let get_cp = ConstantPoolIndex(u16_at(pair_bytes, code_offset(pair_bytes, code.code) + 5));
        let ConstantPoolEntry::Fieldref {
            class_index,
            name_and_type_index,
        } = pair.constant_pool.get(fieldref).unwrap()
        else {
            panic!("getter reads a Fieldref")
        };
        assert_eq!(
            pair.constant_pool.class_name(*class_index).unwrap(),
            "example/Pair"
        );
        assert_eq!(
            pair.constant_pool
                .name_and_type(*name_and_type_index)
                .unwrap(),
            (field, "Lexample/Counter;")
        );
        let ConstantPoolEntry::Methodref {
            class_index,
            name_and_type_index,
        } = pair.constant_pool.get(get_cp).unwrap()
        else {
            panic!("getter invokes a Methodref")
        };
        assert_eq!(
            pair.constant_pool.class_name(*class_index).unwrap(),
            "example/Counter"
        );
        assert_eq!(
            pair.constant_pool
                .name_and_type(*name_and_type_index)
                .unwrap(),
            ("get", "()I")
        );
        assert_eq!(
            code.code,
            [
                0x2a,
                0xb4,
                (fieldref.0 >> 8) as u8,
                fieldref.0 as u8,
                0xb6,
                (get_cp.0 >> 8) as u8,
                get_cp.0 as u8,
                0xac,
            ]
        );
        let field_operand = ConstantPoolIndexOperand::try_from(fieldref.0).unwrap();
        let get_operand = ConstantPoolIndexOperand::try_from(u16_at(
            pair_bytes,
            code_offset(pair_bytes, code.code) + 5,
        ))
        .unwrap();
        let mut sequence = CodeSequence::new();
        sequence
            .instruction(Instruction::ALoad(LocalIndex::new(0)))
            .instruction(Instruction::GetField(FieldReference::new(field_operand)))
            .instruction(Instruction::InvokeVirtual(VirtualMethodReference::new(
                get_operand,
            )))
            .instruction(Instruction::IReturn);
        let graph =
            ControlFlowGraph::build(&sequence).expect("straight-line getter bytecode decodes");
        let analyzed = CfgCodeAnalyzer::analyze_with_current_class(
            &graph,
            label,
            "()I",
            false,
            &pair.constant_pool,
            pair.this_class,
        )
        .expect("Rust verifier accepts the getter");
        assert_eq!((analyzed.max_stack, analyzed.max_locals), (1, 1));
    }
    let constructor_bytes = ctor_code.code;
    let constructor_code_offset = code_offset(pair_bytes, constructor_bytes);
    let object_ref = ConstantPoolIndex(u16_at(pair_bytes, constructor_code_offset + 2));
    let ConstantPoolEntry::Methodref {
        class_index,
        name_and_type_index,
    } = pair.constant_pool.get(object_ref).unwrap()
    else {
        panic!("super initialization invokes a Methodref")
    };
    assert_eq!(
        pair.constant_pool.class_name(*class_index).unwrap(),
        "java/lang/Object"
    );
    assert_eq!(
        pair.constant_pool
            .name_and_type(*name_and_type_index)
            .unwrap(),
        ("<init>", "()V")
    );
    let first_field =
        ConstantPoolIndex(u16_at(pair_bytes, constructor_code_offset + stores[0] + 1));
    let second_field =
        ConstantPoolIndex(u16_at(pair_bytes, constructor_code_offset + stores[1] + 1));
    for (reference, expected) in [(first_field, "left"), (second_field, "right")] {
        let ConstantPoolEntry::Fieldref {
            class_index,
            name_and_type_index,
        } = pair.constant_pool.get(reference).unwrap()
        else {
            panic!("putfield uses a Fieldref")
        };
        assert_eq!(
            pair.constant_pool.class_name(*class_index).unwrap(),
            "example/Pair"
        );
        assert_eq!(
            pair.constant_pool
                .name_and_type(*name_and_type_index)
                .unwrap(),
            (expected, "Lexample/Counter;")
        );
    }
    assert_eq!(
        constructor_bytes,
        [
            0x2a,
            0xb7,
            (object_ref.0 >> 8) as u8,
            object_ref.0 as u8,
            0x2a,
            0x2b,
            0xb5,
            (first_field.0 >> 8) as u8,
            first_field.0 as u8,
            0x2a,
            0x2c,
            0xb5,
            (second_field.0 >> 8) as u8,
            second_field.0 as u8,
            0xb1,
        ]
    );
    assert_eq!(
        pair.constant_pool.class_name(pair.this_class).unwrap(),
        "example/Pair",
        "Rust classfile decoding resolves the constructor's current class"
    );
    eprintln!(
        "dual-reference-rust-classfile Pair.<init> max_stack=2 max_locals=3 store_pcs={stores:?} super_methodref=#{} left_fieldref=#{} right_fieldref=#{}; direct constructor flow is verified by strict JDK runtime",
        object_ref.0, first_field.0, second_field.0
    );

    let harness = scratch.path().join("DualReferenceHarness.java");
    let harness_classes = scratch.path().join("harness-classes");
    std::fs::create_dir(&harness_classes).unwrap();
    std::fs::write(&harness, HARNESS).unwrap();
    let javac = Command::new("javac")
        .args(["--release", "21", "-classpath"])
        .arg(&original)
        .arg("-d")
        .arg(&harness_classes)
        .arg(&harness)
        .output()
        .expect("start JDK 21 javac for the independent Java harness");
    assert!(
        javac.status.success(),
        "javac rejected harness:\n{}",
        String::from_utf8_lossy(&javac.stderr)
    );
    assert_hashes(&files, &hashes, "javac");
    let classpath = std::env::join_paths([harness_classes.as_path(), original.as_path()]).unwrap();
    let java = Command::new("java")
        .args(["-Xverify:all", "-ea", "-classpath"])
        .arg(&classpath)
        .arg("DualReferenceHarness")
        .output()
        .expect("start strict JDK 21 verification and identity harness");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected original Rust artifacts:\n{}\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "pair=7,3;mutations=42,3;42,-9;reverse=3,7;alias=100,100"
    );
    assert_hashes(&files, &hashes, "runtime");
    for ((path, pair), expected) in files
        .iter()
        .zip(
            batch
                .classes()
                .iter()
                .flat_map(|pair| [pair.classfile_bytes(), pair.tasty_bytes()]),
        )
        .zip(&hashes)
    {
        assert_eq!(std::fs::read(path).unwrap().as_slice(), pair);
        assert_eq!(sha256sum(path), *expected);
    }

    // The same verifier-accepted, type-correct field swap is caught by the
    // stronger reflection identity contract in the harness.
    let ctor_start = code_offset(pair_bytes, ctor_code.code);
    let first_store = stores[0];
    let second_store = stores[1];
    let field_operands = [first_store + 1, second_store + 1];
    let left_ref = u16_at(pair_bytes, ctor_start + field_operands[0]);
    let right_ref = u16_at(pair_bytes, ctor_start + field_operands[1]);
    let mut swapped = pair_bytes.to_vec();
    set_u16(&mut swapped, ctor_start + field_operands[0], right_ref);
    set_u16(&mut swapped, ctor_start + field_operands[1], left_ref);
    run_negative(
        &scratch,
        &batch,
        &harness_classes,
        "type-correct-field-swap",
        &swapped,
        "AssertionError: left field must preserve first argument identity",
    );

    let second_load_pc = loads.iter().find(|(_, slot)| *slot == 2).unwrap().0;
    let mut duplicate_slot = pair_bytes.to_vec();
    duplicate_slot[ctor_start + second_load_pc] = 0x2b;
    run_negative(
        &scratch,
        &batch,
        &harness_classes,
        "slot-1-loaded-twice",
        &duplicate_slot,
        "AssertionError: right field must preserve second argument identity",
    );

    let mut wrong_field = pair_bytes.to_vec();
    set_u16(&mut wrong_field, ctor_start + field_operands[1], left_ref);
    run_negative(
        &scratch,
        &batch,
        &harness_classes,
        "wrong-second-fieldref",
        &wrong_field,
        "AssertionError: left field must preserve first argument identity",
    );

    let int_descriptor = find_utf8(&pair, pair_bytes, "()I");
    let mut wrong_field_type = pair.clone();
    replace_fieldref_descriptor(
        &mut wrong_field_type,
        pair_bytes,
        ConstantPoolIndex(right_ref),
        int_descriptor,
    );
    run_jvm_rejection(
        &scratch,
        &batch,
        &harness_classes,
        "wrong-second-field-descriptor",
        &wrong_field_type.encode().unwrap(),
        "ClassFormatError: Field \"right\" in class example/Pair has illegal signature \"()I\"",
    );

    let mut bad_stack = pair_bytes.to_vec();
    let stack_offset = ctor_start - 8;
    set_u16(&mut bad_stack, stack_offset, 1);
    run_jvm_rejection(
        &scratch,
        &batch,
        &harness_classes,
        "insufficient-max-stack",
        &bad_stack,
        "VerifyError: Operand stack overflow",
    );
    let mut bad_locals = pair_bytes.to_vec();
    set_u16(&mut bad_locals, stack_offset + 2, 2);
    run_jvm_rejection(
        &scratch,
        &batch,
        &harness_classes,
        "insufficient-max-locals",
        &bad_locals,
        "ClassFormatError: Arguments can't fit into locals in class file example/Pair",
    );

    let super_ref = find_methodref(&pair, pair_bytes, "java/lang/Object", "<init>", "()V");
    let counter_class = find_class(&pair, pair_bytes, "example/Counter");
    let mut wrong_owner_class = pair.clone();
    replace_methodref_class(&mut wrong_owner_class, pair_bytes, super_ref, counter_class);
    run_jvm_rejection(
        &scratch,
        &batch,
        &harness_classes,
        "wrong-constructor-owner",
        &wrong_owner_class.encode().unwrap(),
        "VerifyError",
    );
    let int_result = find_utf8(&pair, pair_bytes, "()I");
    let mut wrong_descriptor_class = pair.clone();
    replace_methodref_descriptor(
        &mut wrong_descriptor_class,
        pair_bytes,
        super_ref,
        int_result,
    );
    run_jvm_rejection(
        &scratch,
        &batch,
        &harness_classes,
        "wrong-constructor-descriptor",
        &wrong_descriptor_class.encode().unwrap(),
        "ClassFormatError: Method \"<init>\" in class example/Pair has illegal signature \"()I\"",
    );
    let mut before_super = pair_bytes.to_vec();
    let first_load = loads
        .iter()
        .find(|(pc, slot)| *pc == 0 && *slot == 0)
        .unwrap()
        .0;
    before_super[ctor_start + first_load] = 0x2b;
    run_jvm_rejection(
        &scratch,
        &batch,
        &harness_classes,
        "wrong-receiver-before-super",
        &before_super,
        "VerifyError",
    );

    let read = method(&pair, "leftValue", "()I");
    let read_code = code(read);
    let read_offset = code_offset(pair_bytes, read_code.code);
    let invoke = opcode_offsets(read_code.code, 0xb6)[0];
    let get_ref = u16_at(pair_bytes, read_offset + invoke + 1);
    let set_name = find_utf8(&pair, pair_bytes, "leftValue");
    let mut bad_get_class = pair.clone();
    replace_methodref_name(
        &mut bad_get_class,
        pair_bytes,
        ConstantPoolIndex(get_ref),
        set_name,
    );
    run_linkage_rejection(
        &scratch,
        &batch,
        &harness_classes,
        "wrong-counter-get-methodref",
        &bad_get_class.encode().unwrap(),
        "NoSuchMethodError",
    );

    let missing_counter = scratch.path().join("missing-counter");
    write_classes(&missing_counter, &batch, Some("example/Counter.class"));
    let missing_cp =
        std::env::join_paths([harness_classes.as_path(), missing_counter.as_path()]).unwrap();
    let missing = run_java(&missing_cp, "DualReferenceHarness");
    assert!(!missing.status.success());
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&missing.stdout),
        String::from_utf8_lossy(&missing.stderr)
    );
    assert!(
        output.contains("NoClassDefFoundError: example/Counter"),
        "missing Counter rejected at JVM linkage/class loading:\n{output}"
    );
    eprintln!(
        "dual-reference-negative missing-counter first-stage=JVM-linkage NoClassDefFoundError"
    );
}

const HARNESS: &str = r#"
import java.lang.reflect.Field;

public final class DualReferenceHarness {
    public static void main(String[] args) throws Exception {
        example.Counter a = new example.Counter(7);
        example.Counter b = new example.Counter(3);
        example.Pair pair = new example.Pair(a, b);
        identity(pair, "left", a); identity(pair, "right", b); expect(pair.leftValue(), 7); expect(pair.rightValue(), 3);
        example.Pair reverse = new example.Pair(b, a);
        identity(reverse, "left", b); identity(reverse, "right", a); expect(reverse.leftValue(), 3); expect(reverse.rightValue(), 7);
        a.set(42); expect(pair.leftValue(), 42); expect(pair.rightValue(), 3);
        b.set(-9); expect(pair.leftValue(), 42); expect(pair.rightValue(), -9);
        example.Pair alias = new example.Pair(a, a);
        identity(alias, "left", a); identity(alias, "right", a); a.set(100); expect(alias.leftValue(), 100); expect(alias.rightValue(), 100);
        System.out.println("pair=7,3;mutations=42,3;42,-9;reverse=3,7;alias=100,100");
    }
    private static void identity(example.Pair pair, String name, example.Counter expected) throws Exception {
        Field field = example.Pair.class.getDeclaredField(name); field.setAccessible(true);
        if (field.get(pair) != expected) throw new AssertionError(name + " field must preserve " + (name.equals("left") ? "first" : "second") + " argument identity");
    }
    private static void expect(int actual, int expected) { if (actual != expected) throw new AssertionError("expected " + expected + ", got " + actual); }
}
"#;

fn require_jdk_21() -> bool {
    let unavailable = ["java", "javac", "javap"]
        .into_iter()
        .filter_map(|tool| {
            let output = match Command::new(tool).arg("-version").output() {
                Ok(output) => output,
                Err(error) => return Some(format!("{tool} unavailable: {error}")),
            };
            if !output.status.success() {
                return Some(format!("{tool} -version failed"));
            }
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            (!text.contains("21.0.13")).then(|| format!("{tool} reports {text:?}"))
        })
        .collect::<Vec<_>>();
    if unavailable.is_empty() {
        return true;
    }
    let message = format!(
        "dual-reference identity acceptance requires java, javac, and javap from JDK 21.0.13; unavailable: {}",
        unavailable.join(", ")
    );
    if std::env::var("DOTTY_CLASSFILE_REQUIRE_JDK").as_deref() == Ok("1") {
        panic!("{message}");
    }
    eprintln!("skipping optional strict dual-reference identity test: {message}");
    false
}

fn decode(bytes: &[u8]) -> ClassFile<'_> {
    let mut reader = Reader::new(bytes);
    let class = ClassFile::decode(&mut reader).unwrap();
    assert!(reader.is_at_end());
    class
}

fn method<'a, 'b>(
    class: &'a ClassFile<'b>,
    name: &str,
    descriptor: &str,
) -> &'a dotty_classfile::method::MethodInfo<'b> {
    class
        .methods
        .iter()
        .find(|method| {
            class.constant_pool.utf8(method.name_index) == Ok(name)
                && class.constant_pool.utf8(method.descriptor_index) == Ok(descriptor)
        })
        .unwrap()
}

fn code<'a, 'b>(
    method: &'a dotty_classfile::method::MethodInfo<'b>,
) -> &'a dotty_classfile::attribute::CodeAttribute<'b> {
    let [Attribute::Code(code)] = method.attributes.as_slice() else {
        panic!("method has exactly one Code attribute")
    };
    code
}

fn opcode_offsets(code: &[u8], opcode: u8) -> Vec<usize> {
    code.iter()
        .enumerate()
        .filter_map(|(pc, byte)| (*byte == opcode).then_some(pc))
        .collect()
}

fn code_offset(class_bytes: &[u8], code: &[u8]) -> usize {
    (code.as_ptr() as usize) - (class_bytes.as_ptr() as usize)
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
}
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn set_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}
fn set_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

fn javap(root: &std::path::Path, name: &str) -> String {
    let output = Command::new("javap")
        .args(["-v", "-p", "-c", "-classpath"])
        .arg(root)
        .arg(name)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "javap rejected Rust class: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn write_classes(
    root: &std::path::Path,
    batch: &dotty::jvm_tasty_artifacts::PairedJvmTastyBatchArtifacts,
    omitted: Option<&str>,
) {
    for pair in batch.classes() {
        let path = pair.classfile_path();
        if omitted == Some(path) {
            continue;
        }
        let destination = root.join(path);
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        std::fs::write(destination, pair.classfile_bytes()).unwrap();
        let tasty_destination = root.join(pair.tasty_path());
        std::fs::create_dir_all(tasty_destination.parent().unwrap()).unwrap();
        std::fs::write(tasty_destination, pair.tasty_bytes()).unwrap();
    }
}

fn run_negative(
    scratch: &TestDirectory,
    batch: &dotty::jvm_tasty_artifacts::PairedJvmTastyBatchArtifacts,
    harness: &std::path::Path,
    label: &str,
    pair_bytes: &[u8],
    expected: &str,
) {
    let root = scratch.path().join(label);
    write_classes(&root, batch, Some("example/Pair.class"));
    let pair = root.join("example/Pair.class");
    std::fs::write(pair, pair_bytes).unwrap();
    let cp = std::env::join_paths([harness, root.as_path()]).unwrap();
    let result = run_java(&cp, "DualReferenceHarness");
    assert!(
        !result.status.success(),
        "JVM/harness accepted negative {label}"
    );
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        output.contains(expected),
        "{label} did not fail first at expected stage {expected:?}:\n{output}"
    );
    let stage = if expected.starts_with("AssertionError") {
        "identity-harness"
    } else if expected.contains("NoSuchMethodError") {
        "JVM-linkage"
    } else if expected.contains("ClassFormatError") {
        "JVM-class-format"
    } else {
        "JVM-verifier"
    };
    let evidence = output
        .lines()
        .find(|line| {
            line.contains("AssertionError:")
                || line.contains("VerifyError:")
                || line.contains("ClassFormatError:")
                || line.contains("NoSuchMethodError:")
        })
        .unwrap_or("failure detail not found");
    eprintln!("dual-reference-negative {label} first-stage={stage} evidence={evidence}");
}

fn run_jvm_rejection(
    scratch: &TestDirectory,
    batch: &dotty::jvm_tasty_artifacts::PairedJvmTastyBatchArtifacts,
    harness: &std::path::Path,
    label: &str,
    pair_bytes: &[u8],
    expected: &str,
) {
    run_negative(scratch, batch, harness, label, pair_bytes, expected);
}

fn run_linkage_rejection(
    scratch: &TestDirectory,
    batch: &dotty::jvm_tasty_artifacts::PairedJvmTastyBatchArtifacts,
    harness: &std::path::Path,
    label: &str,
    pair_bytes: &[u8],
    expected: &str,
) {
    run_negative(scratch, batch, harness, label, pair_bytes, expected);
}

fn run_java(classpath: &std::ffi::OsStr, main: &str) -> std::process::Output {
    Command::new("java")
        .args(["-Xverify:all", "-ea", "-classpath"])
        .arg(classpath)
        .arg(main)
        .output()
        .unwrap()
}

fn find_methodref(
    class: &ClassFile<'_>,
    class_bytes: &[u8],
    owner: &str,
    name: &str,
    descriptor: &str,
) -> ConstantPoolIndex {
    (1..u16_at(class_bytes, 8) as usize)
        .find_map(|index| {
            let index = ConstantPoolIndex(index as u16);
            match class.constant_pool.get(index)? {
                ConstantPoolEntry::Methodref {
                    class_index,
                    name_and_type_index,
                } if class.constant_pool.class_name(*class_index).ok() == Some(owner) => {
                    (class.constant_pool.name_and_type(*name_and_type_index).ok()
                        == Some((name, descriptor)))
                    .then_some(index)
                }
                _ => None,
            }
        })
        .unwrap()
}

fn rebuild_pool(
    class: &mut ClassFile<'_>,
    class_bytes: &[u8],
    mutate: impl FnOnce(&mut Vec<Option<ConstantPoolEntry>>),
) {
    let mut entries = (1..u16_at(class_bytes, 8) as usize)
        .map(|index| {
            class
                .constant_pool
                .get(ConstantPoolIndex(index as u16))
                .cloned()
        })
        .collect::<Vec<_>>();
    mutate(&mut entries);
    class.constant_pool = dotty_classfile::constant_pool::ConstantPool::from_entries(entries);
}

fn find_utf8(class: &ClassFile<'_>, class_bytes: &[u8], value: &str) -> ConstantPoolIndex {
    (1..=u16_at(class_bytes, 8) as usize)
        .find_map(|index| {
            let index = ConstantPoolIndex(index as u16);
            (class.constant_pool.get(index) == Some(&ConstantPoolEntry::Utf8(value.to_owned())))
                .then_some(index)
        })
        .unwrap()
}

fn find_class(class: &ClassFile<'_>, class_bytes: &[u8], name: &str) -> ConstantPoolIndex {
    (1..=u16_at(class_bytes, 8) as usize)
        .find_map(|index| {
            let index = ConstantPoolIndex(index as u16);
            match class.constant_pool.get(index)? {
                ConstantPoolEntry::Class { .. }
                    if class.constant_pool.class_name(index).ok() == Some(name) =>
                {
                    Some(index)
                }
                _ => None,
            }
        })
        .unwrap()
}

fn replace_methodref_class(
    class: &mut ClassFile<'_>,
    class_bytes: &[u8],
    target: ConstantPoolIndex,
    replacement: ConstantPoolIndex,
) {
    rebuild_pool(class, class_bytes, |entries| {
        let Some(Some(ConstantPoolEntry::Methodref { class_index, .. })) =
            entries.get_mut(target.0 as usize - 1)
        else {
            panic!("target is a Methodref")
        };
        *class_index = replacement;
    });
}

fn replace_methodref_descriptor(
    class: &mut ClassFile<'_>,
    class_bytes: &[u8],
    target: ConstantPoolIndex,
    descriptor: ConstantPoolIndex,
) {
    rebuild_pool(class, class_bytes, |entries| {
        let name_and_type = match entries[target.0 as usize - 1].as_ref().unwrap() {
            ConstantPoolEntry::Methodref {
                name_and_type_index,
                ..
            } => *name_and_type_index,
            _ => panic!("target is Methodref"),
        };
        let Some(Some(ConstantPoolEntry::NameAndType {
            descriptor_index, ..
        })) = entries.get_mut(name_and_type.0 as usize - 1)
        else {
            panic!("Methodref points to NameAndType")
        };
        *descriptor_index = descriptor;
    });
}

fn replace_methodref_name(
    class: &mut ClassFile<'_>,
    class_bytes: &[u8],
    target: ConstantPoolIndex,
    name: ConstantPoolIndex,
) {
    rebuild_pool(class, class_bytes, |entries| {
        let name_and_type = match entries[target.0 as usize - 1].as_ref().unwrap() {
            ConstantPoolEntry::Methodref {
                name_and_type_index,
                ..
            } => *name_and_type_index,
            _ => panic!("target is Methodref"),
        };
        let Some(Some(ConstantPoolEntry::NameAndType { name_index, .. })) =
            entries.get_mut(name_and_type.0 as usize - 1)
        else {
            panic!("Methodref points to NameAndType")
        };
        *name_index = name;
    });
}

fn replace_fieldref_descriptor(
    class: &mut ClassFile<'_>,
    class_bytes: &[u8],
    target: ConstantPoolIndex,
    descriptor: ConstantPoolIndex,
) {
    rebuild_pool(class, class_bytes, |entries| {
        let name_and_type = match entries[target.0 as usize - 1].as_ref().unwrap() {
            ConstantPoolEntry::Fieldref {
                name_and_type_index,
                ..
            } => *name_and_type_index,
            _ => panic!("target is Fieldref"),
        };
        let Some(Some(ConstantPoolEntry::NameAndType {
            descriptor_index, ..
        })) = entries.get_mut(name_and_type.0 as usize - 1)
        else {
            panic!("Fieldref points to NameAndType")
        };
        *descriptor_index = descriptor;
    });
}

fn sha256sum(path: &std::path::Path) -> String {
    let output = Command::new("sha256sum").arg(path).output().unwrap();
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned()
}

fn assert_hashes(paths: &[std::path::PathBuf], expected: &[String], stage: &str) {
    let actual = paths.iter().map(|path| sha256sum(path)).collect::<Vec<_>>();
    assert_eq!(
        actual, expected,
        "the original Rust artifacts changed during {stage}"
    );
    eprintln!("dual-reference-original-hashes-after-{stage} {actual:?}");
}

struct TestDirectory(std::path::PathBuf);
impl TestDirectory {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "dotty-jvm-tasty-{}-{label}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
