use std::path::{Path, PathBuf};
use std::process::Command;

use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_batch;
use dotty_classfile::classfile::{
    Attribute, ClassFile, ConstantPoolEntry, ConstantPoolIndex, Reader,
};
use dotty_core::SourceId;
use dotty_jvm_tasty_contract::validate_classfile_tasty_link;
use dotty_tasty::tasty::TastyFile;

#[path = "support/jvm_tasty_batch_publisher.rs"]
#[allow(dead_code)]
mod batch_publisher;
use batch_publisher::publish_paired_batch;

const SOURCE: &str =
    include_str!("fixtures/jvm_tasty_cross_class_factories/CounterFactoryRuntime.scala");
const REQUIRE_CONSUMER: &str = "DOTTY_REQUIRE_SCALA_TASTY_FACTORY_CONSUMER";

#[test]
fn pinned_scala_consumes_rust_factory_tasty_and_runs_original_classes() {
    if std::env::var_os(REQUIRE_CONSUMER).is_none() {
        eprintln!(
            "skipped pinned Scala factory consumer gate; set {REQUIRE_CONSUMER}=1 to require it"
        );
        return;
    }

    let first = emit_paired_jvm_tasty_batch(
        SOURCE,
        "CounterFactoryRuntime.scala",
        SourceId::from_index(16961),
    )
    .expect("emit first complete Rust Counter/CounterFactory batch");
    let second = emit_paired_jvm_tasty_batch(
        SOURCE,
        "CounterFactoryRuntime.scala",
        SourceId::from_index(16962),
    )
    .expect("emit second complete fresh Rust Counter/CounterFactory batch");
    assert_ne!(first.source_id(), second.source_id());
    assert_eq!(first.classes().len(), 2);

    let expected = [
        (
            "example/Counter",
            "example/Counter.class",
            "example/Counter.tasty",
        ),
        (
            "example/CounterFactory",
            "example/CounterFactory.class",
            "example/CounterFactory.tasty",
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
        let class_file = ClassFile::decode(&mut reader).expect("decode Rust classfile");
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
        let tasty = TastyFile::parse_and_validate_scala_3_9(left.tasty_bytes())
            .expect("decode Rust TASTy sidecar");
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

    let factory = &first.classes()[1];
    let mut reader = Reader::new(factory.classfile_bytes());
    let factory_class = ClassFile::decode(&mut reader).expect("decode Rust CounterFactory.class");
    let create = factory_class
        .methods
        .iter()
        .find(|method| factory_class.constant_pool.utf8(method.name_index).unwrap() == "create")
        .expect("CounterFactory.create exists");
    let [Attribute::Code(code)] = create.attributes.as_slice() else {
        panic!("CounterFactory.create has one Code attribute");
    };
    let mut offset = 0;
    assert_eq!(code.code.get(offset), Some(&0xbb), "new Counter");
    offset += 1;
    let new_operand = &code.code[offset..offset + 2];
    let new_class = ConstantPoolIndex(u16::from_be_bytes([new_operand[0], new_operand[1]]));
    assert_eq!(
        factory_class.constant_pool.class_name(new_class).unwrap(),
        "example/Counter"
    );
    offset += 2;
    assert_eq!(
        code.code.get(offset),
        Some(&0x59),
        "dup preserves return value"
    );
    offset += 1;
    assert_eq!(code.code.get(offset), Some(&0x1b), "iload_1 loads seed");
    offset += 1;
    assert_eq!(
        code.code.get(offset),
        Some(&0xb7),
        "invokespecial initializes Counter"
    );
    offset += 1;
    let constructor_operand = &code.code[offset..offset + 2];
    let constructor = ConstantPoolIndex(u16::from_be_bytes([
        constructor_operand[0],
        constructor_operand[1],
    ]));
    let ConstantPoolEntry::Methodref {
        class_index,
        name_and_type_index,
    } = factory_class.constant_pool.get(constructor).unwrap()
    else {
        panic!("constructor call uses CONSTANT_Methodref");
    };
    assert_eq!(
        factory_class
            .constant_pool
            .class_name(*class_index)
            .unwrap(),
        "example/Counter"
    );
    assert_eq!(
        factory_class
            .constant_pool
            .name_and_type(*name_and_type_index)
            .unwrap(),
        ("<init>", "(I)V")
    );
    offset += 2;
    assert_eq!(
        code.code.get(offset),
        Some(&0xb0),
        "areturn returns Counter"
    );
    offset += 1;
    assert_eq!(offset, code.code.len(), "no extra constructor instructions");

    let directory = TestDirectory::new();
    let fresh_a = directory.path().join("fresh-a");
    let fresh_b = directory.path().join("fresh-b");
    publish_paired_batch(&fresh_a, &first, None).expect("stage first complete Rust batch");
    publish_paired_batch(&fresh_b, &second, None).expect("stage second complete Rust batch");
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

    for (index, (class_name, method_name, descriptor)) in [
        ("Counter", "get", "()I"),
        ("CounterFactory", "create", "(I)Lexample/Counter;"),
    ]
    .into_iter()
    .enumerate()
    {
        let pair = &first.classes()[index];
        let tasty_only_root = directory.path().join(format!("tasty-only-{class_name}"));
        copy_tree(&fresh_a, &tasty_only_root);
        let without_method = remove_method(pair.classfile_bytes(), method_name, descriptor);
        std::fs::write(tasty_only_root.join(pair.classfile_path()), without_method).unwrap();

        let classfile_only_root = directory
            .path()
            .join(format!("classfile-only-{class_name}"));
        copy_tree(&tasty_only_root, &classfile_only_root);
        std::fs::remove_file(classfile_only_root.join(pair.tasty_path())).unwrap();

        let missing_classfile_root = directory
            .path()
            .join(format!("missing-classfile-{class_name}"));
        copy_tree(&fresh_a, &missing_classfile_root);
        std::fs::remove_file(missing_classfile_root.join(pair.classfile_path())).unwrap();

        let uuid_mismatch_root = directory.path().join(format!("uuid-mismatch-{class_name}"));
        copy_tree(&fresh_a, &uuid_mismatch_root);
        std::fs::write(
            uuid_mismatch_root.join(pair.classfile_path()),
            mismatch_link_identifier(pair.classfile_bytes(), pair.tasty_link_id()),
        )
        .unwrap();

        let missing_sidecar_root = directory
            .path()
            .join(format!("missing-sidecar-{class_name}"));
        copy_tree(&fresh_a, &missing_sidecar_root);
        std::fs::remove_file(missing_sidecar_root.join(pair.tasty_path())).unwrap();

        let wrong_path_root = directory.path().join(format!("wrong-path-{class_name}"));
        copy_tree(&fresh_a, &wrong_path_root);
        let misplaced = wrong_path_root
            .join("misplaced")
            .join(format!("{class_name}.tasty"));
        std::fs::create_dir_all(misplaced.parent().unwrap()).unwrap();
        std::fs::rename(wrong_path_root.join(pair.tasty_path()), misplaced).unwrap();

        let corrupt_root = directory
            .path()
            .join(format!("corrupt-sidecar-{class_name}"));
        copy_tree(&fresh_a, &corrupt_root);
        let corrupt_path = corrupt_root.join(pair.tasty_path());
        let mut corrupt = std::fs::read(&corrupt_path).unwrap();
        corrupt[0] ^= 0xff;
        std::fs::write(corrupt_path, corrupt).unwrap();
    }

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let result = Command::new("python3")
        .arg(manifest.join("tools/tasty-pickler-oracle/run.py"))
        .arg("consume-factory")
        .arg(directory.path())
        .current_dir(manifest)
        .output()
        .expect("start pinned Scala factory consumer harness");
    assert!(
        result.status.success(),
        "pinned Scala factory consumer failed:\n{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let report = String::from_utf8(result.stdout).expect("consumer report is UTF-8");
    println!("Pinned Scala factory consumer result: {report}");
    for evidence in [
        "\"scala_consumer\": true",
        "\"fresh_batches_byte_identical\": true",
        "\"factory_tasty_only_create_resolved\": true",
        "\"counter_tasty_only_get_resolved\": true",
        "\"factory_classfile_only_control_rejected\": true",
        "\"counter_classfile_only_control_rejected\": true",
        "\"factory_missing_classfile_tasty_compile_succeeded\": true",
        "\"counter_missing_classfile_tasty_compile_succeeded\": true",
        "\"factory_missing_classfile_runtime_rejected\": true",
        "\"counter_missing_classfile_runtime_rejected\": true",
        "\"factory_uuid_mismatch_control_rejected\": true",
        "\"counter_uuid_mismatch_control_rejected\": true",
        "\"factory_missing_sidecar_control_rejected\": true",
        "\"counter_missing_sidecar_control_rejected\": true",
        "\"factory_wrong_path_control_rejected\": true",
        "\"counter_wrong_path_control_rejected\": true",
        "\"factory_corrupt_sidecar_control_rejected\": true",
        "\"counter_corrupt_sidecar_control_rejected\": true",
        "\"rust_artifacts_unchanged_after_compile_and_run\": true",
        "\"runtime_outputs\": [\n    \"42\",\n    \"42\"\n  ]",
    ] {
        assert!(report.contains(evidence), "missing {evidence} in {report}");
    }
}

fn remove_method(class_bytes: &[u8], method_name: &str, descriptor: &str) -> Vec<u8> {
    let mut reader = Reader::new(class_bytes);
    let mut class = ClassFile::decode(&mut reader).expect("decode emitted class");
    assert!(reader.is_at_end());
    let before = class.methods.len();
    class.methods.retain(|method| {
        class
            .constant_pool
            .utf8(method.name_index)
            .expect("method name resolves")
            != method_name
            || class
                .constant_pool
                .utf8(method.descriptor_index)
                .expect("method descriptor resolves")
                != descriptor
    });
    assert_eq!(
        class.methods.len() + 1,
        before,
        "method {method_name} exists once"
    );
    class.encode().expect("encode TASTy-only method control")
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
    assert_eq!(matches.len(), 1, "TASTY UUID occurs once in classfile");
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
            "dotty-scala-factory-consumer-{}-{}",
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
