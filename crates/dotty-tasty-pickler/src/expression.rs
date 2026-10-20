//! The frozen literal slice has no recursive expression edges.

use std::collections::HashSet;

use dotty_core::ast::{ApplyKind, TypedAst};
use dotty_core::{
    Constant, SemanticStore, SymbolId, SymbolInfo, TermRefTarget, TreeId, TreeKind, Type, Typed,
};
use dotty_tasty::tasty::{
    APPLY_TAG, AstField, AstLabel, AstNode, FALSECONST_TAG, IF_TAG, INTCONST_TAG, SimpleTerm,
    TRUECONST_TAG, TermValue, UNITCONST_TAG,
};

use crate::{
    EncodingContext, LimitKind, PickleError, ProjectionLimits, UnsupportedFeature,
    bounded_expression_depth,
};

mod block;

struct ExpressionBudget {
    max_depth: usize,
    item_limit: usize,
    remaining: usize,
    visited: HashSet<TreeId<Typed>>,
}

struct ExpressionState<'a> {
    depth: usize,
    budget: &'a mut ExpressionBudget,
}

impl ExpressionBudget {
    fn check_depth(&self, depth: usize) -> Result<(), PickleError> {
        if depth > self.max_depth {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::ExpressionDepth,
                limit: self.max_depth,
            });
        }
        Ok(())
    }

    fn visit(&mut self, tree: TreeId<Typed>, depth: usize) -> Result<(), PickleError> {
        self.check_depth(depth)?;
        self.remaining = self
            .remaining
            .checked_sub(1)
            .ok_or(PickleError::LimitExceeded {
                kind: LimitKind::Items,
                limit: self.item_limit,
            })?;
        if !self.visited.insert(tree) {
            return Err(PickleError::RepeatedExpression { tree });
        }
        Ok(())
    }
}

