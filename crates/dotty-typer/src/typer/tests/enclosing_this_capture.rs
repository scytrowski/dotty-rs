use super::*;

fn method_tree(
    parsed: &dotty_parser::ParseResult,
    index: &SourceSemanticIndex,
    source: SourceId,
    method: SymbolId,
) -> TreeId<Untyped> {
    parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            (index.symbol_at(source, tree) == Some(method)
                && matches!(node.kind, TreeKind::DefDef(_)))
            .then_some(tree)
        })
        .expect("source method should have a DefDef")
}

#[test]
fn ordinary_function1_captures_canonical_enclosing_class_and_trait_this() {
    let source_text = concat!(
        "class Outer { ",
        "def self: Int => Outer = _ => this; ",
        "def explicit: Int => Outer = x => Outer.this; ",
        "def mixed(delta: Int): Int => Outer = x => if x + delta > 0 then this else Outer.this ",
        "}; ",
        "trait OuterTrait { def selfTrait: Int => OuterTrait = _ => this }",
    );
    let (parsed, mut store, mut packages, definitions, index, source) = parse_and_name(source_text);
    enter_scala_function_classes(&mut store, &mut packages, [1]);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );

    for (name, expected_kind) in [
        ("self", SymbolKind::Class),
        ("explicit", SymbolKind::Class),
        ("mixed", SymbolKind::Class),
        ("selfTrait", SymbolKind::Trait),
    ] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let method_tree = method_tree(&parsed, &index, source, method);
        let typed_method = typer
            .type_method_definition(method_tree)
            .expect("whole source method with lexical this should type");
        assert_eq!(
            typer.source_typed_index().get(source, method_tree),
            Some(typed_method)
        );
        let typed_rhs = typer
            .source_typed_index()
            .get(source, rhs)
            .expect("source RHS should map to a typed tree");
        let TreeKind::Closure(closure) = &typer.typed_ast().get(typed_rhs).kind else {
            panic!("{name} RHS should be a closure");
        };
        assert_eq!(
            closure.env.len(),
            if name == "mixed" { 2 } else { 1 },
            "{name} should deduplicate this and preserve other captures"
        );
        if name == "mixed" {
            assert!(matches!(
                typer.typed_ast().get(closure.env[0]).kind,
                TreeKind::Ident(_)
            ));
            assert!(matches!(
                typer.store().types.get(typer.typed_ast().get(closure.env[0]).ty),
                Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                    if typer.store().symbols.get(*symbol).kind == SymbolKind::Parameter
            ));
        }
        let captured = *closure.env.last().unwrap();
        assert!(matches!(
            typer.typed_ast().get(captured).kind,
            TreeKind::This(dotty_core::ast::This { qual: _ })
        ));
        let Type::ThisType { class } = typer.store().types.get(typer.typed_ast().get(captured).ty)
        else {
            panic!("capture environment must retain canonical ThisType");
        };
        assert_eq!(typer.store().symbols.get(*class).kind, expected_kind);
        let Type::Applied { args, .. } =
            typer.store().types.get(typer.typed_ast().get(typed_rhs).ty)
        else {
            panic!("closure should retain its expected Function1 type");
        };
        assert!(matches!(
            typer.store().types.get(*args.last().unwrap()),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == *class
        ));
        let lambda_method = typer.function_literal_method_at(source, rhs).unwrap();
        assert!(matches!(
            typer.store().symbols.info(lambda_method),
            SymbolInfo::Complete(method_type)
                if matches!(typer.store().types.get(*method_type), Type::Method(method) if method.result == *args.last().unwrap())
        ));
        assert!(parsed.ast.iter().any(|(source_tree, node)| {
            matches!(node.kind, TreeKind::This(_))
                && typer.source_typed_index().get(source, source_tree) == Some(captured)
        }));
        let class_name = typer.store().symbols.get(*class).name;
        assert_eq!(
            typer.store().names.resolve(class_name.text()),
            if name == "selfTrait" {
                "OuterTrait"
            } else {
                "Outer"
            }
        );
        let lambda_method = typer.function_literal_method_at(source, rhs).unwrap();
        assert_eq!(typer.store().symbols.get(lambda_method).owner, Some(method));
        let TreeKind::DefDef(lambda) = &typer
            .typed_ast()
            .get(typer.function_literal_definition_at(source, rhs).unwrap())
            .kind
        else {
            panic!("synthetic lambda method should remain independently owned");
        };
        assert!(lambda.rhs.is_some());
    }
}

#[test]
fn explicit_this_capture_deduplicates_and_rolls_back_failed_methods() {
    let source_text = concat!(
        "class Outer { ",
        "def repeated: Int => Outer = _ => if true then this else Outer.this; ",
        "def mismatch: Int => Int = _ => this ",
        "}",
    );
    let (parsed, mut store, mut packages, definitions, index, source) = parse_and_name(source_text);
    enter_scala_function_classes(&mut store, &mut packages, [1]);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let (repeated, repeated_rhs) =
        method_definition_and_rhs(&parsed, typer.store(), &index, source, "repeated");
    let repeated_tree = method_tree(&parsed, &index, source, repeated);
    typer.type_method_definition(repeated_tree).unwrap();
    let typed_rhs = typer
        .source_typed_index()
        .get(source, repeated_rhs)
        .unwrap();
    let TreeKind::Closure(closure) = &typer.typed_ast().get(typed_rhs).kind else {
        panic!("repeated-this method should remain a closure");
    };
    assert_eq!(closure.env.len(), 1);

    let (mismatch, mismatch_rhs) =
        method_definition_and_rhs(&parsed, typer.store(), &index, source, "mismatch");
    let mismatch_tree = method_tree(&parsed, &index, source, mismatch);
    assert!(matches!(
        typer.type_method_definition(mismatch_tree),
        Err(TyperError::ExpectedExpressionTypeMismatch { .. })
    ));
    assert_eq!(typer.source_typed_index().get(source, mismatch_tree), None);
    assert_eq!(typer.source_typed_index().get(source, mismatch_rhs), None);
    assert_eq!(typer.function_literal_method_at(source, mismatch_rhs), None);
    assert!(typer.type_method_definition(mismatch_tree).is_err());
    assert_eq!(typer.function_literal_method_at(source, mismatch_rhs), None);
}

#[test]
fn lexical_this_rejects_module_receiver_and_non_enclosing_qualifier() {
    let source_text = concat!(
        "object Module { def self: Int => Module.type = _ => this; ",
        "class Nested { def outer: Int => Module.type = _ => Module.this } }; ",
        "class Outer { def wrong: Int => Outer = _ => Other.this }; ",
        "class Other",
    );
    let (parsed, mut store, mut packages, definitions, index, source) = parse_and_name(source_text);
    enter_scala_function_classes(&mut store, &mut packages, [1]);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    for name in ["self", "outer"] {
        let (module_method, _) =
            method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let module_tree = method_tree(&parsed, &index, source, module_method);
        let result = typer.type_method_definition(module_tree);
        if name == "self" {
            assert!(matches!(
                result,
                Err(TyperError::FunctionLiteralCaptureUnsupported { symbol: None, .. })
            ));
        } else {
            assert!(matches!(
                result,
                Err(TyperError::ThisOwnerNotEnclosing { .. })
            ));
        }
    }
    let (wrong, _) = method_definition_and_rhs(&parsed, typer.store(), &index, source, "wrong");
    let wrong_tree = method_tree(&parsed, &index, source, wrong);
    assert!(matches!(
        typer.type_method_definition(wrong_tree),
        Err(TyperError::ThisOwnerNotEnclosing { .. })
    ));
}
