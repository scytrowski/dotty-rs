use dotty_core::ast::{Block, UntypedNode};
use dotty_core::{NameInterner, SourceId, SourceText, TreeKind};
use dotty_lexer::ContextualScanner;
use dotty_parser::parse_compilation_unit;

#[test]
fn treats_tuple_after_anonymous_new_body_as_the_next_block_expression() {
    let source = include_str!(
        "../../scala-parser-oracle/fixtures/compilation/new-template-followed-by-tuple.scala"
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
    let (tuple, tuple_tree) = result
        .ast
        .iter()
        .find(|(_, tree)| matches!(tree.kind, TreeKind::PhaseSpecific(UntypedNode::Tuple(_))))
        .expect("the following expression remains a tuple");
    let tuple_span = tuple_tree
        .position
        .expect("tuple has a source position")
        .span()
        .range();
    assert_eq!(
        source.get(tuple_span.start() as usize..tuple_span.end() as usize),
        Some("(service, recorded)")
    );
    let (_, block) = result
        .ast
        .iter()
        .find(
            |(_, tree)| matches!(&tree.kind, TreeKind::Block(Block { expr, .. }) if *expr == tuple),
        )
        .expect("the tuple is the final expression in its enclosing block");
    let TreeKind::Block(Block { stats, .. }) = &block.kind else {
        unreachable!("the selected tree is a block");
    };
    assert_eq!(stats.len(), 1);
    assert!(matches!(result.ast.get(stats[0]).kind, TreeKind::ValDef(_)));
}
