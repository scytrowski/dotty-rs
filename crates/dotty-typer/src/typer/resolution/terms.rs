//! Resolution of source expression terms.

use super::super::*;

impl SourceTyper<'_> {
    pub(in crate::typer) fn resolve_expression_term(
        &mut self,
        name: dotty_core::Name,
        context: ExpressionContext,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<SymbolId, TyperError> {
        let candidates = self.expression_term_candidates(name, context, tree_index, position)?;
        self.unique_expression_term(&candidates, name, tree_index, position)
    }

    /// Resolves the highest-precedence lexical/import bucket without choosing
    /// among method overloads. Ordinary expression references continue to use
    /// `resolve_expression_term` and defer when that bucket has several
    /// methods; Apply uses the full bucket for applicability filtering.
    pub(in crate::typer) fn expression_term_candidates(
        &mut self,
        name: dotty_core::Name,
        context: ExpressionContext,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<Vec<SymbolId>, TyperError> {
        self.validate_expression_scope_stack(context.local_scopes)?;
        let mut local_scope = context.local_scopes;
        while let Some(stack) = local_scope {
            let frame = self
                .expression_scopes
                .get(stack.index())
                .copied()
                .ok_or(TyperError::ExpressionLocalScopeStackMissing { stack })?;
            let candidates = self
                .store
                .scopes
                .get(frame.scope)
                .lookup_all(&name)
                .to_vec();
            if !candidates.is_empty() {
                return Ok(candidates);
            }
            local_scope = frame.parent;
        }

        let contexts = self.source_context_chain(context.lexical, tree_index)?;
        let fallback_scope = self
            .store
            .symbols
            .contains(context.owner)
            .then(|| self.store.symbols.get(context.owner).owner)
            .flatten();
        let fallback_scope = fallback_scope
            .filter(|owner| {
                self.store.symbols.contains(*owner)
                    && matches!(
                        self.store.symbols.get(*owner).kind,
                        SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
                    )
            })
            .and_then(|owner| self.index.scope_of(owner))
            .filter(|scope| {
                contexts.iter().all(|context_id| {
                    self.index.source_context(*context_id).lexical_scope != *scope
                })
            });
        for (context_position, context_id) in contexts.iter().enumerate() {
            let source_context = self.index.source_context(*context_id);
            let candidates = self
                .store
                .scopes
                .get(source_context.lexical_scope)
                .lookup_all(&name)
                .to_vec();
            if !candidates.is_empty() {
                return Ok(candidates);
            }
            // Top-level source methods are owned by their synthetic package
            // module class, while their recorded lexical context is the
            // package wrapper. Check those same-unit members before imports,
            // matching their source-declaration precedence.
            if context_position == 0
                && let Some(scope) = fallback_scope
            {
                let candidates = self.store.scopes.get(scope).lookup_all(&name).to_vec();
                if !candidates.is_empty() {
                    return Ok(candidates);
                }
            }
            for selection in [ImportSelection::Explicit, ImportSelection::Wildcard] {
                let candidates = self.lookup_import_candidates(
                    &[*context_id],
                    name,
                    false,
                    selection,
                    SourceTreeLocation {
                        tree_index,
                        position,
                    },
                )?;
                if !candidates.is_empty() {
                    return Ok(candidates);
                }
            }
        }
        Err(TyperError::TermNameNotFound {
            source: self.source,
            tree_index,
            name,
            position,
        })
    }

    pub(in crate::typer) fn unique_expression_term(
        &self,
        candidates: &[SymbolId],
        name: dotty_core::Name,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<SymbolId, TyperError> {
        match candidates {
            [symbol] => Ok(*symbol),
            many if many
                .iter()
                .all(|symbol| self.store.symbols.get(*symbol).kind == SymbolKind::Method) =>
            {
                Err(TyperError::OverloadedReferenceDeferred {
                    source: self.source,
                    tree_index,
                    name,
                })
            }
            _ => Err(TyperError::AmbiguousTermReference {
                source: self.source,
                tree_index,
                name,
                position,
            }),
        }
    }

    pub(in crate::typer) fn source_module_class_of_object(
        &self,
        object: SymbolId,
    ) -> Result<SymbolId, TyperError> {
        if !self.store.symbols.contains(object) {
            return Err(TyperError::UnknownSymbol { symbol: object });
        }
        let declaration = self.store.symbols.get(object);
        if declaration.kind != SymbolKind::Object {
            return Err(TyperError::ObjectModuleClassUnavailable { object });
        }
        let Some(owner) = declaration.owner else {
            return Err(TyperError::ObjectModuleClassUnavailable { object });
        };
        let Some(SourceDefinition::Canonical { source, tree }) = self.index.definition_of(object)
        else {
            return Err(TyperError::ObjectModuleClassUnavailable { object });
        };
        if source != self.source {
            return Err(TyperError::ObjectModuleClassUnavailable { object });
        }
        let Some(module_class) = self.index.derived_symbol_at(owner, source, tree) else {
            return Err(TyperError::ObjectModuleClassUnavailable { object });
        };
        if !self.store.symbols.contains(module_class)
            || self.store.symbols.get(module_class).kind != SymbolKind::ModuleClass
            || self.index.definition_of(module_class)
                != Some(SourceDefinition::Derived { source, tree })
        {
            return Err(TyperError::ObjectModuleClassUnavailable { object });
        }
        Ok(module_class)
    }
}
