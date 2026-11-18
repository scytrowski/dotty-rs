use dotty_tasty::{AstRefKind, NameTable, RawName, RawTree, StructuredNode, TastyFile, TermValue};

#[test]
fn widening_the_package_name_preserves_nested_reference_targets() {
    let bytes = include_bytes!("fixtures/simple_def/SimpleDef.tasty");
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let mut roots = file.structured_asts().unwrap();
    let StructuredNode::Package(package) = &mut roots[0] else {
        panic!("expected a package");
    };
    let RawTree::Leaf(path) = &mut package.path else {
        panic!("expected a package path");
    };
    let TermValue::NameRef(name) = &mut path.value else {
        panic!("expected a package name");
    };

    // Reuse the same name at an index requiring a second byte on the wire.
    let mut names = file.names().entries().to_vec();
    let original_name = names[*name as usize].clone();
    names.resize(128, RawName::Utf8(String::new()));
    names.push(original_name);
    *name = 128;
    let extended = TastyFile::from_parts(
        file.header().clone(),
        NameTable::from_entries(names).unwrap(),
        file.sections().clone(),
    )
    .unwrap();

    let before = file.ast_references().unwrap();
    assert_eq!(before.len(), 19);
    assert_eq!(
        (before[0].reference.kind, before[0].reference.address),
        (AstRefKind::TypeRefSymbol, 25)
    );
    assert_eq!(
        (before[4].reference.kind, before[4].reference.address),
        (AstRefKind::TermRefSymbol, 5)
    );

    assert_eq!(
        dotty_tasty::EncodedSection::structured_asts(0, &roots),
        Err(dotty_tasty::TermEncodeError::Ast(
            dotty_tasty::AstError::InvalidAstReference { address: 25 }
        ))
    );
    let encoded = extended.encode_structured_relocated(&roots).unwrap();
    let relocated = TastyFile::parse_and_validate_scala_3_9(&encoded).unwrap();
    let after = relocated.ast_references().unwrap();
    assert_eq!(before.len(), after.len());
    for (old, new) in before.iter().zip(after) {
        assert_eq!(new.reference.kind, old.reference.kind);
        assert_eq!(new.reference.address, old.reference.address + 1);
    }
}

#[test]
fn relocation_converges_when_canonical_padding_cancels_an_edit() {
    use dotty_tasty::{
        APPLY_TAG, EncodedSection, INTCONST_TAG, RawNodes, Reader, SHAREDTERM_TAG, UNITCONST_TAG,
    };

    // The third root starts at 11. Growing the first literal by one byte
    // moves it to 12 until the nested reference loses its single pad byte.
    let bytes = [
        APPLY_TAG,
        0x82,
        INTCONST_TAG,
        0xbf,
        APPLY_TAG,
        0x85,
        APPLY_TAG,
        0x83,
        SHAREDTERM_TAG,
        0,
        0x8b,
        APPLY_TAG,
        0x81,
        UNITCONST_TAG,
    ];
    let parsed = RawNodes::decode(&mut Reader::new(&bytes)).unwrap();
    let mut roots: Vec<_> = parsed
        .iter()
        .map(|n| n.decode_structured().unwrap())
        .collect();
    let StructuredNode::Apply(first) = &mut roots[0] else {
        panic!("expected an application")
    };
    first.function = RawTree::leaf(INTCONST_TAG, TermValue::Int(64)).unwrap();

    let relocated = EncodedSection::structured_asts_relocated(0, &bytes, &roots).unwrap();
    assert_eq!(relocated.ast_addresses(), &[0, 5, 11]);
    let nodes = RawNodes::decode(&mut Reader::new(relocated.payload())).unwrap();
    let refs = nodes.get(1).unwrap().ast_refs().unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].address as usize, nodes.get(2).unwrap().offset);
}

