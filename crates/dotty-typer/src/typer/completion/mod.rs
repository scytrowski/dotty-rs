//! Declaration completion orchestration and shared completion helpers.

use super::*;

mod constructors;
mod declarations;
mod local_methods;
mod methods;

pub(super) const MAX_INFERRED_FIELD_TYPE_COMPLETION_DEPTH: usize = 64;

impl SourceTyper<'_> {
    /// Completes a source declaration, rolling back this call's mutations on failure.
    ///
    /// Source classes, traits, and module classes publish [`Type::ClassInfo`]
    /// using the declaration scope recorded in the source semantic index.
    /// Class-like symbols receive the canonical `java.lang.Object` parent when
    /// no real class parent is present. Scala 3.9 TASTy also records `Object`
    /// as the parent of a trait with no explicit parent.
    pub fn complete_symbol(&mut self, symbol: SymbolId) -> Result<TypeId, TyperError> {
        self.run_atomic(|typer, info_journal| {
            if !typer.store.symbols.contains(symbol) {
                return Err(TyperError::UnknownSymbol { symbol });
            }
            match *typer.store.symbols.info(symbol) {
                SymbolInfo::Complete(ty) => {
                    if matches!(
                        typer.store.symbols.get(symbol).kind,
                        SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
                    ) {
                        typer.validate_existing_class_info(symbol, ty)?;
                    }
                    return Ok(ty);
                }
                SymbolInfo::Missing => {}
                SymbolInfo::Deferred(_) => {
                    return Err(TyperError::DeferredSymbolCompletion { symbol });
                }
                SymbolInfo::Error => return Err(TyperError::SymbolAlreadyErrored { symbol }),
            }
            typer.complete_symbol_inner(symbol, info_journal)
        })
    }

    pub(super) fn complete_symbol_inner(
        &mut self,
        symbol: SymbolId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let inferred_field = if self.store.symbols.contains(symbol) {
            let field = self.store.symbols.get(symbol);
            let eligible_owner = field.owner.is_some_and(|owner| {
                self.store.symbols.contains(owner)
                    && self.store.symbols.get(owner).kind == SymbolKind::Class
            });
            let eligible_flags = !field.flags.contains(SymbolFlags::MUTABLE)
                && !field.flags.contains(SymbolFlags::LAZY)
                && !field.flags.contains(SymbolFlags::INLINE);
            if field.kind == SymbolKind::Field && eligible_owner && eligible_flags {
                match self.index.definition_of(symbol) {
                    Some(SourceDefinition::Canonical { source, tree })
                        if source == self.source
                            && self.index.symbol_at(source, tree) == Some(symbol)
                            && matches!(
                                self.arena
                                    .try_get(tree)
                                    .and_then(|node| match &node.kind {
                                        TreeKind::ValDef(definition) => {
                                            definition
                                                .rhs
                                                .and_then(|_| self.arena.try_get(definition.tpt))
                                        }
                                        _ => None,
                                    })
                                    .map(|node| &node.kind),
                                Some(TreeKind::TypeTree(_))
                            ) =>
                    {
                        Some((source, tree))
                    }
                    _ => None,
                }
            } else {
                None
            }
        } else {
            None
        };
        if let Some((source, tree)) = inferred_field {
            return self.with_inferred_field_type(symbol, source, tree.index(), |typer| {
                // This pass probes only for field cycles; keep the existing
                // missing-type error for every other unsupported RHS shape.
                if let Err(error) =
                    typer.type_inferred_field_rhs_for_recursion(symbol, info_journal)
                    && matches!(
                        &error,
                        TyperError::RecursiveInferredFieldType { .. }
                            | TyperError::InferredFieldTypeDepthExceeded { .. }
                    )
                {
                    return Err(error);
                }
                typer.complete_symbol_inner_body(symbol, info_journal)
            });
        }
        self.complete_symbol_inner_body(symbol, info_journal)
    }

    fn type_inferred_field_rhs_for_recursion(
        &mut self,
        field: SymbolId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<(), TyperError> {
        let (_, tree) = self
            .index
            .definition_of(field)
            .and_then(|definition| match definition {
                SourceDefinition::Canonical { source, tree } if source == self.source => {
                    Some((source, tree))
                }
                _ => None,
            })
            .ok_or(TyperError::SourceProvenanceMissing { symbol: field })?;
        let Some(node) = self.arena.try_get(tree) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: tree.index(),
            });
        };
        let TreeKind::ValDef(definition) = &node.kind else {
            return Err(TyperError::SymbolSourceKindMismatch {
                source: self.source,
                tree_index: tree.index(),
                symbol: field,
                kind: SymbolKind::Field,
            });
        };
        let Some(rhs) = definition.rhs else {
            return Ok(());
        };
        let context = self.field_initializer_context_for(field)?;
        let mut new_mappings = Vec::new();
        // The enclosing completion transaction rolls these tentative trees
        // back because field type inference remains deferred in this increment.
        self.type_value_expression_inner(rhs, context, info_journal, &mut new_mappings)?;
        Ok(())
    }

    pub(super) fn with_inferred_field_type<T>(
        &mut self,
        symbol: SymbolId,
        source: SourceId,
        tree_index: u32,
        operation: impl FnOnce(&mut Self) -> Result<T, TyperError>,
    ) -> Result<T, TyperError> {
        if self.inferred_field_types_in_progress.contains(&symbol) {
            return Err(TyperError::RecursiveInferredFieldType {
                symbol,
                source,
                tree_index,
            });
        }
        if self.inferred_field_types_in_progress.len() >= MAX_INFERRED_FIELD_TYPE_COMPLETION_DEPTH {
            return Err(TyperError::InferredFieldTypeDepthExceeded {
                symbol,
                source,
                tree_index,
                max_depth: MAX_INFERRED_FIELD_TYPE_COMPLETION_DEPTH,
            });
        }
        self.inferred_field_types_in_progress.insert(symbol);
        let result = operation(self);
        self.inferred_field_types_in_progress.remove(&symbol);
        result
    }

    fn complete_symbol_inner_body(
        &mut self,
        symbol: SymbolId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let kind = self.store.symbols.get(symbol).kind;
        let (source, tree) = self
            .source_tree_for_symbol(symbol)
            .ok_or(TyperError::SourceProvenanceMissing { symbol })?;
        if source != self.source {
            return Err(TyperError::SourceProvenanceMissing { symbol });
        }
        if self.index.declaration_context_of(symbol).is_none()
            && self.method_declaration_context(symbol).is_none()
            && self.parameter_source_context(symbol).is_none()
        {
            return Err(TyperError::DeclarationContextMissing { symbol });
        }

        let Some(source_tree) = self.arena.try_get(tree) else {
            return Err(TyperError::TreeOutsideArena {
                source,
                tree_index: tree.index(),
            });
        };
        let type_tree = match (kind, &source_tree.kind) {
            (
                SymbolKind::Field
                | SymbolKind::Value
                | SymbolKind::Variable
                | SymbolKind::Parameter,
                TreeKind::ValDef(definition),
            ) => Some(definition.tpt),
            (SymbolKind::Method | SymbolKind::Constructor, TreeKind::DefDef(_)) => None,
            (
                SymbolKind::TypeParameter
                | SymbolKind::TypeAlias
                | SymbolKind::Class
                | SymbolKind::Trait,
                TreeKind::TypeDef(_),
            ) => None,
            // Namer records both object term and derived module class against
            // the original parser-only `ModuleDef`; keep their semantic
            // dispatch distinct even though their source tree is shared.
            (
                SymbolKind::Object | SymbolKind::ModuleClass,
                TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::ModuleDef(_)),
            ) => None,
            _ => {
                return Err(TyperError::SymbolSourceKindMismatch {
                    source,
                    tree_index: tree.index(),
                    symbol,
                    kind,
                });
            }
        };
        let type_definition_rhs = match &source_tree.kind {
            TreeKind::TypeDef(definition) => Some(definition.rhs),
            _ => None,
        };
        let method_definition = match &source_tree.kind {
            TreeKind::DefDef(definition) => Some(definition.clone()),
            _ => None,
        };

        match kind {
            SymbolKind::Field
            | SymbolKind::Value
            | SymbolKind::Variable
            | SymbolKind::Parameter => {
                let Some(tpt) = type_tree else {
                    return Err(TyperError::MalformedSourceAst {
                        source,
                        tree_index: tree.index(),
                        symbol,
                        kind,
                    });
                };
                if kind == SymbolKind::Parameter
                    && matches!(
                        self.arena.try_get(tpt).map(|node| &node.kind),
                        Some(TreeKind::PhaseSpecific(UntypedNode::PostfixOp(postfix)))
                            if self.store.names.resolve(postfix.op.text()) == "*"
                    )
                    && self.signature_parameter_in_progress != Some(symbol)
                {
                    return Err(TyperError::RepeatedParameterSignatureContextMissing {
                        parameter: symbol,
                        parameter_tree_index: tree.index(),
                    });
                }
                let context = self
                    .parameter_source_context(symbol)
                    .ok_or(TyperError::DeclarationContextMissing { symbol })?;
                let ty = if kind == SymbolKind::Parameter {
                    self.type_of_parameter_tpt_inner_journaled(tpt, context, info_journal)?
                } else {
                    self.type_of_tpt_inner_journaled(tpt, context, info_journal)?
                };
                let previous = *self.store.symbols.info(symbol);
                info_journal.push((symbol, previous));
                self.store
                    .symbols
                    .set_info(symbol, SymbolInfo::Complete(ty));
                Ok(ty)
            }
            SymbolKind::TypeParameter => {
                let rhs = type_definition_rhs.ok_or(TyperError::SymbolSourceKindMismatch {
                    source,
                    tree_index: tree.index(),
                    symbol,
                    kind,
                })?;
                let context = self
                    .index
                    .declaration_context_of(symbol)
                    .or_else(|| self.local_methods.type_parameter_context(symbol))
                    .ok_or(TyperError::DeclarationContextMissing { symbol })?;
                self.complete_type_parameter(symbol, rhs, context, info_journal)
            }
            SymbolKind::TypeAlias => {
                let rhs = type_definition_rhs.ok_or(TyperError::SymbolSourceKindMismatch {
                    source,
                    tree_index: tree.index(),
                    symbol,
                    kind,
                })?;
                let context = self
                    .index
                    .declaration_context_of(symbol)
                    .ok_or(TyperError::DeclarationContextMissing { symbol })?;
                self.complete_type_alias(symbol, rhs, tree.index(), context, info_journal)
            }
            SymbolKind::Method => {
                let definition = method_definition.ok_or(TyperError::SymbolSourceKindMismatch {
                    source,
                    tree_index: tree.index(),
                    symbol,
                    kind,
                })?;
                let info = if self.local_methods.contains_symbol(symbol) {
                    self.complete_local_method_signature(
                        symbol,
                        tree.index(),
                        &definition,
                        info_journal,
                    )?
                } else {
                    self.complete_method_signature(symbol, tree.index(), &definition, info_journal)?
                };
                let previous = *self.store.symbols.info(symbol);
                info_journal.push((symbol, previous));
                self.store
                    .symbols
                    .set_info(symbol, SymbolInfo::Complete(info));
                Ok(info)
            }
            SymbolKind::Constructor => {
                let definition = method_definition.ok_or(TyperError::SymbolSourceKindMismatch {
                    source,
                    tree_index: tree.index(),
                    symbol,
                    kind,
                })?;
                let info =
                    self.complete_constructor_signature(symbol, tree, &definition, info_journal)?;
                let previous = *self.store.symbols.info(symbol);
                info_journal.push((symbol, previous));
                self.store
                    .symbols
                    .set_info(symbol, SymbolInfo::Complete(info));
                Ok(info)
            }
            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass => {
                self.complete_class_info(symbol, kind, tree, info_journal)
            }
            SymbolKind::Object => Err(TyperError::UnsupportedSymbolCompletion { symbol, kind }),
            SymbolKind::Package | SymbolKind::Local => {
                Err(TyperError::UnsupportedSymbolCompletion { symbol, kind })
            }
        }
    }

    pub(super) fn complete_signature_parameter(
        &mut self,
        symbol: SymbolId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        if !self.store.symbols.contains(symbol) {
            return Err(TyperError::UnknownSymbol { symbol });
        }
        match *self.store.symbols.info(symbol) {
            SymbolInfo::Complete(ty) => Ok(ty),
            SymbolInfo::Missing => self.complete_symbol_inner(symbol, info_journal),
            SymbolInfo::Deferred(_) => Err(TyperError::DeferredSymbolCompletion { symbol }),
            SymbolInfo::Error => Err(TyperError::SymbolAlreadyErrored { symbol }),
        }
    }

    pub(super) fn complete_method_parameter_for_signature(
        &mut self,
        symbol: SymbolId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let previous = self.signature_parameter_in_progress.replace(symbol);
        let result = self.complete_signature_parameter(symbol, info_journal);
        self.signature_parameter_in_progress = previous;
        result
    }

    pub(super) fn source_tree_for_symbol(
        &self,
        symbol: SymbolId,
    ) -> Option<(SourceId, TreeId<Untyped>)> {
        self.local_methods
            .definition(symbol)
            .or_else(|| self.local_methods.parameter_definition(symbol))
            .or_else(|| self.local_methods.type_parameter_definition(symbol))
            .or_else(|| {
                self.index
                    .definition_of(symbol)
                    .map(|definition| match definition {
                        SourceDefinition::Canonical { source, tree }
                        | SourceDefinition::Derived { source, tree } => (source, tree),
                    })
            })
    }
}
