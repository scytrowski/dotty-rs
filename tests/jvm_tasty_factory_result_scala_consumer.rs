use std::path::{Path, PathBuf};
use std::process::Command;

use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_batch;
use dotty_core::SourceId;

#[path = "support/jvm_tasty_batch_publisher.rs"]
#[allow(dead_code)]
mod batch_publisher;
use batch_publisher::publish_paired_batch;

const SOURCE: &str =
    include_str!("fixtures/jvm_tasty_factory_result_arguments/CounterOwnerFactory.scala");
const REQUIRE_CONSUMER: &str = "DOTTY_REQUIRE_SCALA_TASTY_FACTORY_RESULT_CONSUMER";

#[test]
fn pinned_scala_consumes_factory_result_rust_tasty_and_executes_original_classes() {
    if std::env::var(REQUIRE_CONSUMER).as_deref() != Ok("1") {
        eprintln!(
            "skipped pinned Scala factory-result consumer gate; set {REQUIRE_CONSUMER}=1 to require it"
        );
        return;
    }

    let first = emit_paired_jvm_tasty_batch(
        SOURCE,
        "CounterOwnerFactory.scala",
        SourceId::from_index(18371),
    )
    .expect("emit original six-file Rust batch");
    let second = emit_paired_jvm_tasty_batch(
        SOURCE,
        "CounterOwnerFactory.scala",
        SourceId::from_index(18372),
    )
    .expect("emit independent Rust batch for determinism evidence");
    assert_ne!(first.source_id(), second.source_id());
    assert_eq!(first.classes().len(), 3);

    let renamed_reordered = SOURCE
        .replace("package example", "package renamed")
        .replace("CounterOwnerFactory", "WidgetBuilder")
        .replace("CounterOwner", "WidgetHolder")
        .replace("Counter", "Widget");
    let renamed_reordered = reorder_widget_classes(&renamed_reordered);
    let renamed_first = emit_paired_jvm_tasty_batch(
        &renamed_reordered,
        "RenamedFactoryResult.scala",
        SourceId::from_index(18373),
    )
    .expect("emit renamed/reordered Rust batch");
    let renamed_second = emit_paired_jvm_tasty_batch(
        &renamed_reordered,
        "RenamedFactoryResult.scala",
        SourceId::from_index(18374),
    )
    .expect("emit independent renamed/reordered Rust batch");
    assert_eq!(renamed_first.classes().len(), 3);

    let directory = TestDirectory::new();
    for (name, batch) in [
        ("fresh-a", &first),
        ("fresh-b", &second),
        ("renamed-a", &renamed_first),
        ("renamed-b", &renamed_second),
    ] {
        publish_paired_batch(&directory.path().join(name), batch, None)
            .unwrap_or_else(|error| panic!("publish {name} six-artifact batch: {error}"));
    }

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let result = Command::new("python3")
        .arg(manifest.join("tools/tasty-pickler-oracle/run.py"))
        .arg("consume-factory-result")
        .arg(directory.path())
        .current_dir(manifest)
        .output()
        .expect("start pinned Scala factory-result consumer harness");
    assert!(
        result.status.success(),
        "required pinned Scala factory-result consumer failed:\n{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let report = String::from_utf8(result.stdout).expect("consumer evidence is UTF-8");
    println!("Pinned Scala factory-result consumer result: {report}");
    for evidence in [
        "\"scala_consumer\": true",
        "\"runtime_outputs\": [",
        "\"7,-9,3\"",
        "\"counter_tasty_only_get_resolved\": true",
        "\"owner_tasty_only_read_resolved\": true",
        "\"factory_tasty_only_create_resolved\": true",
        "\"factory_tasty_only_make_counter_resolved\": true",
        "\"counter_classfile_only_control_rejected\": true",
        "\"owner_classfile_only_control_rejected\": true",
        "\"factory_classfile_only_create_control_rejected\": true",
        "\"factory_classfile_only_make_counter_control_rejected\": true",
        "\"factory_tasty_methods_decompiled\": true",
        "\"helper_result_descriptor_mismatch\"",
        "\"wrong_owner_constructor_signature\"",
        "\"wrong_internal_owner\"",
        "\"no-such-method\": \"NoSuchMethodError\"",
        "\"no-class-def\": \"NoClassDefFoundError\"",
        "\"per_class_sidecar_matrix_rejected\": true",
        "\"renamed_reordered_scala_consumer\": true",
        "\"rust_artifacts_unchanged_after_compile_and_run\": true",
    ] {
        assert!(report.contains(evidence), "missing {evidence} in {report}");
    }
}

fn reorder_widget_classes(source: &str) -> String {
    let lines = source.lines().collect::<Vec<_>>();
    let starts = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| line.starts_with("class ").then_some(index))
        .collect::<Vec<_>>();
    assert_eq!(starts.len(), 3, "fixture has three top-level classes");
    let prefix = lines[..starts[0]].join("\n");
    let blocks = starts
        .iter()
        .enumerate()
        .map(|(index, start)| {
            let end = starts.get(index + 1).copied().unwrap_or(lines.len());
            lines[*start..end].join("\n").trim_end().to_owned()
        })
        .collect::<Vec<_>>();
    format!(
        "{prefix}\n\n{}\n\n{}\n\n{}\n",
        blocks[1], blocks[0], blocks[2]
    )
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dotty-scala-factory-result-consumer-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time follows Unix epoch")
                .as_nanos(),
        ));
        std::fs::create_dir(&path).expect("create empty Scala consumer fixture root");
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
