//! Source import and qualified-name resolution.

use super::super::*;

#[derive(Clone, Copy)]
pub(in crate::typer) enum ImportSelection {
    Explicit,
    Wildcard,
}

#[derive(Clone, Copy)]
pub(in crate::typer) struct SourceImport {
    tree: TreeId<Untyped>,
    context: SourceContextId,
    parent: Option<SourceContextId>,
}

impl SourceTyper<'_> {
    pub(in crate::typer) fn resolve_qualifier_symbol(
        &mut self,
        tree: TreeId<Untyped>,
        context: SourceContextId,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<Option<SymbolId>, TyperError> {
        let Some(node) = self.arena.try_get(tree) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: tree.index(),
            });
        };
        match &node.kind {
            TreeKind::Ident(ident) => {
                if let Some(symbol) =
                    self.lookup_context_symbol(ident.name, context, tree_index, position)?
                {
                    return Ok(Some(symbol));
                }
                let segment = self.store.names.resolve(ident.name.text()).to_owned();
                self.resolve_external_package(&[segment], tree_index)
            }
            TreeKind::Select(select) => {
                let Some(qualifier) =
                    self.resolve_qualifier_symbol(select.qualifier, context, tree_index, position)?
                else {
                    return Ok(None);
                };
                let mut candidates = Vec::new();
                for scope in self.scopes_of(qualifier) {
                    if let Some(symbol) =
                        self.unique_scoped_symbol(scope, select.name, tree_index, position)?
                    {
                        candidates.push(symbol);
                    }
                }
                candidates.sort_by_key(|symbol| symbol.index());
                candidates.dedup();
                if let Some(symbol) =
                    self.unique_symbol_candidate(&candidates, select.name, tree_index, position)?
                {
                    return Ok(Some(symbol));
                }
                if self.store.symbols.get(qualifier).kind == SymbolKind::Package {
                    let mut path = self.package_path(qualifier);
                    path.push(self.store.names.resolve(select.name.text()).to_owned());
                    if let Some(package) = self.resolve_external_package(&path, tree_index)? {
                        return Ok(Some(package));
                    }
                }
                let prefix = self.type_prefix_for_qualifier(qualifier);
                let request = MemberRequest {
                    prefix,
                    name: select.name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                };
                self.resolver
                    .resolve_member(self.store, &request)
                    .map_err(|error| TyperError::SymbolResolution {
                        source: self.source,
                        tree_index,
                        error,
                    })
            }
            _ => Err(TyperError::UnsupportedTypeTree {
                source: self.source,
                tree_index,
                tree_kind: tree_kind_name(&node.kind),
            }),
        }
    }

    pub(in crate::typer) fn resolve_external_package(
        &mut self,
        path: &[String],
        tree_index: u32,
    ) -> Result<Option<SymbolId>, TyperError> {
        let segments: Vec<_> = path.iter().map(String::as_str).collect();
        let package = self
            .resolver
            .resolve_package(self.store, &segments)
            .map_err(|error| TyperError::SymbolResolution {
                source: self.source,
                tree_index,
                error,
            })?;
        if let Some(symbol) = package {
            if !self.store.symbols.contains(symbol) {
                return Err(TyperError::UnknownSymbol { symbol });
            }
            if self.store.symbols.get(symbol).kind != SymbolKind::Package {
                return Err(TyperError::SymbolResolution {
                    source: self.source,
                    tree_index,
                    error: ResolutionError::Malformed {
                        reason: "package resolution returned a non-package symbol".to_owned(),
                    },
                });
            }
        }
        Ok(package)
    }

    pub(in crate::typer) fn package_path(&self, package: SymbolId) -> Vec<String> {
        let mut path = Vec::new();
        let mut current = Some(package);
        while let Some(symbol) = current {
            let entry = self.store.symbols.get(symbol);
            if entry.kind != SymbolKind::Package {
                break;
            }
            let name = self.store.names.resolve(entry.name.text());
            if !name.is_empty() {
                path.push(name.to_owned());
            }
            current = entry.owner;
        }
        path.reverse();
        path
    }

    pub(in crate::typer) fn source_context_chain(
        &self,
        context: SourceContextId,
        tree_index: u32,
    ) -> Result<Vec<SourceContextId>, TyperError> {
        let mut contexts = Vec::new();
        let mut current = Some(context);
        while let Some(context_id) = current {
            let Some(source_context) = self.index.try_source_context(context_id) else {
                return Err(TyperError::SourceContextMissing {
                    source: self.source,
                    tree_index,
                    context_index: context_id.index(),
                });
            };
            contexts.push(context_id);
            current = source_context.parent;
        }
        Ok(contexts)
    }

    pub(in crate::typer) fn lookup_imported_symbols(
        &mut self,
        source_import: SourceImport,
        wanted: dotty_core::Name,
        type_only: bool,
        selection: ImportSelection,
        location: SourceTreeLocation,
    ) -> Result<Vec<SymbolId>, TyperError> {
        let Some(node) = self.arena.try_get(source_import.tree) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: source_import.tree.index(),
            });
        };
        let TreeKind::Import(import) = &node.kind else {
            return Err(TyperError::MalformedSourceImport {
                source: self.source,
                import_tree_index: source_import.tree.index(),
            });
        };

        let wildcard = self.store.names.get("*");
        let hidden = self.store.names.get("_");
        let mut hidden_names = Vec::new();
        for selector in &import.selectors {
            if let Some(renamed) = selector.renamed {
                let Some(rename_tree) = self.arena.try_get(renamed) else {
                    return Err(TyperError::MalformedSourceImport {
                        source: self.source,
                        import_tree_index: source_import.tree.index(),
                    });
                };
                if !matches!(rename_tree.kind, TreeKind::Ident(_)) {
                    return Err(TyperError::MalformedSourceImport {
                        source: self.source,
                        import_tree_index: source_import.tree.index(),
                    });
                }
                hidden_names.push(selector.imported.text());
            }
        }
        let mut relevant = Vec::new();
        for selector in &import.selectors {
            if Some(selector.imported.text()) == wildcard {
                if matches!(selection, ImportSelection::Explicit)
                    || hidden_names.contains(&wanted.text())
                {
                    continue;
                }
                relevant.push((selector.imported, true));
                continue;
            }
            if matches!(selection, ImportSelection::Wildcard) {
                continue;
            }
            if selector.bound.is_some() {
                continue;
            }
            let public_name = if let Some(renamed) = selector.renamed {
                let Some(rename_tree) = self.arena.try_get(renamed) else {
                    return Err(TyperError::MalformedSourceImport {
                        source: self.source,
                        import_tree_index: source_import.tree.index(),
                    });
                };
                let TreeKind::Ident(ident) = &rename_tree.kind else {
                    return Err(TyperError::MalformedSourceImport {
                        source: self.source,
                        import_tree_index: source_import.tree.index(),
                    });
                };
                ident.name.text()
            } else {
                selector.imported.text()
            };
            if public_name == wanted.text() && Some(public_name) != hidden {
                relevant.push((selector.imported, false));
            }
        }
        if relevant.is_empty() {
            return Ok(Vec::new());
        }

        let qualifier_context = source_import.parent.unwrap_or(source_import.context);
        let Some(qualifier) = self.import_qualifier_symbol(
            import.expr,
            qualifier_context,
            source_import.tree.index(),
            location.position,
        )?
        else {
            return Err(TyperError::ImportQualifierNotFound {
                source: self.source,
                import_tree_index: source_import.tree.index(),
            });
        };
        let scopes = self.scopes_of(qualifier);
        let mut matches = Vec::new();
        for (imported, is_wildcard) in relevant {
            let name = if is_wildcard {
                wanted
            } else if type_only {
                dotty_core::Name::new(imported.text(), dotty_core::Namespace::Type)
            } else {
                imported
            };
            let mut found_locally = false;
            for scope in &scopes {
                if type_only {
                    let candidates = self.store.scopes.get(*scope).lookup_all(&name);
                    found_locally |= !candidates.is_empty();
                    matches.extend_from_slice(candidates);
                } else {
                    let candidates = self.scoped_symbols(*scope, name);
                    found_locally |= !candidates.is_empty();
                    matches.extend(candidates);
                }
            }
            if !found_locally {
                let request = MemberRequest {
                    prefix: self.type_prefix_for_qualifier(qualifier),
                    name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                };
                if let Some(symbol) =
                    self.resolver
                        .resolve_member(self.store, &request)
                        .map_err(|error| TyperError::SymbolResolution {
                            source: self.source,
                            tree_index: location.tree_index,
                            error,
                        })?
                {
                    if !self.store.symbols.contains(symbol) {
                        return Err(TyperError::UnknownSymbol { symbol });
                    }
                    matches.push(symbol);
                }
            }
        }
        matches.sort_by_key(|symbol| symbol.index());
        matches.dedup();
        self.deduplicate_import_candidates(&mut matches, type_only);
        if type_only {
            Ok(self
                .unique_type_candidate(&matches, wanted, location.tree_index, location.position)?
                .into_iter()
                .collect())
        } else {
            Ok(matches)
        }
    }

    pub(in crate::typer) fn type_local_import_statement(
        &mut self,
        tree: TreeId<Untyped>,
        import: dotty_core::ast::Import<Untyped>,
        position: Option<SourceSpan>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let qualifier = self.type_import_qualifier(
            import.expr,
            context,
            tree.index(),
            info_journal,
            new_mappings,
        )?;
        let mut selectors = Vec::with_capacity(import.selectors.len());
        for selector in import.selectors {
            let renamed = selector
                .renamed
                .map(|tree| {
                    self.type_import_selector_child(
                        tree,
                        context.lexical,
                        tree.index(),
                        info_journal,
                        new_mappings,
                    )
                })
                .transpose()?;
            let bound = selector
                .bound
                .map(|tree| {
                    self.type_import_selector_child(
                        tree,
                        context.lexical,
                        tree.index(),
                        info_journal,
                        new_mappings,
                    )
                })
                .transpose()?;
            selectors.push(dotty_core::ast::ImportSelector {
                imported: selector.imported,
                imported_backquoted: selector.imported_backquoted,
                renamed,
                bound,
            });
        }
        // Scala 3.9 keeps Import as a statement node with no value type. The
        // qualifier and selector children carry their own precise types.
        let typed = self.typed_arena.alloc(Tree {
            kind: TreeKind::Import(dotty_core::ast::Import {
                expr: qualifier,
                selectors,
            }),
            position,
            ty: self.store.types.alloc(Type::NoType),
        });
        self.typed_index
            .insert(self.source, tree, typed)
            .map_err(|error| TyperError::ConflictingTypedExpression {
                source: error.source,
                tree_index: error.untyped.index(),
                existing: error.existing.index(),
                attempted: error.attempted.index(),
            })?;
        new_mappings.push((self.source, tree));
        Ok(typed)
    }

    fn type_import_qualifier(
        &mut self,
        tree: TreeId<Untyped>,
        context: ExpressionContext,
        import_tree_index: u32,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        if let Some(typed) = self.typed_index.get(self.source, tree) {
            return Ok(typed);
        }
        let source_node =
            self.arena
                .try_get(tree)
                .cloned()
                .ok_or(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: tree.index(),
                })?;
        let symbol = self
            .resolve_qualifier_symbol(
                tree,
                context.lexical,
                import_tree_index,
                source_node.position,
            )?
            .ok_or(TyperError::ImportQualifierNotFound {
                source: self.source,
                import_tree_index,
            })?;
        let ty = self.import_qualifier_type(
            symbol,
            context.owner,
            tree.index(),
            import_tree_index,
            info_journal,
        )?;
        let typed = match source_node.kind {
            TreeKind::Ident(ident) => TypedAstBuilder::new(&mut self.typed_arena)
                .ident_with_backquoted(ident.name, ident.backquoted, ty, source_node.position),
            TreeKind::Select(selection) => {
                let qualifier = self.type_import_qualifier(
                    selection.qualifier,
                    context,
                    import_tree_index,
                    info_journal,
                    new_mappings,
                )?;
                TypedAstBuilder::new(&mut self.typed_arena).select(
                    qualifier,
                    selection.name,
                    selection.backquoted,
                    ty,
                    source_node.position,
                )
            }
            _ => {
                return Err(TyperError::MalformedSourceImport {
                    source: self.source,
                    import_tree_index,
                });
            }
        };
        self.typed_index
            .insert(self.source, tree, typed)
            .map_err(|error| TyperError::ConflictingTypedExpression {
                source: error.source,
                tree_index: error.untyped.index(),
                existing: error.existing.index(),
                attempted: error.attempted.index(),
            })?;
        new_mappings.push((self.source, tree));
        Ok(typed)
    }

    fn import_qualifier_type(
        &mut self,
        symbol: SymbolId,
        owner: SymbolId,
        tree_index: u32,
        import_tree_index: u32,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let kind = self.store.symbols.get(symbol).kind;
        match kind {
            SymbolKind::Package => Ok(self.store.types.alloc(Type::NoType)),
            SymbolKind::Object
            | SymbolKind::Field
            | SymbolKind::Value
            | SymbolKind::Parameter
            | SymbolKind::Local => {
                let ty = self.expression_type_of_symbol(symbol, owner, tree_index, info_journal)?;
                if self
                    .require_stable_selection_prefix(ty, tree_index)
                    .is_err()
                {
                    return Err(TyperError::ImportQualifierNotStable {
                        source: self.source,
                        import_tree_index,
                        symbol,
                    });
                }
                Ok(ty)
            }
            SymbolKind::Class
            | SymbolKind::Trait
            | SymbolKind::ModuleClass
            | SymbolKind::TypeAlias => {
                let prefix = self.type_symbol_prefix(symbol);
                Ok(self.store.types.alloc(Type::TypeRef {
                    prefix,
                    target: TypeRefTarget::Symbol(symbol),
                }))
            }
            _ => Err(TyperError::ImportQualifierNotStable {
                source: self.source,
                import_tree_index,
                symbol,
            }),
        }
    }

    fn type_import_selector_child(
        &mut self,
        tree: TreeId<Untyped>,
        context: SourceContextId,
        import_tree_index: u32,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        if let Some(typed) = self.typed_index.get(self.source, tree) {
            return Ok(typed);
        }
        let source_node =
            self.arena
                .try_get(tree)
                .cloned()
                .ok_or(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: tree.index(),
                })?;
        let ty = self.store.types.alloc(Type::NoType);
        let typed = match source_node.kind {
            TreeKind::Ident(ident) => TypedAstBuilder::new(&mut self.typed_arena)
                .ident_with_backquoted(ident.name, ident.backquoted, ty, source_node.position),
            TreeKind::TypeTree(_) => {
                let ty = self.type_of_tpt_inner(tree, context)?;
                self.typed_arena.alloc(Tree {
                    kind: TreeKind::TypeTree(dotty_core::ast::TypeTree),
                    position: source_node.position,
                    ty,
                })
            }
            _ => {
                return Err(TyperError::MalformedSourceImport {
                    source: self.source,
                    import_tree_index,
                });
            }
        };
        self.typed_index
            .insert(self.source, tree, typed)
            .map_err(|error| TyperError::ConflictingTypedExpression {
                source: error.source,
                tree_index: error.untyped.index(),
                existing: error.existing.index(),
                attempted: error.attempted.index(),
            })?;
        new_mappings.push((self.source, tree));
        let _ = info_journal;
        Ok(typed)
    }

    pub(in crate::typer) fn import_qualifier_symbol(
        &mut self,
        qualifier: TreeId<Untyped>,
        context: SourceContextId,
        import_tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<Option<SymbolId>, TyperError> {
        self.resolve_qualifier_symbol(qualifier, context, import_tree_index, position)
    }

    pub(in crate::typer) fn lookup_context_symbol(
        &mut self,
        name: dotty_core::Name,
        context: SourceContextId,
        import_tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<Option<SymbolId>, TyperError> {
        let contexts = self.source_context_chain(context, import_tree_index)?;
        for context_id in &contexts {
            let source_context = self.index.source_context(*context_id);
            if let Some(symbol) = self.unique_scoped_symbol(
                source_context.lexical_scope,
                name,
                import_tree_index,
                position,
            )? {
                return Ok(Some(symbol));
            }
        }
        for selection in [ImportSelection::Explicit, ImportSelection::Wildcard] {
            if let Some(symbol) = self.lookup_imports(
                &contexts,
                name,
                false,
                selection,
                SourceTreeLocation {
                    tree_index: import_tree_index,
                    position,
                },
            )? {
                return Ok(Some(symbol));
            }
        }
        Ok(None)
    }

    /// Looks up all imports at the same lexical depth together. Imports in one
    /// scope have equal precedence, so distinct matching symbols are
    /// ambiguous regardless of their source order.
    pub(in crate::typer) fn lookup_imports(
        &mut self,
        contexts: &[SourceContextId],
        name: dotty_core::Name,
        type_only: bool,
        selection: ImportSelection,
        location: SourceTreeLocation,
    ) -> Result<Option<SymbolId>, TyperError> {
        let candidates =
            self.lookup_import_candidates(contexts, name, type_only, selection, location)?;
        if candidates.is_empty() {
            return Ok(None);
        }
        if type_only {
            self.unique_type_candidate(&candidates, name, location.tree_index, location.position)
        } else {
            self.unique_symbol_candidate(&candidates, name, location.tree_index, location.position)
        }
    }

    pub(in crate::typer) fn lookup_import_candidates(
        &mut self,
        contexts: &[SourceContextId],
        name: dotty_core::Name,
        type_only: bool,
        selection: ImportSelection,
        location: SourceTreeLocation,
    ) -> Result<Vec<SymbolId>, TyperError> {
        let mut scopes: Vec<(dotty_core::ScopeId, Vec<SourceImport>)> = Vec::new();
        for context_id in contexts {
            let source_context = self.index.source_context(*context_id);
            let Some(import_tree) = source_context.import else {
                continue;
            };
            let import = SourceImport {
                tree: import_tree,
                context: *context_id,
                parent: source_context.parent,
            };
            if let Some(scope_index) = scopes
                .iter()
                .position(|(scope, _)| *scope == source_context.lexical_scope)
            {
                scopes[scope_index].1.push(import);
            } else {
                scopes.push((source_context.lexical_scope, vec![import]));
            }
        }
        for (_, imports) in scopes {
            let mut candidates = Vec::new();
            for source_import in imports {
                candidates.extend(self.lookup_imported_symbols(
                    source_import,
                    name,
                    type_only,
                    selection,
                    location,
                )?);
            }
            candidates.sort_by_key(|symbol| symbol.index());
            candidates.dedup();
            self.deduplicate_import_candidates(&mut candidates, type_only);
            if !candidates.is_empty() {
                return Ok(candidates);
            }
        }
        Ok(Vec::new())
    }

    /// Resolves imports active in one typer-owned block scope. The statements
    /// are kept in source order for identity and qualifier context, while all
    /// imports at this lexical depth contribute to the same candidate bucket.
    pub(in crate::typer) fn lookup_local_import_candidates(
        &mut self,
        imports: &[(TreeId<Untyped>, SourceContextId)],
        name: dotty_core::Name,
        type_only: bool,
        selection: ImportSelection,
        location: SourceTreeLocation,
    ) -> Result<Vec<SymbolId>, TyperError> {
        let mut candidates = Vec::new();
        for (tree, context) in imports {
            let source_context = self.index.try_source_context(*context).ok_or(
                TyperError::SourceContextMissing {
                    source: self.source,
                    tree_index: location.tree_index,
                    context_index: context.index(),
                },
            )?;
            candidates.extend(self.lookup_imported_symbols(
                SourceImport {
                    tree: *tree,
                    context: *context,
                    parent: source_context.parent,
                },
                name,
                type_only,
                selection,
                location,
            )?);
        }
        candidates.sort_by_key(|symbol| symbol.index());
        candidates.dedup();
        self.deduplicate_import_candidates(&mut candidates, type_only);
        Ok(candidates)
    }

    /// Returns the underlying symbol for a fully known chain of type aliases.
    /// Unknown or structurally described aliases remain distinct so lookup
    /// never guesses that two incomplete types are equivalent.
    pub(in crate::typer) fn imported_type_target(&self, symbol: SymbolId) -> Option<SymbolId> {
        let mut current = symbol;
        let mut seen = Vec::new();
        loop {
            if seen.contains(&current) {
                return None;
            }
            seen.push(current);
            if self.store.symbols.get(current).kind != SymbolKind::TypeAlias {
                return Some(current);
            }
            let SymbolInfo::Complete(info) = self.store.symbols.info(current) else {
                return None;
            };
            let Type::AliasingBounds { alias } = self.store.types.get(*info) else {
                return None;
            };
            let Type::TypeRef {
                target: TypeRefTarget::Symbol(target),
                ..
            } = self.store.types.get(*alias)
            else {
                return None;
            };
            current = *target;
        }
    }

    pub(in crate::typer) fn deduplicate_import_candidates(
        &self,
        candidates: &mut Vec<SymbolId>,
        type_only: bool,
    ) {
        if !type_only {
            return;
        }
        let mut unique = Vec::with_capacity(candidates.len());
        for candidate in candidates.iter().copied() {
            let canonical = self.imported_type_target(candidate);
            let already_present = canonical.is_some_and(|canonical| {
                unique
                    .iter()
                    .any(|existing| self.imported_type_target(*existing) == Some(canonical))
            });
            if !already_present {
                unique.push(candidate);
            }
        }
        *candidates = unique;
    }

    pub(in crate::typer) fn unique_scoped_symbol(
        &self,
        scope: dotty_core::ScopeId,
        name: dotty_core::Name,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<Option<SymbolId>, TyperError> {
        let candidates = self.scoped_symbols(scope, name);
        self.unique_symbol_candidate(&candidates, name, tree_index, position)
    }

    pub(in crate::typer) fn scoped_symbols(
        &self,
        scope: dotty_core::ScopeId,
        name: dotty_core::Name,
    ) -> Vec<SymbolId> {
        let direct = self.store.scopes.get(scope).lookup_all(&name);
        if !direct.is_empty() {
            return direct.to_vec();
        }
        let alternate = dotty_core::Name::new(
            name.text(),
            match name.namespace() {
                dotty_core::Namespace::Term => dotty_core::Namespace::Type,
                dotty_core::Namespace::Type => dotty_core::Namespace::Term,
            },
        );
        self.store.scopes.get(scope).lookup_all(&alternate).to_vec()
    }

    pub(in crate::typer) fn unique_symbol_candidate(
        &self,
        candidates: &[SymbolId],
        name: dotty_core::Name,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<Option<SymbolId>, TyperError> {
        match candidates {
            [] => Ok(None),
            [symbol] => Ok(Some(*symbol)),
            _ => Err(TyperError::AmbiguousTypeName {
                source: self.source,
                tree_index,
                name,
                position,
            }),
        }
    }

    pub(in crate::typer) fn scopes_of(&self, symbol: SymbolId) -> Vec<dotty_core::ScopeId> {
        let mut scopes = Vec::new();
        if let Some(scope) = self
            .packages
            .scope_of(symbol)
            .or_else(|| self.index.scope_of(symbol))
        {
            insert_scope(&mut scopes, scope);
        }
        if self.store.symbols.get(symbol).kind == SymbolKind::Package
            && let Some(package_scope) = scopes.first().copied()
        {
            let wrappers: Vec<_> = self
                .store
                .scopes
                .get(package_scope)
                .entered_symbols()
                .filter(|member| {
                    let wrapper = self.store.symbols.get(*member);
                    wrapper.kind == SymbolKind::ModuleClass
                        && wrapper.owner == Some(symbol)
                        && wrapper.origin == SymbolOrigin::Synthetic
                        && self
                            .store
                            .names
                            .resolve(wrapper.name.text())
                            .ends_with("$package$")
                })
                .collect();
            for wrapper in wrappers {
                if let Some(scope) = self.index.scope_of(wrapper) {
                    insert_scope(&mut scopes, scope);
                }
            }
        }
        if let SymbolInfo::Complete(ty) = self.store.symbols.get(symbol).info
            && let Type::ClassInfo(info) = self.store.types.get(ty)
        {
            insert_scope(&mut scopes, info.declarations);
        }
        let semantic = self.store.symbols.get(symbol);
        if semantic.kind != SymbolKind::Object {
            return scopes;
        }
        let Some(owner) = semantic.owner else {
            return scopes;
        };
        let Some(SourceDefinition::Canonical { source, tree }) = self.index.definition_of(symbol)
        else {
            return scopes;
        };
        if source == self.source
            && let Some(module_class) = self.index.derived_symbol_at(owner, source, tree)
            && self.store.symbols.get(module_class).kind == SymbolKind::ModuleClass
            && let Some(scope) = self.index.scope_of(module_class)
        {
            insert_scope(&mut scopes, scope);
        }
        scopes
    }
}
