//! Bounded, read-only validation of normal and locally returning paths.
use super::*;

impl Validator<'_, '_> {
    pub(super) fn contains_local_return(
        &self,
        source: TreeId<Untyped>,
        root: TreeId<Typed>,
    ) -> bool {
        // Dispatch must not change diagnostics or depth accounting for bodies
        // without returns. Bound this iterative search separately and defer
        // malformed IDs, cycles and exhausted budgets to the real validator.
        let mut pending = vec![(source, root)];
        let mut seen = HashSet::new();
        let mut remaining = self.limits.max_items;
        while let Some((source_id, id)) = pending.pop() {
            if !seen.insert((source_id, id)) {
                continue;
            }
            let Some(next) = remaining.checked_sub(1) else {
                return false;
            };
            remaining = next;
            let Some(node) = self.plan.typed_ast.try_get(id) else {
                return false;
            };
            let Ok(source) = self.source_tree(source_id) else {
                return false;
            };
            // Leave malformed shape pairs to the existing validator instead of
            // following an unrelated typed subtree first.
            let children: Vec<_> = match (&source.kind, &node.kind) {
                (TreeKind::PhaseSpecific(UntypedNode::Parens(parens)), _) => {
                    vec![(parens.inner, id)]
                }
                (_, TreeKind::Block(block))
                    if node.ty == self.plan.definitions.unit
                        && block.stats.len() == 1
                        && block.stats[0] != id
                        && self
                            .plan
                            .typed_bindings
                            .typed_at(self.plan.source.id, source_id)
                            == Some(block.stats[0]) =>
                {
                    // The flow validator authenticates the enclosing incomplete
                    // If and the complete synthetic Unit adaptation shape.
                    vec![(source_id, block.stats[0])]
                }
                (TreeKind::Return(_), TreeKind::Return(_)) => return true,
                (TreeKind::Block(source), TreeKind::Block(block)) => {
                    if source.stats.len() != block.stats.len() || block.stats.len() > remaining {
                        return false;
                    }
                    source
                        .stats
                        .iter()
                        .copied()
                        .zip(block.stats.iter().copied())
                        .chain([(source.expr, block.expr)])
                        .collect()
                }
                (TreeKind::If(source), TreeKind::If(node)) => vec![
                    (source.then_branch, node.then_branch),
                    (source.else_branch, node.else_branch),
                ],
                (TreeKind::While(source), TreeKind::While(node)) => vec![(source.body, node.body)],
                (TreeKind::Match(source), TreeKind::Match(node)) => {
                    if source.cases.len() != node.cases.len() || node.cases.len() > remaining {
                        return false;
                    }
                    source
                        .cases
                        .iter()
                        .copied()
                        .zip(node.cases.iter().copied())
                        .collect()
                }
                (TreeKind::CaseDef(source), TreeKind::CaseDef(node)) => {
                    vec![(source.body, node.body)]
                }
                _ => Vec::new(),
            };
            if children.len() > remaining.saturating_sub(pending.len()) {
                return false;
            }
            pending.extend(children);
        }
        false
    }

    pub(super) fn primitive_flow_type(
        &self,
        mut ty: TypeId,
    ) -> Result<Option<TypeId>, PickleError> {
        let p = self.plan;
        for _ in 0..=self.limits.expression_depth_limit() {
            if [
                p.definitions.int,
                p.definitions.boolean,
                p.definitions.unit,
                p.definitions.long,
                p.definitions.float,
                p.definitions.double,
            ]
            .contains(&ty)
            {
                return Ok(Some(ty));
            }
            match self.ty(ty)? {
                Type::Constant(Constant::Int(_)) => return Ok(Some(p.definitions.int)),
                Type::Constant(Constant::Long(_)) => return Ok(Some(p.definitions.long)),
                Type::Constant(Constant::FloatBits(_)) => return Ok(Some(p.definitions.float)),
                Type::Constant(Constant::DoubleBits(_)) => return Ok(Some(p.definitions.double)),
                Type::Constant(Constant::Boolean(_)) => return Ok(Some(p.definitions.boolean)),
                Type::Constant(Constant::Unit) => return Ok(Some(p.definitions.unit)),
                Type::TermRef {
                    target: TermRefTarget::Symbol(symbol),
                    ..
                } => ty = self.complete(*symbol)?,
                _ => return Ok(None),
            }
        }
        Err(PickleError::LimitExceeded {
            kind: LimitKind::ExpressionDepth,
            limit: self.limits.expression_depth_limit(),
        })
    }

