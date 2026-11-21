//! Extension methods contributed by a contextual value.

use super::*;
use std::cmp::Ordering;

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
    declaring_class: SymbolId,
    symbol: SymbolId,
    signature: TypeId,
}

// Owner order is nominal even when two candidates infer different provider arguments.
type ExtensionBases = HashMap<SymbolId, HashSet<SymbolId>>;

fn unique_extension_winner<T>(
    candidates: &[T],
    mut wins: impl FnMut(&T, &T) -> Result<bool, TyperError>,
) -> Result<Option<usize>, TyperError> {
    if candidates.is_empty() {
        return Ok(None);
    }
    let mut best = 0;
    for (i, candidate) in candidates.iter().enumerate().skip(1) {
        if wins(candidate, &candidates[best])? {
            best = i;
        }
    }
    for (i, candidate) in candidates.iter().enumerate() {
        if i != best && !wins(&candidates[best], candidate)? {
            return Ok(None);
        }
    }
    Ok(Some(best))
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
                .lookup_overload_members_journaled(result, name, journal)
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
                        .lookup_overload_members_journaled(provider.result, name, journal)
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
                    declaring_class: member.declaring_class,
                    provider,
                    symbol: member.symbol,
                    signature,
                });
            }
            let bases = self.extension_declaring_bases(&alternatives, journal)?;
            let selected = if alternatives.len() <= 1 {
                alternatives.pop()
            } else if let Some(index) =
                self.unique_extension_receiver(&alternatives, &bases, tree_index, journal)?
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
                    &bases,
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

    fn extension_declaring_bases(
        &mut self,
        alternatives: &[ExtensionAlternative],
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<ExtensionBases, TyperError> {
        let mut bases = ExtensionBases::new();
        if let Some((first, rest)) = alternatives.split_first()
            && rest
                .iter()
                .any(|alt| alt.declaring_class != first.declaring_class)
        {
            for alt in alternatives {
                if let std::collections::hash_map::Entry::Vacant(slot) =
                    bases.entry(alt.declaring_class)
                {
                    let ancestors = self.contextual_ancestors(alt.declaring_class, journal)?;
                    slot.insert(ancestors.into_iter().map(|(class, _)| class).collect());
                }
            }
        }
        Ok(bases)
    }

    fn extension_wins(
        &self,
        bases: &ExtensionBases,
        left: SymbolId,
        right: SymbolId,
        types: Ordering,
    ) -> bool {
        let derives = |a, b| {
            self.store
                .symbols
                .owner(a)
                .and_then(|owner| bases.get(&owner))
                .zip(self.store.symbols.owner(b))
                .is_some_and(|(parents, owner)| parents.contains(&owner))
        };
        // An advantage in declaring owner cancels a disadvantage in type
        // specificity. Only an unopposed advantage settles this clause.
        match types {
            Ordering::Greater => !derives(right, left),
            Ordering::Equal => derives(left, right),
            Ordering::Less => false,
        }
    }

    fn unique_extension_receiver(
        &mut self,
        alternatives: &[ExtensionAlternative],
        bases: &ExtensionBases,
        site: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<usize>, TyperError> {
        unique_extension_winner(alternatives, |left, right| {
            let forward = self.contextual_conforms(left.receiver, right.receiver, site, journal)?;
            let reverse = self.contextual_conforms(right.receiver, left.receiver, site, journal)?;
            Ok(self.extension_wins(bases, left.symbol, right.symbol, forward.cmp(&reverse)))
        })
    }

    fn choose_extension_overload(
        &mut self,
        alternatives: &[ExtensionAlternative],
        bases: &ExtensionBases,
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
        let mut resolution = if !bases.is_empty()
            || candidates
                .iter()
                .any(|c| matches!(self.store.types.get(c.callable), Type::Poly(_)))
        {
            self.choose_extension_by_specificity(
                &mut candidates,
                bases,
                arguments,
                call.expected_result,
                ApplyKind::Regular,
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
            resolution = self.choose_extension_by_specificity(
                &mut candidates,
                bases,
                &explicit,
                None,
                ApplyKind::Using,
                tree_index,
                journal,
            );
        }
        resolution.map(|candidate| candidate.symbol)
    }

    fn final_extension_result(&self, callable: TypeId) -> Option<TypeId> {
        let Type::Method(method) = self.store.types.get(callable) else {
            return None;
        };
        match self.store.types.get(method.result) {
            Type::Method(_) | Type::Poly(_) => None,
            _ => Some(method.result),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn choose_extension_by_specificity(
        &mut self,
        candidates: &mut [ApplicationCandidate],
        bases: &ExtensionBases,
        arguments: &[TypedArgument],
        expected: Option<TypeId>,
        kind: ApplyKind,
        site: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<ApplicationCandidate, TyperError> {
        let generic: HashSet<_> = candidates
            .iter()
            .filter(|c| matches!(self.store.types.get(c.callable), Type::Poly(_)))
            .map(|c| c.symbol)
            .collect();
        self.instantiate_generic_overload_candidates(
            candidates, arguments, site, kind, false, journal,
        )?;
        let mut applicable = Vec::new();
        let mut rejected = Vec::new();
        for candidate in candidates {
            match self.choose_overload_candidate(
                std::slice::from_mut(candidate),
                arguments,
                site,
                kind,
            ) {
                Ok(c) => applicable.push(c),
                Err(TyperError::OverloadApplicationNoApplicable { candidates, .. }) => {
                    rejected.extend(candidates)
                }
                Err(error) => return Err(error),
            }
        }
        if applicable.is_empty() {
            return Err(TyperError::OverloadApplicationNoApplicable {
                source: self.source,
                tree_index: site,
                candidates: rejected,
            });
        }
        let rank = |typer: &mut Self, alts: &[ApplicationCandidate]| {
            unique_extension_winner(alts, |a, b| {
                // An unbounded unary type parameter is less specific than a
                // concrete formal, even if inference made their types identical.
                let types = match generic
                    .contains(&b.symbol)
                    .cmp(&generic.contains(&a.symbol))
                {
                    Ordering::Equal => {
                        match typer.choose_overload_candidate(&mut [*a, *b], arguments, site, kind)
                        {
                            Ok(best) if best.symbol == a.symbol => Ordering::Greater,
                            Ok(_) => Ordering::Less,
                            Err(TyperError::AmbiguousOverloadApplication { .. }) => Ordering::Equal,
                            Err(error) => return Err(error),
                        }
                    }
                    order => order,
                };
                Ok(typer.extension_wins(bases, a.symbol, b.symbol, types))
            })
        };
        if let [only] = applicable.as_slice() {
            return Ok(*only);
        }
        let winner = rank(self, &applicable)?;
        // The expected result can break a tie or replace an incompatible
        // winner after this clause. Curried results still need their arguments.
        if let Some(expected) = expected
            && winner.map_or_else(
                || {
                    applicable
                        .iter()
                        .all(|c| self.final_extension_result(c.callable).is_some())
                },
                |i| {
                    self.final_extension_result(applicable[i].callable)
                        .is_some()
                },
            )
        {
            let mut compatible = Vec::new();
            for (i, c) in applicable.iter().enumerate() {
                if let Some(result) = self.final_extension_result(c.callable)
                    && self.contextual_conforms(result, expected, site, journal)?
                {
                    if winner == Some(i) {
                        return Ok(*c);
                    }
                    compatible.push(*c);
                }
            }
            if let Some(index) = rank(self, &compatible)? {
                return Ok(compatible[index]);
            }
        }
        winner
            .map(|i| applicable[i])
            .ok_or_else(|| TyperError::AmbiguousOverloadApplication {
                source: self.source,
                tree_index: site,
                candidates: applicable.iter().map(|c| c.symbol).collect(),
            })
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
        // Inference uses the ordinary argument. A final using clause is
        // instantiated with the method type arguments before witness search.
        let result = match types.get(method.result) {
            Type::Method(using) => {
                let [witness] = using.params.as_slice() else {
                    return false;
                };
                if using.kind != MethodKind::Contextual
                    || witness.erased
                    || witness.varargs
                    || matches!(types.get(witness.ty), Type::ByName { .. })
                {
                    return false;
                }
                using.result
            }
            _ => method.result,
        };
        matches!(types.get(parameter.bounds), Type::Bounds { low, high }
            if *low == self.definitions.nothing_type && *high == self.definitions.any_type)
            && method.kind == MethodKind::Plain
            && !arg.erased
            && !arg.varargs
            && matches!(types.get(arg.ty), Type::ParamRef { binder, index: 0 } if *binder == callable)
            && !matches!(types.get(result), Type::Method(_) | Type::Poly(_))
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
