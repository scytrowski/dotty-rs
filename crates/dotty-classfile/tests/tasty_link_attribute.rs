use dotty_classfile::classfile::{
    Attribute, ClassFile, ClassFileBuilder, ClassFileVersion, Reader, TastyLinkError, TastyLinkId,
    TastyLinkLocation,
};

const VERSION: ClassFileVersion = ClassFileVersion {
    major: 65,
    minor: 0,
};
const COUNTER_CLASS: &[u8] =
    include_bytes!("../../../tests/fixtures/jvm_tasty_interop/scala39/example/Counter.class");
const COUNTER_LINK_ID: [u8; 16] = [
    0x00, 0x9d, 0x71, 0x1b, 0xcc, 0x01, 0x99, 0xe8, 0x00, 0xce, 0x94, 0x0d, 0xd0, 0x72, 0x1c, 0x1b,
];

fn decode(bytes: &[u8]) -> ClassFile<'_> {
    let mut reader = Reader::new(bytes);
    ClassFile::decode(&mut reader).expect("classfile decodes")
}

fn linked_class(id: TastyLinkId) -> Vec<u8> {
    let mut builder = ClassFileBuilder::new("example/Linked", VERSION).unwrap();
    builder.tasty_link(id).unwrap();
    builder.encode().unwrap()
}

#[test]
fn emits_the_exact_scala_39_tasty_link_and_reads_it_as_a_typed_id() {
    let id = TastyLinkId::from_bytes(COUNTER_LINK_ID);
    let first = linked_class(id);
    let second = linked_class(id);
    assert_eq!(first, second);

    let class = decode(&first);
    assert_eq!(class.tasty_link_id(), Ok(Some(id)));
    assert_eq!(class.encode().unwrap(), first);
    let [Attribute::Other(raw)] = class.attributes.as_slice() else {
        panic!("the TASTY wire attribute remains lossless raw data");
    };
    assert_eq!(class.constant_pool.utf8(raw.name_index), Ok("TASTY"));
    assert_eq!(raw.bytes, COUNTER_LINK_ID);

    let scala = decode(COUNTER_CLASS);
    assert_eq!(scala.tasty_link_id(), Ok(Some(id)));
}

#[test]
fn distinct_ids_change_only_the_link_payload() {
    let first_id = TastyLinkId::from_bytes(COUNTER_LINK_ID);
    let second_id = TastyLinkId::from_bytes([0x5a; TastyLinkId::LENGTH]);
    let first_bytes = linked_class(first_id);
    let second_bytes = linked_class(second_id);
    let first = decode(&first_bytes);
    let second = decode(&second_bytes);

    assert_eq!(first.constant_pool, second.constant_pool);
    assert_eq!(first.fields, second.fields);
    assert_eq!(first.methods, second.methods);
    assert_eq!(first.tasty_link_id(), Ok(Some(first_id)));
    assert_eq!(second.tasty_link_id(), Ok(Some(second_id)));
}

#[test]
fn fixed_width_id_parser_rejects_non_sixteen_byte_payloads() {
    for length in [0, 15, 17] {
        let payload = vec![0u8; length];
        assert_eq!(
            TastyLinkId::try_from_bytes(&payload),
            Err(TastyLinkError::InvalidPayloadLength { length })
        );
    }
}

#[test]
fn builder_rejects_a_second_link_attribute() {
    let id = TastyLinkId::from_bytes(COUNTER_LINK_ID);
    let mut builder = ClassFileBuilder::new("example/Linked", VERSION).unwrap();
    builder.tasty_link(id).unwrap();
    assert!(matches!(
        builder.tasty_link(id),
        Err(dotty_classfile::classfile::ClassFileBuildError::DuplicateTastyLink)
    ));
}

#[test]
fn accessor_rejects_malformed_duplicate_and_misplaced_links() {
    let id = TastyLinkId::from_bytes(COUNTER_LINK_ID);
    let bytes = linked_class(id);
    let mut malformed = decode(&bytes);
    let Attribute::Other(raw) = &mut malformed.attributes[0] else {
        unreachable!();
    };
    raw.bytes = &[0; 15];
    let malformed_bytes = malformed.encode().unwrap();
    let malformed = decode(&malformed_bytes);
    assert_eq!(
        malformed.tasty_link_id(),
        Err(TastyLinkError::InvalidPayloadLength { length: 15 })
    );

    let mut duplicate = decode(&bytes);
    let link = duplicate.attributes[0].clone();
    duplicate.attributes.push(link);
    assert_eq!(
        duplicate.tasty_link_id(),
        Err(TastyLinkError::DuplicateClassAttribute)
    );

    let mut misplaced = decode(&bytes);
    let link = misplaced.attributes[0].clone();
    let Attribute::Other(raw) = &link else {
        unreachable!();
    };
    let existing_name = raw.name_index;
    misplaced
        .methods
        .push(dotty_classfile::classfile::MethodInfo {
            access_flags: dotty_classfile::classfile::MethodAccessFlags(0x0001),
            name_index: existing_name,
            descriptor_index: existing_name,
            attributes: vec![link],
        });
    assert_eq!(
        misplaced.tasty_link_id(),
        Err(TastyLinkError::MisplacedAttribute {
            location: TastyLinkLocation::Method,
        })
    );
}

#[test]
fn ordinary_class_builder_output_remains_byte_identical_without_a_link() {
    let bytes = ClassFileBuilder::new("C", VERSION)
        .unwrap()
        .encode()
        .unwrap();
    assert_eq!(
        bytes,
        [
            0xca, 0xfe, 0xba, 0xbe, 0x00, 0x00, 0x00, 0x41, 0x00, 0x05, 0x01, 0x00, 0x01, 0x43,
            0x07, 0x00, 0x01, 0x01, 0x00, 0x10, b'j', b'a', b'v', b'a', b'/', b'l', b'a', b'n',
            b'g', b'/', b'O', b'b', b'j', b'e', b'c', b't', 0x07, 0x00, 0x03, 0x00, 0x20, 0x00,
            0x02, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ]
    );
    assert_eq!(decode(&bytes).tasty_link_id(), Ok(None));
}
