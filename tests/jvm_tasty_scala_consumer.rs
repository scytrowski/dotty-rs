use std::path::{Path, PathBuf};
use std::process::Command;

use dotty::jvm_tasty_artifacts::{PairedJvmTastyArtifacts, emit_paired_jvm_tasty_class};
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::reader::Reader;
use dotty_classfile::tasty_link::TastyLinkId;
use dotty_core::SourceId;

const COUNTER: &str = include_str!("fixtures/jvm_tasty_interop/Counter.scala");
const REQUIRE_SCALA_CONSUMER: &str = "DOTTY_REQUIRE_SCALA_TASTY_CONSUMER";

#[test]
fn pinned_scala_consumes_and_executes_the_rust_linked_counter_pair() {
    if std::env::var(REQUIRE_SCALA_CONSUMER).as_deref() != Ok("1") {
        eprintln!(
            "skipped pinned Scala consumer gate; set {REQUIRE_SCALA_CONSUMER}=1 to require it"
        );
        return;
    }

    let first = emit_paired_jvm_tasty_class(COUNTER, "Counter.scala", SourceId::from_index(1638))
        .expect("emit the first fresh Rust Counter pair");
    let second = emit_paired_jvm_tasty_class(COUNTER, "Counter.scala", SourceId::from_index(1639))
        .expect("emit the second fresh Rust Counter pair");
    assert_ne!(first.source_id(), second.source_id());
    assert_eq!(first.classfile_path(), "example/Counter.class");
    assert_eq!(first.tasty_path(), "example/Counter.tasty");
    assert_eq!(first.source_member_order().len(), 3);
    println!(
        "Counter MVP sessions: first source={:?}, class={:?}, members={:?}; second source={:?}, class={:?}, members={:?}",
        first.source_id(),
        first.class(),
        first.source_member_order(),
        second.source_id(),
        second.class(),
        second.source_member_order(),
    );
    assert_eq!(first.classfile_path(), second.classfile_path());
    assert_eq!(first.tasty_path(), second.tasty_path());
    assert_eq!(first.classfile_bytes(), second.classfile_bytes());
    assert_eq!(first.tasty_bytes(), second.tasty_bytes());
    assert_eq!(first.tasty_link_id(), second.tasty_link_id());

    let directory = TestDirectory::new();
    write_pair(&directory.path().join("fresh-a"), &first);
    write_pair(&directory.path().join("fresh-b"), &second);

    let class_without_get = remove_get_method(first.classfile_bytes());
    write_artifacts(
        &directory.path().join("with-tasty-no-get"),
        &class_without_get,
        Some(first.tasty_path()),
        Some(first.tasty_bytes()),
    );
    write_artifacts(
        &directory.path().join("classfile-only-no-get"),
        &class_without_get,
        None,
        None,
    );

    let mismatched_class = mismatch_link_identifier(first.classfile_bytes(), first.tasty_link_id());
    write_artifacts(
        &directory.path().join("uuid-mismatch"),
        &mismatched_class,
        Some(first.tasty_path()),
        Some(first.tasty_bytes()),
    );
    let mut unrecognized_link = first.classfile_bytes().to_vec();
    let tasty_name = b"TASTY";
    let positions = unrecognized_link
        .windows(tasty_name.len())
        .enumerate()
        .filter_map(|(index, window)| (window == tasty_name).then_some(index))
        .collect::<Vec<_>>();
    assert_eq!(
        positions.len(),
        1,
        "classfile has one TASTY constant-pool name"
    );
    unrecognized_link[positions[0]] = b'X';
    write_artifacts(
        &directory.path().join("unrecognized-link"),
        &unrecognized_link,
        Some(first.tasty_path()),
        Some(first.tasty_bytes()),
    );

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = manifest.join("tools/tasty-pickler-oracle/run.py");
    let link_uuid = first
        .tasty_link_id()
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let result = Command::new("python3")
        .arg(script)
        .arg("consume-pair")
        .arg(directory.path())
        .arg("--tasty-link-uuid")
        .arg(&link_uuid)
        .current_dir(manifest)
        .output()
        .expect("start the pinned Scala linked-consumer harness");
    assert!(
        result.status.success(),
        "pinned Scala consumer harness failed:\n{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let report = String::from_utf8(result.stdout).expect("consumer report is UTF-8");
    println!("Pinned Scala consumer result: {report}");
    assert!(
        report.contains("\"tasty_only_method_resolved\": true"),
        "{report}"
    );
    assert!(
        report.contains("\"classfile_only_control_rejected\": true"),
        "{report}"
    );
    assert!(
        report.contains("\"uuid_mismatch_control_rejected\": true"),
        "{report}"
    );
    assert!(
        report.contains("\"unrecognized_link_control_rejected\": true"),
        "{report}"
    );
    assert!(
        report.contains(&format!("\"tasty_link_uuid\": \"{link_uuid}\"")),
        "{report}"
    );
    assert!(
        report.contains(
            "\"fresh_runtime_outputs\": [\n    \"13\",\n    \"-42\",\n    \"-42\",\n    \"-42\",\n    \"13\",\n    \"-42\",\n    \"-42\",\n    \"-42\"\n  ]"
        ),
        "{report}"
    );
    assert!(
        report.contains("\"counter_source_sha256\": \"79cd8926b6385dfafa39e766d80402643deeab673e526ba1abc67e3f67524a3e\""),
        "{report}"
    );
}

fn remove_get_method(class_bytes: &[u8]) -> Vec<u8> {
    let mut reader = Reader::new(class_bytes);
    let mut class = ClassFile::decode(&mut reader).expect("decode emitted Counter class");
    assert!(reader.is_at_end());
    let before = class.methods.len();
    class.methods.retain(|method| {
        class
            .constant_pool
            .utf8(method.name_index)
            .expect("method name resolves")
            != "get"
    });
    assert_eq!(
        class.methods.len() + 1,
        before,
        "Counter.get must exist once"
    );
    class.encode().expect("encode TASTy-only method control")
}

fn mismatch_link_identifier(class_bytes: &[u8], id: TastyLinkId) -> Vec<u8> {
    let mut bytes = class_bytes.to_vec();
    let needle = id.as_bytes();
    let matches = bytes
        .windows(needle.len())
        .enumerate()
        .filter_map(|(index, window)| (window == needle).then_some(index))
        .collect::<Vec<_>>();
    assert_eq!(
        matches.len(),
        1,
        "TASTY UUID should occur once in classfile"
    );
    bytes[matches[0]] ^= 0x80;
    bytes
}

fn write_pair(root: &Path, pair: &PairedJvmTastyArtifacts) {
    write_artifacts(
        root,
        pair.classfile_bytes(),
        Some(pair.tasty_path()),
        Some(pair.tasty_bytes()),
    );
}

fn write_artifacts(root: &Path, classfile: &[u8], tasty_path: Option<&str>, tasty: Option<&[u8]>) {
    let class_path = root.join("example/Counter.class");
    std::fs::create_dir_all(class_path.parent().unwrap()).unwrap();
    std::fs::write(class_path, classfile).unwrap();
    if let (Some(relative), Some(bytes)) = (tasty_path, tasty) {
        let tasty_path = root.join(relative);
        std::fs::create_dir_all(tasty_path.parent().unwrap()).unwrap();
        std::fs::write(tasty_path, bytes).unwrap();
    }
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "dotty-scala-linked-consumer-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).expect("create empty consumer fixture root");
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
