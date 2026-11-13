use super::{SOURCE, conditional_reference_artifacts as receipt};
use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_batch;
use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::{ConstantPoolEntry, ConstantPoolIndex};
use dotty_classfile::reader::Reader as ClassReader;
use dotty_core::SourceId;
use dotty_tasty::file::TastyFile;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

const JAVA_PROBE: &str =
    include_str!("../fixtures/jvm_tasty_conditional_reference_selection/RustRuntimeProbe.java");

const EXPECTED_PATHS: [&str; 6] = [
    "example/Counter.class",
    "example/Counter.tasty",
    "example/Pair.class",
    "example/Pair.tasty",
    "example/PairFactory.class",
    "example/PairFactory.tasty",
];

#[derive(Debug, PartialEq, Eq)]
struct RustBatch {
    files: Vec<(String, Vec<u8>)>,
    uuid_links: Vec<(String, String)>,
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "dotty-conditional-rust-{}-{label}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap_or_else(|error| {
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
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn output(command: &mut Command) -> Output {
    let rendered = format!("{command:?}");
    command
        .output()
        .unwrap_or_else(|error| panic!("run {rendered}: {error}"))
}

fn successful(command: &mut Command) -> Output {
    let rendered = format!("{command:?}");
    let result = output(command);
    assert!(
        result.status.success(),
        "command failed: {rendered}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    result
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let mut child = Command::new("sha256sum")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("start sha256sum for producer bytes");
    use std::io::Write;
    child
        .stdin
        .take()
        .expect("sha256sum has piped stdin")
        .write_all(bytes)
        .expect("write producer bytes to sha256sum");
    let result = child.wait_with_output().expect("finish sha256sum");
    assert!(result.status.success(), "sha256sum accepts producer bytes");
    String::from_utf8(result.stdout)
        .expect("sha256sum output is UTF-8")
        .split_whitespace()
        .next()
        .expect("sha256sum prints a digest")
        .to_owned()
}

fn emit_fresh_batch() -> RustBatch {
    let batch = emit_paired_jvm_tasty_batch(
        SOURCE,
        receipt::PRODUCER_SOURCE_NAME,
        SourceId::from_index(22_710),
    )
    .expect("frozen conditional source emits its original paired Rust artifacts");
    let mut files = Vec::with_capacity(6);
    let mut uuid_links = Vec::with_capacity(3);
    for class in batch.classes() {
        let class_bytes = class.classfile_bytes().to_vec();
        let tasty_bytes = class.tasty_bytes().to_vec();
        let mut class_reader = ClassReader::new(&class_bytes);
        let decoded = ClassFile::decode(&mut class_reader).expect("decode Rust classfile");
        assert!(class_reader.is_at_end());
        assert_eq!(decoded.version.major, 65);
        let tasty = TastyFile::parse_and_validate_scala_3_9(&tasty_bytes)
            .expect("decode Rust TASTy sidecar");
        assert_eq!(tasty.header().major_version, 28);
        assert_eq!(tasty.header().minor_version, 9);
        assert_eq!(
            decoded.tasty_link_id().expect("class TASTy link"),
            Some(class.tasty_link_id())
        );
        assert_eq!(tasty.header().uuid, *class.tasty_link_id().as_bytes());
        uuid_links.push((
            class.classfile_path().to_owned(),
            class
                .tasty_link_id()
                .as_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        ));
        files.push((class.classfile_path().to_owned(), class_bytes));
        files.push((class.tasty_path().to_owned(), tasty_bytes));
    }
    let paths = files
        .iter()
        .map(|(path, _)| path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(paths, EXPECTED_PATHS, "canonical ordered six-file output");
    assert_eq!(uuid_links.len(), 3, "three matched class/TASTy UUIDs");
    assert_eq!(
        uuid_links,
        receipt::TASTY_UUID_LINKS.map(|(path, uuid)| (path.to_owned(), uuid.to_owned()))
    );
    RustBatch { files, uuid_links }
}

fn write_batch(root: &Path, files: &[(String, Vec<u8>)]) {
    for (relative, bytes) in files {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("artifact parent"))
            .expect("create artifact directory");
        fs::write(path, bytes).expect("write Rust artifact copy");
    }
}

fn factory_bytes(batch: &RustBatch) -> &[u8] {
    &batch
        .files
        .iter()
        .find(|(path, _)| path == "example/PairFactory.class")
        .expect("PairFactory class is in the six-file batch")
        .1
}

fn decode_factory(batch: &RustBatch) -> ClassFile<'_> {
    let bytes = factory_bytes(batch);
    let mut reader = ClassReader::new(bytes);
    let class = ClassFile::decode(&mut reader).expect("decode Rust PairFactory");
    assert!(reader.is_at_end());
    class
}

fn assert_factory_contract(batch: &RustBatch) -> Result<(), String> {
    let class = decode_factory(batch);
    let method = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index).ok() == Some("create"))
        .ok_or_else(|| "missing PairFactory.create".to_owned())?;
    if class.constant_pool.utf8(method.descriptor_index).ok() != Some("(IIZI)Lexample/Pair;") {
        return Err("wrong create descriptor".to_owned());
    }
    let [Attribute::Code(code)] = method.attributes.as_slice() else {
        return Err("create has no unique Code attribute".to_owned());
    };
    if code.max_stack != 4 || code.max_locals != 8 {
        return Err("unexpected create stack/local layout".to_owned());
    }
    let instructions = code.code;
    let allocations = instructions
        .iter()
        .filter(|opcode| **opcode == 0xbb)
        .count();
    if allocations != 3 {
        return Err(format!(
            "expected two Counter and one Pair allocations, got {allocations}"
        ));
    }
    let branches = instructions
        .iter()
        .enumerate()
        .filter(|(_, opcode)| matches!(**opcode, 0x99 | 0xa7))
        .map(|(offset, opcode)| (offset, *opcode))
        .collect::<Vec<_>>();
    if branches != [(21, 0x99), (26, 0xa7)] {
        return Err(format!(
            "conditional/join branch shape differs: {branches:?}"
        ));
    }
    let true_branch_target =
        21isize + i16::from_be_bytes([instructions[22], instructions[23]]) as isize;
    let join_target = 26isize + i16::from_be_bytes([instructions[27], instructions[28]]) as isize;
    if instructions[20] != 0x1d
        || instructions[21] != 0x99
        || true_branch_target != 29
        || instructions[26] != 0xa7
        || join_target != 31
        || instructions[24..26] != [0x19, 5]
        || instructions[29..31] != [0x19, 6]
        || instructions[31..33] != [0x3a, 7]
        || instructions[33..35] != [0x19, 7]
        || instructions[35..37] != [0x15, 4]
        || instructions[37] != 0xb6
        || instructions[40] != 0xbb
        || instructions[44..46] != [0x19, 5]
        || instructions[46..48] != [0x19, 6]
        || instructions[48] != 0xb7
    {
        return Err(format!(
            "condition arms, selected slot, or setter receiver differ: condition={:#04x}, ifeq={:#04x}, targets={true_branch_target}/{join_target}, true={:?}, false={:?}, store={:?}, receiver={:?}, arg={:?}, setter={:#04x}, pair={:?}",
            instructions[20],
            instructions[21],
            &instructions[24..26],
            &instructions[29..31],
            &instructions[31..33],
            &instructions[33..35],
            &instructions[35..37],
            instructions[37],
            instructions.get(40..49)
        ));
    }
    let allocated_class = |offset: usize| -> Result<&str, String> {
        let index = ConstantPoolIndex(u16::from_be_bytes([
            instructions[offset + 1],
            instructions[offset + 2],
        ]));
        class
            .constant_pool
            .class_name(index)
            .map_err(|error| format!("invalid allocation class at {offset}: {error:?}"))
    };
    if allocated_class(0)? != "example/Counter"
        || allocated_class(10)? != "example/Counter"
        || allocated_class(40)? != "example/Pair"
    {
        return Err("allocations are not two Counters followed by the Pair result".to_owned());
    }
    let method_ref = |offset: usize| -> Result<(&str, &str, &str), String> {
        let index = ConstantPoolIndex(u16::from_be_bytes([
            instructions[offset + 1],
            instructions[offset + 2],
        ]));
        let Some(ConstantPoolEntry::Methodref {
            class_index,
            name_and_type_index,
        }) = class.constant_pool.get(index)
        else {
            return Err(format!("invocation at {offset} is not a Methodref"));
        };
        let owner = class
            .constant_pool
            .class_name(*class_index)
            .map_err(|error| format!("invalid invocation owner: {error:?}"))?;
        let Some(ConstantPoolEntry::NameAndType {
            name_index,
            descriptor_index,
        }) = class.constant_pool.get(*name_and_type_index)
        else {
            return Err(format!("invocation at {offset} has invalid NameAndType"));
        };
        let name = class
            .constant_pool
            .utf8(*name_index)
            .map_err(|error| format!("invalid invocation name: {error:?}"))?;
        let descriptor = class
            .constant_pool
            .utf8(*descriptor_index)
            .map_err(|error| format!("invalid invocation descriptor: {error:?}"))?;
        Ok((owner, name, descriptor))
    };
    if method_ref(37)? != ("example/Counter", "set", "(I)V") {
        return Err("selected call is not Counter.set(I)V".to_owned());
    }
    if method_ref(48)?
        != (
            "example/Pair",
            "<init>",
            "(Lexample/Counter;Lexample/Counter;)V",
        )
    {
        return Err("Pair construction has the wrong constructor descriptor".to_owned());
    }
    if method_ref(5)? != ("example/Counter", "<init>", "(I)V")
        || method_ref(15)? != ("example/Counter", "<init>", "(I)V")
    {
        return Err("Counter allocations do not use the original constructors".to_owned());
    }
    let invocations = instructions
        .iter()
        .filter(|opcode| matches!(**opcode, 0xb6 | 0xb7))
        .count();
    if invocations != 4 {
        return Err(format!(
            "expected two Counter constructors, one setter, one Pair constructor; got {invocations}"
        ));
    }
    if !method.attributes.iter().any(|attribute| {
        matches!(attribute, Attribute::Code(code) if code.attributes.iter().any(|nested| matches!(nested, Attribute::Other(raw) if class.constant_pool.utf8(raw.name_index).ok() == Some("StackMapTable"))))
    }) {
        return Err("missing StackMapTable".to_owned());
    }
    Ok(())
}

