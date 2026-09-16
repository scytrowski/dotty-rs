use std::fs;
use std::path::Path;

use dotty_tasty::tasty::{
    APPLIEDTPT_TAG, APPLIEDTYPE_TAG, BLOCK_TAG, CASEDEF_TAG, ConstantValue, DEFDEF_TAG,
    DefinitionBody, IF_TAG, INLINE_TAG, LAMBDA_TAG, LAMBDATPT_TAG, MATCH_TAG, MATCHTPT_TAG,
    PARAM_TAG, REFINEDTPT_TAG, RawNode, RawTree, SELECTIN_TAG, StructuredNode, StructuredTree,
    TEMPLATE_TAG, TYPEBOUNDS_TAG, TYPEBOUNDSTPT_TAG, TYPED_TAG, TYPEPARAM_TAG, TastyFile,
    UNAPPLY_TAG, VALDEF_TAG,
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

fn with_fixture<R>(relative_path: &str, check: impl for<'a> FnOnce(&TastyFile<'a>) -> R) -> R {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(relative_path);
    let bytes = fs::read(&path).unwrap_or_else(|error| panic!("failed to read {path:?}: {error}"));
    let file = TastyFile::parse_scala_3_9(&bytes)
        .unwrap_or_else(|error| panic!("failed to decode {path:?}: {error}"));
    check(&file)
}

fn wire_name(file: &TastyFile<'_>, reference: u32) -> String {
    // AST name fields in these fixtures are raw indices into the wire name
    // table. Read the entry directly instead of applying NameTable's public
    // one-based reference convenience API a second time.
    file.names()
        .entries()
        .get(reference as usize)
        .and_then(dotty_tasty::tasty::RawName::as_utf8)
        .map(str::to_owned)
        .unwrap_or_else(|| panic!("wire name reference {reference} is not a direct UTF-8 name"))
}

fn assert_named_definition<F>(
    file: &TastyFile<'_>,
    index: &dotty_tasty::tasty::AstAddressIndex<'_>,
    tag: u8,
    expected_name: &str,
    check: F,
) where
    F: FnOnce(StructuredNode<'_>),
{
    let node = index
        .iter()
        .find(|node| {
            if node.tag != tag {
                return false;
            }
            let Ok(definition) = node.decode_definition() else {
                return false;
            };
            wire_name(file, definition.name()) == expected_name
        })
        .unwrap_or_else(|| panic!("fixture has no definition {expected_name:?} with tag {tag}"));
    check(node.decode_structured().unwrap());
}

fn assert_int_constant(tree: &RawTree<'_>, expected: i32) {
    assert_eq!(
        tree.decode_structured().unwrap(),
        StructuredTree::Constant(ConstantValue::Int(expected))
    );
}

fn assert_boolean_constant(tree: &RawTree<'_>, expected: bool) {
    assert_eq!(
        tree.decode_structured().unwrap(),
        StructuredTree::Constant(ConstantValue::Boolean(expected))
    );
}

fn assert_unit_constant(tree: &RawTree<'_>) {
    assert_eq!(
        tree.decode_structured().unwrap(),
        StructuredTree::Constant(ConstantValue::Unit)
    );
}

fn assert_string_constant(file: &TastyFile<'_>, tree: &RawTree<'_>, expected: &str) {
    let StructuredTree::Constant(ConstantValue::String(reference)) =
        tree.decode_structured().unwrap()
    else {
        panic!("expected a string constant, got {tree:?}");
    };
    assert_eq!(wire_name(file, reference), expected);
}

fn definition_name(file: &TastyFile<'_>, node: &RawNode<'_>) -> String {
    let definition = node.decode_definition().unwrap();
    wire_name(file, definition.name())
}

fn assert_named_valdef<F>(file: &TastyFile<'_>, node: &RawNode<'_>, expected_name: &str, check: F)
where
    F: FnOnce(DefinitionBody<'_>),
{
    assert_eq!(definition_name(file, node), expected_name);
    let DefinitionBody::ValDef { .. } = node.decode_definition_body().unwrap() else {
        panic!("expected val definition {expected_name:?}");
    };
    check(node.decode_definition_body().unwrap());
}

#[test]
fn simple_def_fixture_has_identity_signature_and_rhs() {
    with_fixture("simple_def/SimpleDef.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, DEFDEF_TAG, "identity", |structured| {
            let StructuredNode::DefDef(body) = structured else {
                panic!("identity is not a DefDef");
            };
            assert_eq!(body.parameters.len(), 1);
            assert_eq!(body.parameters[0].tag(), PARAM_TAG);
            assert_eq!(wire_name(file, body.parameters[0].name()), "x");
            assert!(body.clauses.is_empty());
            let Some(RawTree::Leaf(term)) = body.rhs else {
                panic!("identity has no direct parameter RHS");
            };
            let reference = term.ast_ref().expect("identity RHS has no AST reference");
            assert_eq!(index.resolve_node(reference).unwrap().tag, PARAM_TAG);
        });
    });
}

