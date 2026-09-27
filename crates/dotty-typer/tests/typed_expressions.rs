//! End-to-end typed-expression checks over synthetic source.

use dotty_core::ast::TreeKind;
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

    assert!(matches!(
        typer.typed_ast().get(typed).kind,
        TreeKind::Select(_)
    ));
    assert_eq!(typer.typed_ast().get(typed).ty, definitions.int);
    assert_eq!(typer.typed_ast().get(typed).position, source_position);
    assert_eq!(typer.source_typed_index().get(source, rhs), Some(typed));
    for (_, node) in typer.typed_ast().iter() {
        assert!(typer.store().types.contains(node.ty));
    }
}
