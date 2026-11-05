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
            let exact_shape = (1..=2).contains(&source_block.stats.len())
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
                )
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
            let two_local_shape = source_block.stats.len() == 2
                && local_argument_indices.len() == 2
                && initializers.len() == 2
                && initializers[0].local != initializers[1].local
                && initializers[0].scope == initializers[1].scope
                && source_block
                    .stats
                    .iter()
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

        for (&source_stat, &typed_stat) in source_block.stats.iter().zip(&typed_block.stats) {
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
