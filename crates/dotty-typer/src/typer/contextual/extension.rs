//! Extension methods contributed by a contextual value.

use super::*;

pub(in crate::typer) enum ExtensionLookup {
    Selected(TreeId<Typed>, TypeId),
    Missing(Option<TyperError>),
}

pub(in crate::typer) struct ExtensionCall<'a> {
    pub arguments: &'a [TreeId<Untyped>],
    pub expected_result: Option<TypeId>,
    pub following_using: Option<&'a [TreeId<Untyped>]>,
    pub typed_arguments: &'a mut Option<Vec<TypedArgument>>,
    pub mappings: &'a mut Vec<(SourceId, TreeId<Untyped>)>,
}

struct ExtensionAlternative {
    provider: Provider,
    receiver: TypeId,
    symbol: SymbolId,
    signature: TypeId,
}

impl SourceTyper<'_> {
    /// Select an extension on a contextual receiver after ordinary member
    /// lookup has reported that the call has no applicable nominal member.
    pub(in crate::typer) fn contextual_extension(
        &mut self,
        name: Name,
        receiver: TypeId,
        context: ExpressionContext,
        tree_index: u32,
        mut call: Option<ExtensionCall<'_>>,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<ExtensionLookup, TyperError> {
        let mut search = AdaptationSearch::default();
        for level in self.contextual_scopes(context, tree_index)? {
            let refs = self.contextual_scope_refs(level, tree_index, journal)?;
            if let Some((function, signature)) = self.extension_from_refs(
                refs,
                name,
                receiver,
                context,
                tree_index,
                &mut search,
                call.as_mut(),
                journal,
            )? {
                return Ok(ExtensionLookup::Selected(function, signature));
            }
        }
        if let Some(refs) = self.adaptation_companion_refs(receiver, tree_index, journal)?
            && let Some((function, signature)) = self.extension_from_refs(
                refs,
                name,
                receiver,
                context,
                tree_index,
                &mut search,
                call.as_mut(),
                journal,
            )?
        {
            return Ok(ExtensionLookup::Selected(function, signature));
        }
        Ok(ExtensionLookup::Missing(search.dependency_failure))
    }

    #[allow(clippy::too_many_arguments)]
    fn extension_from_refs(
        &mut self,
        refs: Vec<ContextualRef>,
        name: Name,
        receiver: TypeId,
        context: ExpressionContext,
        tree_index: u32,
        search: &mut AdaptationSearch,
        mut call: Option<&mut ExtensionCall<'_>>,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<(TreeId<Typed>, TypeId)>, TyperError> {
        let mut candidates = Vec::new();

        for evidence in refs {
            if self.initializing_local_symbols.contains(&evidence.symbol) {
                continue;
            }
            let view =
                self.contextual_reference_signature(evidence, context.owner, tree_index, journal)?;
            let Some(open) = self.contextual_provider_head(evidence, view, tree_index)? else {
                continue;
            };
            let result = open.result;
            let tycon = match self.store.types.try_get(result) {
                Some(Type::Applied { tycon, .. }) => *tycon,
                _ => result,
            };
            let Some(Type::TypeRef {
                target: TypeRefTarget::Symbol(class),
                ..
            }) = self.store.types.try_get(tycon)
            else {
                continue;
            };
            if !self.store.symbols.contains(*class)
                || !matches!(
                    self.store.symbols.get(*class).kind,
                    SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
                )
                || matches!(self.store.symbols.get(*class).origin, SymbolOrigin::Builtin)
            {
                continue;
            }
            let members = self
                .lookup_members_journaled(result, name, journal)
                .map_err(|error| TyperError::MemberLookup(Box::new(error)))?;
            let mut alternatives = Vec::new();
            for member in members {
                if !self
                    .store
                    .symbols
                    .get(member.symbol)
                    .flags
                    .contains(SymbolFlags::EXTENSION)
                    || !self.is_selected_member_accessible(
                        member.symbol,
                        member.declaring_class,
                        context,
                        tree_index,
                    )?
                {
                    continue;
                }
                let signature = self.member_type_on_journaled(&member, journal)?;
                let Some(input) = self.extension_receiver(signature) else {
                    continue;
                };
                let Some(provider) = self.instantiate_receiver_provider(
                    open.clone(),
                    input,
                    receiver,
                    tree_index,
                    journal,
                )?
                else {
                    continue;
                };
                let (signature, input) = if provider.type_arguments.is_empty() {
                    (signature, input)
                } else {
                    // Get the member as seen on the instantiated provider, including inherited members.
                    let Some(instantiated) = self
                        .lookup_members_journaled(provider.result, name, journal)
                        .map_err(|e| TyperError::MemberLookup(Box::new(e)))?
                        .into_iter()
                        .find(|m| m.symbol == member.symbol)
                    else {
                        continue;
                    };
                    let signature = self.member_type_on_journaled(&instantiated, journal)?;
                    let Some(input) = self.extension_receiver(signature) else {
                        continue;
                    };
                    (signature, input)
                };
                if !self.contextual_conforms(receiver, input, tree_index, journal)? {
                    continue;
                }
                if let Some(cause) =
                    search.dependency_error(self, &provider, context, tree_index)?
                {
                    search.dependency_failure.get_or_insert_with(|| {
                        TyperError::ExtensionProviderDependency {
                            source: self.source,
                            tree_index,
                            name,
                            provider: evidence.symbol,
                            cause: Box::new(cause),
                        }
                    });
                    continue;
                }
                alternatives.push(ExtensionAlternative {
                    receiver: input,
                    provider,
                    symbol: member.symbol,
                    signature,
                });
            }
            let selected = if alternatives.len() <= 1 {
                alternatives.pop()
            } else if let Some(index) =
                self.unique_extension_receiver(&alternatives, tree_index, journal)?
            {
                Some(alternatives.swap_remove(index))
            } else {
                let Some(call) = call.as_deref_mut() else {
                    return Err(TyperError::OverloadedSelectionDeferred {
                        source: self.source,
                        tree_index,
                        name,
                    });
                };
                let winner = match self.choose_extension_overload(
                    &alternatives,
                    call,
                    context,
                    tree_index,
                    journal,
                ) {
                    Ok(symbol) => symbol,
                    Err(
                        cause @ (TyperError::OverloadApplicationNoApplicable {
                            tree_index: site,
                            ..
                        }
                        | TyperError::AmbiguousOverloadApplication {
                            tree_index: site, ..
                        }),
                    ) if site == tree_index => {
                        search
                            .dependency_failure
                            .get_or_insert(TyperError::ExtensionOverload {
                                source: self.source,
                                tree_index,
                                name,
                                provider: evidence.symbol,
                                cause: Box::new(cause),
                            });
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                alternatives.into_iter().find(|alt| alt.symbol == winner)
            };
            if let Some(alt) = selected {
                candidates.push((alt.provider, Ok((alt.symbol, alt.signature))));
            }
        }
        let Some((provider, (symbol, signature))) =
            self.choose_contextual_candidate(candidates, receiver, tree_index, journal)?
        else {
            return Ok(None);
        };
        let qualifier = search.materialize(self, &provider, context, tree_index, journal)?;
        let ty = self.store.types.alloc(Type::TermRef {
            prefix: self.typed_arena.get(qualifier).ty,
            target: TermRefTarget::Symbol(symbol),
        });
        let function = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
            .select(qualifier, name, false, ty, None);
        Ok(Some((function, signature)))
    }

    /// Resolve the receiver clause before consulting a later argument clause.
    /// Only a receiver strictly narrower than every rival settles the call here.
    fn unique_extension_receiver(
        &mut self,
        alternatives: &[ExtensionAlternative],
        site: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<usize>, TyperError> {
        let Some(first) = alternatives.first() else {
            return Ok(None);
        };
        let owner = self.store.symbols.owner(first.symbol);
        let mut best = 0;
        for (i, alt) in alternatives.iter().enumerate().skip(1) {
            // Inherited overloads from different owners also need owner ranking.
            if self.store.symbols.owner(alt.symbol) != owner {
                return Ok(None);
            }
            if self.contextual_conforms(alt.receiver, alternatives[best].receiver, site, journal)? {
                best = i;
            }
        }
        let receiver = alternatives[best].receiver;
        for (i, alt) in alternatives.iter().enumerate() {
            if i != best
                && (!self.contextual_conforms(receiver, alt.receiver, site, journal)?
                    || self.contextual_conforms(alt.receiver, receiver, site, journal)?)
            {
                return Ok(None);
            }
        }
        Ok(Some(best))
    }

    fn choose_extension_overload(
        &mut self,
        alternatives: &[ExtensionAlternative],
        call: &mut ExtensionCall<'_>,
        context: ExpressionContext,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<SymbolId, TyperError> {
        let arguments = match &mut *call.typed_arguments {
            Some(typed) => typed.as_slice(),
            slot @ None => slot
                .insert(self.type_extension_arguments(
                    call.arguments,
                    context,
                    journal,
                    call.mappings,
                )?)
                .as_slice(),
        };
        let mut candidates = self.extension_overload_candidates(alternatives, tree_index)?;
        let mut resolution = if candidates
            .iter()
            .any(|c| matches!(self.store.types.get(c.callable), Type::Poly(_)))
        {
            self.choose_mixed_extension_overload(
                alternatives,
                &mut candidates,
                arguments,
                call.expected_result,
                tree_index,
                journal,
            )
        } else {
            self.choose_overload_candidate(
                &mut candidates,
                arguments,
                tree_index,
                ApplyKind::Regular,
            )
        };
        if let Err(TyperError::AmbiguousOverloadApplication {
            candidates: tied, ..
        }) = &resolution
            && let Some(using) = call.following_using
        {
            // Only written arguments can distinguish otherwise identical ordinary clauses.
            candidates.retain_mut(|candidate| {
                let Type::Method(ordinary) = self.store.types.get(candidate.callable) else {
                    return false;
                };
                candidate.callable = ordinary.result;
                tied.contains(&candidate.symbol)
                    && matches!(self.store.types.get(candidate.callable), Type::Method(_))
            });
            let explicit = self.type_extension_arguments(using, context, journal, call.mappings)?;
            resolution = self.choose_overload_candidate(
                &mut candidates,
                &explicit,
                tree_index,
                ApplyKind::Using,
            );
        }
        resolution.map(|candidate| candidate.symbol)
    }

    fn choose_mixed_extension_overload(
        &mut self,
        alternatives: &[ExtensionAlternative],
        candidates: &mut [ApplicationCandidate],
        arguments: &[TypedArgument],
        expected: Option<TypeId>,
        site: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<ApplicationCandidate, TyperError> {
        // The ordinary clause can decide specificity only after the receivers tie.
        if let Some((first, rest)) = alternatives.split_first() {
            for alt in rest {
                if !self.contextual_conforms(first.receiver, alt.receiver, site, journal)?
                    || !self.contextual_conforms(alt.receiver, first.receiver, site, journal)?
                {
                    return Err(TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                        source: self.source,
                        tree_index: site,
                        candidate: alt.symbol,
                    });
                }
            }
        }

        let mut concrete: Vec<_> = candidates
            .iter()
            .copied()
            .filter(|c| matches!(self.store.types.get(c.callable), Type::Method(_)))
            .collect();
        // Rank concrete formals first. Inferring B = Int for [B](b: B) makes
        // it applicable, but does not make its declaration as specific as (i: Int).
        let ordinary =
            self.choose_overload_candidate(&mut concrete, arguments, site, ApplyKind::Regular);
        let retry = match ordinary {
            Ok(best)
                if let Some(expected) = expected
                    && let Type::Method(method) = self.store.types.get(best.callable)
                    && !matches!(
                        self.store.types.get(method.result),
                        Type::Method(_) | Type::Poly(_)
                    )
                    && !self.contextual_conforms(method.result, expected, site, journal)? =>
            {
                Some((best, expected))
            }
            Err(TyperError::OverloadApplicationNoApplicable { .. }) => None,
            other => return other,
        };
        self.instantiate_generic_overload_candidates(
            candidates,
            arguments,
            site,
            ApplyKind::Regular,
            false,
            journal,
        )?;
        let Some((preferred, expected)) = retry else {
            return self.choose_overload_candidate(candidates, arguments, site, ApplyKind::Regular);
        };

        // Scala retries the other alternatives when the preferred result does
        // not fit. Keep the original choice if no unique replacement fits.
        let mut compatible = Vec::new();
        for candidate in candidates.iter().copied() {
            if candidate.symbol != preferred.symbol
                && candidate.rejection.is_none()
                && let Type::Method(method) = self.store.types.get(candidate.callable)
                && !matches!(
                    self.store.types.get(method.result),
                    Type::Method(_) | Type::Poly(_)
                )
                && self.contextual_conforms(method.result, expected, site, journal)?
            {
                compatible.push(candidate);
            }
        }
        concrete.retain(|c| compatible.iter().any(|alt| alt.symbol == c.symbol));
        let mut retry =
            self.choose_overload_candidate(&mut concrete, arguments, site, ApplyKind::Regular);
        if matches!(
            retry,
            Err(TyperError::OverloadApplicationNoApplicable { .. })
        ) {
            retry = self.choose_overload_candidate(
                &mut compatible,
                arguments,
                site,
                ApplyKind::Regular,
            );
        }
        match retry {
            Err(
                TyperError::OverloadApplicationNoApplicable { .. }
                | TyperError::AmbiguousOverloadApplication { .. },
            ) => Ok(preferred),
            other => other,
        }
    }

    fn extension_overload_candidates(
        &self,
        alternatives: &[ExtensionAlternative],
        tree_index: u32,
    ) -> Result<Vec<ApplicationCandidate>, TyperError> {
        let mut candidates = Vec::with_capacity(alternatives.len());
        for alt in alternatives {
            let Type::Method(receiver) = self.store.types.get(alt.signature) else {
                return Err(TyperError::MalformedOverloadCandidate {
                    symbol: alt.symbol,
                    callable: alt.signature,
                });
            };
            let callable = receiver.result;
            if !matches!(self.store.types.get(callable), Type::Method(_))
                && !self.is_unbounded_unary_extension(callable)
            {
                return Err(TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                    source: self.source,
                    tree_index,
                    candidate: alt.symbol,
                });
            }
            candidates.push(ApplicationCandidate {
                symbol: alt.symbol,
                callable,
                member: None,
                rejection: None,
            });
        }
        Ok(candidates)
    }

    pub(in crate::typer) fn is_unbounded_unary_extension(&self, callable: TypeId) -> bool {
        let types = &self.store.types;
        let Type::Poly(poly) = types.get(callable) else {
            return false;
        };
        let [parameter] = poly.params.as_slice() else {
            return false;
        };
        let Type::Method(method) = types.get(poly.result) else {
            return false;
        };
        let [arg] = method.params.as_slice() else {
            return false;
        };
        matches!(types.get(parameter.bounds), Type::Bounds { low, high }
            if *low == self.definitions.nothing_type && *high == self.definitions.any_type)
            && method.kind == MethodKind::Plain
            && !arg.erased
            && !arg.varargs
            && matches!(types.get(arg.ty), Type::ParamRef { binder, index: 0 } if *binder == callable)
            && !matches!(types.get(method.result), Type::Method(_) | Type::Poly(_))
    }

    fn extension_receiver(&self, signature: TypeId) -> Option<TypeId> {
        let Type::Method(method) = self.store.types.try_get(signature)? else {
            return None;
        };
        let [param] = method.params.as_slice() else {
            return None;
        };
        (method.kind == MethodKind::Plain).then_some(param.ty)
    }
}
