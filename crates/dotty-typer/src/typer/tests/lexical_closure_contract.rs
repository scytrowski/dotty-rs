use super::*;

fn source_function_literal(
    parsed: &dotty_parser::ParseResult,
    rhs: TreeId<Untyped>,
) -> TreeId<Untyped> {
    match &parsed.ast.get(rhs).kind {
        TreeKind::PhaseSpecific(UntypedNode::Function(_)) => rhs,
        TreeKind::Block(block) => {
            assert!(matches!(
                parsed.ast.get(block.expr).kind,
                TreeKind::PhaseSpecific(UntypedNode::Function(_))
            ));
            block.expr
        }
        other => panic!(
            "method RHS should be a function literal or a block ending in one, got {other:?}"
        ),
    }
}

#[test]
fn lexical_closure_contract_records_whole_method_baseline() {
    let source_text =
        include_str!("../../../tests/fixtures/lexical-closure-contract/RustBaseline.scala");
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
    let method_tree = |owner| {
        parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                (index.symbol_at(source, tree) == Some(owner)
                    && matches!(node.kind, TreeKind::DefDef(_)))
                .then_some(tree)
            })
            .unwrap()
    };
    let assert_error_span = |tree_index: u32, expected_start: usize, ident: bool| {
        let (tree, node) = parsed
            .ast
            .iter()
            .find(|(tree, _)| tree.index() == tree_index)
            .expect("first-error tree index must resolve in the source arena");
        assert_eq!(tree.index(), tree_index);
        let span = node
            .position
            .expect("first-error source tree retains its span");
        assert_eq!(span.source(), source);
        assert_eq!(span.span().range().start() as usize, expected_start);
        if ident {
            assert!(matches!(node.kind, TreeKind::Ident(_)));
        }
    };

    let (identity, identity_rhs) =
        method_definition_and_rhs(&parsed, typer.store(), &index, source, "identity");
    let identity_tree = method_tree(identity);
    let typed_identity = typer.type_method_definition(identity_tree).unwrap();
    assert!(matches!(
        typer.typed_ast().get(typed_identity).kind,
        TreeKind::DefDef(_)
    ));
    assert_eq!(
        typer.source_typed_index().get(source, identity_tree),
        Some(typed_identity)
    );
    assert!(matches!(
        typer
            .typed_ast()
            .get(
                typer
                    .source_typed_index()
                    .get(source, identity_rhs)
                    .unwrap()
            )
            .kind,
        TreeKind::Closure(_)
    ));
    assert!(matches!(
        typer.store().symbols.get(identity).info,
        SymbolInfo::Complete(_)
    ));
    assert!(
        typer
            .function_literal_method_at(source, identity_rhs)
            .is_some()
    );

    let (shadow, shadow_rhs) =
        method_definition_and_rhs(&parsed, typer.store(), &index, source, "shadow");
    let shadow_tree = method_tree(shadow);
    let typed_shadow = typer.type_method_definition(shadow_tree).unwrap();
    assert_eq!(
        typer.source_typed_index().get(source, shadow_tree),
        Some(typed_shadow)
    );
    let TreeKind::PhaseSpecific(UntypedNode::Function(shadow_function)) =
        &parsed.ast.get(shadow_rhs).kind
    else {
        panic!("shadow method should end in a lambda");
    };
    let shadow_parameter = typer
        .function_literal_parameter_symbol_at(source, shadow_function.params[0])
        .unwrap();
    let shadow_lambda_method = typer
        .function_literal_method_at(source, shadow_rhs)
        .unwrap();
    assert_eq!(
        typer.store().symbols.get(shadow_parameter).owner,
        Some(shadow_lambda_method)
    );
    assert_ne!(
        typer.store().symbols.get(shadow_parameter).owner,
        Some(shadow)
    );
    let typed_shadow_body = typer
        .source_typed_index()
        .get(source, shadow_function.body)
        .unwrap();
    let Type::TermRef {
        target: TermRefTarget::Symbol(body_symbol),
        ..
    } = typer
        .store()
        .types
        .get(typer.typed_ast().get(typed_shadow_body).ty)
    else {
        panic!("shadowed body reference should retain a canonical term symbol");
    };
    assert_eq!(*body_symbol, shadow_parameter);

    let (parameter, parameter_rhs) =
        method_definition_and_rhs(&parsed, typer.store(), &index, source, "parameter");
    let parameter_tree = method_tree(parameter);
    let parameter_error = typer.type_method_definition(parameter_tree).unwrap_err();
    let parameter_symbol = match parameter_error {
        TyperError::FunctionLiteralCaptureUnsupported {
            symbol: Some(symbol),
            tree_index,
            ..
        } => {
            assert_error_span(tree_index, source_text.find("x + delta").unwrap() + 4, true);
            symbol
        }
        other => panic!("expected outer parameter capture baseline, got {other:?}"),
    };
    assert_eq!(
        typer.store().symbols.get(parameter_symbol).owner,
        Some(parameter)
    );
    let TreeKind::DefDef(parameter_definition) = &parsed.ast.get(parameter_tree).kind else {
        panic!("parameter capture receipt must refer to a source method");
    };
    let source_parameter = parameter_definition.value_param_clauses[0][0];
    assert_eq!(
        index.symbol_at(source, source_parameter),
        Some(parameter_symbol)
    );
    assert_eq!(
        typer
            .store()
            .names
            .resolve(typer.store().symbols.get(parameter_symbol).name.text()),
        "delta"
    );
    assert_eq!(typer.source_typed_index().get(source, parameter_tree), None);
    assert_eq!(typer.source_typed_index().get(source, parameter_rhs), None);
    assert_eq!(
        typer.function_literal_method_at(source, parameter_rhs),
        None
    );
    let retry = typer.type_method_definition(parameter_tree).unwrap_err();
    assert!(
        matches!(retry, TyperError::FunctionLiteralCaptureUnsupported { symbol: Some(symbol), .. } if symbol == parameter_symbol)
    );
    assert_eq!(typer.source_typed_index().get(source, parameter_rhs), None);
    assert_eq!(
        typer.function_literal_method_at(source, parameter_rhs),
        None
    );

    let (local, local_rhs) =
        method_definition_and_rhs(&parsed, typer.store(), &index, source, "local");
    let local_function = source_function_literal(&parsed, local_rhs);
    let local_tree = method_tree(local);
    let local_start = source_text.find("def local").unwrap();
    let local_capture_start =
        local_start + source_text[local_start..].find("x + delta").unwrap() + 4;
    match typer.type_method_definition(local_tree) {
        Err(TyperError::FunctionLiteralCaptureUnsupported {
            symbol: Some(_),
            tree_index,
            ..
        }) => assert_error_span(tree_index, local_capture_start, true),
        other => panic!("expected immutable local capture baseline, got {other:?}"),
    }
    assert_eq!(typer.source_typed_index().get(source, local_tree), None);
    assert_eq!(typer.source_typed_index().get(source, local_rhs), None);
    assert_eq!(typer.source_typed_index().get(source, local_function), None);
    assert_eq!(
        typer.function_literal_method_at(source, local_function),
        None
    );

    let (mutable, mutable_rhs) =
        method_definition_and_rhs(&parsed, typer.store(), &index, source, "mutable");
    let mutable_tree = method_tree(mutable);
    let mutable_start = source_text.find("def mutable").unwrap();
    let mutable_capture_start =
        mutable_start + source_text[mutable_start..].find("x + delta").unwrap() + 4;
    match typer.type_method_definition(mutable_tree) {
        Err(TyperError::FunctionLiteralCaptureUnsupported {
            symbol: Some(_),
            tree_index,
            ..
        }) => assert_error_span(tree_index, mutable_capture_start, true),
        other => panic!("expected deferred mutable capture baseline, got {other:?}"),
    }
    assert_eq!(typer.source_typed_index().get(source, mutable_tree), None);
    assert_eq!(typer.source_typed_index().get(source, mutable_rhs), None);

    let (unknown, _) = method_definition_and_rhs(&parsed, typer.store(), &index, source, "unknown");
    match typer.type_method_definition(method_tree(unknown)) {
        Err(TyperError::TermNameNotFound { tree_index, .. }) => {
            assert_error_span(tree_index, source_text.find("missing").unwrap(), true)
        }
        other => panic!("expected unresolved-name control, got {other:?}"),
    }

    let (explicit_this, explicit_this_rhs) =
        method_definition_and_rhs(&parsed, typer.store(), &index, source, "explicitThis");
    let explicit_this_tree = method_tree(explicit_this);
    match typer.type_method_definition(explicit_this_tree) {
        Err(TyperError::FunctionLiteralCaptureUnsupported {
            symbol: None,
            tree_index,
            ..
        }) => assert_error_span(tree_index, source_text.find("=> this").unwrap() + 3, false),
        other => panic!("expected explicit this capture baseline, got {other:?}"),
    }
    assert_eq!(
        typer.source_typed_index().get(source, explicit_this_tree),
        None
    );
    assert_eq!(
        typer.source_typed_index().get(source, explicit_this_rhs),
        None
    );

    let (implicit_member, implicit_member_rhs) =
        method_definition_and_rhs(&parsed, typer.store(), &index, source, "implicitMember");
    let implicit_member_tree = method_tree(implicit_member);
    let typed_member = typer.type_method_definition(implicit_member_tree).unwrap();
    assert!(matches!(
        typer.typed_ast().get(typed_member).kind,
        TreeKind::DefDef(_)
    ));
    let typed_member_rhs = typer
        .source_typed_index()
        .get(source, implicit_member_rhs)
        .unwrap();
    assert!(
        matches!(typer.typed_ast().get(typed_member_rhs).kind, TreeKind::Closure(ref closure) if closure.env.is_empty())
    );
    assert_eq!(
        typer.source_typed_index().get(source, implicit_member_tree),
        Some(typed_member)
    );

    let (wrong, _) = method_definition_and_rhs(&parsed, typer.store(), &index, source, "wrong");
    match typer.type_method_definition(method_tree(wrong)) {
        Err(TyperError::ExpectedExpressionTypeMismatch { tree_index, .. }) => {
            assert_error_span(tree_index, source_text.find("x + 1").unwrap(), false)
        }
        Err(TyperError::ExpectedExpressionConformanceUnsupported { tree_index, .. }) => {
            assert_error_span(tree_index, source_text.find("x + 1").unwrap(), false)
        }
        other => panic!("expected wrong-result control, got {other:?}"),
    }
}

