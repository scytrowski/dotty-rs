use super::*;

#[test]
fn ordinary_function1_captures_prior_initialized_local_values() {
    let source_text = r#"
object CaptureLocal:
  def offset: Int => Int =
    val bias: Int = 3
    x => x + bias
  def two: Int => Int =
    val first: Int = 1
    val second: Int = 2
    x => x + first + second + first
  def shadowed: Int => Int =
    val value: Int = 100
    (value: Int) => value
"#;
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

    for (name, expected_names) in [
        ("offset", vec!["bias"]),
        ("two", vec!["first", "second"]),
        ("shadowed", vec![]),
    ] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let method_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, _)| (index.symbol_at(source, tree) == Some(method)).then_some(tree))
            .unwrap();
        let typed_method = typer.type_method_definition(method_tree).unwrap();
        assert_eq!(
            typer.source_typed_index().get(source, method_tree),
            Some(typed_method)
        );
        assert!(matches!(
            typer.typed_ast().get(typed_method).kind,
            TreeKind::DefDef(_)
        ));
        let SymbolInfo::Complete(method_type) = *typer.store().symbols.info(method) else {
            panic!("method signature should remain complete after body typing");
        };
        let expected_result = method_type;
        let context = typer.expression_context_for(method).unwrap();
        let expected_rhs = typer
            .type_expression_expected(rhs, context, expected_result)
            .expect("repeated expected typing of the method RHS should succeed");
        let expected_rhs_again = typer
            .type_expression_expected(rhs, context, expected_result)
            .expect("a second expected typing of the method RHS should reuse the cache");
        assert_eq!(expected_rhs, expected_rhs_again, "{name} RHS identity");
        assert_eq!(
            expected_rhs,
            typer.source_typed_index().get(source, rhs).unwrap(),
            "{name} cached source mapping"
        );
        let TreeKind::Block(block) = &parsed.ast.get(rhs).kind else {
            panic!("{name} source body should be a block");
        };
        let typed_block = typer.source_typed_index().get(source, rhs).unwrap();
        let TreeKind::Block(typed_block_node) = &typer.typed_ast().get(typed_block).kind else {
            panic!("{name} typed body should retain its block");
        };
        let local_declarations: Vec<_> = block
            .stats
            .iter()
            .copied()
            .filter(|tree| matches!(parsed.ast.get(*tree).kind, TreeKind::ValDef(_)))
            .collect();
        assert_eq!(typed_block_node.stats.len(), block.stats.len(), "{name}");
        for declaration in &local_declarations {
            let binding = typer
                .typed_local_binding_at(source, *declaration)
                .expect("initialized local should retain its canonical binding");
            assert!(matches!(
                typer.typed_ast().get(binding.declaration).kind,
                TreeKind::ValDef(_)
            ));
            assert_eq!(
                typer.source_typed_index().get(source, *declaration),
                Some(binding.declaration)
            );
        }

        let lambda = block.expr;
        let typed_lambda = typer.source_typed_index().get(source, lambda).unwrap();
        let TreeKind::Closure(closure) = &typer.typed_ast().get(typed_lambda).kind else {
            panic!("{name} expression should type to a closure");
        };
        assert_eq!(closure.env.len(), expected_names.len(), "{name}");
        for (capture, expected_name) in closure.env.iter().zip(expected_names) {
            let TreeKind::Ident(_) = typer.typed_ast().get(*capture).kind else {
                panic!("capture env entry should be a typed identifier");
            };
            let Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } = typer.store().types.get(typer.typed_ast().get(*capture).ty)
            else {
                panic!("capture should refer to its canonical local symbol");
            };
            let binding = local_declarations
                .iter()
                .find_map(|tree| {
                    let TreeKind::ValDef(definition) = &parsed.ast.get(*tree).kind else {
                        return None;
                    };
                    (typer
                        .store()
                        .names
                        .resolve(definition.name.as_name().text())
                        == expected_name)
                        .then(|| typer.typed_local_binding_at(source, *tree))
                        .flatten()
                })
                .unwrap();
            assert_eq!(*symbol, binding.symbol, "{name} capture identity");
            assert_eq!(typer.store().symbols.get(*symbol).owner, Some(method));
            assert_eq!(
                typer.store().symbols.info(*symbol),
                &SymbolInfo::Complete(typer.typed_ast().get(binding.declaration).ty)
            );
            let first_source_use = parsed.ast.iter().find_map(|(source_tree, node)| {
                matches!(&node.kind, TreeKind::Ident(ident) if typer.store().names.resolve(ident.name.text()) == expected_name)
                    .then(|| typer.source_typed_index().get(source, source_tree))
                    .flatten()
            });
            assert_eq!(
                first_source_use,
                Some(*capture),
                "{name} source use mapping"
            );
        }
    }
}

