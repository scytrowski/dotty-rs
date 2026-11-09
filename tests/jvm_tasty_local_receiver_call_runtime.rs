use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_batch;
use dotty_classfile::attribute::Attribute;
use dotty_classfile::classfile::{ClassFile, Reader};
use dotty_classfile::constant_pool::{ConstantPoolEntry, ConstantPoolIndex};
use dotty_core::SourceId;
use dotty_tasty::tasty::TastyFile;
use std::path::{Path, PathBuf};
use std::process::Command;

const SOURCE: &str = include_str!("fixtures/jvm_tasty_local_receiver_call/LocalReceiverCall.scala");
const TWO_CALL_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_ordered_local_unit_calls/OrderedLocalUnitCalls.scala");
const TWO_CALL_HARNESS: &str =
    include_str!("fixtures/jvm_tasty_ordered_local_unit_calls/RuntimeProbe.java");
const HARNESS: &str = r#"package example;

import java.lang.reflect.Field;

public final class JavaConsumer {
  private static Counter field(Pair pair, String name) throws Exception {
    Field field = Pair.class.getDeclaredField(name);
    field.setAccessible(true);
    return (Counter) field.get(pair);
  }

  private static void check(Pair pair, int expected, String label) throws Exception {
    if (pair.leftValue() != expected || pair.rightValue() != expected)
      throw new AssertionError(label + " expected " + expected + "/" + expected + " got " + pair.leftValue() + "/" + pair.rightValue());
    if (field(pair, "left") == field(pair, "right"))
      throw new AssertionError(label + " collapsed distinct Counter identities");
  }

  public static void main(String[] args) throws Exception {
    PairFactory factory = new PairFactory();
    Pair first = factory.create(7, 3);
    check(first, 3, "create(7,3)");
    Counter left = field(first, "left");
    Counter right = field(first, "right");
    left.set(31);
    if (first.leftValue() != 31 || first.rightValue() != 3)
      throw new AssertionError("left mutation affected the wrong Counter");
    right.set(-8);
    if (first.leftValue() != 31 || first.rightValue() != -8)
      throw new AssertionError("right mutation affected the wrong Counter");

    Pair second = factory.create(-9, 42);
    check(second, 42, "create(-9,42)");
    if (field(first, "left") == field(second, "left") || field(first, "right") == field(second, "right"))
      throw new AssertionError("repeated create reused a Counter");
    Pair equal = factory.create(7, 7);
    check(equal, 7, "create(7,7)");
    if (field(equal, "left") == field(equal, "right"))
      throw new AssertionError("equal seed values collapsed identity");
    System.out.println("local-call-runtime=3/3,42/42,7/7;identity=distinct;mutations=independent;repeat=fresh");
  }
}
"#;

#[derive(Debug, Clone, Copy)]
struct Insn {
    pc: usize,
    opcode: u8,
    len: usize,
    local: Option<u8>,
    cp: Option<u16>,
}

struct PrimaryRuntimeBatch {
    originals: Vec<(String, Vec<u8>)>,
    runtime: PathBuf,
    first_local_slot: u8,
}

fn method_code<'a>(class: &'a ClassFile<'a>, name: &str) -> (&'a [u8], u16, u16) {
    let method = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index).ok() == Some(name))
        .expect("PairFactory.create method");
    assert_eq!(
        class.constant_pool.utf8(method.descriptor_index),
        Ok("(II)Lexample/Pair;")
    );
    let [Attribute::Code(code)] = method.attributes.as_slice() else {
        panic!("create Code")
    };
    (code.code, code.max_stack, code.max_locals)
}

fn decode(code: &[u8]) -> Vec<Insn> {
    let mut result = Vec::new();
    let mut pc = 0;
    while pc < code.len() {
        let start = pc;
        let opcode = code[pc];
        pc += 1;
        let mut local = None;
        let mut cp = None;
        match opcode {
            0x15 | 0x19 | 0x3a => {
                local = Some(code[pc]);
                pc += 1;
            }
            0xbb | 0xb7 | 0xb6 => {
                cp = Some(u16::from_be_bytes([code[pc], code[pc + 1]]));
                pc += 2;
            }
            0x00 | 0x59 | 0x1b..=0x1e | 0x2a..=0x2d | 0x4b..=0x4e | 0xb0 => {}
            other => panic!("unexpected bytecode {other:#04x} at {start}"),
        }
        result.push(Insn {
            pc: start,
            opcode,
            len: pc - start,
            local,
            cp,
        });
    }
    result
}

fn aload_slot(insn: &Insn) -> Option<u8> {
    match insn.opcode {
        0x19 => insn.local,
        0x2b => Some(1),
        0x2c => Some(2),
        0x2d => Some(3),
        _ => None,
    }
}

fn iload_slot(insn: &Insn) -> Option<u8> {
    match insn.opcode {
        0x15 => insn.local,
        0x1b..=0x1e => Some(insn.opcode - 0x1a),
        _ => None,
    }
}

fn astore_slot(insn: &Insn) -> Option<u8> {
    match insn.opcode {
        0x3a => insn.local,
        0x4b..=0x4e => Some(insn.opcode - 0x4b),
        _ => None,
    }
}

fn utf8<'a>(class: &'a ClassFile<'a>, index: ConstantPoolIndex) -> &'a str {
    class.constant_pool.utf8(index).expect("constant pool UTF8")
}

fn class_name<'a>(class: &'a ClassFile<'a>, index: ConstantPoolIndex) -> &'a str {
    class
        .constant_pool
        .class_name(index)
        .expect("constant pool class")
}

fn method_ref<'a>(
    class: &'a ClassFile<'a>,
    index: ConstantPoolIndex,
) -> (&'a str, &'a str, &'a str) {
    let ConstantPoolEntry::Methodref {
        class_index,
        name_and_type_index,
    } = class.constant_pool.get(index).expect("methodref")
    else {
        panic!("invoke target must use CONSTANT_Methodref")
    };
    let owner = class_name(class, *class_index);
    let ConstantPoolEntry::NameAndType {
        name_index,
        descriptor_index,
    } = class
        .constant_pool
        .get(*name_and_type_index)
        .expect("name and type")
    else {
        panic!("methodref must have NameAndType")
    };
    (
        owner,
        utf8(class, *name_index),
        utf8(class, *descriptor_index),
    )
}