#[test]
fn simple_def_fixture_has_add_signature_and_application_rhs() {
    with_fixture("simple_def/SimpleDef.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, DEFDEF_TAG, "add", |structured| {
            let StructuredNode::DefDef(body) = structured else {
                panic!("add is not a DefDef");
            };
            assert_eq!(body.parameters.len(), 2);
            assert_eq!(wire_name(file, body.parameters[0].name()), "a");
            assert_eq!(wire_name(file, body.parameters[1].name()), "b");
            let Some(RawTree::LengthNode(node)) = body.rhs else {
                panic!("add has no application RHS");
            };
            let StructuredNode::Apply(application) = node.decode_structured().unwrap() else {
                panic!("add RHS is not an Apply");
            };
            assert_eq!(application.arguments.len(), 1);
            let RawTree::LengthNode(function) = application.function else {
                panic!("add RHS has no selected + operator");
            };
            assert_eq!(function.tag, SELECTIN_TAG);
            let StructuredNode::SelectIn(select) = function.decode_structured().unwrap() else {
                panic!("add operator is not a SelectIn");
            };
            let dotty_tasty::tasty::RawName::Signed { original, .. } =
                file.names().entries()[select.name as usize]
            else {
                panic!("add operator is not signature-qualified");
            };
            assert_eq!(wire_name(file, original), "+");
        });
    });
}

#[test]
fn literal_fixture_has_integer_value() {
    with_fixture("literal/Literal.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        let node = index
            .iter()
            .find(|node| node.tag == VALDEF_TAG && definition_name(file, node) == "int")
            .unwrap();
        let DefinitionBody::ValDef { rhs: Some(rhs), .. } = node.decode_definition_body().unwrap()
        else {
            panic!("int val has no RHS");
        };
        assert_int_constant(&rhs, 42);
    });
}

#[test]
fn literal_fixture_has_string_value() {
    with_fixture("literal/Literal.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        let node = index
            .iter()
            .find(|node| node.tag == VALDEF_TAG && definition_name(file, node) == "string")
            .unwrap();
        let DefinitionBody::ValDef { rhs: Some(rhs), .. } = node.decode_definition_body().unwrap()
        else {
            panic!("string val has no RHS");
        };
        assert_string_constant(file, &rhs, "hello");
    });
}

#[test]
fn literal_fixture_has_boolean_value() {
    with_fixture("literal/Literal.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        let node = index
            .iter()
            .find(|node| node.tag == VALDEF_TAG && definition_name(file, node) == "boolean")
            .unwrap();
        let DefinitionBody::ValDef { rhs: Some(rhs), .. } = node.decode_definition_body().unwrap()
        else {
            panic!("boolean val has no RHS");
        };
        assert_boolean_constant(&rhs, true);
    });
}

#[test]
fn literal_fixture_has_unit_value() {
    with_fixture("literal/Literal.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        let node = index
            .iter()
            .find(|node| node.tag == VALDEF_TAG && definition_name(file, node) == "unit")
            .unwrap();
        let DefinitionBody::ValDef { rhs: Some(rhs), .. } = node.decode_definition_body().unwrap()
        else {
            panic!("unit val has no RHS");
        };
        assert_unit_constant(&rhs);
    });
}

