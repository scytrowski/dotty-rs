use std::path::{Path, PathBuf};
use std::process::Command;

use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_batch;
use dotty_classfile::classfile::{Attribute, ClassFile, Reader};
use dotty_core::SourceId;
use dotty_jvm_tasty_contract::validate_classfile_tasty_link;
use dotty_tasty::tasty::TastyFile;

#[path = "support/jvm_tasty_batch_publisher.rs"]
#[allow(dead_code)]
mod batch_publisher;
use batch_publisher::publish_paired_batch;

const SOURCE: &str = include_str!("fixtures/jvm_tasty_multiclass/CounterGauge.scala");
const REQUIRE_SCALA_MULTICLASS_CONSUMER: &str = "DOTTY_REQUIRE_SCALA_TASTY_MULTICLASS_CONSUMER";

#[test]
fn pinned_scala_consumes_both_rust_tasty_classes_and_executes_the_original_batch() {
    if std::env::var(REQUIRE_SCALA_MULTICLASS_CONSUMER).as_deref() != Ok("1") {
        eprintln!(
            "skipped pinned Scala multi-class consumer gate; set {REQUIRE_SCALA_MULTICLASS_CONSUMER}=1 to require it"
        );
        return;
    }

    let first =
        emit_paired_jvm_tasty_batch(SOURCE, "CounterGauge.scala", SourceId::from_index(1657))
            .expect("emit the first complete fresh Rust batch");
    let second =
        emit_paired_jvm_tasty_batch(SOURCE, "CounterGauge.scala", SourceId::from_index(1658))
            .expect("emit the second complete fresh Rust batch");
    assert_ne!(first.source_id(), second.source_id());
    assert_eq!(first.classes().len(), 2);

    let expected = [
        (
            "example/Counter",
            "example/Counter.class",
            "example/Counter.tasty",
        ),
        (
            "example/Gauge",
            "example/Gauge.class",
            "example/Gauge.tasty",
        ),
    ];
    for ((left, right), (class_name, classfile_path, tasty_path)) in
        first.classes().iter().zip(second.classes()).zip(expected)
    {
        assert_eq!(left.classfile_path(), classfile_path);
        assert_eq!(left.tasty_path(), tasty_path);
        assert_eq!(left.classfile_path(), right.classfile_path());
        assert_eq!(left.tasty_path(), right.tasty_path());
        assert_eq!(left.classfile_bytes(), right.classfile_bytes());
        assert_eq!(left.tasty_bytes(), right.tasty_bytes());
        assert_eq!(left.tasty_link_id(), right.tasty_link_id());

        let mut reader = Reader::new(left.classfile_bytes());
        let class_file = ClassFile::decode(&mut reader).expect("decode emitted classfile");
        assert!(reader.is_at_end());
        assert_eq!(
            (class_file.version.major, class_file.version.minor),
            (65, 0)
        );
        assert_eq!(
            class_file
                .constant_pool
                .class_name(class_file.this_class)
                .unwrap(),
            class_name
        );
        let tasty = TastyFile::parse_and_validate_scala_3_9(left.tasty_bytes()).unwrap();
        assert_eq!(
            (
                tasty.header().major_version,
                tasty.header().minor_version,
                tasty.header().experimental_version,
            ),
            (28, 9, 0)
        );
        validate_classfile_tasty_link(&class_file, left.tasty_bytes()).unwrap();
    }

    let directory = TestDirectory::new();
    let fresh_a = directory.path().join("fresh-a");
    let fresh_b = directory.path().join("fresh-b");
    publish_paired_batch(&fresh_a, &first, None).expect("stage first complete validated batch");
    publish_paired_batch(&fresh_b, &second, None).expect("stage second complete validated batch");
    for pair in first.classes() {
        assert_eq!(
            std::fs::read(fresh_a.join(pair.classfile_path())).unwrap(),
            pair.classfile_bytes()
        );
        assert_eq!(
            std::fs::read(fresh_a.join(pair.tasty_path())).unwrap(),
            pair.tasty_bytes()
        );
    }

    for (index, class_name) in ["Counter", "Gauge"].into_iter().enumerate() {
        let pair = &first.classes()[index];
        let no_get = remove_get_method(pair.classfile_bytes(), class_name);

        let tasty_only = directory.path().join(format!("tasty-only-{class_name}"));
        copy_tree(&fresh_a, &tasty_only);
        std::fs::write(tasty_only.join(pair.classfile_path()), &no_get).unwrap();

        let classfile_only = directory
            .path()
            .join(format!("classfile-only-{class_name}"));
        copy_tree(&tasty_only, &classfile_only);
        std::fs::remove_file(classfile_only.join(pair.tasty_path())).unwrap();

        let missing_sidecar = directory
            .path()
            .join(format!("missing-sidecar-{class_name}"));
        copy_tree(&fresh_a, &missing_sidecar);
        std::fs::remove_file(missing_sidecar.join(pair.tasty_path())).unwrap();

        let uuid_mismatch = directory.path().join(format!("uuid-mismatch-{class_name}"));
        copy_tree(&fresh_a, &uuid_mismatch);
        let mismatched = mismatch_link_identifier(pair.classfile_bytes(), pair.tasty_link_id());
        std::fs::write(uuid_mismatch.join(pair.classfile_path()), mismatched).unwrap();

        let xasty = directory.path().join(format!("xasty-{class_name}"));
        copy_tree(&tasty_only, &xasty);
        std::fs::write(
            xasty.join(pair.classfile_path()),
            rename_tasty_marker(&no_get, "XASTY"),
        )
        .unwrap();
        std::fs::remove_file(xasty.join(pair.tasty_path())).unwrap();

        let missing_marker = directory
            .path()
            .join(format!("missing-marker-{class_name}"));
        copy_tree(&tasty_only, &missing_marker);
        std::fs::write(
            missing_marker.join(pair.classfile_path()),
            remove_tasty_marker(&no_get),
        )
        .unwrap();
        std::fs::remove_file(missing_marker.join(pair.tasty_path())).unwrap();

        let missing_classfile = directory
            .path()
            .join(format!("missing-classfile-{class_name}"));
        copy_tree(&fresh_a, &missing_classfile);
        std::fs::remove_file(missing_classfile.join(pair.classfile_path())).unwrap();

        let wrong_path = directory.path().join(format!("wrong-path-{class_name}"));
        copy_tree(&fresh_a, &wrong_path);
        let misplaced = wrong_path
            .join("misplaced")
            .join(format!("{class_name}.tasty"));
        std::fs::create_dir_all(misplaced.parent().unwrap()).unwrap();
        std::fs::rename(wrong_path.join(pair.tasty_path()), misplaced).unwrap();

        let corrupt_sidecar = directory
            .path()
            .join(format!("corrupt-sidecar-{class_name}"));
        copy_tree(&fresh_a, &corrupt_sidecar);
        let corrupt_path = corrupt_sidecar.join(pair.tasty_path());
        let mut corrupt = std::fs::read(&corrupt_path).unwrap();
        corrupt[0] ^= 0xff;
        std::fs::write(corrupt_path, corrupt).unwrap();
    }

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = manifest.join("tools/tasty-pickler-oracle/run.py");
    let result = Command::new("python3")
        .arg(script)
        .arg("consume-batch")
        .arg(directory.path())
        .current_dir(manifest)
        .output()
        .expect("start the pinned Scala multi-class consumer harness");
    assert!(
        result.status.success(),
        "pinned Scala multi-class consumer harness failed:\n{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let report = String::from_utf8(result.stdout).expect("consumer report is UTF-8");
    println!("Pinned Scala multi-class consumer result: {report}");
    for evidence in [
        "\"scala_consumer\": true",
        "\"fresh_batches_byte_identical\": true",
        "\"counter_tasty_only_method_resolved\": true",
        "\"gauge_tasty_only_method_resolved\": true",
        "\"counter_classfile_only_control_rejected\": true",
        "\"gauge_classfile_only_control_rejected\": true",
        "\"counter_missing_sidecar_control_rejected\": true",
        "\"gauge_missing_sidecar_control_rejected\": true",
        "\"counter_uuid_mismatch_control_rejected\": true",
        "\"gauge_uuid_mismatch_control_rejected\": true",
        "\"counter_xasty_marker_control_rejected\": true",
        "\"gauge_xasty_marker_control_rejected\": true",
        "\"counter_missing_marker_control_rejected\": true",
        "\"gauge_missing_marker_control_rejected\": true",
        "\"counter_missing_classfile_control_rejected\": true",
        "\"gauge_missing_classfile_control_rejected\": true",
        "\"counter_wrong_path_control_rejected\": true",
        "\"gauge_wrong_path_control_rejected\": true",
        "\"counter_corrupt_sidecar_control_rejected\": true",
        "\"gauge_corrupt_sidecar_control_rejected\": true",
        "\"rust_classfiles_unchanged_after_compile_and_run\": true",
    ] {
        assert!(report.contains(evidence), "missing {evidence} in {report}");
    }
    assert!(report.contains("\"runtime_outputs\": [\n    \"1329\",\n    \"1329\"\n  ]"));
}

fn remove_get_method(class_bytes: &[u8], class_name: &str) -> Vec<u8> {
    let mut reader = Reader::new(class_bytes);
    let mut class = ClassFile::decode(&mut reader).expect("decode emitted class");
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
        "{class_name}.get must exist once"
    );
    class.encode().expect("encode TASTy-only semantic control")
}

