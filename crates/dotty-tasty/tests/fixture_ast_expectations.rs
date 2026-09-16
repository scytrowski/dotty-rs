use std::fs;
use std::path::Path;

use dotty_tasty::tasty::{
    APPLIEDTPT_TAG, APPLIEDTYPE_TAG, BLOCK_TAG, CASE_TAG, CASEDEF_TAG, ConstantValue, DEFDEF_TAG,
    DefinitionBody, DefinitionTail, EXTENSION_TAG, GIVEN_TAG, IF_TAG, INLINE_TAG, LAMBDA_TAG,
    LAMBDATPT_TAG, MATCH_TAG, MATCHTPT_TAG, OPAQUE_TAG, PARAM_TAG, REFINEDTPT_TAG, RawNode,
    RawTree, SELECTIN_TAG, StructuredNode, StructuredTree, TEMPLATE_TAG, TRAIT_TAG, TYPEBOUNDS_TAG,
    TYPEBOUNDSTPT_TAG, TYPED_TAG, TYPEDEF_TAG, TYPEPARAM_TAG, TastyFile, UNAPPLY_TAG, VALDEF_TAG,
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

fn try_wire_name(file: &TastyFile<'_>, reference: u32) -> Option<String> {
    file.names()
        .entries()
        .get(reference as usize)
        .and_then(dotty_tasty::tasty::RawName::as_utf8)
        .map(str::to_owned)
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
            try_wire_name(file, definition.name()).as_deref() == Some(expected_name)
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

fn has_modifier(tail: &[DefinitionTail<'_>], expected: u8) -> bool {
    tail.iter()
        .any(|item| matches!(item, DefinitionTail::Modifier(tag) if *tag == expected))
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

#[test]
fn bounds_fixture_has_a_type_parameter_with_an_upper_bound() {
    with_fixture("bounds/Bounds.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, DEFDEF_TAG, "accept", |structured| {
            let StructuredNode::DefDef(body) = structured else {
                panic!("accept is not a DefDef");
            };
            assert_eq!(body.parameters.len(), 2);
            assert_eq!(body.parameters[0].tag(), TYPEPARAM_TAG);
            assert_eq!(wire_name(file, body.parameters[0].name()), "A");
            assert_eq!(body.parameters[1].tag(), PARAM_TAG);
            assert_eq!(wire_name(file, body.parameters[1].name()), "value");

            let type_parameter = body.parameters[0].decode_body().unwrap();
            let RawTree::LengthNode(bounds_node) = type_parameter.type_tree else {
                panic!("type parameter A has no TypeBoundsTpt body");
            };
            assert_eq!(bounds_node.tag, TYPEBOUNDSTPT_TAG);
            let StructuredNode::TypeBounds(bounds) = bounds_node.decode_structured().unwrap()
            else {
                panic!("type parameter A body is not TypeBoundsTpt");
            };
            assert!(bounds.high.is_some());
        });
    });
}

#[test]
fn shape_fixture_is_a_trait() {
    with_fixture("bounds/Shape.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, TYPEDEF_TAG, "Shape", |structured| {
            let StructuredNode::TypeDef(DefinitionBody::TypeDef { tail, .. }) = structured else {
                panic!("Shape is not a TypeDef");
            };
            assert!(has_modifier(&tail, TRAIT_TAG));
        });
    });
}

#[test]
fn circle_fixture_has_shape_as_a_parent() {
    with_fixture("bounds/Circle.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, TYPEDEF_TAG, "Circle", |structured| {
            let StructuredNode::TypeDef(DefinitionBody::TypeDef {
                type_or_template, ..
            }) = structured
            else {
                panic!("Circle is not a TypeDef");
            };
            let RawTree::LengthNode(template_node) = type_or_template else {
                panic!("Circle has no template");
            };
            let StructuredNode::Template(template) = template_node.decode_structured().unwrap()
            else {
                panic!("Circle body is not a Template");
            };
            assert!(template.parents.iter().any(|parent| {
                parent
                    .name_refs()
                    .iter()
                    .any(|reference| wire_name(file, *reference) == "Shape")
            }));
        });
    });
}

