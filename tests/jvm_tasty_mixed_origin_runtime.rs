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
const IMMUTABLE_LOCAL_PAIR_BATCH: &str =
    include_str!("fixtures/jvm_tasty_immutable_reference_local_pair/PairFactory.scala");
const IMMUTABLE_LOCAL_PAIR_REVERSED_BATCH: &str =
    include_str!("fixtures/jvm_tasty_immutable_reference_local_pair/PairFactoryReversed.scala");
const IMMUTABLE_LOCAL_PAIR_IDENTITY_HARNESS: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_immutable_reference_local_pair/PairFactoryIdentityHarness.java"
);
const TWO_LOCAL_PRIMARY_BATCH: &str =
    include_str!("fixtures/jvm_tasty_immutable_reference_local_pair/PairFactoryTwoLocals.scala");
const TWO_LOCAL_REVERSED_BATCH: &str = include_str!(
    "fixtures/jvm_tasty_immutable_reference_local_pair/PairFactoryTwoLocalsReversed.scala"
);
const TWO_LOCAL_ALIAS_BATCH: &str = include_str!(
    "fixtures/jvm_tasty_immutable_reference_local_pair/PairFactoryTwoLocalsAlias.scala"
);
const TWO_LOCAL_IDENTITY_HARNESS: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_immutable_reference_local_pair/PairFactoryTwoLocalIdentityHarness.java"
);

fn require_exact_jdk_21_or_skip() -> bool {
    let mut observed = Vec::new();
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
            observed.push(format!("{tool}={:?}", text.trim()));
            let matches_pin = match tool {
                "java" => {
                    text.contains("openjdk version \"21.0.13\"")
                        && text.contains("Temurin-21.0.13+9 (build 21.0.13+9-LTS)")
                }
                "javac" | "javap" => text.contains("21.0.13"),
                _ => false,
            };
            (!matches_pin).then(|| format!("{tool} reports {text:?}"))
        })
        .collect::<Vec<_>>();
    if unavailable.is_empty() {
        eprintln!("strict-jdk-provenance {}", observed.join("; "));
        true
    } else if std::env::var("DOTTY_CLASSFILE_REQUIRE_JDK").as_deref() == Ok("1") {
        panic!(
            "strict mixed runtime requires Eclipse Temurin JDK 21.0.13+9; observed [{}]; mismatches: {}",
            observed.join("; "),
            unavailable.join("; ")
        );
    } else {
        eprintln!(
            "skipping optional mixed runtime JDK test; observed [{}]; mismatches: {}",
            observed.join("; "),
            unavailable.join("; ")
        );
        false
    }
}

fn require_jdk_21_or_skip() -> bool {
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
            (!text.contains("21.0.")).then(|| format!("{tool} reports {text:?}"))
        })
        .collect::<Vec<_>>();
    if unavailable.is_empty() {
        true
    } else if std::env::var("DOTTY_CLASSFILE_REQUIRE_JDK").as_deref() == Ok("1") {
        panic!(
            "strict immutable-local Pair runtime requires java, javac, and javap from JDK 21; unavailable: {}",
            unavailable.join("; ")
        );
    } else {
        eprintln!(
            "skipping optional immutable-local Pair JDK test: {}",
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
        required_output.contains("strict mixed runtime requires Eclipse Temurin JDK 21.0.13+9"),
        "{required_output}"
    );
    for tool in ["java", "javac", "javap"] {
        assert!(
            required_output.contains(&format!("{tool} unavailable")),
            "{required_output}"
        );
    }
}

#[test]
fn strict_jdk_pin_rejects_other_vendor_in_required_mode() {
    use std::os::unix::fs::PermissionsExt;
    let fake_bin = TestDirectory::new("fake-jdk-vendor");
    let script = r#"#!/bin/sh
case "${0##*/}" in
  java) echo 'openjdk version "21.0.13"'; echo 'OpenJDK Runtime Environment OtherVendor-21.0.13+9 (build 21.0.13+9)';;
  javac) echo 'javac 21.0.13';;
  javap) echo '21.0.13';;