#[test]
fn lexical_closure_identity_and_mapping_are_session_local() {
    for _ in 0..2 {
        let source_text = "class C { def identity: Int => Int = x => x }";
        let (parsed, mut store, mut packages, definitions, index, source) =
            parse_and_name(source_text);
        enter_scala_function_classes(&mut store, &mut packages, [1]);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "identity");
        let method_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                (index.symbol_at(source, tree) == Some(method)
                    && matches!(node.kind, TreeKind::DefDef(_)))
                .then_some(tree)
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let typed_method = typer.type_method_definition(method_tree).unwrap();
        assert_eq!(
            typer.source_typed_index().get(source, method_tree),
            Some(typed_method)
        );
        let lambda_method = typer.function_literal_method_at(source, rhs).unwrap();
        assert_eq!(typer.store().symbols.get(lambda_method).owner, Some(method));
        assert!(
            matches!(typer.typed_ast().get(typer.source_typed_index().get(source, rhs).unwrap()).kind,
                TreeKind::Closure(ref closure) if closure.env.is_empty())
        );
    }
}

#[test]
#[ignore = "capture success receipts are enabled by the scoped downstream issues #1861-#1863"]
fn future_lexical_capture_receipts_require_identity_order_deduplication_and_owner() {
    let source_text = concat!(
        "class C { ",
        "def ordered(left: Int, right: Int): Int => Int = x => right + left + right; ",
        "def local: Int => Int = { val delta: Int = 2; (x: Int) => x + delta }; ",
        "def explicitThis: Int => C = _ => this",
        " }",
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
    for (name, expected_env_len) in [("ordered", 2), ("local", 1), ("explicitThis", 1)] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let lambda_tree = source_function_literal(&parsed, rhs);
        let method_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                (index.symbol_at(source, tree) == Some(method)
                    && matches!(node.kind, TreeKind::DefDef(_)))
                .then_some(tree)
            })
            .unwrap();
        let typed_method = typer
            .type_method_definition(method_tree)
            .expect("downstream capture support must type the whole source method");
        assert!(matches!(
            typer.typed_ast().get(typed_method).kind,
            TreeKind::DefDef(_)
        ));
        assert_eq!(
            typer.source_typed_index().get(source, method_tree),
            Some(typed_method)
        );
        let typed_rhs = typer
            .source_typed_index()
            .get(source, rhs)
            .expect("the complete method RHS should retain its source mapping");
        let typed_lambda = typer
            .source_typed_index()
            .get(source, lambda_tree)
            .expect("the source lambda should retain its own source mapping");
        if lambda_tree != rhs {
            let TreeKind::Block(block) = &typer.typed_ast().get(typed_rhs).kind else {
                panic!("a block-wrapped source lambda should preserve a typed Block RHS");
            };
            assert_eq!(block.expr, typed_lambda);
        } else {
            assert_eq!(typed_rhs, typed_lambda);
        }
        let typed_closure = typed_lambda;
        let TreeKind::Closure(ref closure) = typer.typed_ast().get(typed_closure).kind else {
            panic!("source lambda root must map to its typed Closure");
        };
        assert_eq!(closure.env.len(), expected_env_len);
        if name == "ordered" {
            let TreeKind::DefDef(definition) = &parsed.ast.get(method_tree).kind else {
                panic!("capture order receipt requires its source method");
            };
            let left = index
                .symbol_at(source, definition.value_param_clauses[0][0])
                .unwrap();
            let right = index
                .symbol_at(source, definition.value_param_clauses[0][1])
                .unwrap();
            let captured_symbols = closure
                .env
                .iter()
                .map(|entry| {
                    let Type::TermRef {
                        target: TermRefTarget::Symbol(symbol),
                        ..
                    } = typer.store().types.get(typer.typed_ast().get(*entry).ty)
                    else {
                        panic!("capture environment entry must refer to a canonical term symbol");
                    };
                    *symbol
                })
                .collect::<Vec<_>>();
            assert_eq!(captured_symbols, [right, left]);
        }
        assert!(
            typer
                .function_literal_method_at(source, lambda_tree)
                .is_some()
        );
    }
}

