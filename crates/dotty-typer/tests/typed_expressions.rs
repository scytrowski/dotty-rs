//! End-to-end typed-expression checks over synthetic source.

use dotty_core::ast::TreeKind;
use dotty_core::types::{ClassInfo, TermRefTarget, Type, TypeRefTarget};
use dotty_core::{
    Definitions, NoResolver, Packages, SemanticStore, SourceId, SourceText, SymbolInfo, SymbolKind,
    TreeId, Untyped,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;
use dotty_typer::{ExpressionContext, SourceTyper, TyperError};

const NULL_REFERENCE_IDENTITY_SOURCE: &str =
    include_str!("../../../tests/fixtures/jvm/NullReferenceIdentity.scala");
const NULL_TYPING_SOURCE: &str = include_str!("fixtures/null-typing/NullTyping.scala");

fn class_definition(
    parsed: &dotty_parser::ParseResult,
    store: &SemanticStore,
    index: &dotty_core::SourceSemanticIndex,
    source: SourceId,
    wanted: &str,
) -> (dotty_core::SymbolId, TreeId<Untyped>) {
    parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::TypeDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == wanted)
                .then(|| (index.symbol_at(source, tree).unwrap(), tree))
        })
        .unwrap_or_else(|| panic!("missing class {wanted}"))
}

fn class_template(
    parsed: &dotty_parser::ParseResult,
    class_tree: TreeId<Untyped>,
) -> &dotty_core::ast::Template<Untyped> {
    let TreeKind::TypeDef(definition) = &parsed.ast.get(class_tree).kind else {
        panic!("class identity points to a type definition")
    };
    let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
        panic!("class definition contains a template")
    };
    template
}

#[test]
fn types_a_small_source_program_with_no_resolver_or_jdk_fixture() {
    let text = "class C { def answer: Int = 42 }";
    let source = SourceId::from_index(0);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(text).unwrap();
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "no-resolver.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let (method, rhs) = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == "answer").then(|| {
                (
                    index.symbol_at(source, tree).unwrap(),
                    definition.rhs.unwrap(),
                )
            })
        })
        .expect("source should contain method answer");
    let context = ExpressionContext {
        lexical: index.declaration_context_of(method).unwrap(),
        owner: method,
        local_scopes: None,
    };
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(NoResolver));

    let typed = typer.type_expression(rhs, context).unwrap();

    assert_eq!(
        typer
            .widen_expression_type(typer.typed_ast().get(typed).ty)
            .unwrap(),
        definitions.int
    );
    assert!(matches!(
        typer.typed_ast().get(typed).kind,
        TreeKind::Literal(_)
    ));
}

