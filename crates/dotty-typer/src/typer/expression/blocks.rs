//! Block scopes and local value/method expression typing.

use super::super::{ExpressionContext, ExpressionScopeId, SourceTyper, TyperError};
use super::super::{local_block_declaration_kind, source_method_flags, tree_kind_name};
use dotty_core::ast::*;
use dotty_core::types::*;
use dotty_core::*;
use std::collections::HashMap;

#[derive(Clone, Default)]
pub(in crate::typer) struct LocalMethodIndex {
    pub(in crate::typer) symbols_by_tree: HashMap<(SourceId, TreeId<Untyped>), SymbolId>,
    pub(in crate::typer) definitions: HashMap<SymbolId, (SourceId, TreeId<Untyped>)>,
    pub(in crate::typer) scopes: HashMap<SymbolId, ScopeId>,
    pub(in crate::typer) declaration_contexts: HashMap<SymbolId, ExpressionContext>,
    pub(in crate::typer) parameter_symbols_by_tree: HashMap<(SourceId, TreeId<Untyped>), SymbolId>,
    pub(in crate::typer) parameter_definitions: HashMap<SymbolId, (SourceId, TreeId<Untyped>)>,
    pub(in crate::typer) parameter_contexts: HashMap<SymbolId, SourceContextId>,
    pub(in crate::typer) type_parameter_symbols_by_tree:
        HashMap<(SourceId, TreeId<Untyped>), SymbolId>,
    pub(in crate::typer) type_parameter_definitions: HashMap<SymbolId, (SourceId, TreeId<Untyped>)>,
    pub(in crate::typer) type_parameter_contexts: HashMap<SymbolId, SourceContextId>,
}

impl LocalMethodIndex {
    pub(in crate::typer) fn insert(
        &mut self,
        source: SourceId,
        tree: TreeId<Untyped>,
        symbol: SymbolId,
        scope: ScopeId,
        declaration_context: ExpressionContext,
    ) {
        self.symbols_by_tree.insert((source, tree), symbol);
        self.definitions.insert(symbol, (source, tree));
        self.scopes.insert(symbol, scope);
        self.declaration_contexts
            .insert(symbol, declaration_context);
    }

