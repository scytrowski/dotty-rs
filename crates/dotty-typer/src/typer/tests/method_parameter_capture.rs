use super::*;

#[test]
fn ordinary_function1_captures_enclosing_method_parameters_by_first_use() {
    let source_text = r#"
object CaptureParameter:
  def addBy(delta: Int): Int => Int =
    x => x + delta
  def addBoth(first: Int, second: Int): Int => Int =
    x => x + first + second + first
  def shadow(delta: Int): Int => Int =
    (delta: Int) => delta
  def byName(delta: Int, later: => Int): Int => Int =
    x => x + delta + later
"#;
    let (parsed, mut store, mut packages, definitions, index, source) = parse_and_name(source_text);
    enter_scala_function_classes(&mut store, &mut packages, [1]);
    let methods: Vec<_> = parsed
        .ast
        .iter()
        .filter_map(|(tree, node)| {
            (matches!(node.kind, TreeKind::DefDef(_)) && index.symbol_at(source, tree).is_some())
                .then_some(tree)
        })
        .collect();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );

    for name in ["addBy", "addBoth", "shadow"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let method_tree = methods
            .iter()
            .copied()
            .find(|tree| index.symbol_at(source, *tree) == Some(method))
            .unwrap();
        let typed = typer.type_method_definition(method_tree).unwrap();
        assert_eq!(
            typer.source_typed_index().get(source, method_tree),
            Some(typed)
        );
        let typed_rhs = typer.source_typed_index().get(source, rhs).unwrap();
        let TreeKind::Closure(closure) = &typer.typed_ast().get(typed_rhs).kind else {
            panic!("{name} RHS should be a typed closure");
        };
        let source_definition = match &parsed.ast.get(method_tree).kind {
            TreeKind::DefDef(definition) => definition,
            _ => unreachable!(),
        };
        let source_parameters = &source_definition.value_param_clauses[0];
        let expected_captures: Vec<_> = match name {
            "addBy" => vec![index.symbol_at(source, source_parameters[0]).unwrap()],
            "addBoth" => vec![
                index.symbol_at(source, source_parameters[0]).unwrap(),
                index.symbol_at(source, source_parameters[1]).unwrap(),
            ],
            "shadow" => vec![],
            _ => unreachable!(),
        };
        assert_eq!(closure.env.len(), expected_captures.len(), "{name}");
        for (captured, expected) in closure.env.iter().zip(expected_captures) {
            let TreeKind::Ident(_) = typer.typed_ast().get(*captured).kind else {
                panic!("capture should retain the actual typed identifier use");
            };
            let Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } = typer.store().types.get(typer.typed_ast().get(*captured).ty)
            else {
                panic!("capture should retain canonical symbol provenance");
            };
            assert_eq!(*symbol, expected, "{name} capture identity");
            let source_name = typer.store().symbols.get(expected).name;
            let first_use = parsed.ast.iter().find_map(|(source_tree, source_node)| {
                matches!(
                    &source_node.kind,
                    TreeKind::Ident(identifier) if identifier.name == source_name
                )
                .then(|| typer.source_typed_index().get(source, source_tree))
                .flatten()
            });
            assert_eq!(
                Some(*captured),
                first_use,
                "{name} source-to-typed capture mapping"
            );
        }
    }

    let (by_name, by_name_rhs) =
        method_definition_and_rhs(&parsed, typer.store(), &index, source, "byName");
    let by_name_tree = methods
        .iter()
        .copied()
        .find(|tree| index.symbol_at(source, *tree) == Some(by_name))
        .unwrap();
    let by_name_parameter = match &parsed.ast.get(by_name_tree).kind {
        TreeKind::DefDef(definition) => index
            .symbol_at(source, definition.value_param_clauses[0][1])
            .unwrap(),
        _ => unreachable!(),
    };
    assert!(matches!(
        typer.type_method_definition(by_name_tree),
        Err(TyperError::FunctionLiteralCaptureUnsupported { symbol: Some(symbol), .. })
            if symbol == by_name_parameter
    ));
    assert_eq!(typer.source_typed_index().get(source, by_name_tree), None);
    assert_eq!(typer.source_typed_index().get(source, by_name_rhs), None);
    assert_eq!(typer.function_literal_method_at(source, by_name_rhs), None);
    assert!(matches!(
        typer.type_method_definition(by_name_tree),
        Err(TyperError::FunctionLiteralCaptureUnsupported { symbol: Some(symbol), .. })
            if symbol == by_name_parameter
    ));
    assert_eq!(typer.function_literal_method_at(source, by_name_rhs), None);
}

