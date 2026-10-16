use dotty_tasty::tasty::*;

fn leaf(tag: u8, value: TermValue) -> AstNode {
    AstNode::Leaf(SimpleTerm::new(tag, value).unwrap())
}

#[test]
fn forward_declaration_crosses_nat_width_and_nested_length_boundaries() {
    let mut builder = AstBuilder::new(AstBuildLimits::default());
    let declaration = builder.reserve().unwrap();
    let reference = builder
        .push(AstNode::Reference {
            tag: TYPEREFDIRECT_TAG,
            target: declaration,
        })
        .unwrap();
    let mut fields = vec![AstField::Tree(reference)];
    for _ in 0..128 {
        fields.push(AstField::Tree(
            builder.push(leaf(UNITCONST_TAG, TermValue::Unit)).unwrap(),
        ));
    }
    fields.push(AstField::Tree(declaration));
    // A wire-only synthetic declaration, not a semantic compiler symbol.
    let tpt = builder.push(leaf(UNITCONST_TAG, TermValue::Unit)).unwrap();
    builder
        .bind(
            declaration,
            AstNode::Length {
                tag: TYPEDEF_TAG,
                fields: vec![AstField::Nat(0), AstField::Tree(tpt)],
            },
        )
        .unwrap();
    let root = builder
        .push(AstNode::Length {
            tag: BLOCK_TAG,
            fields,
        })
        .unwrap();
    let built = builder.finish(&[root]).unwrap();
    let target = built.address(declaration).unwrap();
    assert!(target > 127);
    assert_eq!(built.bytes()[target as usize], TYPEDEF_TAG);
    let RawTree::LengthNode(tree) = RawTree::decode(&mut Reader::new(built.bytes())).unwrap()
    else {
        panic!()
    };
    let refs = tree.ast_refs().unwrap();
    assert_eq!(
        refs,
        [AstRef {
            kind: AstRefKind::TypeRefDirect,
            address: target
        }]
    );
    assert_eq!(tree.decode_block().unwrap().stats.len(), 129);
    assert_eq!(builder.finish(&[root]).unwrap(), built);
}

#[test]
fn shared_types_target_the_original_node_and_cannot_point_forward() {
    let mut builder = AstBuilder::new(AstBuildLimits::default());
    let original = builder
        .push(leaf(TERMREFPKG_TAG, TermValue::NameRef(0)))
        .unwrap();
    let shared = builder
        .push(AstNode::Reference {
            tag: SHAREDTYPE_TAG,
            target: original,
        })
        .unwrap();
    let built = builder.finish(&[original, shared]).unwrap();
    let mut reader = Reader::new(built.bytes());
    RawTree::decode(&mut reader).unwrap();
    assert_eq!(
        RawTree::decode(&mut reader).unwrap().ast_ref(),
        Some(AstRef {
            kind: AstRefKind::SharedType,
            address: 0
        })
    );
    assert_eq!(
        builder.finish(&[shared, original]),
        Err(AstBuildError::InvalidReference {
            tag: SHAREDTYPE_TAG,
            target: original
        })
    );
}

#[test]
fn labels_must_be_bound_once_emitted_once_and_acyclic() {
    let mut builder = AstBuilder::new(AstBuildLimits::default());
    let a = builder.reserve().unwrap();
    assert_eq!(builder.finish(&[a]), Err(AstBuildError::Unbound(a)));
    builder
        .bind(a, leaf(UNITCONST_TAG, TermValue::Unit))
        .unwrap();
    assert_eq!(
        builder.bind(a, leaf(UNITCONST_TAG, TermValue::Unit)),
        Err(AstBuildError::AlreadyBound(a))
    );
    assert_eq!(builder.finish(&[a, a]), Err(AstBuildError::RepeatedNode(a)));
    assert_eq!(builder.finish(&[]), Err(AstBuildError::UnusedNode(a)));
    let b = builder.reserve().unwrap();
    builder
        .bind(
            b,
            AstNode::Ast {
                tag: THIS_TAG,
                child: b,
            },
        )
        .unwrap();
    assert_eq!(builder.finish(&[a, b]), Err(AstBuildError::Cycle(b)));
}

