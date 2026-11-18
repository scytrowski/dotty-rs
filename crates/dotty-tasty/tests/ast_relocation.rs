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
