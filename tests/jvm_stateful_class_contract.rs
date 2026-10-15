use dotty_core::ast::{TreeKind, Untyped};
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceId, SourceText, SymbolFlags, SymbolId, SymbolInfo,
    SymbolKind, TermRefTarget, TreeId, Type, Visibility,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_typer::SourceTyper;

fn with_source<R>(
    source_text: &str,
    f: impl FnOnce(
        &dotty_core::AstArena<Untyped>,
        SourceId,
        &dotty_namer::SourceSemanticIndex,
        &Definitions,
        &mut SourceTyper<'_>,
    ) -> R,
) -> R {
    let source_id = SourceId::from_index(1);
    let source = SourceText::new(source_text).unwrap();
    let scanner = ContextualScanner::new(source_text).unwrap();
    assert!(scanner.diagnostics().is_empty());

    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = parse_compilation_unit(source, source_id, scanner, &mut store.names);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        "Stateful.scala",
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
        NamingOutcome::Rejected { diagnostics } => panic!("namer rejected source: {diagnostics:?}"),
    };
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    f(&parsed.ast, source_id, &index, &definitions, &mut typer)
}

fn symbol_named(
    ast: &dotty_core::AstArena<Untyped>,
    index: &dotty_namer::SourceSemanticIndex,
    source: SourceId,
    typer: &SourceTyper<'_>,
    name: &str,
) -> SymbolId {
    ast.iter()
        .find_map(|(tree, _)| {
            let symbol = index.symbol_at(source, tree)?;
            (typer
                .store()
                .names
                .resolve(typer.store().symbols.get(symbol).name.text())
                == name)
                .then_some(symbol)
        })
        .unwrap_or_else(|| panic!("missing source symbol {name}"))
}

fn field_tree_and_rhs(
    ast: &dotty_core::AstArena<Untyped>,
    index: &dotty_namer::SourceSemanticIndex,
    source: SourceId,
    typer: &SourceTyper<'_>,
    name: &str,
) -> (SymbolId, TreeId<Untyped>, TreeId<Untyped>) {
    ast.iter()
        .find_map(|(tree, node)| {
            let TreeKind::ValDef(definition) = &node.kind else {
                return None;
            };
            let symbol = index.symbol_at(source, tree)?;
            let symbol_name = typer
                .store()
                .names
                .resolve(definition.name.as_name().text());
            if symbol_name != name {
                return None;
            }
            Some((symbol, tree, definition.rhs?))
        })
        .unwrap_or_else(|| panic!("missing initialized field {name}"))
}

#[test]
fn private_primitive_field_initializers_keep_canonical_symbols_and_typed_roots() {
    with_source(
        "class Plain { private val seed: Int = 42; private val enabled: Boolean = true }",
        |ast, source, index, definitions, typer| {
            let class = symbol_named(ast, index, source, typer, "Plain");
            assert_eq!(typer.store().symbols.get(class).kind, SymbolKind::Class);
            assert_eq!(
                typer.store().symbols.get(class).visibility,
                Visibility::Public
            );
            typer.complete_symbol(class).unwrap();
            assert!(matches!(
                typer.store().symbols.get(class).info,
                SymbolInfo::Complete(_)
            ));
            let fields =
                ["seed", "enabled"].map(|name| field_tree_and_rhs(ast, index, source, typer, name));
            let template = ast
                .iter()
                .find_map(|(_, node)| match &node.kind {
                    TreeKind::Template(template) => Some(template),
                    _ => None,
                })
                .unwrap();
            let body_order = template
                .body
                .iter()
                .filter_map(|tree| index.symbol_at(source, *tree))
                .collect::<Vec<_>>();
            assert_eq!(
                body_order,
                fields
                    .iter()
                    .map(|(field, _, _)| *field)
                    .collect::<Vec<_>>()
            );

            for ((field, declaration, rhs), (expected_type, expected_constant)) in
                fields.into_iter().zip([
                    (definitions.int, dotty_core::Constant::Int(42)),
                    (definitions.boolean, dotty_core::Constant::Boolean(true)),
                ])
            {
                let symbol = typer.store().symbols.get(field);
                assert_eq!(symbol.kind, SymbolKind::Field);
                assert_eq!(symbol.owner, Some(class));
                assert_eq!(symbol.visibility, Visibility::Private);
                assert!(!symbol.flags.contains(SymbolFlags::MUTABLE));
                typer.complete_symbol(field).unwrap();
                let SymbolInfo::Complete(ty) = typer.store().symbols.get(field).info else {
                    panic!("field type should be complete");
                };
                assert_eq!(
                    typer.store().types.get(ty).reference_symbol(),
                    typer.store().types.get(expected_type).reference_symbol()
                );

                let context = typer.field_initializer_context_for(field).unwrap();
                let typed_rhs = typer.type_expression(rhs, context).unwrap();
                assert_eq!(typer.source_typed_index().get(source, rhs), Some(typed_rhs));
                assert_eq!(
                    typer.store().types.get(typer.typed_ast().get(typed_rhs).ty),
                    &Type::Constant(expected_constant)
                );
                assert_eq!(index.symbol_at(source, declaration), Some(field));
            }
        },
    );
}