fn java_tool(name: &str, root: &Path) -> Command {
    let mut command = Command::new(name);
    let mut options = std::env::var("JAVA_TOOL_OPTIONS").unwrap_or_default();
    if !options.is_empty() {
        options.push(' ');
    }
    options.push_str(&format!("-Djava.io.tmpdir={}", root.display()));
    command.env("JAVA_TOOL_OPTIONS", options);
    command
}

fn require_jdk_21() -> bool {
    let missing = ["java", "javac", "javap"]
        .into_iter()
        .filter_map(|tool| {
            let version = Command::new(tool).arg("-version").output().ok()?;
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&version.stdout),
                String::from_utf8_lossy(&version.stderr)
            );
            (!version.status.success() || !text.contains("21.0.")).then_some((tool, text))
        })
        .collect::<Vec<_>>();
    let temurin = Command::new("java")
        .arg("-version")
        .output()
        .ok()
        .map(|version| {
            format!(
                "{}{}",
                String::from_utf8_lossy(&version.stdout),
                String::from_utf8_lossy(&version.stderr)
            )
            .contains("Temurin")
        })
        .unwrap_or(false);
    if missing.is_empty() && temurin {
        return true;
    }
    let details = missing
        .iter()
        .map(|(tool, version)| format!("{tool}: {version}"))
        .collect::<Vec<_>>()
        .join("; ");
    if std::env::var("DOTTY_CLASSFILE_REQUIRE_JDK").as_deref() == Ok("1") {
        panic!(
            "strict conditional Rust consumer requires pinned Temurin JDK 21; {details}; Temurin={temurin}"
        );
    }
    eprintln!("skipping optional conditional Rust JDK client: {details}; Temurin={temurin}");
    false
}

