use super::*;

impl Validator<'_, '_> {
    pub(super) fn validate_block_expression(
        &mut self,
        source_id: TreeId<Untyped>,
        typed_id: TreeId<Typed>,
        method: &MethodPicklePlan,
        result_type: TypeId,
        depth: usize,
        visited: &mut HashSet<TreeId<Typed>>,
    ) -> Result<(), PickleError> {
        let visible_locals = self.visible_locals.clone();
        let result = self.validate_block_expression_inner(
            source_id,
            typed_id,
            method,
            result_type,
            depth,
            visited,
        );
        self.visible_locals = visible_locals;
        result
    }

    fn validate_block_expression_inner(
        &mut self,
        source_id: TreeId<Untyped>,
        typed_id: TreeId<Typed>,
        method: &MethodPicklePlan,
        result_type: TypeId,
        depth: usize,
        visited: &mut HashSet<TreeId<Typed>>,
    ) -> Result<(), PickleError> {
        let depth_limit = self.limits.expression_depth_limit();
        if depth > depth_limit {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::ExpressionDepth,
                limit: depth_limit,
            });
        }
        self.charge(1)?;
        if !visited.insert(typed_id) {
            return Err(PickleError::RepeatedExpression { tree: typed_id });
        }

        let source = self.source_tree(source_id)?;
        let typed = self
            .plan
            .typed_ast
            .try_get(typed_id)
            .ok_or(PickleError::InvalidTypedTree { tree: typed_id })?;
        self.source_typed_binding(source_id, typed_id)?;
        self.paired_source_typed_span(source.position, typed.position, typed_id)?;
        let (TreeKind::Block(source_block), TreeKind::Block(typed_block)) =
            (&source.kind, &typed.kind)
        else {
            return Err(PickleError::SourceShape {
                tree: source_id,
                expected: SourceRole::Body,
            });
        };
        let source_block = source_block.clone();
        let typed_block = typed_block.clone();
        if source_block.stats.len() != typed_block.stats.len() {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        }
        let typed_result =
            self.plan
                .typed_ast
                .try_get(typed_block.expr)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: typed_block.expr,
                })?;
        if typed.ty != typed_result.ty {
            return Err(PickleError::BodyType {
                tree: typed_id,
                ty: typed.ty,
            });
        }
        let reference_result = result_type != self.plan.definitions.int;
        if reference_result && !self.is_planned_sibling_class_type(result_type, None) {
            return Err(PickleError::BodyType {
                tree: typed_id,
                ty: typed.ty,
            });
        }
        if reference_result {
            let Type::TypeRef {
                target: TypeRefTarget::Symbol(result_class),
                ..
            } = self.ty(result_type)?
            else {
                return Err(PickleError::BodyType {
                    tree: typed_id,
                    ty: typed.ty,
                });
            };
            let result_class = *result_class;
            let same_class_type = |ty: TypeId| {
                matches!(self.plan.store.types.try_get(ty), Some(Type::TypeRef {
                    target: TypeRefTarget::Symbol(class), ..
                }) if *class == result_class)
            };
            if !same_class_type(typed.ty) || !same_class_type(typed_result.ty) {
                return Err(PickleError::BodyType {
                    tree: typed_id,
                    ty: typed.ty,
                });
            }
            let call_binding = self
                .plan
                .typed_bindings
                .local_receiver_unit_call(self.plan.source.id, method.symbol);
            let three_stat_shape = source_block.stats.len() == 3
                && call_binding.is_some()
                && source_block.stats[..2]
                    .iter()
                    .zip(&typed_block.stats[..2])
                    .all(|(&source_stat, &typed_stat)| {
                        matches!(
                            self.source_tree(source_stat).map(|tree| &tree.kind),
                            Ok(TreeKind::ValDef(_))
                        ) && matches!(
                            self.plan
                                .typed_ast
                                .try_get(typed_stat)
                                .map(|tree| &tree.kind),
                            Some(TreeKind::ValDef(_))
                        )
                    })
                && matches!(
                    self.source_tree(source_block.stats[2])?.kind,
                    TreeKind::Apply(_)
                )
                && matches!(
                    self.plan
                        .typed_ast
                        .try_get(typed_block.stats[2])
                        .map(|tree| &tree.kind),
                    Some(TreeKind::Apply(_))
                );
            let local_stat_shape = (1..=2).contains(&source_block.stats.len())
                && source_block.stats.iter().zip(&typed_block.stats).all(
                    |(&source_stat, &typed_stat)| {
                        matches!(
                            self.source_tree(source_stat).map(|tree| &tree.kind),
                            Ok(TreeKind::ValDef(_))
                        ) && matches!(
                            self.plan
                                .typed_ast
                                .try_get(typed_stat)
                                .map(|tree| &tree.kind),
                            Some(TreeKind::ValDef(_))
                        )
                    },
                );
            let exact_shape = (local_stat_shape || three_stat_shape)
                && matches!(
                    self.source_tree(source_block.expr)?.kind,
                    TreeKind::Apply(_)
                )
                && matches!(&typed_result.kind, TreeKind::Apply(_));
            if !exact_shape {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                );
            }
            let TreeKind::Apply(source_result_apply) = &self.source_tree(source_block.expr)?.kind
            else {
                unreachable!("reference block shape was checked")
            };
            let pair = self
                .plan
                .typed_bindings
                .formal_local_constructor_call_pair(self.plan.source.id, method.symbol);
            let local_argument_indices = pair
                .map(|pair| {
                    pair.arguments
                        .iter()
                        .enumerate()
                        .filter_map(|(index, argument)| {
                            matches!(
                                argument.origin,
                                crate::FormalLocalConstructorCallArgumentOrigin::LocalValue { .. }
                            )
                            .then_some(index)
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if !matches!(
                (
                    source_result_apply.args.len(),
                    local_argument_indices.first()
                ),
                (1, None) | (2, Some(_))
            ) {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                );
            }
            let initializers = self
                .plan
                .typed_bindings
                .local_counter_initializers(self.plan.source.id, method.symbol);
            let distinct_local_pair_matches_initializers = pair.is_some_and(|pair| {
                let local_symbols = pair.arguments.map(|argument| match argument.origin {
                    crate::FormalLocalConstructorCallArgumentOrigin::LocalValue {
                        symbol, ..
                    } => Some(symbol),
                    crate::FormalLocalConstructorCallArgumentOrigin::Formal { .. } => None,
                });
                matches!(local_symbols, [Some(first), Some(second)]
                    if first != second
                        && initializers.len() == 2
                        && initializers.iter().any(|initializer| initializer.local == first)
                        && initializers.iter().any(|initializer| initializer.local == second))
            });
            let two_local_shape = (source_block.stats.len() == 2 || three_stat_shape)
                && ((source_block.stats.len() == 2 && call_binding.is_none())
                    || (source_block.stats.len() == 3 && call_binding.is_some()))
                && (source_block.stats.len() == 2 || distinct_local_pair_matches_initializers)
                && local_argument_indices.len() == 2
                && initializers.len() == 2
                && initializers[0].local != initializers[1].local
                && initializers[0].scope == initializers[1].scope
                && source_block
                    .stats
                    .iter()
                    .take(2)
                    .zip(&typed_block.stats)
                    .zip(&initializers)
                    .all(|((&source_stat, &typed_stat), initializer)| {
                        initializer.source_block == source_id
                            && initializer.typed_block == typed_id
                            && initializer.source_declaration == source_stat
                            && initializer.typed_declaration == typed_stat
                    });
            let one_local_shape = source_block.stats.len() == 1
                && initializers.len() <= 1
                && initializers.iter().all(|initializer| {
                    initializer.source_block == source_id
                        && initializer.typed_block == typed_id
                        && initializer.source_declaration == source_block.stats[0]
                        && initializer.typed_declaration == typed_block.stats[0]
                })
                && self.source_tree(source_block.stats[0])?.position.is_some();
            if !(two_local_shape || one_local_shape) {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                );
            }
            let use_index = local_argument_indices.first().copied().unwrap_or(0);
            let source_use = self.source_tree(source_result_apply.args[use_index])?;
            let typed_result_apply = match &typed_result.kind {
                TreeKind::Apply(application)
                    if application.args.len() == source_result_apply.args.len() =>
                {
                    application
                }
                _ => {
                    return Err(
                        self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                    );
                }
            };
            if two_local_shape {
                let pair = pair.expect("two-local shape has a pair receipt");
                for index in 0..2 {
                    let argument = pair.arguments[index];
                    if argument.source_argument != source_result_apply.args[index]
                        || argument.typed_argument != typed_result_apply.args[index]
                    {
                        return Err(self
                            .unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
                    }
                }
            }
            let typed_use = self
                .plan
                .typed_ast
                .try_get(typed_result_apply.args[use_index])
                .ok_or(PickleError::InvalidTypedTree {
                    tree: typed_result_apply.args[use_index],
                })?;
            let (TreeKind::Ident(_), Some(declaration_span), Some(use_span)) = (
                &source_use.kind,
                self.source_tree(source_block.stats[0])?.position,
                source_use.position,
            ) else {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                );
            };
            if declaration_span.span().range().end() > use_span.span().range().start()
                || source.position.is_none()
                || typed.position.is_none()
                || self.source_tree(source_block.expr)?.position.is_none()
                || typed_result.position.is_none()
                || typed_use.position.is_none()
            {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                );
            }
        }

        let unit_call = self
            .plan
            .typed_bindings
            .local_receiver_unit_call(self.plan.source.id, method.symbol);
        for (index, (&source_stat, &typed_stat)) in source_block
            .stats
            .iter()
            .zip(&typed_block.stats)
            .enumerate()
        {
            if visited.contains(&typed_stat) {
                return Err(PickleError::RepeatedExpression { tree: typed_stat });
            }
            let source_stat_node = self.source_tree(source_stat)?;
            let stat = self
                .plan
                .typed_ast
                .try_get(typed_stat)
                .ok_or(PickleError::InvalidTypedTree { tree: typed_stat })?;
            if matches!(source_stat_node.kind, TreeKind::ValDef(_))
                || matches!(stat.kind, TreeKind::ValDef(_))
            {
                self.validate_local_value_statement(
                    source_stat,
                    typed_stat,
                    method,
                    reference_result.then_some(stat.ty),
                    depth,
                    visited,
                )?;
                continue;
            }
            if index == 2
                && let Some(binding) = unit_call
            {
                self.validate_local_receiver_unit_call(
                    binding,
                    source_stat,
                    typed_stat,
                    method,
                    depth,
                    visited,
                )?;
                continue;
            }
            if matches!(source_stat_node.kind, TreeKind::Assign(_))
                || matches!(stat.kind, TreeKind::Assign(_))
            {
                self.validate_assignment_statement(
                    source_stat,
                    typed_stat,
                    method,
                    depth,
                    visited,
                )?;
                continue;
            }
            if matches!(source_stat_node.kind, TreeKind::While(_))
                || matches!(stat.kind, TreeKind::While(_))
            {
                self.validate_while_statement(source_stat, typed_stat, method, depth + 1, visited)?;
                continue;
            }
            if matches!(source_stat_node.kind, TreeKind::Block(_))
                || matches!(stat.kind, TreeKind::Block(_))
            {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                );
            }
            self.validate_method_expression_inner(
                Some(source_stat),
                Some(typed_stat),
                method,
                self.plan.definitions.int,
                depth,
                (Some(visited), false),
            )?;
        }
        let source_result = self.source_tree(source_block.expr)?;
        if visited.contains(&typed_block.expr) {
            return Err(PickleError::RepeatedExpression {
                tree: typed_block.expr,
            });
        }
        if matches!(source_result.kind, TreeKind::Block(_))
            || matches!(typed_result.kind, TreeKind::Block(_))
        {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        }
        self.validate_method_expression_inner(
            Some(source_block.expr),
            Some(typed_block.expr),
            method,
            result_type,
            depth,
            (Some(visited), reference_result),
        )
    }

    fn validate_local_receiver_unit_call(
        &mut self,
        binding: crate::LocalReceiverUnitCallBinding,
        source_stat: TreeId<Untyped>,
        typed_stat: TreeId<Typed>,
        method: &MethodPicklePlan,
        depth: usize,
        visited: &mut HashSet<TreeId<Typed>>,
    ) -> Result<(), PickleError> {
        let p = self.plan;
        let source_apply = self.source_tree(binding.source_apply)?;
        let typed_apply =
            p.typed_ast
                .try_get(binding.typed_apply)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: binding.typed_apply,
                })?;
        let (TreeKind::Apply(source_call), TreeKind::Apply(typed_call)) =
            (&source_apply.kind, &typed_apply.kind)
        else {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        };
        let source_select = self.source_tree(binding.source_select)?;
        let typed_select =
            p.typed_ast
                .try_get(binding.typed_select)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: binding.typed_select,
                })?;
        let (TreeKind::Select(source_selection), TreeKind::Select(typed_selection)) =
            (&source_select.kind, &typed_select.kind)
        else {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        };
        let source_receiver = self.source_tree(binding.source_receiver)?;
        let typed_receiver =
            p.typed_ast
                .try_get(binding.typed_receiver)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: binding.typed_receiver,
                })?;
        let source_argument = self.source_tree(binding.source_argument)?;
        let typed_argument =
            p.typed_ast
                .try_get(binding.typed_argument)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: binding.typed_argument,
                })?;
        let initializers = p
            .typed_bindings
            .local_counter_initializers(p.source.id, method.symbol);
        let receiver_initializer = initializers
            .iter()
            .find(|initializer| initializer.local == binding.receiver);
        let pair = p
            .typed_bindings
            .formal_local_constructor_call_pair(p.source.id, method.symbol);
        let (TreeKind::Ident(source_receiver_ident), TreeKind::Ident(typed_receiver_ident)) =
            (&source_receiver.kind, &typed_receiver.kind)
        else {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        };
        let (TreeKind::Ident(source_argument_ident), TreeKind::Ident(typed_argument_ident)) =
            (&source_argument.kind, &typed_argument.kind)
        else {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        };
        let second_formal = pair.map(|pair| pair.method_formals[1]);
        let canonical_method = matches!(
            p.source_index.definition_of(binding.method),
            Some(dotty_core::SourceDefinition::Canonical { source, tree })
                if source == p.source.id
                    && matches!(p.source_ast.try_get(tree).map(|node| &node.kind), Some(TreeKind::DefDef(_)))
        );
        if binding.source_apply != source_stat
            || binding.typed_apply != typed_stat
            || source_call.kind != ApplyKind::Regular
            || typed_call.kind != ApplyKind::Regular
            || source_call.function != binding.source_select
            || typed_call.function != binding.typed_select
            || source_call.args.as_slice() != [binding.source_argument]
            || typed_call.args.as_slice() != [binding.typed_argument]
            || source_selection.qualifier != binding.source_receiver
            || typed_selection.qualifier != binding.typed_receiver
            || initializers.len() != 2
            || receiver_initializer.is_none()
            || !self.visible_locals.contains(&binding.receiver)
            || !canonical_method
            || second_formal != Some(binding.argument_formal)
            || source_receiver_ident.name != self.symbol(binding.receiver)?.name
            || typed_receiver_ident.name != source_receiver_ident.name
            || source_argument_ident.name != self.symbol(binding.argument_formal)?.name
            || typed_argument_ident.name != source_argument_ident.name
            || !matches!(self.ty(typed_receiver.ty)?, Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) } if *prefix == p.definitions.no_prefix && *symbol == binding.receiver)
            || !matches!(self.ty(typed_select.ty)?, Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) } if *prefix == typed_receiver.ty && *symbol == binding.method)
            || !matches!(self.ty(typed_argument.ty)?, Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) } if *prefix == p.definitions.no_prefix && *symbol == binding.argument_formal)
            || typed_apply.ty != p.definitions.unit
            || !matches!(self.ty(typed_argument.ty)?, Type::TermRef { .. })
            || self.symbol(binding.argument_formal)?.kind != SymbolKind::Parameter
            || self.symbol(binding.argument_formal)?.owner != Some(method.symbol)
            || self.symbol(binding.method)?.owner
                != receiver_initializer.map(|initializer| initializer.call.class)
            || self.symbol(binding.method)?.kind != SymbolKind::Method
            || source_selection.name != self.symbol(binding.method)?.name
            || typed_selection.name != source_selection.name
            || !matches!(self.ty(self.complete(binding.method)?)?, Type::Method(signature)
                if signature.params.len() == 1
                    && signature.params[0].ty == p.definitions.int
                    && signature.result == p.definitions.unit)
        {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        }
        for (source_tree, typed_tree) in [
            (binding.source_apply, binding.typed_apply),
            (binding.source_select, binding.typed_select),
            (binding.source_receiver, binding.typed_receiver),
            (binding.source_argument, binding.typed_argument),
        ] {
            self.source_typed_binding(source_tree, typed_tree)?;
            let source = self.source_tree(source_tree)?;
            let typed = p
                .typed_ast
                .try_get(typed_tree)
                .ok_or(PickleError::InvalidTypedTree { tree: typed_tree })?;
            self.paired_source_typed_span(source.position, typed.position, typed_tree)?;
        }
        self.charge(1)?;
        if !visited.insert(binding.typed_apply)
            || !visited.insert(binding.typed_select)
            || !visited.insert(binding.typed_receiver)
            || !visited.insert(binding.typed_argument)
        {
            return Err(PickleError::RepeatedExpression {
                tree: binding.typed_apply,
            });
        }
        let _ = depth;
        Ok(())
    }

    pub(super) fn validate_unit_expression(
        &mut self,
        source_id: TreeId<Untyped>,
        typed_id: TreeId<Typed>,
        method: &MethodPicklePlan,
        depth: usize,
        visited: &mut HashSet<TreeId<Typed>>,
    ) -> Result<(), PickleError> {
        let depth_limit = self.limits.expression_depth_limit();
        if depth > depth_limit {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::ExpressionDepth,
                limit: depth_limit,
            });
        }
        let typed = self
            .plan
            .typed_ast
            .try_get(typed_id)
            .ok_or(PickleError::InvalidTypedTree { tree: typed_id })?;
        if typed.ty != self.plan.definitions.unit
            && !matches!(self.ty(typed.ty)?, Type::Constant(Constant::Unit))
        {
            return Err(PickleError::BodyType {
                tree: typed_id,
                ty: typed.ty,
            });
        }
        if let TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) =
            &self.source_tree(source_id)?.kind
        {
            self.source_typed_binding(source_id, typed_id)?;
            self.span(self.source_tree(source_id)?.position)?;
            self.charge(1)?;
            return self.validate_unit_expression(
                parens.inner,
                typed_id,
                method,
                depth + 1,
                visited,
            );
        }
        match (&self.source_tree(source_id)?.kind, &typed.kind) {
            (TreeKind::Assign(_), TreeKind::Assign(_)) => {
                self.validate_assignment_statement(source_id, typed_id, method, depth, visited)
            }
            (TreeKind::Block(_), TreeKind::Block(_)) => {
                self.validate_unit_block(source_id, typed_id, method, depth, visited)
            }
            (TreeKind::Literal(_), TreeKind::Literal(_)) => self.validate_method_expression_inner(
                Some(source_id),
                Some(typed_id),
                method,
                self.plan.definitions.unit,
                depth,
                (Some(visited), false),
            ),
            _ => Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))),
        }
    }

    fn validate_unit_block(
        &mut self,
        source_id: TreeId<Untyped>,
        typed_id: TreeId<Typed>,
        method: &MethodPicklePlan,
        depth: usize,
        visited: &mut HashSet<TreeId<Typed>>,
    ) -> Result<(), PickleError> {
        let depth_limit = self.limits.expression_depth_limit();
        if depth > depth_limit {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::ExpressionDepth,
                limit: depth_limit,
            });
        }
        self.charge(1)?;
        if !visited.insert(typed_id) {
            return Err(PickleError::RepeatedExpression { tree: typed_id });
        }
        let source = self.source_tree(source_id)?;
        let typed = self
            .plan
            .typed_ast
            .try_get(typed_id)
            .ok_or(PickleError::InvalidTypedTree { tree: typed_id })?;
        self.source_typed_binding(source_id, typed_id)?;
        self.paired_source_typed_span(source.position, typed.position, typed_id)?;
        let (TreeKind::Block(source_block), TreeKind::Block(typed_block)) =
            (&source.kind, &typed.kind)
        else {
            return Err(PickleError::SourceShape {
                tree: source_id,
                expected: SourceRole::Body,
            });
        };
        let source_block = source_block.clone();
        let typed_block = typed_block.clone();
        if (typed.ty != self.plan.definitions.unit
            && !matches!(self.ty(typed.ty)?, Type::Constant(Constant::Unit)))
            || source_block.stats.len() != typed_block.stats.len()
        {
            return Err(PickleError::BodyType {
                tree: typed_id,
                ty: typed.ty,
            });
        }
        for (&source_stat, &typed_stat) in source_block.stats.iter().zip(&typed_block.stats) {
            let source_stat_node = self.source_tree(source_stat)?;
            let stat = self
                .plan
                .typed_ast
                .try_get(typed_stat)
                .ok_or(PickleError::InvalidTypedTree { tree: typed_stat })?;
            if !matches!(source_stat_node.kind, TreeKind::Assign(_))
                || !matches!(stat.kind, TreeKind::Assign(_))
            {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                );
            }
            self.validate_assignment_statement(
                source_stat,
                typed_stat,
                method,
                depth + 1,
                visited,
            )?;
        }
        self.validate_unit_expression(
            source_block.expr,
            typed_block.expr,
            method,
            depth + 1,
            visited,
        )
    }
}
