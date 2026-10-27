//! Completion of source method signatures and inferred method results.

use super::*;

impl SourceTyper<'_> {
    pub(super) fn generated_case_class_for_apply(
        &self,
        method: SymbolId,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<SymbolId> {
        let method_symbol = self.store.symbols.get(method);
        let owner = method_symbol.owner?;
        if source != self.source
            || method_symbol.kind != SymbolKind::Method
            || !method_symbol.flags.contains(SymbolFlags::SYNTHETIC)
            || self.store.names.resolve(method_symbol.name.text()) != "apply"
            || method_symbol.name.namespace() != dotty_core::Namespace::Term
            || method_symbol.origin != SymbolOrigin::Source(source)
            || self.store.symbols.get(owner).kind != SymbolKind::ModuleClass
            || self.index.definition_of(method) != Some(SourceDefinition::Derived { source, tree })
            || self.index.derived_symbol_at(owner, source, tree) != Some(method)
        {
            return None;
        }
        let class = self.index.symbol_at(source, tree)?;
        if self.index.definition_of(class) != Some(SourceDefinition::Canonical { source, tree }) {
            return None;
        }
        let class_symbol = self.store.symbols.get(class);
        if class_symbol.kind != SymbolKind::Class
            || !class_symbol.flags.contains(SymbolFlags::CASE)
            || class_symbol.origin != SymbolOrigin::Source(source)
        {
            return None;
        }
        let object = class_symbol.links.companion?;
        if !self.store.symbols.contains(object) {
            return None;
        }
        let object_symbol = self.store.symbols.get(object);
        if object_symbol.kind != SymbolKind::Object
            || object_symbol.owner != class_symbol.owner
            || object_symbol.links.companion != Some(class)
            || self.source_module_class_of_object(object).ok()? != owner
            || !matches!(
                self.arena.try_get(tree).map(|node| &node.kind),
                Some(TreeKind::TypeDef(definition))
                    if definition.metadata.modifiers.contains(&dotty_core::ast::Modifier::Case)
            )
        {
            return None;
        }
        Some(class)
    }

    pub(super) fn complete_generated_case_class_apply_signature(
        &mut self,
        method: SymbolId,
        class: SymbolId,
        source: SourceId,
        class_tree: TreeId<Untyped>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let Some(TreeKind::TypeDef(class_definition)) =
            self.arena.try_get(class_tree).map(|node| &node.kind)
        else {
            return Err(TyperError::SymbolSourceKindMismatch {
                source,
                tree_index: class_tree.index(),
                symbol: method,
                kind: SymbolKind::Method,
            });
        };
        let Some(TreeKind::Template(template)) = self
            .arena
            .try_get(class_definition.rhs)
            .map(|node| &node.kind)
        else {
            return Err(TyperError::MalformedMethodClause {
                method,
                method_tree_index: class_tree.index(),
                clause_index: 0,
            });
        };
        let Some(TreeKind::DefDef(constructor)) = self
            .arena
            .try_get(template.constructor)
            .map(|node| &node.kind)
        else {
            return Err(TyperError::MalformedMethodClause {
                method,
                method_tree_index: class_tree.index(),
                clause_index: 0,
            });
        };
        if constructor.value_param_clauses.len() != 1
            || constructor.value_param_clauses[0].is_empty()
        {
            return Err(TyperError::MalformedMethodClause {
                method,
                method_tree_index: class_tree.index(),
                clause_index: 0,
            });
        }

        let type_parameters = self.type_parameter_specs(
            method,
            &constructor.type_params,
            true,
            info_journal,
            class_tree.index(),
            0,
        )?;
        let (value_parameters, kind) = self.method_parameter_specs(
            method,
            &constructor.value_param_clauses[0],
            true,
            class_tree.index(),
            usize::from(!type_parameters.is_empty()),
            info_journal,
        )?;
        if kind != MethodKind::Plain {
            return Err(TyperError::MalformedMethodClause {
                method,
                method_tree_index: class_tree.index(),
                clause_index: usize::from(!type_parameters.is_empty()),
            });
        }

        let class_owner = self
            .store
            .symbols
            .get(class)
            .owner
            .ok_or(TyperError::DeclarationContextMissing { symbol: class })?;
        let prefix = self
            .store
            .types
            .alloc(Type::ThisType { class: class_owner });
        let tycon = self.store.types.alloc(Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(class),
        });
        let arguments = type_parameters
            .iter()
            .map(|parameter| {
                self.store.types.alloc(Type::TypeRef {
                    prefix: self.definitions.no_prefix,
                    target: TypeRefTarget::Symbol(parameter.symbol),
                })
            })
            .collect::<Vec<_>>();
        let result = if arguments.is_empty() {
            tycon
        } else {
            self.store.types.alloc(Type::Applied {
                tycon,
                args: arguments,
            })
        };
        let method_type = method_type_from_symbols(self.store, &value_parameters, result, kind)
            .map_err(|error| TyperError::TypeRebinding {
                source,
                tree_index: class_tree.index(),
                error,
            })?;
        if type_parameters.is_empty() {
            Ok(method_type)
        } else {
            poly_type_from_symbols(self.store, &type_parameters, method_type).map_err(|error| {
                TyperError::TypeRebinding {
                    source,
                    tree_index: class_tree.index(),
                    error,
                }
            })
        }
    }

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
                self.extension_prefix_clauses(method)
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
                self.type_value_expression_inner(rhs, context, info_journal, &mut new_mappings)?;
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
            self.type_of_tpt_inner_journaled(
                definition.tpt,
                signature_parameter_context,
                info_journal,
            )?
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
        for (parameter_index, tree) in trees.iter().enumerate() {
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
            if repeated_parameter && parameter_kind != MethodKind::Plain {
                return Err(TyperError::RepeatedParameterClauseUnsupported {
                    method,
                    method_tree_index,
                    clause_index,
                    parameter_index,
                    kind: parameter_kind,
                });
            }
            if repeated_parameter && parameter_index + 1 != trees.len() {
                return Err(TyperError::RepeatedParameterNotFinal {
                    method,
                    method_tree_index,
                    clause_index,
                    parameter_index,
                });
            }
            let completed_parameter_type =
                self.complete_method_parameter_for_signature(symbol, info_journal)?;
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
            self.index
                .derived_symbol_at(method, self.source, tree)
                .or_else(|| {
                    self.local_methods
                        .extension_receiver_parameter_symbol(method, tree)
                })
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