#[test]
fn method_parameter_capture_is_session_local() {
    fn capture_receipt() -> (String, SymbolKind) {
        let source_text = "object C:\n  def addBy(delta: Int): Int => Int = x => x + delta\n";
        let (parsed, mut store, mut packages, definitions, index, source) =
            parse_and_name(source_text);
        enter_scala_function_classes(&mut store, &mut packages, [1]);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "addBy");
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
        let typed_rhs = typer.source_typed_index().get(source, rhs).unwrap();
        let TreeKind::Closure(closure) = &typer.typed_ast().get(typed_rhs).kind else {
            panic!("source function should type as a closure");
        };
        assert_eq!(closure.env.len(), 1);
        let Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        } = typer
            .store()
            .types
            .get(typer.typed_ast().get(closure.env[0]).ty)
        else {
            panic!("capture should retain a canonical symbol");
        };
        assert_eq!(typer.store().symbols.get(*symbol).owner, Some(method));
        (
            typer
                .store()
                .names
                .resolve(typer.store().symbols.get(*symbol).name.text())
                .to_owned(),
            typer.store().symbols.get(*symbol).kind,
        )
    }

    assert_eq!(
        capture_receipt(),
        ("delta".to_owned(), SymbolKind::Parameter)
    );
    assert_eq!(
        capture_receipt(),
        ("delta".to_owned(), SymbolKind::Parameter)
    );
}

#[test]
fn method_parameter_capture_works_without_an_expected_function1_type() {
    let source_text = r#"
object InferredCapture:
  def inBlock(delta: Int): Int => Int =
    val unused: Int = 0
    (x: Int) => x + delta
  def inferredResult(delta: Int) =
    (x: Int) => x + delta
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

    for name in ["inBlock", "inferredResult"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let method_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, _)| (index.symbol_at(source, tree) == Some(method)).then_some(tree))
            .unwrap();
        let lambda_tree = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(block) => block.expr,
            TreeKind::PhaseSpecific(UntypedNode::Function(_)) => rhs,
            _ => panic!("{name} should end in a function literal"),
        };
        let typed_method = typer.type_method_definition(method_tree).unwrap();
        assert!(matches!(
            typer.typed_ast().get(typed_method).kind,
            TreeKind::DefDef(_)
        ));
        assert_eq!(
            typer.source_typed_index().get(source, method_tree),
            Some(typed_method)
        );
        let typed_lambda = typer.source_typed_index().get(source, lambda_tree).unwrap();
        let TreeKind::Closure(closure) = &typer.typed_ast().get(typed_lambda).kind else {
            panic!("{name} lambda should be a typed closure");
        };
        assert_eq!(closure.env.len(), 1, "{name}");
        let source_parameter = match &parsed.ast.get(method_tree).kind {
            TreeKind::DefDef(definition) => index
                .symbol_at(source, definition.value_param_clauses[0][0])
                .unwrap(),
            _ => unreachable!(),
        };
        let Type::TermRef {
            target: TermRefTarget::Symbol(captured),
            ..
        } = typer
            .store()
            .types
            .get(typer.typed_ast().get(closure.env[0]).ty)
        else {
            panic!("capture should retain a canonical parameter reference");
        };
        assert_eq!(*captured, source_parameter, "{name}");
    }
}