#[test]
fn source_null_literal_uses_the_canonical_null_type() {
    let source_text = NULL_TYPING_SOURCE;
    let source = SourceId::from_index(1469);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(source_text).unwrap();
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "NullTyping.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let methods = parsed
        .ast
        .iter()
        .filter_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            let rhs = definition.rhs?;
            Some((
                store
                    .names
                    .resolve(definition.name.as_name().text())
                    .to_owned(),
                index.symbol_at(source, tree).unwrap(),
                definition.tpt,
                rhs,
            ))
        })
        .collect::<Vec<_>>();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let generic_class = class_definition(&parsed, typer.store(), &index, source, "Generic").0;
    let box_class = class_definition(&parsed, typer.store(), &index, source, "Box").0;
    typer.complete_symbol(generic_class).unwrap();
    typer.complete_symbol(box_class).unwrap();

    let find_method = |name: &str| {
        methods
            .iter()
            .find(|(method_name, ..)| method_name == name)
            .map(|(_, method, tpt, rhs)| (*method, *tpt, *rhs))
            .unwrap_or_else(|| panic!("fixture should contain method {name}"))
    };
    let (exact, exact_tpt, exact_rhs) = find_method("exact");
    assert_eq!(
        typer
            .type_of_tpt(exact_tpt, index.declaration_context_of(exact).unwrap())
            .unwrap(),
        definitions.null_type
    );
    let exact_context = typer.expression_context_for(exact).unwrap();
    let typed_exact = typer
        .type_expression_expected(exact_rhs, exact_context, definitions.null_type)
        .unwrap();
    assert_eq!(typer.typed_ast().get(typed_exact).ty, definitions.null_type);
    assert!(matches!(
        typer.typed_ast().get(typed_exact).kind,
        TreeKind::Literal(_)
    ));

    let (inferred, _, inferred_rhs) = find_method("inferredNull");
    let inferred_context = typer.expression_context_for(inferred).unwrap();
    let typed_inferred = typer
        .type_expression(inferred_rhs, inferred_context)
        .unwrap();
    assert_eq!(
        typer.typed_ast().get(typed_inferred).ty,
        definitions.null_type
    );

    for name in ["choose", "chooseReverse"] {
        let (method, result_tpt, rhs) = find_method(name);
        let expected = typer
            .type_of_tpt(result_tpt, index.declaration_context_of(method).unwrap())
            .unwrap();
        let context = typer.expression_context_for(method).unwrap();
        let typed = typer
            .type_expression_expected(rhs, context, expected)
            .unwrap();
        assert_eq!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(typed).ty)
                .reference_symbol(),
            typer.store().types.get(expected).reference_symbol(),
            "{name} should preserve the expected source class identity"
        );
        let TreeKind::If(if_expression) = &typer.typed_ast().get(typed).kind else {
            panic!("{name} should produce a typed If")
        };
        let null_branch = if name == "choose" {
            if_expression.then_branch
        } else {
            if_expression.else_branch
        };
        assert_eq!(
            typer.typed_ast().get(null_branch).ty,
            definitions.null_type,
            "the null literal keeps its source type in {name}"
        );
    }

    let (local, local_tpt, local_rhs) = find_method("local");
    let expected = typer
        .type_of_tpt(local_tpt, index.declaration_context_of(local).unwrap())
        .unwrap();
    let local_context = typer.expression_context_for(local).unwrap();
    let typed_local = typer
        .type_expression_expected(local_rhs, local_context, expected)
        .unwrap();
    let local_result = typer
        .widen_expression_type(typer.typed_ast().get(typed_local).ty)
        .unwrap();
    assert_eq!(
        typer.store().types.get(local_result).reference_symbol(),
        typer.store().types.get(expected).reference_symbol()
    );

    for (name, expected) in [
        ("badInt", definitions.int),
        ("badBoolean", definitions.boolean),
    ] {
        let (method, _, rhs) = find_method(name);
        let context = typer.expression_context_for(method).unwrap();
        assert!(matches!(
            typer.type_expression_expected(rhs, context, expected),
            Err(TyperError::ExpectedExpressionTypeMismatch { actual, expected: actual_expected, .. })
                if actual == definitions.null_type && actual_expected == expected
        ));
        assert!(typer.source_typed_index().get(source, rhs).is_none());
    }

    let (bad_generic, generic_tpt, generic_rhs) = find_method("badRawGeneric");
    let expected = typer
        .type_of_tpt(
            generic_tpt,
            index.declaration_context_of(bad_generic).unwrap(),
        )
        .unwrap();
    assert_eq!(
        typer.store().types.get(expected).reference_symbol(),
        Some(generic_class)
    );
    let context = typer.expression_context_for(bad_generic).unwrap();
    assert!(matches!(
        typer.type_expression_expected(generic_rhs, context, expected),
        Err(TyperError::ExpectedExpressionTypeMismatch {
            actual,
            expected: actual_expected,
            ..
        }) if actual == definitions.null_type && actual_expected == expected
    ));
    assert!(
        typer
            .source_typed_index()
            .get(source, generic_rhs)
            .is_none()
    );

    let null_literals = typer
        .typed_ast()
        .iter()
        .filter_map(|(_, node)| match &node.kind {
            TreeKind::Literal(literal) if literal.value == dotty_core::Constant::Null => {
                Some(node.ty)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(null_literals.len() >= 5);
    assert!(
        null_literals
            .into_iter()
            .all(|ty| ty == definitions.null_type)
    );
}

#[test]
fn null_reference_identity_fixture_freezes_current_typer_boundaries() {
    let source_text = NULL_REFERENCE_IDENTITY_SOURCE;
    let source = SourceId::from_index(1468);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(source_text).unwrap();
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "NullReferenceIdentity.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let methods = parsed
        .ast
        .iter()
        .filter_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            let name = store.names.resolve(definition.name.as_name().text());
            matches!(name, "same" | "different" | "choose" | "readOrDefault").then(|| {
                (
                    name.to_owned(),
                    index.symbol_at(source, tree).unwrap(),
                    definition.rhs.unwrap(),
                )
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        methods.len(),
        4,
        "the candidate source shape must be parsed"
    );

    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let box_class = class_definition(&parsed, typer.store(), &index, source, "Box").0;
    typer.complete_symbol(box_class).unwrap();
    let mut failures = Vec::new();
    for (name, method, rhs) in methods {
        let context = typer.expression_context_for(method).unwrap();
        if name == "choose" {
            let typed = typer.type_expression(rhs, context).unwrap();
            assert_eq!(
                typer
                    .store()
                    .types
                    .get(typer.typed_ast().get(typed).ty)
                    .reference_symbol(),
                Some(box_class)
            );
            let TreeKind::If(if_expression) = &typer.typed_ast().get(typed).kind else {
                panic!("choose should remain a typed If expression")
            };
            assert_eq!(
                typer.typed_ast().get(if_expression.then_branch).ty,
                definitions.null_type
            );
            continue;
        }
        let Err(error) = typer.type_expression(rhs, context) else {
            panic!("{name} unexpectedly typed")
        };
        assert!(
            typer.source_typed_index().get(source, rhs).is_none(),
            "failed typing of {name} must not publish a typed root"
        );
        failures.push((name, error));
    }

    for (name, error) in failures {
        match (name.as_str(), error) {
            ("same", TyperError::MemberNotFound { name, .. })
                if typer.store().names.resolve(name.text()) == "eq" => {}
            ("different", TyperError::MemberNotFound { name, .. })
                if typer.store().names.resolve(name.text()) == "ne" => {}
            ("readOrDefault", TyperError::MemberNotFound { name, .. })
                if typer.store().names.resolve(name.text()) == "eq" => {}
            (method, error) => panic!("unexpected first typing failure for {method}: {error:?}"),
        }
    }
}

#[test]
fn types_a_synthetic_field_selection_through_the_public_pipeline() {
    let text = "class C { val value: Int = 1; def use: Int = this.value }";
    let source = SourceId::from_index(0);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(text).unwrap();
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "typed-expressions.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let (method, rhs) = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == "use").then(|| {
                (
                    index.symbol_at(source, tree).unwrap(),
                    definition.rhs.unwrap(),
                )
            })
        })
        .expect("source should contain method use");
    let context = ExpressionContext {
        lexical: index.declaration_context_of(method).unwrap(),
        owner: method,
        local_scopes: None,
    };
    let source_position = parsed.ast.get(rhs).position;
    let field = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::ValDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == "value")
                .then(|| index.symbol_at(source, tree).unwrap())
        })
        .expect("source should contain field value");
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );

    let typed = typer.type_expression(rhs, context).unwrap();

    assert!(matches!(
        typer.typed_ast().get(typed).kind,
        TreeKind::Select(_)
    ));
    assert!(matches!(
        typer.store().types.get(typer.typed_ast().get(typed).ty),
        Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == field
    ));
    let widened = typer
        .widen_expression_type(typer.typed_ast().get(typed).ty)
        .unwrap();
    assert_eq!(widened, definitions.int);
    assert_eq!(typer.typed_ast().get(typed).position, source_position);
    assert_eq!(typer.source_typed_index().get(source, rhs), Some(typed));
    for (_, node) in typer.typed_ast().iter() {
        assert!(typer.store().types.contains(node.ty));
    }
}

