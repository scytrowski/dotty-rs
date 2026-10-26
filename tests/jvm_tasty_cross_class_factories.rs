use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_batch;
use dotty::jvm_tasty_plan::with_paired_jvm_tasty_batch_plan;
use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::ConstantPoolEntry;
use dotty_classfile::reader::Reader as ClassReader;
use dotty_core::SourceId;
use dotty_tasty::ast::{
    APPLY_TAG, DEFDEF_TAG, IDENTTPT_TAG, NEW_TAG, PARAM_TAG, SELECTIN_TAG, SHAREDTYPE_TAG,
    TERMREFDIRECT_TAG, TERMREFPKG_TAG, TYPEREF_TAG, VALDEF_TAG,
};
use dotty_tasty::file::TastyFile;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const RUN_SCALA_ORACLE: &str = "DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE";

const FACTORY_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_cross_class_factories/CounterFactory.scala");
const REFERENCE_FIELD_SOURCE: &str = "package example\nclass Counter(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\nclass CounterOwner(seed: Int):\n  private val inner: Counter = new Counter(seed)\n  def constant(): Int = 0\n";
const REFERENCE_FIELD_READ_SOURCE: &str = "package example\nclass Counter(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\nclass CounterOwner(seed: Int):\n  private val inner: Counter = new Counter(seed)\n  def read(): Int = inner.get()\n";
const COUNTER_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_cross_class_factories/scala39/example/Counter.class");
const COUNTER_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_cross_class_factories/scala39/example/Counter.tasty");
const FACTORY_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_cross_class_factories/scala39/example/CounterFactory.class");
const FACTORY_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_cross_class_factories/scala39/example/CounterFactory.tasty");
const IDENTITY_CLASS: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_cross_class_factories/scala39/example/CounterIdentity.class"
);
const IDENTITY_TASTY: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_cross_class_factories/scala39/example/CounterIdentity.tasty"
);

fn decode_class(bytes: &[u8]) -> ClassFile<'_> {
    let mut reader = ClassReader::new(bytes);
    let class = ClassFile::decode(&mut reader).unwrap();
    assert!(reader.is_at_end());
    class
}

fn tasty_uuid(class: &ClassFile<'_>) -> [u8; 16] {
    *class
        .tasty_link_id()
        .unwrap()
        .expect("Scala oracle class has one TASTY link")
        .as_bytes()
}

fn assert_pinned_tasty(bytes: &[u8]) -> TastyFile<'_> {
    let tasty = TastyFile::parse_and_validate_scala_3_9(bytes).unwrap();
    assert_eq!(
        (
            tasty.header().major_version,
            tasty.header().minor_version,
            tasty.header().experimental_version,
        ),
        (28, 9, 0)
    );
    tasty
}

fn method_code<'a>(class: &'a ClassFile<'_>, name: &str) -> &'a [u8] {
    let method = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == name)
        .unwrap_or_else(|| panic!("class declares {name}"));
    method
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::Code(code) => Some(code.code),
            _ => None,
        })
        .expect("concrete method has Code")
}

fn assert_counter_type_ref(
    tree: &dotty_tasty::term::RawTree<'_>,
    index: &dotty_tasty::ast::AstAddressIndex<'_>,
    tasty: &TastyFile<'_>,
) {
    let tags = tree.nodes().iter().map(|node| node.tag).collect::<Vec<_>>();
    let references = tree.ast_refs();
    if !references.is_empty() {
        let [reference] = references.as_slice() else {
            panic!("Counter type tree has one shared TypeRef target: {references:?}");
        };
        assert_eq!(reference.kind, dotty_tasty::term::AstRefKind::SharedType);
        assert!(tags.contains(&SHAREDTYPE_TAG));
        assert_eq!(
            index.get_node(reference.address).unwrap().tag,
            TYPEREF_TAG,
            "shared type reference targets the ordinary TYPEREF node"
        );
    } else {
        assert_eq!(
            tags,
            [IDENTTPT_TAG, TYPEREF_TAG, TERMREFPKG_TAG],
            "reference result uses IDENTtpt -> TYPEREF -> TERMREFpkg"
        );
        let names = tree
            .name_refs()
            .into_iter()
            .map(|name| tasty.names().render(name).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            ["Counter", "example"],
            "cross-sidecar TypeRef uses Counter qualified by example"
        );
    }
}

