use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_batch;
use dotty::jvm_tasty_plan::with_paired_jvm_tasty_batch_plan;
use dotty_classfile::attribute::Attribute;
use dotty_classfile::bytecode::{
    CfgCodeAnalyzer, ClassReference, CodeSequence, ConstantPoolIndexOperand, ControlFlowGraph,
    Instruction, LocalIndex, SpecialMethodReference,
};
use dotty_classfile::classfile::{ClassFile, Reader};
use dotty_classfile::constant_pool::{ConstantPoolEntry, ConstantPoolIndex};
use dotty_classfile::tasty_link::TastyLinkId;
use dotty_core::SourceId;
use dotty_jvm_tasty_contract::{
    JvmTastyBatchArtifacts, JvmTastyClassFileArtifact, JvmTastyContractError,
    JvmTastySidecarArtifact, validate_jvm_tasty_batch, validate_jvm_tasty_batch_artifacts,
};
use dotty_tasty::tasty::TastyFile;
use std::io::Write;
use std::process::{Command, Stdio};

const MIXED_ORIGIN_WRAP_AND_CREATE_BATCH: &str = include_str!(
    "fixtures/jvm_tasty_mixed_origin_constructor_arguments/wrap-and-create/PairFactory.scala"
);
const MIXED_ORIGIN_CREATE_AND_WRAP_BATCH: &str = include_str!(
    "fixtures/jvm_tasty_mixed_origin_constructor_arguments/create-and-wrap/PairFactory.scala"
);
const MIXED_ORIGIN_IDENTITY_HARNESS: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_mixed_origin_constructor_arguments/PairFactoryIdentityHarness.java"
);

fn require_exact_jdk_21_or_skip() -> bool {
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
        true
    } else if std::env::var("DOTTY_CLASSFILE_REQUIRE_JDK").as_deref() == Ok("1") {
        panic!(
            "strict mixed runtime requires java, javac, and javap from JDK 21.0.13; unavailable: {}",
            unavailable.join("; ")
        );
    } else {
        eprintln!(
            "skipping optional mixed runtime JDK test: {}",
            unavailable.join("; ")
        );
        false
    }
}

#[test]
fn missing_jdk_tools_skip_optional_run_and_fail_required_run() {
    let test_binary = std::env::current_exe().expect("current integration test binary");
    let run_jdk_test = |required: bool| {
        let mut command = Command::new(&test_binary);
        command
            .args([
                "--exact",
                "mixed_origin_counter_identity_and_allocation_order_on_strict_jdk21",
                "--nocapture",
            ])
            .env("PATH", "/__dotty_issue2016_no_jdk_tools__");
        if required {
            command.env("DOTTY_CLASSFILE_REQUIRE_JDK", "1");
        } else {
            command.env_remove("DOTTY_CLASSFILE_REQUIRE_JDK");
        }
        command.output().expect("run JDK test in isolated process")
    };

    let optional = run_jdk_test(false);
    let optional_output = format!(
        "{}{}",
        String::from_utf8_lossy(&optional.stdout),
        String::from_utf8_lossy(&optional.stderr)
    );
    assert!(optional.status.success(), "{optional_output}");
    assert!(
        optional_output.contains("skipping optional mixed runtime JDK test"),
        "{optional_output}"
    );
    for tool in ["java", "javac", "javap"] {
        assert!(
            optional_output.contains(&format!("{tool} unavailable")),
            "{optional_output}"
        );
    }

    let required = run_jdk_test(true);
    let required_output = format!(
        "{}{}",
        String::from_utf8_lossy(&required.stdout),
        String::from_utf8_lossy(&required.stderr)
    );
    assert!(!required.status.success(), "{required_output}");
    assert!(
        required_output
            .contains("strict mixed runtime requires java, javac, and javap from JDK 21.0.13"),
        "{required_output}"
    );
    for tool in ["java", "javac", "javap"] {
        assert!(
            required_output.contains(&format!("{tool} unavailable")),
            "{required_output}"
        );
    }
}

fn batch_class_name<'a>(class: &'a ClassFile<'a>, index: ConstantPoolIndex) -> &'a str {
    let ConstantPoolEntry::Class { name_index } = class.constant_pool.get(index).unwrap() else {
        panic!("allocation operand is a CONSTANT_Class");
    };
    class.constant_pool.utf8(*name_index).unwrap()
}