#[test]
fn relocation_rejects_swapped_declarations_even_with_the_same_name() {
    use dotty_tasty::{
        AstError, BLOCK_TAG, EncodedSection, INTCONST_TAG, RawNodes, Reader, TERMREFDIRECT_TAG,
        TYPEREFPKG_TAG, TermEncodeError, VALDEF_TAG,
    };

    for second_name in [0x81, 0x82] {
        let bytes = [
            BLOCK_TAG,
            0x90,
            TERMREFDIRECT_TAG,
            0x84,
            VALDEF_TAG,
            0x85,
            0x81,
            TYPEREFPKG_TAG,
            0x83,
            INTCONST_TAG,
            0xa9,
            VALDEF_TAG,
            0x85,
            second_name,
            TYPEREFPKG_TAG,
            0x83,
            INTCONST_TAG,
            0xaa,
        ];
        let raw = RawNodes::decode(&mut Reader::new(&bytes)).unwrap();
        let mut block = raw.get(0).unwrap().decode_block().unwrap();
        block.stats.swap(0, 1);
        assert_eq!(
            EncodedSection::structured_asts_relocated(0, &bytes, &[StructuredNode::Block(block)]),
            Err(TermEncodeError::Ast(AstError::AmbiguousRelocation {
                tag: VALDEF_TAG,
                offset: 11
            }))
        );
    }
}

#[test]
fn shared_match_case_keeps_its_wire_node_and_relocates_to_the_case() {
    use dotty_tasty::tasty::*;
    let bytes = [
        APPLY_TAG,
        0x82,
        INTCONST_TAG,
        0xbf,
        MATCH_TAG,
        0x87,
        IMPLICIT_TAG,
        CASEDEF_TAG,
        0x82,
        TRUECONST_TAG,
        UNITCONST_TAG,
        SHAREDTERM_TAG,
        0x87,
    ];
    let raw = RawNodes::decode(&mut Reader::new(&bytes)).unwrap();
    let mut roots: Vec<_> = raw
        .iter()
        .map(|node| node.decode_structured().unwrap())
        .collect();
    let StructuredNode::Match(m) = &roots[1] else {
        panic!("expected MATCH")
    };
    assert!(matches!(
        &m.cases[..],
        [
            MatchCase::Inline(_),
            MatchCase::Shared {
                offset: 5,
                address: 7
            }
        ]
    ));
    let StructuredNode::Apply(app) = &mut roots[0] else {
        panic!("expected APPLY")
    };
    app.function = RawTree::leaf(INTCONST_TAG, TermValue::Int(64)).unwrap();
    let section = EncodedSection::structured_asts_relocated(0, &bytes, &roots).unwrap();
    let raw = RawNodes::decode(&mut Reader::new(section.payload())).unwrap();
    let original = TastyFile::parse(include_bytes!("fixtures/simple_def/SimpleDef.tasty")).unwrap();
    let builder = TastyFileBuilder::new(
        original.header().clone(),
        NameTable::from_entries(vec![RawName::Utf8("ASTs".into())]).unwrap(),
    )
    .with_section(section.section().clone());
    let encoded = builder.encode_validated().unwrap();
    let file = TastyFile::parse_and_validate_scala_3_9(&encoded).unwrap();
    let index = file.ast_address_index().unwrap();
    let children: Vec<_> = index.children_of(5).collect();
    assert_eq!(
        children,
        [
            AstTreeNode {
                tag: CASEDEF_TAG,
                offset: 8
            },
            AstTreeNode {
                tag: SHAREDTERM_TAG,
                offset: 12
            }
        ]
    );
    let refs = raw.get(1).unwrap().ast_refs().unwrap();
    assert_eq!(
        refs,
        [AstRef {
            kind: AstRefKind::SharedCase,
            address: 8
        }]
    );
    assert_eq!(index.resolve_node(refs[0]), Some(children[0]));
    let StructuredNode::Match(m) = &mut roots[1] else {
        unreachable!()
    };
    m.cases[1] = MatchCase::Shared {
        offset: 5,
        address: 0,
    };
    assert_eq!(
        EncodedSection::structured_asts_relocated(0, &bytes, &roots),
        Err(TermEncodeError::Ast(AstError::InvalidAstReference {
            address: 0
        }))
    );
}