fn method<'a>(tasty: &'a TastyFile<'_>, name: &str) -> (u32, dotty_tasty::ast::DefDefBody<'a>) {
    let index = tasty.ast_address_index().unwrap();
    index
        .iter_nodes_with_tag(DEFDEF_TAG)
        .find_map(|node| {
            let body = index
                .get(node.offset as u32)
                .unwrap()
                .decode_defdef_body()
                .unwrap();
            (tasty.names().render(body.name).unwrap() == name).then_some((node.offset as u32, body))
        })
        .unwrap_or_else(|| panic!("TASTy contains method {name}"))
}

fn parameter_address(index: &dotty_tasty::ast::AstAddressIndex<'_>, method_address: u32) -> u32 {
    index
        .iter_nodes_with_tag(PARAM_TAG)
        .find_map(|candidate| {
            let mut parent = Some(candidate);
            while let Some(node) = parent {
                if node.offset as u32 == method_address {
                    return Some(candidate.offset as u32);
                }
                parent = index.parent_of(node.offset as u32);
            }
            None
        })
        .expect("method owns its formal parameter")
}

#[test]
fn scala_39_factory_oracle_pins_descriptors_constructor_wire_and_sidecar_links() {
    let counter = decode_class(COUNTER_CLASS);
    let factory = decode_class(FACTORY_CLASS);
    let identity = decode_class(IDENTITY_CLASS);
    assert_eq!((counter.version.major, factory.version.major), (65, 65));
    assert_eq!(
        counter.constant_pool.class_name(counter.this_class),
        Ok("example/Counter")
    );
    assert_eq!(
        factory.constant_pool.class_name(factory.this_class),
        Ok("example/CounterFactory")
    );
    assert_eq!(
        identity.constant_pool.class_name(identity.this_class),
        Ok("example/CounterIdentity")
    );

    let factory_tasty = assert_pinned_tasty(FACTORY_TASTY);
    let counter_tasty = assert_pinned_tasty(COUNTER_TASTY);
    let identity_tasty = assert_pinned_tasty(IDENTITY_TASTY);
    assert_eq!(tasty_uuid(&factory), factory_tasty.header().uuid);
    assert_eq!(tasty_uuid(&counter), counter_tasty.header().uuid);
    assert_eq!(tasty_uuid(&identity), identity_tasty.header().uuid);
    assert_eq!(
        factory_tasty.header().uuid,
        [
            0x00, 0xc4, 0x19, 0xa2, 0x59, 0x77, 0xff, 0xef, 0x00, 0x5f, 0xc3, 0x6c, 0x18, 0x72,
            0x30, 0x1c
        ]
    );
    assert_eq!(
        counter_tasty.header().uuid,
        [
            0x00, 0xa5, 0x63, 0x5c, 0x9f, 0xca, 0x05, 0xe8, 0x00, 0xde, 0xb7, 0x38, 0xf3, 0xed,
            0xb7, 0x1b
        ]
    );

    let create = factory
        .methods
        .iter()
        .find(|method| factory.constant_pool.utf8(method.name_index).unwrap() == "create")
        .expect("CounterFactory declares create");
    assert_eq!(
        factory.constant_pool.utf8(create.descriptor_index),
        Ok("(I)Lexample/Counter;")
    );
    assert!(create.access_flags.is_public());
    let address_index = factory_tasty.ast_address_index().unwrap();
    assert!(
        (0..factory_tasty.names().len()).any(|reference| {
            matches!(
                factory_tasty.names().get(reference as u32),
                Some(dotty_tasty::RawName::Qualified { prefix, selector })
                    if factory_tasty.names().get_utf8(*prefix) == Some("example")
                        && factory_tasty.names().get_utf8(*selector) == Some("Counter")
            )
        }),
        "TYPEREF name table contains the qualified example.Counter name"
    );
    let (create_address, create) = method(&factory_tasty, "create");
    assert_eq!(create_address, 30, "pinned factory method address");
    assert_eq!(create.parameters.len(), 1);
    let seed = &create.parameters[0];
    assert_eq!(seed.tag(), PARAM_TAG);
    assert_eq!(factory_tasty.names().render(seed.name()).unwrap(), "seed");
    let seed_address = parameter_address(&address_index, create_address);
    assert!(address_index.get(seed_address).is_some());
    assert_counter_type_ref(&create.return_type, &address_index, &factory_tasty);

    let Some(dotty_tasty::term::RawTree::LengthNode(apply_node)) = create.rhs.as_ref() else {
        panic!("create RHS is the constructor APPLY");
    };
    assert_eq!(apply_node.tag, APPLY_TAG);
    let apply = apply_node.decode_apply().unwrap();
    assert_eq!(apply.arguments.len(), 1);
    let dotty_tasty::term::RawTree::LengthNode(select_node) = &apply.function else {
        panic!("create applies the selected constructor");
    };
    assert_eq!(select_node.tag, SELECTIN_TAG);
    let select = select_node.decode_select_in().unwrap();
    let signed = factory_tasty
        .names()
        .render_signed_name(select.name)
        .unwrap()
        .unwrap();
    assert!(matches!(
        signed,
        dotty_tasty::tasty::RenderedSignedName::Signed { original, signature }
            if original == "<init>"
                && signature.result == "example.Counter"
                && signature.parameters == [dotty_tasty::name_table::RenderedParamSig::TermParameter("scala.Int".to_owned())]
    ));
    let Some(dotty_tasty::term::RawTree::Ast {
        tag: NEW_TAG,
        child,
        ..
    }) = Some(&select.qualifier)
    else {
        panic!("selected constructor qualifier is NEW");
    };
    assert_counter_type_ref(child, &address_index, &factory_tasty);
    assert_counter_type_ref(&select.underlying_type, &address_index, &factory_tasty);
    assert_eq!(child.ast_refs()[0].address, 44);
    assert_eq!(select.underlying_type.ast_refs()[0].address, 44);
    let seed_references = apply.arguments[0].ast_refs();
    let [seed_reference] = seed_references.as_slice() else {
        panic!("create argument is a direct reference to the seed parameter");
    };
    assert_eq!(
        seed_reference.kind,
        dotty_tasty::term::AstRefKind::TermRefDirect
    );
    assert_eq!(seed_reference.address, seed_address);
    assert_eq!(seed_reference.address, 33);
    assert!(address_index.get(create_address).is_some());
    assert_eq!(
        method_code(&factory, "create"),
        [0xbb, 0x00, 0x11, 0x59, 0x1b, 0xb7, 0x00, 0x14, 0xb0]
    );

    let identity_method = identity
        .methods
        .iter()
        .find(|method| identity.constant_pool.utf8(method.name_index).unwrap() == "identity")
        .expect("CounterIdentity declares the independent identity control");
    assert_eq!(
        identity
            .constant_pool
            .utf8(identity_method.descriptor_index),
        Ok("(Lexample/Counter;)Lexample/Counter;")
    );
    assert_eq!(method_code(&identity, "identity"), [0x2b, 0xb0]);
    let (identity_address, identity_body) = method(&identity_tasty, "identity");
    let identity_index = identity_tasty.ast_address_index().unwrap();
    let counter_formal_address = parameter_address(&identity_index, identity_address);
    assert_eq!(identity_address, 30, "pinned identity method address");
    assert_eq!(identity_body.return_type.ast_refs()[0].address, 38);
    assert_eq!(counter_formal_address, 33);
    assert_counter_type_ref(&identity_body.return_type, &identity_index, &identity_tasty);
    assert_eq!(identity_body.parameters.len(), 1);
    let counter_formal = &identity_body.parameters[0];
    assert_eq!(counter_formal.tag(), PARAM_TAG);
    assert_eq!(
        identity_tasty
            .names()
            .render(counter_formal.name())
            .unwrap(),
        "counter"
    );
    assert_counter_type_ref(
        &counter_formal.decode_body().unwrap().type_tree,
        &identity_index,
        &identity_tasty,
    );
    let Some(dotty_tasty::term::RawTree::Leaf(identity_reference)) = identity_body.rhs else {
        panic!("identity RHS is the original formal reference");
    };
    assert_eq!(identity_reference.tag, TERMREFDIRECT_TAG);
    assert_eq!(
        identity_reference.ast_ref().unwrap().address,
        counter_formal_address
    );
    assert!(
        identity_tasty
            .ast_address_index()
            .unwrap()
            .get(identity_address)
            .is_some()
    );

    assert!((1..=u16::MAX).any(|index| matches!(
        factory.constant_pool.get(dotty_classfile::constant_pool::ConstantPoolIndex(index)),
        Some(ConstantPoolEntry::Methodref { class_index, name_and_type_index })
            if factory.constant_pool.class_name(*class_index) == Ok("example/Counter")
                && factory.constant_pool.name_and_type(*name_and_type_index)
                    .is_ok_and(|(name, descriptor)| name == "<init>" && descriptor == "(I)V")
    )));
}