fn artifacts(
    batch: &dotty::jvm_tasty_artifacts::PairedJvmTastyBatchArtifacts,
) -> Vec<(String, Vec<u8>)> {
    let mut result = Vec::new();
    for artifact in batch.classes() {
        assert_eq!(
            (artifact.classfile_path(), artifact.tasty_path()),
            match artifact.classfile_path() {
                "example/Counter.class" => ("example/Counter.class", "example/Counter.tasty"),
                "example/Pair.class" => ("example/Pair.class", "example/Pair.tasty"),
                "example/PairFactory.class" =>
                    ("example/PairFactory.class", "example/PairFactory.tasty"),
                other => panic!("unexpected class artifact {other}"),
            }
        );
        let mut reader = Reader::new(artifact.classfile_bytes());
        let class = ClassFile::decode(&mut reader).expect("decode emitted class");
        assert!(reader.is_at_end());
        assert_eq!((class.version.major, class.version.minor), (65, 0));
        assert_eq!(
            class.tasty_link_id().unwrap(),
            Some(artifact.tasty_link_id())
        );
        let tasty = TastyFile::parse_and_validate_scala_3_9(artifact.tasty_bytes())
            .expect("parse linked Rust TASTy");
        assert_eq!(
            (
                tasty.header().major_version,
                tasty.header().minor_version,
                tasty.header().experimental_version
            ),
            (28, 9, 0)
        );
        result.push((
            artifact.classfile_path().to_owned(),
            artifact.classfile_bytes().to_vec(),
        ));
        result.push((
            artifact.tasty_path().to_owned(),
            artifact.tasty_bytes().to_vec(),
        ));
    }
    assert_eq!(result.len(), 6, "exactly six genuine Rust artifacts");
    result
}

fn require_jdk_21() -> bool {
    let missing = ["java", "javac", "javap"]
        .into_iter()
        .filter_map(|tool| {
            let output = Command::new(tool).arg("-version").output().ok()?;
            let version = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            (!output.status.success() || !version.contains("21.0."))
                .then_some(format!("{tool}: {version}"))
        })
        .collect::<Vec<_>>();
    let java = Command::new("java").arg("-version").output().ok();
    let temurin = java
        .map(|output| {
            format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
            .contains("Temurin")
        })
        .unwrap_or(false);
    let mut missing = missing;
    if !temurin {
        missing.push("java is not the pinned Temurin distribution".to_owned());
    }
    if missing.is_empty() && temurin {
        return true;
    }
    if std::env::var("DOTTY_CLASSFILE_REQUIRE_JDK").as_deref() == Ok("1") {
        panic!(
            "strict local receiver runtime requires pinned Temurin JDK 21; tools: {}; Temurin: {temurin}",
            missing.join("; ")
        );
    }
    eprintln!(
        "skipping optional local receiver JDK test: {}",
        missing.join("; ")
    );
    false
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "dotty-local-receiver-{}-{label}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap_or_else(|error| {
            panic!("create isolated test directory {}: {error}", path.display())
        });
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn java_tool(name: &str, tmpdir: &Path) -> Command {
    let mut command = Command::new(name);
    let mut options = std::env::var("JAVA_TOOL_OPTIONS").unwrap_or_default();
    if !options.is_empty() {
        options.push(' ');
    }
    options.push_str(&format!("-Djava.io.tmpdir={}", tmpdir.display()));
    command.env("JAVA_TOOL_OPTIONS", options);
    command
}

fn sha256(path: &std::path::Path) -> String {
    let output = Command::new("sha256sum")
        .arg(path)
        .output()
        .expect("sha256sum");
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned()
}

