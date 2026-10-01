//! Direct assignment expression typing.

use super::super::{ExpressionContext, SourceTyper, TyperError, tree_kind_name};
use dotty_core::ast::*;
use dotty_core::types::*;
use dotty_core::*;

impl SourceTyper<'_> {
    pub(in crate::typer) fn type_assignment_expression(
        &mut self,
        tree: TreeId<Untyped>,
        assignment: dotty_core::ast::Assign<Untyped>,
        position: Option<SourceSpan>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let lhs =
            self.type_expression_inner(assignment.lhs, context, info_journal, new_mappings)?;
        let lhs_type = self.typed_arena.get(lhs).ty;
        let target = match self.store.types.try_get(lhs_type) {
            Some(Type::TermRef {
                target: TermRefTarget::Symbol(target),
                ..
            }) => *target,
            _ => {
                let lhs_kind = self
                    .arena
                    .try_get(assignment.lhs)
                    .map_or("tree outside source arena", |node| {
                        tree_kind_name(&node.kind)
                    });
                return Err(TyperError::AssignmentLhsNotAssignable {
                    source: self.source,
                    tree_index: tree.index(),
                    lhs_tree_index: assignment.lhs.index(),
                    expression_kind: lhs_kind,
                });
            }
        };
        if !self.store.symbols.contains(target) {
            return Err(TyperError::UnknownSymbol { symbol: target });
        }
        let (target_kind, target_flags) = {
            let symbol = self.store.symbols.get(target);
            (symbol.kind, symbol.flags)
        };
        if !matches!(
            target_kind,
            SymbolKind::Local | SymbolKind::Field | SymbolKind::Variable | SymbolKind::Parameter
        ) {
            return Err(TyperError::AssignmentTargetKindUnsupported {
                source: self.source,
                tree_index: tree.index(),
                target,
                kind: target_kind,
            });
        }
        if !target_flags.contains(SymbolFlags::MUTABLE) {
            return Err(TyperError::AssignmentTargetImmutable {
                source: self.source,
                tree_index: tree.index(),
                target,
            });
        }
        let expected = self
            .widen_expression_type_journaled(lhs_type, info_journal, 0)
            .map_err(|error| TyperError::WritableAssignmentTypeUnavailable {
                source: self.source,
                tree_index: tree.index(),
                target,
                error: Box::new(error),
            })?;
        let rhs = self.type_expression_expected_inner(
            assignment.rhs,
            context,
            expected,
            info_journal,
            new_mappings,
        )?;
        Ok(TypedAstBuilder::new(&mut self.typed_arena).assign(
            lhs,
            rhs,
            self.definitions.unit,
            position,
        ))
    }
}