#[test]
fn paired_tasty_planning_emits_the_canonical_factory_new_body() {
    let artifacts = emit_paired_jvm_tasty_batch(
        FACTORY_SOURCE,
        "CounterFactory.scala",
        SourceId::from_index(1690),
    )
    .expect("the audited same-batch constructor application emits a complete linked batch");
    let factory_artifacts = artifacts
        .classes()
        .iter()
        .find(|class| class.tasty_path() == "example/CounterFactory.tasty")
        .expect("the factory sidecar is present");
    let tasty = assert_pinned_tasty(factory_artifacts.tasty_bytes());
    let (create_address, create) = method(&tasty, "create");
    assert_counter_type_ref(
        &create.return_type,
        &tasty.ast_address_index().unwrap(),
        &tasty,
    );
    let Some(dotty_tasty::term::RawTree::LengthNode(apply_node)) = create.rhs.as_ref() else {
        panic!("factory RHS is the constructor APPLY");
    };
    assert_eq!(apply_node.tag, APPLY_TAG);
    let apply = apply_node.decode_apply().unwrap();
    assert_eq!(apply.arguments.len(), 1);
    let dotty_tasty::term::RawTree::LengthNode(select_node) = &apply.function else {
        panic!("factory applies the selected constructor");
    };
    assert_eq!(select_node.tag, SELECTIN_TAG);
    let select = select_node.decode_select_in().unwrap();
    let signed = tasty
        .names()
        .render_signed_name(select.name)
        .unwrap()
        .unwrap();
    assert!(matches!(
        signed,
        dotty_tasty::tasty::RenderedSignedName::Signed { original, signature }
            if original == "<init>"
                && signature.result == "example.Counter"
                && signature.parameters == [dotty_tasty::name_table::RenderedParamSig::TermParameter("scala.Int".to_owned())]
    ));
    let Some(dotty_tasty::term::RawTree::Ast {
        tag: NEW_TAG,
        child,
        ..
    }) = Some(&select.qualifier)
    else {
        panic!("constructor receiver is NEW");
    };
    let index = tasty.ast_address_index().unwrap();
    assert_counter_type_ref(child, &index, &tasty);
    assert_counter_type_ref(&select.underlying_type, &index, &tasty);
    let argument_references = apply.arguments[0].ast_refs();
    let [seed_reference] = argument_references.as_slice() else {
        panic!("constructor argument is the original seed formal");
    };
    assert_eq!(
        seed_reference.kind,
        dotty_tasty::term::AstRefKind::TermRefDirect
    );
    assert_eq!(
        seed_reference.address,
        parameter_address(&index, create_address)
    );

    let class = decode_class(factory_artifacts.classfile_bytes());
    let create_member = class
        .methods
        .iter()
        .find(|member| class.constant_pool.utf8(member.name_index).unwrap() == "create")
        .unwrap();
    assert_eq!(
        class.constant_pool.utf8(create_member.descriptor_index),
        Ok("(I)Lexample/Counter;")
    );
    assert_eq!(
        method_code(&class, "create"),
        [0xbb, 0x00, 0x0e, 0x59, 0x1b, 0xb7, 0x00, 0x11, 0xb0]
    );
    assert!(index.get(create_address).is_some());

    let second = emit_paired_jvm_tasty_batch(
        FACTORY_SOURCE,
        "CounterFactory.scala",
        SourceId::from_index(1694),
    )
    .expect("a fresh source session emits the same constructor batch");
    assert_eq!(artifacts.classes().len(), second.classes().len());
    for (first, second) in artifacts.classes().iter().zip(second.classes()) {
        assert_eq!(first.classfile_path(), second.classfile_path());
        assert_eq!(first.classfile_bytes(), second.classfile_bytes());
        assert_eq!(first.tasty_path(), second.tasty_path());
        assert_eq!(first.tasty_bytes(), second.tasty_bytes());
        assert_eq!(first.tasty_link_id(), second.tasty_link_id());
    }

    if std::env::var_os(RUN_SCALA_ORACLE).is_some() {
        let counter_artifacts = artifacts
            .classes()
            .iter()
            .find(|class| class.tasty_path() == "example/Counter.tasty")
            .expect("Counter sidecar is in the complete source batch");
        run_factory_constructor_oracle(
            counter_artifacts.tasty_bytes(),
            factory_artifacts.tasty_bytes(),
        );
    }
}