#[test]
fn primary_constructor_parameter_and_private_var_remain_separate_identities() {
    with_source(
        "class Counter(start: Int) { private var current: Int = start; def get(): Int = current }",
        |ast, source, index, definitions, typer| {
            let class = symbol_named(ast, index, source, typer, "Counter");
            let (field, _, rhs) = field_tree_and_rhs(ast, index, source, typer, "current");
            let template = ast
                .iter()
                .find_map(|(_, node)| match &node.kind {
                    TreeKind::Template(template) => Some(template),
                    _ => None,
                })
                .unwrap();
            let method = symbol_named(ast, index, source, typer, "get");
            let method_rhs = ast
                .iter()
                .find_map(|(tree, node)| {
                    let TreeKind::DefDef(definition) = &node.kind else {
                        return None;
                    };
                    (index.symbol_at(source, tree) == Some(method))
                        .then_some(definition.rhs)
                        .flatten()
                })
                .expect("get method should have a source body");
            assert_eq!(
                template
                    .body
                    .iter()
                    .filter_map(|tree| index.symbol_at(source, *tree))
                    .collect::<Vec<_>>(),
                vec![field, method]
            );
            let TreeKind::DefDef(constructor_def) = &ast.get(template.constructor).kind else {
                panic!("primary constructor should be a DefDef");
            };
            assert_eq!(constructor_def.value_param_clauses.len(), 1);
            assert_eq!(constructor_def.value_param_clauses[0].len(), 1);
            let constructor = index.symbol_at(source, template.constructor).unwrap();
            assert_eq!(
                typer.store().symbols.get(constructor).kind,
                SymbolKind::Constructor
            );
            assert_eq!(typer.store().symbols.get(constructor).owner, Some(class));
            assert_eq!(
                typer.store().symbols.get(constructor).visibility,
                Visibility::Public
            );
            let parameter_tree = constructor_def.value_param_clauses[0][0];
            let canonical_parameter = index.symbol_at(source, parameter_tree).unwrap();
            let constructor_parameter = index
                .derived_symbol_at(constructor, source, parameter_tree)
                .unwrap();

            assert_eq!(
                typer.store().symbols.get(canonical_parameter).kind,
                SymbolKind::Parameter
            );
            assert_eq!(
                typer.store().symbols.get(canonical_parameter).owner,
                Some(class)
            );
            assert_eq!(
                typer.store().symbols.get(canonical_parameter).visibility,
                Visibility::Private
            );
            assert_eq!(
                typer.store().symbols.get(constructor_parameter).visibility,
                Visibility::Public
            );
            assert_eq!(typer.store().symbols.get(field).kind, SymbolKind::Field);
            assert_eq!(typer.store().symbols.get(field).owner, Some(class));
            assert!(
                typer
                    .store()
                    .symbols
                    .get(field)
                    .flags
                    .contains(SymbolFlags::MUTABLE)
            );
            assert_eq!(
                typer.store().symbols.get(field).visibility,
                Visibility::Private
            );
            assert_ne!(canonical_parameter, constructor_parameter);
            assert_eq!(
                typer.store().symbols.get(canonical_parameter).owner,
                Some(class)
            );
            assert_eq!(
                typer.store().symbols.get(constructor_parameter).kind,
                SymbolKind::Parameter
            );
            assert_eq!(
                typer.store().symbols.get(constructor_parameter).owner,
                Some(constructor)
            );
            assert!(
                !typer
                    .store()
                    .symbols
                    .get(constructor_parameter)
                    .flags
                    .contains(SymbolFlags::MUTABLE)
            );
            assert_ne!(field, constructor_parameter);

            typer.complete_symbol(constructor).unwrap();
            typer.complete_symbol(class).unwrap();
            let SymbolInfo::Complete(constructor_type) =
                typer.store().symbols.get(constructor).info
            else {
                panic!("constructor signature should be complete");
            };
            let Type::Method(signature) = typer.store().types.get(constructor_type) else {
                panic!("ordinary primary constructor should have one method clause");
            };
            assert_eq!(signature.params.len(), 1);
            assert_eq!(signature.kind, dotty_core::MethodKind::Plain);
            assert_eq!(signature.params[0].ty, definitions.int);
            let constructor_parameter_type = signature.params[0].ty;
            assert_eq!(
                typer.store().types.get(signature.result).reference_symbol(),
                Some(class)
            );
            assert!(matches!(
                typer.store().symbols.get(class).info,
                SymbolInfo::Complete(_)
            ));

            typer.complete_symbol(field).unwrap();
            let context = typer.field_initializer_context_for(field).unwrap();
            let typed_rhs = typer.type_expression(rhs, context).unwrap();
            assert_eq!(typer.type_expression(rhs, context).unwrap(), typed_rhs);
            assert_eq!(typer.source_typed_index().get(source, rhs), Some(typed_rhs));
            assert!(matches!(
                typer
                    .store()
                    .types
                    .get(typer.typed_ast().get(typed_rhs).ty),
                Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                    if *symbol == constructor_parameter
            ));
            assert_eq!(
                typer.store().types.get(constructor_parameter_type),
                typer.store().types.get(definitions.int)
            );

            typer.complete_symbol(method).unwrap();
            let method_context = typer.expression_context_for(method).unwrap();
            let typed_method_rhs = typer.type_expression(method_rhs, method_context).unwrap();
            assert!(matches!(
                typer
                    .store()
                    .types
                    .get(typer.typed_ast().get(typed_method_rhs).ty),
                Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                    if *symbol == field
            ));
        },
    );
}

