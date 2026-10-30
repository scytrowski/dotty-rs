use super::*;
use dotty_classloader::classloader::{
    ClasspathSymbolResolver, CompositeClassPath, DirectoryClassPath, LoadingSession,
};

fn method_tree(
    parsed: &dotty_parser::ParseResult,
    index: &SourceSemanticIndex,
    source: SourceId,
    method: SymbolId,
) -> TreeId<Untyped> {
    parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            (index.symbol_at(source, tree) == Some(method)
                && matches!(node.kind, TreeKind::DefDef(_)))
            .then_some(tree)
        })
        .expect("source method should have a DefDef")
}

#[test]
fn generic_function_application_rebinds_the_exact_active_source_type_parameter() {
    let source_text = concat!(
        "trait ComposeCase[A, T1, R] { ",
        "def apply(value: T1): R; ",
        "def direct[B](g: B => T1, x: B): R = apply(g(x)); ",
        "def compose[A](g: A => T1): A => R = x => apply(g(x)); ",
        "def wrongArgument[A](g: A => T1): A => R = x => apply(g(1)); ",
        "def wrongOwner[A](g: String => T1): A => R = x => apply(g(x)); ",
        "def wrongVariance[A](g: Nothing => T1): A => R = x => apply(g(x)); ",
        "def wrongOutput[A](g: A => Boolean): A => R = x => apply(g(x)); ",
        "def wrongArity[A](g: A => T1): A => R = x => apply(g(x, x)); ",
        "def unrelatedApply[A](g: Unrelated[A, T1, R]): A => R = x => apply(g(x)); ",
        "def retry[A](g: A => T1): A => R = x => apply(g(x))",
        " }; ",
        "class Unrelated[A, T1, R] { def apply(value: String): T1 }",
    );
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
    let method = |name: &str, store: &SemanticStore| {
        let (symbol, _) = method_definition_and_rhs(&parsed, store, &index, source, name);
        (symbol, method_tree(&parsed, &index, source, symbol))
    };
    let (direct, direct_tree) = method("direct", &store);
    let (compose, compose_tree) = method("compose", &store);
    let (wrong_argument, wrong_argument_tree) = method("wrongArgument", &store);
    let (wrong_owner, wrong_owner_tree) = method("wrongOwner", &store);
    let (wrong_variance, wrong_variance_tree) = method("wrongVariance", &store);
    let (wrong_output, wrong_output_tree) = method("wrongOutput", &store);
    let (wrong_arity, wrong_arity_tree) = method("wrongArity", &store);
    let (unrelated_apply, unrelated_apply_tree) = method("unrelatedApply", &store);
    let (retry, retry_tree) = method("retry", &store);

    let library_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../dotty-tasty/tests/fixtures/scala3-library");
    let resolver = ClasspathSymbolResolver::new(
        CompositeClassPath::new(vec![
            Box::new(DirectoryClassPath::new(library_root)),
            Box::new(JavaObjectStubPath),
        ]),
        definitions,
        LoadingSession::with_packages(packages.snapshot()),
    );
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(resolver));

    let typed_direct = typer
        .type_method_definition(direct_tree)
        .expect("a TypeRef argument and formal from the same source binder should conform");
    assert_eq!(
        typer.source_typed_index().get(source, direct_tree),
        Some(typed_direct)
    );
    assert_eq!(typer.store().symbols.get(direct).kind, SymbolKind::Method);

    let typed_compose = typer.type_method_definition(compose_tree).unwrap_or_else(|error| {
        panic!("generic compose-like DefDef should type with its canonical source binder: {error:?}")
    });
    assert_eq!(
        typer.source_typed_index().get(source, compose_tree),
        Some(typed_compose)
    );
    assert_eq!(typer.store().symbols.get(compose).kind, SymbolKind::Method);
    let TreeKind::DefDef(typed_definition) = &typer.typed_ast().get(typed_compose).kind else {
        panic!("compose-like root should remain an owned typed DefDef");
    };
    let typed_rhs = typed_definition
        .rhs
        .expect("typed DefDef should retain its rhs");
    let TreeKind::Closure(closure) = &typer.typed_ast().get(typed_rhs).kind else {
        panic!("compose-like rhs should remain a typed closure");
    };
    assert_eq!(closure.env.len(), 2, "the lambda captures this and g");
    assert!(matches!(
        typer.typed_ast().get(closure.env[0]).kind,
        TreeKind::This(_)
    ));
    let TreeKind::DefDef(source_definition) = &parsed.ast.get(compose_tree).kind else {
        unreachable!("source method lookup selected a DefDef")
    };
    let g_symbol = index
        .symbol_at(source, source_definition.value_param_clauses[0][0])
        .expect("compose g parameter has a canonical source symbol");
    assert!(matches!(
        typer.store().types.get(typer.typed_ast().get(closure.env[1]).ty),
        Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == g_symbol
    ));
    assert!(index.definition_of(g_symbol).is_some());
    let source_compose_start = parsed
        .ast
        .get(compose_tree)
        .position
        .expect("compose should retain a source position")
        .span()
        .range()
        .start() as usize;
    let inner_start = source_compose_start
        + source_text[source_compose_start..]
            .find("g(x)")
            .expect("compose body should contain g(x)");
    let inner_source_tree = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let start = node.position?.span().range().start() as usize;
            (start == inner_start && matches!(node.kind, TreeKind::Apply(_))).then_some(tree)
        })
        .expect("compose body should retain its exact inner g(x) Apply");
    let typed_inner = typer
        .source_typed_index()
        .get(source, inner_source_tree)
        .expect("inner g(x) should have a source-to-typed mapping");
    let TreeKind::Apply(inner_application) = &typer.typed_ast().get(typed_inner).kind else {
        panic!("inner g(x) should remain a typed Apply")
    };
    let inner_function_type = typer.typed_ast().get(inner_application.function).ty;
    let Type::TermRef {
        target: TermRefTarget::Symbol(apply_symbol),
        ..
    } = typer.store().types.get(inner_function_type)
    else {
        panic!("inner g(x) should select canonical Function1.apply")
    };
    assert_eq!(
        typer
            .store()
            .names
            .resolve(typer.store().symbols.get(*apply_symbol).name.text()),
        "apply"
    );
    let apply_owner = typer
        .store()
        .symbols
        .owner(*apply_symbol)
        .expect("Function1.apply should have its canonical class owner");
    assert_eq!(
        typer
            .store()
            .names
            .resolve(typer.store().symbols.get(apply_owner).name.text()),
        "Function1"
    );
    let SymbolInfo::Complete(signature) = *typer.store().symbols.info(compose) else {
        panic!("compose should retain its complete polymorphic signature")
    };
    let Type::Poly(_) = typer.store().types.get(signature) else {
        panic!("compose should retain its canonical method type-parameter binder")
    };
    let outer_start = source_compose_start
        + source_text[source_compose_start..]
            .find("apply(g(x))")
            .expect("compose body should contain apply(g(x))");
    let outer_source_tree = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let start = node.position?.span().range().start() as usize;
            (start == outer_start && matches!(node.kind, TreeKind::Apply(_))).then_some(tree)
        })
        .expect("compose body should retain its exact outer apply(g(x)) Apply");
    let typed_outer = typer
        .source_typed_index()
        .get(source, outer_source_tree)
        .expect("outer apply(g(x)) should have a source-to-typed mapping");
    let TreeKind::Apply(outer_application) = &typer.typed_ast().get(typed_outer).kind else {
        panic!("outer apply(g(x)) should remain a typed Apply")
    };
    let outer_function_type = typer.typed_ast().get(outer_application.function).ty;
    let Type::TermRef {
        target: TermRefTarget::Symbol(outer_apply_symbol),
        ..
    } = typer.store().types.get(outer_function_type)
    else {
        panic!("outer apply(g(x)) should select the source trait apply")
    };
    assert_eq!(
        typer.store().symbols.owner(*outer_apply_symbol),
        typer.store().symbols.owner(compose),
        "outer apply must select the source trait's own apply method"
    );
    let lambda_tree = rhs_for(compose_tree, &parsed);
    let lambda_method = typer
        .function_literal_method_at(source, lambda_tree)
        .expect("typed compose should register its lambda method");
    let SymbolInfo::Complete(lambda_signature) = *typer.store().symbols.info(lambda_method) else {
        panic!("typed lambda should retain its complete signature")
    };
    let Type::Method(lambda_method_type) = typer.store().types.get(lambda_signature) else {
        panic!("typed lambda should have a method signature")
    };
    assert!(matches!(
        typer.store().types.get(lambda_method_type.params[0].ty),
        Type::ParamRef { binder, index: 0 } if *binder == signature
    ));
    let TreeKind::Closure(closure) = &typer.typed_ast().get(typed_rhs).kind else {
        unreachable!()
    };
    let Type::Applied { args, .. } = typer.store().types.get(typer.typed_ast().get(typed_rhs).ty)
    else {
        panic!("closure result should remain an applied Function1 type")
    };
    assert!(matches!(
        typer.store().types.get(args[0]),
        Type::ParamRef { binder, index: 0 } if *binder == signature
    ));
    assert!(matches!(
        typer.store().types.get(args[1]),
        Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }
            if typer.store().symbols.owner(*symbol) == typer.store().symbols.owner(compose)
                && typer.store().names.resolve(typer.store().symbols.get(*symbol).name.text()) == "R"
    ));
    assert_eq!(closure.env.len(), 2);

    for (symbol, tree) in [
        (wrong_argument, wrong_argument_tree),
        (wrong_owner, wrong_owner_tree),
        (wrong_variance, wrong_variance_tree),
        (wrong_output, wrong_output_tree),
        (wrong_arity, wrong_arity_tree),
        (unrelated_apply, unrelated_apply_tree),
    ] {
        let store_before = typer.store().checkpoint();
        let typed_before = typer.typed_ast().checkpoint();
        let failure = typer
            .type_method_definition(tree)
            .expect_err("non-conformant generic applications must remain rejected");
        assert_eq!(typer.store().checkpoint(), store_before);
        assert_eq!(typer.typed_ast().checkpoint(), typed_before);
        assert_eq!(typer.source_typed_index().get(source, tree), None);
        assert!(
            typer
                .function_literal_method_at(source, rhs_for(tree, &parsed))
                .is_none()
        );
        assert_eq!(typer.store().symbols.get(symbol).kind, SymbolKind::Method);

        let retry = typer
            .type_method_definition(tree)
            .expect_err("failed generic application must be deterministic on retry");
        assert_eq!(format!("{retry:?}"), format!("{failure:?}"));
        assert_eq!(typer.store().checkpoint(), store_before);
        assert_eq!(typer.typed_ast().checkpoint(), typed_before);
        assert_eq!(
            typer.source_typed_index().get(source, direct_tree),
            Some(typed_direct),
            "preexisting success must survive every failed call"
        );
        assert_eq!(
            typer.source_typed_index().get(source, compose_tree),
            Some(typed_compose),
            "preexisting generic closure must survive every failed call"
        );
    }

    let typed_retry = typer
        .type_method_definition(retry_tree)
        .expect("valid generic call should type in the same session after failures");
    assert_eq!(
        typer.source_typed_index().get(source, retry_tree),
        Some(typed_retry)
    );
    assert_eq!(typer.store().symbols.get(retry).kind, SymbolKind::Method);
}

