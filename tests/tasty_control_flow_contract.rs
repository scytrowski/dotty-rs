use dotty_core::ast::*;
use dotty_core::*;
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_typer::SourceTyper;

const DECISIONS: &str = include_str!("fixtures/tasty_pickler/Decisions.scala");
const BLOCKS: &str = include_str!("fixtures/tasty_pickler/Blocks.scala");

fn with_source<R>(
    source_text: &str,
    f: impl FnOnce(
        &AstArena<Untyped>,
        SourceId,
        &SourceSemanticIndex,
        &Definitions,
        &mut SourceTyper<'_>,
    ) -> R,
) -> R {
    let source = SourceId::from_index(1309);
    let scanner = ContextualScanner::new(source_text).unwrap();
    assert!(scanner.diagnostics().is_empty());
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source,
        "ControlFlow.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } => {
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
            index
        }
        NamingOutcome::Rejected { diagnostics } => {
            panic!("namer rejected fixture: {diagnostics:?}")
        }
    };
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    f(&parsed.ast, source, &index, &definitions, &mut typer)
}

fn method<'a>(
    ast: &'a AstArena<Untyped>,
    index: &SourceSemanticIndex,
    source: SourceId,
    store: &SemanticStore,
    name: &str,
) -> (TreeId<Untyped>, SymbolId, &'a DefDef<Untyped>) {
    ast.iter()
        .find_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == name)
                .then(|| (tree, index.symbol_at(source, tree).unwrap(), definition))
        })
        .unwrap_or_else(|| panic!("missing method {name}"))
}

fn term_ref_symbol(store: &SemanticStore, ast: &AstArena<Typed>, tree: TreeId<Typed>) -> SymbolId {
    let Type::TermRef {
        target: TermRefTarget::Symbol(symbol),
        ..
    } = store.types.get(ast.get(tree).ty)
    else {
        panic!("typed reference should retain its canonical term symbol")
    };
    *symbol
}

