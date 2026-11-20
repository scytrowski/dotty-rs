//! Selects either a Conversion instance or an old-style unary implicit method.

use super::*;

pub(in crate::typer) enum ConversionTarget {
    Instance(TreeId<Typed>),
    Method {
        function: TreeId<Typed>,
        signature: TypeId,
    },
}

#[derive(Clone, Copy)]
pub(in crate::typer) struct ConversionGoal {
    pub(in crate::typer) instance: Option<TypeId>,
    pub(in crate::typer) actual: TypeId,
    pub(in crate::typer) expected: TypeId,
}

impl SourceTyper<'_> {
    pub(in crate::typer) fn conversion_target(
        &mut self,
        goal: ConversionGoal,
        context: ExpressionContext,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<ConversionTarget>, TyperError> {
        for level in self.contextual_scopes(context, tree_index)? {
            let refs = self.contextual_scope_refs(level, tree_index, journal)?;
            if let Some(target) =
                self.conversion_from_refs(refs, goal, context, tree_index, journal)?
            {
                return Ok(Some(target));
            }
        }
        let anchors: &[TypeId] = match &goal.instance {
            Some(instance) => std::slice::from_ref(instance),
            None => &[goal.actual, goal.expected],
        };
        let mut refs = Vec::new();
        for &anchor in anchors {
            let Some(candidates) = self.adaptation_companion_refs(anchor, tree_index, journal)?
            else {
                return Ok(None);
            };
            for candidate in candidates {
                if !refs.contains(&candidate) {
                    refs.push(candidate);
                }
            }
        }
        self.conversion_from_refs(refs, goal, context, tree_index, journal)
    }

    fn conversion_from_refs(
        &mut self,
        refs: Vec<ContextualRef>,
        goal: ConversionGoal,
        context: ExpressionContext,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<ConversionTarget>, TyperError> {
        if let Some(instance) = goal.instance {
            let mut history = SearchHistory::default();
            if let Some(value) = self.choose_contextual_refs(
                refs.clone(),
                instance,
                context,
                tree_index,
                &mut history,
                journal,
            )? {
                return Ok(Some(ConversionTarget::Instance(
                    history.dictionary.emit(self, value, tree_index)?,
                )));
            }
        }
        let mut candidates = Vec::new();
        for reference in refs {
            let symbol = self.store.symbols.get(reference.symbol);
            if symbol.kind != SymbolKind::Method || !symbol.flags.contains(SymbolFlags::IMPLICIT) {
                continue;
            }
            let signature =
                self.contextual_reference_signature(reference, context.owner, tree_index, journal)?;
            let Some(Type::Method(method)) = self.store.types.try_get(signature).cloned() else {
                continue;
            };
            let [parameter] = method.params.as_slice() else {
                continue;
            };
            if method.kind != MethodKind::Plain
                || parameter.erased
                || parameter.varargs
                || self.type_contains_param_ref(method.result, signature)?
                || !self.contextual_conforms(goal.actual, parameter.ty, tree_index, journal)?
                || matches!(
                    self.store.types.get(method.result),
                    Type::Poly(_) | Type::Method(_)
                )
                || !self.contextual_conforms(method.result, goal.expected, tree_index, journal)?
            {
                continue;
            }
            // Legacy conversion specificity compares the accepted inputs.
            candidates.push((
                Provider {
                    reference,
                    type_arguments: Vec::new(),
                    callable: signature,
                    result: parameter.ty,
                },
                None,
            ));
        }
        let Some(winner) =
            self.choose_contextual_candidate(candidates, goal.expected, tree_index, journal)?
        else {
            return Ok(None);
        };
        let function =
            self.contextual_reference_tree(winner.reference, context, tree_index, None, journal)?;
        Ok(Some(ConversionTarget::Method {
            function,
            signature: winner.callable,
        }))
    }
}

#[derive(Clone, Copy)]
pub(in crate::typer) struct MemberConversionGoal {
    pub(in crate::typer) class: SymbolId,
    pub(in crate::typer) receiver: TypeId,
    pub(in crate::typer) name: Name,
}

impl SourceTyper<'_> {
    pub(in crate::typer) fn member_conversion_target(
        &mut self,
        goal: MemberConversionGoal,
        context: ExpressionContext,
        site: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<(TreeId<Typed>, TypeId)>, TyperError> {
        for level in self.contextual_scopes(context, site)? {
            let refs = self.contextual_scope_refs(level, site, journal)?;
            if let Some(winner) =
                self.member_conversion_from_refs(refs, goal, context, site, journal)?
            {
                return Ok(Some(winner));
            }
        }
        let Some(refs) = self.adaptation_companion_refs(goal.receiver, site, journal)? else {
            return Ok(None);
        };
        self.member_conversion_from_refs(refs, goal, context, site, journal)
    }

    fn member_conversion_from_refs(
        &mut self,
        refs: Vec<ContextualRef>,
        goal: MemberConversionGoal,
        context: ExpressionContext,
        site: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<(TreeId<Typed>, TypeId)>, TyperError> {
        let mut candidates = Vec::new();
        for reference in refs {
            if self.initializing_local_symbols.contains(&reference.symbol) {
                continue;
            }
            let view =
                self.contextual_reference_signature(reference, context.owner, site, journal)?;
            let Some(Type::Applied { tycon, args }) = self.store.types.try_get(view) else {
                continue;
            };
            let [input, output] = args[..] else { continue };
            if self.store.types.get(*tycon).reference_symbol() != Some(goal.class)
                || !self.contextual_conforms(goal.receiver, input, site, journal)?
            {
                continue;
            }
            let members = self
                .lookup_members_journaled(output, goal.name, journal)
                .map_err(|error| TyperError::MemberLookup(Box::new(error)))?;
            for member in members {
                if self.is_selected_member_accessible(
                    member.symbol,
                    member.declaring_class,
                    context,
                    site,
                )? {
                    candidates.push((
                        Provider {
                            reference,
                            type_arguments: Vec::new(),
                            callable: view,
                            result: view,
                        },
                        None,
                    ));
                    break;
                }
            }
        }
        let winner = self
            .choose_contextual_candidate(candidates, goal.receiver, site, journal)
            .map_err(|err| match err {
                TyperError::AmbiguousContextualArgument { candidates, .. } => {
                    TyperError::AmbiguousMemberConversion {
                        source: self.source,
                        tree_index: site,
                        receiver: goal.receiver,
                        name: goal.name,
                        candidates,
                    }
                }
                other => other,
            })?;
        let Some(winner) = winner else {
            return Ok(None);
        };
        let Some(Type::Applied { args, .. }) = self.store.types.try_get(winner.result) else {
            return Err(TyperError::MemberTypeUnavailable {
                symbol: winner.reference.symbol,
            });
        };
        let output = args[1];
        let evidence =
            self.contextual_reference_tree(winner.reference, context, site, None, journal)?;
        Ok(Some((evidence, output)))
    }
}
