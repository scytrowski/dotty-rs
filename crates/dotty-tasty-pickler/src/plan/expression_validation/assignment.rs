use super::*;

impl Validator<'_, '_> {
    pub(super) fn validate_assignment_statement(
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
        let (TreeKind::Assign(source_assign), TreeKind::Assign(typed_assign)) =
            (&source.kind, &typed.kind)
        else {
            return Err(PickleError::SourceShape {
                tree: source_id,
                expected: SourceRole::Body,
            });
        };
        if typed.ty != self.plan.definitions.unit {
            return Err(PickleError::BodyType {
                tree: typed_id,
                ty: typed.ty,
            });
        }

        for (source_child, typed_child) in [
            (source_assign.lhs, typed_assign.lhs),
            (source_assign.rhs, typed_assign.rhs),
        ] {
            let actual = self
                .plan
                .typed_bindings
                .typed_at(self.plan.source.id, source_child);
            if actual != Some(typed_child) {
                return Err(PickleError::BodyBinding {
                    source: self.plan.source.id,
                    tree: source_child,
                    expected: typed_child,
                    actual,
                });
            }
        }

        let lhs =
            self.plan
                .typed_ast
                .try_get(typed_assign.lhs)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: typed_assign.lhs,
                })?;
        let Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(target),
        } = self.ty(lhs.ty)?
        else {
            return Err(PickleError::BodyType {
                tree: typed_assign.lhs,
                ty: lhs.ty,
            });
        };
        let target = *target;
        if !self.registered_local_symbols.contains(&target)
            || !self.visible_locals.contains(&target)
            || self.symbol(target)?.flags != SymbolFlags::MUTABLE
            || self.complete(target)? != self.plan.definitions.int
            || *prefix != self.plan.definitions.no_prefix
        {
            return Err(PickleError::Reference {
                symbol: target,
                reason: crate::ReferenceError::NotRegistered,
            });
        }

        self.validate_method_expression_inner(
            Some(source_assign.lhs),
            Some(typed_assign.lhs),
            method,
            self.plan.definitions.int,
            depth,
            Some(visited),
        )?;
        self.validate_int_expression(source_assign.rhs, typed_assign.rhs, method, depth, visited)
    }
}