#[test]
fn control_flow_source_types_and_maps_to_typed_trees() {
    with_source(DECISIONS, |ast, source, index, definitions, typer| {
        let mut source_method_trees = Vec::new();
        for (name, result) in [
            ("unit", definitions.unit),
            ("enabled", definitions.boolean),
            ("disabled", definitions.boolean),
            ("choose", definitions.int),
            ("nested", definitions.int),
            ("stored", definitions.int),
        ] {
            let (_, symbol, definition) = method(ast, index, source, typer.store(), name);
            assert_eq!(typer.store().symbols.get(symbol).kind, SymbolKind::Method);
            assert!(definition.rhs.is_some(), "{name} has a source body");
            let signature = typer.complete_symbol(symbol).unwrap();
            let formal_symbols = definition
                .value_param_clauses
                .iter()
                .flatten()
                .map(|tree| {
                    index
                        .symbol_at(source, *tree)
                        .expect("formal has canonical symbol")
                })
                .collect::<Vec<_>>();
            assert!(formal_symbols.iter().all(|formal| {
                typer.store().symbols.get(*formal).owner == Some(symbol)
                    && typer.store().symbols.get(*formal).kind == SymbolKind::Parameter
            }));
            let parameter_type = if matches!(name, "unit" | "enabled" | "disabled") {
                None
            } else if name == "choose" {
                Some(vec![definitions.boolean, definitions.int, definitions.int])
            } else if name == "nested" {
                Some(vec![definitions.boolean, definitions.boolean])
            } else {
                Some(vec![definitions.boolean, definitions.int])
            };
            if let Some(expected) = parameter_type {
                let Type::Method(signature) = typer.store().types.get(signature) else {
                    panic!("{name} must have a method type");
                };
                assert_eq!(
                    signature.params.iter().map(|p| p.ty).collect::<Vec<_>>(),
                    expected
                );
                assert_eq!(signature.params.len(), formal_symbols.len());
                assert_eq!(signature.result, result);
            } else {
                assert_eq!(signature, result);
            }
            let context = typer.expression_context_for(symbol).unwrap();
            let rhs = definition.rhs.unwrap();
            let typed = typer
                .type_expression_expected(rhs, context, result)
                .unwrap();
            assert_eq!(typer.source_typed_index().get(source, rhs), Some(typed));
            source_method_trees.push((name, rhs, typed));
        }

        for (name, rhs, typed) in &source_method_trees {
            let typed_tree = typer.typed_ast().get(*typed);
            match name {
                &"unit" => {
                    assert!(matches!(
                        typed_tree.kind,
                        TreeKind::Literal(Literal {
                            value: Constant::Unit
                        })
                    ));
                    assert!(matches!(
                        typer.store().types.get(typed_tree.ty),
                        Type::Constant(Constant::Unit)
                    ));
                }
                &"enabled" | &"disabled" => {
                    assert!(matches!(
                        typed_tree.kind,
                        TreeKind::Literal(Literal {
                            value: Constant::Boolean(_),
                            ..
                        })
                    ));
                    assert!(matches!(
                        typer.store().types.get(typed_tree.ty),
                        Type::Constant(Constant::Boolean(_))
                    ));
                }
                &"choose" => {
                    let (_, symbol, definition) =
                        method(ast, index, source, typer.store(), "choose");
                    let [flag, a, b] = definition.value_param_clauses[0].as_slice() else {
                        panic!("choose must have its three formal parameters");
                    };
                    let [flag_symbol, a_symbol, b_symbol] =
                        [flag, a, b].map(|tree| index.symbol_at(source, *tree).unwrap());
                    let TreeKind::If(source_if) = ast.get(*rhs).kind else {
                        panic!()
                    };
                    let TreeKind::If(typed_if) = typed_tree.kind else {
                        panic!()
                    };
                    assert_eq!(
                        typer.source_typed_index().get(source, source_if.cond),
                        Some(typed_if.cond)
                    );
                    assert_eq!(
                        term_ref_symbol(typer.store(), typer.typed_ast(), typed_if.cond),
                        flag_symbol
                    );
                    assert_eq!(
                        term_ref_symbol(typer.store(), typer.typed_ast(), typed_if.then_branch),
                        a_symbol
                    );
                    assert_eq!(
                        term_ref_symbol(typer.store(), typer.typed_ast(), typed_if.else_branch),
                        b_symbol
                    );
                    assert_eq!(typer.store().symbols.get(symbol).kind, SymbolKind::Method);
                }
                &"nested" => {
                    let TreeKind::If(source_outer) = ast.get(*rhs).kind else {
                        panic!("nested source body must be source If");
                    };
                    let inner_source = source_outer.then_branch;
                    let inner_typed = typer
                        .source_typed_index()
                        .get(source, inner_source)
                        .expect("nested source If is mapped independently");
                    let TreeKind::If(outer) = typed_tree.kind else {
                        panic!("nested source body must type as If");
                    };
                    assert_eq!(outer.then_branch, inner_typed);
                    assert!(matches!(
                        typer.typed_ast().get(inner_typed).kind,
                        TreeKind::If(_)
                    ));
                    assert_eq!(typer.typed_ast().get(inner_typed).ty, definitions.int);
                    assert_eq!(typer.typed_ast().get(*typed).ty, definitions.int);
                }
                &"stored" => assert!(matches!(typed_tree.kind, TreeKind::Block(_))),
                _ => unreachable!(),
            }
            assert!(typer.source_typed_index().get(source, *rhs).is_some());
        }

        let (_, stored_symbol, stored) = method(ast, index, source, typer.store(), "stored");
        let TreeKind::Block(block) = &ast.get(stored.rhs.unwrap()).kind else {
            panic!("stored source body must be a block");
        };
        let [local_tree] = block.stats.as_slice() else {
            panic!("stored block should expose one local val");
        };
        let binding = typer
            .typed_local_binding_at(source, *local_tree)
            .expect("ordinary local val has typed identity");
        assert_eq!(
            typer.source_typed_index().get(source, *local_tree),
            Some(binding.declaration())
        );
        assert!(matches!(
            typer.typed_ast().get(binding.declaration()).kind,
            TreeKind::ValDef(_)
        ));
        assert_eq!(
            typer.local_symbol_at(source, *local_tree),
            Some(binding.symbol())
        );
        assert_eq!(
            typer.store().symbols.get(binding.symbol()).kind,
            SymbolKind::Local
        );
        assert_eq!(
            typer.store().symbols.get(binding.symbol()).owner,
            Some(stored_symbol)
        );
        let TreeKind::Block(typed_block) = &typer
            .typed_ast()
            .get(
                typer
                    .source_typed_index()
                    .get(source, stored.rhs.unwrap())
                    .unwrap(),
            )
            .kind
        else {
            panic!()
        };
        assert_eq!(typed_block.stats.len(), 1);
        assert!(matches!(
            typer.typed_ast().get(typed_block.stats[0]).kind,
            TreeKind::ValDef(_)
        ));
        let saved_reference = match &typer.typed_ast().get(typed_block.expr).kind {
            TreeKind::If(if_tree) => if_tree.then_branch,
            _ => panic!("stored block result must be If"),
        };
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(saved_reference).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == binding.symbol()
        ));
    });

    with_source(BLOCKS, |ast, source, index, definitions, typer| {
        let (_, symbol, definition) = method(ast, index, source, typer.store(), "ordered");
        typer.complete_symbol(symbol).unwrap();
        let rhs = definition.rhs.unwrap();
        let context = typer.expression_context_for(symbol).unwrap();
        let _typed = typer
            .type_expression_expected(rhs, context, definitions.int)
            .unwrap();
        let TreeKind::Block(source_block) = &ast.get(rhs).kind else {
            panic!("ordered body must be source block");
        };
        assert_eq!(source_block.stats.len(), 1);
        let TreeKind::If(_) = ast.get(source_block.stats[0]).kind else {
            panic!("discarded conditional remains an ordered statement");
        };
        let typed_id = typer.source_typed_index().get(source, rhs).unwrap();
        let TreeKind::Block(typed_block) = &typer.typed_ast().get(typed_id).kind else {
            panic!("ordered body must type as block");
        };
        assert_eq!(typed_block.stats.len(), 1);
        assert!(matches!(
            typer.typed_ast().get(typed_block.stats[0]).kind,
            TreeKind::If(_)
        ));
        let stat_typed = typer
            .source_typed_index()
            .get(source, source_block.stats[0])
            .unwrap();
        assert!(matches!(
            typer.typed_ast().get(stat_typed).kind,
            TreeKind::If(_)
        ));
        assert_eq!(typer.typed_ast().get(stat_typed).ty, definitions.int);
    });
}