#[test]
fn explicit_identities_follow_both_reordered_subtrees_across_address_width_changes() {
    use dotty_tasty::tasty::*;
    // Both definitions have the same name and tag. Only the supplied identities
    // tell the encoder which definition the expression names.
    // The preceding tree puts the definitions on either side of address 128.
    let mut bytes = vec![ALTERNATIVE_TAG, 0xf6];
    bytes.extend(std::iter::repeat_n(UNITCONST_TAG, 118));
    bytes.extend([
        BLOCK_TAG,
        0x90,
        TERMREFDIRECT_TAG,
        0xfc,
        VALDEF_TAG,
        0x85,
        0x81,
        TYPEREFPKG_TAG,
        0x83,
        INTCONST_TAG,
        0xa9,
        VALDEF_TAG,
        0x85,
        0x81,
        TYPEREFPKG_TAG,
        0x83,
        INTCONST_TAG,
        0xaa,
    ]);
    let raw = RawNodes::decode(&mut Reader::new(&bytes)).unwrap();
    let mut block = raw.get(1).unwrap().decode_block().unwrap();
    block.stats.swap(0, 1);
    let roots = [
        raw.get(0).unwrap().decode_structured().unwrap(),
        StructuredNode::Block(block),
    ];
    let order = AstNodeIdentityMap::from_edited_order(
        [0].into_iter()
            .chain(2..120)
            .chain([120, 122, 131, 134, 136, 124, 127, 129]),
    );
    let section =
        EncodedSection::structured_asts_relocated_with_identities(0, &bytes, &roots, &order)
            .unwrap();
    let parsed = RawNodes::decode(&mut Reader::new(section.payload())).unwrap();
    let block = parsed.get(1).unwrap();
    let result = block.decode_block().unwrap();
    let RawTree::LengthNode(a) = &result.stats[1] else {
        panic!("expected VALDEF")
    };
    assert_eq!(a.offset, 10);
    assert_eq!(
        block.ast_refs().unwrap(),
        [AstRef {
            kind: AstRefKind::TermRefDirect,
            address: 132
        }]
    );
    assert_eq!(a.payload.last(), Some(&0xa9));
}

#[test]
fn rejects_incomplete_or_inconsistent_explicit_identities() {
    use dotty_tasty::tasty::*;
    let bytes = [
        APPLY_TAG,
        0x81,
        UNITCONST_TAG,
        APPLY_TAG,
        0x81,
        UNITCONST_TAG,
    ];
    let raw = RawNodes::decode(&mut Reader::new(&bytes)).unwrap();
    let roots: Vec<_> = raw.iter().map(|n| n.decode_structured().unwrap()).collect();
    let run = |ids: &[u32]| {
        EncodedSection::structured_asts_relocated_with_identities(
            0,
            &bytes,
            &roots,
            &AstNodeIdentityMap::from_edited_order(ids.iter().copied()),
        )
    };
    assert_eq!(
        run(&[0, 2, 3]),
        Err(TermEncodeError::Ast(AstError::AstNodeCountMismatch {
            expected: 4,
            actual: 3
        }))
    );
    assert_eq!(
        run(&[0, 2, 3, 4]),
        Err(TermEncodeError::Ast(AstError::InvalidAstReference {
            address: 4
        }))
    );
    assert_eq!(
        run(&[0, 2, 3, 2]),
        Err(TermEncodeError::DuplicateNodeIdentity { address: 2 })
    );
    assert_eq!(
        run(&[0, 5, 3, 2]),
        Err(TermEncodeError::RelocationParentMismatch { address: 5 })
    );
    assert!(matches!(
        run(&[2, 0, 3, 5]),
        Err(TermEncodeError::Ast(AstError::UnexpectedTag { .. }))
    ));
    assert_eq!(run(&[0, 2, 3, 5]).unwrap().payload(), bytes);
}

#[test]
fn explicit_relocation_validates_operands_in_the_original_address_space() {
    use dotty_tasty::tasty::*;
    // Address 4 is inside the old integer operand but becomes a new tree start.
    let bytes = [APPLY_TAG, 0x85, INTCONST_TAG, 0, 0xbf, SHAREDTERM_TAG, 0x84];
    let raw = RawNodes::decode(&mut Reader::new(&bytes)).unwrap();
    let roots = [raw.get(0).unwrap().decode_structured().unwrap()];
    let order = AstNodeIdentityMap::from_edited_order([0, 2, 5]);
    assert_eq!(
        EncodedSection::structured_asts_relocated_with_identities(0, &bytes, &roots, &order),
        Err(TermEncodeError::Ast(AstError::InvalidAstReference {
            address: 4
        }))
    );
}