#[test]
fn types_a_monomorphic_application_through_the_public_pipeline() {
    let text = "class C { def inc(x: Int): Int = x; def use: Int = inc(1) }";
    let source = SourceId::from_index(0);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(text).unwrap();
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "typed-application.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let (method, rhs) = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == "use").then(|| {
                (
                    index.symbol_at(source, tree).unwrap(),
                    definition.rhs.unwrap(),
                )
            })
        })
        .expect("source should contain method use");
    let context = ExpressionContext {
        lexical: index.declaration_context_of(method).unwrap(),
        owner: method,
        local_scopes: None,
    };
    let source_position = parsed.ast.get(rhs).position;
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );

    let typed = typer.type_expression(rhs, context).unwrap();

    let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
        panic!("source method call should produce a typed Apply");
    };
    assert_eq!(typer.typed_ast().get(typed).ty, definitions.int);
    assert_eq!(application.kind, dotty_core::ast::ApplyKind::Regular);
    assert!(matches!(
        typer
            .store()
            .types
            .get(typer.typed_ast().get(application.args[0]).ty),
        Type::Constant(dotty_core::Constant::Int(1))
    ));
    assert_eq!(typer.typed_ast().get(typed).position, source_position);
    assert_eq!(typer.source_typed_index().get(source, rhs), Some(typed));
    for (_, node) in typer.typed_ast().iter() {
        assert!(typer.store().types.contains(node.ty));
    }
}

