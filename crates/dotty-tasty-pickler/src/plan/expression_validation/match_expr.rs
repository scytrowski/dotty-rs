use super::*;

impl Validator<'_, '_> {
    pub(super) fn validate_primitive_match(
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
        self.charge(1)?;
        if !visited.insert(typed_id) {
            return Err(PickleError::RepeatedExpression { tree: typed_id });
        }
        let p = self.plan;
        let source = self.source_tree(source_id)?;
        let typed = p
            .typed_ast
            .try_get(typed_id)
            .ok_or(PickleError::InvalidTypedTree { tree: typed_id })?;
        let (TreeKind::Match(source_match), TreeKind::Match(typed_match)) =
            (&source.kind, &typed.kind)
        else {
            return Err(PickleError::SourceShape {
                tree: source_id,
                expected: SourceRole::Body,
            });
        };
        let source_match = source_match.clone();
        let typed_match = typed_match.clone();
        self.source_typed_binding(source_id, typed_id)?;
        self.paired_source_typed_span(source.position, typed.position, typed_id)?;
        let supported =
            [p.definitions.int, p.definitions.boolean, p.definitions.unit].contains(&result_type);
        let type_matches = typed.ty == result_type
            || matches!((self.ty(typed.ty)?, result_type),
            (Type::Constant(Constant::Int(_)), ty) if ty == p.definitions.int)
            || matches!((self.ty(typed.ty)?, result_type), (Type::Constant(Constant::Boolean(_)), ty) if ty == p.definitions.boolean)
            || matches!((self.ty(typed.ty)?, result_type), (Type::Constant(Constant::Unit), ty) if ty == p.definitions.unit);
        if !supported || !type_matches {
            return Err(PickleError::BodyType {
                tree: typed_id,
                ty: typed.ty,
            });
        }
        if source_match.cases.is_empty() || source_match.cases.len() != typed_match.cases.len() {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        }
        self.validate_int_expression(
            source_match.selector,
            typed_match.selector,
            method,
            depth + 1,
            visited,
        )?;
        for (ordinal, (&source_case, &typed_case)) in source_match
            .cases
            .iter()
            .zip(&typed_match.cases)
            .enumerate()
        {
            self.charge(2)?;
            if !visited.insert(typed_case) {
                return Err(PickleError::RepeatedExpression { tree: typed_case });
            }
            let source = self.source_tree(source_case)?;
            let typed = p
                .typed_ast
                .try_get(typed_case)
                .ok_or(PickleError::InvalidTypedTree { tree: typed_case })?;
            self.source_typed_binding(source_case, typed_case)?;
            self.paired_source_typed_span(source.position, typed.position, typed_case)?;
            let (TreeKind::CaseDef(source_case), TreeKind::CaseDef(typed_case_def)) =
                (&source.kind, &typed.kind)
            else {
                return Err(PickleError::SourceShape {
                    tree: source_case,
                    expected: SourceRole::Body,
                });
            };
            let source_case = *source_case;
            let case = *typed_case_def;
            let body = p
                .typed_ast
                .try_get(case.body)
                .ok_or(PickleError::InvalidTypedTree { tree: case.body })?;
            if source_case.guard.is_some() || case.guard.is_some() || typed.ty != body.ty {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                );
            }
            let pattern = p
                .typed_ast
                .try_get(case.pattern)
                .ok_or(PickleError::InvalidTypedTree { tree: case.pattern })?;
            let source_pattern = self.source_tree(source_case.pattern)?;
            let last = ordinal + 1 == source_match.cases.len();
            if last {
                self.source_typed_binding(source_case.pattern, case.pattern)?;
                self.paired_source_typed_span(
                    source_pattern.position,
                    pattern.position,
                    case.pattern,
                )?;
                if !matches!((&source_pattern.kind, &pattern.kind), (TreeKind::Ident(source), TreeKind::Ident(typed))
                    if source.name == typed.name && !source.backquoted && !typed.backquoted
                        && source.name.is_term() && self.name_text(source.name.text())? == "_")
                    || (pattern.ty != p.definitions.int
                        && !matches!(self.ty(pattern.ty)?, Type::Constant(Constant::Int(_))))
                {
                    return Err(
                        self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                    );
                }
                if !visited.insert(case.pattern) {
                    return Err(PickleError::RepeatedExpression { tree: case.pattern });
                }
            } else {
                if !matches!(&pattern.kind, TreeKind::Literal(literal) if matches!(literal.value, Constant::Int(_)))
                {
                    return Err(
                        self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                    );
                }
                self.validate_int_expression(
                    source_case.pattern,
                    case.pattern,
                    method,
                    depth + 2,
                    visited,
                )?;
            }
            self.validate_match_body(
                source_case.body,
                case.body,
                method,
                result_type,
                depth + 2,
                visited,
            )?;
        }
        Ok(())
    }

    fn validate_match_body(
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
        let source = self.source_tree(source_id)?;
        let typed = self
            .plan
            .typed_ast
            .try_get(typed_id)
            .ok_or(PickleError::InvalidTypedTree { tree: typed_id })?;
        // The parser retains case-body blocks even for a single literal.
        if let (TreeKind::Block(source_block), TreeKind::Block(typed_block)) =
            (&source.kind, &typed.kind)
            && source_block.stats.is_empty()
            && typed_block.stats.is_empty()
        {
            let source_expr = source_block.expr;
            let typed_expr = typed_block.expr;
            self.charge(1)?;
            if !visited.insert(typed_id) {
                return Err(PickleError::RepeatedExpression { tree: typed_id });
            }
            self.source_typed_binding(source_id, typed_id)?;
            self.paired_source_typed_span(source.position, typed.position, typed_id)?;
            if self
                .plan
                .typed_ast
                .try_get(typed_expr)
                .is_none_or(|expr| expr.ty != typed.ty)
            {
                return Err(PickleError::BodyType {
                    tree: typed_id,
                    ty: typed.ty,
                });
            }
            return self.validate_match_body(
                source_expr,
                typed_expr,
                method,
                result_type,
                depth + 1,
                visited,
            );
        }
        self.validate_method_expression_inner(
            Some(source_id),
            Some(typed_id),
            method,
            result_type,
            depth,
            (Some(visited), false),
        )
    }
}
