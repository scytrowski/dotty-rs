use super::*;

fn typed_reference(typer: &SourceTyper<'_>, tree: TreeId<Typed>) -> SymbolId {
    typer
        .store
        .types
        .get(typer.typed_arena.get(tree).ty)
        .reference_symbol()
        .unwrap()
}

#[test]
fn contextual_extension_selects_the_inner_provider_and_applies_its_using_clause() {
    let fixture = include_str!("../../../tests/fixtures/contextual-consumers/Extensions.scala");
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(fixture);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "choose");
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!()
    };
    let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
        panic!()
    };
    let param = |clause: usize, n: usize| {
        index
            .symbol_at(source, def.value_param_clauses[clause][n])
            .unwrap()
    };
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer.type_method_definition(tree).unwrap();
    let typed = typer.typed_index.get(source, rhs).unwrap();
    let TreeKind::Apply(using_call) = &typer.typed_arena.get(typed).kind else {
        panic!()
    };
    assert_eq!(typed_reference(&typer, using_call.args[0]), param(1, 1));
    let TreeKind::Apply(ordinary) = &typer.typed_arena.get(using_call.function).kind else {
        panic!()
    };
    let TreeKind::Apply(receiver) = &typer.typed_arena.get(ordinary.function).kind else {
        panic!()
    };
    assert_eq!(typed_reference(&typer, receiver.args[0]), param(0, 0));
    let TreeKind::Select(extension) = &typer.typed_arena.get(receiver.function).kind else {
        panic!()
    };
    assert_eq!(typed_reference(&typer, extension.qualifier), param(1, 0));
    assert_eq!(typer.store.names.resolve(extension.name.text()), "render");
    let (property, rhs) =
        method_definition_and_rhs(&parsed, typer.store(), &index, source, "property");
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(property) else {
        panic!()
    };
    typer.type_method_definition(tree).unwrap();
    let typed = typer.typed_index.get(source, rhs).unwrap();
    let TreeKind::Apply(evidence) = &typer.typed_arena.get(typed).kind else {
        panic!()
    };
    let TreeKind::Apply(base) = &typer.typed_arena.get(evidence.function).kind else {
        panic!()
    };
    let TreeKind::Select(selected) = &typer.typed_arena.get(base.function).kind else {
        panic!()
    };
    assert_eq!(typer.store.names.resolve(selected.name.text()), "size");
    assert_eq!(base.args.len(), 1);
    assert_eq!(evidence.args.len(), 1);
}

#[test]
fn equally_nested_contextual_extensions_report_the_conflicting_providers() {
    let fixture = include_str!("../../../tests/fixtures/contextual-consumers/Extensions.scala");
    let text = fixture.replace(
        "inner: Ops, fmt: Format",
        "first: Ops, second: Ops, fmt: Format",
    );
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(&text);
    let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "choose");
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!()
    };
    let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
        panic!()
    };
    let expected: Vec<_> = def.value_param_clauses[1][..2]
        .iter()
        .map(|p| index.symbol_at(source, *p).unwrap())
        .collect();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let checkpoint = typer.store.checkpoint();
    assert!(
        matches!(typer.type_method_definition(tree),Err(TyperError::AmbiguousContextualArgument {candidates,..}) if candidates==expected)
    );
    assert_eq!(typer.store.checkpoint(), checkpoint);
    assert!(typer.typed_index.is_empty());
}

