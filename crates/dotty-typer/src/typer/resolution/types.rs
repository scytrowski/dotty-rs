//! Resolution of source-written type names.

use super::super::*;

impl SourceTyper<'_> {
    pub(in crate::typer) fn lookup_type_symbol(
        &mut self,
        name: dotty_core::Name,
        context: SourceContextId,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<Option<SymbolId>, TyperError> {
        for (local_context, scope) in self.active_local_type_scopes.iter().rev() {
            if *local_context != context {
                continue;
            }
            let candidates = self
                .store
                .scopes
                .get(*scope)
                .lookup_all(&name)
                .iter()
                .copied()
                .filter(|symbol| self.store.symbols.get(*symbol).kind == SymbolKind::TypeParameter)
                .collect::<Vec<_>>();
            if !candidates.is_empty() {
                return self.unique_type_candidate(&candidates, name, tree_index, position);
            }
        }
        let contexts = self.source_context_chain(context, tree_index)?;
        let mut package_candidates = Vec::new();
        for context_id in &contexts {
            let source_context = self.index.source_context(*context_id);
            let candidates = self
                .store
                .scopes
                .get(source_context.lexical_scope)
                .lookup_all(&name);
            if self.store.symbols.get(source_context.owner).kind == SymbolKind::Package {
                let (current_unit, other_units): (Vec<_>, Vec<_>) =
                    candidates.iter().copied().partition(|symbol| {
                        self.store.symbols.get(*symbol).origin == SymbolOrigin::Source(self.source)
                    });
                if let Some(symbol) =
                    self.unique_type_candidate(&current_unit, name, tree_index, position)?
                {
                    return Ok(Some(symbol));
                }
                package_candidates.extend(other_units);
            } else if let Some(symbol) =
                self.unique_type_candidate(candidates, name, tree_index, position)?
            {
                return Ok(Some(symbol));
            }
        }
        for selection in [ImportSelection::Explicit, ImportSelection::Wildcard] {
            if let Some(symbol) = self.lookup_imports(
                &contexts,
                name,
                true,
                selection,
                SourceTreeLocation {
                    tree_index,
                    position,
                },
            )? {
                return Ok(Some(symbol));
            }
        }
        self.unique_type_candidate(&package_candidates, name, tree_index, position)
    }

    pub(in crate::typer) fn lookup_term_candidate_for_type_name(
        &mut self,
        name: dotty_core::Name,
        context: SourceContextId,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<Option<SymbolId>, TyperError> {
        let term_name = dotty_core::Name::new(name.text(), dotty_core::Namespace::Term);
        let contexts = self.source_context_chain(context, tree_index)?;
        for context_id in &contexts {
            let source_context = self.index.source_context(*context_id);
            let candidates = self
                .store
                .scopes
                .get(source_context.lexical_scope)
                .lookup_all(&term_name);
            if !candidates.is_empty() {
                return self.unique_symbol_candidate(candidates, name, tree_index, position);
            }
        }
        for selection in [ImportSelection::Explicit, ImportSelection::Wildcard] {
            if let Some(symbol) = self.lookup_imports(
                &contexts,
                name,
                false,
                selection,
                SourceTreeLocation {
                    tree_index,
                    position,
                },
            )? && !matches!(
                self.store.symbols.get(symbol).kind,
                SymbolKind::Class
                    | SymbolKind::Trait
                    | SymbolKind::ModuleClass
                    | SymbolKind::TypeParameter
                    | SymbolKind::TypeAlias
            ) {
                return Ok(Some(symbol));
            }
        }
        Ok(None)
    }

    pub(in crate::typer) fn unique_type_candidate(
        &self,
        candidates: &[SymbolId],
        name: dotty_core::Name,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<Option<SymbolId>, TyperError> {
        let type_candidates: Vec<_> = candidates
            .iter()
            .copied()
            .filter(|symbol| {
                matches!(
                    self.store.symbols.get(*symbol).kind,
                    SymbolKind::Class
                        | SymbolKind::Trait
                        | SymbolKind::ModuleClass
                        | SymbolKind::TypeParameter
                        | SymbolKind::TypeAlias
                )
            })
            .collect();
        if type_candidates.is_empty() {
            match candidates {
                [symbol] => {
                    let kind = self.store.symbols.get(*symbol).kind;
                    Err(TyperError::WrongTypeNameKind {
                        source: self.source,
                        tree_index,
                        name,
                        symbol: *symbol,
                        kind,
                        position,
                    })
                }
                _ => self.unique_symbol_candidate(candidates, name, tree_index, position),
            }
        } else {
            self.unique_symbol_candidate(&type_candidates, name, tree_index, position)
        }
    }
}
