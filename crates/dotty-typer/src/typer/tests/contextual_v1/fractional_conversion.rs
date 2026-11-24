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
    // The implicit clause after (x) records the inferred argument even
    // when an ordinary inferred application has no source TypeApply tree.
    let Type::Method(clause) = typer.store.types.get(arena.get(contextual.function).ty) else {
        panic!("instantiated evidence clause")
    };
    let Type::Applied { args, .. } = typer.store.types.get(clause.params[0].ty) else {
        panic!("Fractional[T]")
    };
    assert_eq!(typer.store.types.get(args[0]).reference_symbol(), Some(t));
    let function = match &arena.get(plain.function).kind {
        TreeKind::TypeApply(poly) => {
            assert_eq!(
                typer
                    .store
                    .types
                    .get(arena.get(poly.args[0]).ty)
                    .reference_symbol(),
                Some(t)
            );
            poly.function
        }
        TreeKind::Select(_) if method == "inferred" => plain.function,
        kind => panic!("unexpected conversion callee in {method}: {kind:?}"),
    };
    let provider = typer.store.symbols.get(referent(&typer, function));
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

#[test]
#[ignore = "requires the pinned Scala 3.9 runtime jars"]
fn omitted() {
    for method in ["searched", "inferred"] {
        assert_fractional_division(method);
    }
}

#[test]
#[ignore = "requires the pinned Scala 3.9 runtime jars"]
fn missing_and_ambiguous() {
    let text = include_str!(
        "../../../../tests/fixtures/contextual-v1/controls/FractionalConversionErrors.scala"
    );
    for method in ["missing", "ambiguous"] {
        let (parsed, mut store, packages, defs, index, source) = parse_and_name(text);
        let (symbol, _) = method_definition_and_rhs(&parsed, &store, &index, source, method);
        let tree = declaration(&index, symbol);
        let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
            unreachable!()
        };
        let TreeKind::Apply(div) = &parsed.ast.get(def.rhs.unwrap()).kind else {
            unreachable!()
        };
        let TreeKind::Select(op) = &parsed.ast.get(div.function).kind else {
            unreachable!()
        };
        let site = op.qualifier.index();
        let resolver = audit_resolver(defs, &packages);
        let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
            .with_resolver(Box::new(resolver));
        let error = typer.type_method_definition(tree).unwrap_err();
        if method == "missing" {
            assert!(
                matches!(error, TyperError::MissingContextualArgument {source: id, tree_index, ..} if id == source && tree_index == site),
                "{error:?}"
            );
        } else {
            let TyperError::AmbiguousContextualArgument {
                source: id,
                tree_index,
                candidates,
                ..
            } = error
            else {
                panic!("{error:?}")
            };
            assert_eq!((id, tree_index), (source, site));
            let expected = def.value_param_clauses[1]
                .iter()
                .map(|&p| index.symbol_at(source, p).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(candidates.len(), 2);
            assert!(expected.iter().all(|p| candidates.contains(p)));
        }
    }
}
