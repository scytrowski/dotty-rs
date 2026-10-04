//! Projection of source type syntax into semantic types.

use super::*;

pub(super) const MAX_SOURCE_TYPE_PROJECTION_DEPTH: usize = 256;

/// Resolves source-written type names without making the type-tree dispatcher
/// depend on the details of lexical scopes, imports, or semantic lookups.
struct SourceNameResolver<'typer, 'store> {
    typer: &'typer mut SourceTyper<'store>,
}

impl SourceNameResolver<'_, '_> {
    fn resolve_type_name(
        &mut self,
        name: dotty_core::Name,
        context: SourceContextId,
        location: SourceTreeLocation,
    ) -> Result<TypeId, TyperError> {
        let target =
            self.typer
                .lookup_type_symbol(name, context, location.tree_index, location.position)?;
        if let Some(target) = target {
            let prefix = self.typer.type_symbol_prefix(target);
            return Ok(self.typer.store.types.alloc(Type::TypeRef {
                prefix,
                target: TypeRefTarget::Symbol(target),
            }));
        }
        if let Some(builtin) = self
            .typer
            .definitions
            .source_builtin_type(self.typer.store, name)
        {
            return Ok(builtin);
        }
        if let Some(symbol) = self.typer.lookup_term_candidate_for_type_name(
            name,
            context,
            location.tree_index,
            location.position,
        )? {
            return Err(TyperError::WrongTypeNameKind {
                source: self.typer.source,
                tree_index: location.tree_index,
                name,
                symbol,
                kind: self.typer.store.symbols.get(symbol).kind,
                position: location.position,
            });
        }
        Err(TyperError::TypeNameNotFound {
            source: self.typer.source,
            tree_index: location.tree_index,
            name,
            position: location.position,
        })
    }

    fn resolve_type_member(
        &mut self,
        qualifier_tree: TreeId<Untyped>,
        name: dotty_core::Name,
        context: SourceContextId,
        location: SourceTreeLocation,
    ) -> Result<TypeId, TyperError> {
        self.typer.type_of_selected_tpt(
            qualifier_tree,
            name,
            context,
            location.tree_index,
            location.position,
        )
    }
}