#[test]
fn references_validate_target_namespace_and_reserved_labels() {
    let mut builder = AstBuilder::new(AstBuildLimits::default());
    let target = builder.push(leaf(UNITCONST_TAG, TermValue::Unit)).unwrap();
    let reference = builder
        .push(AstNode::Reference {
            tag: TYPEREFDIRECT_TAG,
            target,
        })
        .unwrap();
    assert_eq!(
        builder.finish(&[reference, target]),
        Err(AstBuildError::InvalidReference {
            tag: TYPEREFDIRECT_TAG,
            target
        })
    );
    let mut other = AstBuilder::new(AstBuildLimits::default());
    other.reserve().unwrap();
    other.reserve().unwrap();
    let invalid = other.reserve().unwrap();
    assert_eq!(
        builder.finish(&[invalid]),
        Err(AstBuildError::InvalidLabel(invalid))
    );
}

#[test]
fn literal_numeric_addresses_and_wrong_category_four_operands_are_rejected() {
    let mut builder = AstBuilder::new(AstBuildLimits::default());
    let target = builder.push(leaf(UNITCONST_TAG, TermValue::Unit)).unwrap();
    assert!(matches!(
        builder.push(leaf(TYPEREFDIRECT_TAG, TermValue::AstRef(123))),
        Err(AstBuildError::InvalidReference { .. })
    ));
    assert!(matches!(
        builder.push(AstNode::NatAst {
            tag: TYPEREFSYMBOL_TAG,
            value: AstOperand::Nat(0),
            child: target
        }),
        Err(AstBuildError::InvalidReference { .. })
    ));
    assert!(matches!(
        builder.push(AstNode::NatAst {
            tag: TYPEREF_TAG,
            value: AstOperand::Reference(target),
            child: target
        }),
        Err(AstBuildError::InvalidReference { .. })
    ));
    // Failed pushes did not leave unbound or unreachable labels behind.
    assert_eq!(builder.finish(&[target]).unwrap().bytes(), &[UNITCONST_TAG]);
}

#[test]
fn construction_limits_fail_without_publishing_bytes() {
    let mut builder = AstBuilder::new(AstBuildLimits {
        max_items: 1,
        ..AstBuildLimits::default()
    });
    builder.reserve().unwrap();
    assert_eq!(builder.reserve(), Err(AstBuildError::Limit("items")));
    for (limits, expected) in [
        (
            AstBuildLimits {
                max_bytes: 0,
                ..AstBuildLimits::default()
            },
            "bytes",
        ),
        (
            AstBuildLimits {
                max_depth: 0,
                ..AstBuildLimits::default()
            },
            "depth",
        ),
        (
            AstBuildLimits {
                max_layout_passes: 0,
                ..AstBuildLimits::default()
            },
            "layout passes",
        ),
    ] {
        let mut builder = AstBuilder::new(limits);
        let child = builder.push(leaf(UNITCONST_TAG, TermValue::Unit)).unwrap();
        let parent = builder
            .push(AstNode::Ast {
                tag: THIS_TAG,
                child,
            })
            .unwrap();
        assert_eq!(
            builder.finish(&[parent]),
            Err(AstBuildError::Limit(expected))
        );
    }
}

#[test]
fn method_type_fields_keep_name_nats_after_each_parameter_tree() {
    let mut builder = AstBuilder::new(AstBuildLimits::default());
    let result = builder
        .push(leaf(TERMREFPKG_TAG, TermValue::NameRef(0)))
        .unwrap();
    let param = builder
        .push(leaf(TERMREFPKG_TAG, TermValue::NameRef(1)))
        .unwrap();
    let method = builder
        .push(AstNode::Length {
            tag: METHODTYPE_TAG,
            fields: vec![
                AstField::Tree(result),
                AstField::Tree(param),
                AstField::Nat(2),
            ],
        })
        .unwrap();
    let built = builder.finish(&[method]).unwrap();
    let RawTree::LengthNode(raw) = RawTree::decode(&mut Reader::new(built.bytes())).unwrap() else {
        panic!()
    };
    let method = raw.decode_method_type().unwrap();
    assert_eq!(method.type_names.len(), 1);
    assert_eq!(method.type_names[0].name, 2);
    assert_eq!(method.modifiers, []);
}
