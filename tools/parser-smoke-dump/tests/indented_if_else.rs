use dotty_core::ast::{Block, DefDef, If, ModuleDef, Template, UntypedNode};
use dotty_core::{NameInterner, SourceId, SourceText, TreeKind};
use dotty_lexer::ContextualScanner;
use dotty_parser::parse_compilation_unit;

#[test]
fn else_aligned_with_an_indented_then_expression_remains_in_the_if() {
    let source = include_str!(
        "../../scala-parser-oracle/fixtures/compilation/if-else-aligned-with-then-body.scala"
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
        result.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        result.diagnostics
    );
    let TreeKind::PackageDef(package) = &result.ast.get(result.root).kind else {
        panic!("expected package root");
    };
    let [object] = package.stats.as_slice() else {
        panic!("expected one object definition");
    };
    let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(ModuleDef { template, .. })) =
        &result.ast.get(*object).kind
    else {
        panic!("expected object definition");
    };
    let TreeKind::Template(Template { body, .. }) = &result.ast.get(*template).kind else {
        panic!("expected object template");
    };
    assert_eq!(
        body.len(),
        2,
        "following member must remain in the template"
    );
    let TreeKind::DefDef(DefDef { rhs: Some(rhs), .. }) = result.ast.get(body[0]).kind else {
        panic!("expected method body");
    };
    let TreeKind::Block(Block { expr, .. }) = result.ast.get(rhs).kind else {
        panic!("expected the method body to be a block");
    };
    let TreeKind::If(If {
        then_branch,
        else_branch,
        ..
    }) = result.ast.get(expr).kind
    else {
        panic!("expected an if expression");
    };
    assert!(
        matches!(
            result.ast.get(then_branch).kind,
            TreeKind::PhaseSpecific(UntypedNode::Number(_))
        ),
        "unexpected then branch: {:?}",
        result.ast.get(then_branch).kind
    );
    assert!(matches!(
        result.ast.get(else_branch).kind,
        TreeKind::PhaseSpecific(UntypedNode::Number(_))
    ));
    assert!(matches!(result.ast.get(body[1]).kind, TreeKind::ValDef(_)));
}