#[test]
fn types_explicit_super_calls_to_direct_parent_method_symbols() {
    let text = "class Base { def value(): Int = 10; def plainValue: Int = 12; def add(x: Int): Int = x + 2 }; class Derived extends Base { override def value(): Int = super.value() + 1; def direct(): Int = super.add(4); def parameterless(): Int = super.plainValue; def virtualCall(): Int = this.add(4) }";
    let source = SourceId::from_index(0);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(text).unwrap();
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "super-typing.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let base_class = class_definition(&parsed, &store, &index, source, "Base").0;
    let derived_class = class_definition(&parsed, &store, &index, source, "Derived").0;
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );

    let mut typed_methods = std::collections::HashMap::new();
    let mut source_super_trees = Vec::new();
    let mut method_symbols = std::collections::HashMap::new();
    for (tree, node) in parsed.ast.iter() {
        if let TreeKind::Super(_) = node.kind {
            source_super_trees.push(tree);
        }
        let TreeKind::DefDef(definition) = &node.kind else {
            continue;
        };
        let method_name = typer
            .store()
            .names
            .resolve(definition.name.as_name().text())
            .to_owned();
        let method = index.symbol_at(source, tree).unwrap();
        if method_name == "add" {
            method_symbols.insert("base_add", method);
        }
        if method_name == "plainValue" {
            method_symbols.insert("base_plain_value", method);
        }
        if method_name == "value" && typer.store().symbols.get(method).owner == Some(base_class) {
            method_symbols.insert("base_value", method);
        }
        if let Some(rhs) = definition.rhs
            && matches!(
                method_name.as_str(),
                "value" | "direct" | "parameterless" | "virtualCall"
            )
        {
            let context = ExpressionContext {
                lexical: index.declaration_context_of(method).unwrap(),
                owner: method,
                local_scopes: None,
            };
            let typed = typer.type_expression(rhs, context).unwrap_or_else(|error| {
                panic!(
                    "typing {method_name}: {error:?}; source={:?}",
                    parsed.ast.get(rhs).kind
                )
            });
            assert_eq!(typer.type_expression(rhs, context).unwrap(), typed);
            typed_methods.insert(method_name, typed);
        }
    }

    let base_value = method_symbols["base_value"];
    let base_add = method_symbols["base_add"];
    let base_plain_value = method_symbols["base_plain_value"];
    let selected_symbols = typed_methods
        .values()
        .flat_map(|root| {
            let mut pending = vec![*root];
            let mut selected = Vec::new();
            while let Some(tree) = pending.pop() {
                let node = typer.typed_ast().get(tree);
                if let TreeKind::Select(selection) = &node.kind
                    && let Type::TermRef {
                        target: TermRefTarget::Symbol(symbol),
                        ..
                    } = typer.store().types.get(node.ty)
                {
                    selected.push((*symbol, selection.qualifier));
                }
                pending.extend(match &node.kind {
                    TreeKind::Select(selection) => vec![selection.qualifier],
                    TreeKind::Apply(application) => std::iter::once(application.function)
                        .chain(application.args.iter().copied())
                        .collect(),
                    TreeKind::Super(super_tree) => vec![super_tree.qual],
                    TreeKind::Block(block) => block
                        .stats
                        .iter()
                        .copied()
                        .chain(std::iter::once(block.expr))
                        .collect(),
                    _ => Vec::new(),
                });
            }
            selected
        })
        .collect::<Vec<_>>();
    assert!(selected_symbols.iter().any(|(symbol, qualifier)| {
        *symbol == base_value
            && matches!(typer.typed_ast().get(*qualifier).kind, TreeKind::Super(_))
    }));
    assert!(selected_symbols.iter().any(|(symbol, qualifier)| {
        *symbol == base_add && matches!(typer.typed_ast().get(*qualifier).kind, TreeKind::Super(_))
    }));
    assert!(selected_symbols.iter().any(|(symbol, qualifier)| {
        *symbol == base_add && matches!(typer.typed_ast().get(*qualifier).kind, TreeKind::This(_))
    }));
    assert!(selected_symbols.iter().any(|(symbol, qualifier)| {
        *symbol == base_plain_value
            && matches!(typer.typed_ast().get(*qualifier).kind, TreeKind::Super(_))
    }));
    let plain_value_selection = parsed
        .ast
        .iter()
        .find_map(|(source_tree, node)| {
            let TreeKind::Select(selection) = &node.kind else {
                return None;
            };
            (typer.store().names.resolve(selection.name.text()) == "plainValue"
                && matches!(parsed.ast.get(selection.qualifier).kind, TreeKind::Super(_)))
            .then(|| typer.source_typed_index().get(source, source_tree).unwrap())
        })
        .unwrap();
    let plain_value_type = typer.typed_ast().get(plain_value_selection).ty;
    assert!(matches!(
        typer.store().types.get(plain_value_type),
        Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) }
            if *symbol == base_plain_value
                && matches!(typer.store().types.get(*prefix), Type::SuperType { .. })
    ));
    assert_eq!(
        typer.widen_expression_type(plain_value_type).unwrap(),
        definitions.int
    );
    assert_eq!(source_super_trees.len(), 3);
    for source_super in source_super_trees {
        let typed_super = typer
            .source_typed_index()
            .get(source, source_super)
            .unwrap();
        assert!(matches!(
            typer.typed_ast().get(typed_super).kind,
            TreeKind::Super(_)
        ));
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(typed_super).ty),
            Type::SuperType { this_type, super_type }
                if matches!(typer.store().types.get(*this_type), Type::ThisType { class } if *class == derived_class)
                    && matches!(typer.store().types.get(*super_type), Type::TypeRef { target: TypeRefTarget::Symbol(class), .. } if *class == base_class)
        ));
    }
    for (source_tree, source_node) in parsed.ast.iter() {
        let is_super_selection = matches!(
            &source_node.kind,
            TreeKind::Select(selection)
                if matches!(parsed.ast.get(selection.qualifier).kind, TreeKind::Super(_))
        );
        let is_super_application = matches!(
            &source_node.kind,
            TreeKind::Apply(application)
                if matches!(
                    &parsed.ast.get(application.function).kind,
                    TreeKind::Select(selection)
                        if matches!(parsed.ast.get(selection.qualifier).kind, TreeKind::Super(_))
                )
        );
        if is_super_selection || is_super_application {
            assert!(
                typer
                    .source_typed_index()
                    .get(source, source_tree)
                    .is_some()
            );
        }
    }
}

