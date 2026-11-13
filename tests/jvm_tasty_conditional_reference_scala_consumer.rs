use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_batch;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::reader::Reader as ClassReader;
use dotty_core::SourceId;
use dotty_tasty::ast::{DefinitionBody, IF_TAG, SELECTIN_TAG, VALDEF_TAG};
use dotty_tasty::file::TastyFile;
use dotty_tasty::term::{AstRefKind, RawTree};
use dotty_tasty::writer::Writer as TastyWriter;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

#[path = "support/conditional_reference_artifacts.rs"]
mod receipt;
#[path = "support/scala39_toolchain.rs"]
mod scala39_toolchain;

const REQUIRE: &str = "DOTTY_REQUIRE_SCALA_TASTY_CONDITIONAL_REFERENCE_SCALA_CONSUMER";
const SOURCE: &str =
    include_str!("fixtures/jvm_tasty_conditional_reference_selection/PairFactory.scala");
const CLIENT: &str =
    include_str!("fixtures/jvm_tasty_conditional_reference_selection/ScalaConsumer.scala");
const CLIENT_SHA256: &str = "d4668a27c5c660debd71d94a7ce117edec82b14c070dbae70c9edeb764df15c1";
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

fn first_diagnostic(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .lines()
    .find(|line| !line.starts_with("Picked up JAVA_TOOL_OPTIONS:"))
    .unwrap_or("no diagnostic")
    .to_owned()
}

