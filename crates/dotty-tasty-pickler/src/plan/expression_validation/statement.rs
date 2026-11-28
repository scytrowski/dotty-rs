//! Primitive statement contexts and authenticated expected-Unit wrappers.
use super::*;

impl Validator<'_, '_> {
    pub(super) fn validate_discarded_expression(
        &mut self,
        source: TreeId<Untyped>,
        typed: TreeId<Typed>,
        method: &MethodPicklePlan,
        depth: usize,
        visited: &mut HashSet<TreeId<Typed>>,
    ) -> Result<bool, PickleError> {
        let node = self
            .plan
            .typed_ast
            .try_get(typed)
            .ok_or(PickleError::InvalidTypedTree { tree: typed })?;
        let expected = if node.ty == self.plan.definitions.nothing_type {
            self.plan.definitions.unit
        } else {
            self.primitive_flow_type(node.ty)?
                .ok_or(PickleError::BodyType {
                    tree: typed,
                    ty: node.ty,
                })?
        };
        self.validate_return_flow(source, typed, method, expected, depth, visited)
    }

    // Only the wrapper's authenticated original-child edge receives a scoped
    // binding exception. All descendant source edges use their canonical maps.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn validate_unit_adaptation(
        &mut self,
        source_id: TreeId<Untyped>,
        typed_id: TreeId<Typed>,
        method: &MethodPicklePlan,
        depth: usize,
        visited: &mut HashSet<TreeId<Typed>>,
        allow_child_mapping: bool,
    ) -> Result<Option<bool>, PickleError> {
        if depth > self.limits.expression_depth_limit() {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::ExpressionDepth,
                limit: self.limits.expression_depth_limit(),
            });
        }
        let p = self.plan;
        let source = self.source_tree(source_id)?;
        let node = p
            .typed_ast
            .try_get(typed_id)
            .ok_or(PickleError::InvalidTypedTree { tree: typed_id })?;
        let TreeKind::Block(block) = &node.kind else {
            return Ok(None);
        };
        if node.ty != p.definitions.unit {
            return Ok(None);
        }
        // A real source block has its own final expression. Only a wrapper
        // around the complete original block can stand in for that source.
        if let TreeKind::Block(source_block) = &source.kind
            && (matches!(&self.source_tree(source_block.expr)?.kind, TreeKind::Literal(v) if v.value == Constant::Unit)
                || block.stats.len() != 1
                || !matches!(
                    p.typed_ast.try_get(block.stats[0]).map(|n| &n.kind),
                    Some(TreeKind::Block(_))
                ))
        {
            return Ok(None);
        }
        if block.stats.len() != 1 {
            return Err(PickleError::BodyType {
                tree: typed_id,
                ty: node.ty,
            });
        }
        let child = block.stats[0];
        let unit = block.expr;
        let tail = p
            .typed_ast
            .try_get(unit)
            .ok_or(PickleError::InvalidTypedTree { tree: unit })?;
        if tail.ty != p.definitions.unit
            || tail.position.is_some()
            || !matches!(&tail.kind, TreeKind::Literal(v) if v.value == Constant::Unit)
        {
            return Err(PickleError::BodyType {
                tree: unit,
                ty: tail.ty,
            });
        }
        let original = p
            .typed_ast
            .try_get(child)
            .ok_or(PickleError::InvalidTypedTree { tree: child })?;
        let diverging = original.ty == p.definitions.nothing_type && allow_child_mapping;
        let family = if diverging {
            p.definitions.unit
        } else {
            self.primitive_flow_type(original.ty)?
                .ok_or(PickleError::BodyType {
                    tree: child,
                    ty: original.ty,
                })?
        };
        if family == p.definitions.unit && !diverging {
            return Ok(None);
        }
        let mapped = p.typed_bindings.typed_at(p.source.id, source_id);
        if mapped != Some(typed_id) && !(allow_child_mapping && mapped == Some(child)) {
            return Err(PickleError::BodyBinding {
                source: p.source.id,
                tree: source_id,
                expected: typed_id,
                actual: mapped,
            });
        }
        self.paired_source_typed_span(source.position, node.position, typed_id)?;
        self.charge(2)?;
        for id in [typed_id, unit] {
            if !visited.insert(id) {
                return Err(PickleError::RepeatedExpression { tree: id });
            }
        }
        self.adapted_unit_bindings
            .push((source_id, child, typed_id));
        let result =
            self.validate_return_flow(source_id, child, method, family, depth + 1, visited);
        self.adapted_unit_bindings.pop();
        result.map(Some)
    }
}
