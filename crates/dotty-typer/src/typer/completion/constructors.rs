//! Completion of primary and secondary constructor signatures.

use super::*;

impl SourceTyper<'_> {
    pub(super) fn complete_constructor_signature(
        &mut self,
        constructor: SymbolId,
        constructor_tree: TreeId<Untyped>,
        definition: &dotty_core::ast::DefDef<Untyped>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let owner = self.constructor_owner(constructor)?;
        let owner_constructor = self.owner_primary_constructor_tree(constructor, owner)?;
        let is_primary = owner_constructor == Some(constructor_tree);

        if !is_primary && !definition.type_params.is_empty() {
            return Err(TyperError::SecondaryConstructorTypeParametersUnsupported { constructor });
        }

        let mut clauses = Vec::new();
        if !definition.type_params.is_empty() {
            let clause_index = clauses.len();
            clauses.push(MethodClauseSpec::Types(self.type_parameter_specs(
                constructor,
                &definition.type_params,
                is_primary,
                info_journal,
                constructor_tree.index(),
                clause_index,
            )?));
        }
        for trees in &definition.value_param_clauses {
            let clause_index = clauses.len();
            let (parameters, kind) = self.method_parameter_specs(
                constructor,
                trees,
                is_primary,
                constructor_tree.index(),
                clause_index,
                info_journal,
            )?;
            clauses.push(MethodClauseSpec::Terms(parameters, kind));
        }
        let clauses = Self::normalize_constructor_clauses(&clauses);
        let result_parameters = if is_primary {
            match clauses.first() {
                Some(MethodClauseSpec::Types(parameters)) => parameters.clone(),
                _ => Vec::new(),
            }
        } else {
            let owner_parameters = self.owner_type_parameters(constructor, owner)?;
            self.type_parameter_specs(
                owner,
                &owner_parameters,
                false,
                info_journal,
                constructor_tree.index(),
                0,
            )?
        };
        let mut result = self.constructor_effective_result(owner, &result_parameters);
        for clause in clauses.into_iter().rev() {
            result = match clause {
                MethodClauseSpec::Types(parameters) => {
                    poly_type_from_symbols(self.store, &parameters, result).map_err(|error| {
                        TyperError::TypeRebinding {
                            source: self.source,
                            tree_index: constructor_tree.index(),
                            error,
                        }
                    })?
                }
                MethodClauseSpec::Terms(parameters, kind) => {
                    method_type_from_symbols(self.store, &parameters, result, kind).map_err(
                        |error| TyperError::TypeRebinding {
                            source: self.source,
                            tree_index: constructor_tree.index(),
                            error,
                        },
                    )?
                }
            };
        }
        Ok(result)
    }

    fn constructor_owner(&self, constructor: SymbolId) -> Result<SymbolId, TyperError> {
        let owner = self.store.symbols.get(constructor).owner;
        let Some(owner) = owner.filter(|owner| {
            self.store.symbols.contains(*owner)
                && matches!(
                    self.store.symbols.get(*owner).kind,
                    SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
                )
        }) else {
            return Err(TyperError::ConstructorOwnerNotClassLike { constructor, owner });
        };
        if let SymbolInfo::Complete(info) = *self.store.symbols.info(owner)
            && !matches!(
                self.store.types.get(info),
                Type::ClassInfo(class_info)
                    if class_info.class == owner && class_info.prefix == self.definitions.no_prefix
            )
        {
            return Err(TyperError::MalformedConstructorOwnerInfo {
                constructor,
                owner,
                info,
            });
        }
        Ok(owner)
    }

    pub(in crate::typer) fn owner_primary_constructor_tree(
        &self,
        constructor: SymbolId,
        owner: SymbolId,
    ) -> Result<Option<TreeId<Untyped>>, TyperError> {
        let definition = self
            .index
            .definition_of(owner)
            .ok_or(TyperError::SourceProvenanceMissing { symbol: owner })?;
        let (source, tree) = match definition {
            SourceDefinition::Canonical { source, tree }
            | SourceDefinition::Derived { source, tree } => (source, tree),
        };
        if source != self.source {
            return Err(TyperError::SourceProvenanceMissing { symbol: owner });
        }
        let Some(node) = self.arena.try_get(tree) else {
            return Err(TyperError::TreeOutsideArena {
                source,
                tree_index: tree.index(),
            });
        };
        let template_tree = match &node.kind {
            TreeKind::TypeDef(definition) => definition.rhs,
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(definition)) => definition.template,
            _ => {
                return Err(TyperError::MalformedConstructorOwner {
                    constructor,
                    owner,
                    tree_index: tree.index(),
                });
            }
        };
        let Some(template_node) = self.arena.try_get(template_tree) else {
            return Err(TyperError::TreeOutsideArena {
                source,
                tree_index: template_tree.index(),
            });
        };
        match &template_node.kind {
            TreeKind::Template(template) => Ok(Some(template.constructor)),
            _ => Err(TyperError::MalformedConstructorOwner {
                constructor,
                owner,
                tree_index: template_tree.index(),
            }),
        }
    }

    fn owner_type_parameters(
        &self,
        constructor: SymbolId,
        owner: SymbolId,
    ) -> Result<Vec<TreeId<Untyped>>, TyperError> {
        let Some(owner_constructor) = self.owner_primary_constructor_tree(constructor, owner)?
        else {
            return Ok(Vec::new());
        };
        let Some(node) = self.arena.try_get(owner_constructor) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: owner_constructor.index(),
            });
        };
        match &node.kind {
            TreeKind::DefDef(definition) => Ok(definition.type_params.clone()),
            _ => Err(TyperError::MalformedConstructorOwner {
                constructor,
                owner,
                tree_index: owner_constructor.index(),
            }),
        }
    }

    fn constructor_effective_result(
        &mut self,
        owner: SymbolId,
        type_parameters: &[TypeParamSpec],
    ) -> TypeId {
        let owner_ref = self
            .store
            .types
            .alloc(Type::type_ref(self.definitions.no_prefix, owner));
        if type_parameters.is_empty() {
            return owner_ref;
        }
        let args = type_parameters
            .iter()
            .map(|parameter| {
                self.store
                    .types
                    .alloc(Type::type_ref(self.definitions.no_prefix, parameter.symbol))
            })
            .collect();
        self.store.types.alloc(Type::Applied {
            tycon: owner_ref,
            args,
        })
    }

    fn normalize_constructor_clauses(clauses: &[MethodClauseSpec]) -> Vec<MethodClauseSpec> {
        match clauses.split_first() {
            Some((MethodClauseSpec::Types(parameters), rest)) => {
                let mut normalized = vec![MethodClauseSpec::Types(parameters.clone())];
                normalized.extend(Self::normalize_constructor_clauses(rest));
                normalized
            }
            Some((MethodClauseSpec::Terms(parameters, MethodKind::Implicit), _))
                if !parameters.is_empty() =>
            {
                let mut normalized = vec![MethodClauseSpec::Terms(Vec::new(), MethodKind::Plain)];
                normalized.extend(clauses.iter().cloned());
                normalized
            }
            _ => {
                let all_contextual = clauses.iter().all(|clause| match clause {
                    MethodClauseSpec::Types(_) => true,
                    MethodClauseSpec::Terms(parameters, MethodKind::Contextual) => {
                        !parameters.is_empty()
                    }
                    MethodClauseSpec::Terms(_, MethodKind::Plain | MethodKind::Implicit) => false,
                });
                let mut normalized = clauses.to_vec();
                if all_contextual {
                    normalized.push(MethodClauseSpec::Terms(Vec::new(), MethodKind::Plain));
                }
                normalized
            }
        }
    }
}