#[test]
fn paired_tasty_planning_emits_the_canonical_reference_field_new_body() {
    let artifacts = emit_paired_jvm_tasty_batch(
        REFERENCE_FIELD_SOURCE,
        "CounterOwner.scala",
        SourceId::from_index(1717),
    )
    .expect("the audited immutable reference field emits a complete linked batch");
    let counter = artifacts
        .classes()
        .iter()
        .find(|class| class.tasty_path() == "example/Counter.tasty")
        .expect("the Counter dependency sidecar is emitted");
    let owner = artifacts
        .classes()
        .iter()
        .find(|class| class.tasty_path() == "example/CounterOwner.tasty")
        .expect("the owner sidecar is emitted");
    let tasty = assert_pinned_tasty(owner.tasty_bytes());
    let package = tasty
        .asts()
        .unwrap()
        .get(0)
        .unwrap()
        .decode_package()
        .unwrap();
    assert_eq!(
        package.stats.len(),
        1,
        "the owner sidecar has no local Counter clone"
    );
    let dotty_tasty::tasty::DefinitionBody::TypeDef {
        type_or_template, ..
    } = package
        .stats
        .get(0)
        .unwrap()
        .decode_definition_body()
        .unwrap()
    else {
        panic!("the owner sidecar contains CounterOwner")
    };
    let dotty_tasty::term::RawTree::LengthNode(template_node) = type_or_template else {
        panic!("CounterOwner contains a template")
    };
    let template = template_node.decode_template_structure().unwrap();
    let field = template
        .stats
        .iter()
        .find_map(|tree| match tree {
            dotty_tasty::term::RawTree::LengthNode(node) if node.tag == VALDEF_TAG => {
                Some(node.decode_definition_body().unwrap())
            }
            _ => None,
        })
        .unwrap();
    let dotty_tasty::tasty::DefinitionBody::ValDef { name, rhs, .. } = field else {
        panic!("CounterOwner contains its immutable inner field")
    };
    assert_eq!(tasty.names().render(name).unwrap(), "inner");
    let Some(dotty_tasty::term::RawTree::LengthNode(rhs)) = rhs else {
        panic!("inner has a constructor initializer")
    };
    let application = rhs.decode_apply().unwrap();
    assert_eq!(application.arguments.len(), 1);
    let dotty_tasty::term::RawTree::LengthNode(selection_node) = application.function else {
        panic!("the initializer applies the selected Counter constructor")
    };
    let selection = selection_node.decode_select_in().unwrap();
    let signed = tasty
        .names()
        .render_signed_name(selection.name)
        .unwrap()
        .unwrap();
    assert!(matches!(
        signed,
        dotty_tasty::tasty::RenderedSignedName::Signed { original, signature }
            if original == "<init>"
                && signature.result == "example.Counter"
                && signature.parameters == [dotty_tasty::name_table::RenderedParamSig::TermParameter("scala.Int".to_owned())]
    ));
    assert!(matches!(
        selection.qualifier,
        dotty_tasty::term::RawTree::Ast { tag: NEW_TAG, .. }
    ));
    let argument_references = application.arguments[0].ast_refs();
    assert!(matches!(
        argument_references.as_slice(),
        [reference] if reference.kind == dotty_tasty::term::AstRefKind::TermRefDirect
    ));
    if std::env::var_os(RUN_SCALA_ORACLE).is_some() {
        run_reference_field_oracle(counter.tasty_bytes(), owner.tasty_bytes());
    }
}

