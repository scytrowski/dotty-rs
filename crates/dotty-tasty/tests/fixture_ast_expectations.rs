use std::fs;
use std::path::Path;

use dotty_tasty::tasty::{
    APPLIEDTPT_TAG, APPLIEDTYPE_TAG, BLOCK_TAG, CASEDEF_TAG, IF_TAG, INLINE_TAG, LAMBDA_TAG,
    LAMBDATPT_TAG, MATCH_TAG, MATCHTPT_TAG, REFINEDTPT_TAG, RawNode, StructuredNode, TEMPLATE_TAG,
    TYPEBOUNDS_TAG, TYPEBOUNDSTPT_TAG, TYPEPARAM_TAG, TastyFile, UNAPPLY_TAG,
};

fn assert_fixture_has_structured_nodes(relative_path: &str, expected_tags: &[u8]) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(relative_path);
    let bytes = fs::read(&path).unwrap_or_else(|error| panic!("failed to read {path:?}: {error}"));
    let file = TastyFile::parse_scala_3_9(&bytes)
        .unwrap_or_else(|error| panic!("failed to decode {path:?}: {error}"));
    let index = file
        .ast_address_index()
        .unwrap_or_else(|error| panic!("failed to index {path:?}: {error}"));

    assert!(
        index.iter().any(|node| node.tag == TEMPLATE_TAG),
        "fixture {path:?} has no template node"
    );

    for node in index.iter() {
        assert_structured(node, &path);
    }

    for expected_tag in expected_tags {
        assert!(
            index.iter().any(|node| node.tag == *expected_tag),
            "fixture {path:?} has no indexed node with tag {expected_tag}"
        );
    }
}

fn assert_structured(node: &RawNode<'_>, path: &Path) {
    let structured = node
        .decode_structured()
        .unwrap_or_else(|error| panic!("failed to decode tag {} in {path:?}: {error}", node.tag));
    assert!(
        !matches!(structured, StructuredNode::Raw(_)),
        "known indexed tag {} in {path:?} was left as Raw",
        node.tag
    );
}

fn assert_fixture_has_definition_modifier(relative_path: &str, expected_modifier: u8) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(relative_path);
    let bytes = fs::read(&path).unwrap_or_else(|error| panic!("failed to read {path:?}: {error}"));
    let file = TastyFile::parse_scala_3_9(&bytes)
        .unwrap_or_else(|error| panic!("failed to decode {path:?}: {error}"));
    let index = file
        .ast_address_index()
        .unwrap_or_else(|error| panic!("failed to index {path:?}: {error}"));

    let has_modifier = index.iter().any(|node| {
        let modifiers = match node.tag {
            dotty_tasty::tasty::VALDEF_TAG | dotty_tasty::tasty::TYPEDEF_TAG => {
                let body = node.decode_definition_body().unwrap();
                match body {
                    dotty_tasty::tasty::DefinitionBody::ValDef { tail, .. }
                    | dotty_tasty::tasty::DefinitionBody::TypeDef { tail, .. } => tail,
                }
            }
            dotty_tasty::tasty::DEFDEF_TAG => node.decode_defdef_body().unwrap().tail,
            _ => return false,
        };

        modifiers.iter().any(|tail| {
            matches!(
                tail,
                dotty_tasty::tasty::DefinitionTail::Modifier(tag) if *tag == expected_modifier
            )
        })
    });

    assert!(
        has_modifier,
        "fixture {path:?} has no definition with modifier {expected_modifier}"
    );
}

macro_rules! fixture_test {
    ($name:ident, $path:literal $(, $tag:expr)* $(,)?) => {
        #[test]
        fn $name() {
            assert_fixture_has_structured_nodes($path, &[$($tag),*]);
        }
    };
}

