//! Source import and qualified-name resolution.

use super::super::*;

#[derive(Clone, Copy)]
pub(in crate::typer) enum ImportSelection {
    Explicit,
    Wildcard,
}

/// Why a package is present in the ordered import fallback search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::typer) enum LexicalPackageCandidateOrigin {
    CurrentPackage,
    EnclosingPackage,
    Root,
}

/// Whether candidate enumeration searches source package contexts or explicit
/// `_root_` qualification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::typer) enum LexicalPackageSearch {
    Relative,
    RootQualified,
}

/// A validated package identity available from the source lexical context.
///
/// A candidate describes where qualified lookup may search. It does not mean
/// that a requested source name or member has been resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::typer) struct LexicalPackageCandidate {
    pub package: SymbolId,
    pub scope: dotty_core::ScopeId,
    pub source_context: SourceContextId,
    pub origin: LexicalPackageCandidateOrigin,
    pub package_path: Vec<String>,
}

#[derive(Clone)]
pub(in crate::typer) struct SourceImport {
    tree: TreeId<Untyped>,
    context: SourceContextId,
    parent: Option<SourceContextId>,
    expression_context: Option<ExpressionContext>,
    local_import_scope: Option<ExpressionScopeId>,
    preceding_local_imports: Option<Vec<(TreeId<Untyped>, ExpressionContext)>>,
}

