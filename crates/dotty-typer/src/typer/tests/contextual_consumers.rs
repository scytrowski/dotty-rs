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
