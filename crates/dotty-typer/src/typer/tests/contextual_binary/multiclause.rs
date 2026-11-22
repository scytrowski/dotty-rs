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

fn shared_operands<const N: usize>(
    typer: &SourceTyper<'_>,
    root: TreeId<Typed>,
    methods: [&str; N],
) -> (TreeId<Typed>, [SymbolId; N]) {
    let ast = typer.typed_ast();
    let TreeKind::Block(block) = &ast.get(root).kind else {
        panic!("expected operand bindings")
    };
    assert_eq!(block.stats.len(), N);
    let temporaries = std::array::from_fn(|i| {
        let stat = block.stats[i];
        let TreeKind::ValDef(value) = &ast.get(stat).kind else {
            panic!("operand definition")
        };
        let init = application(ast, value.rhs.unwrap());
        assert!(init.args.is_empty());
        let method = reference(typer, init.function);
        assert!(typer.index.definition_of(method).is_some());
        assert_eq!(
            typer
                .store
                .names
                .resolve(typer.store.symbols.get(method).name.text()),
            methods[i]
        );
        typer.typed_index.synthetic_definition_symbol(stat).unwrap()
    });
    (block.expr, temporaries)
}

#[test]
fn both_defaults_replay_the_same_stable_prefix_in_formal_order() {
    with_client("both", |typer, root, params| {
        let ast = typer.typed_ast();
        let outer = application(ast, root);
        let [y, z] = outer.args.as_slice() else {
            panic!("two defaults")
        };
        assert_getter(typer, outer.function, *y, 2);
        assert_getter(typer, outer.function, *z, 3);
        let x = application(ast, outer.function);
        let seed = application(ast, x.function);
        assert_int(ast, x.args[0], 3);
        assert_eq!(reference(typer, seed.args[0]), params[1]);
    });
}

#[test]
fn both_defaults_share_the_applied_receiver_written_seed_and_argument() {
    with_client("effects", |typer, root, _| {
        let ast = typer.typed_ast();
        let (expr, [receiver, seed, x]) = shared_operands(typer, root, ["reader", "seed", "next"]);
        let outer = application(ast, expr);
        let [y, z] = outer.args.as_slice() else {
            panic!("two defaults")
        };
        assert_getter(typer, outer.function, *y, 2);
        assert_getter(typer, outer.function, *z, 3);
        let ordinary = application(ast, outer.function);
        let contextual = application(ast, ordinary.function);
        let TreeKind::Select(selected) = &ast.get(contextual.function).kind else {
            panic!("fold")
        };
        assert_eq!(contextual.kind, ApplyKind::Using);
        assert_eq!(reference(typer, selected.qualifier), receiver);
        assert_eq!(reference(typer, contextual.args[0]), seed);
        assert_eq!(reference(typer, ordinary.args[0]), x);
    });
}

#[test]
fn empty_clause_is_replayed_before_the_last_contextual_search() {
    with_client("empty", |typer, root, params| {
        let ast = typer.typed_ast();
        let flag = application(ast, root);
        assert_eq!(reference(typer, flag.args[0]), params[2]);
        let ordinary = application(ast, flag.function);
        let [getter] = ordinary.args.as_slice() else {
            panic!("defaulted value")
        };
        assert_getter(typer, ordinary.function, *getter, 2);
        let empty = application(ast, ordinary.function);
        assert!(empty.args.is_empty());
        let x = application(ast, empty.function);
        assert_int(ast, x.args[0], 3);
        let seed = application(ast, x.function);
        assert_eq!(reference(typer, seed.args[0]), params[1]);
    });
}

const ERRORS: &str =
    include_str!("../../../../tests/fixtures/contextual-arguments/ClauseErrors.scala");

#[test]
fn missing_and_ambiguous_searches_report_their_own_clause_and_rollback() {
    for (name, early, missing) in [
        ("missing", true, true),
        ("ambiguous", true, false),
        ("late", false, false),
        ("lateEffects", false, false),
    ] {
        let (parsed, mut store, packages, defs, index, source) = parse_and_name(ERRORS);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, name);
        let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
            panic!()
        };
        let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
            panic!()
        };
        let mut site = rhs;
        if early {
            while let TreeKind::Apply(call) = &parsed.ast.get(site).kind {
                site = call.function;
            }
        }
        let candidates = def
            .value_param_clauses
            .last()
            .unwrap()
            .iter()
            .map(|&p| index.symbol_at(source, p).unwrap())
            .collect::<Vec<_>>();
        let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
            .with_resolver(Box::new(contextual_fixture_resolver(defs, &packages)));
        let before = typer.store.checkpoint();
        let trees = typer.typed_arena.checkpoint();
        for _ in 0..2 {
            let err = typer.type_method_definition(tree).unwrap_err();
            let tree_index = match err {
                TyperError::MissingContextualArgument { tree_index, .. } if missing => tree_index,
                TyperError::AmbiguousContextualArgument {
                    tree_index,
                    candidates: found,
                    ..
                } if !missing => {
                    assert_eq!(found, candidates);
                    tree_index
                }
                other => panic!("{name}: {other:?}"),
            };
            assert_eq!(tree_index, site.index(), "{name}");
            assert_eq!(typer.store.checkpoint(), before);
            assert_eq!(typer.typed_arena.checkpoint(), trees);
            assert!(typer.typed_index.is_empty());
        }
    }
}
