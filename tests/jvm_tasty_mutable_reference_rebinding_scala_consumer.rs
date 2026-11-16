use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_batch;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::reader::Reader as ClassReader;
use dotty_core::SourceId;
use dotty_tasty::ast::{ASSIGN_TAG, DefinitionBody, MUTABLE_TAG, VALDEF_TAG};
use dotty_tasty::file::TastyFile;
use dotty_tasty::writer::Writer as TastyWriter;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

#[path = "support/mutable_reference_rebinding_artifacts.rs"]
mod receipt;
#[path = "support/scala39_toolchain.rs"]
mod scala39_toolchain;

const REQUIRE: &str = "DOTTY_REQUIRE_SCALA_TASTY_MUTABLE_REFERENCE_REBINDING_SCALA_CONSUMER";
const SOURCE: &str =
    include_str!("fixtures/jvm_tasty_mutable_reference_rebinding/MutableReferenceRebinding.scala");
const CLIENT: &str =
    include_str!("fixtures/jvm_tasty_mutable_reference_rebinding/ScalaConsumer.scala");
const PATHS: [&str; 6] = [
    "example/Counter.class",
    "example/Counter.tasty",
    "example/Pair.class",
    "example/Pair.tasty",
    "example/PairFactory.class",
    "example/PairFactory.tasty",
];