#[test]
fn case_class_fixture_has_point_constructor_parameters() {
    with_fixture("case_class/Point.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, TYPEDEF_TAG, "Point", |structured| {
            let StructuredNode::TypeDef(DefinitionBody::TypeDef {
                type_or_template,
                tail,
                ..
            }) = structured
            else {
                panic!("Point is not a TypeDef");
            };
            assert!(has_modifier(&tail, CASE_TAG));
            let RawTree::LengthNode(template_node) = type_or_template else {
                panic!("Point has no template");
            };
            let StructuredNode::Template(template) = template_node.decode_structured().unwrap()
            else {
                panic!("Point body is not a Template");
            };
            assert_eq!(template.term_params.len(), 2);
            assert_eq!(wire_name(file, template.term_params[0].name()), "x");
            assert_eq!(wire_name(file, template.term_params[1].name()), "y");
        });
    });
}

#[test]
fn box_fixture_has_a_type_parameter() {
    with_fixture("generic/Box.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, TYPEDEF_TAG, "Box", |structured| {
            let StructuredNode::TypeDef(DefinitionBody::TypeDef {
                type_or_template, ..
            }) = structured
            else {
                panic!("Box is not a TypeDef");
            };
            let RawTree::LengthNode(template_node) = type_or_template else {
                panic!("Box has no template");
            };
            let StructuredNode::Template(template) = template_node.decode_structured().unwrap()
            else {
                panic!("Box body is not a Template");
            };
            assert_eq!(template.type_params.len(), 1);
            assert_eq!(wire_name(file, template.type_params[0].name()), "A");
            assert_eq!(template.term_params.len(), 1);
            assert_eq!(wire_name(file, template.term_params[0].name()), "value");
        });
    });
}

#[test]
fn generic_fixture_has_identity_signature() {
    with_fixture("generic/Generic.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, DEFDEF_TAG, "identity", |structured| {
            let StructuredNode::DefDef(body) = structured else {
                panic!("identity is not a DefDef");
            };
            assert_eq!(body.parameters.len(), 2);
            assert_eq!(body.parameters[0].tag(), TYPEPARAM_TAG);
            assert_eq!(wire_name(file, body.parameters[0].name()), "A");
            assert_eq!(body.parameters[1].tag(), PARAM_TAG);
            assert_eq!(wire_name(file, body.parameters[1].name()), "value");
        });
    });
}

#[test]
fn generic_fixture_has_pair_signature() {
    with_fixture("generic/Generic.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, DEFDEF_TAG, "pair", |structured| {
            let StructuredNode::DefDef(body) = structured else {
                panic!("pair is not a DefDef");
            };
            assert_eq!(body.parameters.len(), 4);
            for (parameter, expected) in body.parameters.iter().zip(["A", "B", "a", "b"]) {
                assert_eq!(wire_name(file, parameter.name()), expected);
            }
            assert_eq!(body.parameters[0].tag(), TYPEPARAM_TAG);
            assert_eq!(body.parameters[1].tag(), TYPEPARAM_TAG);
            assert_eq!(body.parameters[2].tag(), PARAM_TAG);
            assert_eq!(body.parameters[3].tag(), PARAM_TAG);
        });
    });
}

#[test]
fn given_fixture_has_a_given_display_instance() {
    with_fixture("given/Display.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, TYPEDEF_TAG, "Display", |structured| {
            let StructuredNode::TypeDef(DefinitionBody::TypeDef {
                type_or_template,
                tail,
                ..
            }) = structured
            else {
                panic!("Display is not a TypeDef");
            };
            assert!(has_modifier(&tail, TRAIT_TAG));
            let RawTree::LengthNode(template_node) = type_or_template else {
                panic!("Display has no template");
            };
            let StructuredNode::Template(template) = template_node.decode_structured().unwrap()
            else {
                panic!("Display body is not a Template");
            };
            assert_eq!(template.type_params.len(), 1);
            assert_eq!(wire_name(file, template.type_params[0].name()), "A");
        });

        assert_named_definition(
            file,
            &index,
            VALDEF_TAG,
            "given_Display_Int",
            |structured| {
                let StructuredNode::ValDef(DefinitionBody::ValDef {
                    type_tree, tail, ..
                }) = structured
                else {
                    panic!("given_Display_Int is not a ValDef");
                };
                assert!(has_modifier(&tail, GIVEN_TAG));
                assert!(matches!(type_tree, RawTree::NatAst { .. }));
            },
        );
    });
}

