use super::*;

fn definition(index: &SourceSemanticIndex, method: SymbolId) -> TreeId<Untyped> {
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!("expected source method")
    };
    tree
}

// Check both the selected declaration and this use's instantiated receiver type.
fn unary_context_call(
    typer: &SourceTyper<'_>,
    call: TreeId<Typed>,
    kind: ApplyKind,
    input: TypeId,
) -> (TreeId<Typed>, TreeId<Typed>, SymbolId) {
    let ast = typer.typed_ast();
    let store = typer.store();
    let node = ast.get(call);
    let TreeKind::Apply(app) = &node.kind else {
        panic!("expected contextual application: {node:?}")
    };
    assert_eq!(app.kind, kind);
    let [argument] = app.args.as_slice() else {
        panic!("expected unary context function")
    };
    let selection = ast.get(app.function);
    let TreeKind::Select(member) = &selection.kind else {
        panic!("expected selection of ContextFunction1.apply")
    };
    let Type::TermRef {
        prefix,
        target: TermRefTarget::Symbol(apply),
    } = store.types.get(selection.ty)
    else {
        panic!("expected apply reference")
    };
    let Type::Applied { tycon, args } = store.types.get(*prefix) else {
        panic!("expected instantiated ContextFunction1 receiver")
    };
    let class = store.types.get(*tycon).reference_symbol().unwrap();
    assert_eq!(typer.synthesized_context_function_arity(class), Some(1));
    assert_eq!(store.symbols.owner(*apply), Some(class));
    assert_eq!(
        store.names.resolve(store.symbols.get(*apply).name.text()),
        "apply"
    );
    assert_eq!(args, &[input, node.ty]);
    (member.qualifier, *argument, *apply)
}

#[test]
fn nested_context_client_applies_each_layer_with_its_own_evidence() {
    let (parsed, mut store, mut packages, defs, index, source) = parse_and_name(include_str!(
        "../../../tests/fixtures/contextual-arguments/NestedContextClient.scala"
    ));
    let read = method_symbol(&parsed, &store, &index, source, "read");
    let build = method_symbol(&parsed, &store, &index, source, "build");
    packages.enter(&mut store, SymbolOrigin::Synthetic, &["scala"]);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for method in [read, build] {
        typer
            .type_method_definition(definition(&index, method))
            .unwrap();
    }
    for name in ["explicit", "inferred", "mixed"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        typer
            .type_method_definition(definition(&index, method))
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let (value, number, flag) = match name {
            "inferred" => {
                let TreeKind::Block(block) = &parsed.ast.get(rhs).kind else {
                    panic!("expected block with local givens")
                };
                let [number, flag] = block.stats.as_slice() else {
                    panic!("expected two local givens")
                };
                (
                    block.expr,
                    Some(typer.local_symbol_at(source, *number).unwrap()),
                    Some(typer.local_symbol_at(source, *flag).unwrap()),
                )
            }
            "mixed" => (
                rhs,
                None,
                Some(method_parameter_symbol(&parsed, &index, source, method, 0)),
            ),
            _ => (rhs, None, None),
        };
        let mut call = typer.source_typed_index().get(source, value).unwrap();
        let ast = typer.typed_ast();
        let reference = |tree| typer.store().types.get(ast.get(tree).ty).reference_symbol();
        assert_eq!(ast.get(call).ty, defs.int);
        let mut selected = None;
        for (input, evidence, explicit) in [
            (defs.boolean, flag, dotty_core::Constant::Boolean(true)),
            (defs.int, number, dotty_core::Constant::Int(7)),
        ] {
            let kind = if evidence.is_some() {
                ApplyKind::Regular
            } else {
                ApplyKind::Using
            };
            let (receiver, argument, apply) = unary_context_call(&typer, call, kind, input);
            if let Some(first) = selected {
                assert_eq!(first, apply);
            }
            selected = Some(apply);
            if let Some(given) = evidence {
                assert_eq!(reference(argument), Some(given));
            } else {
                assert!(
                    matches!(&ast.get(argument).kind, TreeKind::Literal(lit) if lit.value == explicit)
                );
            }
            call = receiver;
        }
        assert_eq!(reference(call), Some(build));
    }
}