impl SourceTyper<'_> {
    /// Enumerates canonical source package candidates from the source context
    /// chain, most specific first, then the canonical root package.
    ///
    /// This is intentionally separate from qualifier resolution: calling it
    /// does not perform lookup, load symbols, or mutate semantic state.
    pub(in crate::typer) fn lexical_package_candidates(
        &self,
        context: SourceContextId,
        search: LexicalPackageSearch,
        tree_index: u32,
    ) -> Result<Vec<LexicalPackageCandidate>, TyperError> {
        if self.index.try_source_context(context).is_none() {
            return Err(TyperError::SourceContextMissing {
                source: self.source,
                tree_index,
                context_index: context.index(),
            });
        }
        let mut candidates = Vec::new();
        let mut seen_packages = HashSet::new();

        if search == LexicalPackageSearch::Relative {
            let context_limit = self.index.source_context_count();
            let mut seen_contexts = HashSet::new();
            let mut current = Some(context);
            let mut origin = LexicalPackageCandidateOrigin::CurrentPackage;
            while let Some(context_id) = current {
                if !seen_contexts.insert(context_id) || seen_contexts.len() > context_limit {
                    return Err(self.invalid_package_candidate(
                        tree_index,
                        Some(context_id.index()),
                        None,
                        "source context chain contains a cycle",
                    ));
                }
                let Some(source_context) = self.index.try_source_context(context_id) else {
                    return Err(TyperError::SourceContextMissing {
                        source: self.source,
                        tree_index,
                        context_index: context_id.index(),
                    });
                };
                if !self.store.symbols.contains(source_context.owner) {
                    return Err(self.invalid_package_candidate(
                        tree_index,
                        Some(context_id.index()),
                        Some(source_context.owner),
                        "source context owner is absent from the semantic store",
                    ));
                }
                if self.store.symbols.get(source_context.owner).kind == SymbolKind::Package {
                    let candidate = self.validated_package_candidate(
                        source_context.owner,
                        source_context.lexical_scope,
                        context_id,
                        origin,
                        tree_index,
                    )?;
                    if seen_packages.insert(candidate.package) {
                        candidates.push(candidate);
                    }
                    origin = LexicalPackageCandidateOrigin::EnclosingPackage;
                }
                current = source_context.parent;
            }
        }

        let Some(root) = self.packages.get::<&str>(&[]) else {
            if search == LexicalPackageSearch::Relative {
                return Ok(candidates);
            }
            return Err(self.invalid_package_candidate(
                tree_index,
                Some(context.index()),
                None,
                "canonical root package is absent from the package registry",
            ));
        };
        let root_candidate = self.validated_package_candidate(
            root.symbol,
            root.scope,
            context,
            LexicalPackageCandidateOrigin::Root,
            tree_index,
        )?;
        if seen_packages.insert(root_candidate.package) {
            candidates.push(root_candidate);
        }
        Ok(candidates)
    }

    fn validated_package_candidate(
        &self,
        package: SymbolId,
        context_scope: dotty_core::ScopeId,
        source_context: SourceContextId,
        origin: LexicalPackageCandidateOrigin,
        tree_index: u32,
    ) -> Result<LexicalPackageCandidate, TyperError> {
        let mut reversed_chain = Vec::new();
        let mut seen = HashSet::new();
        let mut current = Some(package);
        let canonical_root = self.packages.get::<&str>(&[]);
        if canonical_root.is_none() && origin == LexicalPackageCandidateOrigin::Root {
            return Err(self.invalid_package_candidate(
                tree_index,
                Some(source_context.index()),
                Some(package),
                "canonical root package is absent from the package registry",
            ));
        }
        loop {
            let Some(symbol) = current else {
                return Err(self.invalid_package_candidate(
                    tree_index,
                    Some(source_context.index()),
                    Some(package),
                    "package owner chain does not terminate at the canonical root",
                ));
            };
            if !seen.insert(symbol) {
                return Err(self.invalid_package_candidate(
                    tree_index,
                    Some(source_context.index()),
                    Some(symbol),
                    "package owner chain contains a cycle",
                ));
            }
            if !self.store.symbols.contains(symbol) {
                return Err(self.invalid_package_candidate(
                    tree_index,
                    Some(source_context.index()),
                    Some(symbol),
                    "package owner chain contains an unknown symbol",
                ));
            }
            let entry = self.store.symbols.get(symbol);
            if entry.kind != SymbolKind::Package {
                return Err(self.invalid_package_candidate(
                    tree_index,
                    Some(source_context.index()),
                    Some(symbol),
                    "package owner chain contains a non-package symbol",
                ));
            }
            let name = self.store.names.resolve(entry.name.text()).to_owned();
            if entry.owner.is_none() {
                if !name.is_empty() || canonical_root.is_some_and(|root| symbol != root.symbol) {
                    return Err(self.invalid_package_candidate(
                        tree_index,
                        Some(source_context.index()),
                        Some(symbol),
                        "package owner chain does not end at the canonical root",
                    ));
                }
                break;
            }
            if name.is_empty() {
                return Err(self.invalid_package_candidate(
                    tree_index,
                    Some(source_context.index()),
                    Some(symbol),
                    "non-root package has an empty segment name",
                ));
            }
            reversed_chain.push((symbol, name));
            current = entry.owner;
        }
        reversed_chain.reverse();
        if origin != LexicalPackageCandidateOrigin::Root
            && (!self.store.scopes.contains(context_scope)
                || self.store.scopes.get(context_scope).owner != Some(package))
        {
            return Err(self.invalid_package_candidate(
                tree_index,
                Some(source_context.index()),
                Some(package),
                "current package scope is not owned by its package symbol",
            ));
        }
        let mut path = Vec::with_capacity(reversed_chain.len());
        for (symbol, segment) in &reversed_chain {
            path.push(segment.clone());
            let Some(registered) = self.packages.get(&path) else {
                if origin != LexicalPackageCandidateOrigin::Root {
                    continue;
                }
                return Err(self.invalid_package_candidate(
                    tree_index,
                    Some(source_context.index()),
                    Some(*symbol),
                    "package owner chain is absent from the canonical registry",
                ));
            };
            if registered.symbol != *symbol
                || self.packages.scope_of(*symbol) != Some(registered.scope)
                || !self.store.scopes.contains(registered.scope)
                || self.store.scopes.get(registered.scope).owner != Some(*symbol)
            {
                return Err(self.invalid_package_candidate(
                    tree_index,
                    Some(source_context.index()),
                    Some(*symbol),
                    "package segment and registry identity disagree",
                ));
            }
        }
        let candidate_scope = match (self.packages.get(&path), self.packages.scope_of(package)) {
            (Some(registered), Some(registered_scope)) => {
                if registered.symbol != package
                    || registered.scope != registered_scope
                    || !self.store.scopes.contains(registered_scope)
                    || self.store.scopes.get(registered_scope).owner != Some(package)
                    || registered_scope != context_scope
                {
                    return Err(self.invalid_package_candidate(
                        tree_index,
                        Some(source_context.index()),
                        Some(package),
                        "package symbol, lexical scope, and registry identity disagree",
                    ));
                }
                registered_scope
            }
            (None, None) if origin != LexicalPackageCandidateOrigin::Root => context_scope,
            (None, None) => {
                return Err(self.invalid_package_candidate(
                    tree_index,
                    Some(source_context.index()),
                    Some(package),
                    "package owner chain is absent from the canonical registry",
                ));
            }
            _ => {
                return Err(self.invalid_package_candidate(
                    tree_index,
                    Some(source_context.index()),
                    Some(package),
                    "package symbol and scope registry entries disagree",
                ));
            }
        };
        Ok(LexicalPackageCandidate {
            package,
            scope: candidate_scope,
            source_context,
            origin,
            package_path: path,
        })
    }

    fn invalid_package_candidate(
        &self,
        tree_index: u32,
        context_index: Option<u32>,
        package: Option<SymbolId>,
        reason: &'static str,
    ) -> TyperError {
        TyperError::LexicalPackageCandidateInvalid {
            source: self.source,
            tree_index,
            context_index,
            package,
            reason,
        }
    }

    pub(in crate::typer) fn resolve_qualifier_symbol(
        &mut self,
        tree: TreeId<Untyped>,
        context: SourceContextId,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<Option<SymbolId>, TyperError> {
        self.resolve_qualifier_symbol_with_expression_context(
            tree, context, None, tree_index, position,
        )
    }

    fn resolve_qualifier_symbol_with_expression_context(
        &mut self,
        tree: TreeId<Untyped>,
        context: SourceContextId,
        expression_context: Option<ExpressionContext>,
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
                if self.store.names.resolve(ident.name.text()) == "_root_" {
                    return self.resolve_root_package(context, tree_index).map(Some);
                }
                if let Some(expression_context) = expression_context {
                    match self.expression_term_candidates(
                        ident.name,
                        expression_context,
                        tree_index,
                        position,
                    ) {
                        Ok(candidates) => {
                            self.arbitrate_expression_package_objects(
                                &candidates,
                                expression_context.lexical,
                                ident.name,
                                tree_index,
                                position,
                            )?;
                            return self.unique_symbol_candidate(
                                &candidates,
                                ident.name,
                                tree_index,
                                position,
                            );
                        }
                        Err(TyperError::TermNameNotFound { .. }) => {}
                        Err(error) => return Err(error),
                    }
                }
                if let Some(symbol) =
                    self.lookup_context_symbol(ident.name, context, tree_index, position)?
                {
                    return Ok(Some(symbol));
                }
                self.resolve_relative_import_segment(ident.name, context, tree_index, position)
            }
            TreeKind::Select(select) => {
                let Some(qualifier) = self.resolve_qualifier_symbol_with_expression_context(
                    select.qualifier,
                    context,
                    expression_context,
                    tree_index,
                    position,
                )?
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
                let prefix = self.type_prefix_for_qualifier(qualifier)?;
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

    /// Resolves the first segment of an unrooted import qualifier. The actual
    /// current package is probed first, followed by genuinely enclosing source
    /// package contexts, then the canonical root. A dotted package path does
    /// not create source contexts for its textual parent segments.
    fn resolve_relative_import_segment(
        &mut self,
        name: dotty_core::Name,
        context: SourceContextId,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<Option<SymbolId>, TyperError> {
        let candidates =
            self.lexical_package_candidates(context, LexicalPackageSearch::Relative, tree_index)?;
        let mut has_canonical_root = false;
        for candidate in candidates.into_iter().filter(|candidate| {
            matches!(
                candidate.origin,
                LexicalPackageCandidateOrigin::CurrentPackage
                    | LexicalPackageCandidateOrigin::EnclosingPackage
                    | LexicalPackageCandidateOrigin::Root
            )
        }) {
            has_canonical_root |= candidate.origin == LexicalPackageCandidateOrigin::Root;
            let scoped = self.scoped_symbols(candidate.scope, name);
            let source_symbols: Vec<_> = scoped
                .iter()
                .copied()
                .filter(|symbol| {
                    matches!(
                        self.store.symbols.get(*symbol).origin,
                        dotty_core::SymbolOrigin::Source(_)
                    )
                })
                .collect();
            if !source_symbols.is_empty() {
                return self.unique_symbol_candidate(&source_symbols, name, tree_index, position);
            }

            let external_symbols: Vec<_> = scoped
                .iter()
                .copied()
                .filter(|symbol| {
                    !matches!(
                        self.store.symbols.get(*symbol).origin,
                        dotty_core::SymbolOrigin::Source(_)
                    )
                })
                .collect();
            let cached_object = external_symbols.iter().copied().find(|symbol| {
                let entry = self.store.symbols.get(*symbol);
                entry.kind == SymbolKind::Object && entry.owner == Some(candidate.package)
            });
            let cached_package = external_symbols.iter().copied().find(|symbol| {
                let entry = self.store.symbols.get(*symbol);
                entry.kind == SymbolKind::Package && entry.owner == Some(candidate.package)
            });

            let store_checkpoint = self.store.checkpoint();
            let resolver_checkpoint = self.resolver.checkpoint();
            let mut path = candidate.package_path.clone();
            path.push(self.store.names.resolve(name.text()).to_owned());

            if let Some(object) = cached_object {
                let package = match self.resolve_external_package(&path, tree_index) {
                    Ok(package) => package,
                    Err(TyperError::SymbolResolution {
                        error: ResolutionError::Ambiguous { .. },
                        ..
                    }) => {
                        self.resolver.rollback_to(self.store, resolver_checkpoint);
                        self.store.rollback_to(store_checkpoint);
                        return Err(TyperError::AmbiguousTypeName {
                            source: self.source,
                            tree_index,
                            name,
                            position,
                        });
                    }
                    Err(error) => {
                        self.resolver.rollback_to(self.store, resolver_checkpoint);
                        self.store.rollback_to(store_checkpoint);
                        return Err(error);
                    }
                };
                if package.is_some() {
                    self.resolver.rollback_to(self.store, resolver_checkpoint);
                    self.store.rollback_to(store_checkpoint);
                    return Err(TyperError::AmbiguousTypeName {
                        source: self.source,
                        tree_index,
                        name,
                        position,
                    });
                }
                self.validate_package_object_identity(object, candidate.package, tree_index)?;
                return Ok(Some(object));
            }

            if let Some(package) = cached_package {
                let collision = match self.resolver.package_object_collision(
                    self.store,
                    candidate.package,
                    name,
                ) {
                    Ok(collision) => collision,
                    Err(ResolutionError::Ambiguous { .. }) => {
                        self.resolver.rollback_to(self.store, resolver_checkpoint);
                        self.store.rollback_to(store_checkpoint);
                        return Err(TyperError::AmbiguousTypeName {
                            source: self.source,
                            tree_index,
                            name,
                            position,
                        });
                    }
                    Err(error) => {
                        self.resolver.rollback_to(self.store, resolver_checkpoint);
                        self.store.rollback_to(store_checkpoint);
                        return Err(TyperError::SymbolResolution {
                            source: self.source,
                            tree_index,
                            error,
                        });
                    }
                };
                if collision {
                    self.resolver.rollback_to(self.store, resolver_checkpoint);
                    self.store.rollback_to(store_checkpoint);
                    return Err(TyperError::AmbiguousTypeName {
                        source: self.source,
                        tree_index,
                        name,
                        position,
                    });
                }
                return Ok(Some(package));
            }

            // Existing external class/type identities already answer this
            // qualifier. Avoid repeating classpath package and Object probes;
            // only cached Objects need collision arbitration above.
            if !external_symbols.is_empty() {
                return self.unique_symbol_candidate(&external_symbols, name, tree_index, position);
            }

            let package = match self.resolve_external_package(&path, tree_index) {
                Ok(package) => package,
                Err(error) => {
                    self.resolver.rollback_to(self.store, resolver_checkpoint);
                    self.store.rollback_to(store_checkpoint);
                    return Err(error);
                }
            };
            // Resolve the package first to preserve its established precedence.
            // The classpath resolver arbitrates a package child against TASTy
            // Object evidence without changing ordinary package-term lookup.
            let object = if package.is_some() {
                match self
                    .resolver
                    .package_object_collision(self.store, candidate.package, name)
                {
                    Ok(true) | Err(ResolutionError::Ambiguous { .. }) => {
                        self.resolver.rollback_to(self.store, resolver_checkpoint);
                        self.store.rollback_to(store_checkpoint);
                        return Err(TyperError::AmbiguousTypeName {
                            source: self.source,
                            tree_index,
                            name,
                            position,
                        });
                    }
                    Ok(false) => None,
                    Err(error) => {
                        self.resolver.rollback_to(self.store, resolver_checkpoint);
                        self.store.rollback_to(store_checkpoint);
                        return Err(TyperError::SymbolResolution {
                            source: self.source,
                            tree_index,
                            error,
                        });
                    }
                }
            } else {
                match self.resolve_external_package_object(candidate.package, name, tree_index) {
                    Ok(object) => object,
                    Err(error) => {
                        self.resolver.rollback_to(self.store, resolver_checkpoint);
                        self.store.rollback_to(store_checkpoint);
                        return Err(error);
                    }
                }
            };
            match (package, object) {
                (Some(_), Some(_)) => {
                    self.resolver.rollback_to(self.store, resolver_checkpoint);
                    self.store.rollback_to(store_checkpoint);
                    return Err(TyperError::AmbiguousTypeName {
                        source: self.source,
                        tree_index,
                        name,
                        position,
                    });
                }
                (Some(package), None) => return Ok(Some(package)),
                (None, Some(object)) => return Ok(Some(object)),
                (None, None) => {
                    if let Some(symbol) =
                        self.unique_symbol_candidate(&external_symbols, name, tree_index, position)?
                    {
                        return Ok(Some(symbol));
                    }
                }
            }
        }
        if !has_canonical_root {
            let root_qualified = [
                "_root_".to_owned(),
                self.store.names.resolve(name.text()).to_owned(),
            ];
            return self.resolve_external_package(&root_qualified, tree_index);
        }
        Ok(None)
    }

    /// Validates that a resolved Object and its ModuleClass share the expected
    /// package owner.
    fn validate_package_object_identity(
        &self,
        object: SymbolId,
        package: SymbolId,
        tree_index: u32,
    ) -> Result<(), TyperError> {
        let invalid = |reason: &'static str| TyperError::SymbolResolution {
            source: self.source,
            tree_index,
            error: ResolutionError::Malformed {
                reason: reason.to_owned(),
            },
        };
        if !self.store.symbols.contains(object) {
            return Err(TyperError::UnknownSymbol { symbol: object });
        }
        let entry = self.store.symbols.get(object);
        if entry.kind != SymbolKind::Object || entry.owner != Some(package) {
            return Err(invalid(
                "package-term resolution returned a non-object or an object with the wrong owner",
            ));
        }
        let module_class = self
            .resolver
            .module_class_of_object(self.store, object)
            .map_err(|error| TyperError::SymbolResolution {
                source: self.source,
                tree_index,
                error,
            })?
            .ok_or_else(|| invalid("package Object has no validated ModuleClass identity"))?;
        if !self.store.symbols.contains(module_class) {
            return Err(TyperError::UnknownSymbol {
                symbol: module_class,
            });
        }
        let module = self.store.symbols.get(module_class);
        if module.kind != SymbolKind::ModuleClass || module.owner != Some(package) {
            return Err(invalid(
                "package Object's validated ModuleClass has the wrong kind or owner",
            ));
        }
        Ok(())
    }

    fn arbitrate_expression_package_objects(
        &mut self,
        candidates: &[SymbolId],
        lexical: SourceContextId,
        name: dotty_core::Name,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<(), TyperError> {
        let lexical_packages =
            self.lexical_package_candidates(lexical, LexicalPackageSearch::Relative, tree_index)?;
        for &object in candidates {
            if !self.store.symbols.contains(object) {
                continue;
            }
            let entry = self.store.symbols.get(object);
            let Some(package) = entry.owner else {
                continue;
            };
            // Only arbitrate a candidate discovered under its own spelling
            // in the corresponding lexical package scope. Expression lookup
            // can also return explicit import aliases; combining their target
            // owner with the alias spelling would inspect an unrelated path.
            if self.store.names.resolve(entry.name.text()) != self.store.names.resolve(name.text())
                || !lexical_packages.iter().any(|candidate| {
                    candidate.package == package
                        && self.scoped_symbols(candidate.scope, name).contains(&object)
                })
            {
                continue;
            }
            let external_candidate = match entry.kind {
                SymbolKind::Object => matches!(
                    entry.origin,
                    dotty_core::SymbolOrigin::Classfile(_) | dotty_core::SymbolOrigin::Tasty(_)
                ),
                // Classpath packages are entered by the resolver's synthetic
                // package registry; source packages retain Source origin.
                SymbolKind::Package => matches!(
                    entry.origin,
                    dotty_core::SymbolOrigin::Synthetic
                        | dotty_core::SymbolOrigin::Classfile(_)
                        | dotty_core::SymbolOrigin::Tasty(_)
                ),
                _ => false,
            };
            if !external_candidate
                || !self.store.symbols.contains(package)
                || self.store.symbols.get(package).kind != SymbolKind::Package
            {
                continue;
            }

            let store_checkpoint = self.store.checkpoint();
            let resolver_checkpoint = self.resolver.checkpoint();
            let mut path = self.package_path(package);
            path.push(self.store.names.resolve(name.text()).to_owned());
            let package_candidate = self.resolve_external_package(&path, tree_index);
            let collision = match package_candidate {
                Ok(Some(_)) => self
                    .resolver
                    .package_object_collision(self.store, package, name),
                Ok(None) => Ok(false),
                Err(TyperError::SymbolResolution { error, .. }) => Err(error),
                Err(error) => {
                    self.resolver.rollback_to(self.store, resolver_checkpoint);
                    self.store.rollback_to(store_checkpoint);
                    return Err(error);
                }
            };
            match collision {
                Ok(true) | Err(ResolutionError::Ambiguous { .. }) => {
                    self.resolver.rollback_to(self.store, resolver_checkpoint);
                    self.store.rollback_to(store_checkpoint);
                    return Err(TyperError::AmbiguousTypeName {
                        source: self.source,
                        tree_index,
                        name,
                        position,
                    });
                }
                Ok(false) => {
                    self.resolver.rollback_to(self.store, resolver_checkpoint);
                    self.store.rollback_to(store_checkpoint);
                }
                Err(error) => {
                    self.resolver.rollback_to(self.store, resolver_checkpoint);
                    self.store.rollback_to(store_checkpoint);
                    return Err(TyperError::SymbolResolution {
                        source: self.source,
                        tree_index,
                        error,
                    });
                }
            }
        }
        Ok(())
    }

    /// Looks up a package-owned Object for a relative import segment.
    fn resolve_external_package_object(
        &mut self,
        package: SymbolId,
        name: dotty_core::Name,
        tree_index: u32,
    ) -> Result<Option<SymbolId>, TyperError> {
        let store_checkpoint = self.store.checkpoint();
        let resolver_checkpoint = self.resolver.checkpoint();
        let prefix = self.package_type_prefix(package);
        let request = MemberRequest {
            prefix,
            name: dotty_core::Name::new(name.text(), dotty_core::Namespace::Term),
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };
        let resolved = self.resolver.resolve_member(self.store, &request);
        let object = match resolved {
            Ok(Some(object)) => object,
            Ok(None) => {
                self.resolver.rollback_to(self.store, resolver_checkpoint);
                self.store.rollback_to(store_checkpoint);
                return Ok(None);
            }
            Err(error) => {
                self.resolver.rollback_to(self.store, resolver_checkpoint);
                self.store.rollback_to(store_checkpoint);
                return Err(TyperError::SymbolResolution {
                    source: self.source,
                    tree_index,
                    error,
                });
            }
        };

        let invalid = |reason: &'static str| TyperError::SymbolResolution {
            source: self.source,
            tree_index,
            error: ResolutionError::Malformed {
                reason: reason.to_owned(),
            },
        };
        if self.store.symbols.contains(object)
            && matches!(
                self.store.symbols.get(object).kind,
                SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
            )
        {
            self.resolver.rollback_to(self.store, resolver_checkpoint);
            self.store.rollback_to(store_checkpoint);
            return Ok(None);
        }
        let validation = (|| {
            if !self.store.symbols.contains(object) {
                return Err(TyperError::UnknownSymbol { symbol: object });
            }
            let entry = self.store.symbols.get(object);
            if entry.kind != SymbolKind::Object || entry.owner != Some(package) {
                return Err(invalid(
                    "package-term resolution returned a non-object or an object with the wrong owner",
                ));
            }
            let module_class = self
                .resolver
                .module_class_of_object(self.store, object)
                .map_err(|error| TyperError::SymbolResolution {
                    source: self.source,
                    tree_index,
                    error,
                })?
                .ok_or_else(|| invalid("package Object has no validated ModuleClass identity"))?;
            if !self.store.symbols.contains(module_class) {
                return Err(TyperError::UnknownSymbol {
                    symbol: module_class,
                });
            }
            let module = self.store.symbols.get(module_class);
            if module.kind != SymbolKind::ModuleClass || module.owner != Some(package) {
                return Err(invalid(
                    "package Object's validated ModuleClass has the wrong kind or owner",
                ));
            }
            Ok(())
        })();
        if let Err(error) = validation {
            self.resolver.rollback_to(self.store, resolver_checkpoint);
            self.store.rollback_to(store_checkpoint);
            return Err(error);
        }
        Ok(Some(object))
    }

    fn resolve_root_package(
        &self,
        context: SourceContextId,
        tree_index: u32,
    ) -> Result<SymbolId, TyperError> {
        self.lexical_package_candidates(context, LexicalPackageSearch::RootQualified, tree_index)?
            .into_iter()
            .find(|candidate| candidate.origin == LexicalPackageCandidateOrigin::Root)
            .map(|candidate| candidate.package)
            .ok_or_else(|| {
                self.invalid_package_candidate(
                    tree_index,
                    Some(context.index()),
                    None,
                    "canonical root package is absent from the package registry",
                )
            })
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
        let cache_key =
            source_import
                .expression_context
                .is_none()
                .then_some(ImportQualifierCacheKey {
                    import_tree: source_import.tree.index(),
                    qualifier_tree: import.expr.index(),
                    source_context: source_import.context.index(),
                    qualifier_context: qualifier_context.index(),
                    location_tree: location.tree_index,
                    position: location.position,
                });
        let cached_qualifier =
            cache_key.and_then(|key| self.import_qualifier_cache.get(&key).copied());
        let qualifier_result = if let Some(qualifier) = cached_qualifier {
            Some(qualifier)
        } else if let Some(expression_context) = source_import.expression_context {
            let saved_imports = source_import
                .local_import_scope
                .zip(source_import.preceding_local_imports.clone())
                .map(|(scope, preceding)| {
                    let frame = self
                        .expression_scopes
                        .get_mut(scope.index())
                        .ok_or(TyperError::ExpressionLocalScopeStackMissing { stack: scope })?;
                    Ok::<_, TyperError>(std::mem::replace(&mut frame.imports, preceding))
                })
                .transpose()?;
            let result = self.resolve_qualifier_symbol_with_expression_context(
                import.expr,
                qualifier_context,
                Some(expression_context),
                source_import.tree.index(),
                location.position,
            );
            if let (Some(scope), Some(saved_imports)) =
                (source_import.local_import_scope, saved_imports)
            {
                let frame = self
                    .expression_scopes
                    .get_mut(scope.index())
                    .ok_or(TyperError::ExpressionLocalScopeStackMissing { stack: scope })?;
                frame.imports = saved_imports;
            }
            result?
        } else {
            self.import_qualifier_symbol(
                import.expr,
                qualifier_context,
                source_import.tree.index(),
                location.position,
            )?
        };
        if let (Some(key), Some(qualifier)) = (cache_key, qualifier_result) {
            self.import_qualifier_cache.insert(key, qualifier);
        }
        let Some(qualifier) = qualifier_result else {
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
                    // Import selectors carry a namespace. Falling back to a
                    // same-spelled declaration in the other namespace can
                    // turn a class-only import into a term reference (or an
                    // Object import into a type reference).
                    let candidates = self.store.scopes.get(*scope).lookup_all(&name);
                    found_locally |= !candidates.is_empty();
                    matches.extend_from_slice(candidates);
                }
            }
            if !found_locally {
                let cache_key = ImportMemberCacheKey { qualifier, name };
                if let Some(symbol) = self.import_member_cache.get(&cache_key).copied() {
                    matches.push(symbol);
                    continue;
                }
                let prefix = self.type_prefix_for_qualifier(qualifier)?;
                let request = MemberRequest {
                    prefix,
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
                    self.import_member_cache.insert(cache_key, symbol);
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
            .resolve_qualifier_symbol_with_expression_context(
                tree,
                context.lexical,
                Some(context),
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
            TreeKind::Ident(ident) => {
                TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
                    .ident_with_backquoted(ident.name, ident.backquoted, ty, source_node.position)
            }
            TreeKind::Select(selection) => {
                let qualifier = self.type_import_qualifier(
                    selection.qualifier,
                    context,
                    import_tree_index,
                    info_journal,
                    new_mappings,
                )?;
                TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).select(
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
            TreeKind::Ident(ident) => {
                TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
                    .ident_with_backquoted(ident.name, ident.backquoted, ty, source_node.position)
            }
            TreeKind::TypeTree(_) => {
                let ty = self.type_of_tpt_inner_journaled(tree, context, info_journal)?;
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
            if self.store.symbols.get(source_context.owner).kind == SymbolKind::Package {
                continue;
            }
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
            let term_candidates = self.lookup_import_candidates(
                &contexts,
                name,
                false,
                selection,
                SourceTreeLocation {
                    tree_index: import_tree_index,
                    position,
                },
            )?;
            if !term_candidates.is_empty() {
                return self.unique_symbol_candidate(
                    &term_candidates,
                    name,
                    import_tree_index,
                    position,
                );
            }
            // Qualifiers may name a class or type alias when no term with that
            // spelling exists. Keep this fallback here, where a symbol is
            // being used as a qualifier, rather than in ordinary term import
            // lookup.
            let type_name = dotty_core::Name::new(name.text(), dotty_core::Namespace::Type);
            let type_candidates = self.lookup_import_candidates(
                &contexts,
                type_name,
                true,
                selection,
                SourceTreeLocation {
                    tree_index: import_tree_index,
                    position,
                },
            )?;
            if !type_candidates.is_empty() {
                return self.unique_type_candidate(
                    &type_candidates,
                    type_name,
                    import_tree_index,
                    position,
                );
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
                expression_context: None,
                local_import_scope: None,
                preceding_local_imports: None,
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
        imports: &[(TreeId<Untyped>, ExpressionContext)],
        import_scope: ExpressionScopeId,
        name: dotty_core::Name,
        type_only: bool,
        selection: ImportSelection,
        location: SourceTreeLocation,
    ) -> Result<Vec<SymbolId>, TyperError> {
        let mut candidates = Vec::new();
        for (import_index, (tree, expression_context)) in imports.iter().enumerate() {
            let context = expression_context.lexical;
            let source_context =
                self.index
                    .try_source_context(context)
                    .ok_or(TyperError::SourceContextMissing {
                        source: self.source,
                        tree_index: location.tree_index,
                        context_index: context.index(),
                    })?;
            candidates.extend(self.lookup_imported_symbols(
                SourceImport {
                    tree: *tree,
                    context,
                    parent: source_context.parent,
                    expression_context: Some(*expression_context),
                    local_import_scope: Some(import_scope),
                    preceding_local_imports: Some(imports[..import_index].to_vec()),
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
        self.scopes_of_with_seen(symbol, &mut Vec::new())
    }

    fn scopes_of_with_seen(
        &self,
        symbol: SymbolId,
        seen: &mut Vec<SymbolId>,
    ) -> Vec<dotty_core::ScopeId> {
        if seen.contains(&symbol) {
            return Vec::new();
        }
        seen.push(symbol);
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
        if let SymbolInfo::Complete(ty) = self.store.symbols.get(symbol).info {
            match self.store.types.get(ty) {
                Type::ClassInfo(info) => insert_scope(&mut scopes, info.declarations),
                Type::TermRef {
                    target: TermRefTarget::Symbol(target),
                    ..
                }
                | Type::TypeRef {
                    target: TypeRefTarget::Symbol(target),
                    ..
                } => {
                    for scope in self.scopes_of_with_seen(*target, seen) {
                        insert_scope(&mut scopes, scope);
                    }
                }
                _ => {}
            }
        }
        let semantic = self.store.symbols.get(symbol);
        if semantic.kind != SymbolKind::Object {
            return scopes;
        }
        let Some(owner) = semantic.owner else {
            return scopes;
        };
        if self.index.definition_of(symbol).is_none() {
            if let Some(module_class) = self
                .resolver
                .module_class_of_object(self.store, symbol)
                .ok()
                .flatten()
                && let SymbolInfo::Complete(info) = self.store.symbols.get(module_class).info
                && let Some(Type::ClassInfo(info)) = self.store.types.try_get(info)
                && info.class == module_class
                && self.store.scopes.contains(info.declarations)
                && self.store.scopes.get(info.declarations).owner == Some(module_class)
            {
                insert_scope(&mut scopes, info.declarations);
            }
            return scopes;
        }
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