#[test]
fn local_counter_set_runs_on_unchanged_rust_batch_under_strict_jdk21() {
    if !require_jdk_21() {
        return;
    }
    let scratch = TestDirectory::new("runtime");
    let root = scratch.path();
    let harness_src = root.join("harness-src");
    let harness_classes = root.join("harness-classes");
    std::fs::create_dir_all(&harness_src).unwrap();
    std::fs::create_dir_all(&harness_classes).unwrap();
    let harness = harness_src.join("JavaConsumer.java");
    std::fs::write(&harness, HARNESS).unwrap();

    let reversed = SOURCE.replace(
        "    val first: Counter = new Counter(firstSeed)\n    val second: Counter = new Counter(secondSeed)",
        "    val second: Counter = new Counter(secondSeed)\n    val first: Counter = new Counter(firstSeed)",
    );
    assert_ne!(
        reversed, SOURCE,
        "declaration-reversal fixture must actually differ"
    );
    // Only reach the reversed-local variant after the primary frozen batch passes below.
    for (label, source, source_id, first_slot, second_slot) in [
        ("primary", SOURCE, 21681, 3u8, 4u8),
        ("reversed", reversed.as_str(), 21682, 4u8, 3u8),
    ] {
        let batch = emit_paired_jvm_tasty_batch(
            source,
            "LocalReceiverCall.scala",
            SourceId::from_index(source_id),
        )
        .expect("frozen single-session Rust source produces six linked artifacts");
        let originals = artifacts(&batch);
        let repeated = emit_paired_jvm_tasty_batch(
            source,
            "LocalReceiverCall.scala",
            SourceId::from_index(source_id),
        )
        .expect("a fresh source session emits the same six linked artifacts");
        assert_eq!(
            originals,
            artifacts(&repeated),
            "the {label} source variant is deterministic across fresh sessions"
        );
        for artifact in batch.classes() {
            let class_hash = sha256_bytes(artifact.classfile_bytes());
            let tasty_hash = sha256_bytes(artifact.tasty_bytes());
            let uuid = artifact
                .tasty_link_id()
                .as_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            eprintln!(
                "local-receiver-artifact variant={label} class={} sha256_class={class_hash} sha256_tasty={tasty_hash} tasty_uuid={uuid}",
                artifact.classfile_path()
            );
        }
        let original_hashes = originals
            .iter()
            .map(|(relative, bytes)| (relative.clone(), sha256_bytes(bytes)))
            .collect::<Vec<_>>();
        let runtime = root.join(format!("runtime-{label}"));
        std::fs::create_dir_all(&runtime).unwrap();
        let mut original_paths = Vec::new();
        for (relative, bytes) in &originals {
            let path = runtime.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, bytes).unwrap();
            original_paths.push((path, relative.clone(), bytes.clone()));
        }
        let factory = originals
            .iter()
            .find(|(path, _)| *path == "example/PairFactory.class")
            .unwrap()
            .1
            .as_slice();
        let class = ClassFile::decode(&mut Reader::new(factory)).expect("decode PairFactory");
        let (code, max_stack, max_locals) = method_code(&class, "create");
        assert_eq!((max_stack, max_locals), (4, 5));
        let insns = decode(code);
        let calls = insns
            .iter()
            .filter(|i| matches!(i.opcode, 0xb7 | 0xb6))
            .collect::<Vec<_>>();
        assert_eq!(
            calls.len(),
            4,
            "two Counter constructors, one set and Pair constructor"
        );
        assert_eq!(
            method_ref(&class, ConstantPoolIndex(calls[0].cp.unwrap())),
            ("example/Counter", "<init>", "(I)V")
        );
        assert_eq!(
            method_ref(&class, ConstantPoolIndex(calls[1].cp.unwrap())),
            ("example/Counter", "<init>", "(I)V")
        );
        assert_eq!(
            calls[2].opcode, 0xb6,
            "local Unit call must use invokevirtual"
        );
        assert_eq!(
            method_ref(&class, ConstantPoolIndex(calls[2].cp.unwrap())),
            ("example/Counter", "set", "(I)V")
        );
        assert_eq!(
            method_ref(&class, ConstantPoolIndex(calls[3].cp.unwrap())),
            (
                "example/Pair",
                "<init>",
                "(Lexample/Counter;Lexample/Counter;)V"
            )
        );
        let seed_load = insns
            .iter()
            .find(|i| i.pc + i.len == calls[2].pc)
            .expect("seed load immediately precedes call");
        assert_eq!(
            seed_load.opcode, 0x1c,
            "set argument is secondSeed, not a guessed slot"
        );
        let receiver_load = insns
            .iter()
            .find(|i| i.pc + i.len == seed_load.pc)
            .expect("receiver load immediately precedes seed");
        assert_eq!(aload_slot(receiver_load), Some(first_slot));
        assert!(
            calls[2].pc > calls[1].pc && calls[2].pc < calls[3].pc,
            "set follows both initialized local stores and precedes Pair construction"
        );
        let stores = insns
            .iter()
            .filter(|i| matches!(i.opcode, 0x4e | 0x3a))
            .collect::<Vec<_>>();
        assert_eq!(stores.len(), 2);
        let mut stored_slots = stores
            .iter()
            .map(|i| i.local.unwrap_or(if i.opcode == 0x4e { 3 } else { 0 }))
            .collect::<Vec<_>>();
        stored_slots.sort_unstable();
        assert_eq!(stored_slots, [3, 4]);
        let pair_new = insns
            .iter()
            .find(|i| {
                i.opcode == 0xbb
                    && class_name(&class, ConstantPoolIndex(i.cp.unwrap())) == "example/Pair"
            })
            .unwrap();
        assert!(calls[2].pc < pair_new.pc);
        let pair_loads = insns
            .iter()
            .filter(|i| i.pc > pair_new.pc && matches!(i.opcode, 0x19 | 0x2b | 0x2c | 0x2d))
            .collect::<Vec<_>>();
        assert_eq!(
            pair_loads.len(),
            2,
            "both local references are forwarded to Pair"
        );
        let pair_slots = pair_loads
            .iter()
            .map(|i| match i.opcode {
                0x19 => i.local.unwrap(),
                0x2b => 1,
                0x2c => 2,
                0x2d => 3,
                _ => unreachable!(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            pair_slots,
            [first_slot, second_slot],
            "Pair argument order follows semantic first/second locals"
        );
        let javap = java_tool("javap", root)
            .args(["-c", "-v", "-p", "-classpath"])
            .arg(&runtime)
            .arg("example.PairFactory")
            .output()
            .expect("javap Rust PairFactory");
        assert!(
            javap.status.success(),
            "javap: {}",
            String::from_utf8_lossy(&javap.stderr)
        );
        let dump = String::from_utf8_lossy(&javap.stdout);
        for evidence in [
            "major version: 65",
            "descriptor: (II)Lexample/Pair;",
            "stack=4, locals=5",
            "invokevirtual",
            "example/Counter.set:(I)V",
            "example/Pair.\"<init>\":(Lexample/Counter;Lexample/Counter;)V",
        ] {
            assert!(dump.contains(evidence), "javap omitted {evidence}:\n{dump}");
        }
        let javac = java_tool("javac", root)
            .args(["--release", "21", "-classpath"])
            .arg(&runtime)
            .arg("-d")
            .arg(&harness_classes)
            .arg(&harness)
            .output()
            .expect("javac independent Java consumer");
        assert!(
            javac.status.success(),
            "javac: {}{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let classpath =
            std::env::join_paths([harness_classes.as_path(), runtime.as_path()]).unwrap();
        let java = java_tool("java", root)
            .args(["-Xverify:all", "-ea", "-classpath"])
            .arg(classpath)
            .arg("example.JavaConsumer")
            .output()
            .expect("java original Rust classes with full verification");
        assert!(
            java.status.success(),
            "java -Xverify:all: {}{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&java.stdout).trim(),
            "local-call-runtime=3/3,42/42,7/7;identity=distinct;mutations=independent;repeat=fresh"
        );

        // Runtime negative controls keep verifier failures separate from a
        // verifier-valid call that mutates the wrong local or uses the wrong seed.
        let mut factory_reader = Reader::new(factory);
        let factory_class = ClassFile::decode(&mut factory_reader).expect("decode factory");
        let (code, _, _) = method_code(&factory_class, "create");
        let code_start = code.as_ptr() as usize - factory.as_ptr() as usize;
        let code_len = code.len();
        let instructions = decode(code);
        let calls = instructions
            .iter()
            .filter(|insn| matches!(insn.opcode, 0xb7 | 0xb6))
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), 4);
        let setter = calls[2];
        let seed = instructions
            .iter()
            .find(|insn| insn.pc + insn.len == setter.pc)
            .unwrap();
        let receiver = instructions
            .iter()
            .find(|insn| insn.pc + insn.len == seed.pc)
            .unwrap();
        let pair_new = instructions
            .iter()
            .find(|insn| {
                insn.opcode == 0xbb
                    && class_name(&factory_class, ConstantPoolIndex(insn.cp.unwrap()))
                        == "example/Pair"
            })
            .unwrap();
        let call_cp = &code[setter.pc + 1..setter.pc + 3];
        let setter_cp = u16::from_be_bytes(call_cp.try_into().unwrap());
        let ConstantPoolEntry::Methodref {
            class_index: counter_class_index,
            name_and_type_index,
        } = factory_class
            .constant_pool
            .get(ConstantPoolIndex(setter_cp))
            .expect("set Methodref")
        else {
            panic!("Counter.set uses Methodref");
        };
        assert_eq!(
            factory_class
                .constant_pool
                .class_name(*counter_class_index)
                .unwrap(),
            "example/Counter"
        );
        let ConstantPoolEntry::NameAndType { name_index, .. } = factory_class
            .constant_pool
            .get(*name_and_type_index)
            .expect("set NameAndType")
        else {
            panic!("Counter.set has NameAndType");
        };
        let pair_class_index = (1..256)
            .map(ConstantPoolIndex)
            .find(|index| {
                matches!(
                    factory_class.constant_pool.get(*index),
                    Some(ConstantPoolEntry::Class { name_index })
                        if factory_class.constant_pool.utf8(*name_index) == Ok("example/Pair")
                )
            })
            .expect("Pair class constant");
        let factory_index = originals
            .iter()
            .position(|(path, _)| path == "example/PairFactory.class")
            .unwrap();

        let run_corrupted_factory = |name: &str, edit: &dyn Fn(&mut Vec<u8>)| {
            let mut corrupted = originals.clone();
            edit(&mut corrupted[factory_index].1);
            let directory = root.join(format!("negative-{label}-{name}"));
            std::fs::create_dir_all(&directory).unwrap();
            write_runtime(&directory, &corrupted);
            run_harness(&harness_classes, &directory)
        };
        let wrong_receiver = run_corrupted_factory("wrong-receiver", &|bytes| {
            let replacement = aload(second_slot);
            replace_code_range(
                bytes,
                code_start,
                code_len,
                receiver.pc,
                receiver.pc + receiver.len,
                &replacement,
            );
        });
        assert_failure_at(
            &wrong_receiver,
            "runtime semantics",
            "create(7,3) expected 3/3",
        );

        let wrong_seed = run_corrupted_factory("wrong-seed", &|bytes| {
            assert_eq!(seed.opcode, 0x1c, "secondSeed is local slot two");
            bytes[code_start + seed.pc] = 0x1b; // iload_1: firstSeed
        });
        assert_failure_at(&wrong_seed, "runtime semantics", "create(7,3) expected 3/3");

        let skipped_setter = run_corrupted_factory("skipped-setter", &|bytes| {
            replace_code_range(
                bytes,
                code_start,
                code_len,
                receiver.pc,
                setter.pc + setter.len,
                &vec![0x00; setter.pc + setter.len - receiver.pc],
            );
        });
        assert_failure_at(
            &skipped_setter,
            "runtime semantics",
            "create(7,3) expected 3/3",
        );

        let duplicate_wrong_seed = run_corrupted_factory("duplicate-setter", &|bytes| {
            let mut duplicate = aload(first_slot);
            duplicate.push(0x1b); // iload_1: a second call with the wrong seed
            duplicate.push(0xb6);
            duplicate.extend_from_slice(call_cp);
            replace_code_range(
                bytes,
                code_start,
                code_len,
                pair_new.pc,
                pair_new.pc,
                &duplicate,
            );
        });
        assert_failure_at(
            &duplicate_wrong_seed,
            "runtime semantics",
            "create(7,3) expected 3/3",
        );

        let bad_opcode = run_corrupted_factory("wrong-opcode", &|bytes| {
            bytes[code_start + setter.pc] = 0xb8; // invokestatic with receiver still on stack
        });
        assert_failure_at(&bad_opcode, "bytecode verification", "VerifyError");

        let bad_owner = run_corrupted_factory("wrong-owner", &|bytes| {
            let offset = constant_pool_entry_offset(bytes, setter_cp);
            bytes[offset + 1..offset + 3].copy_from_slice(&pair_class_index.0.to_be_bytes());
        });
        assert_failure_at(&bad_owner, "bytecode verification", "VerifyError");

        let bad_descriptor = run_corrupted_factory("wrong-descriptor", &|bytes| {
            let count = u16::from_be_bytes(bytes[8..10].try_into().unwrap());
            let utf8_index = count;
            let nat_index = count + 1;
            let cp_end = constant_pool_end(bytes);
            let mut additions = vec![0x01, 0x00, 0x03, b'(', b')', b'V'];
            additions.extend_from_slice(&[0x0c]); // CONSTANT_NameAndType
            additions.extend_from_slice(&name_index.0.to_be_bytes());
            additions.extend_from_slice(&utf8_index.to_be_bytes());
            bytes.splice(cp_end..cp_end, additions);
            bytes[8..10].copy_from_slice(&(count + 2).to_be_bytes());
            let offset = constant_pool_entry_offset(bytes, setter_cp);
            bytes[offset + 3..offset + 5].copy_from_slice(&nat_index.to_be_bytes());
        });
        assert_failure_at(&bad_descriptor, "bytecode verification", "VerifyError");

        let bad_stack = run_corrupted_factory("small-stack", &|bytes| {
            bytes[code_start - 8..code_start - 6].copy_from_slice(&1u16.to_be_bytes());
        });
        assert_failure_at(&bad_stack, "bytecode verification", "VerifyError");

        let bad_locals = run_corrupted_factory("small-locals", &|bytes| {
            bytes[code_start - 6..code_start - 4].copy_from_slice(&4u16.to_be_bytes());
        });
        assert_failure_at(&bad_locals, "bytecode verification", "VerifyError");

        let before_first_store = instructions
            .iter()
            .find(|insn| matches!(insn.opcode, 0x4e | 0x3a))
            .unwrap();
        let call_before_receiver_init = run_corrupted_factory("call-before-init", &|bytes| {
            let mut early_call = aload(first_slot);
            early_call.push(0x1c);
            early_call.push(0xb6);
            early_call.extend_from_slice(call_cp);
            replace_code_range(
                bytes,
                code_start,
                code_len,
                before_first_store.pc,
                before_first_store.pc,
                &early_call,
            );
        });
        assert_failure_at(
            &call_before_receiver_init,
            "bytecode verification",
            "VerifyError",
        );

        let missing_method = run_corrupted_factory("missing-method", &|bytes| {
            let name = bytes
                .windows(6)
                .position(|window| window == [0x01, 0x00, 0x03, b's', b'e', b't'])
                .expect("PairFactory has a UTF8 set method name");
            bytes[name + 3..name + 6].copy_from_slice(b"bad");
        });
        assert_failure_at(&missing_method, "method resolution", "NoSuchMethodError");

        let missing_class_dir = root.join(format!("negative-{label}-missing-class"));
        std::fs::create_dir_all(&missing_class_dir).unwrap();
        write_runtime(
            &missing_class_dir,
            &originals
                .iter()
                .filter(|(path, _)| path != "example/Counter.class")
                .cloned()
                .collect::<Vec<_>>(),
        );
        let missing_class = run_harness(&harness_classes, &missing_class_dir);
        assert_failure_at(&missing_class, "class resolution", "NoClassDefFoundError");

        for (path, relative, bytes) in original_paths {
            assert_eq!(
                std::fs::read(&path).unwrap(),
                bytes,
                "Java tool modified Rust artifact {relative}"
            );
            assert_eq!(
                sha256(&path),
                original_hashes
                    .iter()
                    .find(|(name, _)| name == &relative)
                    .unwrap()
                    .1,
                "Rust artifact hash changed: {relative}"
            );
        }
        eprintln!(
            "local-receiver-runtime {label}: JDK verification passed; Counter.set(I)V once; six artifact hashes unchanged"
        );
    }
}

