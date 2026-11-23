//! Bind written using arguments to the selected method clause.
use super::*;

pub(super) struct NamedArgument {
    pub(super) written: usize,
    pub(super) value: TreeId<Untyped>,
}

impl SourceTyper<'_> {
    pub(super) fn has_named_using_arguments(
        &self,
        kind: ApplyKind,
        trees: &[TreeId<Untyped>],
    ) -> bool {
        kind == ApplyKind::Using
            && trees.iter().any(|&t| {
                matches!(
                    self.arena.try_get(t).map(|t| &t.kind),
                    Some(TreeKind::NamedArg(_))
                )
            })
    }

    pub(super) fn named_using_arguments(
        &self,
        kind: ApplyKind,
        callable: TypeId,
        trees: &[TreeId<Untyped>],
        tree_index: u32,
    ) -> Result<Option<Vec<Option<NamedArgument>>>, TyperError> {
        if !self.has_named_using_arguments(kind, trees) {
            return Ok(None);
        }
        // Instantiation changes parameter types but preserves the clause's names
        // and order. Bind once and use the same slots for inference and application.
        let signature = match self.store.types.try_get(callable) {
            Some(Type::Poly(poly)) => poly.result,
            _ => callable,
        };
        let Some(Type::Method(method)) = self.store.types.try_get(signature) else {
            return Ok(None);
        };
        if !application_kind_accepts(kind, method.kind) {
            return Ok(None);
        }
        let ordinals: HashMap<_, _> = method
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| (*p.name.as_name(), i))
            .collect();
        let mut slots: Vec<_> = (0..method.params.len()).map(|_| None).collect();
        let mut next_unfilled = 0;
        for (written, &tree) in trees.iter().enumerate() {
            let node = self
                .arena
                .try_get(tree)
                .ok_or(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: tree.index(),
                })?;
            let (ordinal, value) = if let TreeKind::NamedArg(arg) = node.kind {
                let ordinal = ordinals
                    .get(&arg.name)
                    .ok_or(TyperError::UnknownNamedArgument {
                        source: self.source,
                        tree_index: tree.index(),
                        name: arg.name,
                    })?;
                (*ordinal, arg.arg)
            } else {
                if written != next_unfilled {
                    return Err(TyperError::PositionalAfterNamedArgument {
                        source: self.source,
                        tree_index: tree.index(),
                    });
                }
                (written, tree)
            };
            let Some(slot) = slots.get_mut(ordinal) else {
                return Err(TyperError::ApplicationArityMismatch {
                    source: self.source,
                    tree_index,
                    expected: method.params.len(),
                    actual: trees.len(),
                });
            };
            if slot.is_some() {
                return Err(TyperError::DuplicateNamedArgument {
                    source: self.source,
                    tree_index: tree.index(),
                    name: *method.params[ordinal].name.as_name(),
                });
            }
            *slot = Some(NamedArgument { written, value });
            while slots.get(next_unfilled).is_some_and(Option::is_some) {
                next_unfilled += 1;
            }
        }
        Ok(Some(slots))
    }
}

impl SourceTyper<'_> {
    // Types (including dependent formals) are checked in declaration order. Strict
    // operand initializers that must move are emitted in the original source order.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn order_named_operands(
        &mut self,
        call: TreeId<Typed>,
        slots: &[Option<NamedArgument>],
        method: &MethodType,
        shared: &mut Vec<TreeId<Typed>>,
        context: ExpressionContext,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<(), TyperError> {
        let TreeKind::Apply(mut app) = self.typed_arena.get(call).kind.clone() else {
            return Err(TyperError::UsingApplicationDeferred {
                source: self.source,
                tree_index,
            });
        };
        let mut written: Vec<_> = slots
            .iter()
            .enumerate()
            .filter_map(|(ordinal, a)| a.as_ref().map(|a| (a.written, ordinal)))
            .collect();
        written.sort_unstable();
        let mut initializers = Vec::new();
        for (_, ordinal) in written {
            if !matches!(
                self.store.types.get(method.params[ordinal].ty),
                Type::ByName { .. }
            ) {
                app.args[ordinal] = self.share_strict_operand(
                    app.args[ordinal],
                    &mut initializers,
                    context,
                    journal,
                )?;
            }
        }
        if initializers.is_empty() {
            return Ok(());
        }
        app.function =
            self.lift_named_callee(app.function, shared, context, tree_index, 0, journal)?;
        shared.extend(initializers);
        self.typed_arena.get_mut(call).kind = TreeKind::Apply(app);
        Ok(())
    }

    // The receiver and already-applied strict clauses execute before this clause.
    fn lift_named_callee(
        &mut self,
        tree: TreeId<Typed>,
        stats: &mut Vec<TreeId<Typed>>,
        context: ExpressionContext,
        tree_index: u32,
        depth: usize,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let deferred = TyperError::UsingApplicationDeferred {
            source: self.source,
            tree_index,
        };
        if depth >= crate::types::MAX_TYPE_NORMALIZATION_DEPTH {
            return Err(deferred);
        }
        let node = self.typed_arena.get(tree).clone();
        let kind = match node.kind {
            TreeKind::Ident(_) => return Ok(tree),
            TreeKind::Select(mut s) => {
                let receiver = self.share_strict_operand(s.qualifier, stats, context, journal)?;
                if receiver == s.qualifier {
                    return Ok(tree);
                }
                let sym = self
                    .store
                    .types
                    .get(node.ty)
                    .reference_symbol()
                    .ok_or(deferred)?;
                s.qualifier = receiver;
                let ty = self
                    .store
                    .types
                    .alloc(Type::term_ref(self.typed_arena.get(receiver).ty, sym));
                return Ok(self.typed_arena.alloc(Tree {
                    kind: TreeKind::Select(s),
                    ty,
                    ..node
                }));
            }
            TreeKind::TypeApply(mut a) => {
                a.function = self.lift_named_callee(
                    a.function,
                    stats,
                    context,
                    tree_index,
                    depth + 1,
                    journal,
                )?;
                TreeKind::TypeApply(a)
            }
            TreeKind::Apply(mut a) => {
                let applied = self.widen_expression_type_journaled(
                    self.typed_arena.get(a.function).ty,
                    journal,
                    0,
                )?;
                let Some(Type::Method(previous)) = self.store.types.try_get(applied).cloned()
                else {
                    return Err(deferred);
                };
                if previous.params.len() != a.args.len() {
                    return Err(deferred);
                }
                a.function = self.lift_named_callee(
                    a.function,
                    stats,
                    context,
                    tree_index,
                    depth + 1,
                    journal,
                )?;
                for (arg, p) in a.args.iter_mut().zip(previous.params) {
                    if !matches!(self.store.types.get(p.ty), Type::ByName { .. }) {
                        *arg = self.share_strict_operand(*arg, stats, context, journal)?;
                    }
                }
                TreeKind::Apply(a)
            }
            TreeKind::Block(b) => {
                stats.extend(b.stats);
                return self.lift_named_callee(
                    b.expr,
                    stats,
                    context,
                    tree_index,
                    depth + 1,
                    journal,
                );
            }
            _ => return Err(deferred),
        };
        Ok(self.typed_arena.alloc(Tree { kind, ..node }))
    }
}