#[test]
fn class_state_plan_emits_reference_field_receiver_and_scala_executes_it() {
    with_paired_jvm_tasty_batch_plan(
        REFERENCE_FIELD_READ_SOURCE,
        "CounterOwner.scala",
        SourceId::from_index(1718),
        |batch| {
            assert_eq!(batch.classes.len(), 2);
            let counter = batch.classes[0]
                .tasty_plan
                .pickle()
                .expect("Counter TASTy is complete");
            let owner = batch.classes[1]
                .tasty_plan
                .pickle()
                .expect("CounterOwner TASTy includes its canonical field receiver call");
            TastyFile::parse_and_validate_scala_3_9(counter.bytes()).unwrap();
            TastyFile::parse_and_validate_scala_3_9(owner.bytes()).unwrap();
            if std::env::var_os(RUN_SCALA_ORACLE).is_some() {
                run_reference_field_read_oracle(counter.bytes(), owner.bytes());
            }
        },
    )
    .unwrap();
}

#[test]
fn paired_tasty_planning_rejects_constructor_shapes_outside_the_exact_slice() {
    let cases = [
        (
            "computed constructor argument",
            FACTORY_SOURCE.replace("new Counter(seed)", "new Counter(seed + 1)"),
            Some("BodyExpression"),
        ),
        (
            "wrong constructor arity",
            FACTORY_SOURCE.replace("new Counter(seed)", "new Counter()"),
            None,
        ),
        (
            "wrong constructor argument type",
            FACTORY_SOURCE.replace("new Counter(seed)", "new Counter(true)"),
            None,
        ),
        (
            "wrong method result type",
            FACTORY_SOURCE.replace(
                "def create(seed: Int): Counter",
                "def create(seed: Int): Int",
            ),
            None,
        ),
        (
            "private primary constructor",
            FACTORY_SOURCE.replace(
                "class Counter(start: Int):",
                "class Counter private (start: Int):",
            ),
            None,
        ),
    ];
    for (index, (case, source, expected_error)) in cases.into_iter().enumerate() {
        let error = emit_paired_jvm_tasty_batch(
            &source,
            "CounterFactory.scala",
            SourceId::from_index(1695 + index as u32),
        )
        .expect_err("unsupported constructor application must not publish a batch");
        let evidence = format!("{error:?}");
        if let Some(expected) = expected_error {
            assert!(
                evidence.contains(expected),
                "{case} should fail in TASTy expression validation: {evidence}"
            );
        }
    }
}

