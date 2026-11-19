//! Scope-sensitive representations of source opaque aliases during evidence search.

use super::*;

impl SourceTyper<'_> {
    /// Only the defining lexical owner can see an opaque declaration's RHS.
    /// Public signatures retain the nominal reference, even after such a search.
    pub(super) fn contextual_type_at_owner(
        &mut self,
        ty: TypeId,
        owner: SymbolId,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        self.contextual_type_at_owner_inner(ty, owner, journal, 0, &mut HashMap::new())
    }

    fn contextual_type_at_owner_inner(
        &mut self,
        ty: TypeId,
        owner: SymbolId,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
        depth: usize,
        memo: &mut HashMap<TypeId, TypeId>,
    ) -> Result<TypeId, TyperError> {
        if let Some(&viewed) = memo.get(&ty) {
            return Ok(viewed);
        }
        if depth >= crate::types::MAX_TYPE_NORMALIZATION_DEPTH
            || memo.len() >= MAX_TYPE_RELATION_VIEWS
        {
            return Err(TyperError::TypeNormalization(
                crate::types::TypeNormalizeError::TooDeep,
            ));
        }
        let viewed = match self.store.types.try_get(ty).cloned() {
            Some(Type::Applied { tycon, args }) => {
                let mut viewed = Vec::with_capacity(args.len());
                for &arg in &args {
                    viewed.push(self.contextual_type_at_owner_inner(
                        arg,
                        owner,
                        journal,
                        depth + 1,
                        memo,
                    )?);
                }
                if args == viewed {
                    Ok(ty)
                } else {
                    Ok(self.store.types.alloc(Type::Applied {
                        tycon,
                        args: viewed,
                    }))
                }
            }
            Some(Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                prefix,
            }) if self.store.symbols.contains(symbol) => {
                let declaration = self.store.symbols.get(symbol);
                if declaration.kind != SymbolKind::TypeAlias
                    || !declaration.flags.contains(SymbolFlags::OPAQUE)
                {
                    return Ok(ty);
                }
                let Some(defining) = declaration.owner else {
                    return Ok(ty);
                };
                if !matches!(self.store.types.try_get(prefix),Some(Type::ThisType {class}) if *class == defining)
                {
                    return Ok(ty);
                }
                let mut lexical = Some(owner);
                let mut seen = HashSet::new();
                while let Some(current) = lexical {
                    if current == defining {
                        break;
                    }
                    if !self.store.symbols.contains(current) {
                        return Err(TyperError::UnknownSymbol { symbol: current });
                    }
                    if !seen.insert(current) {
                        return Err(TyperError::ThisOwnerCycle {
                            source: self.source,
                            owner: current,
                        });
                    }
                    lexical = self.store.symbols.get(current).owner;
                }
                if lexical.is_none() {
                    return Ok(ty);
                }
                let Some(SourceDefinition::Canonical { source, tree }) =
                    self.index.definition_of(symbol)
                else {
                    return Ok(ty);
                };
                if source != self.source {
                    return Ok(ty);
                }
                let Some(TreeKind::TypeDef(definition)) = self.arena.try_get(tree).map(|t| &t.kind)
                else {
                    return Ok(ty);
                };
                let rhs = match self.arena.try_get(definition.rhs).map(|n| &n.kind) {
                    Some(TreeKind::TypeBoundsTree(bounds)) => {
                        bounds.alias.ok_or(TyperError::OpaqueAliasDeferred {
                            symbol,
                            tree_index: tree.index(),
                        })?
                    }
                    _ => definition.rhs,
                };
                let context = self
                    .index
                    .declaration_context_of(symbol)
                    .ok_or(TyperError::DeclarationContextMissing { symbol })?;
                let target = self.type_of_tpt_inner_journaled(rhs, context, journal)?;
                self.contextual_type_at_owner_inner(target, owner, journal, depth + 1, memo)
            }
            _ => Ok(ty),
        }?;
        memo.insert(ty, viewed);
        Ok(viewed)
    }
}
