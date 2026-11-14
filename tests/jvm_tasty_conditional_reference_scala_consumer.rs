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
const JAVA_CLIENT: &str =
    include_str!("fixtures/jvm_tasty_conditional_reference_selection/RustRuntimeProbe.java");
const JAVA_CLIENT_SHA256: &str = "97cfe2c30ff7cb0b07627ce83dd2392aaee898c4690bc70b40206200a37d4a1b";
const MUTABLE_REBINDING_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_mutable_reference_rebinding/MutableReferenceRebinding.scala");
const MUTABLE_REBINDING_PROBE: &str =
    include_str!("fixtures/jvm_tasty_mutable_reference_rebinding/RuntimeProbe.java");
const MUTABLE_REBINDING_MANIFEST: &str =
    include_str!("fixtures/jvm_tasty_mutable_reference_rebinding/oracle.json");
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

fn configure_jdk_command(command: &mut Command, java_home: &Path) {
    let tmpdir = std::env::var_os("TMPDIR").expect("TMPDIR must be task-local");
    command.env("JAVA_HOME", java_home).env(
        "JAVA_TOOL_OPTIONS",
        format!(
            "-XX:-UsePerfData -Djava.io.tmpdir={}",
            Path::new(&tmpdir).display()
        ),
    );
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

fn json_string_field<'a>(object: &'a str, field: &str) -> &'a str {
    let key = format!("\"{field}\": \"");
    let value = object
        .split_once(&key)
        .unwrap_or_else(|| panic!("manifest object has {field} string field"))
        .1;
    value
        .split_once('"')
        .expect("manifest string field is terminated")
        .0
}