#[test]
fn ordered_two_local_unit_calls_execute_under_strict_jdk21() {
    if !require_jdk_21() {
        return;
    }
    let scratch = TestDirectory::new("ordered-two-call-runtime");
    let root = scratch.path();
    let harness_src = root.join("harness-src");
    let harness_classes = root.join("harness-classes");
    std::fs::create_dir_all(&harness_src).unwrap();
    std::fs::create_dir_all(&harness_classes).unwrap();
    let harness = harness_src.join("RuntimeProbe.java");
    std::fs::write(&harness, TWO_CALL_HARNESS).unwrap();

    let reversed_declarations = TWO_CALL_SOURCE.replace(
        "    val first: Counter = new Counter(firstSeed)\n    val second: Counter = new Counter(secondSeed)",
        "    val second: Counter = new Counter(secondSeed)\n    val first: Counter = new Counter(firstSeed)",
    );
    let reversed_calls = TWO_CALL_SOURCE.replace(
        "    first.set(secondSeed)\n    second.set(firstSeed)",
        "    second.set(firstSeed)\n    first.set(secondSeed)",
    );
    let same_receiver = TWO_CALL_SOURCE.replace(
        "    first.set(secondSeed)\n    second.set(firstSeed)",
        "    first.set(secondSeed)\n    first.set(firstSeed)",
    );
    assert_ne!(reversed_declarations, TWO_CALL_SOURCE);
    assert_ne!(reversed_calls, TWO_CALL_SOURCE);
    assert_ne!(same_receiver, TWO_CALL_SOURCE);

    let variants = [
        ("ordered", TWO_CALL_SOURCE, 21911, [0, 1], [2, 1], false),
        (
            "declarations-reversed",
            reversed_declarations.as_str(),
            21912,
            [0, 1],
            [2, 1],
            false,
        ),
        (
            "calls-reversed",
            reversed_calls.as_str(),
            21913,
            [1, 0],
            [1, 2],
            false,
        ),
        (
            "same-receiver",
            same_receiver.as_str(),
            21914,
            [0, 0],
            [2, 1],
            true,
        ),
    ];
    let mut primary: Option<PrimaryRuntimeBatch> = None;
    let mut compiled_consumer = false;
    for (label, source, source_id, receiver_ordinals, argument_slots, same_receiver) in variants {
        let batch = emit_paired_jvm_tasty_batch(
            source,
            "OrderedLocalUnitCalls.scala",
            SourceId::from_index(source_id),
        )
        .unwrap_or_else(|error| panic!("{label} two-call batch emits: {error:?}"));
        let originals = artifacts(&batch);
        let repeated = emit_paired_jvm_tasty_batch(
            source,
            "OrderedLocalUnitCalls.scala",
            SourceId::from_index(source_id),
        )
        .unwrap_or_else(|error| panic!("fresh {label} two-call session emits: {error:?}"));
        assert_eq!(
            originals,
            artifacts(&repeated),
            "{label} two-call output is deterministic across fresh sessions"
        );

        let factory = originals
            .iter()
            .find(|(path, _)| path == "example/PairFactory.class")
            .unwrap()
            .1
            .as_slice();
        let class = ClassFile::decode(&mut Reader::new(factory)).expect("decode PairFactory");
        let (code, max_stack, max_locals) = method_code(&class, "create");
        assert_eq!((max_stack, max_locals), (4, 5));
        let instructions = decode(code);
        let calls = instructions
            .iter()
            .filter(|instruction| matches!(instruction.opcode, 0xb7 | 0xb6))
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), 5, "two constructors, two setters, Pair");
        assert_eq!(
            method_ref(&class, ConstantPoolIndex(calls[0].cp.unwrap())),
            ("example/Counter", "<init>", "(I)V")
        );
        assert_eq!(
            method_ref(&class, ConstantPoolIndex(calls[1].cp.unwrap())),
            ("example/Counter", "<init>", "(I)V")
        );
        let mut local_for_formal = [None, None];
        for constructor in &calls[..2] {
            let seed = instructions
                .iter()
                .find(|instruction| instruction.pc + instruction.len == constructor.pc)
                .expect("constructor seed load");
            let seed_slot = iload_slot(seed).expect("Counter constructor receives an int local");
            assert!(matches!(seed_slot, 1 | 2));
            let store = instructions
                .iter()
                .find(|instruction| instruction.pc == constructor.pc + constructor.len)
                .expect("initialized Counter is stored immediately after construction");
            let local_slot = astore_slot(store).expect("Counter initializer stores a local");
            let entry = &mut local_for_formal[(seed_slot - 1) as usize];
            assert!(
                entry.replace(local_slot).is_none(),
                "one local per seed formal"
            );
        }
        let first_slot = local_for_formal[0].expect("first is initialized from firstSeed");
        let second_slot = local_for_formal[1].expect("second is initialized from secondSeed");
        assert_ne!(
            first_slot, second_slot,
            "Pair locals have distinct JVM slots"
        );

        let setter_calls = &calls[2..4];
        for setter in setter_calls {
            assert_eq!(setter.opcode, 0xb6, "Unit calls use invokevirtual");
            assert_eq!(
                method_ref(&class, ConstantPoolIndex(setter.cp.unwrap())),
                ("example/Counter", "set", "(I)V")
            );
        }
        let semantic_locals = [first_slot, second_slot];
        for (index, setter) in setter_calls.iter().enumerate() {
            let argument = instructions
                .iter()
                .find(|instruction| instruction.pc + instruction.len == setter.pc)
                .expect("Int formal load immediately precedes Counter.set");
            let receiver = instructions
                .iter()
                .find(|instruction| instruction.pc + instruction.len == argument.pc)
                .expect("local receiver load immediately precedes the Int argument");
            assert_eq!(
                aload_slot(receiver),
                Some(semantic_locals[receiver_ordinals[index]]),
                "{label} call {index} receiver is derived from its initialized local"
            );
            assert_eq!(
                iload_slot(argument),
                Some(argument_slots[index]),
                "{label} call {index} uses its authenticated Int formal"
            );
            assert!(calls[1].pc < setter.pc);
        }
        assert_eq!(
            method_ref(&class, ConstantPoolIndex(calls[4].cp.unwrap())),
            (
                "example/Pair",
                "<init>",
                "(Lexample/Counter;Lexample/Counter;)V"
            )
        );
        let pair_new = instructions
            .iter()
            .find(|instruction| {
                instruction.opcode == 0xbb
                    && class_name(&class, ConstantPoolIndex(instruction.cp.unwrap()))
                        == "example/Pair"
            })
            .expect("Pair is allocated after both local Unit calls");
        assert!(calls[3].pc < pair_new.pc && pair_new.pc < calls[4].pc);
        let pair_loads = instructions
            .iter()
            .filter(|instruction| instruction.pc > pair_new.pc && instruction.pc < calls[4].pc)
            .filter_map(aload_slot)
            .collect::<Vec<_>>();
        assert_eq!(pair_loads, [first_slot, second_slot]);

        let runtime = root.join(format!("runtime-{label}"));
        std::fs::create_dir_all(&runtime).unwrap();
        write_runtime(&runtime, &originals);
        let original_hashes = originals
            .iter()
            .map(|(path, bytes)| (path.clone(), sha256_bytes(bytes)))
            .collect::<Vec<_>>();
        let javap = java_tool("javap", root)
            .args(["-c", "-v", "-p", "-classpath"])
            .arg(&runtime)
            .arg("example.PairFactory")
            .output()
            .expect("javap two-call Rust PairFactory");
        assert!(
            javap.status.success(),
            "javap failed: {}",
            String::from_utf8_lossy(&javap.stderr)
        );
        let dump = String::from_utf8_lossy(&javap.stdout);
        let setter_lines = dump
            .lines()
            .filter(|line| {
                line.contains("invokevirtual") && line.contains("example/Counter.set:(I)V")
            })
            .collect::<Vec<_>>();
        assert_eq!(setter_lines.len(), 2, "javap shows exactly two set sites");
        assert!(dump.contains("major version: 65"));
        assert!(dump.contains("stack=4, locals=5"));

        if !compiled_consumer {
            let javac = java_tool("javac", root)
                .args(["--release", "21", "-classpath"])
                .arg(&runtime)
                .arg("-d")
                .arg(&harness_classes)
                .arg(&harness)
                .output()
                .expect("javac independent two-call consumer");
            assert!(
                javac.status.success(),
                "javac: {}{}",
                String::from_utf8_lossy(&javac.stdout),
                String::from_utf8_lossy(&javac.stderr)
            );
            compiled_consumer = true;
        }
        let mode = if same_receiver {
            "same-receiver"
        } else {
            "cross-receiver"
        };
        let java = run_runtime_probe(&harness_classes, &runtime, mode);
        assert!(
            java.status.success(),
            "java -Xverify:all: {}{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&java.stdout).trim(),
            format!(
                "two-call-runtime=positive-cases;identity=distinct;mutation=independent;mode={mode}"
            )
        );
        assert_runtime_artifacts_unchanged(&runtime, &originals, &original_hashes);
        if label == "ordered" {
            primary = Some(PrimaryRuntimeBatch {
                originals,
                runtime,
                first_local_slot: first_slot,
            });
        }
    }

    let PrimaryRuntimeBatch {
        originals,
        runtime: primary_runtime,
        first_local_slot,
    } = primary.expect("primary Rust batch is retained");
    let factory_index = originals
        .iter()
        .position(|(path, _)| path == "example/PairFactory.class")
        .expect("PairFactory artifact");
    let factory = originals[factory_index].1.as_slice();
    let original_hashes = originals
        .iter()
        .map(|(path, bytes)| (path.clone(), sha256_bytes(bytes)))
        .collect::<Vec<_>>();
    assert_runtime_artifacts_unchanged(&primary_runtime, &originals, &original_hashes);

    let mut factory_reader = Reader::new(factory);
    let class = ClassFile::decode(&mut factory_reader).expect("decode primary PairFactory");
    let (code, _, _) = method_code(&class, "create");
    let code_start = code.as_ptr() as usize - factory.as_ptr() as usize;
    let code_len = code.len();
    let instructions = decode(code);
    let invocations = instructions
        .iter()
        .filter(|instruction| matches!(instruction.opcode, 0xb7 | 0xb6))
        .collect::<Vec<_>>();
    assert_eq!(invocations.len(), 5);
    let setters = [invocations[2], invocations[3]];
    let load_before = |insn: &Insn| {
        instructions
            .iter()
            .find(|instruction| instruction.pc + instruction.len == insn.pc)
            .copied()
            .expect("immediately preceding bytecode instruction")
    };
    let first_argument = load_before(setters[0]);
    let first_receiver = load_before(&first_argument);
    let second_argument = load_before(setters[1]);
    let second_receiver = load_before(&second_argument);
    let pair_new = instructions
        .iter()
        .find(|instruction| {
            instruction.opcode == 0xbb
                && class_name(&class, ConstantPoolIndex(instruction.cp.unwrap())) == "example/Pair"
        })
        .copied()
        .unwrap();
    let setter_cp = setters[0].cp.unwrap();
    assert_eq!(require_exactly_two_setter_sites(factory), Ok(()));

    let run_corrupted = |name: &str, edit: &dyn Fn(&mut Vec<u8>)| {
        let mut corrupted = originals.clone();
        edit(&mut corrupted[factory_index].1);
        let directory = root.join(format!("negative-{name}"));
        std::fs::create_dir_all(&directory).unwrap();
        write_runtime(&directory, &corrupted);
        run_runtime_probe(&harness_classes, &directory, "cross-receiver")
    };

    let swapped_receivers = run_corrupted("swapped-receivers", &|bytes| {
        let mut replacement = aload(aload_slot(&second_receiver).unwrap());
        replacement
            .extend_from_slice(&code[first_argument.pc..first_argument.pc + first_argument.len]);
        replacement.extend_from_slice(&code[setters[0].pc..setters[0].pc + setters[0].len]);
        replacement.extend(aload(aload_slot(&first_receiver).unwrap()));
        replacement
            .extend_from_slice(&code[second_argument.pc..second_argument.pc + second_argument.len]);
        replacement.extend_from_slice(&code[setters[1].pc..setters[1].pc + setters[1].len]);
        replace_code_range(
            bytes,
            code_start,
            code_len,
            first_receiver.pc,
            setters[1].pc + setters[1].len,
            &replacement,
        );
    });
    assert_failure_at(
        &swapped_receivers,
        "JVM-valid wrong receiver runtime assertion",
        "unexpected pair for seeds 7/3",
    );

    let swapped_arguments = run_corrupted("swapped-arguments", &|bytes| {
        let first_slot = iload_slot(&first_argument).unwrap();
        let second_slot = iload_slot(&second_argument).unwrap();
        bytes[code_start + first_argument.pc] = iload_opcode(second_slot);
        bytes[code_start + second_argument.pc] = iload_opcode(first_slot);
    });
    assert_failure_at(
        &swapped_arguments,
        "JVM-valid wrong seed runtime assertion",
        "unexpected pair for seeds 7/3",
    );

    let skipped_setter = run_corrupted("skipped-second-setter", &|bytes| {
        let start = second_receiver.pc;
        let end = setters[1].pc + setters[1].len;
        replace_code_range(
            bytes,
            code_start,
            code_len,
            start,
            end,
            &vec![0x00; end - start],
        );
    });
    assert_failure_at(
        &skipped_setter,
        "JVM-valid skipped-call runtime assertion",
        "unexpected pair for seeds 7/3",
    );

    let mut duplicate = originals.clone();
    let mut duplicate_call = Vec::new();
    duplicate_call.extend_from_slice(&code[first_receiver.pc..setters[0].pc + setters[0].len]);
    replace_code_range(
        &mut duplicate[factory_index].1,
        code_start,
        code_len,
        pair_new.pc,
        pair_new.pc,
        &duplicate_call,
    );
    assert_eq!(
        require_exactly_two_setter_sites(&duplicate[factory_index].1),
        Err(3),
        "the exact-two-sites bytecode contract rejects a duplicated setter"
    );
    let duplicate_dir = root.join("negative-duplicated-setter");
    std::fs::create_dir_all(&duplicate_dir).unwrap();
    write_runtime(&duplicate_dir, &duplicate);
    let duplicate_java = run_runtime_probe(&harness_classes, &duplicate_dir, "cross-receiver");
    assert!(
        duplicate_java.status.success(),
        "duplicating a same-value setter can preserve final values; exact call-site assertion must reject it: {}{}",
        String::from_utf8_lossy(&duplicate_java.stdout),
        String::from_utf8_lossy(&duplicate_java.stderr)
    );

    let bad_opcode = run_corrupted("wrong-opcode", &|bytes| {
        bytes[code_start + setters[0].pc] = 0xb8;
    });
    assert_failure_at(&bad_opcode, "bytecode verification", "VerifyError");

    let pair_class_index = (1..256u16)
        .map(ConstantPoolIndex)
        .find(|index| {
            matches!(
                class.constant_pool.get(*index),
                Some(ConstantPoolEntry::Class { name_index })
                    if class.constant_pool.utf8(*name_index) == Ok("example/Pair")
            )
        })
        .expect("Pair Class constant");
    let bad_owner = run_corrupted("wrong-methodref-owner", &|bytes| {
        let offset = constant_pool_entry_offset(bytes, setter_cp);
        bytes[offset + 1..offset + 3].copy_from_slice(&pair_class_index.0.to_be_bytes());
    });
    assert_failure_at(&bad_owner, "bytecode verification", "VerifyError");

    let bad_descriptor = run_corrupted("wrong-methodref-descriptor", &|bytes| {
        let count = u16::from_be_bytes(bytes[8..10].try_into().unwrap());
        let utf8_index = count;
        let nat_index = count + 1;
        let cp_end = constant_pool_end(bytes);
        let mut additions = vec![0x01, 0x00, 0x03, b'(', b')', b'I'];
        additions.extend_from_slice(&[0x0c]);
        let ConstantPoolEntry::Methodref {
            name_and_type_index,
            ..
        } = class
            .constant_pool
            .get(ConstantPoolIndex(setter_cp))
            .expect("set Methodref")
        else {
            panic!("set uses Methodref");
        };
        let ConstantPoolEntry::NameAndType { name_index, .. } = class
            .constant_pool
            .get(*name_and_type_index)
            .expect("set NameAndType")
        else {
            panic!("set has NameAndType");
        };
        additions.extend_from_slice(&name_index.0.to_be_bytes());
        additions.extend_from_slice(&utf8_index.to_be_bytes());
        bytes.splice(cp_end..cp_end, additions);
        bytes[8..10].copy_from_slice(&(count + 2).to_be_bytes());
        let offset = constant_pool_entry_offset(bytes, setter_cp);
        bytes[offset + 3..offset + 5].copy_from_slice(&nat_index.to_be_bytes());
    });
    assert_failure_at(&bad_descriptor, "bytecode verification", "VerifyError");

    let bad_stack = run_corrupted("small-max-stack", &|bytes| {
        bytes[code_start - 8..code_start - 6].copy_from_slice(&1u16.to_be_bytes());
    });
    assert_failure_at(&bad_stack, "bytecode verification", "VerifyError");

    let bad_locals = run_corrupted("small-max-locals", &|bytes| {
        bytes[code_start - 6..code_start - 4].copy_from_slice(&4u16.to_be_bytes());
    });
    assert_failure_at(&bad_locals, "bytecode verification", "VerifyError");

    let first_store = instructions
        .iter()
        .find(|instruction| astore_slot(instruction).is_some())
        .copied()
        .expect("first local initializer store");
    let before_store = run_corrupted("use-before-store", &|bytes| {
        let mut early_call = aload(first_local_slot);
        early_call.push(0x1c);
        early_call.push(0xb6);
        early_call.extend_from_slice(&setter_cp.to_be_bytes());
        replace_code_range(
            bytes,
            code_start,
            code_len,
            first_store.pc,
            first_store.pc,
            &early_call,
        );
    });
    assert_failure_at(&before_store, "bytecode verification", "VerifyError");

    let missing_method = run_corrupted("missing-set-abi", &|bytes| {
        replace_utf8_constant(bytes, b"set", b"bad");
    });
    assert_failure_at(&missing_method, "method resolution", "NoSuchMethodError");

    let stale_factory = run_corrupted("stale-factory-abi", &|bytes| {
        replace_utf8_constant(bytes, b"create", b"crater");
    });
    assert_failure_at(&stale_factory, "stale factory ABI", "NoSuchMethodError");

    let missing_factory_dir = root.join("negative-missing-factory-class");
    std::fs::create_dir_all(&missing_factory_dir).unwrap();
    write_runtime(
        &missing_factory_dir,
        &originals
            .iter()
            .filter(|(path, _)| path != "example/PairFactory.class")
            .cloned()
            .collect::<Vec<_>>(),
    );
    let missing_factory =
        run_runtime_probe(&harness_classes, &missing_factory_dir, "cross-receiver");
    assert_failure_at(
        &missing_factory,
        "stale class linkage",
        "NoClassDefFoundError",
    );

    assert_runtime_artifacts_unchanged(&primary_runtime, &originals, &original_hashes);
}

