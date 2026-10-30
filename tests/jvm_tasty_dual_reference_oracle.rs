use dotty_classfile::access_flags::{
    ACC_FINAL, ACC_PRIVATE, ACC_PUBLIC, FieldAccessFlags, MethodAccessFlags,
};
use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::reader::Reader;
use dotty_jvm_tasty_contract::{JvmTastyContractError, validate_classfile_tasty_link};
use dotty_tasty::ast::{
    DEFDEF_TAG, LOCAL_TAG, PARAM_TAG, PRIVATE_TAG, TERMREFSYMBOL_TAG, THIS_TAG, VALDEF_TAG,
};
use dotty_tasty::file::TastyFile;
use dotty_tasty::tasty::{DefinitionBody, DefinitionTail};

const COUNTER_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_dual_reference_constructors/scala39/example/Counter.class");
const COUNTER_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_dual_reference_constructors/scala39/example/Counter.tasty");
const PAIR_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_dual_reference_constructors/scala39/example/Pair.class");
const PAIR_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_dual_reference_constructors/scala39/example/Pair.tasty");

fn decode_class(bytes: &[u8]) -> ClassFile<'_> {
    let mut reader = Reader::new(bytes);
    let class = ClassFile::decode(&mut reader).expect("Scala classfile decodes");
    assert!(reader.is_at_end());
    class
}

fn linked_tasty<'a>(class: &ClassFile<'_>, bytes: &'a [u8]) -> TastyFile<'a> {
    let tasty = TastyFile::parse_and_validate_scala_3_9(bytes).expect("Scala TASTy 28.9 decodes");
    assert_eq!(
        (
            tasty.header().major_version,
            tasty.header().minor_version,
            tasty.header().experimental_version,
        ),
        (28, 9, 0)
    );
    assert_eq!(
        class.tasty_link_id().unwrap().unwrap().as_bytes(),
        &tasty.header().uuid,
        "the classfile TASTY attribute links its exact sidecar UUID"
    );
    tasty
}

fn method<'a>(class: &'a ClassFile<'_>, name: &str) -> &'a dotty_classfile::method::MethodInfo<'a> {
    class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == name)
        .unwrap_or_else(|| panic!("Scala output declares method {name}"))
}

fn field<'a>(class: &'a ClassFile<'_>, name: &str) -> &'a dotty_classfile::field::FieldInfo<'a> {
    class
        .fields
        .iter()
        .find(|field| class.constant_pool.utf8(field.name_index).unwrap() == name)
        .unwrap_or_else(|| panic!("Scala output declares field {name}"))
}

fn code<'a>(method: &'a dotty_classfile::method::MethodInfo<'a>) -> &'a [u8] {
    method
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::Code(code) => Some(code.code),
            _ => None,
        })
        .expect("method has Code")
}