#[test]
fn ordinary_function1_orders_mixed_parameter_and_local_captures_by_first_use() {
    let (parsed, mut store, mut packages, definitions, index, source) = parse_and_name(
        "object Mixed:\n  def f(delta: Int): Int => Int =\n    val bias: Int = 3\n    x => x + delta + bias + delta\n",
    );
    enter_scala_function_classes(&mut store, &mut packages, [1]);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "f");
    let method_tree = parsed
        .ast
        .iter()
        .find_map(|(tree, _)| (index.symbol_at(source, tree) == Some(method)).then_some(tree))
        .unwrap();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer.type_method_definition(method_tree).unwrap();
    let TreeKind::Block(block) = &parsed.ast.get(rhs).kind else {
        panic!("method body should be a block");
    };
    let local = block.stats[0];
    let binding = typer.typed_local_binding_at(source, local).unwrap();
    let parameter = match &parsed.ast.get(method_tree).kind {
        TreeKind::DefDef(definition) => index
            .symbol_at(source, definition.value_param_clauses[0][0])
            .unwrap(),
        _ => unreachable!(),
    };
    let typed_lambda = typer.source_typed_index().get(source, block.expr).unwrap();
    let TreeKind::Closure(closure) = &typer.typed_ast().get(typed_lambda).kind else {
        panic!("result should be a closure");
    };
    let captured: Vec<_> = closure
        .env
        .iter()
        .map(
            |capture| match typer.store().types.get(typer.typed_ast().get(*capture).ty) {
                Type::TermRef {
                    target: TermRefTarget::Symbol(symbol),
                    ..
                } => *symbol,
                _ => panic!("capture should retain its canonical symbol reference"),
            },
        )
        .collect();
    assert_eq!(captured, vec![parameter, binding.symbol]);
}

#[test]
fn nested_block_shadowing_captures_the_innermost_canonical_local() {
    let source_text = r#"
object NestedLocalCapture:
  def f: Int => Int = {
    val value: Int = 100
    {
      val value: Int = 3
      x => x + value
    }
  }
"#;
    let (parsed, mut store, mut packages, definitions, index, source) = parse_and_name(source_text);
    enter_scala_function_classes(&mut store, &mut packages, [1]);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "f");
    let method_tree = parsed
        .ast
        .iter()
        .find_map(|(tree, _)| (index.symbol_at(source, tree) == Some(method)).then_some(tree))
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
    assert!(matches!(
        typer.typed_ast().get(typed_method).kind,
        TreeKind::DefDef(_)
    ));
    let TreeKind::Block(outer_block) = &parsed.ast.get(rhs).kind else {
        panic!("source method body should be an outer block");
    };
    let outer_local = typer
        .typed_local_binding_at(source, outer_block.stats[0])
        .unwrap();
    let outer_typed_block = typer.source_typed_index().get(source, rhs).unwrap();
    let TreeKind::Block(typed_outer_block) = &typer.typed_ast().get(outer_typed_block).kind else {
        panic!("outer typed block should be retained");
    };
    let inner_source_block = outer_block.expr;
    let inner_typed_block = typer
        .source_typed_index()
        .get(source, inner_source_block)
        .expect("nested expected block should retain a source mapping");
    let TreeKind::Block(inner_block) = &parsed.ast.get(inner_source_block).kind else {
        panic!("final source expression should be a nested block");
    };
    let inner_local = typer
        .typed_local_binding_at(source, inner_block.stats[0])
        .unwrap();
    assert_eq!(
        typer.source_typed_index().get(source, inner_block.stats[0]),
        Some(inner_local.declaration)
    );
    assert_eq!(typed_outer_block.expr, inner_typed_block);
    let typed_lambda = typer
        .source_typed_index()
        .get(source, inner_block.expr)
        .expect("inner lambda should be mapped");
    let TreeKind::Closure(closure) = &typer.typed_ast().get(typed_lambda).kind else {
        panic!("inner expression should be a closure");
    };
    assert_eq!(closure.env.len(), 1);
    assert!(matches!(
        typer.store().types.get(typer.typed_ast().get(closure.env[0]).ty),
        Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
            if *symbol == inner_local.symbol && *symbol != outer_local.symbol
    ));
}