    fn local_return_result(&self, method: &MethodPicklePlan) -> Result<TypeId, PickleError> {
        self.kind(method.symbol, SymbolKind::Method)?;
        self.owner(method.symbol, Some(self.plan.class))?;
        let mut ty = self.complete(method.symbol)?;
        for _ in 0..=self.limits.expression_depth_limit() {
            match self.ty(ty)? {
                Type::Method(signature) if signature.kind == MethodKind::Plain => {
                    ty = signature.result
                }
                _ => {
                    return self
                        .primitive_flow_type(ty)?
                        .filter(|result| *result == ty)
                        .ok_or(PickleError::UnsupportedTypeReference { ty });
                }
            }
        }
        Err(PickleError::LimitExceeded {
            kind: LimitKind::ExpressionDepth,
            limit: self.limits.expression_depth_limit(),
        })
    }

    fn flow_node_type(
        &self,
        id: TreeId<Typed>,
        expected: TypeId,
        exits: bool,
    ) -> Result<(), PickleError> {
        let node = self
            .plan
            .typed_ast
            .try_get(id)
            .ok_or(PickleError::InvalidTypedTree { tree: id })?;
        if (exits && node.ty == self.plan.definitions.nothing_type)
            || self.primitive_flow_type(node.ty)? == Some(expected)
        {
            Ok(())
        } else {
            Err(PickleError::BodyType {
                tree: id,
                ty: node.ty,
            })
        }
    }

