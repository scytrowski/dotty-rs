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
        let refs = if let Some(instance) = goal.instance {
            self.companion_refs(instance, tree_index, journal)?
        } else {
            let mut refs = self.companion_refs(goal.actual, tree_index, journal)?;
            for r in self.companion_refs(goal.expected, tree_index, journal)? {
                if !refs.contains(&r) {
                    refs.push(r);
                }
            }
            refs
        };
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