fn rename_tasty_marker(class_bytes: &[u8], replacement: &str) -> Vec<u8> {
    assert_eq!(replacement.len(), 5);
    let mut bytes = class_bytes.to_vec();
    let matches = bytes
        .windows(5)
        .enumerate()
        .filter_map(|(index, window)| (window == b"TASTY").then_some(index))
        .collect::<Vec<_>>();
    assert_eq!(
        matches.len(),
        1,
        "classfile should contain one TASTY marker name"
    );
    bytes[matches[0]..matches[0] + 5].copy_from_slice(replacement.as_bytes());
    bytes
}

fn remove_tasty_marker(class_bytes: &[u8]) -> Vec<u8> {
    let mut reader = Reader::new(class_bytes);
    let mut class = ClassFile::decode(&mut reader).expect("decode emitted class");
    assert!(reader.is_at_end());
    let before = class.attributes.len();
    class.attributes.retain(|attribute| match attribute {
        Attribute::Other(raw) => {
            class
                .constant_pool
                .utf8(raw.name_index)
                .expect("attribute name resolves")
                != "TASTY"
        }
        _ => true,
    });
    assert_eq!(
        class.attributes.len() + 1,
        before,
        "class has one TASTY marker"
    );
    class.encode().expect("encode class without TASTY marker")
}

fn mismatch_link_identifier(
    class_bytes: &[u8],
    id: dotty_classfile::tasty_link::TastyLinkId,
) -> Vec<u8> {
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

fn copy_tree(source: &Path, destination: &Path) {
    std::fs::create_dir(destination).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&source_path, &destination_path);
        } else {
            if let Some(parent) = destination_path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::copy(source_path, destination_path).unwrap();
        }
    }
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "dotty-scala-multiclass-consumer-{}-{}",
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
