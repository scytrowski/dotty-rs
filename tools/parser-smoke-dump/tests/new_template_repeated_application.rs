use dotty_core::ast::{Block, DefDef, ModuleDef, PackageDef, Template, UntypedNode};
use dotty_core::{NameInterner, SourceId, SourceText, TreeKind};
use dotty_lexer::ContextualScanner;
use dotty_parser::parse_compilation_unit;

#[test]
fn reports_repeated_constructor_application_and_preserves_the_next_tuple() {
    let source = include_str!(
        "../../scala-parser-oracle/fixtures/oracle-only/new-template-repeated-application.scala"
    );
    let scanner = ContextualScanner::new(source).expect("source scans");
    let mut names = NameInterner::new();
    let result = parse_compilation_unit(
        SourceText::new(source).expect("source text is valid"),
        SourceId::from_index(0),
        scanner,
        &mut names,
    );

    assert!(
        result.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .legacy_message()
                .expect("legacy parser diagnostic")
                == "a constructor application cannot be applied again"
        }),
        "expected the repeated constructor application diagnostic, got {:?}",
        result.diagnostics
    );
    let TreeKind::PackageDef(PackageDef { stats, .. }) = &result.ast.get(result.root).kind else {
        panic!("expected a package root");
    };
    let [object] = stats.as_slice() else {
        panic!("expected one top-level object");
    };
    let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(ModuleDef { template, .. })) =
        &result.ast.get(*object).kind
    else {
        panic!("expected an object definition");
    };
    let TreeKind::Template(Template { body, .. }) = &result.ast.get(*template).kind else {
        panic!("expected an object template");
    };
    let [method] = body.as_slice() else {
        panic!("expected one method in the object body");
    };
    let TreeKind::DefDef(DefDef { rhs: Some(rhs), .. }) = &result.ast.get(*method).kind else {
        panic!("expected a method body");
    };
    let TreeKind::Block(Block { stats, expr }) = &result.ast.get(*rhs).kind else {
        panic!("expected the malformed expression and tuple in a block");
    };
    assert_eq!(stats.len(), 1, "the malformed `new` precedes the tuple");
    assert!(matches!(result.ast.get(stats[0]).kind, TreeKind::New(_)));
    let malformed_new_span = result.ast.get(stats[0]).position.unwrap().span().range();
    let tuple = *expr;
    assert!(matches!(
        result.ast.get(tuple).kind,
        TreeKind::PhaseSpecific(UntypedNode::Tuple(_))
    ));
    let tuple_tree = result.ast.get(tuple);
    let tuple_span = tuple_tree
        .position
        .expect("tuple has a source position")
        .span()
        .range();
    assert!(malformed_new_span.start() < tuple_span.start());
    assert_eq!(
        source.get(tuple_span.start() as usize..tuple_span.end() as usize),
        Some("(2, 3)")
    );
}
