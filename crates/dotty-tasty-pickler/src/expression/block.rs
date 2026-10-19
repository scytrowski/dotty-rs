use super::*;
use dotty_core::ast::Block;
use dotty_tasty::tasty::{AstField, BLOCK_TAG};

pub(super) fn lower_block_inner(
    store: &SemanticStore,
    ast: &TypedAst,
    block: &Block<Typed>,
    method: SymbolId,
    parameters: &[SymbolId],
    state: ExpressionState<'_>,
    context: &mut EncodingContext<'_>,
) -> Result<AstLabel, PickleError> {
    let ExpressionState { depth, budget } = state;
    let expression = lower_expression_inner(
        store,
        ast,
        block.expr,
        method,
        parameters,
        ExpressionState { depth, budget },
        context,
    )?;
    let mut fields = vec![AstField::Tree(expression)];
    for &stat in &block.stats {
        let stat_depth = if matches!(
            ast.try_get(stat).map(|tree| &tree.kind),
            Some(TreeKind::Block(_))
        ) {
            depth + 1
        } else {
            depth
        };
        fields.push(AstField::Tree(lower_expression_inner(
            store,
            ast,
            stat,
            method,
            parameters,
            ExpressionState {
                depth: stat_depth,
                budget,
            },
            context,
        )?));
    }
    context.wire_node(AstNode::Length {
        tag: BLOCK_TAG,
        fields,
    })
}
