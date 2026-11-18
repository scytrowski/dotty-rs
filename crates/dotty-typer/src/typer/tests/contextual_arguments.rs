use super::*;

fn definition_tree(index: &SourceSemanticIndex, symbol: SymbolId) -> TreeId<Untyped> {
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(symbol) else {
        panic!("expected a canonical source method")
    };
    tree
}

fn reference(typer: &SourceTyper<'_>, tree: TreeId<Typed>) -> SymbolId {
    match typer.store().types.get(typer.typed_ast().get(tree).ty) {
        Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        } => *symbol,
        other => panic!("expected symbol reference, got {other:?}"),
    }
}

#[test]
fn omitted_argument_forwards_the_canonical_parameter() {
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(include_str!(
        "../../../tests/fixtures/contextual-arguments/Calls.scala"
    ));
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "forward");
    let target = method_symbol(&parsed, &store, &index, source, "need");
    let parameter = method_parameter_symbol(&parsed, &index, source, method, 0);
    let method = definition_tree(&index, method);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let root = typer.type_method_definition(method).unwrap();
    assert_eq!(typer.type_method_definition(method).unwrap(), root);
    let rhs = typer.source_typed_index().get(source, rhs).unwrap();
    let node = typer.typed_ast().get(rhs);
    let TreeKind::Apply(app) = &node.kind else {
        panic!("missing contextual application")
    };
    assert_eq!(node.ty, definitions.int);
    assert_eq!(app.kind, ApplyKind::Regular);
    assert_eq!(reference(&typer, app.function), target);
    assert_eq!(app.args.len(), 1);
    assert_eq!(reference(&typer, app.args[0]), parameter);
}

#[test]
fn omitted_argument_reports_missing_and_ambiguous_without_partial_state() {
    let text = "class C { def need(using n: Int): Int = n; def missing(n: Int): Int = need; def amb(using x: Int, y: Int): Int = need }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    for name in ["missing", "amb"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let checkpoint = typer.store().checkpoint();
        for _ in 0..2 {
            let result = typer.type_method_definition(definition_tree(&index, method));
            match result {
                Err(TyperError::MissingContextualArgument {
                    expected,
                    tree_index,
                    ..
                }) if name == "missing" => {
                    assert_eq!((expected, tree_index), (definitions.int, rhs.index()));
                }
                Err(TyperError::AmbiguousContextualArgument {
                    expected,
                    tree_index,
                    candidates,
                    ..
                }) if name == "amb" => {
                    assert_eq!((expected, tree_index), (definitions.int, rhs.index()));
                    assert_eq!(
                        candidates,
                        (0..2)
                            .map(|i| method_parameter_symbol(&parsed, &index, source, method, i))
                            .collect::<Vec<_>>()
                    );
                }
                other => panic!("{name}: {other:?}"),
            }
            assert_eq!(typer.store().checkpoint(), checkpoint);
            assert!(typer.source_typed_index().is_empty());
            assert_eq!(typer.typed_ast().iter().count(), 0);
        }
    }
}

