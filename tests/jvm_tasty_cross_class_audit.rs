use std::fs;
use std::path::PathBuf;
use std::process::Command;

use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::{ConstantPoolEntry, ConstantPoolIndex};
use dotty_classfile::reader::Reader as ClassReader;
use dotty_core::ast::{AstArena, TreeKind};
use dotty_core::{Definitions, Packages, SemanticStore, SourceId, SourceText, Typed, Untyped};
use dotty_jvm::{JvmClassInitializationPlan, JvmClassPlan, JvmCompilationUnit, JvmMethodPlan};
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_tasty::ast::{
    APPLY_TAG, DEFDEF_TAG, IDENTTPT_TAG, PARAM_TAG, SELECTIN_TAG, SHAREDTYPE_TAG,
    TERMREFDIRECT_TAG, TERMREFPKG_TAG, TYPEREF_TAG, TYPEREFDIRECT_TAG,
};
use dotty_tasty::tasty::{Attribute as TastyAttribute, DefinitionBody, RawName, TastyFile};
use dotty_tasty::term::RawTree;
use dotty_typer::SourceTyper;

use dotty::jvm_tasty_cross_class::{
    CrossClassMethodRefError, CrossClassMethodRefInput, validate_cross_class_method_ref,
};
use dotty::jvm_tasty_plan::with_paired_jvm_tasty_batch_plan;

const COUNTER_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_cross_class/scala39/example/Counter.class");
const COUNTER_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_cross_class/scala39/example/Counter.tasty");
const READER_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_cross_class/scala39/example/Reader.class");
const READER_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_cross_class/scala39/example/Reader.tasty");
const SOURCE: &str = include_str!("fixtures/jvm_tasty_cross_class/Reader.scala");

fn clone_typed_ast(ast: &AstArena<Typed>) -> AstArena<Typed> {
    let mut clone = AstArena::new();
    for (expected, tree) in ast.iter() {
        assert_eq!(clone.alloc(tree.clone()), expected);
    }
    clone
}

fn clone_untyped_ast(ast: &AstArena<Untyped>) -> AstArena<Untyped> {
    let mut clone = AstArena::new();
    for (expected, tree) in ast.iter() {
        assert_eq!(clone.alloc(tree.clone()), expected);
    }
    clone
}

const COUNTER_UUID: [u8; 16] = [
    0x00, 0xd4, 0x56, 0x67, 0xbf, 0x83, 0x69, 0xe8, 0x00, 0x62, 0x9f, 0x05, 0xd0, 0xf0, 0x8f, 0x1b,
];
const READER_UUID: [u8; 16] = [
    0x00, 0x70, 0xa0, 0x42, 0xf9, 0xd1, 0xe6, 0xef, 0x00, 0x95, 0x57, 0xa1, 0xd0, 0x2e, 0x6a, 0x1c,
];
const RUN_SCALA_ORACLE: &str = "DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE";

fn run_cross_class_call_oracle(reader_tasty: &[u8]) {
    let scala_root = PathBuf::from(
        std::env::var_os("SCALA39_ROOT").expect("pinned Scala root for explicit oracle"),
    );
    let revision = Command::new("git")
        .arg("-C")
        .arg(&scala_root)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("read pinned Scala source revision");
    assert!(revision.status.success());
    assert_eq!(
        String::from_utf8_lossy(&revision.stdout).trim(),
        "777528f19a58e794c9954a42f433373472ec57f8"
    );
    let compiler = scala_root.join("dist/target/universal/stage/bin/scalac");
    let version = Command::new(&compiler)
        .arg("-version")
        .output()
        .expect("query pinned Scala compiler version");
    assert!(version.status.success());
    assert!(String::from_utf8_lossy(&version.stdout).contains("3.9.0-bin-SNAPSHOT-git-777528f"));

    let temp_root = std::env::var_os("DOTTY_TASTY_PICKLER_ORACLE_TMP")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("TMPDIR").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir);
    let temporary = temp_root.join(format!("dotty-issue-1674-oracle-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temporary);
    let oracle = temporary.join("oracle");
    let candidate = temporary.join("candidate/example");
    let compiled = temporary.join("compiled");
    fs::create_dir_all(&oracle).unwrap();
    fs::create_dir_all(&candidate).unwrap();
    fs::create_dir_all(&compiled).unwrap();
    let source_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/jvm_tasty_cross_class/Reader.scala");
    let oracle_compile = Command::new(&compiler)
        .arg("-d")
        .arg(&oracle)
        .arg(&source_path)
        .output()
        .expect("compile matching Counter with pinned Scala");
    assert!(
        oracle_compile.status.success(),
        "Scala fixture compile failed: {}",
        String::from_utf8_lossy(&oracle_compile.stderr)
    );
    for artifact in ["Counter.class", "Counter.tasty"] {
        fs::copy(
            oracle.join("example").join(artifact),
            candidate.join(artifact),
        )
        .unwrap();
    }
    fs::write(candidate.join("Reader.tasty"), reader_tasty).unwrap();
    let from_tasty = Command::new(&compiler)
        .arg("-from-tasty")
        .arg("-classpath")
        .arg(temporary.join("candidate"))
        .arg("-d")
        .arg(&compiled)
        .arg(candidate.join("Reader.tasty"))
        .output()
        .expect("read Rust-produced Reader.tasty with pinned Scala");
    assert!(
        from_tasty.status.success(),
        "Scala -from-tasty rejected cross-class call: {}",
        String::from_utf8_lossy(&from_tasty.stderr)
    );
    let javap = Command::new("javap")
        .arg("-classpath")
        .arg(format!(
            "{}:{}",
            compiled.display(),
            temporary.join("candidate").display()
        ))
        .args(["-p", "-s", "example.Reader"])
        .output()
        .expect("inspect Scala-rebuilt Reader class");
    assert!(javap.status.success());
    let output = String::from_utf8_lossy(&javap.stdout);
    assert!(output.contains("int read(example.Counter)"), "{output}");
    assert!(
        output.contains("descriptor: (Lexample/Counter;)I"),
        "{output}"
    );
    let _ = fs::remove_dir_all(temporary);
}

fn decode_class(bytes: &[u8]) -> ClassFile<'_> {
    let mut reader = ClassReader::new(bytes);
    let class = ClassFile::decode(&mut reader).unwrap();
    assert!(reader.is_at_end());
    class
}

