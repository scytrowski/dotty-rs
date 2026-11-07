use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_batch;
use dotty_classfile::attribute::Attribute;
use dotty_classfile::classfile::{ClassFile, Reader};
use dotty_classfile::constant_pool::{ConstantPoolEntry, ConstantPoolIndex};
use dotty_core::SourceId;
use dotty_tasty::tasty::TastyFile;
use std::path::{Path, PathBuf};
use std::process::Command;

const SOURCE: &str = include_str!("fixtures/jvm_tasty_local_receiver_call/LocalReceiverCall.scala");
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
            0x19 | 0x3a => {
                local = Some(code[pc]);
                pc += 1;
            }
            0xbb | 0xb7 | 0xb6 => {
                cp = Some(u16::from_be_bytes([code[pc], code[pc + 1]]));
                pc += 2;
            }
            0x59 | 0x1b | 0x1c | 0x2b | 0x2c | 0x2d | 0x4e | 0x4c | 0xb0 => {}
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
