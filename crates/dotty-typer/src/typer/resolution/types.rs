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
        let mut local_type_candidates = Vec::new();
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
                local_type_candidates = candidates;
                break;
            }
        }
        let contexts = self.source_context_chain(context, tree_index)?;
        let active_local_import_scopes = self.active_local_import_scopes.clone();
        for (local_context, mut local_scopes) in active_local_import_scopes.into_iter().rev() {
            if local_context != context {
                continue;
            }
            while let Some(stack) = local_scopes {
                let frame = self
                    .expression_scopes
                    .get(stack.index())
                    .ok_or(TyperError::ExpressionLocalScopeStackMissing { stack })?;
                let imports = frame.imports.clone();
                let parent = frame.parent;
                for selection in [ImportSelection::Explicit, ImportSelection::Wildcard] {
                    let candidates = self.lookup_local_import_candidates(
                        &imports,
                        stack,
                        name,
                        true,
                        selection,
                        SourceTreeLocation {
                            tree_index,
                            position,
                        },
                    )?;
                    if !candidates.is_empty() {
                        let mut visible = local_type_candidates.clone();
                        for context_id in &contexts {
                            let source_context = self.index.source_context(*context_id);
                            let lexical = self
                                .store
                                .scopes
                                .get(source_context.lexical_scope)
                                .lookup_all(&name);
                            let type_members: Vec<_> = lexical
                                .iter()
                                .copied()
                                .filter(|symbol| {
                                    is_type_symbol(self.store.symbols.get(*symbol).kind)
                                })
                                .collect();
                            if !type_members.is_empty() {
                                visible.extend(type_members);
                                break;
                            }
                        }
                        visible.extend(candidates);
                        self.deduplicate_import_candidates(&mut visible, true);
                        return self.unique_type_candidate(&visible, name, tree_index, position);
                    }
                }
                local_scopes = parent;
            }
        }
        if !local_type_candidates.is_empty() {
            return self.unique_type_candidate(&local_type_candidates, name, tree_index, position);
        }
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
        if let Some(symbol) =
            self.unique_type_candidate(&package_candidates, name, tree_index, position)?
        {
            return Ok(Some(symbol));
        }
        self.lookup_default_type_import(name, tree_index, position)
    }

    fn lookup_default_type_import(
        &mut self,
        name: dotty_core::Name,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<Option<SymbolId>, TyperError> {
        // Builtin source spellings have session-canonical types and symbols;
        // keep their existing projection path instead of prefixing them with
        // the package from which the classpath resolver found the name.
        if self
            .definitions
            .source_builtin_type(self.store, name)
            .is_some()
        {
            return Ok(None);
        }
        // Scala 3.9 nests `scala.*` inside `java.lang.*`; therefore a matching
        // Scala member has higher priority. Predef's term import is separate.
        for path in [vec!["scala"], vec!["java", "lang"]] {
            let package = match self.packages.symbol(&path) {
                Some(package) => Some(package),
                None => {
                    let owned_path = path
                        .iter()
                        .map(|part| (*part).to_owned())
                        .collect::<Vec<_>>();
                    self.resolve_external_package(&owned_path, tree_index)?
                }
            };
            let Some(package) = package else {
                continue;
            };
            let candidates = self
                .scopes_of(package)
                .into_iter()
                .flat_map(|scope| {
                    self.store
                        .scopes
                        .get(scope)
                        .lookup_all(&name)
                        .iter()
                        .copied()
                })
                .filter(|symbol| is_type_symbol(self.store.symbols.get(*symbol).kind))
                .collect::<Vec<_>>();
            if let Some(symbol) =
                self.unique_type_candidate(&candidates, name, tree_index, position)?
            {
                return Ok(Some(symbol));
            }

            let request = MemberRequest {
                prefix: self.package_type_prefix(package),
                name,
                selector: MemberSelector::Unique,
                space: MemberSpace::Prefix,
            };
            let Some(symbol) =
                self.resolver
                    .resolve_member(self.store, &request)
                    .map_err(|error| TyperError::SymbolResolution {
                        source: self.source,
                        tree_index,
                        error,
                    })?
            else {
                continue;
            };
            if !self.store.symbols.contains(symbol) {
                return Err(TyperError::UnknownSymbol { symbol });
            }
            let entry = self.store.symbols.get(symbol);
            if !is_type_symbol(entry.kind) {
                return Err(TyperError::WrongTypeNameKind {
                    source: self.source,
                    tree_index,
                    name,
                    symbol,
                    kind: entry.kind,
                    position,
                });
            }
            return Ok(Some(symbol));
        }
        Ok(None)
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

fn is_type_symbol(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Class
            | SymbolKind::Trait
            | SymbolKind::ModuleClass
            | SymbolKind::TypeParameter
            | SymbolKind::TypeAlias
    )
}
