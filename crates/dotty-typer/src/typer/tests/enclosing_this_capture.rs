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
fn ordinary_function1_captures_implicit_same_owner_members() {
    let source_text = concat!(
        "object Helpers { def id(x: Int): Int = x }; ",
        "class Calculator { ",
        "val bias: Int = 2; ",
        "def adjust(x: Int): Int = x + bias; ",
        "def lifted: Int => Int = x => adjust(x); ",
        "def explicit: Int => Int = x => this.adjust(x); ",
        "def withBias: Int => Int = x => x + bias; ",
        "def shadowed: Int => Int = adjust => adjust; ",
        "def objectCall: Int => Int = x => Helpers.id(x) ",
        "}; ",
        "trait Adapter { def apply(x: Int): Int; def lifted: Int => Int = x => apply(x) }",
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

    for name in ["lifted", "explicit", "withBias"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let method_tree = method_tree(&parsed, &index, source, method);
        typer
            .type_method_definition(method_tree)
            .unwrap_or_else(|error| panic!("Calculator.{name} should type: {error:?}"));
        let typed_rhs = typer.source_typed_index().get(source, rhs).unwrap();
        let TreeKind::Closure(closure) = &typer.typed_ast().get(typed_rhs).kind else {
            panic!("Calculator.{name} should produce a closure");
        };
        assert_eq!(closure.env.len(), 1, "Calculator.{name} captures one this");
        let captured = typer.typed_ast().get(closure.env[0]);
        assert!(matches!(captured.kind, TreeKind::This(_)));
        assert!(matches!(
            typer.store().types.get(captured.ty),
            Type::ThisType { class } if typer.store().names.resolve(typer.store().symbols.get(*class).name.text()) == "Calculator"
        ));
    }

    let (adjust, _) = method_definition_and_rhs(&parsed, typer.store(), &index, source, "adjust");
    let call_reference = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            matches!(&node.kind, TreeKind::Ident(ident) if typer.store().names.resolve(ident.name.text()) == "adjust")
                .then_some(tree)
        })
        .unwrap();
    let typed_reference = typer
        .source_typed_index()
        .get(source, call_reference)
        .unwrap();
    assert!(matches!(
        typer.store().types.get(typer.typed_ast().get(typed_reference).ty),
        Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == adjust
    ));
    let explicit_reference = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            matches!(&node.kind, TreeKind::Select(selection)
                if typer.store().names.resolve(selection.name.text()) == "adjust"
                    && matches!(parsed.ast.get(selection.qualifier).kind, TreeKind::This(_)))
            .then_some(tree)
        })
        .unwrap();
    let typed_explicit = typer
        .source_typed_index()
        .get(source, explicit_reference)
        .unwrap();
    assert!(matches!(
        typer.store().types.get(typer.typed_ast().get(typed_explicit).ty),
        Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == adjust
    ));
    let call_tree = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            matches!(&node.kind, TreeKind::Apply(application)
                if matches!(parsed.ast.get(application.function).kind, TreeKind::Ident(ident)
                    if typer.store().names.resolve(ident.name.text()) == "adjust"))
            .then_some(tree)
        })
        .unwrap();
    let typed_call = typer.source_typed_index().get(source, call_tree).unwrap();
    let Type::Method(adjust_signature) =
        typer
            .store()
            .types
            .get(match typer.store().symbols.info(adjust) {
                SymbolInfo::Complete(ty) => *ty,
                _ => panic!("adjust signature should be completed by the call"),
            })
    else {
        panic!("adjust should retain its source method signature");
    };
    assert_eq!(
        typer.typed_ast().get(typed_call).ty,
        adjust_signature.result
    );

    let bias_reference = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            (matches!(&node.kind, TreeKind::Ident(ident)
                if typer.store().names.resolve(ident.name.text()) == "bias")
                && typer.source_typed_index().get(source, tree).is_some())
            .then_some(tree)
        })
        .unwrap();
    let typed_bias = typer
        .source_typed_index()
        .get(source, bias_reference)
        .unwrap();
    let Type::TermRef {
        target: TermRefTarget::Symbol(bias),
        ..
    } = typer
        .store()
        .types
        .get(typer.typed_ast().get(typed_bias).ty)
    else {
        panic!("immutable bias read must retain its canonical field symbol");
    };
    assert_eq!(typer.store().symbols.get(*bias).kind, SymbolKind::Field);
    assert_eq!(
        typer.store().symbols.get(*bias).owner,
        typer.store().symbols.get(adjust).owner
    );

    let (shadowed, rhs) =
        method_definition_and_rhs(&parsed, typer.store(), &index, source, "shadowed");
    typer
        .type_method_definition(method_tree(&parsed, &index, source, shadowed))
        .unwrap();
    let typed_rhs = typer.source_typed_index().get(source, rhs).unwrap();
    let TreeKind::Closure(closure) = &typer.typed_ast().get(typed_rhs).kind else {
        panic!("shadowed lambda should produce a closure");
    };
    assert!(
        closure.env.is_empty(),
        "a lambda parameter shadows the member"
    );

    let (object_call, rhs) =
        method_definition_and_rhs(&parsed, typer.store(), &index, source, "objectCall");
    typer
        .type_method_definition(method_tree(&parsed, &index, source, object_call))
        .unwrap();
    let typed_rhs = typer.source_typed_index().get(source, rhs).unwrap();
    let TreeKind::Closure(closure) = &typer.typed_ast().get(typed_rhs).kind else {
        panic!("object call lambda should produce a closure");
    };
    assert!(
        closure.env.is_empty(),
        "object members do not capture enclosing this"
    );

    let trait_source = concat!(
        "trait Adapter { ",
        "def apply(x: Int): Int; ",
        "def lifted: Int => Int = x => apply(x); ",
        "}",
    );
    let (parsed, mut store, mut packages, definitions, index, source) =
        parse_and_name(trait_source);
    enter_scala_function_classes(&mut store, &mut packages, [1]);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let (lifted, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, "lifted");
    typer
        .type_method_definition(method_tree(&parsed, &index, source, lifted))
        .unwrap();
    let typed_rhs = typer.source_typed_index().get(source, rhs).unwrap();
    let TreeKind::Closure(closure) = &typer.typed_ast().get(typed_rhs).kind else {
        panic!("Adapter.lifted should produce a closure");
    };
    assert_eq!(closure.env.len(), 1);
    assert!(matches!(
        typer.store().types.get(typer.typed_ast().get(closure.env[0]).ty),
        Type::ThisType { class } if typer.store().symbols.get(*class).kind == SymbolKind::Trait
    ));
}