fixture_test!(
    block_fixture_has_structured_nodes,
    "block/Block.tasty",
    BLOCK_TAG
);
fixture_test!(
    bounds_fixture_has_structured_nodes,
    "bounds/Bounds.tasty",
    TYPEBOUNDSTPT_TAG
);
fixture_test!(circle_fixture_has_structured_nodes, "bounds/Circle.tasty");
fixture_test!(shape_fixture_has_structured_nodes, "bounds/Shape.tasty");
fixture_test!(
    case_class_fixture_has_structured_nodes,
    "case_class/Point.tasty",
    CASEDEF_TAG
);
fixture_test!(class_fixture_has_structured_nodes, "class/Person.tasty");
fixture_test!(
    empty_object_fixture_has_structured_nodes,
    "empty_object/EmptyObject.tasty"
);
fixture_test!(
    extension_fixture_has_structured_nodes,
    "extension/Extension.tasty"
);
fixture_test!(
    generic_box_fixture_has_structured_nodes,
    "generic/Box.tasty",
    TYPEBOUNDS_TAG,
    TYPEBOUNDSTPT_TAG
);
fixture_test!(
    generic_fixture_has_structured_nodes,
    "generic/Generic.tasty",
    TYPEPARAM_TAG,
    APPLIEDTPT_TAG,
    TYPEBOUNDSTPT_TAG
);
fixture_test!(
    given_fixture_has_structured_nodes,
    "given/Display.tasty",
    TYPEPARAM_TAG,
    APPLIEDTPT_TAG,
    TYPEBOUNDS_TAG,
    TYPEBOUNDSTPT_TAG
);
fixture_test!(if_fixture_has_structured_nodes, "if/If.tasty", IF_TAG);
#[test]
fn inline_fixture_has_structured_nodes() {
    assert_fixture_has_structured_nodes("inline/Inline.tasty", &[]);
    assert_fixture_has_definition_modifier("inline/Inline.tasty", INLINE_TAG);
}
fixture_test!(
    inline_match_fixture_has_structured_nodes,
    "inline_match/InlineMatch.tasty",
    MATCH_TAG,
    CASEDEF_TAG
);
fixture_test!(
    animal_fixture_has_structured_nodes,
    "inheritance/Animal.tasty"
);
fixture_test!(dog_fixture_has_structured_nodes, "inheritance/Dog.tasty");
fixture_test!(aged_fixture_has_structured_nodes, "intersection/Aged.tasty");
fixture_test!(
    intersection_fixture_has_structured_nodes,
    "intersection/Intersection.tasty",
    APPLIEDTYPE_TAG,
    APPLIEDTPT_TAG
);
fixture_test!(
    named_intersection_fixture_has_structured_nodes,
    "intersection/Named.tasty"
);
fixture_test!(
    lambda_fixture_has_structured_nodes,
    "lambda/Lambda.tasty",
    LAMBDA_TAG
);
fixture_test!(
    literal_fixture_has_structured_nodes,
    "literal/Literal.tasty"
);
fixture_test!(
    match_fixture_has_structured_nodes,
    "match/Match.tasty",
    MATCH_TAG,
    CASEDEF_TAG
);
fixture_test!(
    match_type_fixture_has_structured_nodes,
    "match_type/MatchType.tasty",
    LAMBDATPT_TAG,
    MATCHTPT_TAG
);
fixture_test!(opaque_fixture_has_structured_nodes, "opaque/UserId.tasty");
fixture_test!(
    container_fixture_has_structured_nodes,
    "path_dependent/Container.tasty",
    TYPEBOUNDSTPT_TAG
);
fixture_test!(
    path_dependent_fixture_has_structured_nodes,
    "path_dependent/PathDependent.tasty"
);
fixture_test!(
    product_match_fixture_has_structured_nodes,
    "product_match/ProductMatch.tasty",
    MATCH_TAG,
    UNAPPLY_TAG,
    CASEDEF_TAG
);
fixture_test!(
    refinement_fixture_has_structured_nodes,
    "refinement/Refinement.tasty",
    REFINEDTPT_TAG
);
fixture_test!(
    service_fixture_has_structured_nodes,
    "refinement/Service.tasty",
    TYPEBOUNDSTPT_TAG
);
fixture_test!(
    simple_definition_fixture_has_structured_nodes,
    "simple_def/SimpleDef.tasty"
);
fixture_test!(trait_fixture_has_structured_nodes, "trait/WithName.tasty");
fixture_test!(
    type_lambda_fixture_has_structured_nodes,
    "type_lambda/TypeLambda.tasty",
    LAMBDATPT_TAG
);
fixture_test!(union_fixture_has_structured_nodes, "union/Union.tasty");
fixture_test!(
    show_using_fixture_has_structured_nodes,
    "using/Show.tasty",
    TYPEPARAM_TAG,
    TYPEBOUNDS_TAG,
    TYPEBOUNDSTPT_TAG
);
fixture_test!(
    using_fixture_has_structured_nodes,
    "using/Using.tasty",
    TYPEPARAM_TAG,
    APPLIEDTPT_TAG,
    TYPEBOUNDSTPT_TAG
);