#[test]
fn contextual_conversion_uses_canonical_scala_conversion_in_result_and_argument_positions() {
    let text = include_str!("../../../tests/fixtures/contextual-consumers/Conversions.scala");
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let resolver = super::contextual_binary::library_resolver(definitions, &packages);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(resolver));
    for name in ["result", "argument"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
            panic!()
        };
        let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
            panic!()
        };
        let formal = |clause: usize| {
            index
                .symbol_at(source, def.value_param_clauses[clause][0])
                .unwrap()
        };
        typer.type_method_definition(tree).unwrap();
        let mut adapted = typer.typed_index.get(source, rhs).unwrap();
        if name == "argument" {
            let TreeKind::Apply(accept) = &typer.typed_arena.get(adapted).kind else {
                panic!("expected accept")
            };
            adapted = accept.args[0];
        }
        let TreeKind::Apply(app) = &typer.typed_arena.get(adapted).kind else {
            panic!("expected conversion")
        };
        let TreeKind::Select(selected) = &typer.typed_arena.get(app.function).kind else {
            panic!()
        };
        assert_eq!(typed_reference(&typer, selected.qualifier), formal(1));
        assert_eq!(typed_reference(&typer, app.args[0]), formal(0));
        let apply = typer
            .store
            .symbols
            .get(typed_reference(&typer, app.function));
        assert_eq!(typer.store.names.resolve(apply.name.text()), "apply");
        assert!(matches!(apply.origin, SymbolOrigin::Tasty(_)));
    }
}

#[test]
fn contextual_conversion_reports_ambiguity_and_ignores_an_unrelated_conversion_class() {
    for canonical in [true, false] {
        let text = if canonical {
            "class From; class To; class C { def result(x: From)(using a: Conversion[From, To], b: Conversion[From, To]): To = x }"
        } else {
            "package decoy; class From; class To; class Conversion[A,B] { def apply(x:A):B }; class C { def result(x: From)(using a: Conversion[From, To]): To = x }"
        };
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
        let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "result");
        let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
            panic!()
        };
        let resolver = super::contextual_binary::library_resolver(definitions, &packages);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        )
        .with_resolver(Box::new(resolver));
        let before = typer.store.checkpoint();
        let error = typer.type_method_definition(tree).unwrap_err();
        assert!(
            match error {
                TyperError::AmbiguousContextualArgument { ref candidates, .. } =>
                    canonical && candidates.len() == 2,
                TyperError::ExpectedExpressionTypeMismatch { .. } => !canonical,
                _ => false,
            },
            "{error:?}"
        );
        assert_eq!(typer.store.checkpoint(), before);
        assert!(typer.typed_index.is_empty());
    }
}

#[test]
fn legacy_conversion_respects_nesting_and_conversion_instance_priority() {
    let text = include_str!("../../../tests/fixtures/contextual-consumers/LegacyConversions.scala");
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let resolver = super::contextual_binary::library_resolver(definitions, &packages);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(resolver));
    for (name, target) in [
        ("oldOnly", "old"),
        ("nearest", "inner"),
        ("mixed", "instance"),
    ] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
            panic!()
        };
        typer.type_method_definition(tree).unwrap();
        let root = typer.typed_index.get(source, rhs).unwrap();
        let TreeKind::Apply(app) = &typer.typed_arena.get(root).kind else {
            panic!("missing conversion")
        };
        let converted_by = if target == "old" {
            app.function
        } else {
            let TreeKind::Select(member) = &typer.typed_arena.get(app.function).kind else {
                panic!(
                    "{name} expected Conversion.apply, got {:?}",
                    typer.typed_arena.get(app.function)
                )
            };
            member.qualifier
        };
        let selected = typer
            .store
            .symbols
            .get(typed_reference(&typer, converted_by));
        assert_eq!(typer.store.names.resolve(selected.name.text()), target);
    }
}

#[test]
fn equally_nested_legacy_conversions_report_both_symbols_and_restore_the_store() {
    let text = "class From; class To; class C { implicit def first(x: From): To = new To; implicit def second(x: From): To = new To; def result(x: From): To = x }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "result");
    let candidates: Vec<_> = ["first", "second"]
        .map(|name| method_definition_and_rhs(&parsed, &store, &index, source, name).0)
        .into();
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
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
    let checkpoint = typer.store.checkpoint();
    let error = typer.type_method_definition(tree).unwrap_err();
    assert!(
        matches!(&error,TyperError::AmbiguousContextualArgument{candidates:actual,..} if actual==&candidates),
        "{error:?}"
    );
    assert_eq!(typer.store.checkpoint(), checkpoint);
    assert!(typer.typed_index.is_empty());
}