#[test]
fn rejects_unsupported_super_owners_members_and_application_shapes() {
    let text = "class Root { def bad(): Int = super.value() }; class Base { def add(x: Int): Int = x; def value(): Int = 1; def plainValue: Int = 2; def abstractValue(): Int; def bodyless(): Int; def staticTarget(): Int = 3; def extensionTarget(): Int = 4 }; class Derived extends Base { def missing(): Int = super.missing(); def arity(): Int = super.add(); def argument(): Int = super.add(true); def qualified(): Int = super[Base].value(); def parameterlessApply(): Int = super.plainValue(); def abstractTarget(): Int = super.abstractValue(); def bodylessTarget(): Int = super.bodyless(); def staticFlagTarget(): Int = super.staticTarget(); def extensionFlagTarget(): Int = super.extensionTarget() }";
    let source = SourceId::from_index(1);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(text).unwrap();
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "unsupported-super-typing.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    for (tree, node) in parsed.ast.iter() {
        let TreeKind::DefDef(definition) = &node.kind else {
            continue;
        };
        let flag = match store.names.resolve(definition.name.as_name().text()) {
            "staticTarget" => Some(dotty_core::SymbolFlags::STATIC),
            "extensionTarget" => Some(dotty_core::SymbolFlags::EXTENSION),
            _ => None,
        };
        if let Some(flag) = flag {
            let symbol = index.symbol_at(source, tree).unwrap();
            let flags = store.symbols.get(symbol).flags | flag;
            store.symbols.get_mut(symbol).flags = flags;
        }
    }
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let mut outcomes = std::collections::HashMap::new();
    for (tree, node) in parsed.ast.iter() {
        let TreeKind::DefDef(definition) = &node.kind else {
            continue;
        };
        let name = typer
            .store()
            .names
            .resolve(definition.name.as_name().text())
            .to_owned();
        if !matches!(
            name.as_str(),
            "bad"
                | "missing"
                | "arity"
                | "argument"
                | "qualified"
                | "parameterlessApply"
                | "abstractTarget"
                | "bodylessTarget"
                | "staticFlagTarget"
                | "extensionFlagTarget"
        ) {
            continue;
        }
        let Some(rhs) = definition.rhs else { continue };
        let method = index.symbol_at(source, tree).unwrap();
        let context = ExpressionContext {
            lexical: index.declaration_context_of(method).unwrap(),
            owner: method,
            local_scopes: None,
        };
        outcomes.insert(name, typer.type_expression(rhs, context).unwrap_err());
        assert!(typer.source_typed_index().get(source, rhs).is_none());
    }
    assert!(matches!(
        outcomes["bad"],
        TyperError::UnsupportedSuper {
            reason: "super requires exactly one direct non-Object class parent",
            ..
        }
    ));
    assert!(matches!(
        outcomes["missing"],
        TyperError::MemberNotFound { .. }
    ));
    assert!(matches!(
        outcomes["arity"],
        TyperError::ApplicationArityMismatch {
            expected: 1,
            actual: 0,
            ..
        }
    ));
    assert!(matches!(
        outcomes["argument"],
        TyperError::ApplicationArgumentTypeMismatch { .. }
    ));
    assert!(matches!(
        outcomes["qualified"],
        TyperError::UnsupportedSuper {
            reason: "trait-qualified super is not supported",
            ..
        }
    ));
    assert!(
        matches!(
            outcomes["parameterlessApply"],
            TyperError::ApplicationCalleeNotMethod { .. }
        ),
        "{:?}",
        outcomes["parameterlessApply"]
    );
    for method in ["abstractTarget", "bodylessTarget"] {
        assert!(matches!(
            outcomes[method],
            TyperError::UnsupportedSuper { .. }
        ));
    }
    for method in ["staticFlagTarget", "extensionFlagTarget"] {
        assert!(matches!(
            outcomes[method],
            TyperError::UnsupportedSuper {
                reason: "selected parent member is not a public concrete instance method",
                ..
            }
        ));
    }
}