fn run_factory_constructor_oracle(counter_tasty: &[u8], factory_tasty: &[u8]) {
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

    let temporary_root = std::env::var_os("DOTTY_TASTY_PICKLER_ORACLE_TMP")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("TMPDIR").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir);
    let temporary = temporary_root.join(format!(
        "dotty-issue-1693-constructor-oracle-{}",
        std::process::id()
    ));
    fs::remove_dir_all(&temporary).ok();
    let candidate = temporary.join("candidate/example");
    let compiled = temporary.join("compiled");
    fs::create_dir_all(&candidate).unwrap();
    fs::create_dir_all(&compiled).unwrap();
    let counter_path = candidate.join("Counter.tasty");
    let factory_path = candidate.join("CounterFactory.tasty");
    fs::write(&counter_path, counter_tasty).unwrap();
    fs::write(&factory_path, factory_tasty).unwrap();

    let decompile = Command::new(&compiler)
        .arg("-decompile")
        .arg("-classpath")
        .arg(temporary.join("candidate"))
        .arg(&counter_path)
        .arg(&factory_path)
        .output()
        .expect("decompile Rust-produced Counter and CounterFactory sidecars");
    assert!(
        decompile.status.success(),
        "Scala decompiler rejected the linked sidecars: {}",
        String::from_utf8_lossy(&decompile.stderr)
    );
    let decompiled = strip_ansi(&String::from_utf8_lossy(&decompile.stdout));
    assert!(
        decompiled.contains("= new example.Counter(seed)"),
        "Scala decompiler did not recover the actual constructor body: {decompiled}"
    );

    let from_tasty = Command::new(&compiler)
        .arg("-from-tasty")
        .arg("-classpath")
        .arg(temporary.join("candidate"))
        .arg("-d")
        .arg(&compiled)
        .arg(&counter_path)
        .arg(&factory_path)
        .output()
        .expect("consume Rust-produced factory sidecar with pinned Scala");
    assert!(
        from_tasty.status.success(),
        "Scala -from-tasty rejected the factory constructor application: {}",
        String::from_utf8_lossy(&from_tasty.stderr)
    );
    let javap = Command::new("javap")
        .arg("-classpath")
        .arg(&compiled)
        .arg("-s")
        .arg("example.CounterFactory")
        .output()
        .expect("inspect Scala-rebuilt factory descriptor");
    assert!(javap.status.success());
    assert!(
        String::from_utf8_lossy(&javap.stdout).contains("(I)Lexample/Counter;"),
        "Scala-rebuilt factory has the wrong descriptor: {}",
        String::from_utf8_lossy(&javap.stdout)
    );
    fs::remove_dir_all(temporary).unwrap();
}

