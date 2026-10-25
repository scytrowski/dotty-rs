use dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan;
use dotty_classfile::classfile::{ClassFile, Reader};
use dotty_classfile::tasty_link::TastyLinkId;
use dotty_core::SourceId;
use dotty_jvm::emit_class;
use dotty_jvm_tasty_contract::validate_classfile_tasty_link;
use dotty_tasty::tasty::TastyFile;

use dotty::jvm_tasty_artifacts::{
    PairedArtifactError, PairedBatchArtifactError, emit_paired_jvm_tasty_batch,
    emit_paired_jvm_tasty_class,
};
use dotty::jvm_tasty_plan::PairedPlanError;

const COUNTER: &str = include_str!("fixtures/jvm_tasty_interop/Counter.scala");
const COUNTER_GAUGE: &str = include_str!("fixtures/jvm_tasty_multiclass/CounterGauge.scala");

#[test]
fn counter_emits_complete_class_and_tasty_with_the_same_real_header_uuid() {
    let pair =
        emit_paired_jvm_tasty_class(COUNTER, "Counter.scala", SourceId::from_index(310)).unwrap();

    assert_eq!(pair.classfile_path(), "example/Counter.class");
    assert_eq!(pair.tasty_path(), "example/Counter.tasty");
    assert_eq!(pair.source_member_order().len(), 3);

    let tasty = TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
    assert_eq!(
        (
            tasty.header().major_version,
            tasty.header().minor_version,
            tasty.header().experimental_version,
        ),
        (28, 9, 0)
    );
    assert_eq!(
        pair.tasty_link_id(),
        TastyLinkId::from_bytes(tasty.header().uuid)
    );

    let mut reader = Reader::new(pair.classfile_bytes());
    let class = ClassFile::decode(&mut reader).unwrap();
    assert!(reader.is_at_end());
    assert_eq!((class.version.major, class.version.minor), (65, 0));
    assert_eq!(
        class.constant_pool.class_name(class.this_class).unwrap(),
        "example/Counter"
    );
    assert_eq!(class.tasty_link_id().unwrap(), Some(pair.tasty_link_id()));
    validate_classfile_tasty_link(&class, pair.tasty_bytes()).unwrap();

    let fields = class
        .fields
        .iter()
        .map(|field| {
            (
                class.constant_pool.utf8(field.name_index).unwrap(),
                class.constant_pool.utf8(field.descriptor_index).unwrap(),
                field.access_flags,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(fields.len(), 1);
    assert_eq!((fields[0].0, fields[0].1), ("current", "I"));
    assert!(
        fields[0]
            .2
            .contains(dotty_classfile::access_flags::ACC_PRIVATE)
    );
    assert!(
        !fields[0]
            .2
            .contains(dotty_classfile::access_flags::ACC_FINAL)
    );

    let mut methods = class
        .methods
        .iter()
        .map(|method| {
            (
                class.constant_pool.utf8(method.name_index).unwrap(),
                class.constant_pool.utf8(method.descriptor_index).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    methods.sort();
    assert_eq!(
        methods,
        [("<init>", "(I)V"), ("get", "()I"), ("set", "(I)V")]
    );
}

#[test]
fn standalone_jvm_emission_remains_unlinked() {
    let class_bytes = with_paired_jvm_tasty_class_plan(
        COUNTER,
        "Counter.scala",
        SourceId::from_index(318),
        |plan| {
            emit_class(
                plan.jvm_unit,
                plan.class,
                plan.tasty_plan.plan().definitions,
            )
            .unwrap()
            .into_bytes()
        },
    )
    .unwrap();
    let mut reader = Reader::new(&class_bytes);
    let class = ClassFile::decode(&mut reader).unwrap();

    assert_eq!(class.tasty_link_id().unwrap(), None);
    assert!(reader.is_at_end());
}

#[test]
fn fresh_source_sessions_produce_byte_identical_linked_pairs() {
    let first =
        emit_paired_jvm_tasty_class(COUNTER, "Counter.scala", SourceId::from_index(311)).unwrap();
    let second =
        emit_paired_jvm_tasty_class(COUNTER, "Counter.scala", SourceId::from_index(312)).unwrap();

    assert_ne!(first.source_id(), second.source_id());
    assert_eq!(first.classfile_path(), second.classfile_path());
    assert_eq!(first.tasty_path(), second.tasty_path());
    assert_eq!(first.classfile_bytes(), second.classfile_bytes());
    assert_eq!(first.tasty_bytes(), second.tasty_bytes());
    assert_eq!(first.tasty_link_id(), second.tasty_link_id());
    assert_eq!(first.source_member_order(), second.source_member_order());
}

#[test]
fn batch_emits_the_complete_ordered_class_and_tasty_manifest() {
    let batch = emit_paired_jvm_tasty_batch(
        COUNTER_GAUGE,
        "CounterGauge.scala",
        SourceId::from_index(319),
    )
    .unwrap();

    assert_eq!(batch.source_id(), SourceId::from_index(319));
    assert_eq!(batch.classes().len(), 2);
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
    for (class, (internal_name, classfile_path, tasty_path)) in batch.classes().iter().zip(expected)
    {
        let pair = class;
        assert_eq!(pair.classfile_path(), classfile_path);
        assert_eq!(pair.tasty_path(), tasty_path);
        assert_eq!(pair.source_id(), batch.source_id());

        let tasty = TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
        assert_eq!(
            (
                tasty.header().major_version,
                tasty.header().minor_version,
                tasty.header().experimental_version,
            ),
            (28, 9, 0)
        );
        assert_eq!(
            pair.tasty_link_id(),
            TastyLinkId::from_bytes(tasty.header().uuid)
        );

        let mut reader = Reader::new(pair.classfile_bytes());
        let class_file = ClassFile::decode(&mut reader).unwrap();
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
            internal_name
        );
        assert_eq!(
            class_file.tasty_link_id().unwrap(),
            Some(pair.tasty_link_id())
        );
        validate_classfile_tasty_link(&class_file, pair.tasty_bytes()).unwrap();
    }
}

#[test]
fn fresh_batch_sessions_produce_byte_identical_complete_manifests() {
    let first = emit_paired_jvm_tasty_batch(
        COUNTER_GAUGE,
        "CounterGauge.scala",
        SourceId::from_index(320),
    )
    .unwrap();
    let second = emit_paired_jvm_tasty_batch(
        COUNTER_GAUGE,
        "CounterGauge.scala",
        SourceId::from_index(321),
    )
    .unwrap();

    assert_ne!(first.source_id(), second.source_id());
    assert_eq!(first.classes().len(), second.classes().len());
    for (left, right) in first.classes().iter().zip(second.classes()) {
        assert_eq!(left.classfile_path(), right.classfile_path());
        assert_eq!(left.classfile_bytes(), right.classfile_bytes());
        assert_eq!(left.tasty_path(), right.tasty_path());
        assert_eq!(left.tasty_bytes(), right.tasty_bytes());
        assert_eq!(left.tasty_link_id(), right.tasty_link_id());
    }
}

#[test]
fn unsupported_second_class_rejects_the_entire_batch() {
    let source = "package example\n\nclass Counter(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n\nclass Gauge(start: Int):\n  private var reading: Int = start\n  def get(): Int = reading\n  def set(value: Int): Unit = while reading < value do reading = reading + 1\n";
    assert!(matches!(
        emit_paired_jvm_tasty_batch(source, "CounterGauge.scala", SourceId::from_index(322)),
        Err(PairedBatchArtifactError::Plan(
            PairedPlanError::BatchClass { class_index: 1, .. }
        ))
    ));
}

#[test]
fn unsupported_method_body_rejects_before_returning_either_artifact() {
    let source = "package example\nclass Counter(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = while current < value do current = current + 1\n";
    assert!(matches!(
        emit_paired_jvm_tasty_class(source, "Counter.scala", SourceId::from_index(313)),
        Err(PairedArtifactError::Plan(_))
    ));
}

#[test]
fn pair_publisher_stages_both_artifacts_before_exposing_them() {
    let pair =
        emit_paired_jvm_tasty_class(COUNTER, "Counter.scala", SourceId::from_index(314)).unwrap();
    let output = TestDirectory::new("publish-success");
    publish_pair(output.path(), &pair, None).unwrap();

    assert_eq!(
        std::fs::read(output.path().join(pair.classfile_path())).unwrap(),
        pair.classfile_bytes()
    );
    assert_eq!(
        std::fs::read(output.path().join(pair.tasty_path())).unwrap(),
        pair.tasty_bytes()
    );
    assert_eq!(std::fs::read_dir(output.path()).unwrap().count(), 1);
}

#[test]
fn pair_publisher_cleans_staging_after_failure_following_first_staged_artifact() {
    let pair =
        emit_paired_jvm_tasty_class(COUNTER, "Counter.scala", SourceId::from_index(315)).unwrap();
    let output = TestDirectory::new("publish-failure");

    assert!(publish_pair(output.path(), &pair, Some(1)).is_err());
    assert_eq!(std::fs::read_dir(output.path()).unwrap().count(), 0);
}

#[test]
fn batch_publisher_promotes_the_complete_manifest_and_preserves_every_byte() {
    let batch = emitted_batch(323);
    let base = TestDirectory::new("batch-publish-success");
    let destination = base.path().join("classes");

    let published = publish_paired_batch(&destination, &batch, None).unwrap();
    let expected = [
        "example/Counter.class",
        "example/Counter.tasty",
        "example/Gauge.class",
        "example/Gauge.tasty",
    ];
    assert_eq!(
        published
            .iter()
            .map(|path| path.strip_prefix(&destination).unwrap().to_str().unwrap())
            .collect::<Vec<_>>(),
        expected
    );
    let inventory = read_relative_files(&destination).unwrap();
    assert_eq!(inventory, expected.map(str::to_owned));
    for pair in batch.classes() {
        assert_eq!(
            std::fs::read(destination.join(pair.classfile_path())).unwrap(),
            pair.classfile_bytes()
        );
        assert_eq!(
            std::fs::read(destination.join(pair.tasty_path())).unwrap(),
            pair.tasty_bytes()
        );
    }

    let sentinel = destination.join("example/Counter.class");
    let original = std::fs::read(&sentinel).unwrap();
    assert!(matches!(
        publish_paired_batch(&destination, &batch, None),
        Err(BatchPublishError::DestinationExists { .. })
    ));
    assert_eq!(std::fs::read(sentinel).unwrap(), original);
}

#[test]
fn malformed_second_sidecar_fails_before_creating_staging_or_destination() {
    let batch = emitted_batch(324);
    let mut pairs = batch_publish_pairs(&batch);
    let malformed: [u8; 0] = [];
    pairs[1].tasty_bytes = &malformed;
    let base = TestDirectory::new("batch-publish-invalid");
    let destination = base.path().join("classes");

    assert!(matches!(
        publish_batch_pairs(&destination, &pairs, None),
        Err(BatchPublishError::InvalidManifest {
            class_index: 1,
            phase: "TASTy decode",
            ..
        })
    ));
    assert!(!destination.exists());
    assert_eq!(std::fs::read_dir(base.path()).unwrap().count(), 0);
}

#[test]
fn batch_publisher_enforces_class_and_byte_bounds_before_decoding_or_writing() {
    let batch = emitted_batch(329);
    let base = TestDirectory::new("batch-publish-bounds");
    let original_pairs = batch_publish_pairs(&batch);

    let mut too_many = original_pairs.iter().take(1).cloned().collect::<Vec<_>>();
    too_many.resize_with(MAX_BATCH_CLASSES + 1, || BatchPublishPair {
        class: original_pairs[0].class,
        source_id: original_pairs[0].source_id,
        classfile_path: original_pairs[0].classfile_path,
        classfile_bytes: original_pairs[0].classfile_bytes,
        tasty_path: original_pairs[0].tasty_path,
        tasty_bytes: original_pairs[0].tasty_bytes,
    });
    let too_many_destination = base.path().join("too-many");
    assert!(matches!(
        publish_batch_pairs(&too_many_destination, &too_many, None),
        Err(BatchPublishError::InvalidManifest {
            phase: "manifest shape",
            ..
        })
    ));
    assert!(!too_many_destination.exists());

    let oversized = vec![0; MAX_BATCH_ARTIFACT_BYTES + 1];
    let mut too_large = original_pairs;
    too_large[0].classfile_bytes = &oversized;
    let too_large_destination = base.path().join("too-large");
    assert!(matches!(
        publish_batch_pairs(&too_large_destination, &too_large, None),
        Err(BatchPublishError::InvalidManifest {
            phase: "manifest size",
            ..
        })
    ));
    assert!(!too_large_destination.exists());
    assert_eq!(std::fs::read_dir(base.path()).unwrap().count(), 0);
}

#[test]
fn batch_publisher_cleans_every_staged_artifact_on_handled_failures() {
    let batch = emitted_batch(325);
    let base = TestDirectory::new("batch-publish-faults");
    for (index, fault) in [
        BatchPublishFault::AfterStagedFiles(1),
        BatchPublishFault::AfterStagedFiles(2),
        BatchPublishFault::AfterStagedFiles(3),
        BatchPublishFault::AfterStagedFiles(4),
        BatchPublishFault::BeforePromotion,
        BatchPublishFault::AfterPromotion,
    ]
    .into_iter()
    .enumerate()
    {
        let destination = base.path().join(format!("classes-{index}"));
        let error = publish_paired_batch(&destination, &batch, Some(fault)).unwrap_err();
        assert!(matches!(error, BatchPublishError::Io { .. }));
        assert!(!destination.exists(), "fault case {index}: {error:?}");
        assert_eq!(std::fs::read_dir(base.path()).unwrap().count(), 0);
    }
}

#[test]
fn batch_publisher_rejects_unsafe_or_conflicting_paths_before_writing() {
    let batch = emitted_batch(326);
    let base = TestDirectory::new("batch-publish-paths");
    let bad_paths = [
        "../Gauge.class",
        "/absolute/Gauge.class",
        "C:/drive/Gauge.class",
        "example\\Gauge.class",
        "example//Gauge.class",
        "example/./Gauge.class",
        "example/Gauge.class/",
    ];
    for (index, path) in bad_paths.into_iter().enumerate() {
        let mut pairs = batch_publish_pairs(&batch);
        pairs[1].classfile_path = path;
        let destination = base.path().join(format!("unsafe-{index}"));
        assert!(matches!(
            publish_batch_pairs(&destination, &pairs, None),
            Err(BatchPublishError::InvalidManifest { .. })
        ));
        assert!(!destination.exists());
    }

    let mut duplicates = batch_publish_pairs(&batch);
    duplicates[1].classfile_path = duplicates[0].classfile_path;
    let duplicate_destination = base.path().join("duplicate");
    assert!(matches!(
        publish_batch_pairs(&duplicate_destination, &duplicates, None),
        Err(BatchPublishError::InvalidManifest {
            phase: "artifact paths",
            ..
        })
    ));
    assert!(!duplicate_destination.exists());
    assert_eq!(std::fs::read_dir(base.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn batch_publisher_rejects_symlinked_output_parent_without_following_it() {
    use std::os::unix::fs::symlink;

    let batch = emitted_batch(327);
    let base = TestDirectory::new("batch-publish-symlink");
    let real_parent = base.path().join("real");
    std::fs::create_dir(&real_parent).unwrap();
    let linked_parent = base.path().join("linked");
    symlink(&real_parent, &linked_parent).unwrap();
    let destination = linked_parent.join("classes");

    assert!(matches!(
        publish_paired_batch(&destination, &batch, None),
        Err(BatchPublishError::InvalidDestination { .. })
    ));
    assert!(!destination.exists());
    assert_eq!(std::fs::read_dir(&real_parent).unwrap().count(), 0);
}

#[test]
fn batch_publisher_leaves_a_preexisting_output_tree_untouched() {
    let batch = emitted_batch(328);
    let base = TestDirectory::new("batch-publish-existing");
    let destination = base.path().join("classes");
    std::fs::create_dir(&destination).unwrap();
    let sentinel = destination.join("sentinel");
    std::fs::write(&sentinel, b"keep this file").unwrap();

    assert!(matches!(
        publish_paired_batch(&destination, &batch, None),
        Err(BatchPublishError::DestinationExists { .. })
    ));
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"keep this file");
    assert_eq!(read_relative_files(&destination).unwrap(), ["sentinel"]);
}

#[test]
fn equal_simple_names_in_different_packages_keep_distinct_pair_paths() {
    let left = COUNTER.replace("package example", "package left");
    let right = COUNTER.replace("package example", "package right");
    let left =
        emit_paired_jvm_tasty_class(&left, "Counter.scala", SourceId::from_index(316)).unwrap();
    let right =
        emit_paired_jvm_tasty_class(&right, "Counter.scala", SourceId::from_index(317)).unwrap();

    assert_eq!(left.classfile_path(), "left/Counter.class");
    assert_eq!(left.tasty_path(), "left/Counter.tasty");
    assert_eq!(right.classfile_path(), "right/Counter.class");
    assert_eq!(right.tasty_path(), "right/Counter.tasty");
}

struct TestDirectory(std::path::PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);

        let path = std::env::temp_dir().join(format!(
            "dotty-jvm-tasty-pair-{}-{label}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn publish_pair(
    output: &std::path::Path,
    pair: &dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts,
    fail_after_staged_files: Option<usize>,
) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT_STAGING_ID: AtomicUsize = AtomicUsize::new(0);

    if std::fs::read_dir(output)?.next().is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "paired output directory must be empty",
        ));
    }
    let staging = output.join(format!(
        ".dotty-jvm-tasty-pair-{}-{}",
        std::process::id(),
        NEXT_STAGING_ID.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&staging)?;

    let artifacts = [
        (pair.classfile_path(), pair.classfile_bytes()),
        (pair.tasty_path(), pair.tasty_bytes()),
    ];
    let stage_result = (|| {
        for (index, (relative, bytes)) in artifacts.iter().enumerate() {
            let relative = std::path::Path::new(relative);
            if relative.as_os_str().is_empty()
                || !relative
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_)))
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "unsafe paired artifact path",
                ));
            }
            let staged = staging.join("new").join(relative);
            if let Some(parent) = staged.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(staged, bytes)?;
            if fail_after_staged_files == Some(index + 1) {
                return Err(std::io::Error::other("injected staging write failure"));
            }
        }
        Ok(())
    })();
    if let Err(error) = stage_result {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }

    let mut published = Vec::new();
    let publish_result = (|| {
        for (relative, _) in artifacts {
            let relative = std::path::Path::new(relative);
            let destination = output.join(relative);
            let staged = staging.join("new").join(relative);
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(staged, &destination)?;
            published.push(destination);
        }
        Ok(())
    })();
    if let Err(error) = publish_result {
        for path in published {
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }
    let _ = std::fs::remove_dir_all(staging);
    Ok(())
}

#[path = "support/jvm_tasty_batch_publisher.rs"]
mod batch_publisher;
use batch_publisher::{
    BatchPublishError, BatchPublishFault, BatchPublishPair, MAX_BATCH_ARTIFACT_BYTES,
    MAX_BATCH_CLASSES, batch_publish_pairs, emitted_batch, publish_batch_pairs,
    publish_paired_batch, read_relative_files,
};