#[test]
fn parent_constructor_arguments_keep_child_parameter_and_source_identities() {
    let text = "class Base(seed: Int, enabled: Boolean) {}\nclass ByParameter(start: Int, active: Boolean) extends Base(start, active) {}\nclass ByLiteral(start: Int) extends Base(42, true) {}\nclass Dep {}\nclass RefBase(dep: Dep) {}\nclass RefChild(dep: Dep) extends RefBase(dep) {}\n";
    let source = SourceId::from_index(1370);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(text).unwrap();
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "parent-constructor-arguments.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let (base, base_tree) = class_definition(&parsed, &store, &index, source, "Base");
    let (by_parameter, by_parameter_tree) =
        class_definition(&parsed, &store, &index, source, "ByParameter");
    let (by_literal, by_literal_tree) =
        class_definition(&parsed, &store, &index, source, "ByLiteral");
    let (ref_base, ref_base_tree) = class_definition(&parsed, &store, &index, source, "RefBase");
    let (ref_child, ref_child_tree) = class_definition(&parsed, &store, &index, source, "RefChild");
    let base_constructor = index
        .symbol_at(source, class_template(&parsed, base_tree).constructor)
        .unwrap();
    let ref_base_constructor = index
        .symbol_at(source, class_template(&parsed, ref_base_tree).constructor)
        .unwrap();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );

    for class in [by_parameter, by_literal, ref_child] {
        typer.complete_symbol(class).unwrap();
    }

    for (class, class_tree, expected_parent) in [
        (by_parameter, by_parameter_tree, base),
        (by_literal, by_literal_tree, base),
        (ref_child, ref_child_tree, ref_base),
    ] {
        let template = class_template(&parsed, class_tree);
        let [parent_application] = template.parents.as_slice() else {
            panic!("expected exactly one parent application")
        };
        let typed_application = typer
            .source_typed_index()
            .get(source, *parent_application)
            .expect("parent constructor application has a typed replacement");
        let completed_info = match *typer.store().symbols.info(class) {
            SymbolInfo::Complete(info) => info,
            _ => unreachable!(),
        };
        assert_eq!(typer.complete_symbol(class).unwrap(), completed_info);
        assert_eq!(
            typer.source_typed_index().get(source, *parent_application),
            Some(typed_application),
            "typing the completed class again preserves the same mapping"
        );
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed_application).kind else {
            panic!("parent constructor call retains its application node")
        };
        let typed_function = application.function;
        let TreeKind::Select(_) = &typer.typed_ast().get(typed_function).kind else {
            panic!("parent constructor target retains the selected constructor")
        };
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed_function).ty),
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } if *symbol == if expected_parent == base { base_constructor } else { ref_base_constructor }
        ));
        let TreeKind::Apply(source_application) = &parsed.ast.get(*parent_application).kind else {
            unreachable!()
        };
        assert_eq!(application.args.len(), source_application.args.len());
        for (source_argument, typed_argument) in
            source_application.args.iter().zip(&application.args)
        {
            assert_eq!(
                typer.source_typed_index().get(source, *source_argument),
                Some(*typed_argument),
                "each original argument maps directly to its typed expression"
            );
        }
        let SymbolInfo::Complete(info) = *typer.store().symbols.info(class) else {
            panic!("class completion publishes ClassInfo")
        };
        let Type::ClassInfo(ClassInfo { parents, .. }) = typer.store().types.get(info) else {
            panic!("class completion preserves ClassInfo")
        };
        assert!(matches!(
            typer.store().types.get(parents[0]),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }
                if *symbol == expected_parent
        ));
    }

    let template = class_template(&parsed, by_parameter_tree);
    let TreeKind::DefDef(constructor) = &parsed.ast.get(template.constructor).kind else {
        panic!("child class has a primary constructor")
    };
    let constructor_symbol = index.symbol_at(source, template.constructor).unwrap();
    let parameter_symbols = constructor.value_param_clauses[0]
        .iter()
        .map(|tree| {
            index
                .derived_symbol_at(constructor_symbol, source, *tree)
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert!(parameter_symbols.iter().all(|symbol| {
        typer.store().symbols.get(*symbol).owner == Some(constructor_symbol)
            && typer.store().symbols.get(*symbol).kind == SymbolKind::Parameter
    }));
    let parent_application = template.parents[0];
    let typed_application = typer
        .source_typed_index()
        .get(source, parent_application)
        .unwrap();
    let TreeKind::Apply(application) = &typer.typed_ast().get(typed_application).kind else {
        unreachable!()
    };
    for (argument, expected_symbol) in application.args.iter().zip(parameter_symbols) {
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(*argument).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                if *symbol == expected_symbol
        ));
    }

    let ref_template = class_template(&parsed, ref_child_tree);
    let TreeKind::DefDef(ref_constructor) = &parsed.ast.get(ref_template.constructor).kind else {
        panic!("reference child has a primary constructor")
    };
    let ref_constructor_symbol = index.symbol_at(source, ref_template.constructor).unwrap();
    let ref_parameter_symbol = index
        .derived_symbol_at(
            ref_constructor_symbol,
            source,
            ref_constructor.value_param_clauses[0][0],
        )
        .unwrap();
    let ref_parent_typed = typer
        .source_typed_index()
        .get(source, ref_template.parents[0])
        .unwrap();
    let TreeKind::Apply(ref_application) = &typer.typed_ast().get(ref_parent_typed).kind else {
        unreachable!()
    };
    assert!(matches!(
        typer.store().types.get(typer.typed_ast().get(ref_application.args[0]).ty),
        Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
            if *symbol == ref_parameter_symbol
    ));

    let literal_template = class_template(&parsed, by_literal_tree);
    let literal_parent = literal_template.parents[0];
    let literal_typed = typer
        .source_typed_index()
        .get(source, literal_parent)
        .unwrap();
    let TreeKind::Apply(literal_application) = &typer.typed_ast().get(literal_typed).kind else {
        unreachable!()
    };
    assert_eq!(literal_application.args.len(), 2);
    assert!(matches!(
        typer
            .store()
            .types
            .get(typer.typed_ast().get(literal_application.args[0]).ty),
        Type::Constant(dotty_core::Constant::Int(42))
    ));
    assert!(matches!(
        typer
            .store()
            .types
            .get(typer.typed_ast().get(literal_application.args[1]).ty),
        Type::Constant(dotty_core::Constant::Boolean(true))
    ));
}

#[test]
fn invalid_parent_constructor_arguments_roll_back_typer_state() {
    let text = "class Base(seed: Int) {}\nclass Derived extends Base(true) {}\n";
    let source = SourceId::from_index(1371);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(text).unwrap();
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "invalid-parent-constructor-argument.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let (derived, derived_tree) = class_definition(&parsed, &store, &index, source, "Derived");
    let (base, _) = class_definition(&parsed, &store, &index, source, "Base");
    let parent_application = class_template(&parsed, derived_tree).parents[0];
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );

    let error = typer.complete_symbol(derived).unwrap_err();

    assert!(
        matches!(
            error,
            dotty_typer::TyperError::ApplicationArgumentTypeMismatch {
                argument_index: 0,
                ..
            }
        ),
        "unexpected parent argument error: {error:?}"
    );
    assert_eq!(*typer.store().symbols.info(derived), SymbolInfo::Missing);
    assert_eq!(*typer.store().symbols.info(base), SymbolInfo::Missing);
    assert!(typer.source_typed_index().is_empty());
    assert!(typer.typed_ast().iter().next().is_none());
    assert!(
        typer
            .source_typed_index()
            .get(source, parent_application)
            .is_none()
    );
}