#[test]
fn scala39_oracle_pins_two_distinct_reference_formals_fields_and_tasty_links() {
    let counter = decode_class(COUNTER_CLASS);
    let pair = decode_class(PAIR_CLASS);
    assert_eq!((counter.version.major, pair.version.major), (65, 65));
    let counter_tasty = linked_tasty(&counter, COUNTER_TASTY);
    assert_eq!(
        counter_tasty.header().uuid,
        [
            0x00, 0xad, 0x1e, 0x24, 0x19, 0xc4, 0xd8, 0xe8, 0x00, 0x68, 0xaa, 0x0b, 0xd5, 0x46,
            0x1a, 0x1b
        ]
    );
    let pair_tasty = linked_tasty(&pair, PAIR_TASTY);
    assert_eq!(
        pair_tasty.header().uuid,
        [
            0x00, 0x7f, 0xab, 0xc3, 0xcc, 0xf4, 0x25, 0xd3, 0x00, 0x43, 0x37, 0x29, 0x73, 0x54,
            0x69, 0x5b
        ]
    );
    let mut missing_link = decode_class(PAIR_CLASS);
    missing_link.attributes.retain(|attribute| match attribute {
        Attribute::Other(attribute) => {
            missing_link
                .constant_pool
                .utf8(attribute.name_index)
                .unwrap()
                != "TASTY"
        }
        _ => true,
    });
    assert!(matches!(
        validate_classfile_tasty_link(&missing_link, PAIR_TASTY),
        Err(JvmTastyContractError::MissingTastyLink { .. })
    ));
    let mut malformed = PAIR_TASTY.to_vec();
    malformed[0] ^= 0xff;
    assert!(TastyFile::parse_and_validate_scala_3_9(&malformed).is_err());
    let mut stale_uuid = PAIR_TASTY.to_vec();
    let tasty_uuid = pair_tasty.header().uuid;
    let uuid_offset = stale_uuid
        .windows(tasty_uuid.len())
        .position(|bytes| bytes == tasty_uuid)
        .expect("fixture sidecar contains its header UUID");
    stale_uuid[uuid_offset] ^= 0x01;
    assert!(TastyFile::parse_and_validate_scala_3_9(&stale_uuid).is_ok());
    assert!(matches!(
        validate_classfile_tasty_link(&pair, &stale_uuid),
        Err(JvmTastyContractError::StaleTastyLink { .. })
    ));

    assert_eq!(counter.fields.len(), 1);
    assert_eq!(
        counter
            .constant_pool
            .utf8(field(&counter, "current").descriptor_index)
            .unwrap(),
        "I"
    );
    assert_eq!(
        field(&counter, "current").access_flags,
        FieldAccessFlags(ACC_PRIVATE)
    );
    assert_eq!(
        counter
            .constant_pool
            .utf8(method(&counter, "<init>").descriptor_index)
            .unwrap(),
        "(I)V"
    );
    assert_eq!(
        counter
            .constant_pool
            .utf8(method(&counter, "get").descriptor_index)
            .unwrap(),
        "()I"
    );
    assert_eq!(
        counter
            .constant_pool
            .utf8(method(&counter, "set").descriptor_index)
            .unwrap(),
        "(I)V"
    );

    assert_eq!(
        pair.fields.len(),
        2,
        "the two source aliases are distinct physical fields"
    );
    for name in ["left", "right"] {
        let field = field(&pair, name);
        assert_eq!(
            field.access_flags,
            FieldAccessFlags(ACC_PRIVATE | ACC_FINAL)
        );
        assert_eq!(
            pair.constant_pool.utf8(field.descriptor_index).unwrap(),
            "Lexample/Counter;"
        );
    }
    let constructor = method(&pair, "<init>");
    assert_eq!(constructor.access_flags, MethodAccessFlags(ACC_PUBLIC));
    assert_eq!(
        pair.constant_pool
            .utf8(constructor.descriptor_index)
            .unwrap(),
        "(Lexample/Counter;Lexample/Counter;)V"
    );
    let instructions = code(constructor);
    assert_eq!(instructions[0], 0x2a, "aload_0");
    assert_eq!(
        instructions[4..6],
        [0x2a, 0x2b],
        "left receives constructor slot 1"
    );
    assert_eq!(
        instructions[9..11],
        [0x2a, 0x2c],
        "right receives constructor slot 2"
    );
    assert_eq!(
        *instructions.last().unwrap(),
        0xb1,
        "constructor returns normally"
    );
    for (name, descriptor) in [("leftValue", "()I"), ("rightValue", "()I")] {
        let method = method(&pair, name);
        assert_eq!(method.access_flags, MethodAccessFlags(ACC_PUBLIC));
        assert_eq!(
            pair.constant_pool.utf8(method.descriptor_index).unwrap(),
            descriptor
        );
        assert!(
            code(method).contains(&[0xb4][0]),
            "read path accesses its alias field"
        );
        assert!(
            code(method).contains(&0xb6),
            "read path invokes Counter.get"
        );
    }

    let index = pair_tasty.ast_address_index().unwrap();
    let parameters = index
        .iter_nodes_with_tag(PARAM_TAG)
        .map(|node| {
            let address = node.offset as u32;
            let parameter = index.get(address).unwrap().decode_parameter().unwrap();
            (
                address,
                pair_tasty.names().render(parameter.name()).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        parameters,
        [
            (11, "first".to_owned()),
            (20, "second".to_owned()),
            (42, "first".to_owned()),
            (49, "second".to_owned())
        ]
    );
    for address in [11, 20] {
        let parameter = index.get(address).unwrap().decode_parameter().unwrap();
        let body = parameter.decode_body().unwrap();
        assert!(body.tail.contains(&DefinitionTail::Modifier(PRIVATE_TAG)));
        assert!(body.tail.contains(&DefinitionTail::Modifier(LOCAL_TAG)));
    }
    assert_eq!(
        index
            .iter_nodes_with_tag(VALDEF_TAG)
            .map(|node| node.offset as u32)
            .collect::<Vec<_>>(),
        [60, 76],
        "left and right remain separate source VALDEF declarations"
    );
    for (address, expected_name, expected_template_parameter) in
        [(60, "left", 11), (76, "right", 20)]
    {
        let DefinitionBody::ValDef {
            name,
            rhs: Some(rhs),
            ..
        } = index
            .get(address)
            .unwrap()
            .decode_definition_body()
            .unwrap()
        else {
            panic!("field alias VALDEF retains its source RHS");
        };
        assert_eq!(pair_tasty.names().render(name).unwrap(), expected_name);
        let alias = rhs.decode_reference().unwrap();
        assert_eq!(alias.tag, TERMREFSYMBOL_TAG);
        assert_eq!(alias.reference, expected_template_parameter);
        assert!(
            index
                .iter_nodes_with_tag(THIS_TAG)
                .any(|node| node.offset as u32 == 69)
        );
    }
    for (address, expected_tag) in [
        (99, dotty_tasty::ast::APPLY_TAG),
        (101, dotty_tasty::ast::SELECTIN_TAG),
        (104, dotty_tasty::ast::SELECT_TAG),
        (121, dotty_tasty::ast::APPLY_TAG),
        (123, dotty_tasty::ast::SELECTIN_TAG),
        (126, dotty_tasty::ast::SELECT_TAG),
    ] {
        assert!(
            index
                .iter_nodes_with_tag(expected_tag)
                .any(|node| node.offset as u32 == address)
        );
    }
    let methods = index
        .iter_nodes_with_tag(DEFDEF_TAG)
        .filter_map(|node| {
            let body = index.get(node.offset as u32)?.decode_defdef_body().ok()?;
            Some(pair_tasty.names().render(body.name).ok()?)
        })
        .collect::<Vec<_>>();
    assert!(methods.contains(&"<init>".to_owned()));
    assert!(methods.contains(&"leftValue".to_owned()));
    assert!(methods.contains(&"rightValue".to_owned()));
}

#[test]
fn scala39_signature_only_control_keeps_the_same_two_formal_abi() {
    const CLASS: &[u8] = include_bytes!(
        "fixtures/jvm_tasty_dual_reference_constructors/scala39/signature-only/example/Pair.class"
    );
    const TASTY: &[u8] = include_bytes!(
        "fixtures/jvm_tasty_dual_reference_constructors/scala39/signature-only/example/Pair.tasty"
    );
    let class = decode_class(CLASS);
    assert_eq!(class.version.major, 65);
    let _tasty = linked_tasty(&class, TASTY);
    assert!(class.fields.is_empty());
    assert_eq!(class.methods.len(), 1);
    assert_eq!(
        class
            .constant_pool
            .utf8(method(&class, "<init>").descriptor_index)
            .unwrap(),
        "(Lexample/Counter;Lexample/Counter;)V"
    );
}
