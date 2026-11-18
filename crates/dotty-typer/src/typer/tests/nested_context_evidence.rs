//! The two implicit ContextFunction1 methods reference their own ordered
//! formals, with free receiver and evidence values available at closure creation.

use super::*;

struct Layer {
    method: SymbolId,
    parameter: SymbolId,
    body: TreeId<Typed>,
    captures: Vec<SymbolId>,
}

fn referenced(typer: &SourceTyper<'_>, tree: TreeId<Typed>) -> SymbolId {
    match typer.store().types.get(typer.typed_ast().get(tree).ty) {
        Type::ThisType { class } => *class,
        Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        } => *symbol,
        ty => panic!("expected a capture or a method reference, got {ty:?}"),
    }
}

fn layer(typer: &SourceTyper<'_>, tree: TreeId<Typed>) -> Layer {
    let TreeKind::Closure(closure) = &typer.typed_ast().get(tree).kind else {
        panic!("expected closure")
    };
    let method = referenced(typer, closure.method);
    let implementation = closure
        .implementation
        .as_ref()
        .expect("indexed closure implementation");
    let [parameter] = implementation.parameters.as_slice() else {
        panic!("expected unary context function")
    };
    assert_eq!(typer.store().symbols.owner(*parameter), Some(method));
    let TreeKind::DefDef(def) = &typer.typed_ast().get(implementation.definition).kind else {
        panic!("synthetic method")
    };
    assert_eq!(
        typer
            .source_typed_index()
            .synthetic_definition_symbol(implementation.definition),
        Some(method)
    );
    assert_eq!(
        typer
            .source_typed_index()
            .synthetic_definition_symbol(def.value_param_clauses[0][0]),
        Some(*parameter)
    );
    Layer {
        method,
        parameter: *parameter,
        body: def.rhs.unwrap(),
        captures: closure.env.iter().map(|t| referenced(typer, *t)).collect(),
    }
}

fn method_tree(index: &SourceSemanticIndex, method: SymbolId) -> TreeId<Untyped> {
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!("source method")
    };
    tree
}

fn check_call(
    typer: &SourceTyper<'_>,
    tree: TreeId<Typed>,
    method: SymbolId,
    arguments: &[SymbolId],
) {
    let TreeKind::Apply(call) = &typer.typed_ast().get(tree).kind else {
        panic!("contextual call")
    };
    assert_eq!(referenced(typer, call.function), method);
    let refs: Vec<_> = call.args.iter().map(|t| referenced(typer, *t)).collect();
    assert_eq!(refs, arguments);
}

#[test]
fn nested_context_evidence_captures_the_receiver_and_only_the_free_formal() {
    let (parsed, mut store, mut packages, defs, index, source) = parse_and_name(include_str!(
        "../../../tests/fixtures/contextual-arguments/NestedEvidence.scala"
    ));
    packages.enter(&mut store, SymbolOrigin::Synthetic, &["scala"]);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for (name, target) in [("nested", "read"), ("same", "readOne")] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let target = method_symbol(&parsed, typer.store(), &index, source, target);
        let class = typer.store().symbols.owner(method).unwrap();
        typer
            .type_method_definition(method_tree(&index, method))
            .unwrap();
        let outer = layer(&typer, typer.source_typed_index().get(source, rhs).unwrap());
        let inner = layer(&typer, outer.body);
        assert_eq!(
            typer.store().symbols.owner(inner.method),
            Some(outer.method)
        );
        assert_eq!(typer.store().symbols.owner(outer.method), Some(method));
        assert_ne!(outer.parameter, inner.parameter);
        assert_eq!(outer.captures, [class]);
        if name == "nested" {
            assert_eq!(inner.captures, [class, outer.parameter]);
            check_call(
                &typer,
                inner.body,
                target,
                &[outer.parameter, inner.parameter],
            );
        } else {
            assert_eq!(inner.captures, [class]);
            check_call(&typer, inner.body, target, &[inner.parameter]);
        }
    }
}

#[test]
fn nested_closures_forward_an_enclosing_method_parameter() {
    let (parsed, mut store, mut packages, defs, index, source) = parse_and_name(include_str!(
        "../../../tests/fixtures/contextual-arguments/NestedEvidence.scala"
    ));
    packages.enter(&mut store, SymbolOrigin::Synthetic, &["scala"]);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "fromPlain");
    let x = method_parameter_symbol(&parsed, &index, source, method, 0);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    typer
        .type_method_definition(method_tree(&index, method))
        .unwrap();
    let outer = layer(&typer, typer.source_typed_index().get(source, rhs).unwrap());
    let inner = layer(&typer, outer.body);
    assert_eq!(inner.captures, [x]);
    assert_eq!(outer.captures, [x]);
    assert_eq!(referenced(&typer, inner.body), x);
}

#[test]
fn missing_third_evidence_rolls_back_both_layers_and_allows_sibling_retry() {
    let (parsed, mut store, mut packages, defs, index, source) = parse_and_name(include_str!(
        "../../../tests/fixtures/contextual-arguments/NestedEvidenceMissing.scala"
    ));
    packages.enter(&mut store, SymbolOrigin::Synthetic, &["scala"]);
    let (missing, bad_rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "missing");
    let (recovered, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "recovered");
    let read = method_symbol(&parsed, &store, &index, source, "read");
    let supplied = method_parameter_symbol(&parsed, &index, source, recovered, 0);
    let class = store.symbols.owner(recovered).unwrap();
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let checkpoint = typer.store().checkpoint();
    for _ in 0..2 {
        let error = typer
            .type_method_definition(method_tree(&index, missing))
            .unwrap_err();
        assert!(
            matches!(
                error,
                TyperError::MissingContextualArgument { source: actual_source, expected, tree_index, .. }
                    if actual_source == source && expected == defs.long && tree_index == bad_rhs.index()
            ),
            "{error:?}"
        );
        assert_eq!(typer.store().checkpoint(), checkpoint);
        assert!(typer.function_literal_method_at(source, bad_rhs).is_none());
        assert!(typer.source_typed_index().is_empty());
        assert_eq!(typer.typed_ast().iter().count(), 0);
    }
    typer
        .type_method_definition(method_tree(&index, recovered))
        .unwrap();
    let outer = layer(&typer, typer.source_typed_index().get(source, rhs).unwrap());
    let inner = layer(&typer, outer.body);
    assert_eq!(outer.captures, [class, supplied]);
    assert_eq!(inner.captures, [class, outer.parameter, supplied]);
    check_call(
        &typer,
        inner.body,
        read,
        &[outer.parameter, inner.parameter, supplied],
    );
}
