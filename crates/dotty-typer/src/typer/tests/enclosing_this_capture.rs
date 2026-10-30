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
fn combined_enclosing_this_and_method_parameter_capture_types_whole_method() {
    let source_text =
        include_str!("../../../tests/fixtures/lexical-closure-contract/CombinedCaptures.scala");
    let (parsed, mut store, mut packages, definitions, index, source) = parse_and_name(source_text);
    enter_scala_function_classes(&mut store, &mut packages, [1]);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "makeAdder");
    let method_tree = method_tree(&parsed, &index, source, method);
    let class = store.symbols.get(method).owner.unwrap();
    let (delta_tree, lambda_tree, lambda_parameter_tree, lambda_body_tree) = {
        let TreeKind::DefDef(definition) = &parsed.ast.get(method_tree).kind else {
            panic!("makeAdder source root should be a DefDef");
        };
        let delta_tree = definition.value_param_clauses[0][0];
        let TreeKind::PhaseSpecific(UntypedNode::Function(function)) = &parsed.ast.get(rhs).kind
        else {
            panic!("makeAdder RHS should be the exact source lambda");
        };
        (delta_tree, rhs, function.params[0], function.body)
    };
    let delta = index.symbol_at(source, delta_tree).unwrap();
    let (inc, _inc_rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "inc");
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );

    let typed_method = typer
        .type_method_definition(method_tree)
        .unwrap_or_else(|error| panic!("combined capture root baseline error: {error:?}"));
    assert_eq!(
        typer.source_typed_index().get(source, method_tree),
        Some(typed_method)
    );
    assert_eq!(typer.store().symbols.get(method).owner, Some(class));
    assert_eq!(typer.store().symbols.get(class).kind, SymbolKind::Class);
    assert_eq!(typer.store().symbols.get(inc).owner, Some(class));
    assert_eq!(typer.store().symbols.get(inc).kind, SymbolKind::Method);
    let SymbolInfo::Complete(inc_type) = typer.store().symbols.info(inc) else {
        panic!("Calculator.inc should keep its canonical complete signature");
    };
    let Type::Method(inc_method_type) = typer.store().types.get(*inc_type) else {
        panic!("Calculator.inc should retain a method signature");
    };
    assert_eq!(inc_method_type.params.len(), 1);
    assert_eq!(inc_method_type.params[0].ty, definitions.int);
    assert_eq!(inc_method_type.result, definitions.int);
    assert_eq!(typer.store().symbols.get(delta).owner, Some(method));
    assert_eq!(typer.store().symbols.get(delta).kind, SymbolKind::Parameter);
    let TreeKind::DefDef(typed_definition) = &typer.typed_ast().get(typed_method).kind else {
        panic!("makeAdder should be retained as an owned typed DefDef");
    };
    assert!(typed_definition.rhs.is_some());
    let SymbolInfo::Complete(method_type) = typer.store().symbols.info(method) else {
        panic!("makeAdder symbol should have its complete source signature");
    };
    let Type::Method(source_method_type) = typer.store().types.get(*method_type) else {
        panic!("makeAdder signature should be a Method type");
    };
    assert_eq!(source_method_type.params.len(), 1);
    assert_eq!(source_method_type.params[0].ty, definitions.int);
    let declared_result = typer.store().types.get(source_method_type.result);
    let typed_rhs_type = typer
        .store()
        .types
        .get(typer.typed_ast().get(typed_definition.rhs.unwrap()).ty);
    let (
        Type::Applied {
            tycon: declared_tycon,
            args: declared_args,
        },
        Type::Applied {
            tycon: rhs_tycon,
            args: rhs_args,
        },
    ) = (declared_result, typed_rhs_type)
    else {
        panic!("method result and closure should both be Function1 applications");
    };
    assert_eq!(declared_args, rhs_args);
    assert_eq!(declared_args, &[definitions.int, definitions.int]);
    let function_symbol = |tycon: &TypeId| match typer.store().types.get(*tycon) {
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } => *symbol,
        ty => panic!("Function1 constructor should retain its canonical class symbol: {ty:?}"),
    };
    let function1 = function_symbol(declared_tycon);
    assert_eq!(function1, function_symbol(rhs_tycon));
    assert_eq!(
        typer
            .store()
            .names
            .resolve(typer.store().symbols.get(function1).name.text()),
        "Function1"
    );
    let typed_rhs = typer.source_typed_index().get(source, rhs).unwrap();
    let TreeKind::Closure(closure) = &typer.typed_ast().get(typed_rhs).kind else {
        panic!("combined method RHS should be a closure");
    };
    assert_eq!(
        closure.env.len(),
        2,
        "implicit receiver and delta are captured once"
    );
    let this_capture = closure.env[0];
    assert!(matches!(
        typer.typed_ast().get(this_capture).kind,
        TreeKind::This(_)
    ));
    assert!(matches!(
        typer.store().types.get(typer.typed_ast().get(this_capture).ty),
        Type::ThisType { class: capture_class } if *capture_class == class
    ));
    let delta_capture = closure.env[1];
    assert!(matches!(
        typer.typed_ast().get(delta_capture).kind,
        TreeKind::Ident(_)
    ));
    assert!(matches!(
        typer.store().types.get(typer.typed_ast().get(delta_capture).ty),
        Type::TermRef { target: TermRefTarget::Symbol(captured), .. } if *captured == delta
    ));
    let lambda_method = typer
        .function_literal_method_at(source, lambda_tree)
        .unwrap();
    assert_eq!(typer.store().symbols.get(lambda_method).owner, Some(method));
    let typed_lambda_definition = typer
        .function_literal_definition_at(source, lambda_tree)
        .unwrap();
    let TreeKind::DefDef(lambda_definition) = &typer.typed_ast().get(typed_lambda_definition).kind
    else {
        panic!("synthetic lambda method should have a typed DefDef");
    };
    let typed_lambda_body = typer
        .source_typed_index()
        .get(source, lambda_body_tree)
        .expect("source lambda body should retain its typed mapping");
    assert_eq!(lambda_definition.rhs, Some(typed_lambda_body));
    let SymbolInfo::Complete(lambda_type) = typer.store().symbols.info(lambda_method) else {
        panic!("synthetic lambda method should have a complete signature");
    };
    let Type::Method(lambda_signature) = typer.store().types.get(*lambda_type) else {
        panic!("synthetic lambda signature should be a method type");
    };
    assert_eq!(lambda_signature.params.len(), 1);
    assert_eq!(lambda_signature.params[0].ty, definitions.int);
    assert_eq!(lambda_signature.result, definitions.int);
    let lambda_parameter = typer
        .function_literal_parameter_symbol_at(source, lambda_parameter_tree)
        .expect("lambda's source parameter should map to its synthetic method parameter");
    assert_eq!(
        typer.store().symbols.get(lambda_parameter).owner,
        Some(lambda_method)
    );
    assert_eq!(
        typer.store().symbols.get(lambda_parameter).kind,
        SymbolKind::Parameter
    );
    assert!(
        typer
            .source_typed_index()
            .get(source, lambda_parameter_tree)
            .is_some()
    );
    let mut found_inc_use = false;
    let mut found_delta_use = false;
    let mut found_lambda_x_use = false;
    for (source_tree, node) in parsed.ast.iter() {
        let TreeKind::Ident(identifier) = &node.kind else {
            continue;
        };
        let name = typer.store().names.resolve(identifier.name.text());
        let Some(typed) = typer.source_typed_index().get(source, source_tree) else {
            continue;
        };
        let target = match typer.store().types.get(typer.typed_ast().get(typed).ty) {
            Type::TermRef {
                target: TermRefTarget::Symbol(target),
                ..
            } => Some(*target),
            _ => None,
        };
        if name == "inc" && target == Some(inc) {
            found_inc_use = true;
        } else if name == "delta" && target == Some(delta) {
            assert_eq!(
                typed, delta_capture,
                "capture keeps the original source-use tree"
            );
            found_delta_use = true;
        } else if name == "x" && target == Some(lambda_parameter) {
            found_lambda_x_use = true;
        }
    }
    assert!(
        found_inc_use,
        "unqualified inc use must resolve to Calculator.inc"
    );
    assert!(
        found_delta_use,
        "delta use must retain its source-to-typed mapping"
    );
    assert!(
        found_lambda_x_use,
        "x must resolve to the lambda's own parameter"
    );
}

