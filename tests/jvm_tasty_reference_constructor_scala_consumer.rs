use std::path::{Path, PathBuf};
use std::process::Command;

use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_batch;
use dotty_classfile::classfile::{ClassFile, Reader};
use dotty_core::SourceId;
use dotty_tasty::tasty::TastyFile;

#[path = "support/jvm_tasty_batch_publisher.rs"]
#[allow(dead_code)]
mod batch_publisher;
use batch_publisher::publish_paired_batch;

const SOURCE: &str =
    include_str!("fixtures/jvm_tasty_reference_constructor_calls/CounterOwnerFactory.scala");
const CONSUMER_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_reference_constructor_calls/ScalaConsumer.scala");
const REQUIRE_CONSUMER: &str = "DOTTY_REQUIRE_SCALA_TASTY_REFERENCE_CONSTRUCTOR_CONSUMER";

#[test]
fn pinned_scala_consumes_three_rust_reference_constructor_tasty_sidecars() {
    if std::env::var(REQUIRE_CONSUMER).as_deref() != Ok("1") {
        eprintln!(
            "skipped pinned Scala reference-constructor consumer gate; set {REQUIRE_CONSUMER}=1 to require it"
        );
        return;
    }

    let first = emit_paired_jvm_tasty_batch(
        SOURCE,
        "CounterOwnerFactory.scala",
        SourceId::from_index(17991),
    )
    .expect("emit first complete Rust Counter/Owner/Factory batch");
    let second = emit_paired_jvm_tasty_batch(
        SOURCE,
        "CounterOwnerFactory.scala",
        SourceId::from_index(17992),
    )
    .expect("emit second complete independent Rust batch");
    let renamed_source = SOURCE
        .replace("package example", "package renamed")
        .replace("CounterOwnerFactory", "WidgetBuilder")
        .replace("CounterOwner", "WidgetHolder")
        .replace("Counter", "Widget")
        .replace("wrap(", "build(");
    let renamed_reordered_source = renamed_source
        .replace(
            "class Widget(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n\nclass WidgetHolder(counter: Widget):",
            "class WidgetHolder(counter: Widget):",
        )
        .replace(
            "\nclass WidgetBuilder:",
            "\nclass Widget(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n\nclass WidgetBuilder:",
        );
    let renamed_first = emit_paired_jvm_tasty_batch(
        &renamed_reordered_source,
        "WidgetBuilder.scala",
        SourceId::from_index(17993),
    )
    .expect("emit first renamed/reordered Rust graph");
    let renamed_second = emit_paired_jvm_tasty_batch(
        &renamed_reordered_source,
        "WidgetBuilder.scala",
        SourceId::from_index(17994),
    )
    .expect("emit second renamed/reordered Rust graph");
    assert_ne!(renamed_first.source_id(), renamed_second.source_id());
    assert_eq!(renamed_first.classes().len(), 3);
    for (left, right) in renamed_first.classes().iter().zip(renamed_second.classes()) {
        assert_eq!(left.source_id(), renamed_first.source_id());
        assert_eq!(right.source_id(), renamed_second.source_id());
        assert_eq!(left.class(), right.class());
        assert_eq!(left.source_member_order(), right.source_member_order());
        assert_eq!(left.classfile_path(), right.classfile_path());
        assert_eq!(left.tasty_path(), right.tasty_path());
        assert_eq!(left.classfile_bytes(), right.classfile_bytes());
        assert_eq!(left.tasty_bytes(), right.tasty_bytes());
        assert_eq!(left.tasty_link_id(), right.tasty_link_id());
    }
    assert_ne!(first.source_id(), second.source_id());
    assert_eq!(first.classes().len(), 3);
    assert!(CONSUMER_SOURCE.contains("factory.wrap(counter = original)"));

    let expected = [
        (
            "example/Counter",
            "example/Counter.class",
            "example/Counter.tasty",
        ),
        (
            "example/CounterOwner",
            "example/CounterOwner.class",
            "example/CounterOwner.tasty",
        ),
        (
            "example/CounterOwnerFactory",
            "example/CounterOwnerFactory.class",
            "example/CounterOwnerFactory.tasty",
        ),
    ];
    for ((left, right), (name, classfile_path, tasty_path)) in
        first.classes().iter().zip(second.classes()).zip(expected)
    {
        assert_eq!(left.classfile_path(), classfile_path);
        assert_eq!(left.tasty_path(), tasty_path);
        assert_eq!(left.source_id(), first.source_id());
        assert_eq!(right.source_id(), second.source_id());
        assert_eq!(left.class(), right.class());
        assert_eq!(left.source_member_order(), right.source_member_order());
        assert_eq!(left.classfile_path(), right.classfile_path());
        assert_eq!(left.tasty_path(), right.tasty_path());
        assert_eq!(left.classfile_bytes(), right.classfile_bytes());
        assert_eq!(left.tasty_bytes(), right.tasty_bytes());
        assert_eq!(left.tasty_link_id(), right.tasty_link_id());
        let mut reader = Reader::new(left.classfile_bytes());
        let class = ClassFile::decode(&mut reader).expect("decode Rust classfile");
        assert!(reader.is_at_end());
        assert_eq!((class.version.major, class.version.minor), (65, 0));
        assert_eq!(class.constant_pool.class_name(class.this_class), Ok(name));
        let tasty = TastyFile::parse_and_validate_scala_3_9(left.tasty_bytes())
            .expect("decode Rust TASTy sidecar");
        assert_eq!(
            (
                tasty.header().major_version,
                tasty.header().minor_version,
                tasty.header().experimental_version
            ),
            (28, 9, 0)
        );
    }
    assert_ne!(first.classes()[0].class(), first.classes()[1].class());
    assert_ne!(first.classes()[1].class(), first.classes()[2].class());
    assert_ne!(first.classes()[0].class(), first.classes()[2].class());

    let directory = TestDirectory::new();
    let fresh_a = directory.path().join("fresh-a");
    let fresh_b = directory.path().join("fresh-b");
    let renamed_a = directory.path().join("renamed-a");
    let renamed_b = directory.path().join("renamed-b");
    publish_paired_batch(&fresh_a, &first, None).expect("stage first six-artifact Rust batch");
    publish_paired_batch(&fresh_b, &second, None).expect("stage second six-artifact Rust batch");
    publish_paired_batch(&renamed_a, &renamed_first, None)
        .expect("stage first renamed six-artifact Rust batch");
    publish_paired_batch(&renamed_b, &renamed_second, None)
        .expect("stage second renamed six-artifact Rust batch");
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let result = Command::new("python3")
        .arg(manifest.join("tools/tasty-pickler-oracle/run.py"))
        .arg("consume-reference-constructor")
        .arg(directory.path())
        .current_dir(manifest)
        .output()
        .expect("start strict pinned Scala three-sidecar consumer harness");
    assert!(
        result.status.success(),
        "pinned Scala reference-constructor consumer failed:\n{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let report = String::from_utf8(result.stdout).expect("consumer report is UTF-8");
    println!("Pinned Scala three-sidecar consumer result: {report}");
    for evidence in [
        "\"scala_consumer\": true",
        "\"fresh_batches_byte_identical\": true",
        "\"counter_tasty_only_get_resolved\": true",
        "\"owner_tasty_only_read_resolved\": true",
        "\"factory_tasty_only_wrap_resolved\": true",
        "\"counter_classfile_only_control_rejected\": true",
        "\"owner_classfile_only_control_rejected\": true",
        "\"factory_classfile_only_control_rejected\": true",
        "\"client_factory_wrap_methodref\": true",
        "\"client_does_not_construct_owner\": true",
        "\"runtime_outputs\": [\n    \"-9\",\n    \"-9\"\n  ]",
        "\"rust_artifacts_unchanged_after_compile_and_run\": true",
        "\"factory_tasty_constructor_call_decompiled\": true",
        "\"owner_tasty_parameter_alias_decompiled\": true",
        "\"counter_tasty_get_set_decompiled\": true",
        "\"isolated_from_tasty_semantics_verified\": true",
        "\"renamed_widget_graph_scala_consumer\": true",
        "\"renamed_widget_runtime_outputs\": [\n    \"-9\",\n    \"-9\"\n  ]",
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
            "dotty-scala-reference-constructor-consumer-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
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
