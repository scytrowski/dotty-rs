use std::path::{Path, PathBuf};
use std::process::Command;

use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_batch;
use dotty_core::SourceId;

#[path = "support/jvm_tasty_batch_publisher.rs"]
#[allow(dead_code)]
mod batch_publisher;
use batch_publisher::publish_paired_batch;

const SOURCE: &str =
    include_str!("fixtures/jvm_tasty_nested_constructor_calls/NestedConstructorCalls.scala");
const CONSUMER: &str =
    include_str!("fixtures/jvm_tasty_nested_constructor_calls/ScalaConsumer.scala");
const REQUIRE_CONSUMER: &str = "DOTTY_REQUIRE_SCALA_TASTY_NESTED_CONSTRUCTOR_CONSUMER";

#[test]
fn pinned_scala_consumes_nested_rust_tasty_and_executes_original_classes() {
    if std::env::var(REQUIRE_CONSUMER).as_deref() != Ok("1") {
        eprintln!(
            "skipped pinned Scala nested-constructor consumer gate; set {REQUIRE_CONSUMER}=1 to require it"
        );
        return;
    }

    assert!(CONSUMER.contains("factory.create(7)"));
    assert!(CONSUMER.contains("factory.create(-9)"));
    assert!(CONSUMER.contains("factory.create(3)"));

    let first = emit_paired_jvm_tasty_batch(
        SOURCE,
        "NestedConstructorCalls.scala",
        SourceId::from_index(18201),
    )
    .expect("emit original six-file Rust batch");
    let second = emit_paired_jvm_tasty_batch(
        SOURCE,
        "NestedConstructorCalls.scala",
        SourceId::from_index(18202),
    )
    .expect("emit an independent Rust batch for determinism evidence");
    assert_ne!(first.source_id(), second.source_id());
    assert_eq!(first.classes().len(), 3);

    let renamed = SOURCE
        .replace("package example", "package renamed")
        .replace("CounterOwnerFactory", "WidgetBuilder")
        .replace("CounterOwner", "WidgetHolder")
        .replace("Counter", "Widget");
    let renamed_reordered = renamed
        .replace(
            "class Widget(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n\nclass WidgetHolder(counter: Widget):",
            "class WidgetHolder(counter: Widget):",
        )
        .replace(
            "\nclass WidgetBuilder:",
            "\nclass Widget(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n\nclass WidgetBuilder:",
        );
    let renamed_first = emit_paired_jvm_tasty_batch(
        &renamed_reordered,
        "RenamedNestedConstructorCalls.scala",
        SourceId::from_index(18203),
    )
    .expect("emit renamed/reordered six-file Rust batch");
    let renamed_second = emit_paired_jvm_tasty_batch(
        &renamed_reordered,
        "RenamedNestedConstructorCalls.scala",
        SourceId::from_index(18204),
    )
    .expect("emit independent renamed/reordered Rust batch");
    assert_eq!(renamed_first.classes().len(), 3);

    let wrong_result_source = SOURCE
        .replace(
            "def create(seed: Int): CounterOwner",
            "def create(seed: Int): Counter",
        )
        .replace("new CounterOwner(new Counter(seed))", "new Counter(seed)");
    let wrong_result = emit_paired_jvm_tasty_batch(
        &wrong_result_source,
        "WrongFactoryResult.scala",
        SourceId::from_index(18205),
    )
    .expect("emit valid control with an altered Factory method result");

    let directory = TestDirectory::new();
    let first_root = directory.path().join("fresh-a");
    let second_root = directory.path().join("fresh-b");
    let renamed_first_root = directory.path().join("renamed-a");
    let renamed_second_root = directory.path().join("renamed-b");
    let wrong_result_root = directory.path().join("wrong-result");
    publish_paired_batch(&first_root, &first, None).expect("publish first six Rust artifacts");
    publish_paired_batch(&second_root, &second, None).expect("publish second six Rust artifacts");
    publish_paired_batch(&renamed_first_root, &renamed_first, None)
        .expect("publish renamed/reordered batch");
    publish_paired_batch(&renamed_second_root, &renamed_second, None)
        .expect("publish second renamed/reordered batch");
    publish_paired_batch(&wrong_result_root, &wrong_result, None)
        .expect("publish altered-result negative batch");

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let result = Command::new("python3")
        .arg(manifest.join("tools/tasty-pickler-oracle/run.py"))
        .arg("consume-nested-constructor")
        .arg(directory.path())
        .current_dir(manifest)
        .output()
        .expect("start pinned Scala nested-constructor consumer harness");
    assert!(
        result.status.success(),
        "required pinned Scala nested-constructor consumer failed:\n{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let report = String::from_utf8(result.stdout).expect("consumer evidence is UTF-8");
    println!("Pinned Scala nested-constructor consumer result: {report}");
    for evidence in [
        "\"scala_consumer\": true",
        "\"runtime_outputs\": [\n    \"7,-9,3\",\n    \"7,-9,3\"\n  ]",
        "\"counter_tasty_only_get_resolved\": true",
        "\"owner_tasty_only_read_resolved\": true",
        "\"factory_tasty_only_create_resolved\": true",
        "\"counter_classfile_only_control_rejected\": true",
        "\"owner_classfile_only_control_rejected\": true",
        "\"factory_classfile_only_control_rejected\": true",
        "\"factory_tasty_nested_constructor_call_decompiled\": true",
        "\"rust_artifacts_unchanged_after_compile_and_run\": true",
        "\"renamed_reordered_scala_consumer\": true",
        "\"renamed_classfile_only_control_rejected\": true",
        "\"wrong_factory_result_rejected\": true",
        "\"wrong_factory_constructor_signature_rejected_at_compile\": true",
    ] {
        assert!(report.contains(evidence), "missing {evidence} in {report}");
    }
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "dotty-scala-nested-constructor-consumer-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time follows Unix epoch")
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed),
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