#[test]
fn inline_fixture_marks_method_and_parameter_inline() {
    with_fixture("inline/Inline.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, DEFDEF_TAG, "twice", |structured| {
            let StructuredNode::DefDef(body) = structured else {
                panic!("twice is not a DefDef");
            };
            assert!(has_modifier(&body.tail, INLINE_TAG));
            assert_eq!(body.parameters.len(), 1);
            assert_eq!(wire_name(file, body.parameters[0].name()), "x");
            let parameter_body = body.parameters[0].decode_body().unwrap();
            assert!(has_modifier(&parameter_body.tail, INLINE_TAG));

            let Some(RawTree::LengthNode(rhs_node)) = body.rhs else {
                panic!("twice has no RHS");
            };
            let StructuredNode::Typed(typed) = rhs_node.decode_structured().unwrap() else {
                panic!("twice RHS is not Typed");
            };
            let RawTree::LengthNode(expression_node) = typed.expression else {
                panic!("twice typed expression is not an Apply");
            };
            assert!(matches!(
                expression_node.decode_structured().unwrap(),
                StructuredNode::Apply(_)
            ));
        });
    });
}

#[test]
fn opaque_fixture_has_an_opaque_type_alias() {
    with_fixture("opaque/UserId.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, TYPEDEF_TAG, "UserId", |structured| {
            let StructuredNode::TypeDef(DefinitionBody::TypeDef { tail, .. }) = structured else {
                panic!("UserId is not a TypeDef");
            };
            assert!(has_modifier(&tail, OPAQUE_TAG));
        });
    });
}

#[test]
fn opaque_fixture_has_apply_and_extension_methods() {
    with_fixture("opaque/UserId.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, DEFDEF_TAG, "apply", |structured| {
            let StructuredNode::DefDef(body) = structured else {
                panic!("apply is not a DefDef");
            };
            assert_eq!(body.parameters.len(), 1);
            assert_eq!(wire_name(file, body.parameters[0].name()), "value");
        });
        assert_named_definition(file, &index, DEFDEF_TAG, "value", |structured| {
            let StructuredNode::DefDef(body) = structured else {
                panic!("value is not a DefDef");
            };
            assert_eq!(body.parameters.len(), 1);
            assert_eq!(wire_name(file, body.parameters[0].name()), "id");
            assert!(has_modifier(&body.tail, EXTENSION_TAG));
        });
    });
}

#[test]
fn class_fixture_has_person_constructor_parameters() {
    with_fixture("class/Person.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, TYPEDEF_TAG, "Person", |structured| {
            let StructuredNode::TypeDef(DefinitionBody::TypeDef {
                type_or_template, ..
            }) = structured
            else {
                panic!("Person is not a TypeDef");
            };
            let RawTree::LengthNode(template_node) = type_or_template else {
                panic!("Person has no template");
            };
            let StructuredNode::Template(template) = template_node.decode_structured().unwrap()
            else {
                panic!("Person body is not a Template");
            };
            assert_eq!(template.term_params.len(), 2);
            assert_eq!(wire_name(file, template.term_params[0].name()), "name");
            assert_eq!(wire_name(file, template.term_params[1].name()), "age");
        });
    });
}

#[test]
fn empty_object_fixture_has_an_object_module_value() {
    with_fixture("empty_object/EmptyObject.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, VALDEF_TAG, "EmptyObject", |structured| {
            let StructuredNode::ValDef(DefinitionBody::ValDef { rhs, .. }) = structured else {
                panic!("EmptyObject is not a ValDef");
            };
            assert!(rhs.is_some());
        });
    });
}

#[test]
fn extension_fixture_marks_squared_as_an_extension_method() {
    with_fixture("extension/Extension.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, DEFDEF_TAG, "squared", |structured| {
            let StructuredNode::DefDef(body) = structured else {
                panic!("squared is not a DefDef");
            };
            assert!(has_modifier(&body.tail, EXTENSION_TAG));
            assert_eq!(body.parameters.len(), 1);
            assert_eq!(wire_name(file, body.parameters[0].name()), "value");
        });
    });
}

