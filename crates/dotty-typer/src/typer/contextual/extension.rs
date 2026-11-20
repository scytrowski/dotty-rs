//! Extension methods contributed by a contextual value.

use super::*;

pub(in crate::typer) enum ExtensionLookup {
    Selected(TreeId<Typed>, TypeId),
    Missing(Option<TyperError>),
}

pub(in crate::typer) struct ExtensionCall<'a> {
    pub arguments: &'a [TreeId<Untyped>],
    pub typed_arguments: &'a mut Option<Vec<TypedArgument>>,
    pub mappings: &'a mut Vec<(SourceId, TreeId<Untyped>)>,
}

struct ExtensionAlternative {
    provider: Provider,
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
                    provider,
                    symbol: member.symbol,
                    signature,
                });
            }
            if alternatives.len() > 1 {
                let Some(call) = call.as_deref_mut() else {
                    return Err(TyperError::OverloadedSelectionDeferred {
                        source: self.source,
                        tree_index,
                        name,
                    });
                };
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
                let winner =
                    match self.choose_extension_overload(&alternatives, arguments, tree_index) {
                        Ok(symbol) => symbol,
                        Err(
                            cause @ (TyperError::OverloadApplicationNoApplicable { .. }
                            | TyperError::AmbiguousOverloadApplication { .. }),
                        ) => {
                            search.dependency_failure.get_or_insert(
                                TyperError::ExtensionOverload {
                                    source: self.source,
                                    tree_index,
                                    name,
                                    provider: evidence.symbol,
                                    cause: Box::new(cause),
                                },
                            );
                            continue;
                        }
                        Err(error) => return Err(error),
                    };
                alternatives.retain(|alt| alt.symbol == winner);
            }
            if let Some(alt) = alternatives.pop() {
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

    fn choose_extension_overload(
        &mut self,
        alternatives: &[ExtensionAlternative],
        arguments: &[TypedArgument],
        tree_index: u32,
    ) -> Result<SymbolId, TyperError> {
        let mut candidates = Vec::with_capacity(alternatives.len());
        for alt in alternatives {
            let Type::Method(receiver) = self.store.types.get(alt.signature) else {
                return Err(TyperError::MalformedOverloadCandidate {
                    symbol: alt.symbol,
                    callable: alt.signature,
                });
            };
            let callable = receiver.result;
            if !matches!(self.store.types.get(callable), Type::Method(_)) {
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
        self.choose_overload_candidate(&mut candidates, arguments, tree_index, ApplyKind::Regular)
            .map(|candidate| candidate.symbol)
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