#[test]
fn expected_function_type_reaches_a_lambda_through_parenthesized_blocks() {
    let source_text = r#"
object ParenthesizedLocalCapture:
  def f: Int => Int = {
    val value: Int = 100
    (({
      val value: Int = 3
      (x => x + value)
    }))
  }
"#;
    let (parsed, mut store, mut packages, definitions, index, source) = parse_and_name(source_text);
    enter_scala_function_classes(&mut store, &mut packages, [1]);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "f");
    let method_tree = parsed
        .ast
        .iter()
        .find_map(|(tree, _)| (index.symbol_at(source, tree) == Some(method)).then_some(tree))
        .unwrap();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer.type_method_definition(method_tree).unwrap();
    let TreeKind::Block(outer_block) = &parsed.ast.get(rhs).kind else {
        panic!("method body should be an outer block");
    };
    let outer_local = typer
        .typed_local_binding_at(source, outer_block.stats[0])
        .unwrap();
    let mut inner_tree = outer_block.expr;
    while let TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) =
        &parsed.ast.get(inner_tree).kind
    {
        inner_tree = parens.inner;
    }
    let TreeKind::Block(inner_block) = &parsed.ast.get(inner_tree).kind else {
        panic!("parenthesized final block should remain available as a source tree");
    };
    let inner_local = typer
        .typed_local_binding_at(source, inner_block.stats[0])
        .unwrap();
    let typed_lambda = typer
        .source_typed_index()
        .get(source, inner_block.expr)
        .expect("lambda should map through the block and parentheses");
    let TreeKind::Closure(closure) = &typer.typed_ast().get(typed_lambda).kind else {
        panic!("nested lambda should type as a closure");
    };
    assert_eq!(closure.env.len(), 1);
    assert!(matches!(
        typer.store().types.get(typer.typed_ast().get(closure.env[0]).ty),
        Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
            if *symbol == inner_local.symbol && *symbol != outer_local.symbol
    ));
}

#[test]
fn unsupported_local_shapes_do_not_create_partial_closures() {
    let source_text = r#"
object UnsupportedLocalCapture:
  def outside: Int => Int = x => x + value
  def mutable: Int => Int =
    var value: Int = 3
    x => x + value
  def lazyValue: Int => Int =
    lazy val value: Int = 3
    x => x + value
  def contextual: Int => Int =
    given value: Int = 3
    x => x + value
  def pattern: Int => Int =
    val (value, other) = (3, 4)
    x => x + value
  def forward: Int => Int =
    (x: Int) => x + later
    val later: Int = 3
    x => x
  def mismatch: String => String =
    val bias: Int = 3
    x => x + bias
  def invalidInitializer: Int => Int =
    val bias: Int = "wrong"
    x => x + bias
"#;
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

    for name in [
        "mutable",
        "outside",
        "lazyValue",
        "contextual",
        "pattern",
        "forward",
        "mismatch",
        "invalidInitializer",
    ] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let method_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, _)| (index.symbol_at(source, tree) == Some(method)).then_some(tree))
            .unwrap();
        assert!(typer.type_method_definition(method_tree).is_err(), "{name}");
        assert_eq!(
            typer.source_typed_index().get(source, method_tree),
            None,
            "{name}"
        );
        assert_eq!(typer.source_typed_index().get(source, rhs), None, "{name}");
        assert_eq!(
            typer.function_literal_method_at(source, rhs),
            None,
            "{name}"
        );
        assert!(
            typer.type_method_definition(method_tree).is_err(),
            "{name} retry"
        );
        assert_eq!(
            typer.source_typed_index().get(source, method_tree),
            None,
            "{name} retry"
        );
    }
}

#[test]
fn local_capture_retries_and_fresh_sessions_keep_the_same_identity_shape() {
    fn receipt() -> (String, SymbolKind, bool) {
        let (parsed, mut store, mut packages, definitions, index, source) = parse_and_name(
            "object Fresh:\n  def f: Int => Int =\n    val bias: Int = 3\n    x => x + bias\n",
        );
        enter_scala_function_classes(&mut store, &mut packages, [1]);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "f");
        let method_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, _)| (index.symbol_at(source, tree) == Some(method)).then_some(tree))
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let first = typer.type_method_definition(method_tree).unwrap();
        assert_eq!(typer.type_method_definition(method_tree).unwrap(), first);
        let TreeKind::Block(block) = &parsed.ast.get(rhs).kind else {
            panic!("source body should be a block");
        };
        let typed_block = typer.source_typed_index().get(source, rhs).unwrap();
        let TreeKind::Block(typed_block) = &typer.typed_ast().get(typed_block).kind else {
            panic!("typed body should retain its block");
        };
        assert_eq!(typed_block.stats.len(), block.stats.len());
        let typed_lambda = typer.source_typed_index().get(source, block.expr).unwrap();
        assert_ne!(typed_lambda, first);
        let TreeKind::Closure(closure) = &typer.typed_ast().get(typed_lambda).kind else {
            panic!("body result should be a closure");
        };
        let Type::TermRef {
            target: TermRefTarget::Symbol(captured),
            ..
        } = typer
            .store()
            .types
            .get(typer.typed_ast().get(closure.env[0]).ty)
        else {
            panic!("local capture should keep its canonical symbol reference");
        };
        (
            typer
                .store()
                .names
                .resolve(typer.store().symbols.get(*captured).name.text())
                .to_owned(),
            typer.store().symbols.get(*captured).kind,
            typer.store().symbols.get(*captured).owner == Some(method),
        )
    }

    let expected = ("bias".to_owned(), SymbolKind::Local, true);
    assert_eq!(receipt(), expected);
    assert_eq!(receipt(), expected);
}