#[test]
fn combined_capture_rejects_wrong_member_argument_and_function_result() {
    fn assert_rejected(source_text: &str, expected: fn(&TyperError) -> bool) {
        let (parsed, mut store, mut packages, definitions, index, source) =
            parse_and_name(source_text);
        enter_scala_function_classes(&mut store, &mut packages, [1]);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "makeAdder");
        let method_tree = method_tree(&parsed, &index, source, method);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let error = typer.type_method_definition(method_tree).unwrap_err();
        assert!(
            expected(&error),
            "unexpected combined-capture error: {error:?}"
        );
        assert_eq!(typer.source_typed_index().get(source, method_tree), None);
        assert_eq!(typer.source_typed_index().get(source, rhs), None);
        assert_eq!(typer.function_literal_method_at(source, rhs), None);
    }

    let wrong_argument = concat!(
        "class Calculator { ",
        "def inc(x: Int): Int = x + 1; ",
        "def makeAdder(delta: Int): Int => Int = x => inc(true) + delta ",
        "}",
    );
    assert_rejected(wrong_argument, |error| {
        matches!(error, TyperError::ApplicationArgumentTypeMismatch { .. })
    });

    let wrong_result = concat!(
        "class Calculator { ",
        "def inc(x: Int): Int = x + 1; ",
        "def makeAdder(delta: Int): Int => Boolean = x => inc(x) + delta ",
        "}",
    );
    assert_rejected(wrong_result, |error| {
        matches!(
            error,
            TyperError::ExpectedExpressionTypeMismatch { .. }
                | TyperError::ExpectedExpressionConformanceUnsupported { .. }
        )
    });
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