#[test]
fn block_fixture_has_two_local_bindings_and_returns_the_second() {
    with_fixture("block/Block.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, DEFDEF_TAG, "compute", |structured| {
            let StructuredNode::DefDef(body) = structured else {
                panic!("compute is not a DefDef");
            };
            assert_eq!(body.parameters.len(), 1);
            assert_eq!(wire_name(file, body.parameters[0].name()), "x");
            let Some(RawTree::LengthNode(node)) = body.rhs else {
                panic!("compute has no block RHS");
            };
            let StructuredNode::Block(block) = node.decode_structured().unwrap() else {
                panic!("compute RHS is not a Block");
            };
            assert_eq!(block.stats.len(), 2);
            for (stat, expected_name) in block.stats.iter().zip(["a", "b"]) {
                let RawTree::LengthNode(stat) = stat else {
                    panic!("block stat is not a length-delimited definition");
                };
                assert_named_valdef(file, stat, expected_name, |_| {});
            }
            let RawTree::LengthNode(expression) = block.expression else {
                panic!("block does not return a selected local binding");
            };
            assert_eq!(expression.tag, TYPED_TAG);
            let StructuredNode::Typed(typed) = expression.decode_structured().unwrap() else {
                panic!("block result is not a typed local binding");
            };
            let RawTree::Leaf(term) = &typed.expression else {
                panic!("typed block result is not a direct local reference");
            };
            let reference = term
                .ast_ref()
                .expect("typed block result has no AST reference");
            let target = index
                .get(reference.address)
                .expect("typed block result points outside the AST index");
            assert_eq!(target.tag, VALDEF_TAG);
            assert_eq!(definition_name(file, target), "b");
        });
    });
}

#[test]
fn if_fixture_has_nested_sign_branches() {
    with_fixture("if/If.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, DEFDEF_TAG, "sign", |structured| {
            let StructuredNode::DefDef(body) = structured else {
                panic!("sign is not a DefDef");
            };
            let Some(RawTree::LengthNode(node)) = body.rhs else {
                panic!("sign has no if RHS");
            };
            let StructuredNode::If(outer) = node.decode_structured().unwrap() else {
                panic!("sign RHS is not an If");
            };
            assert!(!outer.inline);
            assert_int_constant(&outer.then_branch, 1);
            let RawTree::LengthNode(inner_node) = outer.else_branch else {
                panic!("sign else branch is not a nested If");
            };
            let StructuredNode::If(inner) = inner_node.decode_structured().unwrap() else {
                panic!("sign else branch is not an If");
            };
            assert!(!inner.inline);
            assert_int_constant(&inner.then_branch, -1);
            assert_int_constant(&inner.else_branch, 0);
        });
    });
}

#[test]
fn lambda_fixture_has_typed_increment_and_lambda_body() {
    with_fixture("lambda/Lambda.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        let node = index
            .iter()
            .find(|node| node.tag == VALDEF_TAG && definition_name(file, node) == "increment")
            .unwrap();
        let DefinitionBody::ValDef { rhs: Some(rhs), .. } = node.decode_definition_body().unwrap()
        else {
            panic!("increment val has no RHS");
        };
        let RawTree::LengthNode(block_node) = rhs else {
            panic!("increment RHS is not the compiler-generated block");
        };
        let StructuredNode::Block(block) = block_node.decode_structured().unwrap() else {
            panic!("increment RHS is not a Block");
        };
        let RawTree::LengthNode(lambda_node) = block.expression else {
            panic!("increment block does not return a Lambda");
        };
        let StructuredNode::Lambda(lambda) = lambda_node.decode_structured().unwrap() else {
            panic!("increment expression is not a Lambda");
        };
        assert!(lambda.target_type.is_none());
        assert!(!block.stats.is_empty());
    });
}

#[test]
fn match_fixture_has_three_cases_and_expected_results() {
    with_fixture("match/Match.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, DEFDEF_TAG, "describe", |structured| {
            let StructuredNode::DefDef(body) = structured else {
                panic!("describe is not a DefDef");
            };
            let Some(RawTree::LengthNode(node)) = body.rhs else {
                panic!("describe has no match RHS");
            };
            let StructuredNode::Match(matched) = node.decode_structured().unwrap() else {
                panic!("describe RHS is not a Match");
            };
            assert!(matched.modifiers.is_empty());
            assert_eq!(matched.cases.len(), 3);
            assert_int_constant(&matched.cases[0].pattern, 0);
            assert_int_constant(&matched.cases[1].pattern, 1);
            assert!(matched.cases[2].guard.is_none());
            for (case, expected) in matched.cases.iter().zip(["zero", "one", "other"]) {
                let RawTree::LengthNode(block_node) = &case.body else {
                    panic!("match case body is not a block");
                };
                let StructuredNode::Block(block) = block_node.decode_structured().unwrap() else {
                    panic!("match case body is not a Block");
                };
                assert!(block.stats.is_empty());
                assert_string_constant(file, &block.expression, expected);
            }
        });
    });
}
