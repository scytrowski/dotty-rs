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
        let typed = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
            .type_tree(ty, source_node.position);
        self.record_new_child(source_tree, typed, new_mappings)?;
        Ok(typed)
    }

    fn record_new_child(
        &mut self,
        source_tree: TreeId<Untyped>,
        typed: TreeId<Typed>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<(), TyperError> {
        self.typed_index
            .insert(self.source, source_tree, typed)
            .map_err(|error| TyperError::ConflictingTypedExpression {
                source: error.source,
                tree_index: error.untyped.index(),
                existing: error.existing.index(),
                attempted: error.attempted.index(),
            })?;
        new_mappings.push((self.source, source_tree));
        Ok(())
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
        if let TreeKind::Template(template) = &type_tree.kind {
            let deferred = TyperError::AnonymousClassInstantiationDeferred {
                source: self.source,
                tree_index: tree.index(),
            };
            let [parent] = template.parents[..] else {
                return Err(deferred);
            };
            if !self.empty_template(template)
                || !matches!(
                    self.arena.try_get(parent).map(|n| &n.kind),
                    Some(TreeKind::Ident(_))
                )
            {
                return Err(deferred);
            }
            let ctor = template.constructor;
            let lexical = self.expression_type_context(context)?;
            let parent_ty = self.type_of_tpt_inner_journaled(parent, lexical, info_journal)?;
            let Some(trait_symbol) = self.store.types.get(parent_ty).reference_symbol() else {
                return Err(deferred);
            };
            if !self.source_marker_trait(trait_symbol) {
                return Err(deferred);
            }
            self.ensure_constructor_class_info(trait_symbol, info_journal)?;
            let typed_parent = self.reify_constructor_type_tree(parent, parent_ty, new_mappings)?;
            let name = Name::new(self.store.names.intern("$anon"), Namespace::Type);
            let class = crate::typer::synthetic_class::synthetic_symbol(
                self,
                name,
                context.owner,
                SymbolKind::Class,
                SymbolFlags::FINAL,
                SymbolInfo::Missing,
            );
            self.store.symbols.get_mut(class).position = position;
            let class_ref = self
                .store
                .types
                .alloc(Type::type_ref(self.definitions.no_prefix, class));
            let scope = self.store.scopes.alloc(Scope::new(Some(class)));
            let trees = self.finish_local_class(
                class,
                class_ref,
                scope,
                vec![],
                Some(typed_parent),
                position,
            );
            self.typed_arena.get_mut(trees.template).position = self.arena.get(new.tpt).position;
            self.typed_arena.get_mut(trees.constructor).position = self.arena.get(ctor).position;
            self.record_new_child(new.tpt, trees.template, new_mappings)?;
            self.record_new_child(ctor, trees.constructor, new_mappings)?;
            let mut builder = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types);
            let widened = builder.typed_expr(trees.instance, typed_parent, parent_ty, position);
            return Ok(builder.block(vec![trees.definition], widened, parent_ty, position));
        }
        let type_context = self.expression_type_context(context)?;
        let instance_type =
            self.type_of_tpt_inner_journaled(new.tpt, type_context, info_journal)?;
        let class = self.instantiable_class_of_type(instance_type, info_journal)?;
        self.ensure_constructor_class_info(class, info_journal)?;
        let typed_tpt = self.reify_constructor_type_tree(new.tpt, instance_type, new_mappings)?;
        Ok(
            TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).new_expr(
                typed_tpt,
                instance_type,
                position,
            ),
        )
    }

    // An empty implementation is sound for a source marker trait: it has no
    // inherited obligations, constructor arguments or self type to satisfy.
    fn source_marker_trait(&self, symbol: SymbolId) -> bool {
        let entry = self.store.symbols.get(symbol);
        if entry.kind != SymbolKind::Trait || entry.flags.contains(SymbolFlags::SEALED) {
            return false;
        }
        let Some(SourceDefinition::Canonical { source, tree }) = self.index.definition_of(symbol)
        else {
            return false;
        };
        if source != self.source {
            return false;
        }
        let Some(TreeKind::TypeDef(def)) = self.arena.try_get(tree).map(|n| &n.kind) else {
            return false;
        };
        let Some(TreeKind::Template(template)) = self.arena.try_get(def.rhs).map(|n| &n.kind)
        else {
            return false;
        };
        template.parents.is_empty() && self.empty_template(template)
    }

    fn empty_template(&self, template: &Template<Untyped>) -> bool {
        template.body.is_empty()
            && template.self_val.is_none()
            && template.metadata.derives.is_empty()
            && template.metadata.uses.is_empty()
            && matches!(self.arena.try_get(template.constructor).map(|n| &n.kind), Some(TreeKind::DefDef(ctor))
                if ctor.type_params.is_empty() && ctor.value_param_clauses.is_empty() && ctor.rhs.is_none()
                && ctor.metadata == Modifiers::default())
    }
}