fn rhs_for(tree: TreeId<Untyped>, parsed: &dotty_parser::ParseResult) -> TreeId<Untyped> {
    let TreeKind::DefDef(definition) = &parsed.ast.get(tree).kind else {
        panic!("negative method should have a DefDef")
    };
    definition.rhs.expect("negative method should have a body")
}

#[test]
fn generic_function_application_reports_missing_function_classpath_and_rolls_back() {
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
        "trait MissingFunctionLibrary[A, T1, R] { def apply(value: T1): R; def compose[A](g: A => T1): A => R = x => apply(g(x)) }",
    );
    let (compose, _) = method_definition_and_rhs(&parsed, &store, &index, source, "compose");
    let compose_tree = method_tree(&parsed, &index, source, compose);
    let resolver = ClasspathSymbolResolver::new(
        CompositeClassPath::new(vec![Box::new(JavaObjectStubPath)]),
        definitions,
        LoadingSession::with_packages(packages.snapshot()),
    );
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(resolver));
    let store_before = typer.store().checkpoint();
    let typed_before = typer.typed_ast().checkpoint();

    let failure = typer
        .type_method_definition(compose_tree)
        .expect_err("function syntax requires scala.Function1 on the classpath");
    assert!(matches!(
        failure,
        TyperError::SourceFunctionClassNotFound {
            kind: SourceFunctionKind::Ordinary,
            arity: 1
        }
    ));
    assert_eq!(typer.store().checkpoint(), store_before);
    assert_eq!(typer.typed_ast().checkpoint(), typed_before);
    assert_eq!(typer.source_typed_index().get(source, compose_tree), None);
}

