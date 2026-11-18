//! Stable paths and nominal companions contributing to implicit scope.

use super::*;

impl SourceTyper<'_> {
    /// Search allocates fresh TypeIds for the same path during independent
    /// probes. The declaration and all of its enclosing selections identify a
    /// candidate for duplicate elimination and recursion detection.
    pub(super) fn same_contextual_ref(&self, left: ContextualRef, right: ContextualRef) -> bool {
        if left == right {
            return true;
        }
        if left.symbol != right.symbol {
            return false;
        }
        let path = |receiver| match receiver {
            Some(ContextualReceiver::Path(path)) => Some(path),
            Some(ContextualReceiver::Import(tree)) => Some(self.typed_arena.get(tree).ty),
            _ => None,
        };
        let (Some(mut left), Some(mut right)) = (path(left.receiver), path(right.receiver)) else {
            return false;
        };
        for _ in 0..crate::types::MAX_TYPE_NORMALIZATION_DEPTH {
            if left == right {
                return true;
            }
            match (
                self.store.types.try_get(left),
                self.store.types.try_get(right),
            ) {
                (
                    Some(Type::TermRef {
                        prefix: l,
                        target: lt,
                    }),
                    Some(Type::TermRef {
                        prefix: r,
                        target: rt,
                    }),
                ) if lt == rt => {
                    left = *l;
                    right = *r;
                }
                (Some(Type::ThisType { class: l }), Some(Type::ThisType { class: r })) => {
                    return l == r;
                }
                (Some(Type::NoPrefix), Some(Type::NoPrefix)) => return true,
                _ => return false,
            }
        }
        false
    }

    fn collect_public_contextual_members(
        &mut self,
        class: SymbolId,
        receiver: ContextualReceiver,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
        refs: &mut Vec<ContextualRef>,
    ) -> Result<(), TyperError> {
        let scope = self
            .class_info(class, journal)
            .map_err(|error| TyperError::MemberLookup(Box::new(error)))?
            .declarations;
        for symbol in self.contextual_members(scope, journal)? {
            let reference = ContextualRef {
                symbol,
                receiver: Some(receiver),
            };
            if self.store.symbols.get(symbol).visibility == Visibility::Public
                && !refs
                    .iter()
                    .any(|&prior| self.same_contextual_ref(prior, reference))
            {
                refs.push(reference);
            }
        }
        Ok(())
    }

    // Source signatures use the defining module's this type for static members.
    // Recover its canonical module value when building an external selection.
    fn contextual_anchor_path(
        &mut self,
        prefix: TypeId,
        tree_index: u32,
    ) -> Result<Option<TypeId>, TyperError> {
        match self.store.types.try_get(prefix) {
            // Classpath types may use a term reference for their package prefix.
            // A package qualifies the type but contributes no instance members.
            Some(Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            }) if self.store.symbols.contains(*symbol)
                && self.store.symbols.get(*symbol).kind == SymbolKind::Package =>
            {
                Ok(None)
            }
            Some(Type::TermRef { .. }) => {
                self.require_stable_selection_prefix(prefix, tree_index)?;
                Ok(Some(prefix))
            }
            Some(Type::ThisType { class })
                if self.store.symbols.contains(*class)
                    && self.store.symbols.get(*class).kind == SymbolKind::ModuleClass =>
            {
                let mut modules = Vec::new();
                let mut class = *class;
                loop {
                    let Some(SourceDefinition::Derived { source, tree }) =
                        self.index.definition_of(class)
                    else {
                        return Ok(None);
                    };
                    let Some(object) = self.index.symbol_at(source, tree) else {
                        return Ok(None);
                    };
                    if self.source_module_class_of_object(object).ok() != Some(class) {
                        return Ok(None);
                    }
                    modules.push(object);
                    let Some(owner) = self.store.symbols.get(class).owner else {
                        return Ok(None);
                    };
                    if !self.store.symbols.contains(owner) {
                        return Err(TyperError::UnknownSymbol { symbol: owner });
                    }
                    match self.store.symbols.get(owner).kind {
                        SymbolKind::Package => break,
                        SymbolKind::ModuleClass
                            if modules.len() < crate::types::MAX_TYPE_NORMALIZATION_DEPTH =>
                        {
                            class = owner
                        }
                        _ => return Ok(None),
                    }
                }
                let mut path = self.definitions.no_prefix;
                for object in modules.into_iter().rev() {
                    path = self.store.types.alloc(Type::TermRef {
                        prefix: path,
                        target: TermRefTarget::Symbol(object),
                    });
                }
                Ok(Some(path))
            }
            _ => Ok(None),
        }
    }

    pub(super) fn contextual_path_tree(
        &mut self,
        path: TypeId,
        position: Option<SourceSpan>,
        tree_index: u32,
    ) -> Result<TreeId<Typed>, TyperError> {
        let mut segments = Vec::new();
        let mut ty = path;
        while let Some(Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(symbol),
        }) = self.store.types.try_get(ty)
        {
            if segments.len() >= crate::types::MAX_TYPE_NORMALIZATION_DEPTH {
                return Err(TyperError::TypeNormalization(
                    crate::types::TypeNormalizeError::TooDeep,
                ));
            }
            self.require_stable_selection_prefix(ty, tree_index)?;
            segments.push((ty, self.store.symbols.get(*symbol).name));
            ty = *prefix;
        }
        let Some((root, name)) = segments.pop() else {
            return Err(TyperError::UnstableSelectionPrefix {
                source: self.source,
                tree_index,
                qualifier_type: path,
            });
        };
        let mut builder = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types);
        let mut tree = builder.ident(name, root, position);
        for (ty, name) in segments.into_iter().rev() {
            tree = builder.select(tree, name, false, ty, position);
        }
        Ok(tree)
    }

    /// Collect nominal anchors and their parent scopes only after lexical search
    /// has no winner. Each companion contributes a stable term prefix.
    pub(super) fn companion_refs(
        &mut self,
        expected: TypeId,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Vec<ContextualRef>, TyperError> {
        let mut pending = vec![expected];
        let mut seen = HashSet::new();
        let mut classes = HashSet::new();
        let mut refs = Vec::new();
        while let Some(ty) = pending.pop() {
            if !seen.insert(ty) {
                continue;
            }
            if seen.len() > MAX_TYPE_RELATION_VIEWS {
                return Err(TyperError::TypeNormalization(
                    crate::types::TypeNormalizeError::TooDeep,
                ));
            }
            let head = match self.store.types.try_get(ty) {
                Some(Type::Applied { tycon, .. }) => *tycon,
                _ => ty,
            };
            if let Some(Type::TypeRef {
                target: TypeRefTarget::Symbol(alias),
                ..
            }) = self.store.types.try_get(head)
                && self.store.symbols.contains(*alias)
                && self.store.symbols.get(*alias).kind == SymbolKind::TypeAlias
                && !self
                    .store
                    .symbols
                    .get(*alias)
                    .flags
                    .contains(SymbolFlags::OPAQUE)
            {
                self.complete_relation_type(ty, journal, &mut HashSet::new(), 0)?;
                pending.push(
                    crate::types::TypeNormalizer::new(self.store)
                        .normalize_for_relation(ty)
                        .map_err(TyperError::TypeNormalization)?,
                );
                continue;
            }
            match self.store.types.try_get(ty).cloned() {
                Some(Type::Applied { tycon, args }) => {
                    pending.extend(args);
                    pending.push(tycon);
                }
                Some(Type::TypeRef {
                    target: TypeRefTarget::Symbol(class),
                    prefix,
                }) if self.store.symbols.contains(class) => {
                    let path = self.contextual_anchor_path(prefix, tree_index)?;
                    if let Some(path) = path {
                        let receiver = self.widen_expression_type_journaled(path, journal, 0)?;
                        let constructor = match self.store.types.try_get(receiver) {
                            Some(Type::Applied { tycon, .. }) => *tycon,
                            _ => receiver,
                        };
                        if let Some(Type::TypeRef {
                            target: TypeRefTarget::Symbol(owner),
                            ..
                        }) = self.store.types.try_get(constructor)
                        {
                            let owner = *owner;
                            self.collect_public_contextual_members(
                                owner,
                                ContextualReceiver::Path(path),
                                journal,
                                &mut refs,
                            )?;
                        }
                    }
                    let entry = self.store.symbols.get(class);
                    let opaque = entry.kind == SymbolKind::TypeAlias
                        && entry.flags.contains(SymbolFlags::OPAQUE);
                    if !(opaque
                        || matches!(
                            entry.kind,
                            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
                        ))
                        || !classes.insert((class, path))
                        || matches!(entry.origin, SymbolOrigin::Builtin)
                    {
                        continue;
                    }
                    if !opaque {
                        let info = self
                            .class_info(class, journal)
                            .map_err(|e| TyperError::MemberLookup(Box::new(e)))?;
                        pending.extend(info.parents);
                    }
                    let Some(object) = self.contextual_companion(class, tree_index)? else {
                        continue;
                    };
                    let module = self.module_class_of_object(object)?;
                    let receiver = if let Some(prefix) = path {
                        ContextualReceiver::Path(self.store.types.alloc(Type::TermRef {
                            prefix,
                            target: TermRefTarget::Symbol(object),
                        }))
                    } else {
                        ContextualReceiver::Companion(object)
                    };
                    self.collect_public_contextual_members(module, receiver, journal, &mut refs)?;
                }
                Some(Type::And { left, right } | Type::Or { left, right }) => {
                    pending.extend([left, right])
                }
                Some(Type::AliasingBounds { alias }) => pending.push(alias),
                _ => {}
            }
        }
        Ok(refs)
    }
}
