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
    let type_syntax = if tree.ty == context.definitions().int
        || tree.ty == context.definitions().boolean
        || tree.ty == context.definitions().unit
    {
        context.primitive_type_syntax(tree.ty)?
    } else {
        context.same_source_reference_type_syntax(tree.ty)?
    };
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

pub(crate) struct MutableCounterFragmentTrees {
    pub prefix: [TreeId<Typed>; 2],
    pub initializer: TreeId<Typed>,
    pub selected: TreeId<Typed>,
}

/// Emits the original declaration prefix, selected mutable VALDEF, and its
/// borrowed initializer reference in a distinct incomplete block. Source and
/// typed roots remain borrowed from the original block; none are rewritten or
/// truncated.
pub(crate) fn lower_mutable_counter_declaration_fragment(
    store: &SemanticStore,
    ast: &TypedAst,
    roots: MutableCounterFragmentTrees,
    method: SymbolId,
    parameters: &[SymbolId],
    limits: ProjectionLimits,
    context: &mut EncodingContext<'_>,
) -> Result<AstLabel, PickleError> {
    let mut budget = ExpressionBudget {
        max_depth: bounded_expression_depth(limits.max_expression_depth),
        item_limit: limits.max_items,
        remaining: limits.max_items,
        visited: HashSet::new(),
    };
    let prefix = roots
        .prefix
        .into_iter()
        .map(|root| {
            lower_local_value_inner(
                store,
                ast,
                root,
                method,
                parameters,
                ExpressionState {
                    depth: 0,
                    budget: &mut budget,
                },
                context,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let selected = lower_local_value_inner(
        store,
        ast,
        roots.selected,
        method,
        parameters,
        ExpressionState {
            depth: 0,
            budget: &mut budget,
        },
        context,
    )?;
    let mut expression_budget = ExpressionBudget {
        max_depth: budget.max_depth,
        item_limit: budget.item_limit,
        remaining: budget.remaining,
        visited: HashSet::new(),
    };
    let expression = lower_expression_inner(
        store,
        ast,
        roots.initializer,
        method,
        parameters,
        ExpressionState {
            depth: 0,
            budget: &mut expression_budget,
        },
        context,
    )?;
    budget.remaining = expression_budget.remaining;
    context.wire_node(AstNode::Length {
        tag: BLOCK_TAG,
        fields: std::iter::once(AstField::Tree(expression))
            .chain(prefix.into_iter().map(AstField::Tree))
            .chain(std::iter::once(AstField::Tree(selected)))
            .collect(),
    })
}