#[test]
fn overloaded_same_owner_calls_capture_the_canonical_receiver_for_both_syntaxes() {
    let source_text = concat!(
        "class Calculator { ",
        "def convert(x: Int): Int = x + 1; ",
        "def convert(x: Boolean): Int = if x then 1 else 0; ",
        "def lifted: Int => Int = x => convert(x); ",
        "def explicit: Int => Int = x => this.convert(x) ",
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

    let mut selected_symbols = Vec::new();
    for name in ["lifted", "explicit"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        typer
            .type_method_definition(method_tree(&parsed, &index, source, method))
            .unwrap_or_else(|error| panic!("Calculator.{name} should type: {error:?}"));
        let typed_rhs = typer.source_typed_index().get(source, rhs).unwrap();
        let TreeKind::Closure(closure) = &typer.typed_ast().get(typed_rhs).kind else {
            panic!("Calculator.{name} should produce a closure");
        };
        assert_eq!(closure.env.len(), 1, "Calculator.{name} captures one this");
        let captured = typer.typed_ast().get(closure.env[0]);
        assert!(matches!(captured.kind, TreeKind::This(_)));
        assert!(matches!(
            typer.store().types.get(captured.ty),
            Type::ThisType { class }
                if typer.store().names.resolve(typer.store().symbols.get(*class).name.text()) == "Calculator"
        ));

        let reference = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let selected = match (&node.kind, name) {
                    (TreeKind::Ident(ident), "lifted") => {
                        typer.store().names.resolve(ident.name.text()) == "convert"
                    }
                    (TreeKind::Select(selection), "explicit") => {
                        typer.store().names.resolve(selection.name.text()) == "convert"
                            && matches!(parsed.ast.get(selection.qualifier).kind, TreeKind::This(_))
                    }
                    _ => false,
                };
                (selected && typer.source_typed_index().get(source, tree).is_some()).then_some(tree)
            })
            .unwrap_or_else(|| panic!("Calculator.{name} should map its selected overload"));
        let typed_reference = typer.source_typed_index().get(source, reference).unwrap();
        let Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        } = typer
            .store()
            .types
            .get(typer.typed_ast().get(typed_reference).ty)
        else {
            panic!("Calculator.{name} should retain the selected overload symbol");
        };
        selected_symbols.push(*symbol);
    }

    assert_eq!(selected_symbols[0], selected_symbols[1]);
    let selected = selected_symbols[0];
    assert_eq!(typer.store().symbols.get(selected).kind, SymbolKind::Method);
    let SymbolInfo::Complete(signature) = typer.store().symbols.get(selected).info else {
        panic!("selected Int overload should have a complete signature");
    };
    let Type::Method(signature) = typer.store().types.get(signature) else {
        panic!("selected Int overload should retain its method type");
    };
    assert!(matches!(
        typer.store().types.get(signature.params[0].ty),
        Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }
            if typer.store().names.resolve(typer.store().symbols.get(*symbol).name.text()) == "Int"
    ));
}

#[test]
fn same_owner_member_capture_fails_closed_and_rolls_back_rejected_lambdas() {
    let source_text = concat!(
        "class Base { def inherited(x: Int): Int = x }; ",
        "class Calculator extends Base { ",
        "var state: Int = 0; ",
        "def adjust(x: Int): Int = x; ",
        "def inheritedUse: Int => Int = x => inherited(x); ",
        "def wrongArgument: Int => Int = x => adjust(true); ",
        "def update: Int => Int = x => { state = x; x } ",
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

    for name in ["inheritedUse", "wrongArgument", "update"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let result = typer.type_method_definition(method_tree(&parsed, &index, source, method));
        assert!(
            result.is_err(),
            "{name} must be rejected by the bounded capture path"
        );
        assert_eq!(typer.source_typed_index().get(source, rhs), None);
        assert_eq!(typer.function_literal_method_at(source, rhs), None);
        if name == "inheritedUse" {
            assert!(
                matches!(result, Err(TyperError::TermNameNotFound { .. })),
                "inherited access should remain outside lookup scope, got {result:?}"
            );
        } else if name == "wrongArgument" {
            assert!(matches!(
                result,
                Err(TyperError::ApplicationArgumentTypeMismatch { .. })
            ));
        } else if name == "update" {
            assert!(matches!(
                result,
                Err(TyperError::FunctionLiteralCaptureUnsupported {
                    symbol: Some(_),
                    ..
                })
            ));
        }
    }

    let (valid, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, "adjust");
    let valid_tree = method_tree(&parsed, &index, source, valid);
    // A non-lambda method remains typeable after rejected lambda candidates.
    assert!(typer.type_method_definition(valid_tree).is_ok());
    assert!(typer.source_typed_index().get(source, rhs).is_some());
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
