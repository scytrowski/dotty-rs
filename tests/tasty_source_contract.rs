use dotty_core::ast::*;
use dotty_core::*;
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_typer::SourceTyper;

const OPERATIONS: &str = include_str!("fixtures/tasty_pickler/Operations.scala");

#[test]
fn real_source_frontend_exposes_canonical_method_clause_and_reference_evidence() {
    let source = SourceId::from_index(1238);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(OPERATIONS).unwrap();
    assert!(scanner.diagnostics().is_empty());
    let parsed = parse_compilation_unit(
        SourceText::new(OPERATIONS).unwrap(),
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
        "Operations.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        outcome => panic!("namer rejected audit fixture: {outcome:?}"),
    };

    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("expected package root")
    };
    let [class_tree] = package.stats.as_slice() else {
        panic!("expected one class")
    };
    let class = index.symbol_at(source, *class_tree).unwrap();
    let TreeKind::TypeDef(class_definition) = &parsed.ast.get(*class_tree).kind else {
        panic!("expected class TypeDef")
    };
    let TreeKind::Template(template) = &parsed.ast.get(class_definition.rhs).kind else {
        panic!("expected class Template")
    };
    let methods = template
        .body
        .iter()
        .filter_map(|tree| match &parsed.ast.get(*tree).kind {
            TreeKind::DefDef(definition) => Some((*tree, definition)),
            _ => None,
        })
        .map(|(tree, definition)| {
            let symbol = index.symbol_at(source, tree).unwrap();
            (
                store
                    .names
                    .resolve(definition.name.as_name().text())
                    .to_owned(),
                tree,
                definition,
                symbol,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(methods.len(), 5);
    let find = |name: &str| methods.iter().find(|entry| entry.0 == name).unwrap();
    assert_eq!(
        methods
            .iter()
            .map(|entry| entry.0.as_str())
            .collect::<Vec<_>>(),
        ["use", "identity", "first", "second", "zero"]
    );
    let method_ids = methods.iter().map(|entry| entry.3).collect::<Vec<_>>();
    assert_eq!(
        method_ids
            .iter()
            .map(|id| store.symbols.get(*id).owner)
            .collect::<Vec<_>>(),
        vec![Some(class); 5]
    );

    let (_, identity_tree, identity_definition, identity) = find("identity");
    assert_eq!(identity_definition.value_param_clauses.len(), 1);
    assert_eq!(identity_definition.value_param_clauses[0].len(), 1);
    let parameter_tree = identity_definition.value_param_clauses[0][0];
    let parameter = index.symbol_at(source, parameter_tree).unwrap();
    assert_eq!(store.symbols.get(parameter).owner, Some(*identity));
    let identity_scope = index.scope_of(*identity).unwrap();
    assert_eq!(store.scopes.get(identity_scope).owner, Some(*identity));
    let parameter_name = store.symbols.get(parameter).name;
    assert!(
        store
            .scopes
            .get(identity_scope)
            .lookup_all(&parameter_name)
            .contains(&parameter)
    );

    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer.complete_symbol(class).unwrap();
    for symbol in &method_ids {
        typer.complete_symbol(*symbol).unwrap();
    }
    let int = definitions.int;
    let SymbolInfo::Complete(class_info_type) = typer.store().symbols.get(class).info else {
        panic!("class info was not completed")
    };
    let Type::ClassInfo(class_info) = typer.store().types.get(class_info_type) else {
        panic!("class declaration scope is unavailable")
    };
    let class_scope = class_info.declarations;
    assert_eq!(index.scope_of(class), Some(class_scope));
    assert_eq!(typer.store().scopes.get(class_scope).owner, Some(class));
    for symbol in &method_ids {
        let name = typer.store().symbols.get(*symbol).name;
        assert!(
            typer
                .store()
                .scopes
                .get(class_scope)
                .lookup_all(&name)
                .contains(symbol)
        );
    }
    for name in ["use", "first", "second"] {
        let method = find(name).3;
        assert!(
            matches!(typer.store().symbols.get(method).info,
            SymbolInfo::Complete(result) if result == int),
            "parameterless `def {name}: Int` has the direct result TypeId"
        );
    }
    let identity_info = typer.store().symbols.get(*identity).info;
    let SymbolInfo::Complete(identity_signature) = identity_info else {
        panic!("identity signature was not completed")
    };
    let Type::Method(identity_method_type) = typer.store().types.get(identity_signature) else {
        panic!("one ordinary value clause must be a method type")
    };
    assert_eq!(identity_method_type.params.len(), 1);
    assert!(matches!(typer.store().symbols.get(parameter).info,
        SymbolInfo::Complete(parameter_type) if parameter_type == int));
    assert_eq!(identity_method_type.params[0].ty, int);
    assert_eq!(identity_method_type.result, int);
    assert_eq!(
        typer
            .store()
            .names
            .resolve(identity_method_type.params[0].name.as_name().text()),
        "x"
    );

    let (_, _, zero_definition, zero) = find("zero");
    assert_eq!(zero_definition.value_param_clauses.len(), 1);
    assert!(zero_definition.value_param_clauses[0].is_empty());
    let SymbolInfo::Complete(zero_signature) = typer.store().symbols.get(*zero).info else {
        panic!("zero signature was not completed")
    };
    assert!(
        matches!(typer.store().types.get(zero_signature), Type::Method(method)
        if method.params.is_empty() && method.result == int)
    );

    for (name, _, definition, method) in &methods {
        let rhs = definition.rhs.expect("each audit method has an RHS");
        let context = typer.expression_context_for(*method).unwrap();
        let typed = typer.type_expression_expected(rhs, context, int).unwrap();
        assert_eq!(
            typer.source_typed_index().get(source, rhs),
            Some(typed),
            "{name}"
        );
        match name.as_str() {
            "use" => {
                assert_eq!(typer.typed_ast().get(typed).ty, int);
                let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
                    panic!("identity(42) typed root should be Apply")
                };
                assert_eq!(application.args.len(), 1);
                assert!(matches!(
                    typer.typed_ast().get(application.args[0]).kind,
                    TreeKind::Literal(Literal {
                        value: Constant::Int(42)
                    })
                ));
                assert!(matches!(
                    typer.typed_ast().get(application.function).kind,
                    TreeKind::Ident(_)
                ));
                assert!(
                    matches!(typer.store().types.get(typer.typed_ast().get(application.function).ty),
                    Type::TermRef {
                        prefix,
                        target: TermRefTarget::Symbol(target),
                    } if *target == *identity
                        && matches!(typer.store().types.get(*prefix), Type::ThisType { class: this_class } if *this_class == class))
                );
            }
            "identity" => {
                let TreeKind::Ident(reference) = &typer.typed_ast().get(typed).kind else {
                    panic!("parameter reference should remain an Ident")
                };
                assert_eq!(index.symbol_at(source, parameter_tree), Some(parameter));
                assert!(
                    matches!(typer.store().types.get(typer.typed_ast().get(typed).ty),
                    Type::TermRef { target: TermRefTarget::Symbol(target), .. } if *target == parameter)
                );
                assert_eq!(typer.store().names.resolve(reference.name.text()), "x");
            }
            "first" => assert!(
                matches!(typer.store().types.get(typer.typed_ast().get(typed).ty),
                Type::TermRef { target: TermRefTarget::Symbol(target), .. }
                    if *target == find("second").3)
            ),
            "second" | "zero" => {
                assert!(
                    matches!(
                        typer.typed_ast().get(typed).kind,
                        TreeKind::Literal(Literal {
                            value: Constant::Int(_)
                        })
                    ),
                    "{name}"
                );
                assert!(matches!(
                    typer.store().types.get(typer.typed_ast().get(typed).ty),
                    Type::Constant(Constant::Int(_))
                ));
            }
            _ => unreachable!(),
        }
    }

    let (_, _, first_definition, first) = find("first");
    let first_rhs = first_definition.rhs.unwrap();
    let first_typed = typer.source_typed_index().get(source, first_rhs).unwrap();
    assert_eq!(index.symbol_at(source, *identity_tree), Some(*identity));
    assert_eq!(typer.store().symbols.get(*first).owner, Some(class));
    assert!(matches!(
        typer.store().symbols.get(*identity).info,
        SymbolInfo::Complete(_)
    ));
    assert_eq!(
        typer.source_typed_index().get(source, first_rhs),
        Some(first_typed)
    );
}