#[test]
#[ignore = "capture-owner validation is completed with the scoped capture work in #1861-#1863"]
fn future_lexical_capture_rejects_a_canonical_symbol_with_a_foreign_owner() {
    let source_text =
        "class C { def capture(delta: Int): Int => Int = x => x + delta }; class Foreign {}";
    let (parsed, mut store, mut packages, definitions, index, source) = parse_and_name(source_text);
    enter_scala_function_classes(&mut store, &mut packages, [1]);
    let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "capture");
    let method_tree = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            (index.symbol_at(source, tree) == Some(method)
                && matches!(node.kind, TreeKind::DefDef(_)))
            .then_some(tree)
        })
        .unwrap();
    let TreeKind::DefDef(definition) = &parsed.ast.get(method_tree).kind else {
        panic!("capture owner probe requires a source method");
    };
    let captured = index
        .symbol_at(source, definition.value_param_clauses[0][0])
        .unwrap();
    let foreign = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::TypeDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == "Foreign")
                .then(|| index.symbol_at(source, tree))
                .flatten()
        })
        .unwrap();
    store.symbols.get_mut(captured).owner = Some(foreign);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    assert!(matches!(
        typer.type_method_definition(method_tree),
        Err(TyperError::FunctionLiteralCaptureUnsupported { symbol: Some(symbol), .. })
            if symbol == captured
    ));
    assert_eq!(typer.source_typed_index().get(source, method_tree), None);
}