fn sha256_bytes(bytes: &[u8]) -> String {
    use std::io::Write;
    let mut output = Command::new("sha256sum")
        .arg("--tag")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("sha256sum");
    output.stdin.take().unwrap().write_all(bytes).unwrap();
    let result = output.wait_with_output().unwrap();
    assert!(result.status.success());
    String::from_utf8(result.stdout)
        .unwrap()
        .split_whitespace()
        .last()
        .unwrap()
        .trim_end_matches('*')
        .to_owned()
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn constant_pool_end(bytes: &[u8]) -> usize {
    let count = u16::from_be_bytes(bytes[8..10].try_into().unwrap());
    let mut index = 1u16;
    let mut offset = 10usize;
    while index < count {
        let tag = bytes[offset];
        let size = match tag {
            1 => 3 + u16::from_be_bytes(bytes[offset + 1..offset + 3].try_into().unwrap()) as usize,
            3 | 4 => 5,
            5 | 6 => 9,
            7 | 8 | 16 | 19 | 20 => 3,
            9 | 10 | 11 | 12 | 17 | 18 => 5,
            15 => 4,
            _ => panic!("unknown constant pool tag {tag}"),
        };
        offset += size;
        index += if matches!(tag, 5 | 6) { 2 } else { 1 };
    }
    offset
}

fn constant_pool_entry_offset(bytes: &[u8], wanted: u16) -> usize {
    let count = u16::from_be_bytes(bytes[8..10].try_into().unwrap());
    let mut index = 1u16;
    let mut offset = 10usize;
    while index < count {
        if index == wanted {
            return offset;
        }
        let tag = bytes[offset];
        let size = match tag {
            1 => 3 + u16::from_be_bytes(bytes[offset + 1..offset + 3].try_into().unwrap()) as usize,
            3 | 4 => 5,
            5 | 6 => 9,
            7 | 8 | 16 | 19 | 20 => 3,
            9 | 10 | 11 | 12 | 17 | 18 => 5,
            15 => 4,
            _ => panic!("unknown constant pool tag {tag}"),
        };
        offset += size;
        index += if matches!(tag, 5 | 6) { 2 } else { 1 };
    }
    panic!("constant pool index {wanted} is missing (count={count})");
}

fn set_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_be_bytes());
}

