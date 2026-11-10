use super::*;

fn direct_this_call(
    parsed: &dotty_parser::ParseResult,
    store: &SemanticStore,
    index: &SourceSemanticIndex,
    source: SourceId,
    method: &str,
) -> (SymbolId, TreeId<Untyped>, TreeId<Untyped>) {
    let method_tree = method_definition_tree(parsed, store, method);
    let method = index.symbol_at(source, method_tree).unwrap();
    let TreeKind::DefDef(definition) = &parsed.ast.get(method_tree).kind else {
        unreachable!("helper found a method definition")
    };
    let rhs = definition.rhs.expect("test method has a body");
    let rhs_range = parsed.ast.get(rhs).position.unwrap().span().range();
    let application = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            if !node.position.is_some_and(|position| {
                let range = position.span().range();
                rhs_range.start() <= range.start() && range.end() <= rhs_range.end()
            }) {
                return None;
            }
            match &node.kind {
                TreeKind::Apply(application)
                    if application.args.len() == 1
                        && matches!(
                            parsed
                                .ast
                                .try_get(application.args[0])
                                .map(|node| &node.kind),
                            Some(TreeKind::This(_))
                        ) =>
                {
                    Some((tree, application.function, application.args[0]))
                }
                _ => None,
            }
        })
        .expect("method contains a direct this argument");
    (method, application.1, application.2)
}

#[test]
fn qualified_direct_this_prepares_only_the_authenticated_source_owner() {
    let source_text = "package other\nclass Tuple {\n  def use = runtime.Tuples.toArray(this)\n  def plain = consume(this)\n}\nobject runtime { object Tuples { def toArray(value: Tuple): Int = 0 } }\nclass Unused";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
    let tuple = class_symbol(&parsed, &store, &index, source, "Tuple");
    let unused = class_symbol(&parsed, &store, &index, source, "Unused");
    let (use_method, use_function, use_argument) = {
        let (method, function, argument) = direct_this_call(&parsed, &store, &index, source, "use");
        (method, function, argument)
    };
    let (plain_method, plain_function, plain_argument) = {
        let (method, function, argument) =
            direct_this_call(&parsed, &store, &index, source, "plain");
        (method, function, argument)
    };
    let tuple_tree = tuple_tree(&parsed, &store, "Tuple");
    assert_eq!(
        index.definition_of(tuple),
        Some(SourceDefinition::Canonical {
            source,
            tree: tuple_tree,
        })
    );
    let (tuples_tree, tuples_object) = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module))
                if store.names.resolve(module.name.as_name().text()) == "Tuples" =>
            {
                Some((tree, index.symbol_at(source, tree).unwrap()))
            }
            _ => None,
        })
        .expect("external-looking source receiver object");
    let tuples_module_class = index
        .derived_symbol_at(
            store.symbols.get(tuples_object).owner.unwrap(),
            source,
            tuples_tree,
        )
        .unwrap();
    let use_context = ExpressionContext {
        lexical: index.declaration_context_of(use_method).unwrap(),
        owner: use_method,
        local_scopes: None,
    };
    let plain_context = ExpressionContext {
        lexical: index.declaration_context_of(plain_method).unwrap(),
        owner: plain_method,
        local_scopes: None,
    };
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    assert!(matches!(
        *typer.store.symbols.info(tuple),
        SymbolInfo::Missing
    ));
    assert!(matches!(
        *typer.store.symbols.info(unused),
        SymbolInfo::Missing
    ));
    assert!(matches!(
        *typer.store.symbols.info(tuples_module_class),
        SymbolInfo::Missing
    ));

    typer
        .prepare_source_owner_for_qualified_this_argument(
            plain_function,
            plain_argument,
            plain_context,
            plain_function.index(),
            &mut Vec::new(),
        )
        .unwrap();
    assert!(matches!(
        *typer.store.symbols.info(tuple),
        SymbolInfo::Missing
    ));

    typer
        .prepare_source_owner_for_qualified_this_argument(
            use_function,
            use_argument,
            use_context,
            use_function.index(),
            &mut Vec::new(),
        )
        .unwrap();
    let SymbolInfo::Complete(class_info_type) = *typer.store.symbols.info(tuple) else {
        panic!("the authentic source owner should be completed");
    };
    let Type::ClassInfo(class_info) = typer.store.types.get(class_info_type) else {
        panic!("source owner completion publishes ClassInfo");
    };
    assert_eq!(class_info.class, tuple);
    assert_eq!(class_info.declarations, index.scope_of(tuple).unwrap());
    assert!(matches!(
        *typer.store.symbols.info(unused),
        SymbolInfo::Missing
    ));
    assert!(
        matches!(
            *typer.store.symbols.info(tuples_module_class),
            SymbolInfo::Missing,
        ),
        "preparation does not complete the selected receiver class"
    );
    let tuple_symbol = typer.store.symbols.get(tuple);
    let package = tuple_symbol.owner.expect("Tuple has its package owner");
    assert_eq!(typer.store.names.resolve(tuple_symbol.name.text()), "Tuple");
    assert_eq!(
        typer
            .store
            .names
            .resolve(typer.store.symbols.get(package).name.text()),
        "other"
    );
}