#[test]
fn animal_fixture_is_a_trait_with_sound_method() {
    with_fixture("inheritance/Animal.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, TYPEDEF_TAG, "Animal", |structured| {
            let StructuredNode::TypeDef(DefinitionBody::TypeDef { tail, .. }) = structured else {
                panic!("Animal is not a TypeDef");
            };
            assert!(has_modifier(&tail, TRAIT_TAG));
        });
        assert_named_definition(file, &index, DEFDEF_TAG, "sound", |structured| {
            let StructuredNode::DefDef(body) = structured else {
                panic!("sound is not a DefDef");
            };
            assert!(body.parameters.is_empty());
        });
    });
}

#[test]
fn dog_fixture_has_animal_as_a_parent() {
    with_fixture("inheritance/Dog.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, TYPEDEF_TAG, "Dog", |structured| {
            let StructuredNode::TypeDef(DefinitionBody::TypeDef {
                type_or_template, ..
            }) = structured
            else {
                panic!("Dog is not a TypeDef");
            };
            let RawTree::LengthNode(template_node) = type_or_template else {
                panic!("Dog has no template");
            };
            let StructuredNode::Template(template) = template_node.decode_structured().unwrap()
            else {
                panic!("Dog body is not a Template");
            };
            assert!(template.parents.iter().any(|parent| {
                parent
                    .name_refs()
                    .iter()
                    .any(|reference| wire_name(file, *reference) == "Animal")
            }));
        });
    });
}

#[test]
fn inline_match_fixture_marks_classify_and_its_parameter_inline() {
    with_fixture("inline_match/InlineMatch.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, DEFDEF_TAG, "classify", |structured| {
            let StructuredNode::DefDef(body) = structured else {
                panic!("classify is not a DefDef");
            };
            assert!(has_modifier(&body.tail, INLINE_TAG));
            assert_eq!(body.parameters.len(), 1);
            assert_eq!(wire_name(file, body.parameters[0].name()), "value");
            assert!(has_modifier(
                &body.parameters[0].decode_body().unwrap().tail,
                INLINE_TAG
            ));
        });
        assert_eq!(
            index.iter().filter(|node| node.tag == CASEDEF_TAG).count(),
            2
        );
    });
}

#[test]
fn aged_fixture_is_a_trait_with_age_method() {
    with_fixture("intersection/Aged.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, TYPEDEF_TAG, "Aged", |structured| {
            let StructuredNode::TypeDef(DefinitionBody::TypeDef { tail, .. }) = structured else {
                panic!("Aged is not a TypeDef");
            };
            assert!(has_modifier(&tail, TRAIT_TAG));
        });
        assert_named_definition(file, &index, DEFDEF_TAG, "age", |structured| {
            let StructuredNode::DefDef(body) = structured else {
                panic!("age is not a DefDef");
            };
            assert!(body.parameters.is_empty());
        });
    });
}

#[test]
fn named_fixture_is_a_trait_with_name_method() {
    with_fixture("intersection/Named.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, TYPEDEF_TAG, "Named", |structured| {
            let StructuredNode::TypeDef(DefinitionBody::TypeDef { tail, .. }) = structured else {
                panic!("Named is not a TypeDef");
            };
            assert!(has_modifier(&tail, TRAIT_TAG));
        });
        assert_named_definition(file, &index, DEFDEF_TAG, "name", |structured| {
            let StructuredNode::DefDef(body) = structured else {
                panic!("name is not a DefDef");
            };
            assert!(body.parameters.is_empty());
        });
    });
}

#[test]
fn intersection_fixture_has_a_named_and_aged_parameter() {
    with_fixture("intersection/Intersection.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, DEFDEF_TAG, "describe", |structured| {
            let StructuredNode::DefDef(body) = structured else {
                panic!("describe is not a DefDef");
            };
            assert_eq!(body.parameters.len(), 1);
            assert_eq!(wire_name(file, body.parameters[0].name()), "value");
        });
    });
}

