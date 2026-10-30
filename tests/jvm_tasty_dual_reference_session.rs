use dotty::jvm_tasty_plan::with_paired_jvm_tasty_batch_plan;
use dotty_core::ast::TreeKind;
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceId, SourceText, SymbolInfo, SymbolKind,
    TermRefTarget, Type, TypeRefTarget,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_typer::SourceTyper;

const SOURCE: &str = include_str!("fixtures/jvm_tasty_dual_reference_constructors/Pair.scala");

#[test]
fn rust_frontend_session_keeps_two_parameter_and_field_alias_identities() {
    let source_id = SourceId::from_index(1874);
    let source_text = SourceText::new(SOURCE).unwrap();
    let scanner = ContextualScanner::new(SOURCE).unwrap();
    assert!(scanner.diagnostics().is_empty());
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = parse_compilation_unit(source_text, source_id, scanner, &mut store.names);
    assert!(parsed.diagnostics.is_empty());
    let mut packages = Packages::new();
    let source_index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        "Pair.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        outcome => panic!("the audit source names without diagnostics: {outcome:?}"),
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("the source has one package root");
    };
    assert_eq!(package.stats.len(), 2);

    let mut pair_data = None;
    let mut counter_class = None;
    let mut counter_get = None;
    let mut counter_constructor = None;
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &source_index,
        &mut store,
        definitions,
        &packages,
    );
    let mut roots = Vec::new();
    for &class_tree in &package.stats {
        let TreeKind::TypeDef(class_definition) = &parsed.ast.get(class_tree).kind else {
            panic!("both declarations are ordinary classes");
        };
        let TreeKind::Template(template) = &parsed.ast.get(class_definition.rhs).kind else {
            panic!("each class has a template");
        };
        let class = source_index.symbol_at(source_id, class_tree).unwrap();
        let constructor_tree = template.constructor;
        let constructor = source_index.symbol_at(source_id, constructor_tree).unwrap();
        let TreeKind::DefDef(constructor_definition) = &parsed.ast.get(constructor_tree).kind
        else {
            panic!("each template has a primary constructor");
        };
        assert_eq!(constructor_definition.value_param_clauses.len(), 1);
        let constructor_params = constructor_definition.value_param_clauses[0]
            .iter()
            .map(|&parameter_tree| {
                source_index
                    .derived_symbol_at(constructor, source_id, parameter_tree)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        typer.complete_symbol(class).unwrap();
        typer.complete_symbol(constructor).unwrap();
        for &parameter in &constructor_params {
            typer.complete_symbol(parameter).unwrap();
        }
        let name = typer
            .store()
            .names
            .resolve(class_definition.name.as_name().text());
        if name == "Pair" {
            let [first_tree, second_tree] =
                constructor_definition.value_param_clauses[0].as_slice()
            else {
                panic!("Pair has two ordered constructor formals");
            };
            let template_params = [*first_tree, *second_tree]
                .map(|tree| source_index.symbol_at(source_id, tree).unwrap());
            for (source_parameter_tree, template_parameter) in
                [*first_tree, *second_tree].into_iter().zip(template_params)
            {
                typer.complete_symbol(template_parameter).unwrap();
                assert_eq!(
                    typer.store().symbols.get(template_parameter).kind,
                    SymbolKind::Parameter
                );
                assert_eq!(
                    typer.store().symbols.get(template_parameter).owner,
                    Some(class)
                );
                assert!(matches!(
                    source_index.definition_of(template_parameter),
                    Some(dotty_core::SourceDefinition::Canonical { source, tree })
                        if source == source_id && tree == source_parameter_tree
                ));
                let SymbolInfo::Complete(template_type) =
                    typer.store().symbols.get(template_parameter).info
                else {
                    panic!("template-owned parameter type completes in the same session");
                };
                assert!(matches!(
                    typer.store().types.get(template_type),
                    Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == source_index.symbol_at(source_id, package.stats[0]).unwrap()
                ));
            }
            let [first_constructor_param, second_constructor_param] = constructor_params.as_slice()
            else {
                panic!("Pair has two constructor-owned formal identities");
            };
            assert_ne!(template_params[0], template_params[1]);
            assert_ne!(*first_constructor_param, *second_constructor_param);
            assert_ne!(template_params[0], *first_constructor_param);
            assert_ne!(template_params[1], *second_constructor_param);
            for (template_parameter, constructor_parameter) in template_params
                .into_iter()
                .zip(constructor_params.iter().copied())
            {
                assert_eq!(
                    typer.store().symbols.get(template_parameter).kind,
                    SymbolKind::Parameter
                );
                assert_eq!(
                    typer.store().symbols.get(template_parameter).owner,
                    Some(class)
                );
                assert!(matches!(
                    source_index.definition_of(constructor_parameter),
                    Some(dotty_core::SourceDefinition::Derived { source, .. }) if source == source_id
                ));
                let SymbolInfo::Complete(parameter_type) =
                    typer.store().symbols.get(constructor_parameter).info
                else {
                    panic!("constructor formal type is completed in this session");
                };
                assert!(matches!(
                    typer.store().types.get(parameter_type),
                    Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == source_index.symbol_at(source_id, package.stats[0]).unwrap()
                ));
            }
            let mut fields = Vec::new();
            let mut methods = Vec::new();
            for &member_tree in &template.body {
                match &parsed.ast.get(member_tree).kind {
                    TreeKind::ValDef(definition) => {
                        let field = source_index.symbol_at(source_id, member_tree).unwrap();
                        typer.complete_symbol(field).unwrap();
                        let SymbolInfo::Complete(field_type) =
                            typer.store().symbols.get(field).info
                        else {
                            panic!("field signature is complete");
                        };
                        let rhs = definition.rhs.unwrap();
                        let context = typer.field_initializer_context_for(field).unwrap();
                        let typed_rhs = typer
                            .type_expression_expected(rhs, context, field_type)
                            .unwrap();
                        assert_eq!(
                            typer.source_typed_index().get(source_id, rhs),
                            Some(typed_rhs)
                        );
                        assert!(matches!(parsed.ast.get(rhs).kind, TreeKind::Ident(_)));
                        let source_position = fields.len();
                        let template_parameter = template_params[source_position];
                        assert_eq!(typer.store().symbols.get(field).owner, Some(class));
                        assert_eq!(typer.store().symbols.get(field).kind, SymbolKind::Field);
                        let typed_target = match typer
                            .store()
                            .types
                            .get(typer.typed_ast().get(typed_rhs).ty)
                        {
                            Type::TermRef {
                                target: TermRefTarget::Symbol(symbol),
                                ..
                            } => *symbol,
                            other => panic!(
                                "typed field initializer retains exact parameter TermRef: {other:?}"
                            ),
                        };
                        assert_eq!(typed_target, constructor_params[source_position]);
                        fields.push((
                            field,
                            member_tree,
                            rhs,
                            typed_rhs,
                            template_parameter,
                            field_type,
                        ));
                        roots.push((member_tree, rhs, typed_rhs));
                    }
                    TreeKind::DefDef(definition) if member_tree != constructor_tree => {
                        let method = source_index.symbol_at(source_id, member_tree).unwrap();
                        typer.complete_symbol(method).unwrap();
                        let rhs = definition.rhs.unwrap();
                        let context = typer.expression_context_for(method).unwrap();
                        let typed_rhs = typer.type_expression(rhs, context).unwrap();
                        assert_eq!(
                            typer.source_typed_index().get(source_id, rhs),
                            Some(typed_rhs)
                        );
                        methods.push((method, member_tree, rhs, typed_rhs));
                        roots.push((member_tree, rhs, typed_rhs));
                    }
                    other => panic!("supported source member: {other:?}"),
                }
            }
            assert_eq!(fields.len(), 2);
            assert_eq!(methods.len(), 2);
            pair_data = Some((class, constructor, constructor_params, fields, methods));
        } else if name == "Counter" {
            counter_class = Some(class);
            counter_constructor = Some(constructor);
            for &member_tree in &template.body {
                if let TreeKind::DefDef(definition) = &parsed.ast.get(member_tree).kind
                    && typer
                        .store()
                        .names
                        .resolve(definition.name.as_name().text())
                        == "get"
                {
                    let method = source_index.symbol_at(source_id, member_tree).unwrap();
                    typer.complete_symbol(method).unwrap();
                    let rhs = definition.rhs.unwrap();
                    let context = typer.expression_context_for(method).unwrap();
                    let typed_rhs = typer.type_expression(rhs, context).unwrap();
                    assert_eq!(
                        typer.source_typed_index().get(source_id, rhs),
                        Some(typed_rhs)
                    );
                    counter_get = Some(method);
                    roots.push((member_tree, rhs, typed_rhs));
                }
            }
        } else {
            panic!("exact source contains only Counter and Pair");
        }
    }
    let (pair_class, pair_constructor, constructor_params, fields, methods) = pair_data.unwrap();
    let counter_class = counter_class.unwrap();
    assert_eq!(
        typer.store().symbols.get(pair_constructor).owner,
        Some(pair_class)
    );
    assert_eq!(
        typer
            .store()
            .symbols
            .get(counter_constructor.unwrap())
            .owner,
        Some(counter_class)
    );
    for (_, _, _, _, _, field_type) in &fields {
        assert!(matches!(
            typer.store().types.get(*field_type),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == counter_class
        ));
    }
    let counter_get = counter_get.unwrap();
    for (method_index, (method, _, _, typed_rhs)) in methods.iter().enumerate() {
        let SymbolInfo::Complete(result_type) = typer.store().symbols.get(*method).info else {
            panic!("read method signature is complete");
        };
        let Type::Method(signature) = typer.store().types.get(result_type) else {
            panic!("method signature is a Method type");
        };
        let result_symbol = match typer.store().types.get(signature.result) {
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("method result retains a canonical source type reference: {other:?}"),
        };
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(*typed_rhs).ty),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == result_symbol
        ));
        let TreeKind::Apply(application) = &typer.typed_ast().get(*typed_rhs).kind else {
            panic!("both methods retain their original call roots");
        };
        let TreeKind::Select(selection) = &typer.typed_ast().get(application.function).kind else {
            panic!("both reads retain their selected Counter.get call");
        };
        let receiver = typer.typed_ast().get(selection.qualifier);
        assert!(matches!(
            typer.store().types.get(receiver.ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                if *symbol == fields[method_index].0
        ));
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(application.function).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == counter_get
        ));
    }
    assert!(roots.iter().all(|(_, rhs, typed)| {
        typer.source_typed_index().get(source_id, *rhs) == Some(*typed)
    }));
    let field_receipts = fields
        .iter()
        .map(|(field, _, _, typed, template_alias, field_type)| {
            (
                *field,
                *template_alias,
                *field_type,
                *typed,
                typer.typed_ast().get(*typed).ty,
            )
        })
        .collect::<Vec<_>>();
    let method_receipts = methods
        .iter()
        .map(|(method, _, _, typed)| {
            let SymbolInfo::Complete(signature) = typer.store().symbols.get(*method).info else {
                unreachable!();
            };
            let Type::Method(signature) = typer.store().types.get(signature) else {
                unreachable!();
            };
            let result = signature.result;
            let result_symbol = match typer.store().types.get(result) {
                Type::TypeRef {
                    target: TypeRefTarget::Symbol(symbol),
                    ..
                } => *symbol,
                _ => unreachable!(),
            };
            (
                *method,
                result,
                result_symbol,
                *typed,
                typer.typed_ast().get(*typed).ty,
            )
        })
        .collect::<Vec<_>>();
    println!(
        "Rust source session SourceId={source_id:?} Counter={counter_class:?} get={counter_get:?} Pair={pair_class:?} constructor={pair_constructor:?} template_parameters={:?} constructor_formals={constructor_params:?} fields={field_receipts:?} methods={method_receipts:?} source-to-typed-roots={roots:?}",
        fields
            .iter()
            .map(|(_, _, _, _, alias, _)| *alias)
            .collect::<Vec<_>>()
    );
}

#[test]
fn paired_batch_reports_the_actual_first_dual_reference_rejection() {
    let error =
        with_paired_jvm_tasty_batch_plan(SOURCE, "Pair.scala", SourceId::from_index(1874), |_| ())
            .expect_err("the bounded paired producer must expose its actual first gate");
    println!("dual-reference paired batch first rejection: {error:?}");
    assert!(matches!(
        error,
        dotty::jvm_tasty_plan::PairedPlanError::BatchClass {
            class_index: 1,
            phase: "TASTy plan validation",
            ref reason,
            ..
        } if reason.starts_with("Unsupported { feature: PrimaryConstructorInput")
    ));
}