pub(crate) fn lower_expression(
    store: &SemanticStore,
    ast: &TypedAst,
    root: TreeId<Typed>,
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
    lower_expression_inner(
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
}

fn lower_expression_inner(
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
    if let TreeKind::If(if_expr) = tree.kind {
        let condition = lower_expression_inner(
            store,
            ast,
            if_expr.cond,
            method,
            parameters,
            ExpressionState { depth, budget },
            context,
        )?;
        let then_depth = if matches!(
            ast.try_get(if_expr.then_branch).map(|tree| &tree.kind),
            Some(TreeKind::If(_) | TreeKind::Block(_))
        ) {
            depth + 1
        } else {
            depth
        };
        let then_branch = lower_expression_inner(
            store,
            ast,
            if_expr.then_branch,
            method,
            parameters,
            ExpressionState {
                depth: then_depth,
                budget,
            },
            context,
        )?;
        let else_depth = if matches!(
            ast.try_get(if_expr.else_branch).map(|tree| &tree.kind),
            Some(TreeKind::If(_) | TreeKind::Block(_))
        ) {
            depth + 1
        } else {
            depth
        };
        let else_branch = lower_expression_inner(
            store,
            ast,
            if_expr.else_branch,
            method,
            parameters,
            ExpressionState {
                depth: else_depth,
                budget,
            },
            context,
        )?;
        return context.wire_node(AstNode::Length {
            tag: IF_TAG,
            fields: vec![
                AstField::Tree(condition),
                AstField::Tree(then_branch),
                AstField::Tree(else_branch),
            ],
        });
    }
    if let TreeKind::Block(block) = &tree.kind {
        return block::lower_block_inner(
            store,
            ast,
            block,
            method,
            parameters,
            ExpressionState { depth, budget },
            context,
        );
    }
    if let TreeKind::Apply(application) = &tree.kind {
        if application.kind != ApplyKind::Regular || application.args.is_empty() {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::BodyExpression,
                symbol: Some(method),
            });
        }
        let function = ast
            .try_get(application.function)
            .ok_or(PickleError::InvalidTypedTree {
                tree: application.function,
            })?;
        budget.visit(application.function, depth)?;
        if let TreeKind::Select(selection) = &function.kind
            && let Some(Type::TermRef {
                target: TermRefTarget::Symbol(target),
                ..
            }) = store.types.try_get(function.ty)
            && (context.is_registered_int_arithmetic_target(*target)?
                || context.is_registered_int_comparison_target(*target)?
                || context.is_registered_boolean_short_circuit_target(*target)?)
        {
            if application.args.len() != 1 || application.kind != ApplyKind::Regular {
                return Err(PickleError::PrimitiveOperatorArity {
                    symbol: *target,
                    expected: Some(1),
                    actual: Some(application.args.len()),
                });
            }
            let receiver =
                ast.try_get(selection.qualifier)
                    .ok_or(PickleError::InvalidTypedTree {
                        tree: selection.qualifier,
                    })?;
            let receiver_depth = if matches!(
                receiver.kind,
                TreeKind::Apply(_) | TreeKind::Block(_) | TreeKind::If(_) | TreeKind::Select(_)
            ) {
                depth + 1
            } else {
                depth
            };
            let receiver_label = lower_expression_inner(
                store,
                ast,
                selection.qualifier,
                method,
                parameters,
                ExpressionState {
                    depth: receiver_depth,
                    budget,
                },
                context,
            )?;
            let argument = application.args[0];
            let argument_depth = if matches!(
                ast.try_get(argument).map(|argument| &argument.kind),
                Some(
                    TreeKind::Apply(_) | TreeKind::Block(_) | TreeKind::If(_) | TreeKind::Select(_)
                )
            ) {
                depth + 1
            } else {
                depth
            };
            let argument_label = lower_expression_inner(
                store,
                ast,
                argument,
                method,
                parameters,
                ExpressionState {
                    depth: argument_depth,
                    budget,
                },
                context,
            )?;
            let SymbolInfo::Complete(signature_type) = store.symbols.get(*target).info else {
                return Err(PickleError::IncompleteSymbol {
                    symbol: *target,
                    info: store.symbols.get(*target).info,
                });
            };
            let Type::Method(signature) = store
                .types
                .try_get(signature_type)
                .ok_or(PickleError::IncompleteType { ty: signature_type })?
            else {
                return Err(PickleError::MalformedType {
                    ty: signature_type,
                    role: crate::TypeRole::Parameter,
                });
            };
            let parameter_types = signature
                .params
                .iter()
                .map(|parameter| parameter.ty)
                .collect::<Vec<_>>();
            let result_type = signature.result;
            let receiver_type = receiver.ty;
            let selected = context.primitive_operator_select(
                *target,
                receiver_label,
                receiver_type,
                &parameter_types,
                result_type,
                Some(1),
            )?;
            return context.wire_node(AstNode::Length {
                tag: APPLY_TAG,
                fields: vec![AstField::Tree(selected), AstField::Tree(argument_label)],
            });
        }
        let Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(target),
        } = store
            .types
            .try_get(function.ty)
            .ok_or(PickleError::BodyType {
                tree: application.function,
                ty: function.ty,
            })?
        else {
            return Err(PickleError::BodyType {
                tree: application.function,
                ty: function.ty,
            });
        };
        let explicit_this = matches!(function.kind, TreeKind::Select(_));
        if let TreeKind::Select(selection) = &function.kind {
            budget.visit(selection.qualifier, depth)?;
        }
        let class = store
            .symbols
            .get(method)
            .owner
            .ok_or(PickleError::InvalidSymbol { symbol: method })?;
        if !matches!(store.types.try_get(*prefix), Some(Type::ThisType { class: prefix_class }) if *prefix_class == class)
        {
            return Err(PickleError::Reference {
                symbol: *target,
                reason: crate::ReferenceError::Owner,
            });
        }
        let function_label = context.same_class_method_select_in(
            class,
            *target,
            explicit_this,
            application.args.len(),
        )?;
        let mut fields = vec![AstField::Tree(function_label)];
        for &argument in &application.args {
            let argument_depth = if matches!(
                ast.try_get(argument).map(|argument| &argument.kind),
                Some(TreeKind::Apply(_) | TreeKind::Block(_) | TreeKind::Select(_))
            ) {
                depth + 1
            } else {
                depth
            };
            fields.push(AstField::Tree(lower_expression_inner(
                store,
                ast,
                argument,
                method,
                parameters,
                ExpressionState {
                    depth: argument_depth,
                    budget,
                },
                context,
            )?));
        }
        return context.wire_node(AstNode::Length {
            tag: APPLY_TAG,
            fields,
        });
    }
    if let TreeKind::Ident(_) = &tree.kind {
        let Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(target),
        } = store.types.try_get(tree.ty).ok_or(PickleError::BodyType {
            tree: root,
            ty: tree.ty,
        })?
        else {
            return Err(PickleError::BodyType {
                tree: root,
                ty: tree.ty,
            });
        };
        if parameters.contains(target) {
            if !matches!(store.types.try_get(*prefix), Some(Type::NoPrefix)) {
                return Err(PickleError::Reference {
                    symbol: *target,
                    reason: crate::ReferenceError::Owner,
                });
            }
            return context.local_reference(*target, None);
        }
        if store.symbols.contains(*target)
            && store.symbols.get(*target).kind == dotty_core::SymbolKind::Local
            && store.symbols.get(*target).owner == Some(method)
        {
            if !matches!(store.types.try_get(*prefix), Some(Type::NoPrefix)) {
                return Err(PickleError::Reference {
                    symbol: *target,
                    reason: crate::ReferenceError::Owner,
                });
            }
            return context.local_reference(*target, None);
        }
        let class = store
            .symbols
            .contains(method)
            .then(|| store.symbols.get(method).owner)
            .flatten()
            .ok_or(PickleError::InvalidSymbol { symbol: method })?;
        if store.symbols.contains(*target)
            && store.symbols.get(*target).kind == dotty_core::SymbolKind::Method
            && store.symbols.get(*target).owner == Some(class)
            && matches!(store.types.try_get(*prefix), Some(Type::ThisType { class: prefix_class }) if *prefix_class == class)
        {
            return context.same_class_method_select(class, *target);
        }
        if !parameters.contains(target) {
            return Err(PickleError::Reference {
                symbol: *target,
                reason: crate::ReferenceError::NotRegistered,
            });
        }
        return context.local_reference(*target, None);
    }
    if let TreeKind::Select(selection) = &tree.kind {
        if let Some(Type::TermRef {
            target: TermRefTarget::Symbol(target),
            ..
        }) = store.types.try_get(tree.ty)
            && context.is_registered_boolean_unary_not_target(*target)?
        {
            let receiver =
                ast.try_get(selection.qualifier)
                    .ok_or(PickleError::InvalidTypedTree {
                        tree: selection.qualifier,
                    })?;
            let receiver_depth = if matches!(
                receiver.kind,
                TreeKind::Apply(_) | TreeKind::Block(_) | TreeKind::If(_) | TreeKind::Select(_)
            ) {
                depth + 1
            } else {
                depth
            };
            let receiver_label = lower_expression_inner(
                store,
                ast,
                selection.qualifier,
                method,
                parameters,
                ExpressionState {
                    depth: receiver_depth,
                    budget,
                },
                context,
            )?;
            let SymbolInfo::Complete(result_type) = store.symbols.get(*target).info else {
                return Err(PickleError::IncompleteSymbol {
                    symbol: *target,
                    info: store.symbols.get(*target).info,
                });
            };
            return context.primitive_operator_select(
                *target,
                receiver_label,
                receiver.ty,
                &[],
                result_type,
                None,
            );
        }
        let qualifier = ast
            .try_get(selection.qualifier)
            .ok_or(PickleError::InvalidTypedTree {
                tree: selection.qualifier,
            })?;
        if !matches!(qualifier.kind, TreeKind::This(this) if this.qual.is_none()) {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::BodyExpression,
                symbol: Some(method),
            });
        }
        let Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(target),
        } = store.types.try_get(tree.ty).ok_or(PickleError::BodyType {
            tree: root,
            ty: tree.ty,
        })?
        else {
            return Err(PickleError::BodyType {
                tree: root,
                ty: tree.ty,
            });
        };
        let class = store
            .symbols
            .contains(method)
            .then(|| store.symbols.get(method).owner)
            .flatten()
            .ok_or(PickleError::InvalidSymbol { symbol: method })?;
        if !store.symbols.contains(*target)
            || store.symbols.get(*target).kind != dotty_core::SymbolKind::Method
            || store.symbols.get(*target).owner != Some(class)
            || !matches!(store.types.try_get(*prefix), Some(Type::ThisType { class: prefix_class }) if *prefix_class == class)
            || !matches!(store.types.try_get(qualifier.ty), Some(Type::ThisType { class: qualifier_class }) if *qualifier_class == class)
        {
            return Err(PickleError::Reference {
                symbol: *target,
                reason: crate::ReferenceError::Owner,
            });
        }
        return context.same_class_method_select_on_this(class, *target);
    }
    let TreeKind::Literal(literal) = &tree.kind else {
        return Err(PickleError::Unsupported {
            feature: UnsupportedFeature::BodyExpression,
            symbol: Some(method),
        });
    };
    // Primitive constants retain their singleton semantic type but serialize
    // directly as their Scala 3.9 constant leaves.
    let tag_and_value = match (&literal.value, store.types.try_get(tree.ty)) {
        (Constant::Int(value), Some(Type::Constant(Constant::Int(type_value))))
            if value == type_value =>
        {
            (INTCONST_TAG, TermValue::Int(*value))
        }
        (Constant::Boolean(value), Some(Type::Constant(Constant::Boolean(type_value))))
            if value == type_value =>
        {
            (
                if *value {
                    TRUECONST_TAG
                } else {
                    FALSECONST_TAG
                },
                TermValue::Boolean(*value),
            )
        }
        (Constant::Unit, Some(Type::Constant(Constant::Unit))) => (UNITCONST_TAG, TermValue::Unit),
        _ => {
            return Err(PickleError::BodyType {
                tree: root,
                ty: tree.ty,
            });
        }
    };
    let term = SimpleTerm::new(tag_and_value.0, tag_and_value.1).map_err(|cause| {
        PickleError::Encoding {
            cause: Box::new(cause.into()),
        }
    })?;
    context.wire_node(AstNode::Leaf(term))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MAX_EXPRESSION_DEPTH;

    #[test]
    fn configured_expression_depth_cannot_raise_recursive_stack_ceiling() {
        let budget = ExpressionBudget {
            max_depth: bounded_expression_depth(usize::MAX),
            item_limit: 1_000,
            remaining: 1_000,
            visited: HashSet::new(),
        };
        assert_eq!(
            budget.check_depth(MAX_EXPRESSION_DEPTH + 1),
            Err(PickleError::LimitExceeded {
                kind: LimitKind::ExpressionDepth,
                limit: MAX_EXPRESSION_DEPTH,
            })
        );
    }
}
