use std::path::{Path, PathBuf};
use std::process::Command;

use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_batch;
use dotty_classfile::classfile::{ClassFile, Reader};
use dotty_core::SourceId;
use dotty_tasty::ast::{
    APPLY_TAG, ASSIGN_TAG, DEFDEF_TAG, IDENTTPT_TAG, NEW_TAG, PARAM_TAG, QUALTHIS_TAG, SELECT_TAG,
    SELECTIN_TAG,
};
use dotty_tasty::tasty::{RenderedParamSig, RenderedSignedName, TastyFile};
use dotty_tasty::term::{AstRefKind, RawTree};

#[path = "support/jvm_tasty_batch_publisher.rs"]
#[allow(dead_code)]
mod batch_publisher;
use batch_publisher::publish_paired_batch;

const SOURCE: &str =
    include_str!("fixtures/jvm_tasty_reference_constructor_calls/CounterOwnerFactory.scala");
const CONSUMER_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_reference_constructor_calls/ScalaConsumer.scala");
const REQUIRE_CONSUMER: &str = "DOTTY_REQUIRE_SCALA_TASTY_REFERENCE_CONSTRUCTOR_CONSUMER";

fn method_body<'a>(
    tasty: &'a TastyFile<'a>,
    name: &str,
) -> (u32, dotty_tasty::ast::DefDefBody<'a>) {
    let index = tasty
        .ast_address_index()
        .expect("valid TASTy AST address index");
    index
        .iter_nodes_with_tag(DEFDEF_TAG)
        .find_map(|node| {
            let address = node.offset as u32;
            let body = index.get(address)?.decode_defdef_body().ok()?;
            (tasty.names().render(body.name).ok()? == name).then_some((address, body))
        })
        .unwrap_or_else(|| panic!("TASTy contains method {name}"))
}

fn parameter_address(index: &dotty_tasty::ast::AstAddressIndex<'_>, owner: u32) -> u32 {
    index
        .iter_nodes_with_tag(PARAM_TAG)
        .find_map(|candidate| {
            let mut parent = Some(candidate);
            while let Some(node) = parent {
                if node.offset as u32 == owner {
                    return Some(candidate.offset as u32);
                }
                parent = index.parent_of(node.offset as u32);
            }
            None
        })
        .expect("method owns a formal PARAM")
}

fn is_counter_current_selection(tasty: &TastyFile<'_>, tree: &RawTree<'_>) -> bool {
    let RawTree::NatAst {
        tag: SELECT_TAG,
        value: name,
        child: qualifier,
        ..
    } = tree
    else {
        return false;
    };
    if tasty.names().render(*name).ok().as_deref() != Some("current") {
        return false;
    }
    let RawTree::Ast {
        tag: QUALTHIS_TAG,
        child: class_type,
        ..
    } = qualifier.as_ref()
    else {
        return false;
    };
    matches!(
        class_type.as_ref(),
        RawTree::NatAst {
            tag: IDENTTPT_TAG,
            value,
            ..
        } if tasty.names().render(*value).ok().as_deref() == Some("Counter")
    )
}

