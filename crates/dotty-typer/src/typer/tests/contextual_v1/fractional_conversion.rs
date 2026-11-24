use super::*;

fn assert_fractional_division(method: &str) {
    let text = include_str!(
        "../../../../tests/fixtures/contextual-v1/controls/FractionalConversion.scala"
    );
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(text);
    let (symbol, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, method);
    let tree = declaration(&index, symbol);
    let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
        unreachable!()
    };
    let parameter = |clause: usize, pos: usize| {
        index
            .symbol_at(source, def.value_param_clauses[clause][pos])
            .unwrap()
    };
    let (x, y, f) = (parameter(0, 0), parameter(0, 1), parameter(1, 0));
    let t = index.symbol_at(source, def.type_params[0]).unwrap();
    let resolver = audit_resolver(defs, &packages);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(resolver));
    typer.type_method_definition(tree).unwrap();
    let body = typer.source_typed_index().get(source, rhs).unwrap();
    let arena = typer.typed_ast();
    let TreeKind::Apply(div) = &arena.get(body).kind else {
        panic!("division application")
    };
    assert_eq!(
        typer.store.types.get(arena.get(body).ty).reference_symbol(),
        Some(t)
    );
    assert_eq!(div.args.len(), 1);
    assert_eq!(referent(&typer, div.args[0]), y);
    let TreeKind::Select(op) = &arena.get(div.function).kind else {
        panic!("division selection")
    };
    let op_symbol = typer.store.symbols.get(referent(&typer, div.function));
    assert_eq!(typer.store.names.resolve(op.name.text()), "/");
    assert_eq!(typer.store.names.resolve(op_symbol.name.text()), "$div");
    let owner = typer.store.symbols.get(op_symbol.owner.unwrap());
    assert_eq!(
        typer.store.names.resolve(owner.name.text()),
        "Fractional$FractionalOps"
    );
    let TreeKind::Apply(contextual) = &arena.get(op.qualifier).kind else {
        panic!("conversion evidence")
    };
    assert_eq!(contextual.args.len(), 1);
    assert_eq!(referent(&typer, contextual.args[0]), f);
    let TreeKind::Apply(plain) = &arena.get(contextual.function).kind else {
        panic!("conversion operand")
    };
    assert_eq!(plain.args.len(), 1);
    assert_eq!(referent(&typer, plain.args[0]), x);
    let TreeKind::TypeApply(poly) = &arena.get(plain.function).kind else {
        panic!("conversion type application")
    };
    assert_eq!(
        typer
            .store
            .types
            .get(arena.get(poly.args[0]).ty)
            .reference_symbol(),
        Some(t)
    );
    let provider = typer.store.symbols.get(referent(&typer, poly.function));
    assert_eq!(
        typer.store.names.resolve(provider.name.text()),
        "infixFractionalOps"
    );
    assert!(provider.flags.contains(SymbolFlags::IMPLICIT));
    let declaring = typer.store.symbols.get(provider.owner.unwrap());
    assert_eq!(
        typer.store.names.resolve(declaring.name.text()),
        "Fractional$ExtraImplicits"
    );
}

#[test]
#[ignore = "requires the pinned Scala 3.9 runtime jars"]
fn explicit() {
    assert_fractional_division("explicit");
}