fn batch_method_ref<'a>(
    class: &'a ClassFile<'a>,
    index: ConstantPoolIndex,
) -> (&'a str, (&'a str, &'a str)) {
    let ConstantPoolEntry::Methodref {
        class_index,
        name_and_type_index,
    } = class.constant_pool.get(index).unwrap()
    else {
        panic!("invokespecial operand is a CONSTANT_Methodref");
    };
    (
        class.constant_pool.class_name(*class_index).unwrap(),
        class
            .constant_pool
            .name_and_type(*name_and_type_index)
            .unwrap(),
    )
}

fn sha256sum(path: &std::path::Path) -> String {
    let output = Command::new("sha256sum")
        .arg(path)
        .output()
        .expect("start sha256sum");
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned()
}

fn sha256sum_bytes(bytes: &[u8]) -> String {
    let mut child = Command::new("sha256sum")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("start sha256sum");
    child.stdin.take().unwrap().write_all(bytes).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned()
}

struct TestDirectory(std::path::PathBuf);
impl TestDirectory {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "dotty-mixed-runtime-{}-{label}-{}",
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

#[derive(Debug, Clone, Copy)]
struct MixedInstruction {
    pc: usize,
    opcode: u8,
    operand: Option<ConstantPoolIndex>,
    len: usize,
}

type MixedBytecodeMutator<'a> = Box<dyn Fn(&mut Vec<u8>) + 'a>;

fn mixed_factory_code<'a>(
    bytes: &'a [u8],
    name: &str,
) -> (
    ClassFile<'a>,
    Vec<MixedInstruction>,
    CodeSequence,
    String,
    usize,
    usize,
) {
    let class = ClassFile::decode(&mut Reader::new(bytes)).unwrap();
    let method = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == name)
        .unwrap_or_else(|| panic!("PairFactory declares {name}"));
    let descriptor = class
        .constant_pool
        .utf8(method.descriptor_index)
        .unwrap()
        .to_owned();
    let [Attribute::Code(code)] = method.attributes.as_slice() else {
        panic!("{name} has one Code attribute")
    };
    let code_start = code.code.as_ptr() as usize - bytes.as_ptr() as usize;
    let mut pc = 0;
    let mut decoded = Vec::new();
    let mut sequence = CodeSequence::new();
    while pc < code.code.len() {
        let start = pc;
        let opcode = code.code[pc];
        pc += 1;
        let operand = match opcode {
            0xbb | 0xb7 => {
                let index =
                    ConstantPoolIndex(u16::from_be_bytes([code.code[pc], code.code[pc + 1]]));
                pc += 2;
                Some(index)
            }
            _ => None,
        };
        let cp = |index: ConstantPoolIndex| {
            ConstantPoolIndexOperand::try_from(index.0).expect("valid constant-pool index")
        };
        match (opcode, operand) {
            (0xbb, Some(index)) => {
                sequence.instruction(Instruction::New(ClassReference::new(cp(index))));
            }
            (0x59, None) => {
                sequence.instruction(Instruction::Dup);
            }
            (0x2b, None) => {
                sequence.instruction(Instruction::ALoad(LocalIndex::new(1)));
            }
            (0x2c, None) => {
                sequence.instruction(Instruction::ALoad(LocalIndex::new(2)));
            }
            (0x1b, None) => {
                sequence.instruction(Instruction::ILoad(LocalIndex::new(1)));
            }
            (0x1c, None) => {
                sequence.instruction(Instruction::ILoad(LocalIndex::new(2)));
            }
            (0xb7, Some(index)) => {
                sequence.instruction(Instruction::InvokeSpecial(SpecialMethodReference::new(cp(
                    index,
                ))));
            }
            (0xb0, None) => {
                sequence.instruction(Instruction::AReturn);
            }
            (other, _) => panic!("unexpected mixed bytecode {other:#04x} at PC {start}"),
        }
        decoded.push(MixedInstruction {
            pc: start,
            opcode,
            operand,
            len: pc - start,
        });
    }
    assert_eq!(
        pc,
        code.code.len(),
        "instruction walk consumes all code bytes"
    );
    let max_stack = usize::from(code.max_stack);
    (class, decoded, sequence, descriptor, code_start, max_stack)
}