#[test]
fn cached_parent_receiver_qualifier_is_rejected_without_rolling_back_prior_typing() {
    let text =
        "class Base(seed: Int) {}\nclass Derived(val seed: Int) extends Base(this.seed) {}\n";
    let source = SourceId::from_index(1378);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(text).unwrap();
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "cached-parent-receiver.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let (base, _) = class_definition(&parsed, &store, &index, source, "Base");
    let (derived, derived_tree) = class_definition(&parsed, &store, &index, source, "Derived");
    let template = class_template(&parsed, derived_tree);
    let parent_application = template.parents[0];
    let TreeKind::Apply(application) = &parsed.ast.get(parent_application).kind else {
        panic!("parent remains an application")
    };
    let TreeKind::Select(selection) = &parsed.ast.get(application.args[0]).kind else {
        panic!("parent argument selects the child field")
    };
    let receiver_tree = selection.qualifier;
    let child_constructor = index.symbol_at(source, template.constructor).unwrap();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let context = typer.expression_context_for(child_constructor).unwrap();

    let cached_receiver = typer.type_expression(receiver_tree, context).unwrap();
    let typed_count_before_completion = typer.typed_ast().iter().count();
    assert_eq!(
        typer.source_typed_index().get(source, receiver_tree),
        Some(cached_receiver)
    );
    assert!(matches!(
        typer.complete_symbol(derived),
        Err(TyperError::UninitializedParentArgumentReceiver {
            source: error_source,
            tree_index,
            receiver,
        }) if error_source == source
            && tree_index == receiver_tree.index()
            && receiver == derived
    ));

    assert_eq!(*typer.store().symbols.info(derived), SymbolInfo::Missing);
    assert_eq!(*typer.store().symbols.info(base), SymbolInfo::Missing);
    assert_eq!(
        typer.typed_ast().iter().count(),
        typed_count_before_completion
    );
    assert_eq!(
        typer.source_typed_index().get(source, receiver_tree),
        Some(cached_receiver),
        "completion rollback keeps the earlier successful receiver mapping"
    );
    assert!(
        typer
            .source_typed_index()
            .get(source, application.args[0])
            .is_none()
    );
    assert!(
        typer
            .source_typed_index()
            .get(source, parent_application)
            .is_none()
    );
}

#[test]
fn cached_parent_composite_is_rejected_when_it_contains_the_uninitialized_receiver() {
    let text = "class Base(seed: Derived) {}\nclass Derived extends Base(if true then this else this) {}\n";
    let source = SourceId::from_index(1379);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(text).unwrap();
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "cached-parent-composite.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let (base, _) = class_definition(&parsed, &store, &index, source, "Base");
    let (derived, derived_tree) = class_definition(&parsed, &store, &index, source, "Derived");
    let template = class_template(&parsed, derived_tree);
    let parent_application = template.parents[0];
    let TreeKind::Apply(application) = &parsed.ast.get(parent_application).kind else {
        panic!("parent remains an application")
    };
    let argument_tree = application.args[0];
    let TreeKind::If(_) = &parsed.ast.get(argument_tree).kind else {
        panic!("parent argument is a composite conditional")
    };
    let child_constructor = index.symbol_at(source, template.constructor).unwrap();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let context = typer.expression_context_for(child_constructor).unwrap();

    let cached_argument = typer.type_expression(argument_tree, context).unwrap();
    let typed_count_before_completion = typer.typed_ast().iter().count();
    assert!(matches!(
        typer.complete_symbol(derived),
        Err(TyperError::UninitializedParentArgumentReceiver {
            source: error_source,
            receiver,
            ..
        }) if error_source == source && receiver == derived
    ));

    assert_eq!(*typer.store().symbols.info(derived), SymbolInfo::Missing);
    assert_eq!(*typer.store().symbols.info(base), SymbolInfo::Missing);
    assert_eq!(
        typer.typed_ast().iter().count(),
        typed_count_before_completion
    );
    assert_eq!(
        typer.source_typed_index().get(source, argument_tree),
        Some(cached_argument)
    );
    assert!(
        typer
            .source_typed_index()
            .get(source, parent_application)
            .is_none()
    );
}

