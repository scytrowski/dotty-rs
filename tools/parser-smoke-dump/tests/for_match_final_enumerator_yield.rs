use dotty_core::ast::{
    Apply, Block, CaseDef, DefDef, ForYield, GenFrom, Match, ModuleDef, PackageDef, Template,
    UntypedNode,
};
use dotty_core::{NameInterner, SourceId, SourceText, TreeKind};
use dotty_lexer::ContextualScanner;
use dotty_parser::parse_compilation_unit;

#[test]
fn preserves_yield_after_a_multiline_match_in_the_final_enumerator_rhs() {
    let source = include_str!(
        "../../scala-parser-oracle/fixtures/compilation/for-match-final-enumerator-yield.scala"
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
        panic!("expected one method");
    };
    let TreeKind::DefDef(DefDef { rhs: Some(rhs), .. }) = &result.ast.get(*method).kind else {
        panic!("expected a method body");
    };
    let TreeKind::Apply(Apply { args, .. }) = &result.ast.get(*rhs).kind else {
        panic!("expected Loop.forever application");
    };
    let [loop_body] = args.as_slice() else {
        panic!("expected one Loop.forever argument");
    };
    let TreeKind::Block(Block { expr, .. }) = &result.ast.get(*loop_body).kind else {
        panic!("expected the braced body block");
    };
    let TreeKind::PhaseSpecific(UntypedNode::ForYield(ForYield { enums, body })) =
        &result.ast.get(*expr).kind
    else {
        panic!(
            "expected a for-yield expression, got {:?}",
            result.ast.get(*expr).kind
        );
    };
    assert_eq!(enums.len(), 3);
    let enumerator = enums[2];
    let TreeKind::PhaseSpecific(UntypedNode::GenFrom(GenFrom { expr, .. })) =
        &result.ast.get(enumerator).kind
    else {
        panic!("expected a generator");
    };
    let TreeKind::Match(Match { cases, .. }) = &result.ast.get(*expr).kind else {
        panic!("expected the generator RHS to be a match");
    };
    assert_eq!(cases.len(), 2);
    assert!(matches!(result.ast.get(*body).kind, TreeKind::Select(_)));
}

#[test]
fn keeps_a_following_enumerator_outside_the_feedback_opened_match_body() {
    let source = include_str!(
        "../../scala-parser-oracle/fixtures/compilation/for-match-followed-by-enumerator.scala"
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

    let (for_tree, for_node) = result
        .ast
        .iter()
        .find(|(_, tree)| matches!(tree.kind, TreeKind::PhaseSpecific(UntypedNode::ForYield(_))))
        .expect("the for-yield expression is present");
    let TreeKind::PhaseSpecific(UntypedNode::ForYield(ForYield { enums, .. })) = &for_node.kind
    else {
        unreachable!("the selected tree is a for-yield expression");
    };
    assert_eq!(
        enums.len(),
        2,
        "the next generator must remain an enumerator"
    );

    let TreeKind::PhaseSpecific(UntypedNode::GenFrom(GenFrom { expr, .. })) =
        &result.ast.get(enums[0]).kind
    else {
        panic!("the first enumerator is a generator");
    };
    let TreeKind::Match(Match { cases, .. }) = &result.ast.get(*expr).kind else {
        panic!("the first generator RHS is a match");
    };
    let [_, second_case] = cases.as_slice() else {
        panic!("expected both match cases");
    };
    let TreeKind::CaseDef(CaseDef { body, .. }) = &result.ast.get(*second_case).kind else {
        panic!("expected the second case body");
    };
    let body_span = result.ast.get(*body).position.unwrap().span().range();
    let case_body_source = &source[body_span.start() as usize..body_span.end() as usize];
    assert!(
        case_body_source.ends_with("Nil"),
        "case body should include its final expression: {case_body_source:?}"
    );

    let second_enumerator_span = result.ast.get(enums[1]).position.unwrap().span().range();
    assert!(body_span.end() < second_enumerator_span.start());
    assert!(matches!(
        result.ast.get(for_tree).kind,
        TreeKind::PhaseSpecific(UntypedNode::ForYield(_))
    ));
}
