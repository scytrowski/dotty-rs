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
        if !matches!(
            self.store.symbols.get(class).kind,
            SymbolKind::Class | SymbolKind::Trait
        ) || !self.is_current_source_symbol(class)
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
        let winner =
            self.search_contextual_scopes(expected, context, site, journal, |t, refs, j| {
                let mut candidates = Vec::new();
                for reference in refs {
                    if t.initializing_local_symbols.contains(&reference.symbol) {
                        continue;
                    }
                    let signature =
                        t.contextual_reference_signature(reference, context.owner, site, j)?;
                    let Some(provider) = t.contextual_provider_head(reference, signature, site)?
                    else {
                        continue;
                    };
                    let actual = t.contextual_type_at_owner(provider.result, context.owner, j)?;
                    if matches!(t.store.types.get(signature), Type::Poly(_))
                        && t.type_contains_param_ref(actual, signature)?
                        && matches!(t.store.types.get(actual), Type::ParamRef { .. })
                    {
                        return Err(TyperError::UsingApplicationDeferred {
                            source: t.source,
                            tree_index: site,
                        });
                    }
                    let Some(argument) =
                        t.open_witness_argument(actual, class, tycon, expected, site, j)?
                    else {
                        continue;
                    };
                    // A matching polymorphic or conditional factory needs its own
                    // inference/dependency search before it can constrain this call.
                    if matches!(
                        t.store.types.get(signature),
                        Type::Poly(_) | Type::Method(_)
                    ) {
                        return Err(TyperError::UsingApplicationDeferred {
                            source: t.source,
                            tree_index: site,
                        });
                    }
                    match t.check_explicit_type_argument_bounds(argument, bounds, index, site, j) {
                        Ok(()) => {}
                        Err(TyperError::ExplicitTypeArgumentBoundViolation { .. }) => continue,
                        Err(error) => return Err(error),
                    }
                    if candidates
                        .iter()
                        .any(|(p, _): &(Provider, _)| t.same_contextual_ref(reference, p.reference))
                    {
                        continue;
                    }
                    candidates.push((
                        Provider {
                            result: actual,
                            ..provider
                        },
                        Ok(argument),
                    ));
                }
                t.choose_contextual_candidate(candidates, expected, site, j)
            })?;
        let (provider, argument) = winner.ok_or(TyperError::MissingContextualArgument {
            source: self.source,
            tree_index: site,
            expected,
        })?;
        inferred[index] = Some(argument);
        self.contextual_reference_tree(provider.reference, context, site, None, journal)
            .map(Some)
    }

    fn open_witness_argument(
        &mut self,
        actual: TypeId,
        class: SymbolId,
        tycon: TypeId,
        expected: TypeId,
        site: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<TypeId>, TyperError> {
        if !matches!(
            self.store.types.get(actual),
            Type::Applied { .. } | Type::TypeRef { .. } | Type::ThisType { .. }
        ) {
            return Ok(None);
        }
        self.complete_relation_type(actual, journal, &mut HashSet::new(), 0)?;
        let views = self.nominal_views_as(actual, class).map_err(|error| {
            TyperError::ExpectedExpressionConformanceUnsupported {
                source: self.source,
                tree_index: site,
                actual,
                expected,
                error: Box::new(error),
            }
        })?;
        let mut inferred = None;
        for view in views {
            let Type::Applied { args, .. } = self.store.types.get(view) else {
                continue;
            };
            let [argument] = args[..] else { continue };
            let closed = self.store.types.alloc(Type::Applied {
                tycon,
                args: vec![argument],
            });
            if self.contextual_conforms(actual, closed, site, journal)? {
                if let Some(prior) = inferred {
                    if !self.contextual_conforms(prior, argument, site, journal)?
                        || !self.contextual_conforms(argument, prior, site, journal)?
                    {
                        return Err(TyperError::UsingApplicationDeferred {
                            source: self.source,
                            tree_index: site,
                        });
                    }
                } else {
                    inferred = Some(argument);
                }
            }
        }
        Ok(inferred)
    }
}
