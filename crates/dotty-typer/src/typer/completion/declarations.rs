//! Completion of fields, type parameters, aliases, and class metadata.

use super::*;

impl SourceTyper<'_> {
    pub(in crate::typer) fn validate_existing_class_info(
        &self,
        symbol: SymbolId,
        ty: TypeId,
    ) -> Result<(), TyperError> {
        let expected_scope = if self.index.definition_of(symbol).is_some() {
            Some(
                self.index
                    .scope_of(symbol)
                    .ok_or(TyperError::MissingClassScope { symbol })?,
            )
        } else {
            None
        };
        match self.store.types.try_get(ty) {
            Some(Type::ClassInfo(info))
                if info.class == symbol
                    && info.prefix == self.definitions.no_prefix
                    && expected_scope.is_none_or(|scope| info.declarations == scope)
                    && self.store.scopes.contains(info.declarations)
                    && self.store.scopes.get(info.declarations).owner == Some(symbol) =>
            {
                Ok(())
            }
            _ => Err(TyperError::MalformedClassInfo { symbol, info: ty }),
        }
    }

    pub(super) fn complete_class_info(
        &mut self,
        symbol: SymbolId,
        kind: SymbolKind,
        source_tree: TreeId<Untyped>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let source_node = self
            .arena
            .try_get(source_tree)
            .ok_or(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: source_tree.index(),
            })?;
        let template = match (kind, &source_node.kind) {
            (SymbolKind::Class | SymbolKind::Trait, TreeKind::TypeDef(definition)) => {
                let Some(node) = self.arena.try_get(definition.rhs) else {
                    return Err(TyperError::TreeOutsideArena {
                        source: self.source,
                        tree_index: definition.rhs.index(),
                    });
                };
                let TreeKind::Template(template) = &node.kind else {
                    return Err(TyperError::MalformedSourceAst {
                        source: self.source,
                        tree_index: definition.rhs.index(),
                        symbol,
                        kind,
                    });
                };
                template.clone()
            }
            (SymbolKind::ModuleClass, TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module))) => {
                let Some(node) = self.arena.try_get(module.template) else {
                    return Err(TyperError::TreeOutsideArena {
                        source: self.source,
                        tree_index: module.template.index(),
                    });
                };
                let TreeKind::Template(template) = &node.kind else {
                    return Err(TyperError::MalformedSourceAst {
                        source: self.source,
                        tree_index: module.template.index(),
                        symbol,
                        kind,
                    });
                };
                template.clone()
            }
            _ => {
                return Err(TyperError::SymbolSourceKindMismatch {
                    source: self.source,
                    tree_index: source_tree.index(),
                    symbol,
                    kind,
                });
            }
        };

        let class_context = self
            .index
            .declaration_context_of(symbol)
            .ok_or(TyperError::DeclarationContextMissing { symbol })?;
        let constructor_node =
            self.arena
                .try_get(template.constructor)
                .ok_or(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: template.constructor.index(),
                })?;
        let TreeKind::DefDef(constructor) = &constructor_node.kind else {
            return Err(TyperError::MalformedSourceAst {
                source: self.source,
                tree_index: template.constructor.index(),
                symbol,
                kind,
            });
        };

        let mut type_context = class_context;
        for parameter_tree in &constructor.type_params {
            let Some(parameter_node) = self.arena.try_get(*parameter_tree) else {
                return Err(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: parameter_tree.index(),
                });
            };
            if !matches!(parameter_node.kind, TreeKind::TypeDef(_)) {
                return Err(TyperError::MalformedSourceAst {
                    source: self.source,
                    tree_index: parameter_tree.index(),
                    symbol,
                    kind,
                });
            }
            let parameter = self.index.symbol_at(self.source, *parameter_tree).ok_or(
                TyperError::MethodParameterSymbolMissing {
                    method: symbol,
                    parameter_tree_index: parameter_tree.index(),
                },
            )?;
            if self.store.symbols.get(parameter).kind != SymbolKind::TypeParameter {
                return Err(TyperError::MalformedSourceAst {
                    source: self.source,
                    tree_index: parameter_tree.index(),
                    symbol,
                    kind,
                });
            }
            if self.store.symbols.get(parameter).owner != Some(symbol) {
                return Err(TyperError::MalformedSourceAst {
                    source: self.source,
                    tree_index: parameter_tree.index(),
                    symbol,
                    kind,
                });
            }
            if let Some(context) = self.index.declaration_context_of(parameter) {
                type_context = context;
            }
            self.complete_signature_parameter(parameter, info_journal)?;
        }

        let mut parents = Vec::with_capacity(template.parents.len().max(1));
        for parent in &template.parents {
            parents.push(self.project_parent_type(*parent, type_context, 0)?);
        }
        let first_parent_is_trait = match (parents.first(), template.parents.first()) {
            (Some(parent_type), Some(parent_tree)) => {
                self.parent_is_trait(*parent_type, parent_tree.index(), info_journal)?
            }
            _ => false,
        };
        if parents.is_empty() || first_parent_is_trait {
            parents.insert(0, self.definitions.object_type);
        }

        let self_type = match template.self_val {
            Some(self_tree) => {
                let Some(node) = self.arena.try_get(self_tree) else {
                    return Err(TyperError::TreeOutsideArena {
                        source: self.source,
                        tree_index: self_tree.index(),
                    });
                };
                let TreeKind::ValDef(self_definition) = &node.kind else {
                    return Err(TyperError::MalformedSourceAst {
                        source: self.source,
                        tree_index: self_tree.index(),
                        symbol,
                        kind,
                    });
                };
                let Some(type_tree) = self.arena.try_get(self_definition.tpt) else {
                    return Err(TyperError::TreeOutsideArena {
                        source: self.source,
                        tree_index: self_definition.tpt.index(),
                    });
                };
                if matches!(type_tree.kind, TreeKind::TypeTree(_)) {
                    // `self =>` has a source binder but imposes no additional
                    // self-type constraint. The parser represents its absent
                    // type with a synthetic TypeTree; ClassInfo encodes this
                    // default as `None`.
                    None
                } else {
                    Some(self.type_of_tpt_inner(self_definition.tpt, type_context)?)
                }
            }
            None => None,
        };

        let declarations = self
            .index
            .scope_of(symbol)
            .ok_or(TyperError::MissingClassScope { symbol })?;
        let info = self.store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: self.definitions.no_prefix,
            class: symbol,
            parents,
            declarations,
            self_type,
        }));
        let previous = *self.store.symbols.info(symbol);
        info_journal.push((symbol, previous));
        self.store
            .symbols
            .set_info(symbol, SymbolInfo::Complete(info));
        Ok(info)
    }

    pub(in crate::typer) fn project_parent_type(
        &mut self,
        tree: TreeId<Untyped>,
        context: SourceContextId,
        depth: usize,
    ) -> Result<TypeId, TyperError> {
        if depth > 256 {
            return Err(TyperError::MalformedClassParent {
                source: self.source,
                tree_index: tree.index(),
            });
        }
        let Some(node) = self.arena.try_get(tree) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: tree.index(),
            });
        };
        match &node.kind {
            TreeKind::Apply(application) => {
                self.project_parent_type(application.function, context, depth + 1)
            }
            TreeKind::Block(block) => self.project_parent_type(block.expr, context, depth + 1),
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => {
                self.project_parent_type(parens.inner, context, depth + 1)
            }
            TreeKind::New(new) => self.type_of_tpt_inner(new.tpt, context),
            TreeKind::TypeApply(application) => {
                let repeated_arguments =
                    self.parent_constructor_has_applied_tpt(application.function, depth + 1);
                let tycon = self.project_parent_type(application.function, context, depth + 1)?;
                if repeated_arguments {
                    return Ok(tycon);
                }
                let mut args = Vec::with_capacity(application.args.len());
                for argument in &application.args {
                    args.push(self.type_of_tpt_inner(*argument, context)?);
                }
                Ok(self.store.types.alloc(Type::Applied { tycon, args }))
            }
            TreeKind::Select(selection)
                if self.store.names.resolve(selection.name.text()) == "<init>" =>
            {
                let Some(qualifier) = self.arena.try_get(selection.qualifier) else {
                    return Err(TyperError::TreeOutsideArena {
                        source: self.source,
                        tree_index: selection.qualifier.index(),
                    });
                };
                let TreeKind::New(new) = &qualifier.kind else {
                    return Err(TyperError::MalformedClassParent {
                        source: self.source,
                        tree_index: tree.index(),
                    });
                };
                self.type_of_tpt_inner(new.tpt, context)
            }
            _ => self.type_of_tpt_inner(tree, context),
        }
    }

    fn parent_constructor_has_applied_tpt(&self, tree: TreeId<Untyped>, depth: usize) -> bool {
        if depth > 256 {
            return false;
        }
        let Some(node) = self.arena.try_get(tree) else {
            return false;
        };
        match &node.kind {
            TreeKind::Apply(application) => {
                self.parent_constructor_has_applied_tpt(application.function, depth + 1)
            }
            TreeKind::Block(block) => {
                self.parent_constructor_has_applied_tpt(block.expr, depth + 1)
            }
            TreeKind::TypeApply(application) => {
                self.parent_constructor_has_applied_tpt(application.function, depth + 1)
            }
            TreeKind::Select(selection)
                if self.store.names.resolve(selection.name.text()) == "<init>" =>
            {
                let Some(qualifier) = self.arena.try_get(selection.qualifier) else {
                    return false;
                };
                let TreeKind::New(new) = &qualifier.kind else {
                    return false;
                };
                self.is_applied_type_tree(new.tpt, depth + 1)
            }
            TreeKind::New(new) => self.is_applied_type_tree(new.tpt, depth + 1),
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => {
                self.parent_constructor_has_applied_tpt(parens.inner, depth + 1)
            }
            _ => false,
        }
    }

    fn is_applied_type_tree(&self, tree: TreeId<Untyped>, depth: usize) -> bool {
        if depth > 256 {
            return false;
        }
        let Some(node) = self.arena.try_get(tree) else {
            return false;
        };
        match &node.kind {
            TreeKind::AppliedTypeTree(_) => true,
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => {
                self.is_applied_type_tree(parens.inner, depth + 1)
            }
            _ => false,
        }
    }

    fn parent_type_symbol(&self, ty: TypeId) -> Option<SymbolId> {
        match self.store.types.get(ty) {
            Type::TypeRef { target, .. } => target.symbol(),
            Type::Applied { tycon, .. } => self.parent_type_symbol(*tycon),
            _ => None,
        }
    }

    fn parent_is_trait(
        &mut self,
        ty: TypeId,
        tree_index: u32,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<bool, TyperError> {
        let mut current_type = ty;
        let mut visited = Vec::new();
        for _ in 0..256 {
            let symbol = self.parent_type_symbol(current_type);
            let Some(symbol) = symbol else {
                return Err(TyperError::UnresolvedParentClassKind {
                    source: self.source,
                    tree_index,
                    symbol: None,
                });
            };
            if !self.store.symbols.contains(symbol) {
                return Err(TyperError::UnknownSymbol { symbol });
            }
            if visited.contains(&symbol) {
                return Err(TyperError::UnresolvedParentClassKind {
                    source: self.source,
                    tree_index,
                    symbol: Some(symbol),
                });
            }
            visited.push(symbol);
            match self.store.symbols.get(symbol).kind {
                SymbolKind::Trait => return Ok(true),
                SymbolKind::Class | SymbolKind::ModuleClass => return Ok(false),
                SymbolKind::TypeAlias => {
                    let alias_info = match *self.store.symbols.info(symbol) {
                        SymbolInfo::Complete(info) => info,
                        SymbolInfo::Missing
                            if self.index.definition_of(symbol).is_some_and(|definition| {
                                matches!(
                                    definition,
                                    SourceDefinition::Canonical { source, .. }
                                        | SourceDefinition::Derived { source, .. }
                                        if source == self.source
                                )
                            }) =>
                        {
                            self.complete_symbol_inner(symbol, info_journal)?
                        }
                        SymbolInfo::Missing => {
                            return Err(TyperError::UnresolvedParentClassKind {
                                source: self.source,
                                tree_index,
                                symbol: Some(symbol),
                            });
                        }
                        SymbolInfo::Deferred(_) => {
                            return Err(TyperError::DeferredSymbolCompletion { symbol });
                        }
                        SymbolInfo::Error => {
                            return Err(TyperError::SymbolAlreadyErrored { symbol });
                        }
                    };
                    let Type::AliasingBounds { alias } = self.store.types.get(alias_info) else {
                        return Err(TyperError::UnresolvedParentClassKind {
                            source: self.source,
                            tree_index,
                            symbol: Some(symbol),
                        });
                    };
                    current_type = *alias;
                }
                _ => {
                    return Err(TyperError::UnresolvedParentClassKind {
                        source: self.source,
                        tree_index,
                        symbol: Some(symbol),
                    });
                }
            }
        }
        Err(TyperError::UnresolvedParentClassKind {
            source: self.source,
            tree_index,
            symbol: self.parent_type_symbol(current_type),
        })
    }

    pub(super) fn complete_type_alias(
        &mut self,
        symbol: SymbolId,
        rhs: TreeId<Untyped>,
        declaration_tree_index: u32,
        context: SourceContextId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        if self
            .store
            .symbols
            .get(symbol)
            .flags
            .contains(SymbolFlags::OPAQUE)
        {
            return Err(TyperError::OpaqueAliasDeferred {
                symbol,
                tree_index: declaration_tree_index,
            });
        }
        let Some(rhs_node) = self.arena.try_get(rhs) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: rhs.index(),
            });
        };
        let info = match &rhs_node.kind {
            TreeKind::LambdaTypeTree(_) => {
                return Err(TyperError::HigherKindedTypeAliasDeferred {
                    symbol,
                    tree_index: rhs.index(),
                });
            }
            TreeKind::TypeBoundsTree(bounds) => {
                let bounds = *bounds;
                if let Some(alias) = bounds.alias {
                    let projected = self.type_of_tpt_inner(alias, context)?;
                    self.alias_bounds_for_type(projected, alias.index())?
                } else {
                    self.project_type_bounds(&bounds, context)?
                }
            }
            _ => {
                let projected = self.type_of_tpt_inner(rhs, context)?;
                self.alias_bounds_for_type(projected, rhs.index())?
            }
        };
        let previous = *self.store.symbols.info(symbol);
        info_journal.push((symbol, previous));
        self.store
            .symbols
            .set_info(symbol, SymbolInfo::Complete(info));
        Ok(info)
    }

    /// Source-side equivalent of Dotty's `toBounds`: genuine bounds and
    /// aliases keep their distinction, ordinary types become aliases, and
    /// methodic/by-name types are rejected.
    pub(in crate::typer) fn alias_bounds_for_type(
        &mut self,
        ty: TypeId,
        tree_index: u32,
    ) -> Result<TypeId, TyperError> {
        dotty_core::types::to_bounds(&mut self.store.types, ty).map_err(|_| {
            TyperError::InvalidCompletedBounds {
                source: self.source,
                tree_index,
                ty,
            }
        })
    }

    pub(super) fn complete_type_parameter(
        &mut self,
        symbol: SymbolId,
        rhs: TreeId<Untyped>,
        context: SourceContextId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let Some(rhs_node) = self.arena.try_get(rhs) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: rhs.index(),
            });
        };
        let bounds_tree = match &rhs_node.kind {
            TreeKind::TypeBoundsTree(_) => rhs,
            TreeKind::PhaseSpecific(UntypedNode::ContextBounds(context_bounds)) => {
                // Context-bound evidence remains represented in the source AST
                // for later lowering; this stage completes only the ordinary
                // bounds carried by the wrapper.
                context_bounds.bounds
            }
            TreeKind::LambdaTypeTree(_) => {
                return Err(TyperError::HigherKindedTypeParameterDeferred {
                    symbol,
                    tree_index: rhs.index(),
                });
            }
            _ => {
                return Err(TyperError::UnsupportedTypeTree {
                    source: self.source,
                    tree_index: rhs.index(),
                    tree_kind: tree_kind_name(&rhs_node.kind),
                });
            }
        };
        let Some(bounds_node) = self.arena.try_get(bounds_tree) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: bounds_tree.index(),
            });
        };
        let TreeKind::TypeBoundsTree(bounds) = &bounds_node.kind else {
            return Err(TyperError::UnsupportedTypeTree {
                source: self.source,
                tree_index: bounds_tree.index(),
                tree_kind: tree_kind_name(&bounds_node.kind),
            });
        };
        let bounds = *bounds;
        if bounds.alias.is_some() {
            return Err(TyperError::UnsupportedTypeTree {
                source: self.source,
                tree_index: bounds_tree.index(),
                tree_kind: "aliased type parameter bounds",
            });
        }
        let info = self.project_type_bounds(&bounds, context)?;
        let previous = *self.store.symbols.info(symbol);
        info_journal.push((symbol, previous));
        self.store
            .symbols
            .set_info(symbol, SymbolInfo::Complete(info));
        Ok(info)
    }

    fn project_type_bounds(
        &mut self,
        bounds: &TypeBoundsTree<Untyped>,
        context: SourceContextId,
    ) -> Result<TypeId, TyperError> {
        let low = if let Some(low) = bounds.low {
            self.type_of_tpt_inner(low, context)?
        } else {
            self.definitions.nothing_type
        };
        let high = if let Some(high) = bounds.high {
            self.type_of_tpt_inner(high, context)?
        } else {
            self.definitions.any_type
        };
        Ok(self.store.types.alloc(Type::Bounds { low, high }))
    }
}
