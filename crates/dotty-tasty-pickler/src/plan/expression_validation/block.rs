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
        if result_type != self.plan.definitions.int {
            return Err(PickleError::BodyType {
                tree: typed_id,
                ty: typed.ty,
            });
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
                    depth,
                    visited,
                )?;
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
                Some(visited),
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
            Some(visited),
        )
    }
}