fn class_tasty_uuid(class: &ClassFile<'_>) -> [u8; 16] {
    *class
        .tasty_link_id()
        .unwrap()
        .expect("Scala class has one class-level TASTY link")
        .as_bytes()
}

#[test]
fn scala_39_oracle_records_reader_jvm_link_and_per_class_tasty_links() {
    let counter = decode_class(COUNTER_CLASS);
    let reader = decode_class(READER_CLASS);
    assert_eq!(
        counter.constant_pool.class_name(counter.this_class),
        Ok("example/Counter")
    );
    assert_eq!(
        reader.constant_pool.class_name(reader.this_class),
        Ok("example/Reader")
    );
    assert_eq!((counter.version.major, reader.version.major), (61, 61));
    assert_eq!(class_tasty_uuid(&counter), COUNTER_UUID);
    assert_eq!(class_tasty_uuid(&reader), READER_UUID);

    let read = reader
        .methods
        .iter()
        .find(|method| reader.constant_pool.utf8(method.name_index).unwrap() == "read")
        .expect("Reader declares read");
    assert_eq!(
        reader.constant_pool.utf8(read.descriptor_index),
        Ok("(Lexample/Counter;)I")
    );
    let get_reference = reader
        .constant_pool
        .get(ConstantPoolIndex(21))
        .expect("javap oracle identifies the get Methodref at constant-pool #21");
    let ConstantPoolEntry::Methodref {
        class_index,
        name_and_type_index,
    } = get_reference
    else {
        panic!("Reader.read invokes get through CONSTANT_Methodref");
    };
    assert_eq!(
        reader.constant_pool.class_name(*class_index),
        Ok("example/Counter")
    );
    assert_eq!(
        reader.constant_pool.name_and_type(*name_and_type_index),
        Ok(("get", "()I"))
    );

    let counter_tasty = TastyFile::parse_and_validate_scala_3_9(COUNTER_TASTY).unwrap();
    let reader_tasty = TastyFile::parse_and_validate_scala_3_9(READER_TASTY).unwrap();
    assert_eq!(counter_tasty.header().uuid, COUNTER_UUID);
    assert_eq!(reader_tasty.header().uuid, READER_UUID);
    assert_eq!(
        (
            reader_tasty.header().major_version,
            reader_tasty.header().minor_version
        ),
        (28, 9)
    );
    let reader_attributes = reader_tasty.attributes().unwrap().unwrap();
    let [TastyAttribute::SourceFile(source_path)] = reader_attributes.as_slice() else {
        panic!("Reader sidecar records its single source-file path");
    };
    assert_eq!(
        reader_tasty.names().render(*source_path).unwrap(),
        "tests/fixtures/jvm_tasty_cross_class/Reader.scala"
    );

    let counter_package = counter_tasty.asts().unwrap();
    let counter_root = counter_package.get(0).unwrap().decode_package().unwrap();
    assert_eq!(
        counter_root.stats.len(),
        1,
        "Counter sidecar owns only Counter"
    );
    let DefinitionBody::TypeDef { name, .. } = counter_root
        .stats
        .get(0)
        .unwrap()
        .decode_definition_body()
        .unwrap()
    else {
        panic!("Counter sidecar contains a class type definition");
    };
    assert_eq!(counter_tasty.names().render(name).unwrap(), "Counter");

    let asts = reader_tasty.asts().unwrap();
    let package = asts.get(0).unwrap().decode_package().unwrap();
    assert_eq!(
        reader_tasty
            .names()
            .render(package.path_name().unwrap())
            .unwrap(),
        "example"
    );
    assert_eq!(package.stats.len(), 1, "Reader sidecar owns only Reader");
    let reader_definition = package.stats.get(0).unwrap();
    let DefinitionBody::TypeDef { name, .. } = reader_definition.decode_definition_body().unwrap()
    else {
        panic!("Reader sidecar contains a class type definition");
    };
    assert_eq!(reader_tasty.names().render(name).unwrap(), "Reader");
    assert!(matches!(
        reader_tasty.names().get(name),
        Some(RawName::Utf8(_))
    ));

    let address_index = reader_tasty.ast_address_index().unwrap();
    let mut reader_method = None;
    let mut reader_formal = None;
    for node in address_index.iter() {
        if node.tag == DEFDEF_TAG {
            let method = node.decode_defdef_body().unwrap();
            if reader_tasty.names().render(method.name).unwrap() == "read" {
                reader_method = Some((node.offset as u32, method));
            }
        }
        if node.tag == PARAM_TAG {
            reader_formal = Some((node.offset as u32, node.decode_parameter().unwrap()));
        }
    }
    let (method_address, method) = reader_method.expect("Reader.read node exists");
    let (formal_address, formal) = reader_formal.expect("Reader.read counter formal exists");
    let parameter_name = formal.name();
    assert_eq!(
        reader_tasty.names().render(parameter_name).unwrap(),
        "counter"
    );
    let type_body = formal.decode_body().unwrap();
    let type_nodes = type_body.type_tree.nodes();
    assert_eq!(type_nodes.first().unwrap().tag, IDENTTPT_TAG);
    let type_ref = type_nodes
        .iter()
        .find(|node| node.tag == TYPEREF_TAG)
        .expect("formal type is ordinary TYPEREF");
    assert_eq!(type_ref.tag, TYPEREF_TAG);
    let formal_type_node = address_index
        .iter_nodes_with_tag(TYPEREF_TAG)
        .find(|candidate| {
            let mut parent = Some(*candidate);
            while let Some(node) = parent {
                if node.offset as u32 == formal_address {
                    return true;
                }
                parent = address_index.parent_of(node.offset as u32);
            }
            false
        })
        .expect("formal type's TYPEREF is indexed beneath the formal node");
    assert_eq!(
        reader_tasty
            .names()
            .render(type_body.type_tree.name_refs()[0])
            .unwrap(),
        "Counter"
    );
    assert!(type_nodes.iter().any(|node| node.tag == TERMREFPKG_TAG));
    assert!(
        type_body
            .type_tree
            .name_refs()
            .iter()
            .any(|name| reader_tasty.names().render(*name).unwrap() == "example")
    );
    assert!(
        !type_nodes.iter().any(|node| node.tag == TYPEREFDIRECT_TAG),
        "cross-sidecar type reference is not TYPEREFdirect"
    );

    let rhs = method.rhs.expect("Reader.read has a body");
    let RawTree::LengthNode(apply) = rhs else {
        panic!("Reader.read body is an APPLY category-five node");
    };
    assert_eq!(apply.tag, APPLY_TAG);
    let apply_node = apply.decode_apply().unwrap();
    let RawTree::LengthNode(select) = apply_node.function else {
        panic!("Reader.read selects get through SELECTin");
    };
    assert_eq!(select.tag, SELECTIN_TAG);
    let selected_node = select.decode_select_in().unwrap();
    let signed = reader_tasty
        .names()
        .render_signed_name(selected_node.name)
        .unwrap()
        .unwrap();
    assert!(matches!(
        signed,
        dotty_tasty::tasty::RenderedSignedName::Signed { original, signature }
            if original == "get" && signature.parameters.is_empty() && signature.result == "scala.Int"
    ));
    let qualifier = selected_node.qualifier.nodes();
    assert_eq!(qualifier[0].tag, TERMREFDIRECT_TAG);
    assert_eq!(
        selected_node.qualifier.ast_refs()[0].address,
        formal_address
    );
    assert_eq!(selected_node.underlying_type.nodes()[0].tag, SHAREDTYPE_TAG);
    let selected_type_references = selected_node.underlying_type.ast_refs();
    let [selected_type_reference] = selected_type_references.as_slice() else {
        panic!("selection has one shared owner-type reference");
    };
    assert_eq!(
        selected_type_reference.address, formal_type_node.offset as u32,
        "SELECTin owner type shares Reader.read's exact Counter formal type node"
    );
    assert_eq!(
        address_index
            .get_node(selected_type_reference.address)
            .unwrap()
            .tag,
        TYPEREF_TAG,
        "SELECTin owner type references the Counter formal's TYPEREF node"
    );
    assert!(address_index.get_node(method_address).is_some());
}

