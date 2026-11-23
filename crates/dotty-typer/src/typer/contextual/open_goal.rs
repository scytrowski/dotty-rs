//! Infer a selected call's binder from a concrete contextual value.

use super::*;

impl SourceTyper<'_> {
    pub(in crate::typer) fn infer_omitted_witness(
        &mut self,
        expected: TypeId,
        call: &PolyApplicationInference<'_>,
        inferred: &mut [Option<TypeId>],
        context: ExpressionContext,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<TreeId<Typed>>, TyperError> {
        let Some(Type::Applied { tycon, args }) = self.store.types.try_get(expected) else {
            return Ok(None);
        };
        let [arg] = args.as_slice() else {
            return Ok(None);
        };
        let Some(Type::ParamRef { binder, index }) = self.store.types.try_get(*arg) else {
            return Ok(None);
        };
        let index = *index as usize;
        if *binder != call.binder || inferred.get(index) != Some(&None) {
            return Ok(None);
        }
        let tycon = *tycon;
        let Some(class) = self.store.types.get(tycon).reference_symbol() else {
            return Ok(None);
        };
        if !self.is_current_source_symbol(class)
            || !matches!(self.source_class_type_parameter_metadata(class)?.as_slice(), [p] if p.variance == Some(dotty_core::Variance::Invariant))
        {
            return Ok(None);
        }
        let bounds = call.poly.params[index].bounds;
        // This search handles independently bounded type parameters. A dependent
        // bound needs constraints on more than the binder slot of this goal.
        if self.type_contains_param_ref(bounds, call.binder)? {
            return Ok(None);
        }
        let site = call.tree_index;
        for level in self.contextual_scopes(context, site)? {
            let mut candidates = Vec::new();
            for reference in self.contextual_members(level.scope, journal)? {
                if self.initializing_local_symbols.contains(&reference) {
                    continue;
                }
                let reference = ContextualRef {
                    symbol: reference,
                    receiver: None,
                };
                let actual =
                    self.contextual_reference_signature(reference, context.owner, site, journal)?;
                let Some(Type::Applied { args, .. }) = self.store.types.try_get(actual) else {
                    continue;
                };
                let [argument] = args.as_slice() else {
                    continue;
                };
                let argument = *argument;
                let closed = self.store.types.alloc(Type::Applied {
                    tycon,
                    args: vec![argument],
                });
                if !self.contextual_conforms(actual, closed, site, journal)? {
                    continue;
                }
                match self
                    .check_explicit_type_argument_bounds(argument, bounds, index, site, journal)
                {
                    Ok(()) => {}
                    Err(TyperError::ExplicitTypeArgumentBoundViolation { .. }) => continue,
                    Err(error) => return Err(error),
                }
                candidates.push((
                    Provider {
                        reference,
                        callable: actual,
                        result: actual,
                        type_arguments: vec![],
                    },
                    Ok(argument),
                ));
            }
            if let Some((provider, argument)) =
                self.choose_contextual_candidate(candidates, expected, site, journal)?
            {
                inferred[index] = Some(argument);
                return self
                    .contextual_reference_tree(provider.reference, context, site, None, journal)
                    .map(Some);
            }
        }
        Err(TyperError::MissingContextualArgument {
            source: self.source,
            tree_index: site,
            expected,
        })
    }
}
