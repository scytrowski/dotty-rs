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