fn run_reference_field_oracle(counter_tasty: &[u8], owner_tasty: &[u8]) {
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

    let temporary_root = std::env::var_os("DOTTY_TASTY_PICKLER_ORACLE_TMP")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("TMPDIR").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir);
    let temporary = temporary_root.join(format!(
        "dotty-issue-1717-reference-field-oracle-{}",
        std::process::id()
    ));
    fs::remove_dir_all(&temporary).ok();
    let candidate = temporary.join("candidate/example");
    let compiled = temporary.join("compiled");
    fs::create_dir_all(&candidate).unwrap();
    fs::create_dir_all(&compiled).unwrap();
    let counter_path = candidate.join("Counter.tasty");
    let owner_path = candidate.join("CounterOwner.tasty");
    fs::write(&counter_path, counter_tasty).unwrap();
    fs::write(&owner_path, owner_tasty).unwrap();

    let decompile = Command::new(&compiler)
        .arg("-decompile")
        .arg("-classpath")
        .arg(temporary.join("candidate"))
        .arg(&counter_path)
        .arg(&owner_path)
        .output()
        .expect("decompile Rust-produced Counter and CounterOwner sidecars");
    assert!(
        decompile.status.success(),
        "Scala decompiler rejected the linked sidecars: {}",
        String::from_utf8_lossy(&decompile.stderr)
    );
    let decompiled = strip_ansi(&String::from_utf8_lossy(&decompile.stdout));
    assert!(
        decompiled.contains("new example.Counter(seed)"),
        "Scala decompiler did not recover the field constructor initializer: {decompiled}"
    );
    let from_tasty = Command::new(&compiler)
        .arg("-from-tasty")
        .arg("-classpath")
        .arg(temporary.join("candidate"))
        .arg("-d")
        .arg(&compiled)
        .arg(&counter_path)
        .arg(&owner_path)
        .output()
        .expect("consume Rust-produced reference field sidecars with pinned Scala");
    assert!(
        from_tasty.status.success(),
        "Scala -from-tasty rejected the field constructor initializer: {}",
        String::from_utf8_lossy(&from_tasty.stderr)
    );
    let javap = Command::new("javap")
        .arg("-classpath")
        .arg(&compiled)
        .arg("-s")
        .arg("example.CounterOwner")
        .output()
        .expect("inspect Scala-rebuilt owner constructor descriptor");
    assert!(javap.status.success());
    assert!(String::from_utf8_lossy(&javap.stdout).contains("(I)V"));
    fs::remove_dir_all(temporary).unwrap();
}

