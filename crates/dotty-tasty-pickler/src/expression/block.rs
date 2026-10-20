use super::*;
use dotty_core::ast::Block;
use dotty_tasty::tasty::{
    AstField, AstNode, BLOCK_TAG, MUTABLE_TAG, SimpleTerm, TermValue, VALDEF_TAG,
};

fn lower_local_value_inner(
    store: &SemanticStore,
    ast: &TypedAst,
    root: TreeId<Typed>,
    method: SymbolId,
    parameters: &[SymbolId],
    state: ExpressionState<'_>,
    context: &mut EncodingContext<'_>,
) -> Result<AstLabel, PickleError> {
    let ExpressionState { depth, budget } = state;
    budget.visit(root, depth)?;
    let tree = ast
        .try_get(root)
        .ok_or(PickleError::InvalidTypedTree { tree: root })?;
    let TreeKind::ValDef(value) = &tree.kind else {
        return Err(PickleError::Unsupported {
            feature: UnsupportedFeature::BodyExpression,
            symbol: Some(method),
        });
    };
    let symbol = context
        .local_value_symbol(root)
        .ok_or(PickleError::Reference {
            symbol: method,
            reason: crate::ReferenceError::NotRegistered,
        })?;
    if !store.symbols.contains(symbol)
        || store.symbols.get(symbol).kind != dotty_core::SymbolKind::Local
        || store.symbols.get(symbol).owner != Some(method)
    {
        return Err(PickleError::Reference {
            symbol,
            reason: crate::ReferenceError::Owner,
        });
    }
    let rhs = value.rhs.ok_or(PickleError::MissingBody { method })?;
    let rhs_depth = child_expression_depth(ast, rhs, depth);
    let rhs = lower_expression_inner(
        store,
        ast,
        rhs,
        method,
        parameters,
        ExpressionState {
            depth: rhs_depth,
            budget,
        },
        context,
    )?;
    let name = context.intern_name(*value.name.as_name())?;
    let type_syntax = context.primitive_type_syntax(tree.ty)?;
    let mut fields = vec![
        AstField::Nat(name),
        AstField::Tree(type_syntax),
        AstField::Tree(rhs),
    ];
    if store.symbols.get(symbol).flags == dotty_core::SymbolFlags::MUTABLE {
        let mutable = SimpleTerm::new(MUTABLE_TAG, TermValue::Tag).map_err(|cause| {
            PickleError::Encoding {
                cause: Box::new(cause.into()),
            }
        })?;
        fields.push(AstField::Tree(context.wire_node(AstNode::Leaf(mutable))?));
    }
    context.bind_declaration(
        symbol,
        AstNode::Length {
            tag: VALDEF_TAG,
            fields,
        },
    )?;
    context
        .declaration_label(symbol)
        .ok_or(PickleError::Reference {
            symbol,
            reason: crate::ReferenceError::NotRegistered,
        })
}

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
        let stat_tree = ast
            .try_get(stat)
            .ok_or(PickleError::InvalidTypedTree { tree: stat })?;
        if matches!(stat_tree.kind, TreeKind::ValDef(_)) {
            fields.push(AstField::Tree(lower_local_value_inner(
                store,
                ast,
                stat,
                method,
                parameters,
                ExpressionState { depth, budget },
                context,
            )?));
            continue;
        }
        let stat_depth = if matches!(&stat_tree.kind, TreeKind::Block(_)) {
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