#[test]
fn cross_class_call_is_pickled_as_a_bounded_select_in_application() {
    let source_id = SourceId::from_index(1674);
    let source = SourceText::new(SOURCE).unwrap();
    let scanner = ContextualScanner::new(SOURCE).unwrap();
    assert!(scanner.diagnostics().is_empty());
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = parse_compilation_unit(source, source_id, scanner, &mut store.names);
    assert!(parsed.diagnostics.is_empty());
    let mut packages = Packages::new();
    let index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        "Reader.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        other => panic!("cross-class audit source names cleanly: {other:?}"),
    };
    let dotty_core::ast::TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("fixture parses as one package");
    };
    let [counter_tree, reader_tree] = package.stats.as_slice() else {
        panic!("fixture has Counter and Reader in this one session");
    };
    let counter = index.symbol_at(source_id, *counter_tree).unwrap();
    let reader = index.symbol_at(source_id, *reader_tree).unwrap();
    assert_ne!(counter, reader);

    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer.complete_symbol(counter).unwrap();
    typer.complete_symbol(reader).unwrap();
    let dotty_core::ast::TreeKind::TypeDef(counter_def) = &parsed.ast.get(*counter_tree).kind
    else {
        panic!("Counter is a class definition");
    };
    let dotty_core::ast::TreeKind::Template(counter_template) =
        &parsed.ast.get(counter_def.rhs).kind
    else {
        panic!("Counter has a class template");
    };
    let get_tree = counter_template
        .body
        .iter()
        .copied()
        .find(|tree| matches!(&parsed.ast.get(*tree).kind, dotty_core::ast::TreeKind::DefDef(def) if typer.store().names.resolve(def.name.as_name().text()) == "get"))
        .expect("Counter declares get");
    let get_symbol = index.symbol_at(source_id, get_tree).unwrap();
    typer.complete_symbol(get_symbol).unwrap();

    let dotty_core::ast::TreeKind::TypeDef(reader_def) = &parsed.ast.get(*reader_tree).kind else {
        panic!("Reader is a class definition");
    };
    let dotty_core::ast::TreeKind::Template(reader_template) = &parsed.ast.get(reader_def.rhs).kind
    else {
        panic!("Reader has a class template");
    };
    let read_tree = reader_template
        .body
        .iter()
        .copied()
        .find(|tree| matches!(&parsed.ast.get(*tree).kind, dotty_core::ast::TreeKind::DefDef(def) if typer.store().names.resolve(def.name.as_name().text()) == "read"))
        .expect("Reader declares read");
    let read_symbol = index.symbol_at(source_id, read_tree).unwrap();
    typer.complete_symbol(read_symbol).unwrap();
    let dotty_core::ast::TreeKind::DefDef(read_definition) = &parsed.ast.get(read_tree).kind else {
        panic!("read is a method");
    };
    let [counter_parameter_tree] = read_definition.value_param_clauses[0].as_slice() else {
        panic!("read has one Counter formal");
    };
    let counter_parameter = index.symbol_at(source_id, *counter_parameter_tree).unwrap();
    typer.complete_symbol(counter_parameter).unwrap();
    assert_eq!(
        typer.store().symbols.get(counter_parameter).owner,
        Some(read_symbol)
    );
    let dotty_core::SymbolInfo::Complete(parameter_type) =
        typer.store().symbols.get(counter_parameter).info
    else {
        panic!("read formal has a completed type");
    };
    assert!(matches!(
        typer.store().types.get(parameter_type),
        dotty_core::Type::TypeRef {
            target: dotty_core::TypeRefTarget::Symbol(symbol),
            ..
        } if *symbol == counter
    ));
    let rhs = read_definition.rhs.unwrap();
    let context = typer.expression_context_for(read_symbol).unwrap();
    let typed = typer.type_expression(rhs, context);
    let typed_rhs = typed.expect("typer resolves Counter.get to one canonical method");
    assert_eq!(
        typer.source_typed_index().get(source_id, rhs),
        Some(typed_rhs)
    );
    let TreeKind::Apply(typed_apply) = &typer.typed_ast().get(typed_rhs).kind else {
        panic!("typed Reader.read body remains an application");
    };
    let typed_select_id = typed_apply.function;
    let TreeKind::Select(typed_select) = &typer.typed_ast().get(typed_apply.function).kind else {
        panic!("typed application selects Counter.get");
    };
    let typed_receiver_id = typed_select.qualifier;
    assert!(matches!(
        typer.store().types.get(typer.typed_ast().get(typed_apply.function).ty),
        dotty_core::Type::TermRef {
            target: dotty_core::TermRefTarget::Symbol(symbol),
            ..
        } if *symbol == get_symbol
    ));
    let TreeKind::Ident(typed_qualifier) = &typer.typed_ast().get(typed_select.qualifier).kind
    else {
        panic!("the selected receiver is the counter formal");
    };
    assert_eq!(
        typer.store().names.resolve(typed_qualifier.name.text()),
        "counter"
    );
    assert!(matches!(
        typer.store().types.get(typer.typed_ast().get(typed_select.qualifier).ty),
        dotty_core::Type::TermRef {
            target: dotty_core::TermRefTarget::Symbol(symbol),
            ..
        } if *symbol == counter_parameter
    ));

    let TreeKind::DefDef(get_definition) = &parsed.ast.get(get_tree).kind else {
        panic!("Counter.get is a source method");
    };
    let get_rhs = get_definition.rhs.expect("Counter.get has a body");
    let get_context = typer.expression_context_for(get_symbol).unwrap();
    let typed_get_rhs = typer
        .type_expression(get_rhs, get_context)
        .expect("Counter.get body types in the same source session");
    assert_eq!(
        typer.source_typed_index().get(source_id, get_rhs),
        Some(typed_get_rhs)
    );

    let jvm_unit = JvmCompilationUnit {
        store: typer.store(),
        typed_ast: typer.typed_ast(),
        classes: vec![
            JvmClassPlan {
                class: counter,
                methods: vec![JvmMethodPlan {
                    method: get_symbol,
                    parameters: vec![],
                    locals: vec![],
                    body: Some(typed_get_rhs),
                }],
                fields: vec![],
                initialization: JvmClassInitializationPlan::Unsupported,
            },
            JvmClassPlan {
                class: reader,
                methods: vec![JvmMethodPlan {
                    method: read_symbol,
                    parameters: vec![counter_parameter],
                    locals: vec![],
                    body: Some(typed_rhs),
                }],
                fields: vec![],
                initialization: JvmClassInitializationPlan::Unsupported,
            },
        ],
    };
    let proof = validate_cross_class_method_ref(CrossClassMethodRefInput {
        source_id,
        source_root: parsed.root,
        store: typer.store(),
        definitions: &definitions,
        source_ast: &parsed.ast,
        typed_ast: typer.typed_ast(),
        source_index: &index,
        source_typed_index: typer.source_typed_index(),
        jvm_unit: &jvm_unit,
    })
    .expect("one-session source and planned-class identities validate");
    assert_eq!(proof.source_id(), source_id);
    assert_eq!(proof.counter_class(), counter);
    assert_eq!(proof.reader_class(), reader);
    assert_eq!(proof.counter_get(), get_symbol);
    assert_eq!(proof.reader_read(), read_symbol);
    assert_eq!(proof.counter_formal(), counter_parameter);
    assert_eq!(proof.source_rhs(), rhs);
    assert_eq!(proof.typed_rhs(), typed_rhs);
    assert_eq!(proof.typed_select(), typed_select_id);
    assert_eq!(proof.typed_receiver(), typed_receiver_id);
    assert_eq!(proof.method_descriptor(), "()I");

    let mut wrong_prefix_ast = clone_typed_ast(typer.typed_ast());
    wrong_prefix_ast.get_mut(typed_receiver_id).ty = definitions.int;
    let wrong_prefix_unit = JvmCompilationUnit {
        store: typer.store(),
        typed_ast: &wrong_prefix_ast,
        classes: jvm_unit.classes.clone(),
    };
    assert_eq!(
        validate_cross_class_method_ref(CrossClassMethodRefInput {
            source_id,
            source_root: parsed.root,
            store: typer.store(),
            definitions: &definitions,
            source_ast: &parsed.ast,
            typed_ast: &wrong_prefix_ast,
            source_index: &index,
            source_typed_index: typer.source_typed_index(),
            jvm_unit: &wrong_prefix_unit,
        })
        .err()
        .unwrap(),
        CrossClassMethodRefError::TypedBinding(
            "typed receiver and selected method must retain canonical symbol targets"
        )
    );

    let mut stale_typed_ast = clone_typed_ast(typer.typed_ast());
    let checkpoint = stale_typed_ast.checkpoint();
    let stale_rhs = stale_typed_ast.alloc(stale_typed_ast.get(typed_rhs).clone());
    stale_typed_ast.rollback_to(checkpoint);
    let mut stale_source_typed_index = typer.source_typed_index().clone();
    stale_source_typed_index.remove(source_id, rhs);
    stale_source_typed_index
        .insert(source_id, rhs, stale_rhs)
        .unwrap();
    let stale_jvm_unit = JvmCompilationUnit {
        store: typer.store(),
        typed_ast: &stale_typed_ast,
        classes: jvm_unit.classes.clone(),
    };
    assert_eq!(
        validate_cross_class_method_ref(CrossClassMethodRefInput {
            source_id,
            source_root: parsed.root,
            store: typer.store(),
            definitions: &definitions,
            source_ast: &parsed.ast,
            typed_ast: &stale_typed_ast,
            source_index: &index,
            source_typed_index: &stale_source_typed_index,
            jvm_unit: &stale_jvm_unit,
        })
        .err()
        .unwrap(),
        CrossClassMethodRefError::TypedBinding("Reader.read typed body root is stale")
    );

    let mut stale_counter_body_ast = clone_typed_ast(typer.typed_ast());
    let checkpoint = stale_counter_body_ast.checkpoint();
    let stale_counter_body =
        stale_counter_body_ast.alloc(stale_counter_body_ast.get(typed_get_rhs).clone());
    stale_counter_body_ast.rollback_to(checkpoint);
    let mut stale_counter_body_index = typer.source_typed_index().clone();
    stale_counter_body_index.remove(source_id, get_rhs);
    stale_counter_body_index
        .insert(source_id, get_rhs, stale_counter_body)
        .unwrap();
    let mut stale_counter_body_classes = jvm_unit.classes.clone();
    stale_counter_body_classes[0].methods[0].body = Some(stale_counter_body);
    let stale_counter_body_unit = JvmCompilationUnit {
        store: typer.store(),
        typed_ast: &stale_counter_body_ast,
        classes: stale_counter_body_classes,
    };
    assert_eq!(
        validate_cross_class_method_ref(CrossClassMethodRefInput {
            source_id,
            source_root: parsed.root,
            store: typer.store(),
            definitions: &definitions,
            source_ast: &parsed.ast,
            typed_ast: &stale_counter_body_ast,
            source_index: &index,
            source_typed_index: &stale_counter_body_index,
            jvm_unit: &stale_counter_body_unit,
        })
        .err()
        .unwrap(),
        CrossClassMethodRefError::TypedBinding("Counter.get typed body root is stale")
    );

    let mut stale_counter_source_ast = clone_untyped_ast(&parsed.ast);
    let checkpoint = stale_counter_source_ast.checkpoint();
    let stale_counter_source_rhs =
        stale_counter_source_ast.alloc(stale_counter_source_ast.get(get_rhs).clone());
    stale_counter_source_ast.rollback_to(checkpoint);
    let TreeKind::DefDef(stale_counter_source_definition) =
        &mut stale_counter_source_ast.get_mut(get_tree).kind
    else {
        unreachable!("Counter.get is a source method")
    };
    stale_counter_source_definition.rhs = Some(stale_counter_source_rhs);
    let mut stale_counter_source_index = typer.source_typed_index().clone();
    stale_counter_source_index.remove(source_id, get_rhs);
    stale_counter_source_index
        .insert(source_id, stale_counter_source_rhs, typed_get_rhs)
        .unwrap();
    assert_eq!(
        validate_cross_class_method_ref(CrossClassMethodRefInput {
            source_id,
            source_root: parsed.root,
            store: typer.store(),
            definitions: &definitions,
            source_ast: &stale_counter_source_ast,
            typed_ast: typer.typed_ast(),
            source_index: &index,
            source_typed_index: &stale_counter_source_index,
            jvm_unit: &jvm_unit,
        })
        .err()
        .unwrap(),
        CrossClassMethodRefError::SourceShape("Counter.get source body root is stale")
    );

    let mut stale_child_ast = clone_typed_ast(typer.typed_ast());
    let checkpoint = stale_child_ast.checkpoint();
    let stale_child = stale_child_ast.alloc(stale_child_ast.get(typed_rhs).clone());
    stale_child_ast.rollback_to(checkpoint);
    let TreeKind::Apply(stale_child_apply) = &mut stale_child_ast.get_mut(typed_rhs).kind else {
        unreachable!("the typed source root is an Apply")
    };
    stale_child_apply.function = stale_child;
    let TreeKind::Apply(source_apply) = &parsed.ast.get(rhs).kind else {
        unreachable!("the source body root is an Apply")
    };
    let mut stale_child_index = typer.source_typed_index().clone();
    stale_child_index.remove(source_id, source_apply.function);
    stale_child_index
        .insert(source_id, source_apply.function, stale_child)
        .unwrap();
    let stale_child_unit = JvmCompilationUnit {
        store: typer.store(),
        typed_ast: &stale_child_ast,
        classes: jvm_unit.classes.clone(),
    };
    assert_eq!(
        validate_cross_class_method_ref(CrossClassMethodRefInput {
            source_id,
            source_root: parsed.root,
            store: typer.store(),
            definitions: &definitions,
            source_ast: &parsed.ast,
            typed_ast: &stale_child_ast,
            source_index: &index,
            source_typed_index: &stale_child_index,
            jvm_unit: &stale_child_unit,
        })
        .err()
        .unwrap(),
        CrossClassMethodRefError::TypedBinding("typed selected function root is stale")
    );

    let mut stale_source_ast = clone_untyped_ast(&parsed.ast);
    let checkpoint = stale_source_ast.checkpoint();
    let stale_source_child = stale_source_ast.alloc(stale_source_ast.get(rhs).clone());
    stale_source_ast.rollback_to(checkpoint);
    let TreeKind::Apply(stale_source_apply) = &mut stale_source_ast.get_mut(rhs).kind else {
        unreachable!("the source body root is an Apply")
    };
    stale_source_apply.function = stale_source_child;
    assert_eq!(
        validate_cross_class_method_ref(CrossClassMethodRefInput {
            source_id,
            source_root: parsed.root,
            store: typer.store(),
            definitions: &definitions,
            source_ast: &stale_source_ast,
            typed_ast: typer.typed_ast(),
            source_index: &index,
            source_typed_index: typer.source_typed_index(),
            jvm_unit: &jvm_unit,
        })
        .err()
        .unwrap(),
        CrossClassMethodRefError::SourceShape(
            "Reader.read selected function is not present in the source AST"
        )
    );

    let missing_counter = JvmCompilationUnit {
        store: typer.store(),
        typed_ast: typer.typed_ast(),
        classes: vec![jvm_unit.classes[1].clone()],
    };
    assert_eq!(
        validate_cross_class_method_ref(CrossClassMethodRefInput {
            source_id,
            source_root: parsed.root,
            store: typer.store(),
            definitions: &definitions,
            source_ast: &parsed.ast,
            typed_ast: typer.typed_ast(),
            source_index: &index,
            source_typed_index: typer.source_typed_index(),
            jvm_unit: &missing_counter,
        })
        .err()
        .unwrap(),
        CrossClassMethodRefError::JvmPlan("canonical Counter is absent from the complete JVM unit")
    );

    let wrong_target = JvmCompilationUnit {
        store: typer.store(),
        typed_ast: typer.typed_ast(),
        classes: vec![
            JvmClassPlan {
                class: counter,
                methods: jvm_unit.classes[1].methods.clone(),
                fields: vec![],
                initialization: JvmClassInitializationPlan::Unsupported,
            },
            jvm_unit.classes[1].clone(),
        ],
    };
    assert_eq!(
        validate_cross_class_method_ref(CrossClassMethodRefInput {
            source_id,
            source_root: parsed.root,
            store: typer.store(),
            definitions: &definitions,
            source_ast: &parsed.ast,
            typed_ast: typer.typed_ast(),
            source_index: &index,
            source_typed_index: typer.source_typed_index(),
            jvm_unit: &wrong_target,
        })
        .err()
        .unwrap(),
        CrossClassMethodRefError::JvmPlan(
            "canonical Counter.get is absent from Counter's JVM plan"
        )
    );

    let mut missing_typed_rhs = typer.source_typed_index().clone();
    missing_typed_rhs.remove(source_id, rhs);
    assert_eq!(
        validate_cross_class_method_ref(CrossClassMethodRefInput {
            source_id,
            source_root: parsed.root,
            store: typer.store(),
            definitions: &definitions,
            source_ast: &parsed.ast,
            typed_ast: typer.typed_ast(),
            source_index: &index,
            source_typed_index: &missing_typed_rhs,
            jvm_unit: &jvm_unit,
        })
        .err()
        .unwrap(),
        CrossClassMethodRefError::TypedBinding("Reader.read body has no source-to-typed binding")
    );

    let wrong_formal_plan = JvmCompilationUnit {
        store: typer.store(),
        typed_ast: typer.typed_ast(),
        classes: vec![
            jvm_unit.classes[0].clone(),
            JvmClassPlan {
                class: reader,
                methods: vec![JvmMethodPlan {
                    method: read_symbol,
                    parameters: vec![],
                    locals: vec![],
                    body: Some(typed_rhs),
                }],
                fields: vec![],
                initialization: JvmClassInitializationPlan::Unsupported,
            },
        ],
    };
    assert_eq!(
        validate_cross_class_method_ref(CrossClassMethodRefInput {
            source_id,
            source_root: parsed.root,
            store: typer.store(),
            definitions: &definitions,
            source_ast: &parsed.ast,
            typed_ast: typer.typed_ast(),
            source_index: &index,
            source_typed_index: typer.source_typed_index(),
            jvm_unit: &wrong_formal_plan,
        })
        .err()
        .unwrap(),
        CrossClassMethodRefError::JvmPlan(
            "planned method identity, formal order, or typed body does not match source"
        )
    );

    assert_eq!(
        validate_cross_class_method_ref(CrossClassMethodRefInput {
            source_id: SourceId::from_index(source_id.index() + 1),
            source_root: parsed.root,
            store: typer.store(),
            definitions: &definitions,
            source_ast: &parsed.ast,
            typed_ast: typer.typed_ast(),
            source_index: &index,
            source_typed_index: typer.source_typed_index(),
            jvm_unit: &jvm_unit,
        })
        .err()
        .unwrap(),
        CrossClassMethodRefError::SourceIdentity("Counter has no canonical source symbol")
    );

    let foreign_store = SemanticStore::new();
    let foreign_typed_ast = dotty_core::ast::TypedAst::new();
    let foreign_unit = JvmCompilationUnit {
        store: &foreign_store,
        typed_ast: &foreign_typed_ast,
        classes: vec![],
    };
    assert_eq!(
        validate_cross_class_method_ref(CrossClassMethodRefInput {
            source_id,
            source_root: parsed.root,
            store: typer.store(),
            definitions: &definitions,
            source_ast: &parsed.ast,
            typed_ast: typer.typed_ast(),
            source_index: &index,
            source_typed_index: typer.source_typed_index(),
            jvm_unit: &foreign_unit,
        })
        .err()
        .unwrap(),
        CrossClassMethodRefError::SemanticIdentity("JVM unit uses a different semantic store")
    );
    drop(typer);

    with_paired_jvm_tasty_batch_plan(SOURCE, "Reader.scala", source_id, |batch| {
        let reader = &batch.classes[1];
        let output = reader
            .tasty_plan
            .pickle()
            .expect("the canonical same-batch Counter.get call is supported");
        let file = TastyFile::parse_and_validate_scala_3_9(output.bytes()).unwrap();
        let package = file
            .asts()
            .unwrap()
            .get(0)
            .unwrap()
            .decode_package()
            .unwrap();
        assert_eq!(
            package.stats.len(),
            1,
            "Reader sidecar has no Counter clone"
        );
        let address_index = file.ast_address_index().unwrap();
        let tags = address_index
            .iter()
            .map(|node| node.tag)
            .collect::<Vec<_>>();
        assert!(tags.iter().filter(|&&tag| tag == APPLY_TAG).count() >= 1);
        assert!(tags.contains(&SELECTIN_TAG));
        assert_eq!(tags.iter().filter(|&&tag| tag == PARAM_TAG).count(), 1);

        let (read_address, read_method) = address_index
            .iter()
            .filter(|node| node.tag == DEFDEF_TAG)
            .map(|node| (node.offset as u32, node.decode_defdef_body().unwrap()))
            .find(|(_, method)| file.names().render(method.name).unwrap() == "read")
            .expect("generated Reader sidecar declares read");
        let RawTree::LengthNode(apply) = read_method.rhs.expect("read has an RHS") else {
            panic!("generated read body is an APPLY");
        };
        assert_eq!(apply.tag, APPLY_TAG);
        let apply = apply.decode_apply().unwrap();
        let RawTree::LengthNode(select) = apply.function else {
            panic!("generated call selects the external get method");
        };
        assert_eq!(select.tag, SELECTIN_TAG);
        let select = select.decode_select_in().unwrap();
        let signed = file
            .names()
            .render_signed_name(select.name)
            .unwrap()
            .unwrap();
        assert!(matches!(
            signed,
            dotty_tasty::tasty::RenderedSignedName::Signed { original, signature }
                if original == "get" && signature.parameters.is_empty() && signature.result == "scala.Int"
        ));
        let receiver_references = select.qualifier.ast_refs();
        let [receiver] = receiver_references.as_slice() else {
            panic!("selected receiver references exactly one formal declaration");
        };
        assert_eq!(
            address_index.get_node(receiver.address).unwrap().tag,
            PARAM_TAG,
            "generated selection receiver points to Reader.read's PARAM"
        );
        assert_eq!(select.underlying_type.nodes()[0].tag, SHAREDTYPE_TAG);
        let owner_type_references = select.underlying_type.ast_refs();
        let [owner_type] = owner_type_references.as_slice() else {
            panic!("selection owner type shares the formal's class type");
        };
        assert_eq!(
            address_index.get_node(owner_type.address).unwrap().tag,
            TYPEREF_TAG,
            "generated selection owner type references the formal's TYPEREF"
        );
        assert!(address_index.get_node(read_address).is_some());
        if std::env::var_os(RUN_SCALA_ORACLE).is_some() {
            run_cross_class_call_oracle(output.bytes());
        }
    })
    .expect("the full same-source batch and TASTy cross-contract validates");
}
