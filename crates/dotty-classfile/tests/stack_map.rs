use dotty_classfile::classfile::{
    Attribute, ClassAccessFlags, ClassFile, ClassFileVersion, ClassReference, CodeSequence,
    ConstantPool, ConstantPoolEntry, ConstantPoolIndex, ConstantPoolIndexOperand, EncodeError,
    FullFrame, Instruction, RawAttribute, StackMapTable, VerificationTypeInfo, assemble_labeled,
};

fn empty_pool() -> ConstantPool {
    ConstantPool::default()
}

fn frame(
    pc: u32,
    locals: Vec<VerificationTypeInfo>,
    stack: Vec<VerificationTypeInfo>,
) -> FullFrame {
    FullFrame { pc, locals, stack }
}

#[test]
fn encodes_empty_and_absolute_pc_full_frames_exactly() {
    assert_eq!(
        StackMapTable::default()
            .encode_payload(&empty_pool())
            .unwrap(),
        [0x00, 0x00]
    );

    let pool = ConstantPool::from_entries(vec![
        Some(ConstantPoolEntry::Utf8("Example".to_owned())),
        Some(ConstantPoolEntry::Class {
            name_index: ConstantPoolIndex(1),
        }),
    ]);
    let table = StackMapTable {
        frames: vec![
            frame(3, vec![VerificationTypeInfo::Integer], vec![]),
            frame(
                9,
                vec![
                    VerificationTypeInfo::Top,
                    VerificationTypeInfo::Object(ConstantPoolIndex(2)),
                ],
                vec![VerificationTypeInfo::Integer],
            ),
        ],
    };
    assert_eq!(
        table.encode_payload(&pool).unwrap(),
        [
            0x00, 0x02, // number_of_entries
            0xff, 0x00, 0x03, // first frame: offset_delta = absolute PC 3
            0x00, 0x01, 0x01, // one Integer local
            0x00, 0x00, // empty stack
            0xff, 0x00, 0x05, // second frame: 9 - 3 - 1
            0x00, 0x02, 0x00, 0x07, 0x00, 0x02, // Top and Object(#2)
            0x00, 0x01, 0x01, // one Integer stack item
        ]
    );
}

#[test]
fn counts_verification_entries_separately_from_slots() {
    let table = StackMapTable {
        frames: vec![frame(
            0,
            vec![
                VerificationTypeInfo::Long,
                VerificationTypeInfo::Top,
                VerificationTypeInfo::Double,
            ],
            vec![VerificationTypeInfo::Long, VerificationTypeInfo::Double],
        )],
    };
    let encoded = table.encode_payload(&empty_pool()).unwrap();
    assert_eq!(
        encoded,
        [
            0x00, 0x01, 0xff, 0x00, 0x00, 0x00, 0x03, 0x04, 0x00, 0x03, 0x00, 0x02, 0x04, 0x03,
        ]
    );
    assert_eq!(VerificationTypeInfo::Long.slot_count(), 2);
    assert_eq!(VerificationTypeInfo::Double.slot_count(), 2);
    assert_eq!(VerificationTypeInfo::Top.slot_count(), 1);
}

#[test]
fn encodes_every_verification_type_tag() {
    let table = StackMapTable {
        frames: vec![frame(
            0,
            vec![
                VerificationTypeInfo::Float,
                VerificationTypeInfo::Null,
                VerificationTypeInfo::UninitializedThis,
            ],
            vec![],
        )],
    };
    assert_eq!(
        table.encode_payload(&empty_pool()).unwrap(),
        [
            0x00, 0x01, 0xff, 0x00, 0x00, 0x00, 0x03, 0x02, 0x05, 0x06, 0x00, 0x00,
        ]
    );
}

#[test]
fn validates_order_ranges_and_constant_pool_kinds() {
    let unsorted = StackMapTable {
        frames: vec![frame(4, vec![], vec![]), frame(4, vec![], vec![])],
    };
    assert_eq!(
        unsorted.encode_payload(&empty_pool()),
        Err(EncodeError::InvalidStackMapFrameOrder {
            frame_index: 1,
            previous_pc: 4,
            pc: 4,
        })
    );
    let descending = StackMapTable {
        frames: vec![frame(5, vec![], vec![]), frame(4, vec![], vec![])],
    };
    assert!(matches!(
        descending.encode_payload(&empty_pool()),
        Err(EncodeError::InvalidStackMapFrameOrder { frame_index: 1, .. })
    ));
    let too_large = StackMapTable {
        frames: vec![frame(65_536, vec![], vec![])],
    };
    assert_eq!(
        too_large.encode_payload(&empty_pool()),
        Err(EncodeError::InvalidStackMapFrameOffset {
            frame_index: 0,
            pc: 65_536,
        })
    );

    let wrong_kind_pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Utf8(
        "not a class".to_owned(),
    ))]);
    let object = StackMapTable {
        frames: vec![frame(
            0,
            vec![VerificationTypeInfo::Object(ConstantPoolIndex(1))],
            vec![],
        )],
    };
    assert_eq!(
        object.encode_payload(&wrong_kind_pool),
        Err(EncodeError::InvalidReference { index: 1 })
    );
    let missing_pool = StackMapTable {
        frames: vec![frame(
            0,
            vec![VerificationTypeInfo::Object(ConstantPoolIndex(1))],
            vec![],
        )],
    };
    assert_eq!(
        missing_pool.encode_payload(&empty_pool()),
        Err(EncodeError::InvalidIndex { index: 1, count: 0 })
    );
}

