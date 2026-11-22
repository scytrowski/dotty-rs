use super::*;

const LIBRARY: &str =
    include_str!("../../../tests/fixtures/contextual-arguments/ExportLibrary.scala");
const CLIENT: &str =
    include_str!("../../../tests/fixtures/contextual-arguments/ExportClient.scala");

fn unit(client: &str) -> NamedTestUnit {
    parse_and_name(&format!(
        "{LIBRARY}\n{}",
        client.strip_prefix("package exportsprint\n").unwrap()
    ))
}

fn reference(typer: &SourceTyper<'_>, tree: TreeId<Typed>) -> SymbolId {
    typer
        .store
        .types
        .get(typer.typed_ast().get(tree).ty)
        .reference_symbol()
        .unwrap()
}

#[test]
fn named_export_has_its_own_identity_and_delegates_to_the_original_given() {
    let (parsed, mut store, packages, defs, index, source) = unit(CLIENT);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "named");
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!()
    };
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    typer.type_method_definition(tree).unwrap();
    let selected = typer.typed_index.get(source, rhs).unwrap();
    let forwarder = reference(&typer, selected);
    let TreeKind::Select(sel) = &typer.typed_ast().get(selected).kind else {
        panic!()
    };
    let owner = typer
        .module_class_of_object(reference(&typer, sel.qualifier))
        .unwrap();
    assert_eq!(typer.store.symbols.owner(forwarder), Some(owner));
    assert!(
        typer
            .store
            .symbols
            .get(forwarder)
            .flags
            .contains(SymbolFlags::EXPORTED | SymbolFlags::GIVEN)
    );
    let definition = typer.type_export_forwarder_definition(forwarder).unwrap();
    assert_eq!(
        typer.type_export_forwarder_definition(forwarder).unwrap(),
        definition
    );
    let TreeKind::DefDef(def) = &typer.typed_ast().get(definition).kind else {
        panic!("export definition")
    };
    let body = def.rhs.unwrap();
    let TreeKind::Select(delegate) = &typer.typed_ast().get(body).kind else {
        panic!("delegate selection")
    };
    let original = reference(&typer, body);
    assert_eq!(
        original,
        val_symbol(&parsed, typer.store(), &index, source, "ints").0
    );
    assert_ne!(original, forwarder);
    assert_eq!(
        typer.store.symbols.owner(original),
        Some(
            typer
                .module_class_of_object(reference(&typer, delegate.qualifier))
                .unwrap()
        )
    );
    assert_eq!(
        typer.store.symbols.info(forwarder),
        &SymbolInfo::Complete(typer.typed_ast().get(body).ty)
    );
    let declared = typer.store.symbols.get(forwarder).name;
    typer.complete_symbol(owner).unwrap();
    assert_eq!(
        typer
            .store
            .scopes
            .get(index.scope_of(owner).unwrap())
            .lookup_all(&declared),
        &[forwarder]
    );
}

#[test]
fn failed_export_use_removes_its_entry_from_the_existing_module_scope() {
    let (parsed, mut store, packages, defs, index, source) =
        unit("package exportsprint\nclass Bad { def bad: Boolean = Named.ints }");
    let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "bad");
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!()
    };
    let scope = parsed
        .ast
        .iter()
        .find_map(|(tree, _)| index.export_site_at(source, tree))
        .and_then(|site| index.scope_of(site.owner))
        .unwrap();
    let name = *dotty_core::TermName::new(store.names.intern("ints")).as_name();
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let before = typer.store.checkpoint();
    for _ in 0..2 {
        assert!(matches!(
            typer.type_method_definition(tree),
            Err(TyperError::ExpectedExpressionTypeMismatch { .. })
        ));
        assert_eq!(typer.store.checkpoint(), before);
        assert!(typer.store.scopes.get(scope).lookup(&name).is_none());
    }
}