#[test]
fn malformed_completed_source_owner_is_rejected_without_replacement() {
    let source_text = "package other\nclass Tuple { def use = runtime.Tuples.toArray(this) }\nobject runtime { object Tuples { def toArray(value: Tuple): Int = 0 } }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
    let tuple = class_symbol(&parsed, &store, &index, source, "Tuple");
    let (method, function, argument) = direct_this_call(&parsed, &store, &index, source, "use");
    let method_tree = method_definition_tree(&parsed, &store, "use");
    let context = ExpressionContext {
        lexical: index.declaration_context_of(method).unwrap(),
        owner: method,
        local_scopes: None,
    };
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer
        .store
        .symbols
        .set_info(tuple, SymbolInfo::Complete(definitions.int));
    assert!(matches!(
        typer.prepare_source_owner_for_qualified_this_argument(
            function,
            argument,
            context,
            function.index(),
            &mut Vec::new(),
        ),
        Err(TyperError::MalformedClassInfo { symbol, info })
            if symbol == tuple && info == definitions.int
    ));
    assert_eq!(
        *typer.store.symbols.info(tuple),
        SymbolInfo::Complete(definitions.int),
        "malformed prior owner metadata is preserved for the caller to roll back"
    );
    assert_eq!(typer.source_typed_index().get(source, method_tree), None);
}

#[test]
fn inconsistent_selected_formal_stays_a_rejection_and_rolls_back_owner_preparation() {
    let source_text = "package other\nclass Tuple { def use: Int = runtime.Tuples.toArray(this) }\nobject runtime { object Tuples { def toArray(value: Int): Int = value } }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
    let tuple = class_symbol(&parsed, &store, &index, source, "Tuple");
    let method_tree = method_definition_tree(&parsed, &store, "use");
    let method = index.symbol_at(source, method_tree).unwrap();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );

    let error = typer
        .type_method_definition(method_tree)
        .expect_err("Tuple must not be coerced to the selected Int formal");
    assert!(
        matches!(
            &error,
            TyperError::OverloadApplicationNoApplicable { .. }
                | TyperError::ApplicationArgumentTypeMismatch { .. }
        ),
        "unexpected inconsistent-formal result: {error:?}"
    );
    assert!(
        matches!(*typer.store.symbols.info(tuple), SymbolInfo::Missing),
        "the failed call rolls back its source-owner preparation"
    );
    assert_eq!(typer.source_typed_index().get(source, method_tree), None);
    assert!(index.definition_of(method).is_some());
}

fn tuple_tree(
    parsed: &dotty_parser::ParseResult,
    store: &SemanticStore,
    name: &str,
) -> TreeId<Untyped> {
    parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::TypeDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == name =>
            {
                Some(tree)
            }
            _ => None,
        })
        .unwrap()
}

fn method_definition_tree(
    parsed: &dotty_parser::ParseResult,
    store: &SemanticStore,
    name: &str,
) -> TreeId<Untyped> {
    parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::DefDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == name =>
            {
                Some(tree)
            }
            _ => None,
        })
        .unwrap()
}
