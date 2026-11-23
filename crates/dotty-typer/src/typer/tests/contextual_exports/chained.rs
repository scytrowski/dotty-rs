use super::*;

const CHAIN: &str =
    include_str!("../../../../tests/fixtures/contextual-arguments/ChainedExports.scala");

fn qualified_name<'a>(typer: &'a SourceTyper<'_>, id: SymbolId) -> (&'a str, &'a str) {
    let entry = typer.store.symbols.get(id);
    let owner = typer.store.symbols.get(entry.owner.unwrap());
    (
        typer
            .store
            .names
            .resolve(owner.name.text())
            .trim_end_matches('$'),
        typer.store.names.resolve(entry.name.text()),
    )
}

fn stable_delegate(typer: &mut SourceTyper<'_>, method: SymbolId) -> SymbolId {
    let tree = typer.type_export_forwarder_definition(method).unwrap();
    assert_eq!(
        tree,
        typer.type_export_forwarder_definition(method).unwrap()
    );
    let TreeKind::DefDef(def) = &typer.typed_ast().get(tree).kind else {
        panic!("forwarding method")
    };
    assert!(def.type_params.is_empty() && def.value_param_clauses.is_empty());
    let rhs = def.rhs.unwrap();
    let TreeKind::Select(sel) = &typer.typed_ast().get(rhs).kind else {
        panic!("qualified delegation")
    };
    let target = reference(typer, rhs);
    assert_eq!(
        typer
            .module_class_of_object(reference(typer, sel.qualifier))
            .unwrap(),
        typer.store.symbols.owner(target).unwrap()
    );
    assert_eq!(
        typer.store.export_target(method),
        Some(typer.typed_ast().get(rhs).ty)
    );
    assert_eq!(
        typer.store.symbols.info(method),
        &SymbolInfo::Complete(typer.typed_ast().get(rhs).ty)
    );
    target
}

#[test]
fn renamed_reexport_delegates_through_the_intermediate_stable_method() {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(CHAIN);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let tree = client_result(&mut typer, &parsed, "named").unwrap();
    let outer = reference(&typer, tree);
    assert_eq!(qualified_name(&typer, outer), ("Renamed", "selected"));
    let middle = stable_delegate(&mut typer, outer);
    let control = client_result(&mut typer, &parsed, "control").unwrap();
    assert_eq!(reference(&typer, control), middle);
    assert_eq!(qualified_name(&typer, middle), ("Middle", "first"));
    let original = stable_delegate(&mut typer, middle);
    assert_eq!(
        original,
        val_symbol(&parsed, typer.store(), &index, source, "ints").0
    );
}