#[test]
fn val_and_var_constructor_parameters_have_distinct_field_and_parameter_symbols() {
    for source_text in ["class C(val x: Int)", "class C(var x: Int)"] {
        with_source(source_text, |ast, source, index, _, typer| {
            let class = symbol_named(ast, index, source, typer, "C");
            let template = ast
                .iter()
                .find_map(|(_, node)| match &node.kind {
                    TreeKind::Template(template) => Some(template),
                    _ => None,
                })
                .unwrap();
            let TreeKind::DefDef(constructor_def) = &ast.get(template.constructor).kind else {
                panic!("primary constructor should be a DefDef");
            };
            let constructor = index.symbol_at(source, template.constructor).unwrap();
            let parameter_tree = constructor_def.value_param_clauses[0][0];
            let field = index.symbol_at(source, parameter_tree).unwrap();
            let parameter = index
                .derived_symbol_at(constructor, source, parameter_tree)
                .unwrap();

            assert_ne!(field, parameter);
            assert_eq!(typer.store().symbols.get(field).kind, SymbolKind::Field);
            assert_eq!(typer.store().symbols.get(field).owner, Some(class));
            assert_eq!(
                typer.store().symbols.get(parameter).kind,
                SymbolKind::Parameter
            );
            assert_eq!(
                typer.store().symbols.get(parameter).owner,
                Some(constructor)
            );
            assert_eq!(
                typer
                    .store()
                    .symbols
                    .get(field)
                    .flags
                    .contains(SymbolFlags::MUTABLE),
                source_text.contains("var x")
            );
        });
    }
}

#[test]
fn wrong_typed_field_initializer_is_a_typer_error_and_rolls_back_mapping() {
    with_source(
        "class Broken { private val count: Int = true }",
        |ast, source, index, definitions, typer| {
            let (field, _, rhs) = field_tree_and_rhs(ast, index, source, typer, "count");
            typer.complete_symbol(field).unwrap();
            let context = typer.field_initializer_context_for(field).unwrap();
            let store_checkpoint = typer.store().checkpoint();
            let typed_checkpoint = typer.typed_ast().checkpoint();

            let result = typer.type_expression_expected(rhs, context, definitions.int);
            assert!(
                matches!(
                    result,
                    Err(dotty_typer::TyperError::ExpectedExpressionTypeMismatch {
                        source: actual_source,
                        tree_index,
                        expected,
                        ..
                    }) if actual_source == source && tree_index == rhs.index() && expected == definitions.int
                ),
                "unexpected typing result: {result:?}"
            );
            assert_eq!(typer.store().checkpoint(), store_checkpoint);
            assert_eq!(typer.typed_ast().checkpoint(), typed_checkpoint);
            assert_eq!(typer.source_typed_index().get(source, rhs), None);
        },
    );
}

#[test]
fn duplicate_source_fields_are_not_yet_rejected_by_frontend() {
    let source_text = "class Duplicate { val value: Int = 1; val value: Int = 2 }";
    let source_id = SourceId::from_index(1);
    let source = SourceText::new(source_text).unwrap();
    let scanner = ContextualScanner::new(source_text).unwrap();
    assert!(scanner.diagnostics().is_empty());
    let mut store = SemanticStore::new();
    let parsed = parse_compilation_unit(source, source_id, scanner, &mut store.names);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let outcome = name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        "Duplicate.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap();
    let index = match outcome {
        NamingOutcome::Named { index, diagnostics } => {
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
            index
        }
        NamingOutcome::Rejected { diagnostics } => {
            panic!("this audit expects to observe current namer behavior: {diagnostics:?}")
        }
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::TypeDef(class_def) = &parsed.ast.get(package.stats[0]).kind else {
        panic!("source declaration should be a class");
    };
    let class = index.symbol_at(source_id, package.stats[0]).unwrap();
    let TreeKind::Template(template) = &parsed.ast.get(class_def.rhs).kind else {
        panic!("class RHS should be a template");
    };
    let duplicate_fields = template
        .body
        .iter()
        .filter_map(|tree| {
            let symbol = index.symbol_at(source_id, *tree)?;
            (store.symbols.get(symbol).kind == SymbolKind::Field).then_some(symbol)
        })
        .collect::<Vec<_>>();
    assert_eq!(duplicate_fields.len(), 2);
    assert_ne!(duplicate_fields[0], duplicate_fields[1]);
    for field in duplicate_fields {
        assert_eq!(store.symbols.get(field).owner, Some(class));
        assert_eq!(
            store.names.resolve(store.symbols.get(field).name.text()),
            "value"
        );
    }
}
