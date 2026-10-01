//! Source `new` expression typing.

use super::super::{ExpressionContext, SourceTyper, TyperError};
use dotty_core::ast::*;
use dotty_core::*;

impl SourceTyper<'_> {
    pub(in crate::typer) fn reify_constructor_type_tree(
        &mut self,
        source_tree: TreeId<Untyped>,
        ty: TypeId,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        if let Some(typed) = self.typed_index.get(self.source, source_tree) {
            return Ok(typed);
        }
        let Some(source_node) = self.arena.try_get(source_tree) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: source_tree.index(),
            });
        };
        let typed = TypedAstBuilder::new(&mut self.typed_arena).type_tree(ty, source_node.position);
        self.typed_index
            .insert(self.source, source_tree, typed)
            .map_err(|error| TyperError::ConflictingTypedExpression {
                source: error.source,
                tree_index: error.untyped.index(),
                existing: error.existing.index(),
                attempted: error.attempted.index(),
            })?;
        new_mappings.push((self.source, source_tree));
        Ok(typed)
    }
    pub(in crate::typer) fn type_new_expression(
        &mut self,
        tree: TreeId<Untyped>,
        new: dotty_core::ast::New<Untyped>,
        position: Option<SourceSpan>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let Some(type_tree) = self.arena.try_get(new.tpt) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: new.tpt.index(),
            });
        };
        if matches!(type_tree.kind, TreeKind::Template(_)) {
            return Err(TyperError::AnonymousClassInstantiationDeferred {
                source: self.source,
                tree_index: tree.index(),
            });
        }
        let type_context = self.expression_type_context(context)?;
        let instance_type = self.type_of_tpt_inner(new.tpt, type_context)?;
        let class = self.instantiable_class_of_type(instance_type, info_journal)?;
        self.ensure_constructor_class_info(class, info_journal)?;
        let typed_tpt = self.reify_constructor_type_tree(new.tpt, instance_type, new_mappings)?;
        Ok(
            TypedAstBuilder::new(&mut self.typed_arena).new_expr(
                typed_tpt,
                instance_type,
                position,
            ),
        )
    }
}