#[test]
fn rejects_u2_entry_count_overflow_without_truncation() {
    let too_many_frames = StackMapTable {
        frames: vec![frame(0, vec![], vec![]); 65_536],
    };
    assert_eq!(
        too_many_frames.encode_payload(&empty_pool()),
        Err(EncodeError::CountOverflow {
            value: 65_536,
            width: dotty_classfile::classfile::LengthWidth::U16,
        })
    );
    let too_many_locals = StackMapTable {
        frames: vec![frame(0, vec![VerificationTypeInfo::Top; 65_536], vec![])],
    };
    assert_eq!(
        too_many_locals.encode_payload(&empty_pool()),
        Err(EncodeError::CountOverflow {
            value: 65_536,
            width: dotty_classfile::classfile::LengthWidth::U16,
        })
    );
    let too_many_stack_items = StackMapTable {
        frames: vec![frame(0, vec![], vec![VerificationTypeInfo::Top; 65_536])],
    };
    assert_eq!(
        too_many_stack_items.encode_payload(&empty_pool()),
        Err(EncodeError::CountOverflow {
            value: 65_536,
            width: dotty_classfile::classfile::LengthWidth::U16,
        })
    );
}

#[test]
fn layout_validation_checks_instruction_boundaries_and_new_targets() {
    let pool = ConstantPool::from_entries(vec![
        Some(ConstantPoolEntry::Utf8("Example".to_owned())),
        Some(ConstantPoolEntry::Class {
            name_index: ConstantPoolIndex(1),
        }),
    ]);
    let mut sequence = CodeSequence::new();
    sequence.instruction(Instruction::New(ClassReference::new(
        ConstantPoolIndexOperand::new(ConstantPoolIndex(2)).unwrap(),
    )));
    sequence.instruction(Instruction::AConstNull);
    let assembled = assemble_labeled(&sequence, &pool).unwrap();

    let good = StackMapTable {
        frames: vec![frame(
            3,
            vec![VerificationTypeInfo::Uninitialized(0)],
            vec![],
        )],
    };
    assert!(
        good.encode_payload_with_layout(&pool, &assembled.bytes, &assembled.layout)
            .is_ok()
    );

    let bad_frame_boundary = StackMapTable {
        frames: vec![frame(1, vec![], vec![])],
    };
    assert!(matches!(
        bad_frame_boundary.encode_payload_with_layout(&pool, &assembled.bytes, &assembled.layout),
        Err(EncodeError::InvalidStackMapFrameOffset {
            frame_index: 0,
            pc: 1
        })
    ));
    let bad_new_target = StackMapTable {
        frames: vec![frame(
            3,
            vec![VerificationTypeInfo::Uninitialized(3)],
            vec![],
        )],
    };
    assert!(matches!(
        bad_new_target.encode_payload_with_layout(&pool, &assembled.bytes, &assembled.layout),
        Err(EncodeError::InvalidStackMapUninitializedOffset { pc: 3, .. })
    ));
    assert_eq!(
        good.encode_payload_with_layout(&pool, &[0], &assembled.layout),
        Err(EncodeError::InvalidStackMapCodeLayout)
    );
}

#[test]
fn opaque_attribute_classfile_roundtrip_preserves_stack_map_payload() {
    let payload = StackMapTable {
        frames: vec![frame(2, vec![VerificationTypeInfo::Integer], vec![])],
    }
    .encode_payload(&ConstantPool::default())
    .unwrap();
    let pool = ConstantPool::from_entries(vec![
        Some(ConstantPoolEntry::Utf8("Example".to_owned())),
        Some(ConstantPoolEntry::Class {
            name_index: ConstantPoolIndex(1),
        }),
        Some(ConstantPoolEntry::Utf8("StackMapTable".to_owned())),
    ]);
    let class_file = ClassFile {
        version: ClassFileVersion {
            major: 65,
            minor: 0,
        },
        constant_pool: pool,
        access_flags: ClassAccessFlags(0x0021),
        this_class: ConstantPoolIndex(2),
        super_class: None,
        interfaces: vec![],
        fields: vec![],
        methods: vec![],
        attributes: vec![Attribute::Other(RawAttribute {
            name_index: ConstantPoolIndex(3),
            bytes: &payload,
        })],
    };
    let encoded = class_file.encode().unwrap();
    let mut reader = dotty_classfile::classfile::Reader::new(&encoded);
    let decoded = ClassFile::decode(&mut reader).unwrap();
    assert!(reader.is_at_end());
    assert_eq!(decoded.encode().unwrap(), encoded.as_slice());
    assert_eq!(
        decoded.attributes,
        [Attribute::Other(RawAttribute {
            name_index: ConstantPoolIndex(3),
            bytes: &payload,
        })]
    );
}