fn counter_source_body_failure(tasty: &TastyFile<'_>) -> Option<&'static str> {
    let (_, get) = method_body(tasty, "get");
    if !get
        .rhs
        .as_ref()
        .is_some_and(|rhs| is_counter_current_selection(tasty, rhs))
    {
        return Some("get does not select Counter.current through Counter.this");
    }

    let (set_address, set) = method_body(tasty, "set");
    let index = tasty.ast_address_index().ok()?;
    let [formal] = set.parameters.as_slice() else {
        return Some("set does not have exactly one formal parameter");
    };
    if tasty.names().render(formal.name()).ok().as_deref() != Some("value") {
        return Some("set formal is not the source value parameter");
    }
    let formal_address = parameter_address(&index, set_address);
    let Some(RawTree::LengthNode(assignment_node)) = set.rhs.as_ref() else {
        return Some("set body is not an assignment");
    };
    if assignment_node.tag != ASSIGN_TAG {
        return Some("set body is not an assignment");
    }
    let Ok(assignment) = assignment_node.decode_assign() else {
        return Some("set assignment does not decode");
    };
    if !is_counter_current_selection(tasty, &assignment.left) {
        return Some("set assignment does not target Counter.current through Counter.this");
    }
    let references = assignment.right.ast_refs();
    if !matches!(
        references.as_slice(),
        [reference]
            if reference.kind == AstRefKind::TermRefDirect
                && reference.address == formal_address
    ) {
        return Some("set assignment RHS does not reference its original value PARAM");
    }
    None
}

fn assert_factory_constructor_argument_provenance(tasty: &TastyFile<'_>) {
    let (wrap_address, wrap) = method_body(tasty, "wrap");
    let index = tasty
        .ast_address_index()
        .expect("valid Factory address index");
    let [formal] = wrap.parameters.as_slice() else {
        panic!("Factory.wrap has exactly one formal");
    };
    assert_eq!(tasty.names().render(formal.name()).unwrap(), "counter");
    let formal_address = parameter_address(&index, wrap_address);
    let Some(RawTree::LengthNode(application_node)) = wrap.rhs.as_ref() else {
        panic!("Factory.wrap body is an APPLY");
    };
    assert_eq!(application_node.tag, APPLY_TAG);
    let application = application_node
        .decode_apply()
        .expect("decode Factory APPLY");
    let [argument] = application.arguments.as_slice() else {
        panic!("Factory constructor APPLY has one argument");
    };
    let RawTree::LengthNode(select_node) = &application.function else {
        panic!("Factory constructor function is a SELECTin");
    };
    assert_eq!(select_node.tag, SELECTIN_TAG);
    let select = select_node
        .decode_select_in()
        .expect("decode Factory SELECTin");
    let RenderedSignedName::Signed {
        original,
        signature,
    } = tasty
        .names()
        .render_signed_name(select.name)
        .unwrap()
        .expect("constructor name is signed")
    else {
        panic!("Factory constructor reference has a signed name");
    };
    assert_eq!(original, "<init>");
    assert_eq!(signature.result, "example.CounterOwner");
    assert_eq!(
        signature.parameters,
        [RenderedParamSig::TermParameter(
            "example.Counter".to_owned()
        )]
    );
    assert!(matches!(
        &select.qualifier,
        RawTree::Ast { tag: NEW_TAG, .. }
    ));
    assert!(matches!(
        argument.ast_refs().as_slice(),
        [reference]
            if reference.kind == AstRefKind::TermRefDirect
                && reference.address == formal_address
    ));
}

fn assert_counter_source_body_contract(tasty_bytes: &[u8]) {
    let tasty = TastyFile::parse_and_validate_scala_3_9(tasty_bytes)
        .expect("Rust-produced Counter sidecar is valid Scala 3.9 TASTy");
    assert_eq!(
        counter_source_body_failure(&tasty),
        None,
        "Counter.get and Counter.set retain their source body references"
    );
}

#[test]
fn emitted_reference_constructor_tasty_preserves_counter_bodies_and_formal_identity() {
    let batch = emit_paired_jvm_tasty_batch(
        SOURCE,
        "CounterOwnerFactory.scala",
        SourceId::from_index(17995),
    )
    .expect("emit Counter/Owner/Factory source batch");
    assert_counter_source_body_contract(batch.classes()[0].tasty_bytes());
    let factory_tasty = TastyFile::parse_and_validate_scala_3_9(batch.classes()[2].tasty_bytes())
        .expect("Rust-produced Factory sidecar is valid Scala 3.9 TASTy");
    assert_factory_constructor_argument_provenance(&factory_tasty);

    let constant_getter = SOURCE.replace("def get(): Int = current", "def get(): Int = 0");
    assert_ne!(
        constant_getter, SOURCE,
        "constant-getter control mutates the fixture"
    );
    let constant_getter_batch = emit_paired_jvm_tasty_batch(
        &constant_getter,
        "CounterOwnerFactory.scala",
        SourceId::from_index(17996),
    )
    .expect("emit constant-getter negative control");
    let constant_getter_tasty =
        TastyFile::parse_and_validate_scala_3_9(constant_getter_batch.classes()[0].tasty_bytes())
            .expect("constant-getter control still has valid TASTy");
    assert_eq!(
        counter_source_body_failure(&constant_getter_tasty),
        Some("get does not select Counter.current through Counter.this")
    );

    let no_op_setter = SOURCE.replace(
        "def set(value: Int): Unit = current = value",
        "def set(value: Int): Unit = ()",
    );
    assert_ne!(
        no_op_setter, SOURCE,
        "no-op-setter control mutates the fixture"
    );
    let no_op_setter_batch = emit_paired_jvm_tasty_batch(
        &no_op_setter,
        "CounterOwnerFactory.scala",
        SourceId::from_index(17997),
    )
    .expect("emit no-op-setter negative control");
    let no_op_setter_tasty =
        TastyFile::parse_and_validate_scala_3_9(no_op_setter_batch.classes()[0].tasty_bytes())
            .expect("no-op-setter control still has valid TASTy");
    assert_eq!(
        counter_source_body_failure(&no_op_setter_tasty),
        Some("set body is not an assignment")
    );
}

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
        "\"counter_tasty_source_bodies_verified\": true",
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
            "dotty-scala-reference-constructor-consumer-{}-{}-{}",
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
