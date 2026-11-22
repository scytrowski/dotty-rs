use super::*;
use dotty_core::ast::Apply;

const CLIENT: &str =
    include_str!("../../../../tests/fixtures/contextual-arguments/ClauseClient.scala");

fn with_client(name: &str, check: impl FnOnce(&SourceTyper<'_>, TreeId<Typed>, &[SymbolId])) {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(CLIENT);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, name);
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!("client method")
    };
    let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
        panic!("client method")
    };
    let parameters = def
        .value_param_clauses
        .iter()
        .flatten()
        .map(|&p| index.symbol_at(source, p).unwrap())
        .collect::<Vec<_>>();
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(contextual_fixture_resolver(defs, &packages)));
    typer
        .type_method_definition(tree)
        .unwrap_or_else(|e| panic!("{name}: {e:?}"));
    check(
        &typer,
        typer.typed_index.get(source, rhs).unwrap(),
        &parameters,
    );
}

fn application(ast: &AstArena<Typed>, tree: TreeId<Typed>) -> &Apply<Typed> {
    let TreeKind::Apply(call) = &ast.get(tree).kind else {
        panic!("expected application")
    };
    call
}

fn reference(typer: &SourceTyper<'_>, tree: TreeId<Typed>) -> SymbolId {
    typer
        .store
        .types
        .get(typer.typed_ast().get(tree).ty)
        .reference_symbol()
        .expect("term symbol")
}

fn assert_int(ast: &AstArena<Typed>, tree: TreeId<Typed>, value: i32) {
    assert!(
        matches!(&ast.get(tree).kind, TreeKind::Literal(lit) if lit.value == dotty_core::Constant::Int(value))
    );
}

// Each replayed application must reuse both the clause kind and the exact argument trees.
fn assert_getter(
    typer: &SourceTyper<'_>,
    mut target: TreeId<Typed>,
    mut getter: TreeId<Typed>,
    ordinal: usize,
) {
    let ast = typer.typed_ast();
    while let TreeKind::Apply(call) = &ast.get(target).kind {
        let copy = application(ast, getter);
        assert_eq!((copy.kind, &copy.args), (call.kind, &call.args));
        (target, getter) = (call.function, copy.function);
    }
    let (TreeKind::Select(method), TreeKind::Select(accessor)) =
        (&ast.get(target).kind, &ast.get(getter).kind)
    else {
        panic!("expected binary member selections")
    };
    assert_eq!(method.qualifier, accessor.qualifier);
    assert_eq!(
        typer
            .store
            .default_getter(reference(typer, target), ordinal),
        Some(reference(typer, getter))
    );
}

#[test]
fn partial_and_full_clauses_keep_the_written_values_and_the_selected_seed() {
    for name in ["partial", "supplied"] {
        with_client(name, |typer, root, params| {
            let ast = typer.typed_ast();
            let outer = application(ast, root);
            let x = application(ast, outer.function);
            let seed = application(ast, x.function);
            let TreeKind::Select(fold) = &ast.get(seed.function).kind else {
                panic!("fold")
            };
            assert_eq!(reference(typer, fold.qualifier), params[0]);
            assert_eq!(reference(typer, seed.args[0]), params[1]);
            assert_int(ast, x.args[0], 3);
            let [y, z] = outer.args.as_slice() else {
                panic!("expected both parameters")
            };
            assert_int(ast, *y, 7);
            if name == "partial" {
                assert_getter(typer, outer.function, *z, 3);
            } else {
                assert_int(ast, *z, 8);
            }
        });
    }
}
