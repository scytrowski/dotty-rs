use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_batch;
use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::ConstantPoolEntry;
use dotty_classfile::reader::Reader as ClassReader;
use dotty_core::SourceId;
use dotty_tasty::ast::{
    APPLY_TAG, DEFDEF_TAG, IDENTTPT_TAG, NEW_TAG, PARAM_TAG, SELECTIN_TAG, SHAREDTYPE_TAG,
    TERMREFDIRECT_TAG, TERMREFPKG_TAG, TYPEREF_TAG,
};
use dotty_tasty::file::TastyFile;

const FACTORY_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_cross_class_factories/CounterFactory.scala");
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
fn paired_tasty_planning_rejects_factory_new_at_body_validation() {
    let error = emit_paired_jvm_tasty_batch(
        FACTORY_SOURCE,
        "CounterFactory.scala",
        SourceId::from_index(1690),
    )
    .expect_err("constructor New remains outside the #1692 body subset");
    let evidence = format!("{error:?}");
    assert!(
        evidence.contains("phase: \"JVM/TASTy cross-contract validation\""),
        "the real factory body must reach TASTy/JVM contract validation: {evidence}"
    );
    assert!(
        evidence.contains("TastyPlan(BodyType"),
        "the unsupported New expression must fail at body validation, not MethodResult: {evidence}"
    );
}