#[test]
fn match_type_fixture_has_head_match_type_alias() {
    with_fixture("match_type/MatchType.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, TYPEDEF_TAG, "Head", |structured| {
            let StructuredNode::TypeDef(DefinitionBody::TypeDef {
                type_or_template, ..
            }) = structured
            else {
                panic!("Head is not a TypeDef");
            };
            let RawTree::LengthNode(node) = type_or_template else {
                panic!("Head has no type lambda");
            };
            assert_eq!(node.tag, LAMBDATPT_TAG);
        });
    });
}

#[test]
fn match_type_fixture_has_element_match_type_alias() {
    with_fixture("match_type/MatchType.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, TYPEDEF_TAG, "Element", |structured| {
            let StructuredNode::TypeDef(DefinitionBody::TypeDef {
                type_or_template, ..
            }) = structured
            else {
                panic!("Element is not a TypeDef");
            };
            let RawTree::LengthNode(node) = type_or_template else {
                panic!("Element has no type lambda");
            };
            assert_eq!(node.tag, LAMBDATPT_TAG);
        });
    });
}

#[test]
fn path_dependent_container_has_an_abstract_element_type() {
    with_fixture("path_dependent/Container.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, TYPEDEF_TAG, "Container", |structured| {
            let StructuredNode::TypeDef(DefinitionBody::TypeDef { tail, .. }) = structured else {
                panic!("Container is not a TypeDef");
            };
            assert!(has_modifier(&tail, TRAIT_TAG));
        });
        assert_named_definition(file, &index, TYPEDEF_TAG, "Element", |structured| {
            let StructuredNode::TypeDef(DefinitionBody::TypeDef {
                type_or_template, ..
            }) = structured
            else {
                panic!("Element is not a TypeDef");
            };
            let RawTree::LengthNode(node) = type_or_template else {
                panic!("Element has no type bounds");
            };
            assert_eq!(node.tag, TYPEBOUNDSTPT_TAG);
        });
    });
}

#[test]
fn path_dependent_fixture_gets_a_container_parameter() {
    with_fixture("path_dependent/PathDependent.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, DEFDEF_TAG, "get", |structured| {
            let StructuredNode::DefDef(body) = structured else {
                panic!("get is not a DefDef");
            };
            assert_eq!(body.parameters.len(), 1);
            assert_eq!(wire_name(file, body.parameters[0].name()), "container");
        });
    });
}

#[test]
fn product_match_fixture_first_takes_a_tuple_parameter() {
    with_fixture("product_match/ProductMatch.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, DEFDEF_TAG, "first", |structured| {
            let StructuredNode::DefDef(body) = structured else {
                panic!("first is not a DefDef");
            };
            assert_eq!(body.parameters.len(), 1);
            assert_eq!(wire_name(file, body.parameters[0].name()), "value");
        });
    });
}

#[test]
fn refinement_fixture_has_a_string_service_refinement() {
    with_fixture("refinement/Refinement.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, TYPEDEF_TAG, "StringService", |structured| {
            let StructuredNode::TypeDef(DefinitionBody::TypeDef {
                type_or_template, ..
            }) = structured
            else {
                panic!("StringService is not a TypeDef");
            };
            let RawTree::LengthNode(node) = type_or_template else {
                panic!("StringService has no refinement type");
            };
            assert_eq!(node.tag, REFINEDTPT_TAG);
        });
    });
}

#[test]
fn service_fixture_has_an_abstract_result_type() {
    with_fixture("refinement/Service.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, TYPEDEF_TAG, "Service", |structured| {
            let StructuredNode::TypeDef(DefinitionBody::TypeDef { tail, .. }) = structured else {
                panic!("Service is not a TypeDef");
            };
            assert!(has_modifier(&tail, TRAIT_TAG));
        });
        assert_named_definition(file, &index, TYPEDEF_TAG, "Result", |structured| {
            let StructuredNode::TypeDef(DefinitionBody::TypeDef {
                type_or_template, ..
            }) = structured
            else {
                panic!("Result is not a TypeDef");
            };
            let RawTree::LengthNode(node) = type_or_template else {
                panic!("Result has no type bounds");
            };
            assert_eq!(node.tag, TYPEBOUNDSTPT_TAG);
        });
    });
}