fn output(command: &mut Command) -> Output {
    let rendered = format!("{command:?}");
    let result = command
        .output()
        .unwrap_or_else(|error| panic!("run {rendered}: {error}"));
    assert!(
        result.status.success(),
        "{rendered}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    result
}

fn attempt(command: &mut Command) -> Output {
    let rendered = format!("{command:?}");
    command
        .output()
        .unwrap_or_else(|error| panic!("run {rendered}: {error}"))
}

fn combined(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn first_diagnostic(output: &Output) -> String {
    combined(output)
        .lines()
        .find(|line| !line.starts_with("Picked up JAVA_TOOL_OPTIONS:"))
        .unwrap_or("no diagnostic")
        .to_owned()
}

fn require_compiler_success(success: bool, control: &str, diagnostic: &str) -> Result<(), String> {
    if success {
        Ok(())
    } else {
        Err(format!(
            "{control} Scala compilation unexpectedly failed: {diagnostic}"
        ))
    }
}

#[test]
fn descriptor_control_rejects_unrelated_compiler_failure() {
    let injected = "INJECTED: unrelated compiler infrastructure failure";
    let result = require_compiler_success(false, "descriptor disagreement", injected);

    assert!(
        result.unwrap_err().contains(injected),
        "unrelated compiler failure cannot satisfy the descriptor control"
    );
}

fn sha256(path: &Path) -> String {
    String::from_utf8_lossy(&output(Command::new("sha256sum").arg(path)).stdout)
        .split_whitespace()
        .next()
        .expect("sha256sum output")
        .to_owned()
}

fn task_dir() -> PathBuf {
    let root = PathBuf::from(std::env::var_os("TMPDIR").expect("TMPDIR is managed scratch"));
    root.join(format!(
        "dotty-mutable-scala-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn copy_batch(root: &Path, files: &[(String, Vec<u8>)], omit: Option<&str>) {
    for (relative, bytes) in files {
        if Some(relative.as_str()) == omit {
            continue;
        }
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
}

fn configure_jdk(command: &mut Command, java_home: &Path, tmp: &Path) {
    command.env("JAVA_HOME", java_home).env(
        "JAVA_TOOL_OPTIONS",
        format!("-XX:-UsePerfData -Djava.io.tmpdir={}", tmp.display()),
    );
}

fn printed_body(line: &str) -> Option<&str> {
    line.trim().split_once(": ").map(|(_, body)| body.trim())
}

fn printed_address(line: &str) -> Option<u32> {
    line.trim()
        .split_once(": ")
        .and_then(|(address, _)| address.parse().ok())
}

fn direct_targets(lines: &[&str], from: usize, to: usize) -> Vec<u32> {
    lines[from..to]
        .iter()
        .filter_map(|line| {
            printed_body(line)
                .and_then(|body| body.strip_prefix("TERMREFdirect "))
                .and_then(|target| target.parse().ok())
        })
        .collect()
}

fn mutate_reference(bytes: &[u8], assignment_side: bool) -> Vec<u8> {
    let tasty = TastyFile::parse_and_validate_scala_3_9(bytes).unwrap();
    let index = tasty.ast_address_index().unwrap();
    let asts = tasty
        .section(dotty_tasty::section::StandardSection::Asts)
        .unwrap();
    let node = index
        .iter_nodes_with_tag(ASSIGN_TAG)
        .next()
        .expect("selected assignment");
    let raw = index.get(node.offset as u32).unwrap();
    let assign = raw.decode_assign().unwrap();
    let tree = if assignment_side {
        assign.right
    } else {
        assign.left
    };
    let reference = tree
        .ast_refs()
        .into_iter()
        .next()
        .expect("assignment reference");
    let invalid = reference.address + 1;
    assert!(index.get_node(invalid).is_none());
    let mut old = TastyWriter::default();
    old.write_nat(reference.address);
    let mut new = TastyWriter::default();
    new.write_nat(invalid);
    assert_eq!(old.as_slice().len(), new.as_slice().len());
    let tree_offset = tree.nodes().first().expect("reference node").offset;
    let file_offset = asts.payload.as_ptr() as usize - bytes.as_ptr() as usize
        + (raw.payload.as_ptr() as usize - asts.payload.as_ptr() as usize)
        + tree_offset
        + 1;
    let mut corrupted = bytes.to_vec();
    corrupted[file_offset..file_offset + new.as_slice().len()].copy_from_slice(new.as_slice());
    TastyFile::parse_and_validate_scala_3_9(&corrupted)
        .expect("reference corruption preserves framing");
    corrupted
}

fn corrupt_mutable_modifier(bytes: &[u8], tasty: &TastyFile<'_>) -> Vec<u8> {
    let index = tasty.ast_address_index().unwrap();
    let selected = index.iter_nodes_with_tag(VALDEF_TAG).find(|node| {
        index.get(node.offset as u32).and_then(|raw| raw.decode_definition_body().ok()).is_some_and(|body| {
            matches!(body, DefinitionBody::ValDef { name, .. } if tasty.names().render(name).ok().as_deref() == Some("selected"))
        })
    }).expect("selected ValDef");
    let raw = index.get(selected.offset as u32).unwrap();
    let body = raw.decode_definition_body().unwrap();
    let DefinitionBody::ValDef { tail, .. } = body else {
        panic!("selected is ValDef")
    };
    assert!(tail.contains(&dotty_tasty::ast::DefinitionTail::Modifier(MUTABLE_TAG)));
    // The last MUTABLE tag in this bounded VALDEF payload is its modifier; tag
    // replacement is one byte and leaves every enclosing TASTy length intact.
    let position = raw
        .payload
        .iter()
        .rposition(|byte| *byte == MUTABLE_TAG)
        .expect("mutable tag byte in selected definition");
    let file_offset = raw.payload.as_ptr() as usize - bytes.as_ptr() as usize + position;
    let mut corrupted = bytes.to_vec();
    corrupted[file_offset] = dotty_tasty::ast::FINAL_TAG;
    let reparsed = TastyFile::parse_and_validate_scala_3_9(&corrupted)
        .expect("modifier corruption preserves the sidecar structure");
    let reparsed_index = reparsed.ast_address_index().unwrap();
    let selected = reparsed_index
        .iter_nodes_with_tag(VALDEF_TAG)
        .find(|node| {
            reparsed_index
                .get(node.offset as u32)
                .and_then(|raw| raw.decode_definition_body().ok())
                .is_some_and(|body| {
                    matches!(body, DefinitionBody::ValDef { name, tail, .. }
                        if reparsed.names().render(name).ok().as_deref() == Some("selected")
                            && !tail.contains(&dotty_tasty::ast::DefinitionTail::Modifier(MUTABLE_TAG)))
                })
        })
        .expect("selected remains a val without the mutable modifier");
    let _ = selected;
    corrupted
}

fn write_batch(root: &Path, files: &[(String, Vec<u8>)]) {
    copy_batch(root, files, None);
}

#[test]
fn scala39_consumes_original_rust_mutable_reference_rebinding_batch() {
    match std::env::var(REQUIRE).as_deref() {
        Ok("1") => {}
        Ok(value) => panic!("{REQUIRE} must equal 1 when set, got {value:?}"),
        Err(std::env::VarError::NotPresent) => {
            eprintln!("SKIP strict Scala mutable reference consumer gate is not required");
            return;
        }
        Err(error) => panic!("read {REQUIRE}: {error}"),
    }
    assert_eq!(
        sha256(Path::new(
            "tests/fixtures/jvm_tasty_mutable_reference_rebinding/MutableReferenceRebinding.scala"
        )),
        receipt::PRODUCER_SOURCE_SHA256
    );
    assert_eq!(
        sha256(Path::new(
            "tests/fixtures/jvm_tasty_mutable_reference_rebinding/ScalaConsumer.scala"
        )),
        receipt::CLIENT_SHA256
    );
    let batch = emit_paired_jvm_tasty_batch(
        SOURCE,
        "MutableReferenceRebinding.scala",
        SourceId::from_index(23_110),
    )
    .expect("original source passes true Rust frontend and emits frozen batch");
    assert_eq!(batch.classes().len(), 3);
    let mut files = Vec::new();
    let mut uuids = Vec::new();
    for class in batch.classes() {
        files.push((
            class.classfile_path().to_owned(),
            class.classfile_bytes().to_vec(),
        ));
        files.push((class.tasty_path().to_owned(), class.tasty_bytes().to_vec()));
        let class_bytes = class.classfile_bytes();
        let tasty_bytes = class.tasty_bytes();
        let mut reader = ClassReader::new(class_bytes);
        let decoded = ClassFile::decode(&mut reader).unwrap();
        assert!(reader.is_at_end());
        assert_eq!(decoded.version.major, 65);
        if class.classfile_path() == "example/PairFactory.class" {
            let method = decoded
                .methods
                .iter()
                .find(|method| decoded.constant_pool.utf8(method.name_index).ok() == Some("create"))
                .expect("original PairFactory.create method");
            assert_eq!(
                decoded.constant_pool.utf8(method.descriptor_index).unwrap(),
                "(III)Lexample/Pair;"
            );
        }
        let tasty = TastyFile::parse_and_validate_scala_3_9(tasty_bytes).unwrap();
        assert_eq!(
            (tasty.header().major_version, tasty.header().minor_version),
            (28, 9)
        );
        assert_eq!(
            decoded.tasty_link_id().unwrap(),
            Some(class.tasty_link_id())
        );
        assert_eq!(tasty.header().uuid, *class.tasty_link_id().as_bytes());
        let uuid = class
            .tasty_link_id()
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        uuids.push((class.classfile_path().to_owned(), uuid));
    }
    assert_eq!(
        files
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        PATHS
    );
    assert_eq!(
        uuids,
        receipt::UUIDS.map(|(path, uuid)| (path.to_owned(), uuid.to_owned()))
    );
    let root = task_dir();
    let artifacts = root.join("rust-artifacts");
    let client_out = root.join("scala-client-classes");
    fs::create_dir_all(&client_out).unwrap();
    write_batch(&artifacts, &files);
    for (path, expected) in receipt::ARTIFACT_SHA256 {
        assert_eq!(
            sha256(&artifacts.join(path)),
            expected,
            "canonical Rust artifact {path}"
        );
    }
    let pinned = scala39_toolchain::from_environment(output, |command| {
        command.env(
            "JAVA_HOME",
            std::env::var_os("JAVA_HOME").expect("JAVA_HOME"),
        );
        command.env(
            "JAVA_TOOL_OPTIONS",
            format!(
                "-XX:-UsePerfData -Djava.io.tmpdir={}",
                std::env::var("TMPDIR").unwrap()
            ),
        );
    });
    let source = root.join("ScalaConsumer.scala");
    fs::write(&source, CLIENT).unwrap();
    let classpath = format!("{}:{}", artifacts.display(), pinned.libraries);
    output(
        scala39_toolchain::scalac(
            &pinned.java_home,
            &pinned.scalac,
            PathBuf::from(std::env::var_os("TMPDIR").unwrap()),
        )
        .args(["-Werror", "-release", "21", "-classpath"])
        .arg(&classpath)
        .arg("-d")
        .arg(&client_out)
        .arg(&source),
    );
    for forbidden in ["Counter.class", "Pair.class", "PairFactory.class"] {
        assert!(
            !client_out.join("example").join(forbidden).exists(),
            "consumer compilation must not emit fallback producer class {forbidden}"
        );
    }
    let mut run = Command::new(&pinned.java);
    run.args(["-Xverify:all", "-ea", "-cp"])
        .arg(format!(
            "{}:{}:{}",
            client_out.display(),
            artifacts.display(),
            pinned.libraries
        ))
        .arg("consumer.ScalaConsumer");
    configure_jdk(
        &mut run,
        &pinned.java_home,
        Path::new(&std::env::var("TMPDIR").unwrap()),
    );
    assert_eq!(
        String::from_utf8_lossy(&output(&mut run).stdout).trim(),
        "scala39-rust-mutable-reference-rebinding=ok"
    );
    for (path, expected) in receipt::ARTIFACT_SHA256 {
        assert_eq!(
            sha256(&artifacts.join(path)),
            expected,
            "Scala client leaves original {path} unchanged"
        );
    }

    let factory_tasty = artifacts.join("example/PairFactory.tasty");
    let printed = output(
        scala39_toolchain::scalac(
            &pinned.java_home,
            &pinned.scalac,
            PathBuf::from(std::env::var_os("TMPDIR").unwrap()),
        )
        .args(["-color:never", "-print-tasty"])
        .arg(&factory_tasty),
    );
    let printed = String::from_utf8_lossy(&printed.stdout).into_owned();
    for evidence in [
        "version: 28.9.0",
        "[first]",
        "[second]",
        "[selected]",
        "MUTABLE",
        "ASSIGN(",
        "[set[",
    ] {
        assert!(
            printed.contains(evidence),
            "official Scala reader omitted {evidence}:\n{printed}"
        );
    }
    let lines = printed.lines().collect::<Vec<_>>();
    let first_address = lines
        .iter()
        .find(|line| {
            printed_body(line)
                .is_some_and(|body| body.starts_with("VALDEF(") && body.ends_with("[first]"))
        })
        .and_then(|line| printed_address(line))
        .expect("first VALDEF address");
    let second_address = lines
        .iter()
        .find(|line| {
            printed_body(line)
                .is_some_and(|body| body.starts_with("VALDEF(") && body.ends_with("[second]"))
        })
        .and_then(|line| printed_address(line))
        .expect("second VALDEF address");
    let selected_address = lines
        .iter()
        .find(|line| {
            printed_body(line)
                .is_some_and(|body| body.starts_with("VALDEF(") && body.ends_with("[selected]"))
        })
        .and_then(|line| printed_address(line))
        .expect("selected VALDEF address");
    let next_value_address = lines
        .iter()
        .find(|line| {
            printed_body(line)
                .is_some_and(|body| body.starts_with("PARAM(") && body.ends_with("[nextValue]"))
        })
        .and_then(|line| printed_address(line))
        .expect("nextValue PARAM address");
    let block_start = lines
        .iter()
        .position(|line| printed_body(line).is_some_and(|body| body.starts_with("BLOCK(")))
        .expect("ordered factory BLOCK");
    let first_line = lines
        .iter()
        .position(|line| printed_address(line) == Some(first_address))
        .unwrap();
    let second_line = lines
        .iter()
        .position(|line| printed_address(line) == Some(second_address))
        .unwrap();
    let selected_line = lines
        .iter()
        .position(|line| printed_address(line) == Some(selected_address))
        .unwrap();
    let assign = lines
        .iter()
        .position(|line| printed_body(line).is_some_and(|body| body.starts_with("ASSIGN(")))
        .unwrap();
    let setter = lines
        .iter()
        .position(|line| printed_body(line).is_some_and(|body| body.contains("[set[")))
        .unwrap();
    assert!(
        block_start < first_line
            && first_line < second_line
            && second_line < selected_line
            && selected_line < assign
            && assign < setter,
        "Scala reader preserves source order:\n{printed}"
    );
    assert_eq!(
        direct_targets(&lines, selected_line + 1, assign),
        [first_address],
        "selected initializer resolves to first"
    );
    assert_eq!(
        direct_targets(&lines, assign + 1, setter),
        [selected_address, second_address],
        "ASSIGN LHS/RHS resolve to selected/second"
    );
    assert_eq!(
        direct_targets(&lines, setter, lines.len()),
        [selected_address, next_value_address],
        "setter receiver and argument resolve to selected/nextValue"
    );
    assert_eq!(
        direct_targets(&lines, block_start + 1, first_line),
        [first_address, second_address],
        "Pair result preserves first/second argument identities"
    );
    let decompiled = output(
        scala39_toolchain::scalac(
            &pinned.java_home,
            &pinned.scalac,
            PathBuf::from(std::env::var_os("TMPDIR").unwrap()),
        )
        .args(["-color:never", "-decompile"])
        .arg(&factory_tasty),
    );
    let decompiled = String::from_utf8_lossy(&decompiled.stdout).into_owned();
    for evidence in [
        "var selected: example.Counter = first",
        "selected = second",
        "selected.set(nextValue)",
        "new example.Pair(first, second)",
    ] {
        assert!(
            decompiled.contains(evidence),
            "official decompiler omitted {evidence}:\n{decompiled}"
        );
    }
    let from_tasty = root.join("from-tasty");
    fs::create_dir_all(&from_tasty).unwrap();
    output(
        scala39_toolchain::scalac(
            &pinned.java_home,
            &pinned.scalac,
            PathBuf::from(std::env::var_os("TMPDIR").unwrap()),
        )
        .args(["-release", "21", "-from-tasty", "-classpath"])
        .arg(&classpath)
        .arg("-d")
        .arg(&from_tasty)
        .args(
            ["Counter", "Pair", "PairFactory"]
                .map(|name| artifacts.join(format!("example/{name}.tasty"))),
        ),
    );
    for name in ["Counter", "Pair", "PairFactory"] {
        assert!(from_tasty.join(format!("example/{name}.class")).is_file());
    }

    // Negative copies are isolated from the positive directory; classify the
    // first stage that actually rejects each corruption.
    let missing = root.join("negative-missing-sidecar");
    copy_batch(&missing, &files, Some("example/PairFactory.tasty"));
    let missing_out = root.join("missing-out");
    fs::create_dir_all(&missing_out).unwrap();
    let failed = attempt(
        scala39_toolchain::scalac(
            &pinned.java_home,
            &pinned.scalac,
            PathBuf::from(std::env::var_os("TMPDIR").unwrap()),
        )
        .args(["-Werror", "-release", "21", "-classpath"])
        .arg(format!("{}:{}", missing.display(), pinned.libraries))
        .arg("-d")
        .arg(&missing_out)
        .arg(&source),
    );
    assert!(
        !failed.status.success(),
        "missing sidecar rejected by Scala consumer"
    );
    assert!(
        combined(&failed).contains("file is missing"),
        "missing sidecar first fails during Scala source linking: {}",
        first_diagnostic(&failed)
    );
    eprintln!(
        "SCALA_NEGATIVE_FIRST_FAILURE missing-sidecar: {}",
        first_diagnostic(&failed)
    );

    let wrong_uuid = root.join("negative-wrong-uuid");
    copy_batch(&wrong_uuid, &files, None);
    let uuid_out = root.join("uuid-out");
    fs::create_dir_all(&uuid_out).unwrap();
    let tasty_path = wrong_uuid.join("example/PairFactory.tasty");
    let mut bytes = fs::read(&tasty_path).unwrap();
    let tasty = TastyFile::parse_and_validate_scala_3_9(&bytes).unwrap();
    let uuid_offset = bytes
        .windows(tasty.header().uuid.len())
        .position(|window| window == tasty.header().uuid)
        .unwrap();
    bytes[uuid_offset] ^= 1;
    fs::write(tasty_path, bytes).unwrap();
    let failed = attempt(
        scala39_toolchain::scalac(
            &pinned.java_home,
            &pinned.scalac,
            PathBuf::from(std::env::var_os("TMPDIR").unwrap()),
        )
        .args(["-Werror", "-release", "21", "-classpath"])
        .arg(format!("{}:{}", wrong_uuid.display(), pinned.libraries))
        .arg("-d")
        .arg(&uuid_out)
        .arg(&source),
    );
    assert!(
        !failed.status.success(),
        "wrong UUID rejected by Scala consumer"
    );
    assert!(
        first_diagnostic(&failed).contains("out of sync"),
        "UUID mismatch is rejected by the Scala linker: {}",
        first_diagnostic(&failed)
    );
    eprintln!(
        "SCALA_NEGATIVE_FIRST_FAILURE wrong-uuid: {}",
        first_diagnostic(&failed)
    );

    for (label, corrupt) in [
        (
            "assignment-lhs",
            mutate_reference(&fs::read(&factory_tasty).unwrap(), false),
        ),
        (
            "assignment-rhs",
            mutate_reference(&fs::read(&factory_tasty).unwrap(), true),
        ),
    ] {
        let negative = root.join(format!("negative-{label}"));
        copy_batch(&negative, &files, None);
        let path = negative.join("example/PairFactory.tasty");
        fs::write(&path, corrupt).unwrap();
        let failed = attempt(
            scala39_toolchain::scalac(
                &pinned.java_home,
                &pinned.scalac,
                PathBuf::from(std::env::var_os("TMPDIR").unwrap()),
            )
            .args(["-color:never", "-decompile"])
            .arg(&path),
        );
        assert!(
            !failed.status.success(),
            "official Scala decompiler rejects {label}"
        );
        let report = combined(&failed);
        assert!(
            report.contains("cannot be unpickled")
                || (report.contains("during phase: readTasty")
                    && report.contains("TreeWithoutOwner")),
            "{label} must fail in the Scala TASTy reader/unpickler, got: {report}"
        );
        eprintln!(
            "SCALA_NEGATIVE_FIRST_FAILURE {label}: {}",
            if report.contains("TreeWithoutOwner") {
                "official Scala readTasty: TreeWithoutOwner".to_owned()
            } else {
                first_diagnostic(&failed)
            }
        );
    }

    let mut original = fs::read(&factory_tasty).unwrap();
    let tasty = TastyFile::parse_and_validate_scala_3_9(&original).unwrap();
    let malformed_modifier = corrupt_mutable_modifier(&original, &tasty);
    original.clear();
    let negative_modifier = root.join("negative-mutable-modifier");
    copy_batch(&negative_modifier, &files, None);
    let path = negative_modifier.join("example/PairFactory.tasty");
    fs::write(&path, malformed_modifier).unwrap();
    let failed = attempt(
        scala39_toolchain::scalac(
            &pinned.java_home,
            &pinned.scalac,
            PathBuf::from(std::env::var_os("TMPDIR").unwrap()),
        )
        .args(["-color:never", "-decompile"])
        .arg(&path),
    );
    if failed.status.success() {
        assert!(
            String::from_utf8_lossy(&failed.stdout).contains("val selected"),
            "official decompiler exposes stale mutable modifier as an immutable val"
        );
    } else {
        let report = combined(&failed);
        assert!(
            report.contains("cannot be unpickled")
                || (report.contains("during phase: readTasty")
                    && report.contains("TreeWithoutOwner")),
            "malformed mutable modifier fails in the TASTy reader/unpickler: {report}"
        );
    }
    eprintln!(
        "SCALA_NEGATIVE_FIRST_FAILURE mutable-modifier: {}",
        if failed.status.success() {
            "official decompiler accepted stale modifier; semantic witness is val selected"
                .to_owned()
        } else {
            if combined(&failed).contains("TreeWithoutOwner") {
                "official Scala readTasty: TreeWithoutOwner".to_owned()
            } else {
                first_diagnostic(&failed)
            }
        }
    );

    let mismatch = root.join("negative-descriptor");
    copy_batch(&mismatch, &files, None);
    let class = mismatch.join("example/PairFactory.class");
    let mut bytes = fs::read(&class).unwrap();
    let old = b"(III)Lexample/Pair;";
    let new = b"(IIZ)Lexample/Pair;";
    let offset = bytes
        .windows(old.len())
        .position(|window| window == old)
        .expect("Rust method descriptor");
    bytes[offset..offset + old.len()].copy_from_slice(new);
    fs::write(class, bytes).unwrap();
    let mismatch_out = root.join("descriptor-out");
    fs::create_dir_all(&mismatch_out).unwrap();
    let compile = attempt(
        scala39_toolchain::scalac(
            &pinned.java_home,
            &pinned.scalac,
            PathBuf::from(std::env::var_os("TMPDIR").unwrap()),
        )
        .args(["-Werror", "-release", "21", "-classpath"])
        .arg(format!("{}:{}", mismatch.display(), pinned.libraries))
        .arg("-d")
        .arg(&mismatch_out)
        .arg(&source),
    );
    require_compiler_success(
        compile.status.success(),
        "descriptor disagreement",
        &first_diagnostic(&compile),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let mut run = Command::new(&pinned.java);
    run.args(["-Xverify:all", "-ea", "-cp"])
        .arg(format!(
            "{}:{}:{}",
            mismatch_out.display(),
            mismatch.display(),
            pinned.libraries
        ))
        .arg("consumer.ScalaConsumer");
    configure_jdk(
        &mut run,
        &pinned.java_home,
        Path::new(&std::env::var("TMPDIR").unwrap()),
    );
    let failed = attempt(&mut run);
    assert!(
        !failed.status.success(),
        "descriptor mismatch fails at JVM linkage"
    );
    assert!(
        combined(&failed).contains("NoSuchMethodError"),
        "descriptor mismatch first fails at JVM linkage: {}",
        first_diagnostic(&failed)
    );
    eprintln!(
        "SCALA_NEGATIVE_FIRST_FAILURE descriptor-disagreement: {}",
        first_diagnostic(&failed)
    );

    for (path, expected) in receipt::ARTIFACT_SHA256 {
        assert_eq!(
            sha256(&artifacts.join(path)),
            expected,
            "positive canonical artifact remains unchanged: {path}"
        );
    }
    fs::remove_dir_all(root).unwrap();
}