#[test]
fn shadowed_active_binders_reject_outer_function_application_and_allow_retry() {
    let source_text = concat!(
        "trait ShadowedComposeCase[T1, R] { ",
        "def apply(value: T1): R; ",
        "def binderFailure[A](g: A => T1): Unit = { ",
        "def inner[A](x: A): T1 = g(x); ",
        "() }; ",
        "def retry[A](g: A => T1): A => R = x => apply(g(x))",
        " }",
    );
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
    let (outer_method, _) =
        method_definition_and_rhs(&parsed, &store, &index, source, "binderFailure");
    let outer_tree = method_tree(&parsed, &index, source, outer_method);
    let (retry_method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "retry");
    let retry_tree = method_tree(&parsed, &index, source, retry_method);

    let (outer_rhs, inner_tree, inner_type_parameter_tree, outer_type_parameter, g_symbol) = {
        let TreeKind::DefDef(outer_definition) = &parsed.ast.get(outer_tree).kind else {
            unreachable!("source method lookup selected a DefDef")
        };
        let outer_type_parameter = index
            .symbol_at(source, outer_definition.type_params[0])
            .expect("outer method A has a canonical source symbol");
        let g_symbol = index
            .symbol_at(source, outer_definition.value_param_clauses[0][0])
            .expect("outer g parameter has a canonical source symbol");
        let TreeKind::Block(block) = &parsed.ast.get(outer_definition.rhs.unwrap()).kind else {
            panic!("binderFailure should retain its local-method block")
        };
        let inner_tree = block.stats[0];
        let TreeKind::DefDef(inner_definition) = &parsed.ast.get(inner_tree).kind else {
            panic!("binderFailure should retain its nested inner DefDef")
        };
        (
            outer_definition.rhs.unwrap(),
            inner_tree,
            inner_definition.type_params[0],
            outer_type_parameter,
            g_symbol,
        )
    };
    let inner_method_rhs = match &parsed.ast.get(inner_tree).kind {
        TreeKind::DefDef(definition) => definition.rhs.unwrap(),
        _ => unreachable!(),
    };
    let inner_rhs_range = parsed
        .ast
        .get(inner_method_rhs)
        .position
        .expect("inner method body should have a source position")
        .span()
        .range();
    let inner_application_tree = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let start = node.position?.span().range().start() as usize;
            (start >= inner_rhs_range.start() as usize
                && start < inner_rhs_range.end() as usize
                && matches!(node.kind, TreeKind::Apply(_))
                && source_text[start..].starts_with("g(x)"))
            .then_some(tree)
        })
        .expect("inner generic method should retain its exact g(x) Apply");

    let library_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../dotty-tasty/tests/fixtures/scala3-library");
    let resolver = ClasspathSymbolResolver::new(
        CompositeClassPath::new(vec![
            Box::new(DirectoryClassPath::new(library_root)),
            Box::new(JavaObjectStubPath),
        ]),
        definitions,
        LoadingSession::with_packages(packages.snapshot()),
    );
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(resolver));
    typer
        .complete_symbol(outer_method)
        .expect("outer generic method signature should complete before body typing");
    let outer_context = typer
        .expression_context_for(outer_method)
        .expect("outer method should have an expression context");
    preindex_block_for_test(&mut typer, outer_rhs, outer_context);
    let inner_method = typer
        .local_method_symbol_at(source, inner_tree)
        .expect("inner method should receive a canonical local symbol");
    typer
        .complete_symbol(inner_method)
        .expect("inner generic method signature should complete before body typing");
    let inner_type_parameter = typer
        .local_method_type_parameter_symbol_at(source, inner_type_parameter_tree)
        .expect("inner method A should receive a canonical local type-parameter symbol");
    assert_ne!(
        outer_type_parameter, inner_type_parameter,
        "shadowed A declarations must retain distinct canonical symbols"
    );
    assert_eq!(
        typer.store().symbols.get(outer_type_parameter).owner,
        Some(outer_method)
    );
    assert_eq!(
        typer.store().symbols.get(inner_type_parameter).owner,
        Some(inner_method)
    );
    assert_eq!(
        typer.store().symbols.get(g_symbol).owner,
        Some(outer_method)
    );
    let store_before = typer.store().checkpoint();
    let typed_before = typer.typed_ast().checkpoint();
    let failure = typer
        .type_method_definition(outer_tree)
        .expect_err("inner A must not be rebound to the shadowed outer A");
    let error_debug = format!("{failure:?}");
    let TyperError::ApplicationArgumentConformanceUnsupported {
        source: error_source,
        tree_index,
        argument_index,
        actual,
        expected,
        error,
    } = &failure
    else {
        panic!("expected the inner g(x) conformance diagnostic, got {failure:?}")
    };
    assert_eq!(*error_source, source);
    assert_eq!(*tree_index, inner_application_tree.index());
    assert_eq!(*argument_index, 0);
    let outer_signature = match *typer.store().symbols.info(outer_method) {
        SymbolInfo::Complete(signature) => signature,
        _ => panic!("outer generic method signature should be complete"),
    };
    let inner_signature = match *typer.store().symbols.info(inner_method) {
        SymbolInfo::Complete(signature) => signature,
        _ => panic!("inner generic method signature should be complete"),
    };
    assert_ne!(outer_signature, inner_signature);
    assert!(matches!(
        typer.store().types.get(outer_signature),
        Type::Poly(poly) if poly.params.len() == 1
    ));
    assert!(matches!(
        typer.store().types.get(inner_signature),
        Type::Poly(poly) if poly.params.len() == 1
    ));
    assert!(matches!(
        typer.store().types.get(*actual),
        Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }
            if *symbol == inner_type_parameter
    ));
    assert!(matches!(
        typer.store().types.get(*expected),
        Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }
            if *symbol == outer_type_parameter
    ));
    assert!(
        matches!(
            error.as_ref(),
            TypeRelationError::NonClassLikeSymbol { symbol, kind: SymbolKind::TypeParameter }
                if *symbol == inner_type_parameter
        ),
        "actual={actual:?}, expected={expected:?}, relation_error={error:?}"
    );
    assert_ne!(
        inner_signature, outer_signature,
        "the nested and enclosing generic method signatures must stay distinct"
    );
    assert_eq!(
        typer.store().symbols.get(g_symbol).owner,
        Some(outer_method)
    );
    assert_eq!(typer.store().checkpoint(), store_before);
    assert_eq!(typer.typed_ast().checkpoint(), typed_before);
    assert_eq!(typer.source_typed_index().get(source, outer_tree), None);
    assert_eq!(typer.source_typed_index().get(source, inner_tree), None);
    assert_eq!(
        typer
            .source_typed_index()
            .get(source, inner_application_tree),
        None
    );
    let retried_failure = typer
        .type_method_definition(outer_tree)
        .expect_err("the binder failure should remain deterministic after rollback");
    assert_eq!(format!("{retried_failure:?}"), error_debug);
    assert_eq!(typer.store().checkpoint(), store_before);
    assert_eq!(typer.typed_ast().checkpoint(), typed_before);

    let typed_retry = typer
        .type_method_definition(retry_tree)
        .expect("a valid generic closure should type in the same session after rollback");
    assert_eq!(
        typer.source_typed_index().get(source, retry_tree),
        Some(typed_retry)
    );
    assert_eq!(
        typer.store().symbols.get(retry_method).kind,
        SymbolKind::Method
    );
}
