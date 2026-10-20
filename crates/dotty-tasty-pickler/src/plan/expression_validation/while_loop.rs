use super::*;

impl Validator<'_, '_> {
    pub(super) fn validate_while_statement(
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
        let (TreeKind::While(source_while), TreeKind::While(typed_while)) =
            (&source.kind, &typed.kind)
        else {
            return Err(PickleError::SourceShape {
                tree: source_id,
                expected: SourceRole::Body,
            });
        };
        let source_while = *source_while;
        let typed_while = *typed_while;
        if typed.ty != self.plan.definitions.unit {
            return Err(PickleError::BodyType {
                tree: typed_id,
                ty: typed.ty,
            });
        }
        self.validate_if_condition(
            source_while.cond,
            typed_while.cond,
            method,
            depth + 1,
            visited,
        )?;
        self.validate_unit_expression(
            source_while.body,
            typed_while.body,
            method,
            depth + 1,
            visited,
        )
    }
}
