//! Conditional, loop, and return expression typing.

use super::super::{ExpressionContext, SourceTyper, TyperError};
use crate::typer::MAX_TYPE_RELATION_DEPTH;
use crate::typer::TypeRelationError;
use dotty_core::ast::*;
use dotty_core::types::*;
use dotty_core::*;
use std::collections::HashSet;

impl SourceTyper<'_> {
    pub(in crate::typer) fn join_expression_types(
        &mut self,
        left: TypeId,
        right: TypeId,
    ) -> Result<TypeId, TypeRelationError> {
        if left == right {
            return Ok(left);
        }
        let left_is_subtype = self.is_subtype(left, right);
        let right_is_subtype = self.is_subtype(right, left);
        match (left_is_subtype, right_is_subtype) {
            (Ok(true), Ok(true)) => Ok(left),
            (Ok(true), _) => Ok(right),
            (_, Ok(true)) => Ok(left),
            (Ok(false), Ok(false)) => Ok(self.store.types.alloc(Type::Or { left, right })),
            (Err(error), _) | (_, Err(error)) => Err(error),
        }
    }

    pub(in crate::typer) fn require_if_child_tree(
        &self,
        if_tree: TreeId<Untyped>,
        child: TreeId<Untyped>,
        role: &'static str,
    ) -> Result<(), TyperError> {
        if self.arena.try_get(child).is_none() {
            return Err(TyperError::IfChildTreeOutsideArena {
                source: self.source,
                tree_index: if_tree.index(),
                child_tree_index: child.index(),
                role,
            });
        }
        Ok(())
    }

    pub(in crate::typer) fn require_while_child_tree(
        &self,
        while_tree: TreeId<Untyped>,
        child: TreeId<Untyped>,
        role: &'static str,
    ) -> Result<(), TyperError> {
        if self.arena.try_get(child).is_none() {
            return Err(TyperError::WhileChildTreeOutsideArena {
                source: self.source,
                tree_index: while_tree.index(),
                child_tree_index: child.index(),
                role,
            });
        }
        Ok(())
    }

    pub(in crate::typer) fn enclosing_method_for_return(
        &self,
        owner: SymbolId,
        tree_index: u32,
    ) -> Result<SymbolId, TyperError> {
        let mut current = Some(owner);
        let mut visited = HashSet::new();
        while let Some(symbol) = current {
            if !self.store.symbols.contains(symbol) || !visited.insert(symbol) {
                return Err(TyperError::MalformedReturnOwnerChain {
                    source: self.source,
                    tree_index,
                    owner: symbol,
                });
            }
            let semantic = self.store.symbols.get(symbol);
            if semantic.kind == SymbolKind::Method {
                return Ok(symbol);
            }
            if semantic.kind == SymbolKind::Constructor {
                return Err(TyperError::ReturnOutsideSupportedMethod {
                    source: self.source,
                    tree_index,
                    owner: symbol,
                });
            }
            current = semantic.owner;
        }
        Err(TyperError::ReturnOutsideSupportedMethod {
            source: self.source,
            tree_index,
            owner,
        })
    }

    pub(in crate::typer) fn explicit_return_result_type(
        &mut self,
        method: SymbolId,
        tree_index: u32,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let Some(definition) = self.index.definition_of(method) else {
            return Err(TyperError::ReturnMethodProvenanceMalformed {
                source: self.source,
                tree_index,
                method,
            });
        };
        let (source, method_tree) = match definition {
            SourceDefinition::Canonical { source, tree }
            | SourceDefinition::Derived { source, tree } => (source, tree),
        };
        if source != self.source {
            return Err(TyperError::ReturnMethodProvenanceMalformed {
                source: self.source,
                tree_index,
                method,
            });
        }
        let Some(method_node) = self.arena.try_get(method_tree) else {
            return Err(TyperError::ReturnMethodProvenanceMalformed {
                source: self.source,
                tree_index,
                method,
            });
        };
        let TreeKind::DefDef(definition) = &method_node.kind else {
            return Err(TyperError::ReturnMethodProvenanceMalformed {
                source: self.source,
                tree_index,
                method,
            });
        };
        let Some(result_tree) = self.arena.try_get(definition.tpt) else {
            return Err(TyperError::ReturnMethodProvenanceMalformed {
                source: self.source,
                tree_index,
                method,
            });
        };
        if matches!(result_tree.kind, TreeKind::TypeTree(_)) {
            return Err(TyperError::ReturnInInferredResultMethodDeferred {
                source: self.source,
                tree_index,
                method,
            });
        }

        let signature = match *self.store.symbols.info(method) {
            SymbolInfo::Complete(signature) => signature,
            SymbolInfo::Missing => self.complete_symbol_inner(method, info_journal)?,
            SymbolInfo::Deferred(_) => {
                return Err(TyperError::DeferredSymbolCompletion { symbol: method });
            }
            SymbolInfo::Error => return Err(TyperError::SymbolAlreadyErrored { symbol: method }),
        };
        let mut result = signature;
        let mut visited = HashSet::new();
        for _ in 0..MAX_TYPE_RELATION_DEPTH {
            if !visited.insert(result) {
                return Err(TyperError::MalformedReturnMethodSignature {
                    source: self.source,
                    tree_index,
                    method,
                    signature,
                });
            }
            match self.store.types.try_get(result) {
                Some(Type::Method(method_type)) => result = method_type.result,
                Some(Type::Poly(poly_type)) => result = poly_type.result,
                Some(_) => return Ok(result),
                None => {
                    return Err(TyperError::MalformedReturnMethodSignature {
                        source: self.source,
                        tree_index,
                        method,
                        signature,
                    });
                }
            }
        }
        Err(TyperError::MalformedReturnMethodSignature {
            source: self.source,
            tree_index,
            method,
            signature,
        })
    }

    pub(in crate::typer) fn check_return_expression_type(
        &mut self,
        tree_index: u32,
        actual: TypeId,
        expected: TypeId,
    ) -> Result<(), TyperError> {
        match self.conforms(actual, expected) {
            Ok(true) => Ok(()),
            Ok(false) => Err(TyperError::ReturnExpressionTypeMismatch {
                source: self.source,
                tree_index,
                actual,
                expected,
            }),
            Err(error) => Err(TyperError::ReturnExpressionConformanceUnsupported {
                source: self.source,
                tree_index,
                actual,
                expected,
                error: Box::new(error),
            }),
        }
    }
    pub(in crate::typer) fn type_if_expression(
        &mut self,
        tree: TreeId<Untyped>,
        if_expr: dotty_core::ast::If<Untyped>,
        position: Option<SourceSpan>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        self.require_if_child_tree(tree, if_expr.cond, "condition")?;
        self.require_if_child_tree(tree, if_expr.then_branch, "then branch")?;
        self.require_if_child_tree(tree, if_expr.else_branch, "else branch")?;
        let cond = self
            .type_expression_expected_inner(
                if_expr.cond,
                context,
                self.definitions.boolean,
                info_journal,
                new_mappings,
            )
            .map_err(|error| match error {
                TyperError::ExpectedExpressionTypeMismatch { actual, .. } => {
                    TyperError::IfConditionTypeMismatch {
                        source: self.source,
                        tree_index: tree.index(),
                        actual,
                        expected: self.definitions.boolean,
                    }
                }
                TyperError::ExpectedExpressionConformanceUnsupported { actual, error, .. } => {
                    TyperError::IfConditionConformanceUnsupported {
                        source: self.source,
                        tree_index: tree.index(),
                        actual,
                        expected: self.definitions.boolean,
                        error,
                    }
                }
                other => other,
            })?;
        let then_branch =
            self.type_expression_inner(if_expr.then_branch, context, info_journal, new_mappings)?;
        let else_branch =
            self.type_expression_inner(if_expr.else_branch, context, info_journal, new_mappings)?;
        let then_type = self
            .widen_expression_type_journaled(self.typed_arena.get(then_branch).ty, info_journal, 0)
            .map_err(|error| TyperError::IfBranchTypeCannotBeWidened {
                source: self.source,
                tree_index: tree.index(),
                branch_tree_index: if_expr.then_branch.index(),
                error: Box::new(error),
            })?;
        let else_type = self
            .widen_expression_type_journaled(self.typed_arena.get(else_branch).ty, info_journal, 0)
            .map_err(|error| TyperError::IfBranchTypeCannotBeWidened {
                source: self.source,
                tree_index: tree.index(),
                branch_tree_index: if_expr.else_branch.index(),
                error: Box::new(error),
            })?;
        let ty = self
            .join_expression_types(then_type, else_type)
            .map_err(|error| TyperError::IfBranchJoinUnsupported {
                source: self.source,
                tree_index: tree.index(),
                left: then_type,
                right: else_type,
                error: Box::new(error),
            })?;
        Ok(TypedAstBuilder::new(&mut self.typed_arena).if_expr(
            cond,
            then_branch,
            else_branch,
            ty,
            position,
        ))
    }

    pub(in crate::typer) fn type_while_expression(
        &mut self,
        tree: TreeId<Untyped>,
        while_expr: dotty_core::ast::While<Untyped>,
        position: Option<SourceSpan>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        self.require_while_child_tree(tree, while_expr.cond, "condition")?;
        self.require_while_child_tree(tree, while_expr.body, "body")?;
        let cond = self
            .type_expression_expected_inner(
                while_expr.cond,
                context,
                self.definitions.boolean,
                info_journal,
                new_mappings,
            )
            .map_err(|error| match error {
                TyperError::ExpectedExpressionTypeMismatch { actual, .. } => {
                    TyperError::WhileConditionTypeMismatch {
                        source: self.source,
                        tree_index: tree.index(),
                        actual,
                        expected: self.definitions.boolean,
                    }
                }
                TyperError::ExpectedExpressionConformanceUnsupported { actual, error, .. } => {
                    TyperError::WhileConditionConformanceUnsupported {
                        source: self.source,
                        tree_index: tree.index(),
                        actual,
                        expected: self.definitions.boolean,
                        error,
                    }
                }
                other => other,
            })?;
        let body =
            self.type_expression_inner(while_expr.body, context, info_journal, new_mappings)?;
        Ok(TypedAstBuilder::new(&mut self.typed_arena).while_expr(
            cond,
            body,
            self.definitions.unit,
            position,
        ))
    }

    pub(in crate::typer) fn type_return_expression(
        &mut self,
        tree: TreeId<Untyped>,
        return_expr: dotty_core::ast::Return<Untyped>,
        position: Option<SourceSpan>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let method = self.enclosing_method_for_return(context.owner, tree.index())?;
        if let Some(target) = return_expr.from {
            let Some(target_tree) = self.arena.try_get(target) else {
                return Err(TyperError::MalformedReturnTarget {
                    source: self.source,
                    tree_index: tree.index(),
                    target_tree_index: target.index(),
                });
            };
            if !matches!(&target_tree.kind, TreeKind::Ident(_) | TreeKind::DefDef(_)) {
                return Err(TyperError::MalformedReturnTarget {
                    source: self.source,
                    tree_index: tree.index(),
                    target_tree_index: target.index(),
                });
            }
            return Err(TyperError::NonLocalReturnDeferred {
                source: self.source,
                tree_index: tree.index(),
                target_tree_index: target.index(),
            });
        }
        let expected = self.explicit_return_result_type(method, tree.index(), info_journal)?;
        let expr = if let Some(expr_tree) = return_expr.expr {
            Some(
                self.type_expression_expected_inner(
                    expr_tree,
                    context,
                    expected,
                    info_journal,
                    new_mappings,
                )
                .map_err(|error| match error {
                    TyperError::ExpectedExpressionTypeMismatch { actual, .. } => {
                        TyperError::ReturnExpressionTypeMismatch {
                            source: self.source,
                            tree_index: tree.index(),
                            actual,
                            expected,
                        }
                    }
                    TyperError::ExpectedExpressionConformanceUnsupported {
                        actual, error, ..
                    } => TyperError::ReturnExpressionConformanceUnsupported {
                        source: self.source,
                        tree_index: tree.index(),
                        actual,
                        expected,
                        error,
                    },
                    other => other,
                })?,
            )
        } else {
            let unit_literal_type = self
                .store
                .types
                .alloc(Type::Constant(dotty_core::Constant::Unit));
            let actual =
                self.widen_expression_type_journaled(unit_literal_type, info_journal, 0)?;
            self.check_return_expression_type(tree.index(), actual, expected)?;
            Some(TypedAstBuilder::new(&mut self.typed_arena).literal(
                dotty_core::Constant::Unit,
                unit_literal_type,
                position,
            ))
        };
        Ok(TypedAstBuilder::new(&mut self.typed_arena).return_expr(
            expr,
            None,
            self.definitions.nothing_type,
            position,
        ))
    }
}
