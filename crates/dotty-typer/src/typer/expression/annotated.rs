//! Source term annotation lowering.

use std::collections::HashSet;

use super::super::{ExpressionContext, SourceTyper, TyperError};
use dotty_core::ast::*;
use dotty_core::types::*;
use dotty_core::*;

impl SourceTyper<'_> {
    pub(in crate::typer) fn type_annotated_expression(
        &mut self,
        _tree: TreeId<Untyped>,
        annotated: Annotated<Untyped>,
        position: Option<SourceSpan>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let annotation = self.type_source_annotation_inner(
            annotated.annotation,
            context,
            info_journal,
            new_mappings,
        )?;
        let expr =
            self.type_value_expression_inner(annotated.expr, context, info_journal, new_mappings)?;
        let expr_type = self.typed_arena.get(expr).ty;
        let underlying =
            if self.is_stable_annotated_expression_type(expr_type, annotated.expr.index())? {
                expr_type
            } else {
                self.widen_expression_type_journaled(expr_type, info_journal, 0)?
            };
        let annotated_type = self.store.types.alloc(Type::Annotated {
            underlying,
            annotation,
        });
        let type_tree_position = self
            .arena
            .try_get(annotated.annotation)
            .and_then(|tree| tree.position);
        let mut builder = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types);
        let tpt = builder.type_tree(annotated_type, type_tree_position);
        Ok(builder.typed_expr(expr, tpt, annotated_type, position))
    }

    fn is_stable_annotated_expression_type(
        &self,
        ty: TypeId,
        tree_index: u32,
    ) -> Result<bool, TyperError> {
        let mut current = ty;
        let mut visited = HashSet::new();
        for _ in 0..crate::types::MAX_TYPE_NORMALIZATION_DEPTH {
            if !visited.insert(current) {
                return Err(TyperError::TypeNormalization(
                    crate::types::TypeNormalizeError::NormalizationCycle { ty: current },
                ));
            }
            match self.store.types.try_get(current) {
                Some(Type::ThisType { .. }) => return Ok(true),
                Some(Type::TermRef {
                    prefix,
                    target: TermRefTarget::Symbol(symbol),
                }) => {
                    if !self.store.symbols.contains(*symbol) {
                        return Err(TyperError::UnknownSymbol { symbol: *symbol });
                    }
                    if self.store.symbols.get(*symbol).kind == SymbolKind::Parameter
                        || self
                            .require_stable_selection_prefix(current, tree_index)
                            .is_err()
                    {
                        return Ok(false);
                    }
                    if *prefix == self.definitions.no_prefix {
                        return Ok(true);
                    }
                    current = *prefix;
                }
                _ => return Ok(false),
            }
        }
        Err(TyperError::TypeNormalization(
            crate::types::TypeNormalizeError::TooDeep,
        ))
    }
}
