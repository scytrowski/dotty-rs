//! Completion of source method signatures and inferred method results.

use super::*;

impl SourceTyper<'_> {
    pub(super) fn complete_method_signature(
        &mut self,
        method: SymbolId,
        method_tree_index: u32,
        definition: &dotty_core::ast::DefDef<Untyped>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let method_name = self.store.names.resolve(definition.name.as_name().text());
        let is_extension = self
            .store
            .symbols
            .get(method)
            .flags
            .contains(SymbolFlags::EXTENSION);
        if is_extension && method_name.ends_with(':') {
            return Err(TyperError::RightAssociativeExtensionDeferred {
                symbol: method,
                tree_index: method_tree_index,
            });
        }
        let Some(result_node) = self.arena.try_get(definition.tpt) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: definition.tpt.index(),
            });
        };
        let infer_result = matches!(result_node.kind, TreeKind::TypeTree(_));
        if infer_result && !self.inferred_method_results_in_progress.insert(method) {
            return Err(TyperError::RecursiveInferredMethodResult { symbol: method });
        }
        let result = self.complete_method_signature_body(
            method,
            method_tree_index,
            definition,
            infer_result,
            info_journal,
        );
        if infer_result {
            self.inferred_method_results_in_progress.remove(&method);
        }
        result
    }

    pub(super) fn complete_method_signature_body(
        &mut self,
        method: SymbolId,
        method_tree_index: u32,
        definition: &dotty_core::ast::DefDef<Untyped>,
        infer_result: bool,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let is_extension = self
            .store
            .symbols
            .get(method)
            .flags
            .contains(SymbolFlags::EXTENSION);
        let declaration_context = self
            .method_declaration_context(method)
            .ok_or(TyperError::DeclarationContextMissing { symbol: method })?;
        let mut clauses = Vec::new();
        let prefix_clauses = if is_extension {
            Some(
                self.index
                    .extension_prefix_clauses(method)
                    .ok_or(TyperError::ExtensionPrefixClausesMissing { method })?
                    .to_vec(),
            )
        } else {
            None
        };
        let first_signature_parameter = prefix_clauses
            .as_ref()
            .and_then(|clauses| clauses.iter().flatten().next().map(|tree| (*tree, true)))
            .or_else(|| definition.type_params.first().map(|tree| (*tree, false)))
            .or_else(|| {
                definition
                    .value_param_clauses
                    .iter()
                    .flatten()
                    .next()
                    .map(|tree| (*tree, false))
            });
        let signature_parameter_context = if let Some((tree, derived)) = first_signature_parameter {
            let parameter = self.method_parameter_symbol(method, tree, derived)?;
            self.parameter_source_context(parameter)
                .ok_or(TyperError::DeclarationContextMissing { symbol: parameter })?
        } else {
            declaration_context.lexical
        };
        if let Some(prefix_clauses) = prefix_clauses {
            for (clause_index, trees) in prefix_clauses.iter().enumerate() {
                clauses.push(self.extension_prefix_clause(
                    method,
                    method_tree_index,
                    clause_index,
                    trees,
                    info_journal,
                )?);
            }
        }
        if !definition.type_params.is_empty() {
            clauses.push(MethodClauseSpec::Types(self.type_parameter_specs(
                method,
                &definition.type_params,
                false,
                info_journal,
                method_tree_index,
                clauses.len(),
            )?));
        }
        for trees in &definition.value_param_clauses {
            let clause_index = clauses.len();
            let (parameters, kind) = self.method_parameter_specs(
                method,
                trees,
                false,
                method_tree_index,
                clause_index,
                info_journal,
            )?;
            clauses.push(MethodClauseSpec::Terms(parameters, kind));
        }

        let Some(result_node) = self.arena.try_get(definition.tpt) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: definition.tpt.index(),
            });
        };
        let result_type = if infer_result {
            let rhs =
                definition
                    .rhs
                    .ok_or(TyperError::InferredMethodResultRightHandSideMissing {
                        symbol: method,
                        tree_index: method_tree_index,
                    })?;
            let context = self.expression_context_for(method)?;
            let mut new_mappings = Vec::new();
            let typed_rhs =
                self.type_expression_inner(rhs, context, info_journal, &mut new_mappings)?;
            let rhs_type = self.typed_arena.get(typed_rhs).ty;
            let inferred = self.widen_expression_type_journaled(rhs_type, info_journal, 0)?;
            self.validate_inferred_method_result(method, method_tree_index, inferred)?;
            inferred
        } else {
            if matches!(&result_node.kind, TreeKind::TypeTree(_)) {
                return Err(TyperError::MissingDeclaredType {
                    source: self.source,
                    tree_index: definition.tpt.index(),
                    position: result_node.position,
                });
            }
            self.type_of_tpt_inner(definition.tpt, signature_parameter_context)?
        };
        let mut signature = result_type;
        for clause in clauses.into_iter().rev() {
            signature = match clause {
                MethodClauseSpec::Types(parameters) => {
                    poly_type_from_symbols(self.store, &parameters, signature).map_err(|error| {
                        TyperError::TypeRebinding {
                            source: self.source,
                            tree_index: method_tree_index,
                            error,
                        }
                    })?
                }
                MethodClauseSpec::Terms(parameters, kind) => {
                    method_type_from_symbols(self.store, &parameters, signature, kind).map_err(
                        |error| TyperError::TypeRebinding {
                            source: self.source,
                            tree_index: method_tree_index,
                            error,
                        },
                    )?
                }
            };
        }
        Ok(signature)
    }

    pub(super) fn validate_inferred_method_result(
        &self,
        method: SymbolId,
        tree_index: u32,
        inferred: TypeId,
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
            return Err(TyperError::InvalidInferredMethodResult {
                symbol: method,
                tree_index,
                inferred,
            });
        }
        Ok(())
    }

    pub(super) fn extension_prefix_clause(
        &mut self,
        method: SymbolId,
        method_tree_index: u32,
        clause_index: usize,
        trees: &[TreeId<Untyped>],
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<MethodClauseSpec, TyperError> {
        let Some(first_tree) = trees.first() else {
            return Ok(MethodClauseSpec::Terms(Vec::new(), MethodKind::Plain));
        };
        let Some(first_node) = self.arena.try_get(*first_tree) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: first_tree.index(),
            });
        };
        match &first_node.kind {
            TreeKind::TypeDef(_) => Ok(MethodClauseSpec::Types(self.type_parameter_specs(
                method,
                trees,
                true,
                info_journal,
                method_tree_index,
                clause_index,
            )?)),
            TreeKind::ValDef(_) => {
                let (parameters, kind) = self.method_parameter_specs(
                    method,
                    trees,
                    true,
                    method_tree_index,
                    clause_index,
                    info_journal,
                )?;
                Ok(MethodClauseSpec::Terms(parameters, kind))
            }
            _ => Err(TyperError::MalformedMethodClause {
                method,
                method_tree_index,
                clause_index,
            }),
        }
    }

    pub(super) fn type_parameter_specs(
        &mut self,
        method: SymbolId,
        trees: &[TreeId<Untyped>],
        derived: bool,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        method_tree_index: u32,
        clause_index: usize,
    ) -> Result<Vec<TypeParamSpec>, TyperError> {
        let mut parameters = Vec::with_capacity(trees.len());
        for tree in trees {
            let Some(node) = self.arena.try_get(*tree) else {
                return Err(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: tree.index(),
                });
            };
            let TreeKind::TypeDef(definition) = &node.kind else {
                return Err(TyperError::MalformedMethodClause {
                    method,
                    method_tree_index,
                    clause_index,
                });
            };
            let symbol = self.method_parameter_symbol(method, *tree, derived)?;
            if self.store.symbols.get(symbol).kind != SymbolKind::TypeParameter {
                return Err(TyperError::MalformedMethodClause {
                    method,
                    method_tree_index,
                    clause_index,
                });
            }
            let bounds = self.complete_signature_parameter(symbol, info_journal)?;
            parameters.push(TypeParamSpec {
                symbol,
                name: definition.name,
                bounds,
                declared_variance: None,
            });
        }
        Ok(parameters)
    }

    pub(super) fn method_parameter_specs(
        &mut self,
        method: SymbolId,
        trees: &[TreeId<Untyped>],
        derived: bool,
        method_tree_index: u32,
        clause_index: usize,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<(Vec<MethodParamSpec>, MethodKind), TyperError> {
        let mut parameters = Vec::with_capacity(trees.len());
        let mut clause_kind = None;
        for tree in trees {
            let Some(node) = self.arena.try_get(*tree) else {
                return Err(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: tree.index(),
                });
            };
            let TreeKind::ValDef(definition) = &node.kind else {
                return Err(TyperError::MalformedMethodClause {
                    method,
                    method_tree_index,
                    clause_index,
                });
            };
            let symbol = self.method_parameter_symbol(method, *tree, derived)?;
            if self.store.symbols.get(symbol).kind != SymbolKind::Parameter {
                return Err(TyperError::MalformedMethodClause {
                    method,
                    method_tree_index,
                    clause_index,
                });
            }
            let flags = self.store.symbols.get(symbol).flags;
            let given = flags.contains(SymbolFlags::GIVEN);
            let implicit = flags.contains(SymbolFlags::IMPLICIT);
            if given && implicit {
                return Err(TyperError::MalformedMethodClause {
                    method,
                    method_tree_index,
                    clause_index,
                });
            }
            let parameter_kind = if given {
                MethodKind::Contextual
            } else if implicit {
                MethodKind::Implicit
            } else {
                MethodKind::Plain
            };
            if clause_kind.is_some_and(|kind| kind != parameter_kind) {
                return Err(TyperError::MalformedMethodClause {
                    method,
                    method_tree_index,
                    clause_index,
                });
            }
            clause_kind = Some(parameter_kind);
            let repeated_parameter = matches!(
                self.arena.try_get(definition.tpt).map(|node| &node.kind),
                Some(TreeKind::PhaseSpecific(UntypedNode::PostfixOp(postfix)))
                    if self.store.names.resolve(postfix.op.text()) == "*"
            );
            let completed_parameter_type =
                self.complete_signature_parameter(symbol, info_journal)?;
            let ty = if repeated_parameter {
                match self.store.types.try_get(completed_parameter_type) {
                    Some(Type::Repeated { element }) => *element,
                    _ => {
                        return Err(TyperError::MalformedMethodClause {
                            method,
                            method_tree_index,
                            clause_index,
                        });
                    }
                }
            } else {
                completed_parameter_type
            };
            parameters.push(MethodParamSpec {
                symbol,
                name: definition.name,
                ty,
                erased: flags.contains(SymbolFlags::ERASED),
                varargs: repeated_parameter,
            });
        }
        Ok((parameters, clause_kind.unwrap_or(MethodKind::Plain)))
    }

    pub(super) fn method_parameter_symbol(
        &self,
        method: SymbolId,
        tree: TreeId<Untyped>,
        derived: bool,
    ) -> Result<SymbolId, TyperError> {
        let symbol = if derived {
            self.index.derived_symbol_at(method, self.source, tree)
        } else {
            self.local_methods
                .type_parameter_symbol_at(self.source, tree)
                .or_else(|| self.local_methods.parameter_symbol_at(self.source, tree))
                .or_else(|| self.index.symbol_at(self.source, tree))
        };
        symbol.ok_or(TyperError::MethodParameterSymbolMissing {
            method,
            parameter_tree_index: tree.index(),
        })
    }
}