    pub(in crate::typer) fn symbol_at(
        &self,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<SymbolId> {
        self.symbols_by_tree.get(&(source, tree)).copied()
    }

    pub(in crate::typer) fn definition(
        &self,
        symbol: SymbolId,
    ) -> Option<(SourceId, TreeId<Untyped>)> {
        self.definitions.get(&symbol).copied()
    }

    pub(in crate::typer) fn scope(&self, symbol: SymbolId) -> Option<ScopeId> {
        self.scopes.get(&symbol).copied()
    }

    pub(in crate::typer) fn declaration_context(
        &self,
        symbol: SymbolId,
    ) -> Option<ExpressionContext> {
        self.declaration_contexts.get(&symbol).copied()
    }

    pub(in crate::typer) fn contains_symbol(&self, symbol: SymbolId) -> bool {
        self.definitions.contains_key(&symbol)
    }

    pub(in crate::typer) fn insert_parameter(
        &mut self,
        source: SourceId,
        tree: TreeId<Untyped>,
        symbol: SymbolId,
        context: SourceContextId,
    ) {
        self.parameter_symbols_by_tree
            .insert((source, tree), symbol);
        self.parameter_definitions.insert(symbol, (source, tree));
        self.parameter_contexts.insert(symbol, context);
    }

    pub(in crate::typer) fn parameter_symbol_at(
        &self,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<SymbolId> {
        self.parameter_symbols_by_tree.get(&(source, tree)).copied()
    }

    pub(in crate::typer) fn parameter_definition(
        &self,
        symbol: SymbolId,
    ) -> Option<(SourceId, TreeId<Untyped>)> {
        self.parameter_definitions.get(&symbol).copied()
    }

    pub(in crate::typer) fn parameter_context(&self, symbol: SymbolId) -> Option<SourceContextId> {
        self.parameter_contexts.get(&symbol).copied()
    }

    pub(in crate::typer) fn insert_type_parameter(
        &mut self,
        source: SourceId,
        tree: TreeId<Untyped>,
        symbol: SymbolId,
        context: SourceContextId,
    ) {
        self.type_parameter_symbols_by_tree
            .insert((source, tree), symbol);
        self.type_parameter_definitions
            .insert(symbol, (source, tree));
        self.type_parameter_contexts.insert(symbol, context);
    }

    pub(in crate::typer) fn type_parameter_symbol_at(
        &self,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<SymbolId> {
        self.type_parameter_symbols_by_tree
            .get(&(source, tree))
            .copied()
    }

    pub(in crate::typer) fn type_parameter_definition(
        &self,
        symbol: SymbolId,
    ) -> Option<(SourceId, TreeId<Untyped>)> {
        self.type_parameter_definitions.get(&symbol).copied()
    }

    pub(in crate::typer) fn type_parameter_context(
        &self,
        symbol: SymbolId,
    ) -> Option<SourceContextId> {
        self.type_parameter_contexts.get(&symbol).copied()
    }
}

impl SourceTyper<'_> {
    pub(in crate::typer) fn preindex_local_methods(
        &mut self,
        stats: &[TreeId<Untyped>],
        declaration_context: ExpressionContext,
        block_scope: ScopeId,
    ) -> Result<(), TyperError> {
        for tree in stats {
            let Some(source_tree) = self.arena.try_get(*tree) else {
                return Err(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: tree.index(),
                });
            };
            let TreeKind::DefDef(definition) = &source_tree.kind else {
                continue;
            };
            let name = *definition.name.as_name();
            let symbol = self.store.symbols.alloc(dotty_core::Symbol {
                name,
                owner: Some(declaration_context.owner),
                kind: SymbolKind::Method,
                flags: source_method_flags(&definition.metadata.modifiers),
                visibility: dotty_core::Visibility::Public,
                info: SymbolInfo::Missing,
                origin: SymbolOrigin::Source(self.source),
                annotations: Vec::new(),
                position: source_tree.position,
                links: dotty_core::SymbolLinks::default(),
            });
            let method_scope = self
                .store
                .scopes
                .alloc(dotty_core::Scope::new(Some(symbol)));
            self.store.scopes.get_mut(block_scope).enter(name, symbol);
            self.local_methods.insert(
                self.source,
                *tree,
                symbol,
                method_scope,
                declaration_context,
            );
        }
        Ok(())
    }

    pub(in crate::typer) fn type_local_value(
        &mut self,
        tree: TreeId<Untyped>,
        definition: &dotty_core::ast::ValDef<Untyped>,
        position: Option<SourceSpan>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let Some(local_stack) = context.local_scopes else {
            return Err(TyperError::ExpressionLocalScopeStackMissing {
                stack: ExpressionScopeId::new(
                    self.expression_scope_owner,
                    self.expression_scopes.len(),
                ),
            });
        };
        let frame = self
            .expression_scopes
            .get(local_stack.index())
            .ok_or(TyperError::ExpressionLocalScopeStackMissing { stack: local_stack })?;
        if !frame.is_block_scope {
            return Err(TyperError::LocalValueOutsideBlock {
                source: self.source,
                tree_index: tree.index(),
            });
        }
        let scope = frame.scope;
        let name = *definition.name.as_name();
        if !self.store.scopes.get(scope).lookup_all(&name).is_empty() {
            return Err(TyperError::DuplicateLocalValue {
                source: self.source,
                tree_index: tree.index(),
                name,
            });
        }
        let Some(source_tpt) = self.arena.try_get(definition.tpt) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: definition.tpt.index(),
            });
        };
        let inferred = matches!(source_tpt.kind, TreeKind::TypeTree(_));
        let rhs = definition
            .rhs
            .ok_or(TyperError::LocalValueRightHandSideMissing {
                source: self.source,
                tree_index: tree.index(),
            })?;
        let declared_type = if inferred {
            None
        } else {
            self.active_local_import_scopes
                .push((context.lexical, context.local_scopes));
            let projected = self.type_of_tpt_inner(definition.tpt, context.lexical);
            self.active_local_import_scopes.pop();
            Some(projected?)
        };

        // Scala 3 gives a local definition scope over the entire statement
        // sequence, including its own initializer. Enter the binding now so it
        // shadows outer names, and diagnose a reference to it while initializing.
        let symbol = self.store.symbols.alloc(dotty_core::Symbol {
            name,
            owner: Some(context.owner),
            kind: SymbolKind::Local,
            flags: if definition
                .metadata
                .modifiers
                .contains(&dotty_core::ast::Modifier::Var)
            {
                SymbolFlags::MUTABLE
            } else {
                SymbolFlags::EMPTY
            },
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Source(self.source),
            annotations: Vec::new(),
            position,
            links: dotty_core::SymbolLinks::default(),
        });
        if !inferred {
            self.store.scopes.get_mut(scope).enter(name, symbol);
            self.initializing_local_symbols.insert(symbol);
        }
        let typed_rhs_result = self.type_expression_inner(rhs, context, info_journal, new_mappings);
        if !inferred {
            self.initializing_local_symbols.remove(&symbol);
        }
        let typed_rhs = typed_rhs_result?;
        let rhs_type = self.typed_arena.get(typed_rhs).ty;
        let actual = self.widen_expression_type_journaled(rhs_type, info_journal, 0)?;
        let declared_type = if let Some(declared_type) = declared_type {
            match self.conforms(actual, declared_type) {
                Ok(true) => {}
                Ok(false) => {
                    return Err(TyperError::LocalValueTypeMismatch {
                        source: self.source,
                        tree_index: tree.index(),
                        actual,
                        expected: declared_type,
                    });
                }
                Err(error) => {
                    return Err(TyperError::LocalValueConformanceUnsupported {
                        source: self.source,
                        tree_index: tree.index(),
                        actual,
                        expected: declared_type,
                        error: Box::new(error),
                    });
                }
            }
            declared_type
        } else {
            self.validate_inferred_local_value_type(actual, tree.index())?;
            actual
        };
        self.store
            .symbols
            .set_info(symbol, SymbolInfo::Complete(declared_type));
        if inferred {
            self.store.scopes.get_mut(scope).enter(name, symbol);
        }
        self.local_symbols.insert((self.source, tree), symbol);

        let typed_tpt = if inferred {
            self.reify_inferred_local_type_tree(definition.tpt, declared_type, new_mappings)?
        } else {
            self.reify_type_argument(definition.tpt, declared_type, new_mappings)?
        };
        Ok(self.typed_arena.alloc(Tree {
            kind: TreeKind::ValDef(ValDef {
                name: definition.name,
                tpt: typed_tpt,
                rhs: Some(typed_rhs),
                metadata: (),
            }),
            position,
            ty: declared_type,
        }))
    }

    pub(in crate::typer) fn type_local_method_definition(
        &mut self,
        tree: TreeId<Untyped>,
        definition: &dotty_core::ast::DefDef<Untyped>,
        position: Option<SourceSpan>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        if let Some(typed) = self.typed_index.get(self.source, tree) {
            return Ok(typed);
        }
        let method = self.local_methods.symbol_at(self.source, tree).ok_or(
            TyperError::LocalBlockDeclarationDeferred {
                source: self.source,
                tree_index: tree.index(),
                kind: "method definition",
            },
        )?;
        let signature = match *self.store.symbols.info(method) {
            SymbolInfo::Complete(signature) => signature,
            SymbolInfo::Missing => self.complete_symbol_inner(method, info_journal)?,
            SymbolInfo::Deferred(_) => {
                return Err(TyperError::DeferredSymbolCompletion { symbol: method });
            }
            SymbolInfo::Error => return Err(TyperError::SymbolAlreadyErrored { symbol: method }),
        };
        let mut result = signature;
        if !definition.type_params.is_empty() {
            result = match self.store.types.try_get(result) {
                Some(Type::Poly(poly)) if poly.params.len() == definition.type_params.len() => {
                    poly.result
                }
                _ => {
                    return Err(TyperError::LocalBlockDeclarationDeferred {
                        source: self.source,
                        tree_index: tree.index(),
                        kind: "method definition",
                    });
                }
            };
        }
        for _ in &definition.value_param_clauses {
            result = match self.store.types.try_get(result) {
                Some(Type::Method(method_type)) => method_type.result,
                _ => {
                    return Err(TyperError::LocalBlockDeclarationDeferred {
                        source: self.source,
                        tree_index: tree.index(),
                        kind: "method definition",
                    });
                }
            };
        }
        let mut typed_type_params = Vec::with_capacity(definition.type_params.len());
        for type_parameter_tree in &definition.type_params {
            let Some(parameter_node) = self.arena.try_get(*type_parameter_tree).cloned() else {
                return Err(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: type_parameter_tree.index(),
                });
            };
            let TreeKind::TypeDef(parameter) = parameter_node.kind else {
                return Err(TyperError::LocalBlockDeclarationDeferred {
                    source: self.source,
                    tree_index: type_parameter_tree.index(),
                    kind: "method type parameter",
                });
            };
            let parameter_symbol = self
                .local_methods
                .type_parameter_symbol_at(self.source, *type_parameter_tree)
                .ok_or(TyperError::MethodParameterSymbolMissing {
                    method,
                    parameter_tree_index: type_parameter_tree.index(),
                })?;
            let bounds = match *self.store.symbols.info(parameter_symbol) {
                SymbolInfo::Complete(bounds) => bounds,
                _ => {
                    return Err(TyperError::MethodParameterSymbolMissing {
                        method,
                        parameter_tree_index: type_parameter_tree.index(),
                    });
                }
            };
            let (low, high) = match self.store.types.try_get(bounds).cloned() {
                Some(Type::Bounds { low, high }) => (low, high),
                _ => {
                    return Err(TyperError::LocalBlockDeclarationDeferred {
                        source: self.source,
                        tree_index: type_parameter_tree.index(),
                        kind: "method type parameter bounds",
                    });
                }
            };
            let Some(rhs_node) = self.arena.try_get(parameter.rhs) else {
                return Err(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: parameter.rhs.index(),
                });
            };
            let TreeKind::TypeBoundsTree(source_bounds) = rhs_node.kind else {
                return Err(TyperError::LocalBlockDeclarationDeferred {
                    source: self.source,
                    tree_index: parameter.rhs.index(),
                    kind: "method type parameter bounds",
                });
            };
            let typed_low = match source_bounds.low {
                Some(source_tree) => {
                    Some(self.reify_constructor_type_tree(source_tree, low, new_mappings)?)
                }
                None => None,
            };
            let typed_high = match source_bounds.high {
                Some(source_tree) => {
                    Some(self.reify_constructor_type_tree(source_tree, high, new_mappings)?)
                }
                None => None,
            };
            let typed_rhs = self.typed_arena.alloc(Tree {
                kind: TreeKind::TypeBoundsTree(TypeBoundsTree {
                    low: typed_low,
                    high: typed_high,
                    alias: None,
                }),
                position: rhs_node.position,
                ty: bounds,
            });
            self.typed_index
                .insert(self.source, parameter.rhs, typed_rhs)
                .map_err(|error| TyperError::ConflictingTypedExpression {
                    source: error.source,
                    tree_index: error.untyped.index(),
                    existing: error.existing.index(),
                    attempted: error.attempted.index(),
                })?;
            new_mappings.push((self.source, parameter.rhs));
            let typed_parameter = self.typed_arena.alloc(Tree {
                kind: TreeKind::TypeDef(TypeDef {
                    name: parameter.name,
                    rhs: typed_rhs,
                    metadata: (),
                    variance: parameter.variance,
                }),
                position: parameter_node.position,
                ty: self.store.types.alloc(Type::TypeRef {
                    prefix: self.definitions.no_prefix,
                    target: TypeRefTarget::Symbol(parameter_symbol),
                }),
            });
            self.typed_index
                .insert(self.source, *type_parameter_tree, typed_parameter)
                .map_err(|error| TyperError::ConflictingTypedExpression {
                    source: error.source,
                    tree_index: error.untyped.index(),
                    existing: error.existing.index(),
                    attempted: error.attempted.index(),
                })?;
            new_mappings.push((self.source, *type_parameter_tree));
            typed_type_params.push(typed_parameter);
        }

        let mut typed_clauses = Vec::with_capacity(definition.value_param_clauses.len());
        for clause in &definition.value_param_clauses {
            let mut typed_parameters = Vec::with_capacity(clause.len());
            for parameter_tree in clause {
                let Some(parameter_node) = self.arena.try_get(*parameter_tree).cloned() else {
                    return Err(TyperError::TreeOutsideArena {
                        source: self.source,
                        tree_index: parameter_tree.index(),
                    });
                };
                let TreeKind::ValDef(parameter) = parameter_node.kind else {
                    return Err(TyperError::LocalBlockDeclarationDeferred {
                        source: self.source,
                        tree_index: parameter_tree.index(),
                        kind: "method parameter",
                    });
                };
                if let Some(typed) = self.typed_index.get(self.source, *parameter_tree) {
                    typed_parameters.push(typed);
                    continue;
                }
                let parameter_symbol = self
                    .local_methods
                    .parameter_symbol_at(self.source, *parameter_tree)
                    .ok_or(TyperError::MethodParameterSymbolMissing {
                        method,
                        parameter_tree_index: parameter_tree.index(),
                    })?;
                let parameter_type = match *self.store.symbols.info(parameter_symbol) {
                    SymbolInfo::Complete(ty) => ty,
                    _ => {
                        return Err(TyperError::MethodParameterSymbolMissing {
                            method,
                            parameter_tree_index: parameter_tree.index(),
                        });
                    }
                };
                let typed_tpt =
                    self.reify_constructor_type_tree(parameter.tpt, parameter_type, new_mappings)?;
                let typed_parameter = self.typed_arena.alloc(Tree {
                    kind: TreeKind::ValDef(ValDef {
                        name: parameter.name,
                        tpt: typed_tpt,
                        rhs: None,
                        metadata: (),
                    }),
                    position: parameter_node.position,
                    ty: parameter_type,
                });
                self.typed_index
                    .insert(self.source, *parameter_tree, typed_parameter)
                    .map_err(|error| TyperError::ConflictingTypedExpression {
                        source: error.source,
                        tree_index: error.untyped.index(),
                        existing: error.existing.index(),
                        attempted: error.attempted.index(),
                    })?;
                new_mappings.push((self.source, *parameter_tree));
                typed_parameters.push(typed_parameter);
            }
            typed_clauses.push(typed_parameters);
        }

        let typed_result =
            self.reify_constructor_type_tree(definition.tpt, result, new_mappings)?;
        let rhs = definition
            .rhs
            .ok_or(TyperError::LocalBlockDeclarationDeferred {
                source: self.source,
                tree_index: tree.index(),
                kind: "method definition without body",
            })?;
        let method_context = self.expression_context_for(method)?;
        let declaration_context = self
            .local_methods
            .declaration_context(method)
            .ok_or(TyperError::DeclarationContextMissing { symbol: method })?;
        let method_scope = self.method_scope(method)?;
        if !definition.type_params.is_empty() {
            let type_parameters = definition
                .type_params
                .iter()
                .map(|parameter_tree| {
                    self.local_methods
                        .type_parameter_symbol_at(self.source, *parameter_tree)
                        .ok_or(TyperError::MethodParameterSymbolMissing {
                            method,
                            parameter_tree_index: parameter_tree.index(),
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            self.active_local_type_binders
                .push((signature, type_parameters));
        }
        self.active_local_type_scopes
            .push((declaration_context.lexical, method_scope));
        self.active_local_import_scopes.push((
            declaration_context.lexical,
            declaration_context.local_scopes,
        ));
        let typed_rhs = self.type_expression_expected_inner(
            rhs,
            method_context,
            result,
            info_journal,
            new_mappings,
        );
        self.active_local_import_scopes.pop();
        self.active_local_type_scopes.pop();
        if !definition.type_params.is_empty() {
            self.active_local_type_binders.pop();
        }
        let typed_rhs = typed_rhs?;
        let definition_type = self.store.types.alloc(Type::TermRef {
            prefix: self.definitions.no_prefix,
            target: TermRefTarget::Symbol(method),
        });
        let typed = self.typed_arena.alloc(Tree {
            kind: TreeKind::DefDef(DefDef {
                name: definition.name,
                type_params: typed_type_params,
                value_param_clauses: typed_clauses,
                tpt: typed_result,
                rhs: Some(typed_rhs),
                metadata: (),
            }),
            position,
            ty: definition_type,
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

    pub(in crate::typer) fn validate_inferred_local_value_type(
        &self,
        inferred: TypeId,
        tree_index: u32,
    ) -> Result<(), TyperError> {
        if matches!(
            self.store.types.try_get(inferred),
            None | Some(
                Type::NoType
                    | Type::NoPrefix
                    | Type::Error(_)
                    | Type::Bounds { .. }
                    | Type::AliasingBounds { .. }
                    | Type::ByName { .. }
                    | Type::Repeated { .. }
                    | Type::Method(_)
                    | Type::Poly(_)
                    | Type::TypeLambda(_)
                    | Type::RecThis { .. }
                    | Type::Wildcard { .. }
                    | Type::MatchCase { .. }
                    | Type::ClassInfo(_)
            )
        ) {
            return Err(TyperError::InvalidInferredLocalValueType {
                source: self.source,
                tree_index,
                inferred,
            });
        }
        Ok(())
    }

    /// The parser uses an empty source `TypeTree` for a missing local annotation.
    /// In typed trees that source identity maps to a concrete type tree carrying
    /// the inferred local symbol info.
    pub(in crate::typer) fn reify_inferred_local_type_tree(
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
        if !matches!(source_node.kind, TreeKind::TypeTree(_)) {
            return Err(TyperError::TypeArgumentTreeCannotBeReified {
                source: self.source,
                tree_index: source_tree.index(),
                tree_kind: tree_kind_name(&source_node.kind),
            });
        }
        let typed = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
            .type_tree(ty, source_node.position);
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
    pub(in crate::typer) fn type_block_expression(
        &mut self,
        tree: TreeId<Untyped>,
        block: dotty_core::ast::Block<Untyped>,
        position: Option<SourceSpan>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let contains_local_method = block.stats.iter().any(|stat| {
            self.arena
                .try_get(*stat)
                .is_some_and(|node| matches!(node.kind, TreeKind::DefDef(_)))
        });
        let block_scope = self
            .store
            .scopes
            .alloc(dotty_core::Scope::new(Some(context.owner)));
        let block_context = self.push_local_scope(context, block_scope)?;
        if let Some(frame) = block_context
            .local_scopes
            .and_then(|stack| self.expression_scopes.get_mut(stack.index()))
        {
            frame.is_block_scope = true;
        }
        self.preindex_local_methods(&block.stats, block_context, block_scope)?;
        let mut stats = Vec::with_capacity(block.stats.len());
        for stat in block.stats {
            let Some(source_stat) = self.arena.try_get(stat) else {
                return Err(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: stat.index(),
                });
            };
            if let TreeKind::Import(import) = &source_stat.kind {
                let typed = self.type_local_import_statement(
                    stat,
                    import.clone(),
                    source_stat.position,
                    block_context,
                    info_journal,
                    new_mappings,
                )?;
                stats.push(typed);
                let Some(scope_stack) = block_context.local_scopes else {
                    return Err(TyperError::LocalBlockDeclarationDeferred {
                        source: self.source,
                        tree_index: stat.index(),
                        kind: "import scope",
                    });
                };
                self.expression_scopes
                    .get_mut(scope_stack.index())
                    .ok_or(TyperError::ExpressionLocalScopeStackMissing { stack: scope_stack })?
                    .imports
                    .push((stat, block_context));
                continue;
            }
            if let Some(kind) = local_block_declaration_kind(&source_stat.kind) {
                if let TreeKind::DefDef(definition) = &source_stat.kind
                    && definition.rhs.is_some()
                {
                    stats.push(self.type_local_method_definition(
                        stat,
                        definition,
                        source_stat.position,
                        info_journal,
                        new_mappings,
                    )?);
                    continue;
                }
                if let TreeKind::ValDef(definition) = &source_stat.kind {
                    if definition
                        .metadata
                        .modifiers
                        .iter()
                        .any(|modifier| *modifier != dotty_core::ast::Modifier::Var)
                    {
                        return Err(TyperError::LocalBlockDeclarationDeferred {
                            source: self.source,
                            tree_index: stat.index(),
                            kind,
                        });
                    }
                    stats.push(self.type_expression_inner(
                        stat,
                        block_context,
                        info_journal,
                        new_mappings,
                    )?);
                    continue;
                }
                return Err(TyperError::LocalBlockDeclarationDeferred {
                    source: self.source,
                    tree_index: stat.index(),
                    kind,
                });
            }
            stats.push(self.type_expression_inner(
                stat,
                block_context,
                info_journal,
                new_mappings,
            )?);
        }
        let expr =
            self.type_expression_inner(block.expr, block_context, info_journal, new_mappings)?;
        let ty = self.typed_arena.get(expr).ty;
        let typed = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
            .block(stats, expr, ty, position);
        if contains_local_method {
            self.typed_index
                .insert(self.source, tree, typed)
                .map_err(|error| TyperError::ConflictingTypedExpression {
                    source: error.source,
                    tree_index: error.untyped.index(),
                    existing: error.existing.index(),
                    attempted: error.attempted.index(),
                })?;
            new_mappings.push((self.source, tree));
        }
        Ok(typed)
    }
}