#[test]
fn explicit_relocation_rejects_hidden_operands_even_in_stationary_opaque_nodes() {
    use dotty_tasty::tasty::*;
    let bytes = [
        200,
        0x80,
        APPLY_TAG,
        0x81,
        UNITCONST_TAG,
        APPLY_TAG,
        0x81,
        TRUECONST_TAG,
    ];
    let raw = RawNodes::decode(&mut Reader::new(&bytes)).unwrap();
    let mut roots: Vec<_> = raw.iter().map(|n| n.decode_structured().unwrap()).collect();
    roots.swap(1, 2);
    let order = AstNodeIdentityMap::from_edited_order([0, 5, 7, 2, 4]);
    assert_eq!(
        EncodedSection::structured_asts_relocated_with_identities(0, &bytes, &roots, &order),
        Err(TermEncodeError::Ast(AstError::UnsupportedRelocation {
            tag: 200,
            offset: 0
        }))
    );
}

#[test]
fn explicit_file_relocation_keeps_spans_sources_and_comments_with_their_nodes() {
    use dotty_tasty::tasty::*;
    let header = TastyFile::parse(include_bytes!("fixtures/simple_def/SimpleDef.tasty"))
        .unwrap()
        .header()
        .clone();
    let names = NameTable::from_entries(
        ["ASTs", "Positions", "Comments", "A.scala", "B.scala"]
            .map(|n| RawName::Utf8(n.into()))
            .into(),
    )
    .unwrap();
    let positions = PositionSection {
        line_sizes: vec![12],
        entries: vec![
            PositionEntry::Source(3),
            PositionEntry::Association {
                address_delta: 3,
                start_delta: Some(0),
                end_delta: None,
                point_delta: None,
            },
            PositionEntry::Source(4),
            PositionEntry::Association {
                address_delta: -3,
                start_delta: Some(1),
                end_delta: Some(8),
                point_delta: Some(1),
            },
        ],
    };
    let comment = Comment {
        address: 2,
        text: "original leaf".into(),
        coordinates: 19,
    };
    let builder = TastyFileBuilder::new(header, names)
        .with_section(EncodedSection::raw(
            0,
            [
                APPLY_TAG,
                0x81,
                UNITCONST_TAG,
                APPLY_TAG,
                0x81,
                TRUECONST_TAG,
            ],
        ))
        .with_section(EncodedSection::positions(1, &positions).unwrap())
        .with_section(EncodedSection::comments(2, std::slice::from_ref(&comment)).unwrap());
    let file = builder.build().unwrap();
    let mut roots = file.structured_asts().unwrap();
    roots.swap(0, 1);
    let ids = AstNodeIdentityMap::from_edited_order([3, 5, 0, 2]);
    let bytes = file
        .encode_structured_relocated_with_identities(&roots, &ids)
        .unwrap();
    let result = TastyFile::parse_and_validate_scala_3_9(&bytes).unwrap();
    assert_eq!(
        result.ast_address_index().unwrap().get_node(2).unwrap().tag,
        TRUECONST_TAG
    );
    let mut spans = positions.resolved_associations().unwrap();
    spans[0].address = 0;
    spans[1].address = 3;
    assert_eq!(
        result.resolved_position_associations().unwrap().unwrap(),
        spans
    );
    assert_eq!(
        result.comments().unwrap().unwrap(),
        [Comment {
            address: 5,
            ..comment
        }]
    );
}

#[test]
fn explicit_file_relocation_rejects_positions_with_no_original_node_identity() {
    use dotty_tasty::tasty::*;
    let header = TastyFile::parse(include_bytes!("fixtures/simple_def/SimpleDef.tasty"))
        .unwrap()
        .header()
        .clone();
    let names = NameTable::from_entries(vec![
        RawName::Utf8("ASTs".into()),
        RawName::Utf8("Positions".into()),
    ])
    .unwrap();
    let positions = PositionSection {
        line_sizes: vec![],
        entries: vec![PositionEntry::Association {
            address_delta: 1,
            start_delta: None,
            end_delta: None,
            point_delta: None,
        }],
    };
    let builder = TastyFileBuilder::new(header, names)
        .with_section(EncodedSection::raw(
            0,
            [
                APPLY_TAG,
                0x81,
                UNITCONST_TAG,
                APPLY_TAG,
                0x81,
                TRUECONST_TAG,
            ],
        ))
        .with_section(EncodedSection::positions(1, &positions).unwrap());
    let file = builder.build().unwrap();
    let mut roots = file.structured_asts().unwrap();
    roots.swap(0, 1);
    assert_eq!(
        file.encode_structured_relocated_with_identities(
            &roots,
            &AstNodeIdentityMap::from_edited_order([3, 5, 0, 2])
        ),
        Err(TastyFileError::Sections(
            SectionError::MissingPositionIdentity { address: 1 }
        ))
    );
}
