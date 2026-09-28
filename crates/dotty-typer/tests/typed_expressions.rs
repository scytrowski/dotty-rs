//! End-to-end typed-expression checks over synthetic source.

use dotty_core::ast::TreeKind;
use dotty_core::types::{TermRefTarget, Type};
use dotty_core::{Definitions, Packages, SemanticStore, SourceId, SourceText};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;
use dotty_typer::{ExpressionContext, SourceTyper};

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
