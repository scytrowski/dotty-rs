//! Replay earlier argument clauses on the selected method's default getter.
use super::*;

pub(super) struct DefaultCall {
    root: TreeId<Typed>,
    applications: Vec<TreeId<Typed>>,
    getter: SymbolId,
}

impl SourceTyper<'_> {
    pub(super) fn default_call(
        &self,
        mut tree: TreeId<Typed>,
        ordinal: usize,
    ) -> Option<DefaultCall> {
        let mut applications = Vec::new();
        let mut clause = 0;
        loop {
            if applications.len() >= crate::types::MAX_TYPE_NORMALIZATION_DEPTH {
                return None;
            }
            let previous = match &self.typed_arena.get(tree).kind {
                TreeKind::Apply(app) => {
                    clause += 1;
                    app.function
                }
                TreeKind::TypeApply(app) => app.function,
                _ => break,
            };
            applications.push(tree);
            tree = previous;
        }
        let method = self
            .store
            .types
            .get(self.typed_arena.get(tree).ty)
            .reference_symbol()?;
        let SourceDefinition::Canonical {
            source,
            tree: def_tree,
        } = self.index.definition_of(method)?
        else {
            return None;
        };
        if source != self.source {
            return None;
        }
        let TreeKind::DefDef(def) = &self.arena.try_get(def_tree)?.kind else {
            return None;
        };
        let param = *self.context_bound_clauses(def).get(clause)?.get(ordinal)?;
        let getter =
            self.index
                .derived_symbol_at(self.store.symbols.owner(method)?, source, param)?;
        self.contextual_getter_source(getter)?;
        applications.reverse();
        Some(DefaultCall {
            root: tree,
            applications,
            getter,
        })
    }

    pub(super) fn apply_default_getter(
        &mut self,
        call: DefaultCall,
        function: &mut TreeId<Typed>,
        stats: &mut Vec<TreeId<Typed>>,
        context: ExpressionContext,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let deferred = || TyperError::UsingApplicationDeferred {
            source: self.source,
            tree_index,
        };
        let original = self.typed_arena.get(call.root).clone();
        let method = self
            .store
            .types
            .get(original.ty)
            .reference_symbol()
            .ok_or_else(deferred)?;
        let name = self.store.symbols.get(call.getter).name;
        let mut target = call.root;
        let mut getter = match original.kind {
            TreeKind::Select(sel) => {
                let receiver =
                    self.share_default_operand(sel.qualifier, stats, context, journal)?;
                let prefix = self.typed_arena.get(receiver).ty;
                let getter_type = self.store.types.alloc(Type::term_ref(prefix, call.getter));
                if receiver != sel.qualifier {
                    let method_type = self.store.types.alloc(Type::term_ref(prefix, method));
                    target = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).select(
                        receiver,
                        sel.name,
                        sel.backquoted,
                        method_type,
                        original.position,
                    );
                }
                TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).select(
                    receiver,
                    name,
                    false,
                    getter_type,
                    original.position,
                )
            }
            TreeKind::Ident(_) => self.type_resolved_identifier_expression(
                call.getter,
                Ident {
                    name,
                    backquoted: false,
                },
                original.position,
                context,
                tree_index,
                journal,
            )?,
            _ => return Err(deferred()),
        };
        let mut signature =
            self.widen_expression_type_journaled(self.typed_arena.get(getter).ty, journal, 0)?;
        for tree in call.applications {
            let node = self.typed_arena.get(tree).clone();
            match node.kind {
                TreeKind::TypeApply(app) => {
                    let args = app
                        .args
                        .iter()
                        .map(|t| self.typed_arena.get(*t).ty)
                        .collect::<Vec<_>>();
                    signature = dotty_core::types::instantiate_poly(self.store, signature, &args)
                        .map_err(TyperError::PolyInstantiation)?
                        .result;
                    let mut b = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types);
                    if target != app.function {
                        target = b.type_apply(target, app.args.clone(), node.ty, node.position)
                    } else {
                        target = tree
                    }
                    getter = b.type_apply(getter, app.args, signature, node.position);
                }
                TreeKind::Apply(app) => {
                    let Some(Type::Method(previous)) = self.store.types.try_get(signature).cloned()
                    else {
                        return Err(TyperError::UsingApplicationDeferred {
                            source: self.source,
                            tree_index,
                        });
                    };
                    if previous.params.len() != app.args.len()
                        || self.type_contains_param_ref(previous.result, signature)?
                    {
                        return Err(TyperError::UsingApplicationDeferred {
                            source: self.source,
                            tree_index,
                        });
                    }
                    let mut shared = Vec::with_capacity(app.args.len());
                    for (&arg, param) in app.args.iter().zip(previous.params) {
                        shared.push(
                            if matches!(self.store.types.get(param.ty), Type::ByName { .. }) {
                                arg
                            } else {
                                self.share_default_operand(arg, stats, context, journal)?
                            },
                        );
                    }
                    signature = previous.result;
                    let mut b = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types);
                    if target != app.function || shared != app.args {
                        target = b.apply_with_kind(
                            target,
                            shared.clone(),
                            app.kind,
                            node.ty,
                            node.position,
                        )
                    } else {
                        target = tree
                    }
                    getter = b.apply_with_kind(getter, shared, app.kind, signature, node.position);
                }
                _ => {
                    return Err(TyperError::UsingApplicationDeferred {
                        source: self.source,
                        tree_index,
                    });
                }
            }
        }
        *function = target;
        Ok(getter)
    }

    fn share_default_operand(
        &mut self,
        tree: TreeId<Typed>,
        stats: &mut Vec<TreeId<Typed>>,
        context: ExpressionContext,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let node = self.typed_arena.get(tree);
        let ty = node.ty;
        if matches!(node.kind, TreeKind::This(_) | TreeKind::Literal(_)) {
            return Ok(tree);
        }
        if matches!(&node.kind, TreeKind::Ident(_))
            && let Some(id) = self.store.types.get(ty).reference_symbol()
            && self.store.symbols.contains(id)
        {
            let s = self.store.symbols.get(id);
            if matches!(
                s.kind,
                SymbolKind::Parameter | SymbolKind::Local | SymbolKind::Object
            ) && !s.flags.contains(SymbolFlags::MUTABLE)
                && !s.flags.contains(SymbolFlags::LAZY)
                && !matches!(s.info,SymbolInfo::Complete(t) if matches!(self.store.types.try_get(t),Some(Type::ByName{..})))
            {
                return Ok(tree);
            }
        }
        let position = node.position;
        let value_type = self.widen_expression_type_journaled(ty, journal, 0)?;
        let name = dotty_core::TermName::new(self.store.names.intern("defaultArg$"));
        let symbol = self.store.symbols.alloc(dotty_core::Symbol {
            name: *name.as_name(),
            owner: Some(context.owner),
            kind: SymbolKind::Local,
            flags: SymbolFlags::SYNTHETIC,
            visibility: Visibility::Private,
            info: SymbolInfo::Complete(value_type),
            origin: SymbolOrigin::Synthetic,
            position,
            annotations: Vec::new(),
            links: Default::default(),
        });
        let tpt = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
            .type_tree(value_type, None);
        let definition = self.typed_index.alloc_synthetic_definition(
            &mut self.typed_arena,
            symbol,
            Tree {
                ty: value_type,
                position,
                kind: TreeKind::ValDef(dotty_core::ast::ValDef {
                    name,
                    tpt,
                    rhs: Some(tree),
                    metadata: (),
                }),
            },
        );
        stats.push(definition);
        let reference = self
            .store
            .types
            .alloc(Type::term_ref(self.definitions.no_prefix, symbol));
        Ok(
            TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).ident(
                *name.as_name(),
                reference,
                None,
            ),
        )
    }
}
