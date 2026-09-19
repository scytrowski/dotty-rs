//! Pins the TASTy encoding of the `semantic/Foo.scala` fixture that the
//! symbol-entering pass is built against.
//!
//! The fixture is real Scala 3.9.0 compiler output (see
//! `tests/fixtures/semantic/generate.sh`). These tests do not exercise the
//! unpickler; they record the shape of the input so that a change in the
//! fixture, or a wrong assumption about the encoding, fails here first.

use dotty_tasty::tasty::{
    DEFDEF_TAG, DefinitionBody, DefinitionTail, LOCAL_TAG, PACKAGE_TAG, PARAM_TAG, PRIVATE_TAG,
    ParameterNode, RawName, RawTree, StructuredNode, TEMPLATE_TAG, TYPEDEF_TAG, TYPEPARAM_TAG,
    TastyFile, TemplateStructure,
};

const FOO: &[u8] = include_bytes!("fixtures/semantic/Foo.tasty");

/// Resolves a wire name reference to its text.
///
/// Name references in AST payloads *and* inside composite name entries are
/// zero-based indexes into the name table in this compiler output (for
/// example `<init>` is entry 14 and `Signed { original: 14, .. }` refers to
/// it as 14). `TastyFile::render_name` is one-based and therefore resolves
/// composite names against the wrong entries, so the fixture tests resolve
/// them here instead of relying on it.
fn name(file: &TastyFile<'_>, reference: u32) -> String {
    match file.names().entries().get(reference as usize) {
        Some(RawName::Utf8(text)) => text.clone(),
        Some(RawName::Qualified { prefix, selector }) => {
            format!("{}.{}", name(file, *prefix), name(file, *selector))
        }
        other => panic!("name reference {reference} is not a text name: {other:?}"),
    }
}

fn foo_template<'a>(file: &TastyFile<'a>) -> TemplateStructure<'a> {
    let package = file
        .asts()
        .unwrap()
        .iter()
        .find(|node| node.tag == PACKAGE_TAG)
        .expect("the fixture has a package node")
        .decode_package()
        .unwrap();
    let type_def = package
        .stats
        .iter()
        .find(|node| node.tag == TYPEDEF_TAG)
        .expect("the package declares one type definition");
    let StructuredNode::TypeDef(DefinitionBody::TypeDef {
        name: type_name,
        type_or_template: RawTree::LengthNode(template),
        ..
    }) = type_def.decode_structured().unwrap()
    else {
        panic!("the type definition has a template body");
    };
    assert_eq!(name(file, type_name), "Foo");
    assert_eq!(template.tag, TEMPLATE_TAG);
    template.decode_template_structure().unwrap()
}