#[test]
fn unsupported_parent_constructor_shapes_fail_at_the_typer_boundary() {
    for (label, text, expected) in [
        (
            "too-few-arguments",
            "class Base(seed: Int, enabled: Boolean) {}\nclass Derived(start: Int) extends Base(start) {}\n",
            "arity",
        ),
        (
            "too-many-arguments",
            "class Base(seed: Int) {}\nclass Derived(start: Int) extends Base(start, true) {}\n",
            "arity",
        ),
        (
            "curried-parent-constructor",
            "class Base(seed: Int)(enabled: Boolean) {}\nclass Derived(start: Int) extends Base(start)(true) {}\n",
            "shape",
        ),
        (
            "inaccessible-parent-constructor",
            "class Base private (seed: Int) {}\nclass Derived(start: Int) extends Base(start) {}\n",
            "access",
        ),
        (
            "invalid-parent-name",
            "class Derived extends Missing(1) {}\n",
            "missing",
        ),
        (
            "alias-to-non-class-parent",
            "class Alias { type Parent = Int; class Derived extends Parent(1) {} }\n",
            "no-constructor",
        ),
        (
            "this-is-not-an-int-parent-argument",
            "class Base(seed: Int) {}\nclass Derived extends Base(this) {}\n",
            "receiver",
        ),
        (
            "field-prefix-is-not-an-int-parent-argument",
            "class Base(seed: Int) {}\nclass Derived(val flag: Boolean) extends Base(this.flag) {}\n",
            "receiver",
        ),
        (
            "same-type-this-field-is-uninitialized",
            "class Base(seed: Int) {}\nclass Derived(val seed: Int) extends Base(this.seed) {}\n",
            "receiver",
        ),
        (
            "nested-this-field-is-uninitialized",
            "class Base(seed: Int) {}\nclass Derived(seed: Int) extends Base(if true then this.seed else seed) {}\n",
            "receiver",
        ),
        (
            "unqualified-child-field-is-uninitialized",
            "class Base(seed: Int) {}\nclass Derived extends Base(childSeed) { val childSeed: Int = 1 }\n",
            "receiver",
        ),
    ] {
        let source = SourceId::from_index(1400 + label.len() as u32);
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let scanner = ContextualScanner::new(text).unwrap();
        let parsed = dotty_parser::parse_compilation_unit(
            SourceText::new(text).unwrap(),
            source,
            scanner,
            &mut store.names,
        );
        assert!(
            parsed.diagnostics.is_empty(),
            "{label}: {:?}",
            parsed.diagnostics
        );
        let mut packages = Packages::new();
        let index = name_compilation_unit(
            &parsed.ast,
            parsed.root,
            source,
            "unsupported-parent-shape.scala",
            &mut store,
            &mut packages,
        )
        .unwrap();
        let (derived, derived_tree) = class_definition(&parsed, &store, &index, source, "Derived");
        let parent_application = class_template(&parsed, derived_tree).parents[0];
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let error = match typer.complete_symbol(derived) {
            Ok(_) => panic!("{label}: invalid parent constructor unexpectedly typed"),
            Err(error) => error,
        };

        let accepted = match expected {
            "arity" => matches!(error, TyperError::ApplicationArityMismatch { .. }),
            "shape" => matches!(error, TyperError::MalformedClassParent { .. }),
            "access" => matches!(error, TyperError::ParentConstructorInaccessible { .. }),
            "type" => matches!(error, TyperError::ApplicationArgumentTypeMismatch { .. }),
            "missing" => matches!(error, TyperError::TypeNameNotFound { .. }),
            "parent" => matches!(error, TyperError::UnresolvedParentClassKind { .. }),
            "no-constructor" => {
                matches!(error, TyperError::ConstructorApplicationUnavailable { .. })
            }
            "receiver" => matches!(
                error,
                TyperError::UninitializedParentArgumentReceiver { .. }
            ),
            _ => unreachable!(),
        };
        assert!(accepted, "{label}: unexpected Typer error: {error:?}");
        assert_eq!(*typer.store().symbols.info(derived), SymbolInfo::Missing);
        assert!(typer.source_typed_index().is_empty());
        assert!(typer.typed_ast().iter().next().is_none());
        assert!(
            typer
                .source_typed_index()
                .get(source, parent_application)
                .is_none()
        );
    }
}

#[test]
fn types_return_from_an_explicit_result_method_through_the_public_pipeline() {
    let text = "class C { def choose(flag: Boolean, value: Int, fallback: Int): Int = if flag then return value else fallback }";
    let source = SourceId::from_index(0);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(text).unwrap();
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "typed-return.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let (method, rhs) = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == "choose").then(|| {
                (
                    index.symbol_at(source, tree).unwrap(),
                    definition.rhs.unwrap(),
                )
            })
        })
        .expect("source should contain method choose");
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let context = typer.expression_context_for(method).unwrap();

    let typed = typer.type_expression(rhs, context).unwrap();

    assert_eq!(typer.typed_ast().get(typed).ty, definitions.int);
    let TreeKind::If(typed_if) = &typer.typed_ast().get(typed).kind else {
        panic!("source method body should remain a typed if");
    };
    assert_eq!(
        typer.typed_ast().get(typed_if.then_branch).ty,
        definitions.nothing_type
    );
    assert!(matches!(
        typer.typed_ast().get(typed_if.then_branch).kind,
        TreeKind::Return(_)
    ));
    assert_eq!(
        typer
            .widen_expression_type(typer.typed_ast().get(typed_if.else_branch).ty)
            .unwrap(),
        definitions.int
    );
}