impl SourceTyper<'_> {
    /// Projects a source type tree using its indexed declaration context.
    pub fn type_of_tpt(
        &mut self,
        tree: TreeId<Untyped>,
        context: SourceContextId,
    ) -> Result<TypeId, TyperError> {
        self.run_atomic(|typer, info_journal| {
            typer.type_of_tpt_inner_journaled(tree, context, info_journal)
        })
    }

    pub(super) fn type_of_tpt_inner_journaled(
        &mut self,
        tree: TreeId<Untyped>,
        context: SourceContextId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        if let Some(ty) = self.type_index.type_at(self.source, tree) {
            return Ok(ty);
        }
        if self.source_type_projection_depth >= MAX_SOURCE_TYPE_PROJECTION_DEPTH {
            return Err(TyperError::SourceTypeProjectionDepthExceeded {
                source: self.source,
                tree_index: tree.index(),
                max_depth: MAX_SOURCE_TYPE_PROJECTION_DEPTH,
            });
        }
        self.source_type_projection_depth += 1;
        let result = self.type_of_tpt_uncached(tree, context, info_journal);
        self.source_type_projection_depth -= 1;
        result
    }

    fn type_of_tpt_uncached(
        &mut self,
        tree: TreeId<Untyped>,
        context: SourceContextId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        if let Some(ty) = self.type_index.type_at(self.source, tree) {
            return Ok(ty);
        }
        let Some(source_tree) = self.arena.try_get(tree) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: tree.index(),
            });
        };
        if matches!(&source_tree.kind, TreeKind::TypeTree(_)) {
            return Err(TyperError::MissingDeclaredType {
                source: self.source,
                tree_index: tree.index(),
                position: source_tree.position,
            });
        }
        if let TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) = &source_tree.kind {
            let ty = self.type_of_tpt_inner_journaled(parens.inner, context, info_journal)?;
            if let Err(existing) = self.type_index.insert(self.source, tree, ty) {
                return Err(TyperError::DuplicateSourceTypeCacheEntry {
                    source: self.source,
                    tree_index: tree.index(),
                    existing,
                    attempted: ty,
                });
            }
            return Ok(ty);
        }
        if let TreeKind::ByNameTypeTree(by_name) = &source_tree.kind {
            let result = self.type_of_tpt_inner_journaled(by_name.result, context, info_journal)?;
            let ty = self.store.types.alloc(Type::ByName { result });
            if let Err(existing) = self.type_index.insert(self.source, tree, ty) {
                return Err(TyperError::DuplicateSourceTypeCacheEntry {
                    source: self.source,
                    tree_index: tree.index(),
                    existing,
                    attempted: ty,
                });
            }
            return Ok(ty);
        }
        if let TreeKind::SingletonTypeTree(singleton) = &source_tree.kind {
            let ty = self.project_stable_term_reference(
                singleton.reference,
                context,
                tree.index(),
                info_journal,
            )?;
            self.require_stable_selection_prefix(ty, tree.index())?;
            if let Err(existing) = self.type_index.insert(self.source, tree, ty) {
                return Err(TyperError::DuplicateSourceTypeCacheEntry {
                    source: self.source,
                    tree_index: tree.index(),
                    existing,
                    attempted: ty,
                });
            }
            return Ok(ty);
        }
        if let TreeKind::AppliedTypeTree(applied) = &source_tree.kind {
            let tycon = self.type_of_tpt_inner_journaled(applied.tpt, context, info_journal)?;
            let mut args = Vec::with_capacity(applied.args.len());
            for argument in &applied.args {
                let is_wildcard = self
                    .arena
                    .try_get(*argument)
                    .is_some_and(|argument| matches!(argument.kind, TreeKind::TypeBoundsTree(_)));
                let projected = if is_wildcard {
                    self.type_of_wildcard_bounds(*argument, context, info_journal)?
                } else {
                    self.type_of_tpt_inner_journaled(*argument, context, info_journal)?
                };
                args.push(projected);
            }
            let ty = self.store.types.alloc(Type::Applied { tycon, args });
            if let Err(existing) = self.type_index.insert(self.source, tree, ty) {
                return Err(TyperError::DuplicateSourceTypeCacheEntry {
                    source: self.source,
                    tree_index: tree.index(),
                    existing,
                    attempted: ty,
                });
            }
            return Ok(ty);
        }
        if let TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) = &source_tree.kind {
            let operator = self.store.names.resolve(infix.op.text());
            let is_union = match operator {
                "|" => true,
                "&" => false,
                _ => {
                    return Err(TyperError::UnsupportedTypeTree {
                        source: self.source,
                        tree_index: tree.index(),
                        tree_kind: "infix type operator",
                    });
                }
            };
            let left = self.type_of_tpt_inner_journaled(infix.left, context, info_journal)?;
            let right = self.type_of_tpt_inner_journaled(infix.right, context, info_journal)?;
            let ty = if is_union {
                self.store.types.alloc(Type::Or { left, right })
            } else {
                self.store.types.alloc(Type::And { left, right })
            };
            if let Err(existing) = self.type_index.insert(self.source, tree, ty) {
                return Err(TyperError::DuplicateSourceTypeCacheEntry {
                    source: self.source,
                    tree_index: tree.index(),
                    existing,
                    attempted: ty,
                });
            }
            return Ok(ty);
        }
        if let TreeKind::Select(select) = &source_tree.kind {
            let ty = SourceNameResolver { typer: self }.resolve_type_member(
                select.qualifier,
                select.name,
                context,
                SourceTreeLocation {
                    tree_index: tree.index(),
                    position: source_tree.position,
                },
            )?;
            if let Err(existing) = self.type_index.insert(self.source, tree, ty) {
                return Err(TyperError::DuplicateSourceTypeCacheEntry {
                    source: self.source,
                    tree_index: tree.index(),
                    existing,
                    attempted: ty,
                });
            }
            return Ok(ty);
        }
        let TreeKind::Ident(Ident { name, .. }) = &source_tree.kind else {
            return Err(TyperError::UnsupportedTypeTree {
                source: self.source,
                tree_index: tree.index(),
                tree_kind: tree_kind_name(&source_tree.kind),
            });
        };
        if !name.is_type() {
            return Err(TyperError::UnsupportedTypeTree {
                source: self.source,
                tree_index: tree.index(),
                tree_kind: "term identifier",
            });
        }
        if self.index.try_source_context(context).is_none() {
            return Err(TyperError::SourceContextMissing {
                source: self.source,
                tree_index: tree.index(),
                context_index: context.index(),
            });
        }
        let position = source_tree.position;
        let ty = SourceNameResolver { typer: self }.resolve_type_name(
            *name,
            context,
            SourceTreeLocation {
                tree_index: tree.index(),
                position,
            },
        )?;
        if let Err(existing) = self.type_index.insert(self.source, tree, ty) {
            return Err(TyperError::DuplicateSourceTypeCacheEntry {
                source: self.source,
                tree_index: tree.index(),
                existing,
                attempted: ty,
            });
        }
        Ok(ty)
    }

    fn type_of_wildcard_bounds(
        &mut self,
        tree: TreeId<Untyped>,
        context: SourceContextId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        if let Some(ty) = self.type_index.type_at(self.source, tree) {
            return Ok(ty);
        }
        let Some(source_tree) = self.arena.try_get(tree) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: tree.index(),
            });
        };
        let TreeKind::TypeBoundsTree(bounds) = &source_tree.kind else {
            return Err(TyperError::UnsupportedTypeTree {
                source: self.source,
                tree_index: tree.index(),
                tree_kind: tree_kind_name(&source_tree.kind),
            });
        };
        if bounds.alias.is_some() {
            return Err(TyperError::UnsupportedTypeTree {
                source: self.source,
                tree_index: tree.index(),
                tree_kind: "aliased wildcard bounds",
            });
        }
        let projected_bounds = self.project_type_bounds(bounds, context, info_journal)?;
        let wildcard = self.store.types.alloc(Type::Wildcard {
            bounds: projected_bounds,
        });
        if let Err(existing) = self.type_index.insert(self.source, tree, wildcard) {
            return Err(TyperError::DuplicateSourceTypeCacheEntry {
                source: self.source,
                tree_index: tree.index(),
                existing,
                attempted: wildcard,
            });
        }
        Ok(wildcard)
    }

    fn project_stable_term_reference(
        &mut self,
        tree: TreeId<Untyped>,
        context: SourceContextId,
        type_tree_index: u32,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        if self.source_type_projection_depth >= MAX_SOURCE_TYPE_PROJECTION_DEPTH {
            return Err(TyperError::SourceTypeProjectionDepthExceeded {
                source: self.source,
                tree_index: tree.index(),
                max_depth: MAX_SOURCE_TYPE_PROJECTION_DEPTH,
            });
        }
        self.source_type_projection_depth += 1;
        let result =
            self.project_stable_term_reference_inner(tree, context, type_tree_index, info_journal);
        self.source_type_projection_depth -= 1;
        result
    }

    fn project_stable_term_reference_inner(
        &mut self,
        tree: TreeId<Untyped>,
        context: SourceContextId,
        type_tree_index: u32,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let source_tree = self
            .arena
            .try_get(tree)
            .ok_or(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: tree.index(),
            })?;
        match source_tree.kind {
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => self
                .project_stable_term_reference(
                    parens.inner,
                    context,
                    type_tree_index,
                    info_journal,
                ),
            TreeKind::This(this) => {
                let owner = self
                    .index
                    .try_source_context(context)
                    .ok_or(TyperError::SourceContextMissing {
                        source: self.source,
                        tree_index: type_tree_index,
                        context_index: context.index(),
                    })?
                    .owner;
                let class = self.enclosing_this_owner(this.qual, owner, tree.index())?;
                Ok(self.store.types.alloc(Type::ThisType { class }))
            }
            TreeKind::Ident(ident) if ident.name.is_term() => {
                let source_context = self.index.try_source_context(context).ok_or(
                    TyperError::SourceContextMissing {
                        source: self.source,
                        tree_index: tree.index(),
                        context_index: context.index(),
                    },
                )?;
                let local_scopes = self
                    .active_local_import_scopes
                    .iter()
                    .rev()
                    .find(|(active_context, _)| *active_context == context)
                    .and_then(|(_, scopes)| *scopes);
                let expression = ExpressionContext {
                    lexical: context,
                    owner: source_context.owner,
                    local_scopes,
                };
                let symbol = self.resolve_expression_term(
                    ident.name,
                    expression,
                    tree.index(),
                    source_tree.position,
                )?;
                self.expression_type_of_symbol(symbol, expression.owner, tree.index(), info_journal)
            }
            TreeKind::Select(selection) if selection.name.is_term() => {
                let qualifier = self.project_stable_term_reference(
                    selection.qualifier,
                    context,
                    type_tree_index,
                    info_journal,
                )?;
                self.require_stable_selection_prefix(qualifier, tree.index())?;
                let receiver = self.widen_expression_type_journaled(qualifier, info_journal, 0)?;
                let receiver = self.this_type_receiver_view(receiver)?;
                let candidates = self
                    .lookup_members_journaled(receiver, selection.name, info_journal)
                    .map_err(|error| TyperError::MemberLookup(Box::new(error)))?;
                let candidate = match candidates.as_slice() {
                    [] => {
                        return Err(TyperError::MemberNotFound {
                            source: self.source,
                            tree_index: tree.index(),
                            receiver,
                            name: selection.name,
                        });
                    }
                    [candidate] => candidate,
                    _ => {
                        return Err(TyperError::OverloadedSelectionDeferred {
                            source: self.source,
                            tree_index: tree.index(),
                            name: selection.name,
                        });
                    }
                };
                if !matches!(
                    self.store.symbols.get(candidate.symbol).kind,
                    SymbolKind::Parameter
                        | SymbolKind::Field
                        | SymbolKind::Value
                        | SymbolKind::Local
                        | SymbolKind::Object
                ) || self
                    .store
                    .symbols
                    .get(candidate.symbol)
                    .flags
                    .contains(SymbolFlags::MUTABLE)
                {
                    return Err(TyperError::UnstableSelectionPrefix {
                        source: self.source,
                        tree_index: tree.index(),
                        qualifier_type: qualifier,
                    });
                }
                let _ = self.completed_expression_symbol_info(candidate.symbol, info_journal)?;
                Ok(self.store.types.alloc(Type::TermRef {
                    prefix: qualifier,
                    target: TermRefTarget::Symbol(candidate.symbol),
                }))
            }
            _ => Err(TyperError::UnsupportedSingletonReference {
                source: self.source,
                tree_index: tree.index(),
                reference_kind: tree_kind_name(&source_tree.kind),
            }),
        }
    }

    pub(super) fn type_symbol_prefix(&mut self, symbol: SymbolId) -> TypeId {
        let Some(owner) = self.store.symbols.get(symbol).owner else {
            return self.definitions.no_prefix;
        };
        match self.store.symbols.get(owner).kind {
            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass => {
                self.store.types.alloc(Type::ThisType { class: owner })
            }
            SymbolKind::Package => self.package_type_prefix(owner),
            _ => self.definitions.no_prefix,
        }
    }

    pub(super) fn package_type_prefix(&mut self, package: SymbolId) -> TypeId {
        self.store.types.alloc(Type::TypeRef {
            prefix: self.definitions.no_prefix,
            target: TypeRefTarget::Symbol(package),
        })
    }

    fn type_of_selected_tpt(
        &mut self,
        qualifier_tree: TreeId<Untyped>,
        name: dotty_core::Name,
        context: SourceContextId,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<TypeId, TyperError> {
        if !name.is_type() {
            return Err(TyperError::UnsupportedTypeTree {
                source: self.source,
                tree_index,
                tree_kind: "term selection",
            });
        }
        let Some(qualifier) =
            self.resolve_qualifier_symbol(qualifier_tree, context, tree_index, position)?
        else {
            return Err(TyperError::TypeNameNotFound {
                source: self.source,
                tree_index,
                name,
                position,
            });
        };
        let scopes = self.scopes_of(qualifier);
        let type_candidates: Vec<_> = scopes
            .iter()
            .flat_map(|scope| {
                self.store
                    .scopes
                    .get(*scope)
                    .lookup_all(&name)
                    .iter()
                    .copied()
            })
            .collect();
        if let Some(symbol) =
            self.unique_type_candidate(&type_candidates, name, tree_index, position)?
        {
            let prefix = self.type_symbol_prefix(symbol);
            return Ok(self.store.types.alloc(Type::TypeRef {
                prefix,
                target: TypeRefTarget::Symbol(symbol),
            }));
        }
        let term_name = dotty_core::Name::new(name.text(), dotty_core::Namespace::Term);
        let term_candidates: Vec<_> = scopes
            .iter()
            .flat_map(|scope| {
                self.store
                    .scopes
                    .get(*scope)
                    .lookup_all(&term_name)
                    .iter()
                    .copied()
            })
            .collect();
        if let Some(symbol) =
            self.unique_symbol_candidate(&term_candidates, name, tree_index, position)?
        {
            return Err(TyperError::WrongTypeNameKind {
                source: self.source,
                tree_index,
                name,
                symbol,
                kind: self.store.symbols.get(symbol).kind,
                position,
            });
        }
        let prefix = self.type_prefix_for_qualifier(qualifier);
        let request = MemberRequest {
            prefix,
            name,
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };
        let external = self
            .resolver
            .resolve_member(self.store, &request)
            .map_err(|error| TyperError::SymbolResolution {
                source: self.source,
                tree_index,
                error,
            })?;
        if let Some(symbol) = external {
            if !self.store.symbols.contains(symbol) {
                return Err(TyperError::UnknownSymbol { symbol });
            }
            if matches!(
                self.store.symbols.get(symbol).kind,
                SymbolKind::Class
                    | SymbolKind::Trait
                    | SymbolKind::ModuleClass
                    | SymbolKind::TypeParameter
                    | SymbolKind::TypeAlias
            ) {
                let prefix = self.type_symbol_prefix(symbol);
                return Ok(self.store.types.alloc(Type::TypeRef {
                    prefix,
                    target: TypeRefTarget::Symbol(symbol),
                }));
            }
            return Err(TyperError::WrongTypeNameKind {
                source: self.source,
                tree_index,
                name,
                symbol,
                kind: self.store.symbols.get(symbol).kind,
                position,
            });
        }
        Err(TyperError::TypeNameNotFound {
            source: self.source,
            tree_index,
            name,
            position,
        })
    }

    pub(in crate::typer) fn type_prefix_for_qualifier(&mut self, symbol: SymbolId) -> TypeId {
        match self.store.symbols.get(symbol).kind {
            SymbolKind::Package => self.package_type_prefix(symbol),
            SymbolKind::Object => {
                let owner = self.store.symbols.get(symbol).owner;
                let module_class = owner
                    .and_then(|owner| self.index.definition_of(symbol).map(|_| owner))
                    .and_then(|owner| {
                        self.index
                            .definition_of(symbol)
                            .and_then(|definition| match definition {
                                SourceDefinition::Canonical { source, tree }
                                | SourceDefinition::Derived { source, tree }
                                    if source == self.source =>
                                {
                                    self.index.derived_symbol_at(owner, source, tree)
                                }
                                _ => None,
                            })
                    });
                if let Some(module_class) = module_class {
                    self.store.types.alloc(Type::ThisType {
                        class: module_class,
                    })
                } else {
                    self.definitions.no_prefix
                }
            }
            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass => {
                let prefix = self.type_symbol_prefix(symbol);
                self.store.types.alloc(Type::TypeRef {
                    prefix,
                    target: TypeRefTarget::Symbol(symbol),
                })
            }
            _ => self.definitions.no_prefix,
        }
    }
}
