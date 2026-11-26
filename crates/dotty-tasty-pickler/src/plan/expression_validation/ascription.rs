use super::*;

impl Validator<'_, '_> {
    pub(super) fn validate_primitive_ascription(
        &mut self,
        source_id: TreeId<Untyped>,
        typed_id: TreeId<Typed>,
        method: &MethodPicklePlan,
        result_type: TypeId,
        depth: usize,
        visited: &mut HashSet<TreeId<Typed>>,
    ) -> Result<(), PickleError> {
        if depth > self.limits.expression_depth_limit() {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::ExpressionDepth,
                limit: self.limits.expression_depth_limit(),
            });
        }
        self.charge(2)?;
        if !visited.insert(typed_id) {
            return Err(PickleError::RepeatedExpression { tree: typed_id });
        }
        let source = self.source_tree(source_id)?;
        let typed = self
            .plan
            .typed_ast
            .try_get(typed_id)
            .ok_or(PickleError::InvalidTypedTree { tree: typed_id })?;
        let (TreeKind::Typed(source_value), TreeKind::Typed(typed_value)) =
            (&source.kind, &typed.kind)
        else {
            return Err(PickleError::SourceShape {
                tree: source_id,
                expected: SourceRole::Body,
            });
        };
        let source_value = *source_value;
        let typed_value = *typed_value;
        self.source_typed_binding(source_id, typed_id)?;
        self.paired_source_typed_span(source.position, typed.position, typed_id)?;
        let expected_name = if result_type == self.plan.definitions.int {
            "Int"
        } else if result_type == self.plan.definitions.boolean {
            "Boolean"
        } else if result_type == self.plan.definitions.unit {
            "Unit"
        } else {
            return Err(PickleError::BodyType {
                tree: typed_id,
                ty: typed.ty,
            });
        };
        if typed.ty != result_type {
            return Err(PickleError::BodyType {
                tree: typed_id,
                ty: typed.ty,
            });
        }
        let source_tpt = self.source_tree(source_value.tpt)?;
        let typed_tpt =
            self.plan
                .typed_ast
                .try_get(typed_value.tpt)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: typed_value.tpt,
                })?;
        self.source_typed_binding(source_value.tpt, typed_value.tpt)?;
        self.paired_source_typed_span(source_tpt.position, typed_tpt.position, typed_value.tpt)?;
        if !matches!(&source_tpt.kind, TreeKind::Ident(name)
            if name.name.is_type() && self.name_text(name.name.text())? == expected_name)
            || !matches!(typed_tpt.kind, TreeKind::TypeTree(_))
            || typed_tpt.ty != result_type
        {
            return Err(PickleError::BodyType {
                tree: typed_value.tpt,
                ty: typed_tpt.ty,
            });
        }
        if !visited.insert(typed_value.tpt) {
            return Err(PickleError::RepeatedExpression {
                tree: typed_value.tpt,
            });
        }
        self.validate_method_expression_inner(
            Some(source_value.expr),
            Some(typed_value.expr),
            method,
            result_type,
            depth + 1,
            (Some(visited), false),
        )
    }
}