fn modifiers(tail: &[DefinitionTail<'_>]) -> Vec<u8> {
    tail.iter()
        .filter_map(|entry| match entry {
            DefinitionTail::Modifier(tag) => Some(*tag),
            _ => None,
        })
        .collect()
}

#[test]
fn the_fixture_parses_as_scala_3_9_tasty() {
    TastyFile::parse_and_validate_scala_3_9(FOO).unwrap();
}

#[test]
fn the_package_path_is_a_direct_package_reference() {
    let file = TastyFile::parse_scala_3_9(FOO).unwrap();
    let package = file
        .asts()
        .unwrap()
        .iter()
        .find(|node| node.tag == PACKAGE_TAG)
        .unwrap()
        .decode_package()
        .unwrap();

    let path_name = package.path_name().expect("TERMREFpkg package path");

    // The wire name is the fully qualified package, not one segment.
    assert_eq!(
        name(&file, path_name),
        "me.cytrowski.tastyfixtures.semantic"
    );
}

#[test]
fn definitions_have_distinct_absolute_addresses() {
    let file = TastyFile::parse_scala_3_9(FOO).unwrap();
    let index = file.ast_address_index().unwrap();

    let addresses = |tag| {
        index
            .iter_nodes_with_tag(tag)
            .map(|node| node.offset)
            .collect::<Vec<_>>()
    };

    // Foo, plus A/x/bar/B/b. The constructor and the class template each
    // carry their own TYPEPARAM/PARAM nodes, so parameters appear twice.
    assert_eq!(addresses(TYPEDEF_TAG), vec![4]);
    assert_eq!(addresses(TYPEPARAM_TAG), vec![9, 49, 73]);
    assert_eq!(addresses(PARAM_TAG), vec![24, 58, 82]);
    assert_eq!(addresses(DEFDEF_TAG), vec![46, 70]);
}

#[test]
fn the_template_holds_the_class_type_parameter_and_constructor_field() {
    let file = TastyFile::parse_scala_3_9(FOO).unwrap();
    let template = foo_template(&file);

    let type_params: Vec<_> = template
        .type_params
        .iter()
        .map(|parameter| name(&file, parameter.name()))
        .collect();
    let term_params: Vec<_> = template
        .term_params
        .iter()
        .map(|parameter| name(&file, parameter.name()))
        .collect();

    assert_eq!(type_params, ["A"]);
    assert_eq!(term_params, ["x"]);
}

#[test]
fn a_class_type_parameter_is_private_local() {
    let file = TastyFile::parse_scala_3_9(FOO).unwrap();
    let template = foo_template(&file);

    let body = template.type_params[0].decode_body().unwrap();

    assert_eq!(modifiers(&body.tail), [PRIVATE_TAG, LOCAL_TAG]);
}

#[test]
fn a_val_constructor_parameter_carries_no_visibility_modifier() {
    // `val x` is the public param accessor: unlike a plain constructor
    // parameter (which is `private[this]`), it has no PRIVATE/LOCAL tail. It
    // is *not* marked FIELDACCESSOR, so it cannot be recognized by that tag.
    let file = TastyFile::parse_scala_3_9(FOO).unwrap();
    let template = foo_template(&file);

    let body = template.term_params[0].decode_body().unwrap();

    assert_eq!(modifiers(&body.tail), Vec::<u8>::new());
}

#[test]
fn the_template_stats_hold_the_constructor_and_the_method() {
    let file = TastyFile::parse_scala_3_9(FOO).unwrap();
    let template = foo_template(&file);

    let defs: Vec<_> = template
        .stats
        .iter()
        .map(|stat| {
            let RawTree::LengthNode(node) = stat else {
                panic!("template stat is a length node");
            };
            assert_eq!(node.tag, DEFDEF_TAG);
            let StructuredNode::DefDef(body) = node.decode_structured().unwrap() else {
                panic!("stat decodes as a DefDef");
            };
            body
        })
        .collect();

    let names: Vec<_> = defs.iter().map(|def| name(&file, def.name)).collect();
    assert_eq!(names, ["<init>", "bar"]);

    let parameter_names = |def: &dotty_tasty::tasty::DefDefBody<'_>| {
        def.parameters
            .iter()
            .map(|parameter| {
                let kind = match parameter {
                    ParameterNode::TypeParam { .. } => "type",
                    ParameterNode::TermParam { .. } => "term",
                };
                format!("{kind} {}", name(&file, parameter.name()))
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(parameter_names(&defs[0]), ["type A", "term x"]);
    assert_eq!(parameter_names(&defs[1]), ["type B", "term b"]);
}

/// Regression test for issue #13: the index payload of a parameter node used
/// to omit the name, so decoding it from the index read the first body byte
/// as the name.
#[test]
fn index_parameter_nodes_decode_like_definition_nodes() {
    let file = TastyFile::parse_scala_3_9(FOO).unwrap();
    let index = file.ast_address_index().unwrap();
    let template = foo_template(&file);

    let type_param = index.get(9).unwrap().decode_parameter().unwrap();
    assert_eq!(type_param.tag(), TYPEPARAM_TAG);
    assert_eq!(name(&file, type_param.name()), "A");
    assert!(type_param.decode_body().is_ok());

    // Same node as seen through the parent's structural decoding.
    let through_parent = &template.type_params[0];
    assert_eq!(type_param.name(), through_parent.name());
    assert_eq!(type_param.body(), through_parent.body());

    let term_param = index.get(24).unwrap().decode_parameter().unwrap();
    assert_eq!(term_param.tag(), PARAM_TAG);
    assert_eq!(name(&file, term_param.name()), "x");
    assert!(term_param.decode_body().is_ok());
}