fn run_java_probe(classes: &Path, runtime: &Path, args: &[&str]) -> Output {
    let classpath = std::env::join_paths([classes, runtime]).expect("join Java classpath");
    output(
        java_tool("java", runtime)
            .args(["-Xverify:all", "-ea", "-classpath"])
            .arg(classpath)
            .arg("example.RustRuntimeProbe")
            .args(args),
    )
}

fn assert_java_failure(output: &Output, stage: &str, evidence: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "{stage} unexpectedly passed: {stdout}{stderr}"
    );
    assert!(
        format!("{stdout}{stderr}").contains(evidence),
        "first failure stage {stage} lacks {evidence:?}: {stdout}{stderr}"
    );
}

#[test]
fn independent_java21_client_consumes_the_original_conditional_rust_batch() {
    let source_hash = sha256_bytes(SOURCE.as_bytes());
    let client_hash = sha256_bytes(JAVA_PROBE.as_bytes());
    let scala_support_hash = sha256_bytes(super::SCALA_SUPPORT_SOURCE.as_bytes());
    eprintln!(
        "conditional-source-sha256={source_hash} java-client-sha256={client_hash} scala-support-sha256={scala_support_hash}"
    );
    assert_eq!(source_hash, receipt::PRODUCER_SOURCE_SHA256);
    assert_eq!(client_hash, receipt::JAVA_CLIENT_SHA256);
    assert_eq!(scala_support_hash, receipt::SCALA_SUPPORT_SOURCE_SHA256);

    let original = emit_fresh_batch();
    let repeated = emit_fresh_batch();
    assert_eq!(
        original, repeated,
        "two independent Rust sessions match paths, bytes and UUID links"
    );
    let observed_hashes = original
        .files
        .iter()
        .map(|(path, bytes)| (path.as_str(), sha256_bytes(bytes)))
        .collect::<Vec<_>>();
    eprintln!("conditional-artifact-hashes={observed_hashes:?}");
    assert_eq!(
        observed_hashes,
        receipt::ARTIFACT_SHA256.map(|(path, hash)| (path, hash.to_owned()))
    );
    eprintln!(
        "conditional-rust-provenance source_sha256={source_hash} java_sha256={client_hash} paths={} uuids={:?}",
        original.files.len(),
        original.uuid_links
    );
    for (path, hash) in &observed_hashes {
        eprintln!("conditional-rust-artifact path={path} sha256={hash}");
    }

    let pair_factory_tasty = &original
        .files
        .iter()
        .find(|(path, _)| path == "example/PairFactory.tasty")
        .expect("PairFactory sidecar in original batch")
        .1;
    super::verify_rust_candidate_with_pinned_scala(pair_factory_tasty, "selected");

    if !require_jdk_21() {
        return;
    }
    assert_eq!(assert_factory_contract(&original), Ok(()));

    let scratch = TestDirectory::new("java");
    let root = scratch.path();
    let runtime = root.join("original-rust-batch");
    let java_source = root.join("RustRuntimeProbe.java");
    let java_classes = root.join("java-client");
    fs::create_dir_all(&runtime).expect("create runtime classpath");
    fs::create_dir_all(&java_classes).expect("create Java output directory");
    fs::write(&java_source, JAVA_PROBE).expect("write independent Java client");
    write_batch(&runtime, &original.files);

    let mut javac = java_tool("javac", root);
    successful(
        javac
            .args(["--release", "21", "-classpath"])
            .arg(&runtime)
            .arg("-d")
            .arg(&java_classes)
            .arg(&java_source),
    );
    let javap = successful(
        java_tool("javap", root)
            .args(["-v", "-c", "-p", "-classpath"])
            .arg(&runtime)
            .arg("example.PairFactory"),
    );
    let javap = String::from_utf8_lossy(&javap.stdout);
    for evidence in [
        "major version: 65",
        "descriptor: (IIZI)Lexample/Pair;",
        "stack=4, locals=8, args_size=5",
        "ifeq          29",
        "goto          31",
        "invokevirtual #",
        "example/Counter.set:(I)V",
        "StackMapTable: number_of_entries = 2",
        "frame_type = 255 /* full_frame */",
        "offset_delta = 29",
        "stack = [ class example/Counter ]",
    ] {
        assert!(
            javap.contains(evidence),
            "javap lacks {evidence:?}:\n{javap}"
        );
    }
    assert_eq!(
        javap
            .lines()
            .filter(|line| line.contains("new           ") && line.contains("class example/Counter"))
            .count(),
        2,
        "javap sees exactly two Counter allocation sites"
    );
    assert_eq!(
        javap
            .lines()
            .filter(
                |line| line.contains("invokevirtual") && line.contains("example/Counter.set:(I)V")
            )
            .count(),
        1,
        "javap sees exactly one selected Counter.set call site"
    );
    let java = successful(
        java_tool("java", root)
            .args(["-Xverify:all", "-ea", "-classpath"])
            .arg(std::env::join_paths([java_classes.as_path(), runtime.as_path()]).unwrap())
            .arg("example.RustRuntimeProbe"),
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "conditional-rust-runtime=true:42/3,false:7/42,negative:0/42,-9/0,equal:42/7,identity=original-distinct,fresh=true"
    );

    // A valid but semantically flipped conditional must fail the paired artifact contract and
    // the independent runtime; keep the corruption out of the original six-file batch.
    let factory_index = original
        .files
        .iter()
        .position(|(path, _)| path == "example/PairFactory.class")
        .unwrap();
    let mut wrong_branch = original.files.clone();
    let class = decode_factory(&original);
    let method = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index).ok() == Some("create"))
        .expect("create method");
    let code = method
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::Code(code) => Some(code.code.to_vec()),
            _ => None,
        })
        .expect("create bytecode");
    assert_eq!(code[21], 0x99);
    // The conditional opcode is in the classfile payload; locate its unique IF in create.
    let offset = factory_bytes(&original)
        .windows(code.len())
        .position(|window| window == code)
        .expect("locate create code bytes in classfile");
    wrong_branch[factory_index].1[offset + 21] = 0x9a; // ifne: verifier-valid, opposite choice
    let wrong_batch = RustBatch {
        files: wrong_branch,
        uuid_links: original.uuid_links.clone(),
    };
    let wrong_branch_contract = assert_factory_contract(&wrong_batch).unwrap_err();
    assert!(
        wrong_branch_contract.contains("conditional/join branch shape differs"),
        "wrong branch fails the paired artifact contract first: {wrong_branch_contract}"
    );
    let wrong_runtime = root.join("negative-wrong-branch");
    fs::create_dir_all(&wrong_runtime).expect("create isolated wrong-branch classpath");
    write_batch(&wrong_runtime, &wrong_batch.files);
    let wrong_java = run_java_probe(&java_classes, &wrong_runtime, &[]);
    assert_java_failure(
        &wrong_java,
        "verifier-valid wrong branch runtime assertion",
        "expected 42/3",
    );

    let mut wrong_slot = original.files.clone();
    wrong_slot[factory_index].1[offset + 25] = 6; // true arm loads second instead of first
    let wrong_slot_batch = RustBatch {
        files: wrong_slot,
        uuid_links: original.uuid_links.clone(),
    };
    let wrong_slot_contract = assert_factory_contract(&wrong_slot_batch).unwrap_err();
    assert!(
        wrong_slot_contract.contains("condition arms, selected slot"),
        "wrong selected slot fails the paired artifact contract: {wrong_slot_contract}"
    );
    let wrong_slot_runtime = root.join("negative-wrong-selected-slot");
    fs::create_dir_all(&wrong_slot_runtime).expect("create isolated wrong-slot classpath");
    write_batch(&wrong_slot_runtime, &wrong_slot_batch.files);
    let wrong_slot_java = run_java_probe(&java_classes, &wrong_slot_runtime, &[]);
    assert_java_failure(
        &wrong_slot_java,
        "verifier-valid wrong selected slot runtime assertion",
        "expected 42/3",
    );

    // Bad branch target and selected read-before-initialize are verifier failures.
    for (label, mutate) in [("invalid-branch", 0u8), ("read-before-initialize", 1u8)] {
        let mut corrupted = original.files.clone();
        let classfile = &mut corrupted[factory_index].1;
        if mutate == 0 {
            classfile[offset + 22] = 0x7f;
            classfile[offset + 23] = 0xff;
        } else {
            assert_eq!(classfile[offset + 24..offset + 26], [0x19, 5]);
            classfile[offset + 25] = 7;
        }
        let directory = root.join(format!("negative-{label}"));
        fs::create_dir_all(&directory).expect("create isolated verifier-failure classpath");
        write_batch(&directory, &corrupted);
        let java = run_java_probe(&java_classes, &directory, &[]);
        assert_java_failure(&java, "JVM bytecode verification", "VerifyError");
    }
    assert_eq!(
        original, repeated,
        "corrupt controls leave canonical Rust output unchanged"
    );
    assert_eq!(
        original.files.len(),
        6,
        "no negative control publishes a partial batch"
    );
}