#[test]
fn trait_fixture_is_a_trait_with_name_method() {
    with_fixture("trait/WithName.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, TYPEDEF_TAG, "WithName", |structured| {
            let StructuredNode::TypeDef(DefinitionBody::TypeDef { tail, .. }) = structured else {
                panic!("WithName is not a TypeDef");
            };
            assert!(has_modifier(&tail, TRAIT_TAG));
        });
        assert_named_definition(file, &index, DEFDEF_TAG, "name", |structured| {
            let StructuredNode::DefDef(body) = structured else {
                panic!("name is not a DefDef");
            };
            assert!(body.parameters.is_empty());
        });
    });
}

#[test]
fn type_lambda_fixture_has_container_alias() {
    with_fixture("type_lambda/TypeLambda.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, TYPEDEF_TAG, "Container", |structured| {
            let StructuredNode::TypeDef(DefinitionBody::TypeDef {
                type_or_template, ..
            }) = structured
            else {
                panic!("Container is not a TypeDef");
            };
            let RawTree::LengthNode(node) = type_or_template else {
                panic!("Container has no type lambda");
            };
            assert_eq!(node.tag, LAMBDATPT_TAG);
        });
    });
}

#[test]
fn type_lambda_fixture_has_either_string_alias() {
    with_fixture("type_lambda/TypeLambda.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, TYPEDEF_TAG, "EitherString", |structured| {
            let StructuredNode::TypeDef(DefinitionBody::TypeDef {
                type_or_template, ..
            }) = structured
            else {
                panic!("EitherString is not a TypeDef");
            };
            let RawTree::LengthNode(node) = type_or_template else {
                panic!("EitherString has no type lambda");
            };
            assert_eq!(node.tag, LAMBDATPT_TAG);
        });
    });
}

#[test]
fn union_fixture_has_string_or_int_alias_and_value_method() {
    with_fixture("union/Union.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, TYPEDEF_TAG, "StringOrInt", |structured| {
            let StructuredNode::TypeDef(DefinitionBody::TypeDef {
                type_or_template, ..
            }) = structured
            else {
                panic!("StringOrInt is not a TypeDef");
            };
            let RawTree::LengthNode(node) = type_or_template else {
                panic!("StringOrInt has no type tree");
            };
            assert_eq!(node.tag, APPLIEDTPT_TAG);
        });
        assert_named_definition(file, &index, DEFDEF_TAG, "value", |structured| {
            let StructuredNode::DefDef(body) = structured else {
                panic!("value is not a DefDef");
            };
            assert_eq!(body.parameters.len(), 1);
            assert_eq!(wire_name(file, body.parameters[0].name()), "flag");
        });
    });
}

#[test]
fn show_fixture_is_a_generic_trait() {
    with_fixture("using/Show.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, TYPEDEF_TAG, "Show", |structured| {
            let StructuredNode::TypeDef(DefinitionBody::TypeDef {
                type_or_template,
                tail,
                ..
            }) = structured
            else {
                panic!("Show is not a TypeDef");
            };
            assert!(has_modifier(&tail, TRAIT_TAG));
            let RawTree::LengthNode(template_node) = type_or_template else {
                panic!("Show has no template");
            };
            let StructuredNode::Template(template) = template_node.decode_structured().unwrap()
            else {
                panic!("Show body is not a Template");
            };
            assert_eq!(template.type_params.len(), 1);
            assert_eq!(wire_name(file, template.type_params[0].name()), "A");
        });
    });
}

#[test]
fn using_fixture_render_has_value_and_using_parameters() {
    with_fixture("using/Using.tasty", |file| {
        let index = file.ast_address_index().unwrap();
        assert_named_definition(file, &index, DEFDEF_TAG, "render", |structured| {
            let StructuredNode::DefDef(body) = structured else {
                panic!("render is not a DefDef");
            };
            assert_eq!(body.parameters.len(), 3);
            for (parameter, expected) in body.parameters.iter().zip(["A", "value", "show"]) {
                assert_eq!(wire_name(file, parameter.name()), expected);
            }
            assert_eq!(body.parameters[0].tag(), TYPEPARAM_TAG);
            assert_eq!(body.parameters[1].tag(), PARAM_TAG);
            assert_eq!(body.parameters[2].tag(), PARAM_TAG);
            assert!(!body.clauses.is_empty());
        });
    });
}