fn snapshot_receipt<'a>(manifest: &'a str, name: &str) -> (&'a str, &'a str) {
    let snapshots = manifest
        .split_once("\"snapshots\": {")
        .expect("oracle manifest has snapshots object")
        .1;
    let key = format!("\"{name}\": {{");
    let object = snapshots
        .split_once(&key)
        .unwrap_or_else(|| panic!("oracle manifest has {name} snapshot"))
        .1
        .split_once('}')
        .expect("snapshot entry is terminated")
        .0;
    (
        json_string_field(object, "path"),
        json_string_field(object, "sha256"),
    )
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
    assert_eq!(
        sha256(Path::new(
            "tests/fixtures/jvm_tasty_conditional_reference_selection/RustRuntimeProbe.java"
        )),
        JAVA_CLIENT_SHA256,
        "independently authored Java client source"
    );

    let batch = emit_paired_jvm_tasty_batch(
        SOURCE,
        receipt::PRODUCER_SOURCE_NAME,
        SourceId::from_index(22_710),
    )
    .expect("canonical conditional Rust source emits its frozen paired batch");
    let repeated_batch = emit_paired_jvm_tasty_batch(
        SOURCE,
        receipt::PRODUCER_SOURCE_NAME,
        SourceId::from_index(22_710),
    )
    .expect("a fresh canonical source session emits a second paired batch");
    let mut files = Vec::with_capacity(6);
    let mut repeated_files = Vec::with_capacity(6);
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
    for class in repeated_batch.classes() {
        repeated_files.push((
            class.classfile_path().to_owned(),
            class.classfile_bytes().to_vec(),
        ));
        repeated_files.push((class.tasty_path().to_owned(), class.tasty_bytes().to_vec()));
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
        repeated_files, files,
        "two fresh source sessions produce identical ordered artifact paths and complete bytes"
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
    for (path, expected) in receipt::ARTIFACT_SHA256 {
        assert_eq!(
            sha256(&artifacts.join(path)),
            expected,
            "original batch verified before either consumer: {path}"
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
    eprintln!(
        "SCALA_CONSUMER_PROVENANCE revision={} client_sha256={}",
        scala39_toolchain::SCALA_REVISION,
        sha256(Path::new(
            "tests/fixtures/jvm_tasty_conditional_reference_selection/ScalaConsumer.scala"
        ))
    );

    // Both independent clients below consume this single physical directory. Keep
    // the frozen original hashes around each client so a client cannot silently
    // replace or regenerate one of the six Rust artifacts for the other.
    let java_source = root.join("RustRuntimeProbe.java");
    let java_classes = root.join("java-consumer-classes");
    fs::create_dir_all(&java_classes).unwrap();
    fs::write(&java_source, JAVA_CLIENT).unwrap();
    let mut javac = Command::new(pinned.java_home.join("bin/javac"));
    javac
        .args(["--release", "21", "-classpath"])
        .arg(&artifacts)
        .arg("-d")
        .arg(&java_classes)
        .arg(&java_source);
    configure_jdk_command(&mut javac, &pinned.java_home);
    output(&mut javac);
    let mut javap = Command::new(pinned.java_home.join("bin/javap"));
    javap
        .args(["-v", "-c", "-p", "-classpath"])
        .arg(&artifacts)
        .arg("example.PairFactory");
    configure_jdk_command(&mut javap, &pinned.java_home);
    let javap = output(&mut javap);
    let javap = String::from_utf8_lossy(&javap.stdout);
    for evidence in [
        "major version: 65",
        "descriptor: (IIZI)Lexample/Pair;",
        "stack=4, locals=8, args_size=5",
        "ifeq          29",
        "goto          31",
        "example/Counter.set:(I)V",
        "StackMapTable: number_of_entries = 2",
        "frame_type = 255 /* full_frame */",
        "offset_delta = 29",
        "stack = [ class example/Counter ]",
        "44: aload         5",
        "46: aload         6",
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
        "javap sees exactly two Counter allocations"
    );
    assert_eq!(
        javap
            .lines()
            .filter(
                |line| line.contains("invokevirtual") && line.contains("example/Counter.set:(I)V")
            )
            .count(),
        1,
        "javap sees exactly one setter call"
    );
    let mut java = Command::new(&pinned.java);
    java.args(["-Xverify:all", "-ea", "-cp"])
        .arg(format!(
            "{}:{}",
            java_classes.display(),
            artifacts.display()
        ))
        .arg("example.RustRuntimeProbe");
    configure_jdk_command(&mut java, &pinned.java_home);
    let java_run = output(&mut java);
    assert_eq!(
        String::from_utf8_lossy(&java_run.stdout).trim(),
        "conditional-rust-runtime=true:42/3,false:7/42,negative:0/42,-9/0,equal:42/7,identity=original-distinct,fresh=true"
    );
    for (path, expected) in receipt::ARTIFACT_SHA256 {
        assert_eq!(
            sha256(&artifacts.join(path)),
            expected,
            "Java leaves {path} unchanged"
        );
    }

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
    for (path, expected) in receipt::ARTIFACT_SHA256 {
        assert_eq!(
            sha256(&artifacts.join(path)),
            expected,
            "Scala leaves {path} unchanged"
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

#[test]
fn scala39_mutable_reference_rebinding_oracle_is_pinned_and_executable() {
    if std::env::var(REQUIRE).as_deref() != Ok("1") {
        if std::env::var_os(REQUIRE).is_some() {
            panic!("{REQUIRE} must equal 1 when set");
        }
        eprintln!("SKIP Scala 3.9 mutable reference rebinding oracle: strict gate is not required");
        return;
    }
    let source_path = Path::new(
        "tests/fixtures/jvm_tasty_mutable_reference_rebinding/MutableReferenceRebinding.scala",
    );
    let probe_path =
        Path::new("tests/fixtures/jvm_tasty_mutable_reference_rebinding/RuntimeProbe.java");
    assert_eq!(
        sha256(source_path),
        "2cbc91af8bcac9cc158f2c4b1045838d428ac58ae8e174890852e279efd1d469"
    );
    assert_eq!(
        sha256(probe_path),
        "709e0695297b0237d9e0b1a19c52ef4f2fbe981d3f4357e26fae6bf6363b59e3"
    );
    for name in ["javap", "print_tasty", "decompile", "typed_tree"] {
        let (relative_path, expected_hash) = snapshot_receipt(MUTABLE_REBINDING_MANIFEST, name);
        let path =
            Path::new("tests/fixtures/jvm_tasty_mutable_reference_rebinding").join(relative_path);
        assert_eq!(
            sha256(&path),
            expected_hash,
            "oracle manifest SHA-256 for the {name} snapshot"
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
    let root = task_dir();
    let classes = root.join("classes");
    fs::create_dir_all(&classes).unwrap();
    let mut scalac = scala39_toolchain::scalac(
        &pinned.java_home,
        &pinned.scalac,
        PathBuf::from(std::env::var_os("TMPDIR").unwrap()),
    );
    scalac
        .args(["-Werror", "-release", "21", "-d"])
        .arg(&classes)
        .arg(source_path);
    output(&mut scalac);

    for relative in PATHS {
        let checked_in = Path::new("tests/fixtures/jvm_tasty_mutable_reference_rebinding/scala39")
            .join(relative);
        assert_eq!(
            fs::read(classes.join(relative)).unwrap(),
            fs::read(&checked_in).unwrap(),
            "pinned Scala output must match checked-in oracle: {relative}"
        );
    }
    for (relative, expected_hash) in [
        (
            "example/Counter.class",
            "7b46dd47c9f27861ff6e66beb95b4936fa613851407cac9c64465bdbbf634d6e",
        ),
        (
            "example/Counter.tasty",
            "5b1c23bb7f960014cbbe3d37edcda07019ee968209ee9d4f24c08855bc683057",
        ),
        (
            "example/Pair.class",
            "90fe0433064510240969e1b2d5057baeb737b86ee9fca89242a5056b1f83de06",
        ),
        (
            "example/Pair.tasty",
            "72396247c4e8661b1e26f982daad2a7c30bad18984f1b11e103458e1a478c5ce",
        ),
        (
            "example/PairFactory.class",
            "5fe95908cb6cde1a417c90ad72e4560636c4c8c16e060545177a644f8f0cbffe",
        ),
        (
            "example/PairFactory.tasty",
            "736c135470d5aea4c80a786df751a5c4d10a6bca8076b4f5ff10b7639ee1b706",
        ),
    ] {
        assert_eq!(
            sha256(&classes.join(relative)),
            expected_hash,
            "pinned artifact manifest hash for {relative}"
        );
    }
    for (name, expected_uuid) in [
        ("Counter", "0037406b89e4eee800ccab48e5a4bd1b"),
        ("Pair", "00d6f05f6fd1c0d300a94710d6406d5b"),
        ("PairFactory", "0046fa06c00ff8d4002ce145a844c627"),
    ] {
        let class_bytes = fs::read(classes.join(format!("example/{name}.class"))).unwrap();
        let tasty_bytes = fs::read(classes.join(format!("example/{name}.tasty"))).unwrap();
        let mut reader = ClassReader::new(&class_bytes);
        let class = ClassFile::decode(&mut reader).expect("pinned Scala class decodes");
        assert!(reader.is_at_end());
        assert_eq!(class.version.major, 65);
        let link = class.tasty_link_id().unwrap().unwrap();
        let rendered_uuid = link
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(rendered_uuid, expected_uuid, "pinned TASTy link for {name}");
        let tasty = TastyFile::parse_and_validate_scala_3_9(&tasty_bytes)
            .expect("pinned Scala sidecar validates");
        assert_eq!(
            (
                tasty.header().major_version,
                tasty.header().minor_version,
                tasty.header().experimental_version
            ),
            (28, 9, 0)
        );
        assert_eq!(tasty.header().uuid, *link.as_bytes());
    }
    let probe = root.join("RuntimeProbe.java");
    let probe_classes = root.join("probe-classes");
    fs::create_dir_all(&probe_classes).unwrap();
    fs::write(&probe, MUTABLE_REBINDING_PROBE).unwrap();
    let mut javac = Command::new(pinned.java_home.join("bin/javac"));
    javac
        .args(["--release", "21", "-classpath"])
        .arg(&classes)
        .arg("-d")
        .arg(&probe_classes)
        .arg(&probe);
    configure_jdk_command(&mut javac, &pinned.java_home);
    output(&mut javac);
    let mut java = Command::new(&pinned.java);
    java.args(["-Xverify:all", "-ea", "-cp"])
        .arg(format!("{}:{}", probe_classes.display(), classes.display()))
        .arg("example.RuntimeProbe");
    configure_jdk_command(&mut java, &pinned.java_home);
    let run = output(&mut java);
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "mutable reference rebinding values=7/42,-9/11,5/-13 distinct=true"
    );

    let mut javap = Command::new(pinned.java_home.join("bin/javap"));
    javap
        .args(["-v", "-c", "-p", "-classpath"])
        .arg(&classes)
        .arg("example.PairFactory");
    configure_jdk_command(&mut javap, &pinned.java_home);
    let javap = String::from_utf8_lossy(&output(&mut javap).stdout).into_owned();
    for evidence in [
        "major version: 65",
        "descriptor: (III)Lexample/Pair;",
        "example/Counter.set:(I)V",
    ] {
        assert!(
            javap.contains(evidence),
            "javap lacks {evidence:?}:\n{javap}"
        );
    }
    assert_eq!(
        javap
            .lines()
            .filter(
                |line| line.contains("invokevirtual") && line.contains("example/Counter.set:(I)V")
            )
            .count(),
        1
    );
    let printed_tasty = fs::read_to_string(
        "tests/fixtures/jvm_tasty_mutable_reference_rebinding/oracle/PairFactory.print-tasty.txt",
    )
    .unwrap();
    for evidence in [
        "tooling: Scala 3.9.0-bin-SNAPSHOT-git-777528f",
        "VALDEF(8) 26 [selected]",
        "ASSIGN(5)",
        "TERMREFdirect 106",
    ] {
        assert!(
            printed_tasty.contains(evidence),
            "printed TASTy oracle lacks {evidence:?}"
        );
    }

    let base = concat!(
        "    val first: Counter = new Counter(firstSeed)\n",
        "    val second: Counter = new Counter(secondSeed)\n",
        "    var selected: Counter = first\n",
        "    selected = second\n",
        "    selected.set(nextValue)\n",
        "    new Pair(first, second)"
    );
    let replace = |old: &str, new: &str| MUTABLE_REBINDING_SOURCE.replace(old, new);
    let renamed = replace(
        base,
        concat!(
            "    val left: Counter = new Counter(firstSeed)\n",
            "    val right: Counter = new Counter(secondSeed)\n",
            "    var active: Counter = left\n",
            "    active = right\n",
            "    active.set(nextValue)\n",
            "    new Pair(left, right)"
        ),
    );
    let reversed = replace(
        "    val first: Counter = new Counter(firstSeed)\n    val second: Counter = new Counter(secondSeed)",
        "    val second: Counter = new Counter(secondSeed)\n    val first: Counter = new Counter(firstSeed)",
    );
    let select_first = replace("    selected = second", "    selected = first");
    let duplicate = replace(
        "    selected = second\n    selected.set(nextValue)",
        "    selected = second\n    selected = first\n    selected.set(nextValue)",
    );
    let allocation_rhs = replace(
        "    selected = second",
        "    selected = new Counter(nextValue)",
    );
    let nested_branch = replace(
        "    selected = second",
        "    if nextValue >= 0 then\n      selected = second\n    else\n      selected = first",
    );
    let nested_loop = replace(
        "    selected = second",
        "    while nextValue > 0 do\n      selected = second",
    );
    let val_write = replace(
        "    var selected: Counter = first",
        "    val selected: Counter = first",
    );
    let foreign = replace("    selected = second", "    selected = foreign");
    let forward = replace(
        "    var selected: Counter = first\n    selected = second",
        "    var selected: Counter = later\n    selected = second\n    val later: Counter = first",
    );
    let uninitialized = replace(
        "    var selected: Counter = first",
        "    var selected: Counter = _",
    );
    let wrong_type = replace("    selected = second", "    selected = nextValue");
    for (name, variant, expected_diagnostic) in [
        ("renamed", renamed, None),
        ("reversed", reversed, None),
        ("select-first", select_first, None),
        ("duplicate", duplicate, None),
        ("allocation-rhs", allocation_rhs, None),
        ("nested-branch", nested_branch, None),
        ("nested-loop", nested_loop, None),
        ("val-write", val_write, Some("[E052]")),
        ("foreign", foreign, Some("[E119]")),
        ("forward-reference", forward, Some("[E039]")),
        ("uninitialized", uninitialized, Some("Warning:")),
        ("wrong-type", wrong_type, Some("[E007]")),
    ] {
        let control_dir = root.join(format!("control-{name}"));
        let control_classes = control_dir.join("classes");
        fs::create_dir_all(&control_classes).unwrap();
        let control_source = control_dir.join("MutableReferenceRebinding.scala");
        fs::write(&control_source, variant).unwrap();
        let mut command = scala39_toolchain::scalac(
            &pinned.java_home,
            &pinned.scalac,
            PathBuf::from(std::env::var_os("TMPDIR").unwrap()),
        );
        command
            .args(["-Werror", "-release", "21", "-d"])
            .arg(&control_classes)
            .arg(&control_source);
        let result = attempt(&mut command);
        match expected_diagnostic {
            None => assert!(
                result.status.success(),
                "Scala-valid control {name}: {}",
                complete_output(&result)
            ),
            Some(expected) => {
                assert!(
                    !result.status.success(),
                    "invalid Scala control {name} unexpectedly compiled"
                );
                assert!(
                    complete_output(&result).contains(expected),
                    "{name} lacks {expected}: {}",
                    complete_output(&result)
                );
            }
        }
    }
}