fn mixed_factory_bytes(batch: &dotty::jvm_tasty_artifacts::PairedJvmTastyBatchArtifacts) -> &[u8] {
    batch
        .classes()
        .iter()
        .find(|artifact| artifact.classfile_path() == "example/PairFactory.class")
        .expect("original Rust PairFactory classfile")
        .classfile_bytes()
}

fn mixed_code_bytes(bytes: &[u8], name: &str) -> (usize, Vec<u8>) {
    let class = ClassFile::decode(&mut Reader::new(bytes)).unwrap();
    let method = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == name)
        .unwrap();
    let [Attribute::Code(code)] = method.attributes.as_slice() else {
        panic!("{name} has one Code attribute")
    };
    (
        code.code.as_ptr() as usize - bytes.as_ptr() as usize,
        code.code.to_vec(),
    )
}

fn assert_mixed_original_files_unchanged(
    snapshots: &[(std::path::PathBuf, String, Vec<u8>)],
    stage: &str,
) {
    for (path, relative, bytes) in snapshots {
        assert_eq!(
            sha256sum(path),
            sha256sum_bytes(bytes),
            "{stage}: {relative}"
        );
        assert_eq!(std::fs::read(path).unwrap(), *bytes, "{stage}: {relative}");
    }
}

fn write_mixed_runtime_copy(
    root: &std::path::Path,
    batch: &dotty::jvm_tasty_artifacts::PairedJvmTastyBatchArtifacts,
    factory_override: Option<&[u8]>,
    missing: Option<&str>,
) {
    for artifact in batch.classes() {
        if missing == Some(artifact.classfile_path()) {
            continue;
        }
        let path = root.join(artifact.classfile_path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bytes = if artifact.classfile_path() == "example/PairFactory.class" {
            factory_override.unwrap_or(artifact.classfile_bytes())
        } else {
            artifact.classfile_bytes()
        };
        std::fs::write(path, bytes).unwrap();
    }
}

fn run_mixed_java(
    root: &std::path::Path,
    harness_classes: &std::path::Path,
    reversed: bool,
) -> std::process::Output {
    let classpath = std::env::join_paths([harness_classes, root]).unwrap();
    let mut command = Command::new("java");
    command
        .args(["-Xverify:all", "-ea", "-classpath"])
        .arg(classpath)
        .arg("example.PairFactoryIdentityHarness");
    if reversed {
        command.arg("reversed");
    }
    command
        .output()
        .expect("start strict JDK21 identity harness")
}

#[test]
fn mixed_origin_counter_identity_and_allocation_order_on_strict_jdk21() {
    if !require_exact_jdk_21_or_skip() {
        return;
    }
    let version = Command::new("java")
        .arg("-version")
        .output()
        .expect("read Java version");
    assert!(version.status.success());
    let version = format!(
        "{}{}",
        String::from_utf8_lossy(&version.stdout),
        String::from_utf8_lossy(&version.stderr)
    );
    assert!(
        version.contains("Temurin") && version.contains("21.0.13"),
        "{version}"
    );
    eprintln!("mixed-runtime-jdk: {}", version.trim());

    let scratch = TestDirectory::new("mixed-origin-runtime");
    let roots = scratch.path().join("six-original-files");
    let compile_classes = scratch.path().join("original-classfiles");
    let harness_source_dir = scratch.path().join("java-source");
    let harness_classes = scratch.path().join("java-classes");
    for path in [
        &roots,
        &compile_classes,
        &harness_source_dir,
        &harness_classes,
    ] {
        std::fs::create_dir_all(path).unwrap();
    }
    let harness = harness_source_dir.join("PairFactoryIdentityHarness.java");
    std::fs::write(&harness, MIXED_ORIGIN_IDENTITY_HARNESS).unwrap();
    eprintln!("mixed-runtime-harness-sha256: {}", sha256sum(&harness));

    let variants = [
        (
            MIXED_ORIGIN_WRAP_AND_CREATE_BATCH,
            "wrapAndCreate",
            false,
            20161,
        ),
        (
            MIXED_ORIGIN_CREATE_AND_WRAP_BATCH,
            "createAndWrap",
            true,
            20162,
        ),
    ];
    let mut batches = Vec::new();
    let mut snapshots = Vec::new();
    for (source, method_name, reversed, source_index) in variants {
        let batch = emit_paired_jvm_tasty_batch(
            source,
            "PairFactory.scala",
            SourceId::from_index(source_index),
        )
        .expect("one authentic source/typed session emits six Rust artifacts");
        assert_eq!(batch.classes().len(), 3);
        let original_dir = roots.join(method_name);
        let runtime_dir = compile_classes.join(method_name);
        std::fs::create_dir_all(&original_dir).unwrap();
        std::fs::create_dir_all(&runtime_dir).unwrap();
        let mut count = 0;
        for artifact in batch.classes() {
            let mut reader = Reader::new(artifact.classfile_bytes());
            let class = ClassFile::decode(&mut reader).unwrap();
            assert!(reader.is_at_end());
            assert_eq!((class.version.major, class.version.minor), (65, 0));
            assert_eq!(
                class.tasty_link_id().unwrap(),
                Some(artifact.tasty_link_id())
            );
            let tasty = TastyFile::parse_and_validate_scala_3_9(artifact.tasty_bytes()).unwrap();
            assert_eq!(
                (
                    tasty.header().major_version,
                    tasty.header().minor_version,
                    tasty.header().experimental_version
                ),
                (28, 9, 0)
            );
            assert_eq!(
                TastyLinkId::from_bytes(tasty.header().uuid),
                artifact.tasty_link_id()
            );
            for (relative, bytes) in [
                (artifact.classfile_path(), artifact.classfile_bytes()),
                (artifact.tasty_path(), artifact.tasty_bytes()),
            ] {
                let path = original_dir.join(relative);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, bytes).unwrap();
                snapshots.push((path, relative.to_owned(), bytes.to_vec()));
                eprintln!(
                    "mixed-runtime-original-sha256 {method_name} {relative} {}",
                    sha256sum_bytes(bytes)
                );
                count += 1;
            }
            let path = runtime_dir.join(artifact.classfile_path());
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, artifact.classfile_bytes()).unwrap();
        }
        assert_eq!(count, 6);

        let (class, instructions, sequence, descriptor, code_start, max_stack) =
            mixed_factory_code(mixed_factory_bytes(&batch), method_name);
        assert_eq!(descriptor, "(Lexample/Counter;I)Lexample/Pair;");
        assert_eq!(class.version.major, 65);
        let method = class
            .methods
            .iter()
            .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == method_name)
            .unwrap();
        let [Attribute::Code(code)] = method.attributes.as_slice() else {
            panic!("factory method has Code")
        };
        assert_eq!(code.max_locals, 3);
        let allocations = instructions
            .iter()
            .filter(|i| i.opcode == 0xbb)
            .map(|i| (i.pc, batch_class_name(&class, i.operand.unwrap())))
            .collect::<Vec<_>>();
        assert_eq!(allocations.len(), 2);
        assert_eq!(
            allocations
                .iter()
                .filter(|(_, n)| *n == "example/Pair")
                .count(),
            1
        );
        assert_eq!(
            allocations
                .iter()
                .filter(|(_, n)| *n == "example/Counter")
                .count(),
            1
        );
        let constructors = instructions
            .iter()
            .filter(|i| i.opcode == 0xb7)
            .map(|i| (i.pc, batch_method_ref(&class, i.operand.unwrap())))
            .collect::<Vec<_>>();
        assert_eq!(constructors.len(), 2);
        assert_eq!(
            constructors
                .iter()
                .filter(
                    |(_, (owner, sig))| *owner == "example/Counter" && *sig == ("<init>", "(I)V")
                )
                .count(),
            1
        );
        assert_eq!(
            constructors
                .iter()
                .filter(|(_, (owner, sig))| *owner == "example/Pair"
                    && *sig == ("<init>", "(Lexample/Counter;Lexample/Counter;)V"))
                .count(),
            1
        );
        assert_eq!(instructions.last().map(|i| i.opcode), Some(0xb0));
        assert!(
            instructions.iter().any(|i| i.opcode == 0x2b),
            "aload_1 carries existing Counter"
        );
        assert!(
            instructions.iter().any(|i| i.opcode == 0x1c),
            "iload_2 carries seed"
        );
        let graph = ControlFlowGraph::build(&sequence).expect("decoded straight-line control flow");
        let analysis = CfgCodeAnalyzer::analyze_with_current_class(
            &graph,
            method_name,
            &descriptor,
            false,
            &class.constant_pool,
            class.this_class,
        )
        .expect("class-aware CFG confirms initialized Pair return and allocation flow");
        assert_eq!(analysis.max_stack, u16::try_from(max_stack).unwrap());
        assert_eq!(analysis.max_locals, code.max_locals);
        eprintln!(
            "mixed-runtime-method {method_name}{descriptor} code_start={code_start} max_stack={max_stack} max_locals={}",
            code.max_locals
        );
        for i in &instructions {
            let detail = match (i.opcode, i.operand) {
                (0xbb, Some(cp)) => format!("new {} cp#{}", batch_class_name(&class, cp), cp.0),
                (0xb7, Some(cp)) => {
                    let (owner, (name, desc)) = batch_method_ref(&class, cp);
                    format!("invokespecial {owner}.{name}{desc} cp#{}", cp.0)
                }
                (0x59, _) => "dup".to_owned(),
                (0x2b, _) => "aload_1 (existing Counter)".to_owned(),
                (0x1c, _) => "iload_2 (seed Int)".to_owned(),
                (0xb0, _) => "areturn".to_owned(),
                _ => unreachable!(),
            };
            eprintln!("mixed-runtime-bytecode {method_name} pc={} {detail}", i.pc);
        }
        let javap = Command::new("javap")
            .args(["-v", "-p", "-c", "-classpath"])
            .arg(&runtime_dir)
            .arg("example.PairFactory")
            .output()
            .expect("run javap on original Rust factory");
        assert!(
            javap.status.success(),
            "javap: {}",
            String::from_utf8_lossy(&javap.stderr)
        );
        let dump = String::from_utf8_lossy(&javap.stdout);
        let stack_line = format!("stack={max_stack}, locals=3");
        for expected in [
            "major version: 65",
            "descriptor: (Lexample/Counter;I)Lexample/Pair;",
            "example/Counter.\"<init>\":(I)V",
            "example/Pair.\"<init>\":(Lexample/Counter;Lexample/Counter;)V",
            "areturn",
            stack_line.as_str(),
        ] {
            assert!(dump.contains(expected), "javap omitted {expected}:\n{dump}");
        }
        eprintln!("mixed-runtime-javap {method_name}: inspected the original classfile");
        batches.push((batch, method_name, reversed, runtime_dir));
    }

    for (path, relative, bytes) in &snapshots {
        assert_eq!(
            sha256sum(path),
            sha256sum_bytes(bytes),
            "pre-javac hash: {relative}"
        );
    }
    let javac = Command::new("javac")
        .args(["--release", "21", "-classpath"])
        .arg(&batches[0].3)
        .arg("-d")
        .arg(&harness_classes)
        .arg(&harness)
        .output()
        .expect("javac --release 21 for separate Java harness");
    assert!(
        javac.status.success(),
        "javac failed:\n{}\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    for (batch, name, reversed, runtime_dir) in &batches {
        let _ = batch;
        let result = run_mixed_java(runtime_dir, &harness_classes, *reversed);
        assert!(
            result.status.success(),
            "java -Xverify:all failed for {name}:\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        let expected =
            format!("{name}=identity:true;first=42/-9;repeat=11;zero=0;negative=-5;fresh:true");
        assert_eq!(String::from_utf8_lossy(&result.stdout).trim(), expected);
        eprintln!(
            "mixed-runtime-positive {name}: {}",
            String::from_utf8_lossy(&result.stdout).trim()
        );
    }
    for (path, relative, bytes) in &snapshots {
        assert_eq!(
            sha256sum(path),
            sha256sum_bytes(bytes),
            "post-javac/runtime hash: {relative}"
        );
        assert_eq!(
            std::fs::read(path).unwrap(),
            *bytes,
            "original file unchanged: {relative}"
        );
    }

    // Use the reversed method body as a verifier-valid corruption of the forward call. Its ABI
    // and constructor references remain type-correct; the original source-bound batch rejects it
    // and the runtime identity assertion independently exposes the changed Pair field.
    let (forward, _, _, _) = &batches[0];
    let (reverse, _, _, _) = &batches[1];
    let forward_factory = mixed_factory_bytes(forward);
    let reverse_factory = mixed_factory_bytes(reverse);
    let (_, forward_bytes) = mixed_code_bytes(forward_factory, "wrapAndCreate");
    let (forward_class, forward_instructions, _, _, forward_start, _) =
        mixed_factory_code(forward_factory, "wrapAndCreate");
    let (reverse_class, reverse_instructions, _, _, _, _) =
        mixed_factory_code(reverse_factory, "createAndWrap");
    let reverse_method = reverse
        .classes()
        .iter()
        .find(|artifact| artifact.classfile_path() == "example/PairFactory.class")
        .unwrap();
    let (_, reverse_bytes) = mixed_code_bytes(reverse_factory, "createAndWrap");
    assert_eq!(forward_bytes.len(), reverse_bytes.len());
    for i in reverse_instructions
        .iter()
        .filter(|i| matches!(i.opcode, 0xbb | 0xb7))
    {
        let target = i.operand.unwrap();
        let forward_target = forward_instructions
            .iter()
            .filter(|candidate| candidate.opcode == i.opcode)
            .find(|candidate| {
                let left = candidate.operand.unwrap();
                if i.opcode == 0xbb {
                    batch_class_name(&forward_class, left)
                        == batch_class_name(&reverse_class, target)
                } else {
                    batch_method_ref(&forward_class, left)
                        == batch_method_ref(&reverse_class, target)
                }
            });
        assert!(
            forward_target.is_some(),
            "source orientations resolve identical constructor CP entries"
        );
    }
    let mut swapped = forward_factory.to_vec();
    swapped[forward_start..forward_start + forward_bytes.len()].copy_from_slice(&reverse_bytes);
    let swapped_artifacts = JvmTastyBatchArtifacts {
        classfiles: forward
            .classes()
            .iter()
            .map(|a| JvmTastyClassFileArtifact {
                classfile_path: a.classfile_path().to_owned(),
                classfile_bytes: if a.classfile_path() == "example/PairFactory.class" {
                    swapped.clone()
                } else {
                    a.classfile_bytes().to_vec()
                },
            })
            .collect(),
        sidecars: forward
            .classes()
            .iter()
            .map(|a| JvmTastySidecarArtifact {
                tasty_path: a.tasty_path().to_owned(),
                tasty_bytes: a.tasty_bytes().to_vec(),
            })
            .collect(),
    };
    with_paired_jvm_tasty_batch_plan(
        MIXED_ORIGIN_WRAP_AND_CREATE_BATCH,
        "PairFactory.scala",
        SourceId::from_index(20163),
        |plan| {
            let tasty = plan
                .classes
                .iter()
                .map(|class| class.tasty_plan)
                .collect::<Vec<_>>();
            let validated = validate_jvm_tasty_batch(plan.jvm_unit, &tasty).unwrap();
            let error = validate_jvm_tasty_batch_artifacts(&validated, swapped_artifacts)
                .expect_err("verifier-valid reverse orientation fails the frozen forward receipt");
            assert!(
                matches!(
                    error,
                    JvmTastyContractError::BatchArtifact {
                        class_index: 2,
                        phase: "cross-class invocation",
                        ..
                    }
                ),
                "{error:?}"
            );
            eprintln!(
                "mixed-runtime-negative swapped-argument first-stage=Rust batch contract: {error:?}"
            );
        },
    )
    .unwrap();
    let swapped_root = scratch.path().join("swapped-source-orientation-copy");
    write_mixed_runtime_copy(&swapped_root, forward, Some(&swapped), None);
    let output = run_mixed_java(&swapped_root, &harness_classes, false);
    let output_text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success()
            && output_text.contains("AssertionError: caller Counter identity moved"),
        "wrong argument positions fail Java identity assertion:\n{output_text}"
    );
    assert!(
        !output_text.contains("VerifyError") && !output_text.contains("LinkageError"),
        "the swap is JVM-valid:\n{output_text}"
    );
    eprintln!(
        "mixed-runtime-negative swapped-argument first-stage=Java identity assertion: {}",
        output_text
            .lines()
            .find(|line| line.contains("AssertionError"))
            .unwrap()
    );

    // Replacing the nested allocation/init with a second aload_1 is valid Counter bytecode, but
    // aliases the existing object and violates exactly-one-fresh-Counter semantics.
    let inner_new = forward_instructions
        .iter()
        .find(|i| {
            i.opcode == 0xbb
                && batch_class_name(&forward_class, i.operand.unwrap()) == "example/Counter"
        })
        .unwrap();
    let counter_init = forward_instructions
        .iter()
        .find(|i| {
            i.opcode == 0xb7
                && batch_method_ref(&forward_class, i.operand.unwrap())
                    == ("example/Counter", ("<init>", "(I)V"))
        })
        .unwrap();
    let mut duplicate = forward_factory.to_vec();
    duplicate[forward_start + inner_new.pc] = 0x2b;
    duplicate[forward_start + inner_new.pc + 1..forward_start + counter_init.pc + counter_init.len]
        .fill(0x00);
    let duplicate_root = scratch.path().join("duplicate-existing-copy");
    write_mixed_runtime_copy(&duplicate_root, forward, Some(&duplicate), None);
    let output = run_mixed_java(&duplicate_root, &harness_classes, false);
    let output_text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success()
            && output_text.contains("AssertionError: factory did not allocate a distinct Counter"),
        "duplicated argument reaches identity assertion:\n{output_text}"
    );
    assert!(
        !output_text.contains("VerifyError"),
        "duplicating initialized Counter is verifier-valid:\n{output_text}"
    );
    eprintln!(
        "mixed-runtime-negative duplicate-existing first-stage=Java identity assertion: {}",
        output_text
            .lines()
            .find(|line| line.contains("AssertionError"))
            .unwrap()
    );

    let get_pc = |opcode: u8, condition: &dyn Fn(ConstantPoolIndex) -> bool| {
        forward_instructions
            .iter()
            .find(|i| i.opcode == opcode && i.operand.is_some_and(condition))
            .unwrap()
            .pc
    };
    let counter_new_pc = get_pc(0xbb, &|index| {
        batch_class_name(&forward_class, index) == "example/Counter"
    });
    let counter_init_pc = get_pc(0xb7, &|index| {
        batch_method_ref(&forward_class, index).0 == "example/Counter"
    });
    let pair_init_pc = get_pc(0xb7, &|index| {
        batch_method_ref(&forward_class, index).0 == "example/Pair"
    });
    let seed_pc = forward_instructions
        .iter()
        .find(|i| i.opcode == 0x1c)
        .unwrap()
        .pc;
    let inner_dup_pc = forward_instructions
        .iter()
        .find(|i| i.opcode == 0x59 && i.pc > counter_new_pc)
        .unwrap()
        .pc;
    let pair_init_index = forward_instructions
        .iter()
        .find(|i| i.pc == pair_init_pc)
        .unwrap()
        .operand
        .unwrap();
    let run_bad = |label: &str, edit: &dyn Fn(&mut Vec<u8>)| {
        let mut bytes = forward_factory.to_vec();
        edit(&mut bytes);
        let root = scratch.path().join(label);
        write_mixed_runtime_copy(&root, forward, Some(&bytes), None);
        let result = run_mixed_java(&root, &harness_classes, false);
        format!(
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        )
    };
    let pair_new_index = forward_instructions
        .iter()
        .find(|i| {
            i.opcode == 0xbb
                && batch_class_name(&forward_class, i.operand.unwrap()) == "example/Pair"
        })
        .unwrap()
        .operand
        .unwrap();
    let pair_new_pc = forward_instructions
        .iter()
        .find(|i| {
            i.opcode == 0xbb
                && batch_class_name(&forward_class, i.operand.unwrap()) == "example/Pair"
        })
        .unwrap()
        .pc;
    let counter_new_index = forward_instructions
        .iter()
        .find(|i| {
            i.opcode == 0xbb
                && batch_class_name(&forward_class, i.operand.unwrap()) == "example/Counter"
        })
        .unwrap()
        .operand
        .unwrap();
    let verifier_cases: [(&str, MixedBytecodeMutator<'_>); 7] = [
        (
            "missing-counter-new",
            Box::new(|bytes| {
                bytes[forward_start + counter_new_pc..forward_start + counter_new_pc + 3].fill(0)
            }),
        ),
        (
            "wrong-seed-slot",
            Box::new(|bytes| bytes[forward_start + seed_pc] = 0x2b),
        ),
        (
            "wrong-constructor-owner",
            Box::new(|bytes| {
                bytes[forward_start + counter_init_pc + 1..forward_start + counter_init_pc + 3]
                    .copy_from_slice(&pair_init_index.0.to_be_bytes())
            }),
        ),
        (
            "missing-counter-init",
            Box::new(|bytes| {
                bytes[forward_start + counter_init_pc..forward_start + counter_init_pc + 3].fill(0)
            }),
        ),
        (
            "invalid-uninitialized-receiver",
            Box::new(|bytes| bytes[forward_start + inner_dup_pc] = 0x00),
        ),
        (
            "wrong-new-class",
            Box::new(|bytes| {
                bytes[forward_start + counter_new_pc + 1..forward_start + counter_new_pc + 3]
                    .copy_from_slice(&pair_new_index.0.to_be_bytes())
            }),
        ),
        (
            "extra-counter-allocation",
            Box::new(|bytes| {
                bytes[forward_start + pair_new_pc + 1..forward_start + pair_new_pc + 3]
                    .copy_from_slice(&counter_new_index.0.to_be_bytes())
            }),
        ),
    ];
    for (label, edit) in verifier_cases {
        let output = run_bad(label, &edit);
        assert!(
            output.contains("VerifyError"),
            "{label} should fail JVM verification:\n{output}"
        );
        assert!(
            !output.contains("AssertionError") && !output.contains("NoSuchMethodError"),
            "{label} first fails verification:\n{output}"
        );
        eprintln!(
            "mixed-runtime-negative {label} first-stage=JVM verifier: {}",
            output
                .lines()
                .find(|line| line.contains("VerifyError"))
                .unwrap_or("VerifyError")
        );
    }
    let (max_stack_start, _) = mixed_code_bytes(forward_factory, "wrapAndCreate");
    let low_stack = run_bad("max-stack-too-low", &|bytes| {
        bytes[max_stack_start - 8..max_stack_start - 6].copy_from_slice(&1u16.to_be_bytes())
    });
    assert!(
        low_stack.contains("VerifyError"),
        "undersized max_stack: {low_stack}"
    );
    eprintln!(
        "mixed-runtime-negative max_stack-too-low first-stage=JVM verifier: {}",
        low_stack
            .lines()
            .find(|line| line.contains("VerifyError"))
            .unwrap_or("VerifyError")
    );
    let low_locals = run_bad("max-locals-too-low", &|bytes| {
        bytes[max_stack_start - 6..max_stack_start - 4].copy_from_slice(&2u16.to_be_bytes())
    });
    assert!(
        low_locals.contains("ClassFormatError"),
        "undersized max_locals is rejected while loading the class: {low_locals}"
    );
    eprintln!(
        "mixed-runtime-negative max_locals-too-low first-stage=JVM classfile loading: {}",
        low_locals
            .lines()
            .find(|line| line.contains("ClassFormatError"))
            .unwrap_or("ClassFormatError")
    );

    for missing in ["example/Counter.class", "example/Pair.class"] {
        let root = scratch
            .path()
            .join(format!("missing-{}", missing.replace('/', "-")));
        write_mixed_runtime_copy(&root, forward, None, Some(missing));
        let output = run_mixed_java(&root, &harness_classes, false);
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !output.status.success() && text.contains("NoClassDefFoundError"),
            "missing {missing}:\n{text}"
        );
        eprintln!(
            "mixed-runtime-negative missing {missing} first-stage=JVM linker: {}",
            text.lines()
                .find(|line| line.contains("NoClassDefFoundError"))
                .unwrap_or("NoClassDefFoundError")
        );
    }

    assert_mixed_original_files_unchanged(&snapshots, "all compiler/runtime/corrupt-copy stages");
    assert_eq!(reverse_method.classfile_path(), "example/PairFactory.class");
}
