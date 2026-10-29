use std::path::{Path, PathBuf};
use std::process::Command;

use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_batch;
use dotty_core::SourceId;
use dotty_tasty::ast::{
    APPLY_TAG, AstError, BLOCK_TAG, DEFDEF_TAG, IDENTTPT_TAG, SELECTIN_TAG, TERMREFDIRECT_TAG,
    VALDEF_TAG,
};
use dotty_tasty::file::TastyFile;
use dotty_tasty::reader::Reader;
use dotty_tasty::section::StandardSection;
use dotty_tasty::tasty::RenderedSignedName;
use dotty_tasty::term::{AstRefKind, RawTree, TermError};

#[path = "support/jvm_tasty_batch_publisher.rs"]
#[allow(dead_code)]
mod batch_publisher;
use batch_publisher::publish_paired_batch;

const SOURCE: &str =
    include_str!("fixtures/jvm_tasty_reference_local_constructor_flows/CounterOwnerFactory.scala");
const REQUIRE_CONSUMER: &str = "DOTTY_REQUIRE_SCALA_TASTY_REFERENCE_LOCAL_CONSUMER";

#[test]
fn pinned_scala_consumes_reference_local_rust_tasty_and_executes_original_classes() {
    let first = emit_paired_jvm_tasty_batch(
        SOURCE,
        "CounterOwnerFactory.scala",
        SourceId::from_index(18551),
    )
    .expect("emit original six-file Rust batch from one source session");
    let second = emit_paired_jvm_tasty_batch(
        SOURCE,
        "CounterOwnerFactory.scala",
        SourceId::from_index(18552),
    )
    .expect("emit independent same-source Rust batch");
    assert_ne!(first.source_id(), second.source_id());
    assert_eq!(first.classes().len(), 3);
    let factory = first
        .classes()
        .iter()
        .find(|artifact| {
            artifact
                .classfile_path()
                .ends_with("CounterOwnerFactory.class")
        })
        .expect("the original batch includes Factory");
    assert_rust_local_tasty_structure(factory.tasty_bytes());

    if std::env::var(REQUIRE_CONSUMER).as_deref() != Ok("1") {
        eprintln!(
            "skipped pinned Scala reference-local consumer gate; set {REQUIRE_CONSUMER}=1 to require it"
        );
        return;
    }

    let renamed_reordered = SOURCE
        .replace("package example", "package renamed")
        .replace("CounterOwnerFactory", "WidgetBuilder")
        .replace("CounterOwner", "WidgetHolder")
        .replace("Counter", "Widget")
        .replace("val counter: Widget", "val item: Widget")
        .replace("new WidgetHolder(counter)", "new WidgetHolder(item)");
    let renamed_reordered = reorder_widget_classes(&renamed_reordered);
    let renamed_first = emit_paired_jvm_tasty_batch(
        &renamed_reordered,
        "RenamedReferenceLocal.scala",
        SourceId::from_index(18553),
    )
    .expect("emit renamed/reordered Rust batch");
    let renamed_second = emit_paired_jvm_tasty_batch(
        &renamed_reordered,
        "RenamedReferenceLocal.scala",
        SourceId::from_index(18554),
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
        .arg("--reference-local")
        .current_dir(manifest)
        .output()
        .expect("start pinned Scala reference-local consumer harness");
    assert!(
        result.status.success(),
        "required pinned Scala reference-local consumer failed:\n{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let report = String::from_utf8(result.stdout).expect("consumer evidence is UTF-8");
    println!("Pinned Scala reference-local consumer result: {report}");
    for evidence in [
        "\"scala_consumer\": true",
        "\"runtime_outputs\": [",
        "scala39-reference-local-constructor-flow=ok",
        "\"counter_tasty_only_get_resolved\": true",
        "\"owner_tasty_only_read_resolved\": true",
        "\"factory_tasty_only_create_resolved\": true",
        "\"factory_tasty_only_make_counter_resolved\": true",
        "\"counter_classfile_only_control_rejected\": true",
        "\"owner_classfile_only_control_rejected\": true",
        "\"factory_classfile_only_create_control_rejected\": true",
        "\"factory_classfile_only_make_counter_control_rejected\": true",
        "\"factory_create_decompile_has_block_valdef_helper_and_local_use\": true",
        "\"per_class_sidecar_matrix_rejected\": true",
        "\"renamed_reordered_scala_consumer\": true",
        "\"rust_artifacts_unchanged_after_compile_and_run\": true",
    ] {
        assert!(report.contains(evidence), "missing {evidence} in {report}");
    }
}

fn assert_rust_local_tasty_structure(bytes: &[u8]) {
    let tasty = TastyFile::parse_and_validate_scala_3_9(bytes).expect("valid Rust Factory TASTy");
    let index = tasty.ast_address_index().expect("index Rust Factory TASTy");
    let (create_address, create) = index
        .iter_nodes_with_tag(DEFDEF_TAG)
        .find_map(|node| {
            let address = node.offset as u32;
            let body = index.get(address)?.decode_defdef_body().ok()?;
            (tasty.names().render(body.name).ok()? == "create").then_some((address, body))
        })
        .expect("Rust Factory TASTy contains create");
    let RawTree::LengthNode(block_node) = create.rhs.expect("create has source body") else {
        panic!("Rust create body is an AST node");
    };
    assert_eq!(block_node.tag, BLOCK_TAG);
    let block_address = index
        .iter_nodes_with_tag(BLOCK_TAG)
        .find(|node| node.offset as u32 > create_address)
        .expect("create has a visible BLOCK address")
        .offset as u32;
    let block = index
        .get(block_address)
        .expect("BLOCK address is indexed")
        .decode_block()
        .expect("decode Rust create BLOCK");
    assert_eq!(block.stats.len(), 1);
    let RawTree::LengthNode(local_node) = &block.stats[0] else {
        panic!("Rust create BLOCK keeps the original VALDEF");
    };
    assert_eq!(local_node.tag, VALDEF_TAG);
    let local_address = index
        .iter_nodes_with_tag(VALDEF_TAG)
        .find(|node| {
            index
                .parent_of(node.offset as u32)
                .is_some_and(|parent| parent.offset as u32 == block_address)
        })
        .expect("local VALDEF has its own indexed address")
        .offset as u32;
    let definition = index
        .get(local_address)
        .expect("local address is indexed")
        .decode_definition_body()
        .expect("decode local VALDEF");
    let dotty_tasty::ast::DefinitionBody::ValDef {
        name,
        type_tree,
        rhs: Some(initializer),
        ..
    } = definition
    else {
        panic!("Rust local is initialized and immutable");
    };
    assert_eq!(tasty.names().render(name).unwrap(), "counter");
    assert_eq!(type_tree.nodes()[0].tag, IDENTTPT_TAG);
    let RawTree::LengthNode(initializer_node) = initializer else {
        panic!("local initializer is a call node");
    };
    assert_eq!(initializer_node.tag, APPLY_TAG);
    let initializer = initializer_node.decode_apply().unwrap();
    let RawTree::LengthNode(helper_node) = initializer.function else {
        panic!("local initializer selects the original helper");
    };
    assert_eq!(helper_node.tag, SELECTIN_TAG);
    let helper = helper_node.decode_select_in().unwrap();
    assert!(matches!(
        tasty.names().render_signed_name(helper.name).unwrap().unwrap(),
        RenderedSignedName::Signed { original, .. } if original == "makeCounter"
    ));
    let RawTree::LengthNode(owner_node) = block.expression else {
        panic!("BLOCK result is the Owner constructor call");
    };
    assert_eq!(owner_node.tag, APPLY_TAG);
    let owner_apply = owner_node.decode_apply().unwrap();
    let [argument] = owner_apply.arguments.as_slice() else {
        panic!("Owner constructor receives exactly the local");
    };
    assert!(matches!(argument, RawTree::Leaf(term) if term.tag == TERMREFDIRECT_TAG));
    let references = argument.ast_refs();
    let [reference] = references.as_slice() else {
        panic!("local use has one direct TASTy reference");
    };
    assert_eq!(reference.kind, AstRefKind::TermRefDirect);
    assert_eq!(reference.address, local_address);
    assert_eq!(
        index.parent_of(local_address).unwrap().offset as u32,
        block_address
    );

    let local_reference_offset = argument
        .nodes()
        .into_iter()
        .find(|node| node.tag == TERMREFDIRECT_TAG)
        .expect("the local constructor argument has its own TERMREFdirect node")
        .offset;
    let asts_section = tasty
        .section(StandardSection::Asts)
        .expect("Factory has ASTs section");
    let constructor_payload_offset =
        owner_node.payload.as_ptr() as usize - asts_section.payload.as_ptr() as usize;
    let local_reference_offset = constructor_payload_offset + local_reference_offset;
    let asts_offset = asts_section.offset;
    assert_eq!(
        bytes[asts_offset + local_reference_offset],
        TERMREFDIRECT_TAG
    );
    assert_eq!(bytes[asts_offset + local_address as usize], VALDEF_TAG);

    let mut missing_local = bytes.to_vec();
    missing_local[asts_offset + local_address as usize] = 0;
    let missing_local_file = TastyFile::parse_and_validate_scala_3_9(&missing_local)
        .expect("the parser reaches the lazily indexed nested declaration");
    let missing_local_index = missing_local_file.ast_address_index();
    let missing_local_index = missing_local_index.expect("index actual corrupted file bytes");
    let missing_local_block_node = missing_local_index
        .get(block_address)
        .expect("create's BLOCK remains indexed in the corrupted file");
    let missing_asts_section = missing_local_file
        .section(StandardSection::Asts)
        .expect("corrupted file retains its ASTs section");
    let block_payload_address = missing_local_block_node.payload.as_ptr() as usize
        - missing_asts_section.payload.as_ptr() as usize;
    let missing_local_offset = local_address as usize - block_payload_address;
    assert_eq!(missing_local_offset, 14);
    let missing_local_error = missing_local_block_node.decode_block();
    assert!(
        matches!(
            missing_local_error,
            Err(AstError::Term(TermError::InvalidTag {
                tag: 0,
                offset,
            })) if offset == missing_local_offset
        ),
        "the actual file/index/BLOCK decoder must reject the missing local tag at its first byte offset: {missing_local_error:?}"
    );

    let mut malformed_reference = bytes.to_vec();
    malformed_reference[asts_offset + local_reference_offset] = 0;
    let mut local_reference_reader =
        Reader::new(&malformed_reference[asts_offset + local_reference_offset..]);
    let malformed_argument =
        RawTree::decode_with_base_offset(&mut local_reference_reader, local_reference_offset);
    assert!(
        matches!(
            malformed_argument,
            Err(TermError::InvalidTag {
                tag: 0,
                offset,
            }) if offset == local_reference_offset
        ),
        "local TERMREFdirect is rejected by its term decoder: {malformed_argument:?}"
    );

    let missing_reference_target = (0..128)
        .map(|address| address as u32)
        .find(|address| index.get(*address).is_none())
        .expect("the small AST address range has an unoccupied node address");
    let mut dangling_bytes = bytes.to_vec();
    dangling_bytes[asts_offset + local_reference_offset + 1] =
        0x80 | missing_reference_target as u8;
    let mut dangling_reference_reader =
        Reader::new(&dangling_bytes[asts_offset + local_reference_offset..]);
    let dangling_term =
        RawTree::decode_with_base_offset(&mut dangling_reference_reader, local_reference_offset)
            .expect("dangling local TERMREFdirect still decodes as a reference leaf");
    let reference = dangling_term
        .ast_ref()
        .expect("corrupted local argument remains a direct TASTy reference");
    assert_eq!(reference.address, missing_reference_target);
    let dangling_file =
        TastyFile::parse(&dangling_bytes).expect("parse isolated dangling-reference copy");
    assert_eq!(
        dangling_file.resolve_ast_reference(reference).unwrap(),
        None,
        "the AST resolver rejects the local reference's unoccupied address"
    );
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
            "dotty-scala-reference-local-consumer-{}-{}",
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