#[test]
fn nested_local_evidence_is_applied_in_clause_and_parameter_order() {
    let text = "class C { def f(x: Int)(using n: Int)(using b: Boolean): Int = n; def use(using outer: Int): Int = { implicit val b: Boolean = true; given local: Int = 7; f(1) } }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
    let TreeKind::Block(block) = &parsed.ast.get(rhs).kind else {
        panic!()
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
        .type_method_definition(definition_tree(&index, method))
        .unwrap();
    let mut call = typer.source_typed_index().get(source, block.expr).unwrap();
    for stat in &block.stats {
        let expected = typer.local_symbol_at(source, *stat).unwrap();
        let TreeKind::Apply(app) = &typer.typed_ast().get(call).kind else {
            panic!()
        };
        assert_eq!(app.args.len(), 1);
        assert_eq!(reference(&typer, app.args[0]), expected);
        call = app.function;
    }
    assert!(matches!(
        typer.typed_ast().get(call).kind,
        TreeKind::Apply(_)
    ));
}

#[test]
fn same_level_contextual_ranking_prefers_more_general_result_regardless_of_order() {
    for entries in [
        "given broad: Base = new Base; given narrow: Child = new Child",
        "given narrow: Child = new Child; given broad: Base = new Base",
    ] {
        let text = format!(
            "class Base; class Child extends Base; class C {{ {entries}; def need(using b: Base): Base = b; def call: Base = need }}"
        );
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(&text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "call");
        let (broad, _, _) = val_definition_and_rhs(&parsed, &store, &index, source, "broad");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        typer
            .type_method_definition(definition_tree(&index, method))
            .unwrap();
        let rhs = typer.source_typed_index().get(source, rhs).unwrap();
        let TreeKind::Apply(app) = &typer.typed_ast().get(rhs).kind else {
            panic!()
        };
        assert_eq!(reference(&typer, app.args[0]), broad);
        let Type::TermRef { prefix, .. } =
            typer.store.types.get(typer.typed_ast().get(app.args[0]).ty)
        else {
            panic!()
        };
        assert!(
            matches!(typer.store.types.get(*prefix), Type::ThisType { class } if Some(*class) == typer.store.symbols.get(method).owner)
        );
    }
}

#[test]
fn imported_values_retain_the_qualifier_and_selector_filter() {
    for (selector, selected) in [
        ("given", Some("modern")),
        ("given Int", Some("modern")),
        ("modern", Some("modern")),
        ("{modern as renamed}", Some("modern")),
        ("{modern as _, given}", None),
        ("*", None),
    ] {
        let text = format!(
            "object Instances {{ given modern: Int = 7; given flag: Boolean = true }}; class C {{ def need(using n: Int): Int = n; def use: Int = {{ import Instances.{selector}; need }} }}"
        );
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(&text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let TreeKind::Block(block) = &parsed.ast.get(rhs).kind else {
            panic!()
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let result = typer.type_method_definition(definition_tree(&index, method));
        if let Some(name) = selected {
            result.unwrap_or_else(|e| panic!("{selector}: {e:?}"));
            let call = typer.typed_index.get(source, block.expr).unwrap();
            let TreeKind::Apply(app) = &typer.typed_ast().get(call).kind else {
                panic!()
            };
            let TreeKind::Select(select) = &typer.typed_ast().get(app.args[0]).kind else {
                panic!("missing imported receiver")
            };
            let symbol = reference(&typer, app.args[0]);
            let receiver = reference(&typer, select.qualifier);
            assert_eq!(
                typer
                    .store
                    .names
                    .resolve(typer.store.symbols.get(symbol).name.text()),
                name
            );
            assert_eq!(
                typer
                    .store
                    .names
                    .resolve(typer.store.symbols.get(receiver).name.text()),
                "Instances"
            );
        } else {
            assert!(
                matches!(result, Err(TyperError::MissingContextualArgument { .. })),
                "{selector}: {result:?}"
            );
        }
    }
}

#[test]
fn imports_from_distinct_instances_keep_distinct_evidence_paths() {
    let text = "class Provider { given value: Int = 1 }; class C { def need(using n: Int): Int = n; def ok(p: Provider): Int = { import p.given; need }; def latest(a: Provider, b: Provider): Int = { import a.given; import b.given; need } }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, "ok");
    typer
        .type_method_definition(definition_tree(&index, method))
        .unwrap();
    let TreeKind::Block(block) = &parsed.ast.get(rhs).kind else {
        panic!()
    };
    let call = typer.typed_index.get(source, block.expr).unwrap();
    let TreeKind::Apply(app) = &typer.typed_ast().get(call).kind else {
        panic!()
    };
    let TreeKind::Select(selection) = &typer.typed_ast().get(app.args[0]).kind else {
        panic!()
    };
    assert_eq!(
        reference(&typer, selection.qualifier),
        method_parameter_symbol(&parsed, &index, source, method, 0)
    );
    let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, "latest");
    typer
        .type_method_definition(definition_tree(&index, method))
        .unwrap();
    let TreeKind::Block(block) = &parsed.ast.get(rhs).kind else {
        panic!()
    };
    let call = typer.typed_index.get(source, block.expr).unwrap();
    let TreeKind::Apply(app) = &typer.typed_ast().get(call).kind else {
        panic!()
    };
    let TreeKind::Select(select) = &typer.typed_ast().get(app.args[0]).kind else {
        panic!()
    };
    assert_eq!(
        reference(&typer, select.qualifier),
        method_parameter_symbol(&parsed, &index, source, method, 1)
    );
}
