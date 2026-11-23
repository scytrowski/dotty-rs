use super::*;

const LIB: &str =
    include_str!("../../../../tests/fixtures/contextual-arguments/InheritedExports.scala");

fn stable_path(typer: &mut SourceTyper<'_>, symbol: SymbolId) -> (SymbolId, SymbolId) {
    let tree = typer.type_export_forwarder_definition(symbol).unwrap();
    let TreeKind::DefDef(def) = &typer.typed_ast().get(tree).kind else {
        panic!("exported method")
    };
    let rhs = def.rhs.unwrap();
    let TreeKind::Select(sel) = &typer.typed_ast().get(rhs).kind else {
        panic!("stable delegation")
    };
    assert_eq!(
        typer.store.symbols.info(symbol),
        &SymbolInfo::Complete(typer.typed_ast().get(rhs).ty)
    );
    (reference(typer, sel.qualifier), reference(typer, rhs))
}

#[test]
fn named_inherited_export_preserves_receiver_and_original_declaration() {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(LIB);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let call = client_result(&mut typer, &parsed, "named").unwrap();
    let alias = reference(&typer, call);
    assert_eq!(qualified_name(&typer, alias), ("Named", "selected"));
    let (receiver, original) = stable_path(&mut typer, alias);
    assert_eq!(qualified_name(&typer, receiver).1, "Ints");
    assert_eq!(qualified_name(&typer, original), ("Base", "key"));
    let tree = client_result(&mut typer, &parsed, "direct").unwrap();
    assert_eq!(reference(&typer, tree), original);
    let call = client_result(&mut typer, &parsed, "directSearch").unwrap();
    assert_eq!(reference(&typer, evidence(&typer, call)), original);
}