fn replace_code_range(
    classfile: &mut Vec<u8>,
    code_start: usize,
    code_len: usize,
    start: usize,
    end: usize,
    replacement: &[u8],
) {
    let delta = replacement.len() as isize - (end - start) as isize;
    classfile.splice(
        code_start + start..code_start + end,
        replacement.iter().copied(),
    );
    let new_code_len = (code_len as isize + delta) as u32;
    let new_attribute_len = (u32_at(classfile, code_start - 12) as isize + delta) as u32;
    set_u32(classfile, code_start - 4, new_code_len);
    set_u32(classfile, code_start - 12, new_attribute_len);
}

fn write_runtime(root: &std::path::Path, artifacts: &[(String, Vec<u8>)]) {
    for (relative, bytes) in artifacts {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
}

fn run_runtime_probe(
    harness_classes: &std::path::Path,
    runtime: &std::path::Path,
    mode: &str,
) -> std::process::Output {
    let classpath = std::env::join_paths([harness_classes, runtime]).unwrap();
    java_tool("java", runtime)
        .args(["-Xverify:all", "-ea", "-classpath"])
        .arg(classpath)
        .arg("example.RuntimeProbe")
        .arg(mode)
        .output()
        .expect("run strict JDK 21 two-call runtime probe")
}

fn assert_runtime_artifacts_unchanged(
    runtime: &Path,
    originals: &[(String, Vec<u8>)],
    hashes: &[(String, String)],
) {
    assert_eq!(originals.len(), 6);
    assert_eq!(hashes.len(), 6);
    for (relative, bytes) in originals {
        let path = runtime.join(relative);
        assert_eq!(
            std::fs::read(&path).unwrap().as_slice(),
            bytes.as_slice(),
            "artifact changed: {relative}"
        );
        assert_eq!(
            sha256(&path),
            hashes.iter().find(|(name, _)| name == relative).unwrap().1,
            "artifact hash changed: {relative}"
        );
    }
}

fn exact_setter_invocation_count(factory: &[u8]) -> usize {
    let mut reader = Reader::new(factory);
    let class = ClassFile::decode(&mut reader).expect("decode PairFactory");
    let (code, _, _) = method_code(&class, "create");
    decode(code)
        .iter()
        .filter(|instruction| {
            instruction.opcode == 0xb6
                && method_ref(&class, ConstantPoolIndex(instruction.cp.unwrap()))
                    == ("example/Counter", "set", "(I)V")
        })
        .count()
}

fn require_exactly_two_setter_sites(factory: &[u8]) -> Result<(), usize> {
    let count = exact_setter_invocation_count(factory);
    (count == 2).then_some(()).ok_or(count)
}

fn run_harness(
    harness_classes: &std::path::Path,
    runtime: &std::path::Path,
) -> std::process::Output {
    let classpath = std::env::join_paths([harness_classes, runtime]).unwrap();
    java_tool("java", runtime)
        .args(["-Xverify:all", "-ea", "-classpath"])
        .arg(classpath)
        .arg("example.JavaConsumer")
        .output()
        .expect("run strict JDK 21 negative control")
}

fn aload(slot: u8) -> Vec<u8> {
    if slot <= 3 {
        vec![0x2a + slot]
    } else {
        vec![0x19, slot]
    }
}

fn iload_opcode(slot: u8) -> u8 {
    assert!(slot <= 3, "fixture Int formals use compact local loads");
    0x1a + slot
}

fn replace_utf8_constant(classfile: &mut [u8], original: &[u8], replacement: &[u8]) {
    assert_eq!(original.len(), replacement.len());
    let length = u16::try_from(original.len()).unwrap().to_be_bytes();
    let mut entry = vec![0x01, length[0], length[1]];
    entry.extend_from_slice(original);
    let matches = classfile
        .get(..constant_pool_end(classfile))
        .expect("constant-pool range")
        .windows(entry.len())
        .enumerate()
        .filter_map(|(offset, window)| (window == entry).then_some(offset))
        .collect::<Vec<_>>();
    assert_eq!(matches.len(), 1, "one constant-pool UTF8 entry is mutated");
    let value_start = matches[0] + 3;
    classfile[value_start..value_start + replacement.len()].copy_from_slice(replacement);
}

fn assert_failure_at(output: &std::process::Output, stage: &str, message: &str) {
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "negative control unexpectedly passed"
    );
    assert!(
        text.contains(message),
        "expected {stage} failure containing {message:?}, got:\n{text}"
    );
}
