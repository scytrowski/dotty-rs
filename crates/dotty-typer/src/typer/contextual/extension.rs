//! Extension methods contributed by a contextual value.

use super::*;

impl SourceTyper<'_> {
    /// Select an extension on a contextual receiver after ordinary member
    /// lookup has reported that the call has no applicable nominal member.
    pub(in crate::typer) fn contextual_extension(
        &mut self,
        name: Name,
        receiver: TypeId,
        context: ExpressionContext,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<(TreeId<Typed>, TypeId)>, TyperError> {
        for level in self.contextual_scopes(context, tree_index)? {
            let refs = self.contextual_scope_refs(level, tree_index, journal)?;
            if let Some(method) =
                self.extension_from_refs(refs, name, receiver, context, tree_index, journal)?
            {
                return Ok(Some(method));
            }
        }
        let Some(refs) = self.adaptation_companion_refs(receiver, tree_index, journal)? else {
            return Ok(None);
        };
        self.extension_from_refs(refs, name, receiver, context, tree_index, journal)
    }

    fn extension_from_refs(
        &mut self,
        refs: Vec<ContextualRef>,
        name: Name,
        receiver: TypeId,
        context: ExpressionContext,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<(TreeId<Typed>, TypeId)>, TyperError> {
        let mut candidates = Vec::new();

        for evidence in refs {
            if self.initializing_local_symbols.contains(&evidence.symbol) {
                continue;
            }
            let view =
                self.contextual_reference_signature(evidence, context.owner, tree_index, journal)?;
            let tycon = match self.store.types.try_get(view) {
                Some(Type::Applied { tycon, .. }) => *tycon,
                _ => view,
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
                .lookup_members_journaled(view, name, journal)
                .map_err(|error| TyperError::MemberLookup(Box::new(error)))?;
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
                let Some(Type::Method(first)) = self.store.types.try_get(signature) else {
                    continue;
                };
                if first.kind != MethodKind::Plain || first.params.len() != 1 {
                    continue;
                }
                let expected_receiver = first.params[0].ty;
                if !self.contextual_conforms(receiver, expected_receiver, tree_index, journal)? {
                    continue;
                }
                if candidates
                    .iter()
                    .any(|(p, _): &(Provider, _)| p.reference == evidence)
                {
                    return Err(TyperError::OverloadedSelectionDeferred {
                        source: self.source,
                        tree_index,
                        name,
                    });
                }
                candidates.push((
                    Provider {
                        reference: evidence,
                        type_arguments: Vec::new(),
                        callable: view,
                        result: view,
                    },
                    Ok((member.symbol, signature)),
                ));
            }
        }
        let Some((provider, (symbol, signature))) =
            self.choose_contextual_candidate(candidates, receiver, tree_index, journal)?
        else {
            return Ok(None);
        };
        let qualifier =
            self.contextual_reference_tree(provider.reference, context, tree_index, None, journal)?;
        let ty = self.store.types.alloc(Type::TermRef {
            prefix: self.typed_arena.get(qualifier).ty,
            target: TermRefTarget::Symbol(symbol),
        });
        let function = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
            .select(qualifier, name, false, ty, None);
        Ok(Some((function, signature)))
    }
}