#[test]
fn nested_context_client_reports_second_layer_search_errors_atomically() {
    let (parsed, mut store, mut packages, defs, index, source) = parse_and_name(include_str!(
        "../../../tests/fixtures/contextual-arguments/NestedContextClientErrors.scala"
    ));
    packages.enter(&mut store, SymbolOrigin::Synthetic, &["scala"]);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for name in ["missing", "ambiguous"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let checkpoint = typer.store().checkpoint();
        for _ in 0..2 {
            let result = typer.type_method_definition(definition(&index, method));
            match result {
                Err(TyperError::MissingContextualArgument {
                    expected,
                    tree_index,
                    ..
                }) if name == "missing" => {
                    assert_eq!((expected, tree_index), (defs.boolean, rhs.index()));
                }
                Err(TyperError::AmbiguousContextualArgument {
                    expected,
                    tree_index,
                    candidates,
                    ..
                }) if name == "ambiguous" => {
                    assert_eq!((expected, tree_index), (defs.boolean, rhs.index()));
                    assert_eq!(
                        candidates,
                        [1, 2].map(|i| method_parameter_symbol(&parsed, &index, source, method, i))
                    );
                }
                other => panic!("{name}: {other:?}"),
            }
            assert_eq!(typer.store().checkpoint(), checkpoint);
            assert!(typer.source_typed_index().is_empty());
            assert_eq!(typer.typed_ast().iter().count(), 0);
        }
    }
}

#[test]
fn nested_context_client_keeps_a_partially_applied_result() {
    let (parsed, mut store, mut packages, defs, index, source) = parse_and_name(include_str!(
        "../../../tests/fixtures/contextual-arguments/NestedContextClient.scala"
    ));
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "partial");
    let build = method_symbol(&parsed, &store, &index, source, "build");
    packages.enter(&mut store, SymbolOrigin::Synthetic, &["scala"]);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    typer
        .type_method_definition(definition(&index, method))
        .unwrap();
    let root = typer.source_typed_index().get(source, rhs).unwrap();
    let ast = typer.typed_ast();
    let TreeKind::Closure(closure) = &ast.get(root).kind else {
        panic!("expected Boolean ?=> Int closure")
    };
    let implementation = closure.implementation.as_ref().unwrap();
    let TreeKind::DefDef(method) = &ast.get(implementation.definition).kind else {
        panic!("expected synthetic method")
    };
    let (first_call, flag, _) = unary_context_call(
        &typer,
        method.rhs.unwrap(),
        ApplyKind::Regular,
        defs.boolean,
    );
    assert_eq!(
        typer.store().types.get(ast.get(flag).ty).reference_symbol(),
        Some(implementation.parameters[0])
    );
    let (base, number, _) = unary_context_call(&typer, first_call, ApplyKind::Using, defs.int);
    assert_eq!(
        typer.store().types.get(ast.get(base).ty).reference_symbol(),
        Some(build)
    );
    assert!(
        matches!(&ast.get(number).kind, TreeKind::Literal(lit) if lit.value == dotty_core::Constant::Int(7))
    );
}

#[test]
fn nested_context_client_rejects_cyclic_context_results_atomically() {
    let (parsed, mut store, mut packages, defs, index, source) = parse_and_name(
        "package clients; abstract class Client { def build: Int ?=> Int; def value(using n: Int): Int = build }",
    );
    let build = method_symbol(&parsed, &store, &index, source, "build");
    let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "value");
    packages.enter(&mut store, SymbolOrigin::Synthetic, &["scala"]);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let signature = typer.complete_symbol(build).unwrap();
    let Type::Applied { tycon, .. } = *typer.store.types.get(signature) else {
        panic!("expected context function")
    };
    let slot = typer.store.types.reserve();
    let cycle = slot.id();
    typer.store.types.fill(
        slot,
        Type::Applied {
            tycon,
            args: vec![defs.int, cycle],
        },
    );
    typer
        .store
        .symbols
        .set_info(build, SymbolInfo::Complete(cycle));
    let checkpoint = typer.store().checkpoint();
    let error = typer
        .type_method_definition(definition(&index, method))
        .unwrap_err();
    assert!(
        matches!(
            error,
            TyperError::TypeNormalization(crate::types::TypeNormalizeError::NormalizationCycle { ty }) if ty == cycle
        ),
        "{error:?}"
    );
    assert_eq!(typer.store().checkpoint(), checkpoint);
    assert!(typer.source_typed_index().is_empty());
    assert_eq!(typer.typed_ast().iter().count(), 0);
}