    pub(super) fn validate_return_flow(
        &mut self,
        source_id: TreeId<Untyped>,
        typed_id: TreeId<Typed>,
        method: &MethodPicklePlan,
        expected: TypeId,
        depth: usize,
        visited: &mut HashSet<TreeId<Typed>>,
    ) -> Result<bool, PickleError> {
        if depth > self.limits.expression_depth_limit() {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::ExpressionDepth,
                limit: self.limits.expression_depth_limit(),
            });
        }
        let p = self.plan;
        let source = self.source_tree(source_id)?;
        let typed = p
            .typed_ast
            .try_get(typed_id)
            .ok_or(PickleError::InvalidTypedTree { tree: typed_id })?;
        if let TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) = &source.kind {
            self.source_typed_binding(source_id, typed_id)?;
            self.span(source.position)?;
            self.charge(1)?;
            return self.validate_return_flow(
                parens.inner,
                typed_id,
                method,
                expected,
                depth + 1,
                visited,
            );
        }
        if matches!(
            (&source.kind, &typed.kind),
            (TreeKind::Match(_), TreeKind::Match(_))
        ) {
            return self
                .validate_return_match(source_id, typed_id, method, expected, depth, visited);
        }
        if !matches!(
            &typed.kind,
            TreeKind::Return(_) | TreeKind::Block(_) | TreeKind::If(_) | TreeKind::While(_)
        ) {
            self.validate_method_expression_inner(
                Some(source_id),
                Some(typed_id),
                method,
                expected,
                depth,
                (Some(visited), false),
            )?;
            return Ok(false);
        }
        self.charge(1)?;
        if !visited.insert(typed_id) {
            return Err(PickleError::RepeatedExpression { tree: typed_id });
        }
        self.source_typed_binding(source_id, typed_id)?;
        self.paired_source_typed_span(source.position, typed.position, typed_id)?;
        let exits = match (&source.kind, &typed.kind) {
            (TreeKind::Return(source_return), TreeKind::Return(returned)) => {
                if source_return.from.is_some()
                    || returned.from.is_some()
                    || typed.ty != p.definitions.nothing_type
                {
                    return Err(
                        self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                    );
                }
                let result = self.local_return_result(method)?;
                match (source_return.expr, returned.expr) {
                    (Some(source_expr), Some(expr)) => self.validate_method_expression_inner(
                        Some(source_expr),
                        Some(expr),
                        method,
                        result,
                        depth + 1,
                        (Some(visited), false),
                    )?,
                    (None, Some(expr)) if result == p.definitions.unit => {
                        self.charge(1)?;
                        let value = p
                            .typed_ast
                            .try_get(expr)
                            .ok_or(PickleError::InvalidTypedTree { tree: expr })?;
                        if !matches!(&value.kind, TreeKind::Literal(value) if value.value == Constant::Unit)
                            || !matches!(self.ty(value.ty)?, Type::Constant(Constant::Unit))
                            || value.position != typed.position
                        {
                            return Err(PickleError::BodyType {
                                tree: expr,
                                ty: value.ty,
                            });
                        }
                        if !visited.insert(expr) {
                            return Err(PickleError::RepeatedExpression { tree: expr });
                        }
                    }
                    _ => {
                        return Err(self
                            .unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
                    }
                }
                true
            }
            (TreeKind::Block(source_block), TreeKind::Block(block)) => {
                if source_block.stats.len() != block.stats.len()
                    || block.stats.len() > self.limits.max_items
                {
                    return Err(
                        self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                    );
                }
                let source_block = source_block.clone();
                let block = block.clone();
                let tail = p
                    .typed_ast
                    .try_get(block.expr)
                    .ok_or(PickleError::InvalidTypedTree { tree: block.expr })?;
                if typed.ty != tail.ty {
                    return Err(PickleError::BodyType {
                        tree: typed_id,
                        ty: typed.ty,
                    });
                }
                let visible = self.visible_locals.clone();
                let mut exits = false;
                for (&source_stat, &stat) in source_block.stats.iter().zip(&block.stats) {
                    let node = p
                        .typed_ast
                        .try_get(stat)
                        .ok_or(PickleError::InvalidTypedTree { tree: stat })?;
                    if matches!(node.kind, TreeKind::ValDef(_)) {
                        self.validate_local_value_statement(
                            source_stat,
                            stat,
                            method,
                            super::local::LocalValueValidation {
                                reference_type: None,
                                depth: depth + 1,
                                visited,
                                fragment: false,
                            },
                        )?;
                    } else {
                        let expected_stat = if node.ty == p.definitions.nothing_type {
                            p.definitions.unit
                        } else {
                            self.primitive_flow_type(node.ty)?
                                .ok_or(PickleError::BodyType {
                                    tree: stat,
                                    ty: node.ty,
                                })?
                        };
                        let stat_exits = self.validate_return_flow(
                            source_stat,
                            stat,
                            method,
                            expected_stat,
                            depth + 1,
                            visited,
                        )?;
                        if [
                            p.definitions.long,
                            p.definitions.float,
                            p.definitions.double,
                        ]
                        .contains(&expected_stat)
                            && !stat_exits
                        {
                            return Err(self.unsupported(
                                UnsupportedFeature::BodyExpression,
                                Some(method.symbol),
                            ));
                        }
                        exits |= stat_exits;
                    }
                }
                exits |= self.validate_return_flow(
                    source_block.expr,
                    block.expr,
                    method,
                    expected,
                    depth + 1,
                    visited,
                )?;
                self.visible_locals = visible;
                exits
            }
            (TreeKind::If(source_if), TreeKind::If(conditional)) => {
                let source_if = *source_if;
                let conditional = *conditional;
                self.validate_if_condition(
                    source_if.cond,
                    conditional.cond,
                    method,
                    depth + 1,
                    visited,
                )?;
                let visible = self.visible_locals.clone();
                let adapt = self.incomplete_else_chain(source_if.else_branch, depth + 1)?;
                let a = self.validate_return_branch(
                    source_if.then_branch,
                    conditional.then_branch,
                    method,
                    expected,
                    depth + 1,
                    visited,
                    adapt,
                )?;
                self.visible_locals = visible.clone();
                let b = self.validate_return_flow(
                    source_if.else_branch,
                    conditional.else_branch,
                    method,
                    expected,
                    depth + 1,
                    visited,
                )?;
                self.visible_locals = visible;
                let branch_type = |id| -> Result<Option<TypeId>, PickleError> {
                    let node = p
                        .typed_ast
                        .try_get(id)
                        .ok_or(PickleError::InvalidTypedTree { tree: id })?;
                    if node.ty == p.definitions.nothing_type {
                        Ok(None)
                    } else {
                        self.primitive_flow_type(node.ty)
                    }
                };
                let left = branch_type(conditional.then_branch)?;
                let right = branch_type(conditional.else_branch)?;
                let joined = match (left, right) {
                    (None, None) => p.definitions.nothing_type,
                    (Some(value), None) | (None, Some(value)) => value,
                    (Some(a), Some(b)) if a == b => a,
                    _ => {
                        return Err(PickleError::BodyType {
                            tree: typed_id,
                            ty: typed.ty,
                        });
                    }
                };
                if typed.ty != joined {
                    return Err(PickleError::BodyType {
                        tree: typed_id,
                        ty: typed.ty,
                    });
                }
                a && b
            }
            (TreeKind::While(source_loop), TreeKind::While(looped)) => {
                let source_loop = *source_loop;
                let looped = *looped;
                if expected != p.definitions.unit || typed.ty != p.definitions.unit {
                    return Err(PickleError::BodyType {
                        tree: typed_id,
                        ty: typed.ty,
                    });
                }
                self.validate_if_condition(
                    source_loop.cond,
                    looped.cond,
                    method,
                    depth + 1,
                    visited,
                )?;
                let visible = self.visible_locals.clone();
                self.validate_return_flow(
                    source_loop.body,
                    looped.body,
                    method,
                    p.definitions.unit,
                    depth + 1,
                    visited,
                )?;
                self.visible_locals = visible;
                false // the condition may be false at the first iteration
            }
            _ => {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                );
            }
        };
        self.flow_node_type(typed_id, expected, exits)?;
        Ok(exits)
    }

    fn incomplete_else_chain(
        &self,
        id: TreeId<Untyped>,
        depth: usize,
    ) -> Result<bool, PickleError> {
        if depth > self.limits.expression_depth_limit() {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::ExpressionDepth,
                limit: self.limits.expression_depth_limit(),
            });
        }
        let node = self.source_tree(id)?;
        match &node.kind {
            TreeKind::Literal(value)
                if value.value == Constant::Unit
                    && node.position.is_some_and(|p| p.span().range().is_empty()) =>
            {
                Ok(true)
            }
            TreeKind::If(conditional) => {
                self.incomplete_else_chain(conditional.else_branch, depth + 1)
            }
            _ => Ok(false),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_return_branch(
        &mut self,
        source: TreeId<Untyped>,
        typed: TreeId<Typed>,
        method: &MethodPicklePlan,
        expected: TypeId,
        depth: usize,
        visited: &mut HashSet<TreeId<Typed>>,
        allow_adaptation: bool,
    ) -> Result<bool, PickleError> {
        let mapped = self
            .plan
            .typed_bindings
            .typed_at(self.plan.source.id, source);
        if mapped == Some(typed) || !allow_adaptation {
            return self.validate_return_flow(source, typed, method, expected, depth, visited);
        }
        if depth > self.limits.expression_depth_limit() {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::ExpressionDepth,
                limit: self.limits.expression_depth_limit(),
            });
        }
        let node = self
            .plan
            .typed_ast
            .try_get(typed)
            .ok_or(PickleError::InvalidTypedTree { tree: typed })?;
        let TreeKind::Block(block) = &node.kind else {
            return Err(PickleError::BodyType {
                tree: typed,
                ty: node.ty,
            });
        };
        if node.ty != self.plan.definitions.unit
            || block.stats.len() != 1
            || mapped != Some(block.stats[0])
        {
            return Err(PickleError::BodyType {
                tree: typed,
                ty: node.ty,
            });
        }
        let child = block.stats[0];
        let unit = block.expr;
        self.paired_source_typed_span(self.source_tree(source)?.position, node.position, typed)?;
        self.charge(2)?;
        if !visited.insert(typed) {
            return Err(PickleError::RepeatedExpression { tree: typed });
        }
        let tail = self
            .plan
            .typed_ast
            .try_get(unit)
            .ok_or(PickleError::InvalidTypedTree { tree: unit })?;
        if tail.ty != self.plan.definitions.unit
            || tail.position.is_some()
            || !matches!(&tail.kind, TreeKind::Literal(value) if value.value == Constant::Unit)
            || !visited.insert(unit)
        {
            return Err(PickleError::BodyType {
                tree: unit,
                ty: tail.ty,
            });
        }
        let child_ty = self
            .plan
            .typed_ast
            .try_get(child)
            .ok_or(PickleError::InvalidTypedTree { tree: child })?
            .ty;
        let child_expected = if child_ty == self.plan.definitions.nothing_type {
            expected
        } else {
            self.primitive_flow_type(child_ty)?
                .ok_or(PickleError::BodyType {
                    tree: child,
                    ty: child_ty,
                })?
        };
        self.validate_return_flow(source, child, method, child_expected, depth + 1, visited)
    }
}
