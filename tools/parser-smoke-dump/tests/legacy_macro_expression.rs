use dotty_core::ast::{DefDef, MacroTree, ModuleDef, Template, UntypedNode};
use dotty_core::{NameInterner, SourceId, SourceText, TreeKind};
use dotty_lexer::ContextualScanner;
use dotty_parser::parse_compilation_unit;

#[test]
fn legacy_macro_rhs_is_preserved_as_a_source_tree() {
    let source = include_str!(
        "../../scala-parser-oracle/fixtures/compilation/legacy-macro-expression.scala"
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
        panic!("expected one object");
    };
    let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(ModuleDef { template, .. })) =
        &result.ast.get(*object).kind
    else {
        panic!("expected object definition");
    };
    let TreeKind::Template(Template { body, .. }) = &result.ast.get(*template).kind else {
        panic!("expected object template");
    };
    let [method, following] = body.as_slice() else {
        panic!("expected the following method to survive macro parsing");
    };
    let TreeKind::DefDef(DefDef { rhs: Some(rhs), .. }) = result.ast.get(*method).kind else {
        panic!("expected method with a right-hand side");
    };
    let TreeKind::PhaseSpecific(UntypedNode::MacroTree(MacroTree { expr })) =
        result.ast.get(rhs).kind
    else {
        panic!("expected source-level MacroTree");
    };
    assert!(matches!(result.ast.get(expr).kind, TreeKind::Ident(_)));
    assert!(matches!(
        result.ast.get(*following).kind,
        TreeKind::DefDef(_)
    ));
}