esac
"#;
    for tool in ["java", "javac", "javap"] {
        let path = fake_bin.path().join(tool);
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let test_binary = std::env::current_exe().expect("current integration test binary");
    let run = |required: bool| {
        let mut command = Command::new(&test_binary);
        command
            .args([
                "--exact",
                "two_immutable_local_pair_identity_and_decoded_flow_on_strict_jdk21",
                "--nocapture",
            ])
            .env("PATH", fake_bin.path());
        if required {
            command.env("DOTTY_CLASSFILE_REQUIRE_JDK", "1");
        } else {
            command.env_remove("DOTTY_CLASSFILE_REQUIRE_JDK");
        }
        command
            .output()
            .expect("run strict test with simulated non-Temurin tools")
    };
    let optional = run(false);
    let optional_text = format!(
        "{}{}",
        String::from_utf8_lossy(&optional.stdout),
        String::from_utf8_lossy(&optional.stderr)
    );
    assert!(optional.status.success(), "{optional_text}");
    assert!(
        optional_text.contains("skipping optional mixed runtime JDK test"),
        "{optional_text}"
    );
    assert!(
        optional_text.contains("OtherVendor-21.0.13+9"),
        "{optional_text}"
    );
    let required = run(true);
    let required_text = format!(
        "{}{}",
        String::from_utf8_lossy(&required.stdout),
        String::from_utf8_lossy(&required.stderr)
    );
    assert!(!required.status.success(), "{required_text}");
    assert!(
        required_text.contains("requires Eclipse Temurin JDK 21.0.13+9"),
        "{required_text}"
    );
    assert!(
        required_text.contains("OtherVendor-21.0.13+9"),
        "{required_text}"
    );
    let java = fake_bin.path().join("java");
    std::fs::write(
        &java,
        "#!/bin/sh\necho 'openjdk version \"21.0.13\"'\necho 'OpenJDK Runtime Environment Temurin-21.0.13+10 (build 21.0.13+10-LTS)'\n",
    )
    .unwrap();
    std::fs::set_permissions(&java, std::fs::Permissions::from_mode(0o755)).unwrap();
    let wrong_build = run(true);
    let wrong_build_text = format!(
        "{}{}",
        String::from_utf8_lossy(&wrong_build.stdout),
        String::from_utf8_lossy(&wrong_build.stderr)
    );
    assert!(!wrong_build.status.success(), "{wrong_build_text}");
    assert!(
        wrong_build_text.contains("Temurin-21.0.13+10"),
        "{wrong_build_text}"
    );
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
        let mut local_operand = None;
        let operand = match opcode {
            0x19 | 0x3a => {
                local_operand = Some(code.code[pc]);
                pc += 1;
                None
            }
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
            (0x2d, None) => {
                sequence.instruction(Instruction::ALoad(LocalIndex::new(3)));
            }
            (0x4e, None) => {
                sequence.instruction(Instruction::AStore(LocalIndex::new(3)));
            }
            (0x4c, None) => {
                sequence.instruction(Instruction::AStore(LocalIndex::new(1)));
            }
            (0x3a, None) => {
                sequence.instruction(Instruction::AStore(LocalIndex::new(u16::from(
                    local_operand.expect("explicit astore index"),
                ))));
            }
            (0x19, None) => {
                sequence.instruction(Instruction::ALoad(LocalIndex::new(u16::from(
                    local_operand.expect("explicit aload index"),
                ))));
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

fn constant_pool_entry_offset(bytes: &[u8], wanted: ConstantPoolIndex) -> usize {
    let count = usize::from(u16::from_be_bytes([bytes[8], bytes[9]]));
    let mut index = 1usize;
    let mut offset = 10usize;
    while index < count {
        if index == usize::from(wanted.0) {
            return offset;
        }
        let tag = bytes[offset];
        match tag {
            1 => {
                let length =
                    usize::from(u16::from_be_bytes([bytes[offset + 1], bytes[offset + 2]]));
                offset += 3 + length;
            }
            3 | 4 => offset += 5,
            5 | 6 => {
                offset += 9;
                index += 1;
            }
            7 | 8 | 16 | 19 | 20 => offset += 3,
            9 | 10 | 11 | 12 | 17 | 18 => offset += 5,
            15 => offset += 4,
            _ => panic!("unsupported constant-pool tag {tag}"),
        }
        index += 1;
    }
    panic!("constant-pool index {} does not exist", wanted.0)
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

#[test]
fn two_immutable_local_pair_identity_and_decoded_flow_on_strict_jdk21() {
    if !require_exact_jdk_21_or_skip() {
        return;
    }

    let scratch = TestDirectory::new("two-immutable-local-pair-runtime");
    let harness_source = scratch
        .path()
        .join("PairFactoryTwoLocalIdentityHarness.java");
    std::fs::write(&harness_source, TWO_LOCAL_IDENTITY_HARNESS).unwrap();

    let variants = [
        ("primary", TWO_LOCAL_PRIMARY_BATCH, "primary"),
        ("reversed", TWO_LOCAL_REVERSED_BATCH, "reversed"),
        ("alias", TWO_LOCAL_ALIAS_BATCH, "alias"),
    ];
    for (index, (label, source, harness_mode)) in variants.into_iter().enumerate() {
        let batch = emit_paired_jvm_tasty_batch(
            source,
            "PairFactory.scala",
            SourceId::from_index(21270 + index as u32),
        )
        .unwrap_or_else(|error| {
            panic!("{label} source emits its own six-file Rust batch: {error:?}")
        });
        assert_eq!(batch.classes().len(), 3);
        let original = scratch.path().join(format!("original-{label}"));
        let runtime = scratch.path().join(format!("runtime-{label}"));
        std::fs::create_dir_all(&original).unwrap();
        std::fs::create_dir_all(&runtime).unwrap();
        let mut hashes_before = Vec::new();
        for artifact in batch.classes() {
            let mut reader = Reader::new(artifact.classfile_bytes());
            let decoded = ClassFile::decode(&mut reader).unwrap();
            assert!(reader.is_at_end());
            assert_eq!((decoded.version.major, decoded.version.minor), (65, 0));
            assert_eq!(
                decoded.tasty_link_id().unwrap(),
                Some(artifact.tasty_link_id())
            );
            let tasty = TastyFile::parse_and_validate_scala_3_9(artifact.tasty_bytes()).unwrap();
            assert_eq!(
                (tasty.header().major_version, tasty.header().minor_version),
                (28, 9)
            );
            for (relative, bytes, runtime_class) in [
                (artifact.classfile_path(), artifact.classfile_bytes(), true),
                (artifact.tasty_path(), artifact.tasty_bytes(), false),
            ] {
                let path = original.join(relative);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, bytes).unwrap();
                let hash = sha256sum_bytes(bytes);
                eprintln!("two-local-original {label} {relative} sha256={hash}");
                hashes_before.push((path, relative.to_owned(), hash));
                if runtime_class {
                    let destination = runtime.join(relative);
                    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
                    std::fs::write(destination, bytes).unwrap();
                }
            }
        }
        assert_eq!(hashes_before.len(), 6);
        let harness_classes = scratch.path().join(format!("harness-{label}"));
        std::fs::create_dir(&harness_classes).unwrap();
        let harness_compile = Command::new("javac")
            .args(["--release", "21", "-classpath"])
            .arg(&runtime)
            .arg("-d")
            .arg(&harness_classes)
            .arg(&harness_source)
            .output()
            .expect("compile independent Java 21 identity harness against original classes");
        assert!(
            harness_compile.status.success(),
            "javac: {}{}",
            String::from_utf8_lossy(&harness_compile.stdout),
            String::from_utf8_lossy(&harness_compile.stderr)
        );

        let factory = mixed_factory_bytes(&batch);
        let (class, instructions, sequence, descriptor, code_start, _) =
            mixed_factory_code(factory, "create");
        assert_eq!(descriptor, "(II)Lexample/Pair;");
        let method = class
            .methods
            .iter()
            .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == "create")
            .unwrap();
        let [Attribute::Code(code)] = method.attributes.as_slice() else {
            panic!("create has one Code attribute")
        };
        assert_eq!(code.max_locals, 5);
        let allocations = instructions
            .iter()
            .filter(|i| i.opcode == 0xbb)
            .map(|i| {
                (
                    i.pc,
                    batch_class_name(&class, i.operand.unwrap()).to_owned(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            allocations
                .iter()
                .map(|(_, n)| n.as_str())
                .collect::<Vec<_>>(),
            ["example/Counter", "example/Counter", "example/Pair"]
        );
        let calls = instructions
            .iter()
            .filter(|i| i.opcode == 0xb7)
            .map(|i| (i.pc, batch_method_ref(&class, i.operand.unwrap())))
            .collect::<Vec<_>>();
        assert_eq!(
            calls
                .iter()
                .map(|(_, (owner, (name, desc)))| (*owner, *name, *desc))
                .collect::<Vec<_>>(),
            [
                ("example/Counter", "<init>", "(I)V"),
                ("example/Counter", "<init>", "(I)V"),
                (
                    "example/Pair",
                    "<init>",
                    "(Lexample/Counter;Lexample/Counter;)V"
                ),
            ]
        );
        let local_ops = instructions
            .iter()
            .filter(|i| matches!(i.opcode, 0x4e | 0x3a | 0x2d | 0x19))
            .map(|i| {
                let slot = match i.opcode {
                    0x4e => 3,
                    0x3a | 0x19 => factory[code_start + i.pc + 1] as usize,
                    0x2d => 3,
                    _ => unreachable!(),
                };
                (i.pc, i.opcode, slot)
            })
            .collect::<Vec<_>>();
        let stores = local_ops
            .iter()
            .filter(|(_, opcode, _)| matches!(opcode, 0x4e | 0x3a))
            .map(|(_, _, slot)| *slot)
            .collect::<Vec<_>>();
        let loads = local_ops
            .iter()
            .filter(|(_, opcode, _)| matches!(opcode, 0x2d | 0x19))
            .map(|(_, _, slot)| *slot)
            .collect::<Vec<_>>();
        assert_eq!(
            stores,
            [3, 4],
            "each initialized Counter has a distinct reference local store"
        );
        assert_eq!(
            loads,
            match label {
                "reversed" => vec![4, 3],
                "alias" => vec![3, 3],
                _ => vec![3, 4],
            },
            "Pair arguments preserve source local identity and order"
        );
        let graph = ControlFlowGraph::build(&sequence).expect("two-local method CFG decodes");
        let analysis = CfgCodeAnalyzer::analyze_with_current_class(
            &graph,
            "create",
            &descriptor,
            false,
            &class.constant_pool,
            class.this_class,
        )
        .expect("both reference locals are initialized before use");
        assert_eq!((analysis.max_stack, analysis.max_locals), (4, 5));
        eprintln!(
            "two-local-flow {label}{descriptor} allocations={allocations:?} constructors={calls:?} local_ops={local_ops:?} stack={} locals={}",
            analysis.max_stack, analysis.max_locals
        );

        let javap = Command::new("javap")
            .args(["-v", "-p", "-c", "-classpath"])
            .arg(&runtime)
            .arg("example.PairFactory")
            .output()
            .expect("disassemble original Rust PairFactory");
        assert!(
            javap.status.success(),
            "javap: {}",
            String::from_utf8_lossy(&javap.stderr)
        );
        let disassembly = String::from_utf8_lossy(&javap.stdout);
        assert!(disassembly.contains("descriptor: (II)Lexample/Pair;"));
        assert!(disassembly.contains("stack=4, locals=5"));
        for (pc, opcode, slot) in &local_ops {
            if *slot == 4 && matches!(opcode, 0x3a | 0x19) {
                let mnemonic = if *opcode == 0x3a { "astore" } else { "aload" };
                assert!(
                    disassembly.lines().any(|line| {
                        let fields = line.split_whitespace().collect::<Vec<_>>();
                        fields.len() >= 3
                            && fields[0] == format!("{pc}:")
                            && fields[1] == mnemonic
                            && fields[2] == "4"
                    }),
                    "javap lacks decoded {mnemonic} slot 4 at PC {pc}: {disassembly}"
                );
            }
        }
        eprintln!("two-local-javap {label}: actual PairFactory.create disassembly inspected");

        let java = Command::new("java")
            .args(["-Xverify:all", "-ea", "-classpath"])
            .arg(std::env::join_paths([harness_classes.as_path(), runtime.as_path()]).unwrap())
            .arg("example.PairFactoryTwoLocalIdentityHarness")
            .arg(harness_mode)
            .output()
            .expect("run unchanged original Rust classes under strict verifier");
        assert!(
            java.status.success(),
            "strict java {label}: {}{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        let expected = format!(
            "{harness_mode}=identity:true;mutations=isolated;repeat=fresh;seeds=7/3,3/7,-9/42;equalSeeds={}",
            if label == "alias" {
                "n/a"
            } else {
                "distinct-isolated"
            }
        );
        assert_eq!(String::from_utf8_lossy(&java.stdout).trim(), expected);
        if label == "primary" {
            let run_negative = |case: &str,
                                pair_factory: Option<&[u8]>,
                                omit: Option<&str>,
                                expected: &str| {
                let root = scratch.path().join(format!("negative-{case}"));
                for artifact in batch.classes() {
                    if omit == Some(artifact.classfile_path()) {
                        continue;
                    }
                    let destination = root.join(artifact.classfile_path());
                    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
                    let bytes = if artifact.classfile_path() == "example/PairFactory.class" {
                        pair_factory.unwrap_or(artifact.classfile_bytes())
                    } else {
                        artifact.classfile_bytes()
                    };
                    std::fs::write(destination, bytes).unwrap();
                }
                let output = Command::new("java")
                    .args(["-Xverify:all", "-ea", "-classpath"])
                    .arg(std::env::join_paths([harness_classes.as_path(), root.as_path()]).unwrap())
                    .arg("example.PairFactoryTwoLocalIdentityHarness")
                    .arg("primary")
                    .output()
                    .expect("run isolated two-local bytecode corruption");
                let text = format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(
                    !output.status.success(),
                    "negative case {case} unexpectedly passed"
                );
                assert!(
                    text.contains(expected),
                    "{case} expected first failure {expected:?}: {text}"
                );
                assert!(
                    !text.contains("VerifyError") || expected.contains("VerifyError"),
                    "misclassified verifier result for {case}: {text}"
                );
                if case == "uninitialized-counter-escapes-into-pair" {
                    assert!(
                        text.to_ascii_lowercase().contains("uninitialized"),
                        "must fail because the uninitialized Counter escapes: {text}"
                    );
                    assert!(
                        text.contains("Pair.<init>") || text.contains("@26: invokespecial"),
                        "failure must identify Pair constructor escape point: {text}"
                    );
                }
                eprintln!("two-local-negative {case} first-stage={expected}: {text}");
            };
            let (_, primary_instructions, _, _, primary_code_start, _) =
                mixed_factory_code(factory, "create");
            let load3 = primary_instructions
                .iter()
                .find(|i| i.opcode == 0x2d)
                .unwrap()
                .pc;
            let load4 = primary_instructions
                .iter()
                .find(|i| i.opcode == 0x19)
                .unwrap()
                .pc;
            let store4 = primary_instructions
                .iter()
                .find(|i| i.opcode == 0x3a)
                .unwrap()
                .pc;
            let mut swapped = factory.to_vec();
            swapped[primary_code_start + load3..primary_code_start + load4 + 2]
                .copy_from_slice(&[0x19, 4, 0x2d]);
            run_negative("swapped-pair-loads", Some(&swapped), None, "AssertionError");
            let swapped_artifacts = JvmTastyBatchArtifacts {
                classfiles: batch
                    .classes()
                    .iter()
                    .map(|artifact| JvmTastyClassFileArtifact {
                        classfile_path: artifact.classfile_path().to_owned(),
                        classfile_bytes: if artifact.classfile_path() == "example/PairFactory.class"
                        {
                            swapped.clone()
                        } else {
                            artifact.classfile_bytes().to_vec()
                        },
                    })
                    .collect(),
                sidecars: batch
                    .classes()
                    .iter()
                    .map(|artifact| JvmTastySidecarArtifact {
                        tasty_path: artifact.tasty_path().to_owned(),
                        tasty_bytes: artifact.tasty_bytes().to_vec(),
                    })
                    .collect(),
            };
            with_paired_jvm_tasty_batch_plan(
                TWO_LOCAL_PRIMARY_BATCH,
                "PairFactory.scala",
                SourceId::from_index(21279),
                |plan| {
                    let tasty = plan.classes.iter().map(|class| class.tasty_plan).collect::<Vec<_>>();
                    let validated = validate_jvm_tasty_batch(plan.jvm_unit, &tasty).unwrap();
                    let error = validate_jvm_tasty_batch_artifacts(&validated, swapped_artifacts)
                        .expect_err("the genuine paired Rust contract rejects swapped two-local Pair loads");
                    assert!(matches!(error, JvmTastyContractError::BatchArtifact { phase: "cross-class invocation", .. }), "unexpected first Rust contract error: {error:?}");
                    eprintln!("two-local-negative swapped-pair-loads first-stage=Rust paired contract: {error:?}");
                },
            ).expect("authenticate the same original two-local source session");

            let mut extra_counter = factory.to_vec();
            let counter_new_ref = primary_instructions
                .iter()
                .find(|i| i.opcode == 0xbb)
                .unwrap()
                .operand
                .unwrap();
            let counter_ctor_ref = primary_instructions
                .iter()
                .find(|i| {
                    i.opcode == 0xb7
                        && batch_method_ref(&class, i.operand.unwrap()).0 == "example/Counter"
                })
                .unwrap()
                .operand
                .unwrap();
            let pair_new_insert_pc = primary_instructions
                .iter()
                .find(|i| {
                    i.opcode == 0xbb
                        && batch_class_name(&class, i.operand.unwrap()) == "example/Pair"
                })
                .unwrap()
                .pc;
            let extra = [
                0xbb,
                (counter_new_ref.0 >> 8) as u8,
                counter_new_ref.0 as u8,
                0x59,
                0x1b,
                0xb7,
                (counter_ctor_ref.0 >> 8) as u8,
                counter_ctor_ref.0 as u8,
                0x57,
            ];
            let old_code_len = u32::from_be_bytes(
                extra_counter[primary_code_start - 4..primary_code_start]
                    .try_into()
                    .unwrap(),
            );
            let old_attribute_len = u32::from_be_bytes(
                extra_counter[primary_code_start - 12..primary_code_start - 8]
                    .try_into()
                    .unwrap(),
            );
            extra_counter.splice(
                primary_code_start + pair_new_insert_pc..primary_code_start + pair_new_insert_pc,
                extra,
            );
            extra_counter[primary_code_start - 4..primary_code_start]
                .copy_from_slice(&(old_code_len + 9).to_be_bytes());
            extra_counter[primary_code_start - 12..primary_code_start - 8]
                .copy_from_slice(&(old_attribute_len + 9).to_be_bytes());
            let extra_runtime = scratch.path().join("runtime-extra-counter-evaluation");
            for artifact in batch.classes() {
                let destination = extra_runtime.join(artifact.classfile_path());
                std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
                let bytes = if artifact.classfile_path() == "example/PairFactory.class" {
                    &extra_counter
                } else {
                    artifact.classfile_bytes()
                };
                std::fs::write(destination, bytes).unwrap();
            }
            let extra_java = Command::new("java")
                .args(["-Xverify:all", "-ea", "-classpath"])
                .arg(
                    std::env::join_paths([harness_classes.as_path(), extra_runtime.as_path()])
                        .unwrap(),
                )
                .arg("example.PairFactoryTwoLocalIdentityHarness")
                .arg("primary")
                .output()
                .expect("verify and execute independently constructed extra Counter clone");
            assert!(
                extra_java.status.success(),
                "balanced extra Counter class must independently verify and execute: {}{}",
                String::from_utf8_lossy(&extra_java.stdout),
                String::from_utf8_lossy(&extra_java.stderr)
            );
            assert_eq!(
                extra_java.stdout, java.stdout,
                "extra evaluation leaves the original runtime result unchanged"
            );
            eprintln!(
                "two-local-extra-counter-clone verifier=passed runtime=passed; added sequence at empty stack before Pair NEW, max_stack=4"
            );
            let extra_artifacts = JvmTastyBatchArtifacts {
                classfiles: batch
                    .classes()
                    .iter()
                    .map(|artifact| JvmTastyClassFileArtifact {
                        classfile_path: artifact.classfile_path().to_owned(),
                        classfile_bytes: if artifact.classfile_path() == "example/PairFactory.class"
                        {
                            extra_counter.clone()
                        } else {
                            artifact.classfile_bytes().to_vec()
                        },
                    })
                    .collect(),
                sidecars: batch
                    .classes()
                    .iter()
                    .map(|artifact| JvmTastySidecarArtifact {
                        tasty_path: artifact.tasty_path().to_owned(),
                        tasty_bytes: artifact.tasty_bytes().to_vec(),
                    })
                    .collect(),
            };
            with_paired_jvm_tasty_batch_plan(TWO_LOCAL_PRIMARY_BATCH, "PairFactory.scala", SourceId::from_index(21280), |plan| {
                let tasty = plan.classes.iter().map(|class| class.tasty_plan).collect::<Vec<_>>();
                let validated = validate_jvm_tasty_batch(plan.jvm_unit, &tasty).unwrap();
                let error = validate_jvm_tasty_batch_artifacts(&validated, extra_artifacts)
                    .expect_err("paired Rust contract rejects extra Counter allocation and constructor");
                let JvmTastyContractError::BatchArtifact { phase, reason, .. } = &error else {
                    panic!("extra evaluation must fail in the authenticated bytecode proof: {error:?}");
                };
                assert_eq!(*phase, "cross-class invocation");
                assert!(
                    reason.contains("expected aload_3 in bounded call bytecode")
                        && reason.contains("found opcode 0x1b"),
                    "rejection must identify the extra Counter constructor seed load where the Pair argument load was expected: {error:?}"
                );
                eprintln!("two-local-negative extra-counter-allocation-and-constructor first-stage=Rust paired contract phase={phase} reason={reason:?}");
            }).expect("authenticate source session for extra Counter control");

            let mut same_local = factory.to_vec();
            same_local[primary_code_start + load3..primary_code_start + load4 + 2]
                .copy_from_slice(&[0x2d, 0x2d, 0x00]);
            run_negative(
                "same-local-loaded-twice",
                Some(&same_local),
                None,
                "AssertionError",
            );

            let mut wrong_slot_store = factory.to_vec();
            wrong_slot_store[primary_code_start + store4 + 1] = 3;
            run_negative(
                "second-local-stored-to-slot-three",
                Some(&wrong_slot_store),
                None,
                "VerifyError",
            );
            let mut missing_second_store = factory.to_vec();
            missing_second_store[primary_code_start + store4..primary_code_start + store4 + 2]
                .copy_from_slice(&[0x00, 0x00]);
            run_negative(
                "missing-second-local-store",
                Some(&missing_second_store),
                None,
                "VerifyError",
            );
            let second_new = primary_instructions
                .iter()
                .filter(|i| i.opcode == 0xbb)
                .nth(1)
                .unwrap()
                .pc;
            let mut missing_second_new = factory.to_vec();
            missing_second_new
                [primary_code_start + second_new..primary_code_start + second_new + 3]
                .copy_from_slice(&[0x00, 0x00, 0x00]);
            run_negative(
                "missing-second-counter-allocation",
                Some(&missing_second_new),
                None,
                "VerifyError",
            );
            let second_counter_init = primary_instructions
                .iter()
                .filter(|i| {
                    i.opcode == 0xb7
                        && batch_method_ref(&class, i.operand.unwrap()).0 == "example/Counter"
                })
                .nth(1)
                .unwrap()
                .pc;
            let mut missing_second_init = factory.to_vec();
            missing_second_init[primary_code_start + second_counter_init
                ..primary_code_start + second_counter_init + 3]
                .copy_from_slice(&[0x00, 0x00, 0x00]);
            run_negative(
                "missing-second-counter-constructor",
                Some(&missing_second_init),
                None,
                "VerifyError",
            );

            let load_before_store = {
                let mut bytes = factory.to_vec();
                let load4_pc = primary_instructions
                    .iter()
                    .find(|i| i.opcode == 0x19)
                    .unwrap()
                    .pc;
                let load4_bytes = bytes
                    [primary_code_start + load4_pc..primary_code_start + load4_pc + 2]
                    .to_vec();
                let store4_bytes =
                    bytes[primary_code_start + store4..primary_code_start + store4 + 2].to_vec();
                bytes[primary_code_start + load4_pc..primary_code_start + load4_pc + 2]
                    .copy_from_slice(&store4_bytes);
                bytes[primary_code_start + store4..primary_code_start + store4 + 2]
                    .copy_from_slice(&load4_bytes);
                bytes
            };
            run_negative(
                "load-before-second-local-store",
                Some(&load_before_store),
                None,
                "VerifyError",
            );

            let mut uninitialized_escape = factory.to_vec();
            uninitialized_escape[primary_code_start + second_new..primary_code_start + store4 + 2]
                .copy_from_slice(&[
                    0xbb,
                    (counter_new_ref.0 >> 8) as u8,
                    counter_new_ref.0 as u8,
                    0x3a,
                    4,
                    0,
                    0,
                    0,
                    0,
                    0,
                ]);
            run_negative(
                "uninitialized-counter-escapes-into-pair",
                Some(&uninitialized_escape),
                None,
                "VerifyError",
            );

            let mut wrong_pair_owner = factory.to_vec();
            let pair_init_pc = primary_instructions
                .iter()
                .find(|i| {
                    i.opcode == 0xb7
                        && batch_method_ref(&class, i.operand.unwrap()).0 == "example/Pair"
                })
                .unwrap()
                .pc;
            let counter_init_ref = primary_instructions
                .iter()
                .find(|i| {
                    i.opcode == 0xb7
                        && batch_method_ref(&class, i.operand.unwrap()).0 == "example/Counter"
                })
                .unwrap()
                .operand
                .unwrap();
            wrong_pair_owner
                [primary_code_start + pair_init_pc + 1..primary_code_start + pair_init_pc + 3]
                .copy_from_slice(&counter_init_ref.0.to_be_bytes());
            run_negative(
                "wrong-pair-constructor-reference",
                Some(&wrong_pair_owner),
                None,
                "VerifyError",
            );
            let mut wrong_pair_descriptor = factory.to_vec();
            let pair_methodref = primary_instructions
                .iter()
                .find(|i| {
                    i.opcode == 0xb7
                        && batch_method_ref(&class, i.operand.unwrap()).0 == "example/Pair"
                })
                .unwrap()
                .operand
                .unwrap();
            let name_and_type = match class.constant_pool.get(pair_methodref).unwrap() {
                ConstantPoolEntry::Methodref {
                    name_and_type_index,
                    ..
                } => *name_and_type_index,
                other => panic!("Pair constructor reference is a methodref: {other:?}"),
            };
            let wrong_descriptor = (1..=u16::from_be_bytes([factory[8], factory[9]]))
                .map(ConstantPoolIndex)
                .find(|index| {
                    class.constant_pool.get(*index)
                        == Some(&ConstantPoolEntry::Utf8("(I)V".to_owned()))
                })
                .expect("Counter constructor descriptor exists");
            let descriptor_offset = constant_pool_entry_offset(factory, name_and_type) + 3;
            wrong_pair_descriptor[descriptor_offset..descriptor_offset + 2]
                .copy_from_slice(&wrong_descriptor.0.to_be_bytes());
            run_negative(
                "wrong-pair-constructor-descriptor",
                Some(&wrong_pair_descriptor),
                None,
                "VerifyError",
            );

            let mut low_locals = factory.to_vec();
            low_locals[primary_code_start - 6..primary_code_start - 4]
                .copy_from_slice(&4u16.to_be_bytes());
            run_negative("max-locals-too-low", Some(&low_locals), None, "VerifyError");
            let mut low_stack = factory.to_vec();
            low_stack[primary_code_start - 8..primary_code_start - 6]
                .copy_from_slice(&3u16.to_be_bytes());
            run_negative("max-stack-too-low", Some(&low_stack), None, "VerifyError");
            for missing in [
                "example/Counter.class",
                "example/Pair.class",
                "example/PairFactory.class",
            ] {
                run_negative(
                    &format!("missing-{}", missing.rsplit('/').next().unwrap()),
                    None,
                    Some(missing),
                    "NoClassDefFoundError",
                );
            }
            let pair_artifact = batch
                .classes()
                .iter()
                .find(|artifact| artifact.classfile_path() == "example/Pair.class")
                .unwrap();
            let pair_class =
                ClassFile::decode(&mut Reader::new(pair_artifact.classfile_bytes())).unwrap();
            let cp_count = u16::from_be_bytes([
                pair_artifact.classfile_bytes()[8],
                pair_artifact.classfile_bytes()[9],
            ]);
            let method_name = (1..=cp_count)
                .map(ConstantPoolIndex)
                .find(|index| pair_class.constant_pool.utf8(*index).ok() == Some("leftValue"))
                .expect("Pair has leftValue UTF8 constant");
            let mut no_such_method = pair_artifact.classfile_bytes().to_vec();
            let name_offset = constant_pool_entry_offset(&no_such_method, method_name);
            assert_eq!(no_such_method[name_offset], 1);
            let name_len = usize::from(u16::from_be_bytes([
                no_such_method[name_offset + 1],
                no_such_method[name_offset + 2],
            ]));
            assert_eq!(
                name_len,
                "missingXX".len(),
                "replacement keeps the classfile layout stable"
            );
            no_such_method[name_offset + 3..name_offset + 3 + name_len]
                .copy_from_slice(b"missingXX");
            let root = scratch.path().join("negative-no-such-method");
            for artifact in batch.classes() {
                let destination = root.join(artifact.classfile_path());
                std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
                let bytes = if artifact.classfile_path() == "example/Pair.class" {
                    &no_such_method
                } else {
                    artifact.classfile_bytes()
                };
                std::fs::write(destination, bytes).unwrap();
            }
            let output = Command::new("java")
                .args(["-Xverify:all", "-ea", "-classpath"])
                .arg(std::env::join_paths([harness_classes.as_path(), root.as_path()]).unwrap())
                .arg("example.PairFactoryTwoLocalIdentityHarness")
                .arg("primary")
                .output()
                .expect("run isolated missing-method classfile control");
            let linkage = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                !output.status.success(),
                "missing method unexpectedly linked"
            );
            assert!(
                linkage.contains("NoSuchMethodError"),
                "expected method-resolution failure: {linkage}"
            );
            eprintln!(
                "two-local-negative missing-leftValue-method first-stage=JVM linkage NoSuchMethodError"
            );
        }
        for (path, relative, before) in &hashes_before {
            assert_eq!(
                sha256sum(path),
                *before,
                "original {label} {relative} hash changed after javac/runtime"
            );
        }
        eprintln!("two-local-runtime {expected}; six original artifact hashes unchanged");
    }
}

#[test]
fn immutable_reference_local_pair_executes_original_rust_classes_on_strict_jdk21() {
    if !require_jdk_21_or_skip() {
        return;
    }

    let scratch = TestDirectory::new("immutable-reference-local-pair-runtime");
    let harness_source = scratch.path().join("PairFactoryIdentityHarness.java");
    let harness_classes = scratch.path().join("harness-classes");
    std::fs::create_dir(&harness_classes).unwrap();
    std::fs::write(&harness_source, IMMUTABLE_LOCAL_PAIR_IDENTITY_HARNESS).unwrap();
    eprintln!(
        "immutable-local-pair-harness-sha256 {}",
        sha256sum(&harness_source)
    );

    let variants = [
        (IMMUTABLE_LOCAL_PAIR_BATCH, "create", false, 20741),
        (
            IMMUTABLE_LOCAL_PAIR_REVERSED_BATCH,
            "createReversed",
            true,
            20742,
        ),
    ];
    let mut originals = Vec::new();
    for (source, method_name, reversed, source_index) in variants {
        let batch = emit_paired_jvm_tasty_batch(
            source,
            "PairFactory.scala",
            SourceId::from_index(source_index),
        )
        .expect("one authentic Rust frontend session emits the frozen six-file batch");
        assert_eq!(batch.classes().len(), 3);
        let original = scratch.path().join(format!("original-{method_name}"));
        let runtime_classes = scratch.path().join(format!("runtime-{method_name}"));
        std::fs::create_dir_all(&original).unwrap();
        std::fs::create_dir_all(&runtime_classes).unwrap();
        let mut paths = Vec::new();
        for artifact in batch.classes() {
            let mut reader = Reader::new(artifact.classfile_bytes());
            let decoded = ClassFile::decode(&mut reader).unwrap();
            assert!(reader.is_at_end());
            assert_eq!((decoded.version.major, decoded.version.minor), (65, 0));
            assert_eq!(
                decoded.tasty_link_id().unwrap(),
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
            for (relative, bytes) in [
                (artifact.classfile_path(), artifact.classfile_bytes()),
                (artifact.tasty_path(), artifact.tasty_bytes()),
            ] {
                let path = original.join(relative);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, bytes).unwrap();
                paths.push((path, relative.to_owned(), bytes.to_vec()));
                eprintln!(
                    "immutable-local-pair-original {method_name} {relative} sha256={} tasty-link-id={:?}",
                    sha256sum_bytes(bytes),
                    artifact.tasty_link_id()
                );
            }
            let runtime_path = runtime_classes.join(artifact.classfile_path());
            std::fs::create_dir_all(runtime_path.parent().unwrap()).unwrap();
            std::fs::write(runtime_path, artifact.classfile_bytes()).unwrap();
        }
        assert_eq!(paths.len(), 6);

        let factory = mixed_factory_bytes(&batch);
        let (class, instructions, sequence, descriptor, code_start, _) =
            mixed_factory_code(factory, method_name);
        assert_eq!(descriptor, "(Lexample/Counter;I)Lexample/Pair;");
        let method = class
            .methods
            .iter()
            .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == method_name)
            .unwrap();
        let [Attribute::Code(code)] = method.attributes.as_slice() else {
            panic!("{method_name} has one Code attribute")
        };
        assert_eq!((code.max_stack, code.max_locals), (4, 4));

        let allocations = instructions
            .iter()
            .filter(|instruction| instruction.opcode == 0xbb)
            .map(|instruction| {
                (
                    instruction.pc,
                    batch_class_name(&class, instruction.operand.unwrap()).to_owned(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            allocations
                .iter()
                .map(|(_, name)| name.as_str())
                .collect::<Vec<_>>(),
            ["example/Counter", "example/Pair"]
        );
        let calls = instructions
            .iter()
            .filter(|instruction| instruction.opcode == 0xb7)
            .map(|instruction| {
                (
                    instruction.pc,
                    batch_method_ref(&class, instruction.operand.unwrap()),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].1, ("example/Counter", ("<init>", "(I)V")));
        assert_eq!(
            calls[1].1,
            (
                "example/Pair",
                ("<init>", "(Lexample/Counter;Lexample/Counter;)V")
            )
        );
        let counter_new_pc = allocations[0].0;
        let counter_init_pc = calls[0].0;
        let pair_new_pc = allocations[1].0;
        let pair_init_pc = calls[1].0;
        let store = instructions
            .iter()
            .find(|instruction| instruction.opcode == 0x4e)
            .expect("fresh Counter is stored to local slot 3");
        let loads = instructions
            .iter()
            .filter(|instruction| matches!(instruction.opcode, 0x2b | 0x2d))
            .map(|instruction| (instruction.pc, instruction.opcode))
            .collect::<Vec<_>>();
        assert_eq!(store.pc, counter_init_pc + 3);
        assert!(pair_new_pc > store.pc, "local store follows Counter init");
        let expected_loads = if reversed { [0x2d, 0x2b] } else { [0x2b, 0x2d] };
        assert_eq!(
            [loads[0].1, loads[1].1],
            expected_loads,
            "Pair arguments use the source's formal/local order"
        );
        assert!(loads[1].0 > store.pc, "fresh load follows its store");
        assert!(pair_init_pc > loads[1].0);
        assert_eq!(
            instructions.last().map(|instruction| instruction.opcode),
            Some(0xb0)
        );

        let graph = ControlFlowGraph::build(&sequence).expect("original method CFG decodes");
        let analysis = CfgCodeAnalyzer::analyze_with_current_class(
            &graph,
            method_name,
            &descriptor,
            false,
            &class.constant_pool,
            class.this_class,
        )
        .expect("CFG proves local initialization before the Pair constructor use");
        assert_eq!((analysis.max_stack, analysis.max_locals), (4, 4));
        eprintln!(
            "immutable-local-pair-flow {method_name}{descriptor} this=0 existing=1 seed=2 fresh=3 counter_new_pc={counter_new_pc} counter_init_pc={counter_init_pc} astore_3_pc={} pair_new_pc={pair_new_pc} pair_init_pc={pair_init_pc} loads={loads:?} stack={} locals={}",
            store.pc, analysis.max_stack, analysis.max_locals
        );

        let classpath = runtime_classes.clone();
        let javap = Command::new("javap")
            .args(["-v", "-p", "-c", "-classpath"])
            .arg(&runtime_classes)
            .arg("example.PairFactory")
            .output()
            .expect("run javap on original Rust classfile");
        assert!(
            javap.status.success(),
            "javap failed: {}",
            String::from_utf8_lossy(&javap.stderr)
        );
        let dump = String::from_utf8_lossy(&javap.stdout);
        for expected in [
            "major version: 65",
            "descriptor: (Lexample/Counter;I)Lexample/Pair;",
            "stack=4, locals=4",
            "astore_3",
            "aload_1",
            "aload_3",
            "example/Counter.\"<init>\":(I)V",
            "example/Pair.\"<init>\":(Lexample/Counter;Lexample/Counter;)V",
        ] {
            assert!(
                dump.contains(expected),
                "javap omitted {expected:?}:\n{dump}"
            );
        }
        let javac = Command::new("javac")
            .args(["--release", "21", "-classpath"])
            .arg(&classpath)
            .arg("-d")
            .arg(&harness_classes)
            .arg(&harness_source)
            .output()
            .expect("run javac --release 21 for independent Java client");
        assert!(
            javac.status.success(),
            "javac failed: {}{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        for (path, relative, bytes) in &paths {
            assert_eq!(
                std::fs::read(path).unwrap(),
                *bytes,
                "javac changed {relative}"
            );
        }
        let java = Command::new("java")
            .args(["-Xverify:all", "-ea", "-classpath"])
            .arg(
                std::env::join_paths([harness_classes.as_path(), runtime_classes.as_path()])
                    .unwrap(),
            )
            .arg("example.PairFactoryIdentityHarness")
            .arg(if reversed { "reversed" } else { "primary" })
            .output()
            .expect("run original Rust producer classes with strict JVM verification");
        assert!(
            java.status.success(),
            "java -Xverify:all failed for {method_name}: {}{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        let expected = if reversed {
            "reversed=identity:true;mutations=isolated;repeat=fresh;seeds=0,-5"
        } else {
            "primary=identity:true;mutations=isolated;repeat=fresh;seeds=0,-5"
        };
        assert_eq!(String::from_utf8_lossy(&java.stdout).trim(), expected);
        eprintln!("immutable-local-pair-runtime {expected}");
        for (path, relative, bytes) in &paths {
            assert_eq!(
                std::fs::read(path).unwrap(),
                *bytes,
                "runtime changed {relative}"
            );
            assert_eq!(
                sha256sum(path),
                sha256sum_bytes(bytes),
                "runtime hash {relative}"
            );
        }
        let factory_copy = factory.to_vec();
        originals.push((
            batch,
            method_name,
            paths,
            factory_copy,
            code_start,
            reversed,
        ));
    }

    // Swap the two same-typed argument loads in an isolated forward copy. The JVM accepts this
    // bytecode, so only the identity assertion should detect that the existing object moved.
    let (batch, _, paths, factory, code_start, _) = &originals[0];
    let (_, instructions, _, _, _, _) = mixed_factory_code(factory, "create");
    let argument_loads = instructions
        .iter()
        .filter(|instruction| matches!(instruction.opcode, 0x2b | 0x2d))
        .collect::<Vec<_>>();
    let mut swapped = factory.clone();
    swapped.swap(
        code_start + argument_loads[0].pc,
        code_start + argument_loads[1].pc,
    );
    let corrupt = scratch.path().join("primary-swapped-arguments");
    for artifact in batch.classes() {
        let destination = corrupt.join(artifact.classfile_path());
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        let bytes = if artifact.classfile_path() == "example/PairFactory.class" {
            swapped.as_slice()
        } else {
            artifact.classfile_bytes()
        };
        std::fs::write(destination, bytes).unwrap();
    }
    let output = Command::new("java")
        .args(["-Xverify:all", "-ea", "-classpath"])
        .arg(std::env::join_paths([harness_classes.as_path(), corrupt.as_path()]).unwrap())
        .arg("example.PairFactoryIdentityHarness")
        .arg("primary")
        .output()
        .expect("execute isolated verifier-valid same-type swap");
    let output_text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "same-type swap unexpectedly passed"
    );
    assert!(
        output_text.contains("AssertionError: caller Counter identity moved"),
        "same-type swap must fail the identity assertion rather than JVM verification:\n{output_text}"
    );
    assert!(
        !output_text.contains("VerifyError"),
        "swap is JVM-valid: {output_text}"
    );
    eprintln!(
        "immutable-local-pair-negative swapped-pair-arguments first-stage=runtime-identity: {}",
        output_text
            .lines()
            .find(|line| line.contains("AssertionError"))
            .unwrap()
    );

    let (_, instructions, _, _, _, _) = mixed_factory_code(factory, "create");
    let pc = |opcode: u8| {
        instructions
            .iter()
            .find(|instruction| instruction.opcode == opcode)
            .expect("expected opcode exists in original method")
            .pc
    };
    let store_pc = pc(0x4e);
    let counter_new_pc = instructions
        .iter()
        .find(|instruction| {
            instruction.opcode == 0xbb
                && batch_class_name(
                    &ClassFile::decode(&mut Reader::new(factory)).unwrap(),
                    instruction.operand.unwrap(),
                ) == "example/Counter"
        })
        .unwrap()
        .pc;
    let counter_init_pc = instructions
        .iter()
        .find(|instruction| {
            instruction.opcode == 0xb7
                && batch_method_ref(
                    &ClassFile::decode(&mut Reader::new(factory)).unwrap(),
                    instruction.operand.unwrap(),
                )
                .0 == "example/Counter"
        })
        .unwrap()
        .pc;
    let pair_init_pc = instructions
        .iter()
        .find(|instruction| {
            instruction.opcode == 0xb7
                && batch_method_ref(
                    &ClassFile::decode(&mut Reader::new(factory)).unwrap(),
                    instruction.operand.unwrap(),
                )
                .0 == "example/Pair"
        })
        .unwrap()
        .pc;
    let fresh_load_pc = instructions
        .iter()
        .find(|instruction| instruction.opcode == 0x2d)
        .unwrap()
        .pc;
    let counter_init_ref = instructions
        .iter()
        .find(|instruction| instruction.pc == counter_init_pc)
        .unwrap()
        .operand
        .unwrap();
    let pair_init_ref = instructions
        .iter()
        .find(|instruction| instruction.pc == pair_init_pc)
        .unwrap()
        .operand
        .unwrap();
    let class = ClassFile::decode(&mut Reader::new(factory)).unwrap();
    let assert_negative =
        |label: &str, changed: Option<&[u8]>, omit: Option<&str>, expected: &str| {
            let root = scratch.path().join(format!("primary-{label}"));
            for artifact in batch.classes() {
                if omit == Some(artifact.classfile_path()) {
                    continue;
                }
                let destination = root.join(artifact.classfile_path());
                std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
                let bytes = if artifact.classfile_path() == "example/PairFactory.class" {
                    changed.unwrap_or(factory)
                } else {
                    artifact.classfile_bytes()
                };
                std::fs::write(destination, bytes).unwrap();
            }
            let output = Command::new("java")
                .args(["-Xverify:all", "-ea", "-classpath"])
                .arg(std::env::join_paths([harness_classes.as_path(), root.as_path()]).unwrap())
                .arg("example.PairFactoryIdentityHarness")
                .arg("primary")
                .output()
                .expect("run isolated corruption control");
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(!output.status.success(), "{label} unexpectedly passed");
            assert!(
                text.contains(expected),
                "{label} first failure expected {expected:?}:\n{text}"
            );
            eprintln!(
                "immutable-local-pair-negative {label} first-stage={} evidence={}",
                if expected.starts_with("AssertionError") {
                    "runtime identity assertion"
                } else if expected.contains("NoClassDefFoundError") {
                    "JVM linkage"
                } else if expected.contains("ClassFormatError") {
                    "JVM classfile loading"
                } else {
                    "JVM verifier"
                },
                text.lines()
                    .find(|line| line.contains(expected.split(':').next().unwrap()))
                    .unwrap_or(expected)
            );
            text
        };

    let insert_code = |at: usize, insertion: &[u8]| {
        let code_start = *code_start;
        let mut bytes = factory.clone();
        let old_code_len =
            u32::from_be_bytes(bytes[code_start - 4..code_start].try_into().unwrap());
        let old_attribute_len =
            u32::from_be_bytes(bytes[code_start - 12..code_start - 8].try_into().unwrap());
        bytes.splice(code_start + at..code_start + at, insertion.iter().copied());
        bytes[code_start - 4..code_start]
            .copy_from_slice(&(old_code_len + insertion.len() as u32).to_be_bytes());
        bytes[code_start - 12..code_start - 8]
            .copy_from_slice(&(old_attribute_len + insertion.len() as u32).to_be_bytes());
        bytes
    };

    let mut wrong_store = factory.clone();
    wrong_store[code_start + store_pc] = 0x4c; // astore_1 overwrites existing; slot 3 stays uninitialized.
    let (_, wrong_store_instructions, _, _, _, _) = mixed_factory_code(&wrong_store, "create");
    assert!(
        wrong_store_instructions
            .iter()
            .any(|instruction| { instruction.pc == store_pc && instruction.opcode == 0x4c })
    );
    let wrong_store_output = assert_negative(
        "wrong-reference-astore-slot",
        Some(&wrong_store),
        None,
        "VerifyError",
    );
    assert!(
        wrong_store_output.contains("locals[3]"),
        "wrong reference store must be rejected when the uninitialized fresh local is loaded:\n{wrong_store_output}"
    );

    let mut load_before_store = factory.clone();
    load_before_store.swap(code_start + store_pc, code_start + fresh_load_pc);
    assert_negative(
        "load-before-store",
        Some(&load_before_store),
        None,
        "VerifyError",
    );

    let mut existing_twice = factory.clone();
    existing_twice[code_start + fresh_load_pc] = 0x2b;
    assert_negative(
        "existing-loaded-twice",
        Some(&existing_twice),
        None,
        "AssertionError: fresh local aliases the caller Counter",
    );

    let mut wrong_aload = factory.clone();
    wrong_aload[code_start + fresh_load_pc] = 0x2a; // aload_0 is the PairFactory receiver.
    assert_negative("wrong-aload-index", Some(&wrong_aload), None, "VerifyError");

    let mut omitted_new = factory.clone();
    omitted_new[code_start + counter_new_pc..code_start + counter_new_pc + 3].fill(0);
    assert_negative(
        "missing-counter-new",
        Some(&omitted_new),
        None,
        "VerifyError",
    );

    let mut omitted_init = factory.clone();
    omitted_init[code_start + counter_init_pc..code_start + counter_init_pc + 3].fill(0);
    assert_negative(
        "missing-counter-init",
        Some(&omitted_init),
        None,
        "VerifyError",
    );

    let seed_load_pc = instructions
        .iter()
        .find(|instruction| instruction.opcode == 0x1c)
        .unwrap()
        .pc;
    let mut uninitialized_reference_escape = factory.clone();
    uninitialized_reference_escape[code_start + seed_load_pc] = 0x57; // pop duplicate, leaving the uninitialized Counter
    uninitialized_reference_escape[code_start + counter_init_pc..code_start + counter_init_pc + 3]
        .fill(0);
    let uninitialized_escape_output = assert_negative(
        "uninitialized-counter-escapes-to-pair",
        Some(&uninitialized_reference_escape),
        None,
        "VerifyError",
    );
    assert!(
        uninitialized_escape_output
            .to_ascii_lowercase()
            .contains("uninitialized"),
        "Pair use must be rejected because the Counter reference remains uninitialized:\n{uninitialized_escape_output}"
    );

    let counter_class_ref = instructions
        .iter()
        .find(|instruction| instruction.opcode == 0xbb)
        .unwrap()
        .operand
        .unwrap();
    let counter_cp = counter_class_ref.0.to_be_bytes();
    let counter_init_cp = counter_init_ref.0.to_be_bytes();
    let duplicate_counter_allocation = [
        0xbb,
        counter_cp[0],
        counter_cp[1],
        0x59,
        0x1c,
        0xb7,
        counter_init_cp[0],
        counter_init_cp[1],
    ];
    let duplicate_allocation = insert_code(pair_init_pc - 6, &duplicate_counter_allocation);
    assert_negative(
        "duplicate-counter-allocation",
        Some(&duplicate_allocation),
        None,
        "VerifyError",
    );

    let duplicate_counter_init = [0xb7, counter_init_cp[0], counter_init_cp[1]];
    let duplicate_init = insert_code(counter_init_pc + 3, &duplicate_counter_init);
    assert_negative(
        "duplicate-counter-constructor",
        Some(&duplicate_init),
        None,
        "VerifyError",
    );

    let mut wrong_owner = factory.clone();
    wrong_owner[code_start + pair_init_pc + 1..code_start + pair_init_pc + 3]
        .copy_from_slice(&counter_init_ref.0.to_be_bytes());
    assert_negative(
        "wrong-pair-constructor-owner",
        Some(&wrong_owner),
        None,
        "VerifyError",
    );

    let mut wrong_descriptor = factory.clone();
    let ConstantPoolEntry::Methodref {
        name_and_type_index,
        ..
    } = class.constant_pool.get(pair_init_ref).unwrap()
    else {
        panic!("Pair constructor is a method reference")
    };
    let ConstantPoolEntry::NameAndType {
        descriptor_index, ..
    } = class.constant_pool.get(*name_and_type_index).unwrap()
    else {
        panic!("Pair constructor method reference names a descriptor")
    };
    let wrong_descriptor_index = (1..=u16::from_be_bytes([factory[8], factory[9]]))
        .map(ConstantPoolIndex)
        .find(|index| {
            class.constant_pool.get(*index) == Some(&ConstantPoolEntry::Utf8("(I)V".to_owned()))
        })
        .expect("Counter constructor descriptor exists in the original pool");
    let descriptor_offset = constant_pool_entry_offset(factory, *name_and_type_index) + 3;
    assert_eq!(
        u16::from_be_bytes([factory[descriptor_offset], factory[descriptor_offset + 1]]),
        descriptor_index.0
    );
    wrong_descriptor[descriptor_offset..descriptor_offset + 2]
        .copy_from_slice(&wrong_descriptor_index.0.to_be_bytes());
    assert_negative(
        "wrong-pair-constructor-descriptor",
        Some(&wrong_descriptor),
        None,
        "VerifyError",
    );

    let mut pair_init_before_counter_init = factory.clone();
    pair_init_before_counter_init
        [code_start + counter_init_pc + 1..code_start + counter_init_pc + 3]
        .copy_from_slice(&pair_init_ref.0.to_be_bytes());
    assert_negative(
        "pair-constructor-before-counter-init",
        Some(&pair_init_before_counter_init),
        None,
        "VerifyError",
    );

    let code_start_from_class = mixed_code_bytes(factory, "create").0;
    let mut low_stack = factory.clone();
    low_stack[code_start_from_class - 8..code_start_from_class - 6]
        .copy_from_slice(&1u16.to_be_bytes());
    assert_negative("low-max-stack", Some(&low_stack), None, "VerifyError");
    let mut low_locals = factory.clone();
    low_locals[code_start_from_class - 6..code_start_from_class - 4]
        .copy_from_slice(&3u16.to_be_bytes());
    assert_negative("low-max-locals", Some(&low_locals), None, "VerifyError");

    assert_negative(
        "missing-counter-class",
        None,
        Some("example/Counter.class"),
        "NoClassDefFoundError",
    );
    assert_negative(
        "missing-pair-class",
        None,
        Some("example/Pair.class"),
        "NoClassDefFoundError",
    );

    for (path, relative, bytes) in paths {
        assert_eq!(
            std::fs::read(path).unwrap(),
            *bytes,
            "corruption changed original {relative}"
        );
    }
}