fn run_reference_field_read_oracle(counter_tasty: &[u8], owner_tasty: &[u8]) {
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

    let temporary_root = std::env::var_os("DOTTY_TASTY_PICKLER_ORACLE_TMP")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("TMPDIR").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir);
    let temporary = temporary_root.join(format!(
        "dotty-issue-1718-reference-read-oracle-{}",
        std::process::id()
    ));
    fs::remove_dir_all(&temporary).ok();
    let candidate = temporary.join("candidate/example");
    let compiled = temporary.join("compiled");
    let client = temporary.join("client");
    fs::create_dir_all(&candidate).unwrap();
    fs::create_dir_all(&compiled).unwrap();
    fs::create_dir_all(&client).unwrap();
    let counter_path = candidate.join("Counter.tasty");
    let owner_path = candidate.join("CounterOwner.tasty");
    fs::write(&counter_path, counter_tasty).unwrap();
    fs::write(&owner_path, owner_tasty).unwrap();

    let decompile = Command::new(&compiler)
        .arg("-decompile")
        .arg("-classpath")
        .arg(temporary.join("candidate"))
        .arg(&counter_path)
        .arg(&owner_path)
        .output()
        .expect("decompile Rust-produced Counter and CounterOwner sidecars");
    assert!(
        decompile.status.success(),
        "Scala decompiler rejected the linked sidecars: {}",
        String::from_utf8_lossy(&decompile.stderr)
    );
    let decompiled = strip_ansi(&String::from_utf8_lossy(&decompile.stdout));
    assert!(
        decompiled.contains("new example.Counter(seed)"),
        "Scala decompiler did not recover the field initializer: {decompiled}"
    );
    assert!(
        decompiled.contains("inner.get()"),
        "Scala decompiler did not recover the cross-class field receiver call: {decompiled}"
    );

    let from_tasty = Command::new(&compiler)
        .arg("-from-tasty")
        .arg("-classpath")
        .arg(temporary.join("candidate"))
        .arg("-d")
        .arg(&compiled)
        .arg(&counter_path)
        .arg(&owner_path)
        .output()
        .expect("consume Rust-produced reference-field sidecars with pinned Scala");
    assert!(
        from_tasty.status.success(),
        "Scala -from-tasty rejected the field receiver call: {}",
        String::from_utf8_lossy(&from_tasty.stderr)
    );
    let javap = Command::new("javap")
        .arg("-classpath")
        .arg(&compiled)
        .arg("-c")
        .arg("-s")
        .arg("example.CounterOwner")
        .output()
        .expect("inspect Scala-rebuilt owner constructor and receiver call");
    assert!(javap.status.success());
    let javap_output = String::from_utf8_lossy(&javap.stdout);
    assert!(javap_output.contains("(I)V"));
    assert!(javap_output.contains("read();"));
    assert!(javap_output.contains("()I"));
    assert!(javap_output.contains("getfield"));
    assert!(javap_output.contains("invokevirtual"));

    let client_source = temporary.join("CounterOwnerClient.scala");
    fs::write(
        &client_source,
        "package example\nobject CounterOwnerClient:\n  def main(args: Array[String]): Unit =\n    val owner = new CounterOwner(7)\n    assert(owner.read() == 7)\n",
    )
    .unwrap();
    let compile_client_classpath =
        std::env::join_paths([temporary.join("candidate"), temporary.join("compiled")]).unwrap();
    let compiled_client = Command::new(&compiler)
        .arg("-classpath")
        .arg(compile_client_classpath)
        .arg("-d")
        .arg(&client)
        .arg(&client_source)
        .output()
        .expect("compile Scala client against rebuilt sidecars");
    assert!(
        compiled_client.status.success(),
        "Scala client failed to compile against rebuilt sidecars: {}",
        String::from_utf8_lossy(&compiled_client.stderr)
    );
    let scala3_library = scala_root.join(
        "dist/target/universal/stage/maven2/org/scala-lang/scala3-library_3/3.9.0-bin-SNAPSHOT/scala3-library_3-3.9.0-bin-SNAPSHOT.jar",
    );
    let scala_library = scala_root.join(
        "dist/target/universal/stage/maven2/org/scala-lang/scala-library/3.9.0-bin-SNAPSHOT/scala-library-3.9.0-bin-SNAPSHOT.jar",
    );
    let runtime_classpath =
        std::env::join_paths([client, compiled, scala3_library, scala_library]).unwrap();
    let execution = Command::new("java")
        .args(["-Xverify:all", "-ea", "-classpath"])
        .arg(runtime_classpath)
        .arg("example.CounterOwnerClient")
        .output()
        .expect("run Scala client against the rebuilt sidecars");
    assert!(
        execution.status.success(),
        "Scala client failed to execute the rebuilt field receiver: {}",
        String::from_utf8_lossy(&execution.stderr)
    );
    fs::remove_dir_all(temporary).unwrap();
}

fn strip_ansi(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for character in chars.by_ref() {
                if character.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            output.push(character);
        }
    }
    output
}