fn complete_output(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn single_ast_reference(tree: &RawTree<'_>, kind: AstRefKind) -> u32 {
    let references = tree.ast_refs();
    let [reference] = references.as_slice() else {
        panic!("one AST reference expected, got {references:?}");
    };
    assert_eq!(reference.kind, kind);
    reference.address
}

fn corrupt_if_arm(bytes: &[u8]) -> Vec<u8> {
    let tasty = TastyFile::parse_and_validate_scala_3_9(bytes).unwrap();
    let index = tasty.ast_address_index().unwrap();
    let asts = tasty
        .section(dotty_tasty::section::StandardSection::Asts)
        .unwrap();
    let selected = index
        .iter_nodes_with_tag(VALDEF_TAG)
        .find(|node| {
            index
                .get(node.offset as u32)
                .and_then(|raw| raw.decode_definition_body().ok())
                .is_some_and(|body| {
                    matches!(body, DefinitionBody::ValDef { name, .. } if tasty.names().render(name).ok().as_deref() == Some("selected"))
                })
        })
        .expect("selected VALDEF");
    let if_node = index
        .children_of(selected.offset as u32)
        .find(|node| node.tag == IF_TAG)
        .expect("selected local owns IF");
    let raw_if = index.get(if_node.offset as u32).unwrap();
    let decoded_if = raw_if.decode_if().unwrap();
    let old = single_ast_reference(&decoded_if.then_branch, AstRefKind::SharedType);
    let stale = old + 1;
    assert!(index.get_node(stale).is_none());
    let mut encoded_old = TastyWriter::default();
    encoded_old.write_nat(old);
    let mut encoded_stale = TastyWriter::default();
    encoded_stale.write_nat(stale);
    assert_eq!(encoded_old.as_slice().len(), encoded_stale.as_slice().len());
    let reference_offset = decoded_if.then_branch.nodes()[0].offset;
    let if_offset = raw_if.payload.as_ptr() as usize - asts.payload.as_ptr() as usize;
    let file_asts_offset = asts.payload.as_ptr() as usize - bytes.as_ptr() as usize;
    let reference_file_offset = file_asts_offset + if_offset + reference_offset + 1;
    let mut corrupted = bytes.to_vec();
    corrupted[reference_file_offset..reference_file_offset + encoded_stale.as_slice().len()]
        .copy_from_slice(encoded_stale.as_slice());
    TastyFile::parse_and_validate_scala_3_9(&corrupted)
        .expect("stale arm retains valid TASTy framing");
    corrupted
}

fn corrupt_selected_valdef_reference(bytes: &[u8]) -> Vec<u8> {
    let tasty = TastyFile::parse_and_validate_scala_3_9(bytes).unwrap();
    let index = tasty.ast_address_index().unwrap();
    let selected = index
        .iter_nodes_with_tag(VALDEF_TAG)
        .find(|node| {
            index
                .get(node.offset as u32)
                .and_then(|raw| raw.decode_definition_body().ok())
                .is_some_and(|body| {
                    matches!(body, DefinitionBody::ValDef { name, .. } if tasty.names().render(name).ok().as_deref() == Some("selected"))
                })
        })
        .expect("selected VALDEF");
    let selected_address = selected.offset as u32;
    let setter = index
        .iter_nodes_with_tag(SELECTIN_TAG)
        .find_map(|node| {
            let raw = index.get(node.offset as u32)?;
            let selection = raw.decode_select_in().ok()?;
            let signed = tasty.names().render_signed_name(selection.name).ok()??;
            matches!(signed, dotty_tasty::tasty::RenderedSignedName::Signed { original, .. } if original == "set")
                .then_some(selection)
        })
        .expect("signed setter SELECTin");
    let old_address = single_ast_reference(&setter.qualifier, AstRefKind::TermRefDirect);
    assert_eq!(old_address, selected_address);
    let invalid_address = old_address + 1;
    assert!(index.get_node(invalid_address).is_none());
    let mut encoded_old = TastyWriter::default();
    encoded_old.write_nat(old_address);
    let mut encoded_invalid = TastyWriter::default();
    encoded_invalid.write_nat(invalid_address);
    assert_eq!(
        encoded_old.as_slice().len(),
        encoded_invalid.as_slice().len()
    );
    let asts = tasty
        .section(dotty_tasty::section::StandardSection::Asts)
        .unwrap();
    let setter_node = index
        .iter_nodes_with_tag(SELECTIN_TAG)
        .find(|node| {
            index
                .get(node.offset as u32)
                .and_then(|raw| raw.decode_select_in().ok())
                .is_some_and(|selection| {
                    let signed = tasty.names().render_signed_name(selection.name).ok().flatten();
                    matches!(signed, Some(dotty_tasty::tasty::RenderedSignedName::Signed { original, .. }) if original == "set")
                })
        })
        .expect("signed setter SELECTin");
    let setter_node = index.get(setter_node.offset as u32).unwrap();
    let mut encoded_qualifier = TastyWriter::default();
    setter
        .qualifier
        .encode(&mut encoded_qualifier)
        .expect("encode parsed setter qualifier for isolated corruption");
    let qualifier_offset = setter_node
        .payload
        .windows(encoded_qualifier.as_slice().len())
        .position(|window| window == encoded_qualifier.as_slice())
        .expect("setter qualifier bytes occur in SELECTin payload");
    let payload_offset = setter_node.payload.as_ptr() as usize - asts.payload.as_ptr() as usize;
    let offset = asts.offset + payload_offset + qualifier_offset + 1;
    let mut corrupted = bytes.to_vec();
    corrupted[offset..offset + encoded_invalid.as_slice().len()]
        .copy_from_slice(encoded_invalid.as_slice());
    TastyFile::parse_and_validate_scala_3_9(&corrupted)
        .expect("bad local reference preserves TASTy framing");
    corrupted
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

fn sha256(path: &Path) -> String {
    String::from_utf8_lossy(&output(Command::new("sha256sum").arg(path)).stdout)
        .split_whitespace()
        .next()
        .expect("sha256sum output")
        .to_owned()
}

fn task_dir() -> PathBuf {
    let root = PathBuf::from(std::env::var_os("TMPDIR").expect("TMPDIR must be task-local"));
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    root.join(format!(
        "dotty-conditional-scala-consumer-{}-{suffix}",
        std::process::id()
    ))
}

fn write_batch(root: &Path, files: &[(String, Vec<u8>)]) {
    for (relative, bytes) in files {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("artifact parent")).unwrap();
        fs::write(path, bytes).unwrap();
    }
}

#[test]
fn scala39_compiles_and_runs_against_the_original_rust_conditional_batch() {
    match std::env::var(REQUIRE).as_deref() {
        Ok("1") => {}
        Ok(value) => panic!("{REQUIRE} must equal 1 when set, got {value:?}"),
        Err(std::env::VarError::NotPresent) => {
            eprintln!(
                "SKIP Scala 3.9 Rust conditional artifact consumer: strict gate is not required"
            );
            return;
        }
        Err(error) => panic!("read {REQUIRE}: {error}"),
    }

    assert_eq!(
        sha256(Path::new(
            "tests/fixtures/jvm_tasty_conditional_reference_selection/PairFactory.scala"
        )),
        receipt::PRODUCER_SOURCE_SHA256,
        "canonical Rust-authored producer source is unchanged"
    );
    assert_eq!(
        sha256(Path::new(
            "tests/fixtures/jvm_tasty_conditional_reference_selection/ScalaConsumer.scala"
        )),
        CLIENT_SHA256,
        "independently authored Scala client source"
    );

    let batch = emit_paired_jvm_tasty_batch(
        SOURCE,
        receipt::PRODUCER_SOURCE_NAME,
        SourceId::from_index(22_710),
    )
    .expect("canonical conditional Rust source emits its frozen paired batch");
    let mut files = Vec::with_capacity(6);
    let mut uuid_links = Vec::with_capacity(3);
    for class in batch.classes() {
        let class_bytes = class.classfile_bytes().to_vec();
        let tasty_bytes = class.tasty_bytes().to_vec();
        let mut class_reader = ClassReader::new(&class_bytes);
        let decoded = ClassFile::decode(&mut class_reader).expect("decode Rust class");
        assert!(class_reader.is_at_end());
        assert_eq!(decoded.version.major, 65);
        let tasty = TastyFile::parse_and_validate_scala_3_9(&tasty_bytes)
            .expect("Rust sidecar is valid Scala 3.9 TASTy");
        assert_eq!(
            (tasty.header().major_version, tasty.header().minor_version),
            (28, 9)
        );
        assert_eq!(
            decoded.tasty_link_id().unwrap(),
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
                .collect::<String>(),
        ));
        files.push((class.classfile_path().to_owned(), class_bytes));
        files.push((class.tasty_path().to_owned(), tasty_bytes));
    }
    assert_eq!(
        files
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        PATHS,
        "canonical six-file batch order"
    );
    assert_eq!(
        uuid_links,
        receipt::TASTY_UUID_LINKS.map(|(path, uuid)| (path.to_owned(), uuid.to_owned()))
    );
    for (path, expected) in receipt::ARTIFACT_SHA256 {
        let actual = files
            .iter()
            .find(|(candidate, _)| candidate == path)
            .unwrap();
        let digest = {
            use std::io::Write;
            use std::process::Stdio;
            let mut child = Command::new("sha256sum")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .expect("start artifact checksum");
            child.stdin.take().unwrap().write_all(&actual.1).unwrap();
            String::from_utf8_lossy(&child.wait_with_output().unwrap().stdout)
                .split_whitespace()
                .next()
                .unwrap()
                .to_owned()
        };
        assert_eq!(digest, expected, "canonical artifact hash for {path}");
    }

    let factory = files
        .iter()
        .find(|(path, _)| path == "example/PairFactory.class")
        .unwrap();
    let mut reader = ClassReader::new(&factory.1);
    let decoded = ClassFile::decode(&mut reader).unwrap();
    let create = decoded
        .methods
        .iter()
        .find(|method| decoded.constant_pool.utf8(method.name_index).ok() == Some("create"))
        .expect("PairFactory.create member lookup");
    assert_eq!(
        decoded.constant_pool.utf8(create.descriptor_index).unwrap(),
        "(IIZI)Lexample/Pair;"
    );

    let root = task_dir();
    let artifacts = root.join("artifacts");
    let classes = root.join("consumer-classes");
    fs::create_dir_all(&classes).unwrap();
    write_batch(&artifacts, &files);
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
    eprintln!(
        "SCALA_CONSUMER_PROVENANCE revision={} client_sha256={}",
        scala39_toolchain::SCALA_REVISION,
        sha256(Path::new(
            "tests/fixtures/jvm_tasty_conditional_reference_selection/ScalaConsumer.scala"
        ))
    );
    let client = root.join("ScalaConsumer.scala");
    fs::write(&client, CLIENT).unwrap();
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
        .arg(&classes)
        .arg(&client),
    );
    for forbidden in [
        "example/Counter.class",
        "example/Pair.class",
        "example/PairFactory.class",
    ] {
        assert!(
            !classes.join(forbidden).exists(),
            "Scala output does not contain a fallback {forbidden}"
        );
    }
    let run = output(
        Command::new(&pinned.java)
            .args(["-Xverify:all", "-ea", "-cp"])
            .arg(format!(
                "{}:{}:{}",
                classes.display(),
                artifacts.display(),
                pinned.libraries
            ))
            .arg("consumer.ScalaConsumer"),
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "scala39-rust-conditional-reference-consumer=ok"
    );

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
    let printed = String::from_utf8_lossy(&printed.stdout);
    let printed_lines = printed
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n");
    for evidence in [
        "32: DEFDEF(134) 12 [create]",
        "52: PARAM(7) 16 [chooseFirst]",
        "IDENTtpt 17 [Boolean]",
        "92: VALDEF(23) 24 [first]",
        "117: VALDEF(21) 25 [second]",
        "140: VALDEF(15) 26 [selected]",
        "149: IF(6)",
        "151: TERMREFdirect 52",
        "153: SHAREDtype 88",
        "155: SHAREDtype 90",
        "88: TERMREFdirect 92",
        "90: TERMREFdirect 117",
        "set[Signed Signature(List(scala.Int),scala.Unit) @set]",
        "162: TERMREFdirect 140",
        "167: TERMREFdirect 61",
        "<init>[Signed Signature(List(example.Counter, example.Counter),example.Pair)",
        "TERMREFdirect 92",
        "TERMREFdirect 117",
    ] {
        assert!(
            printed_lines.contains(evidence),
            "official -print-tasty includes {evidence}:\n{printed}"
        );
    }
    let decompiled = output(
        scala39_toolchain::scalac(
            &pinned.java_home,
            &pinned.scalac,
            PathBuf::from(std::env::var_os("TMPDIR").unwrap()),
        )
        .args(["-color:never", "-decompile"])
        .arg(&factory_tasty),
    );
    let decompiled = String::from_utf8_lossy(&decompiled.stdout);
    for evidence in [
        "def create(firstSeed: scala.Int, secondSeed: scala.Int, chooseFirst: scala.Boolean, nextValue: scala.Int): example.Pair",
        "chooseFirst: scala.Boolean",
        "selected: example.Counter = if (chooseFirst) first else second",
        "selected.set(nextValue)",
        "new example.Pair(first, second)",
    ] {
        assert!(
            decompiled.contains(evidence),
            "official -decompile includes {evidence}:\n{decompiled}"
        );
    }

    let from_tasty = root.join("scala-from-tasty");
    fs::create_dir_all(&from_tasty).unwrap();
    let sidecars = ["Counter", "Pair", "PairFactory"]
        .map(|name| artifacts.join(format!("example/{name}.tasty")));
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
        .args(sidecars),
    );
    for name in ["Counter", "Pair", "PairFactory"] {
        assert!(from_tasty.join(format!("example/{name}.class")).is_file());
    }

    let missing = root.join("negative-missing-sidecar");
    copy_batch(&missing, &files, Some("example/PairFactory.tasty"));
    let missing_out = root.join("negative-missing-sidecar-classes");
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
        .arg(&client),
    );
    assert!(
        !failed.status.success(),
        "missing PairFactory sidecar rejects Scala source compilation"
    );
    assert!(
        complete_output(&failed).contains("file is missing"),
        "missing sidecar failed at its first Scala consumer stage: {}",
        first_diagnostic(&failed)
    );
    eprintln!(
        "SCALA_NEGATIVE_FIRST_FAILURE missing-sidecar: {}",
        first_diagnostic(&failed)
    );

    let wrong_uuid = root.join("negative-wrong-uuid");
    copy_batch(&wrong_uuid, &files, None);
    let sidecar = wrong_uuid.join("example/PairFactory.tasty");
    let mut sidecar_bytes = fs::read(&sidecar).unwrap();
    let tasty = TastyFile::parse_and_validate_scala_3_9(&sidecar_bytes).unwrap();
    let uuid_offset = sidecar_bytes
        .windows(tasty.header().uuid.len())
        .position(|window| window == tasty.header().uuid)
        .expect("UUID occurs in TASTy header");
    sidecar_bytes[uuid_offset] ^= 1;
    fs::write(&sidecar, sidecar_bytes).unwrap();
    let wrong_uuid_out = root.join("negative-wrong-uuid-classes");
    fs::create_dir_all(&wrong_uuid_out).unwrap();
    let failed = attempt(
        scala39_toolchain::scalac(
            &pinned.java_home,
            &pinned.scalac,
            PathBuf::from(std::env::var_os("TMPDIR").unwrap()),
        )
        .args(["-Werror", "-release", "21", "-classpath"])
        .arg(format!("{}:{}", wrong_uuid.display(), pinned.libraries))
        .arg("-d")
        .arg(&wrong_uuid_out)
        .arg(&client),
    );
    assert!(
        !failed.status.success(),
        "classfile/sidecar UUID mismatch rejects Scala source compilation"
    );
    assert!(first_diagnostic(&failed).contains("out of sync"));
    eprintln!(
        "SCALA_NEGATIVE_FIRST_FAILURE wrong-uuid: {}",
        first_diagnostic(&failed)
    );

    let wrong_signature = root.join("negative-wrong-signature");
    copy_batch(&wrong_signature, &files, None);
    let class_path = wrong_signature.join("example/PairFactory.class");
    let mut class_bytes = fs::read(&class_path).unwrap();
    let old = b"(IIZI)Lexample/Pair;";
    let new = b"(ZIZI)Lexample/Pair;";
    let position = class_bytes
        .windows(old.len())
        .position(|window| window == old)
        .expect("create descriptor in classfile");
    class_bytes[position..position + old.len()].copy_from_slice(new);
    fs::write(&class_path, class_bytes).unwrap();
    let wrong_signature_out = root.join("negative-wrong-signature-classes");
    fs::create_dir_all(&wrong_signature_out).unwrap();
    output(
        scala39_toolchain::scalac(
            &pinned.java_home,
            &pinned.scalac,
            PathBuf::from(std::env::var_os("TMPDIR").unwrap()),
        )
        .args(["-Werror", "-release", "21", "-classpath"])
        .arg(format!(
            "{}:{}",
            wrong_signature.display(),
            pinned.libraries
        ))
        .arg("-d")
        .arg(&wrong_signature_out)
        .arg(&client),
    );
    let failed = attempt(
        Command::new(&pinned.java)
            .args(["-Xverify:all", "-ea", "-cp"])
            .arg(format!(
                "{}:{}:{}",
                wrong_signature_out.display(),
                wrong_signature.display(),
                pinned.libraries
            ))
            .arg("consumer.ScalaConsumer"),
    );
    assert!(
        !failed.status.success(),
        "mismatched classfile descriptor fails at JVM linkage"
    );
    assert!(
        String::from_utf8_lossy(&failed.stderr).contains("NoSuchMethodError"),
        "descriptor mismatch first fails at method linkage: {}",
        String::from_utf8_lossy(&failed.stderr)
    );
    eprintln!(
        "SCALA_NEGATIVE_FIRST_FAILURE wrong-signature: {}",
        first_diagnostic(&failed)
    );

    for (label, corrupt, expected) in [
        (
            "stale-if-arm",
            corrupt_if_arm(&fs::read(&factory_tasty).unwrap()),
            "cannot be unpickled",
        ),
        (
            "malformed-selected-valdef",
            corrupt_selected_valdef_reference(&fs::read(&factory_tasty).unwrap()),
            "cannot be unpickled",
        ),
    ] {
        let path = root.join(format!("negative-{label}.tasty"));
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
        assert!(
            first_diagnostic(&failed).contains(expected),
            "{label} first fails in official TASTy decompilation: {}",
            first_diagnostic(&failed)
        );
        eprintln!(
            "SCALA_NEGATIVE_FIRST_FAILURE {label}: {}",
            first_diagnostic(&failed)
        );
    }

    for (path, expected) in receipt::ARTIFACT_SHA256 {
        assert_eq!(
            sha256(&artifacts.join(path)),
            expected,
            "consumption leaves Rust artifact unchanged: {path}"
        );
    }
    fs::remove_dir_all(root).unwrap();
}
