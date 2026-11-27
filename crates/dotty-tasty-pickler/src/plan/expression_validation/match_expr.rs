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
        self.validate_match_flow(
            source_id,
            typed_id,
            method,
            result_type,
            depth,
            visited,
            false,
        )
        .map(|_| ())
    }

    pub(super) fn validate_return_match(
        &mut self,
        source_id: TreeId<Untyped>,
        typed_id: TreeId<Typed>,
        method: &MethodPicklePlan,
        result_type: TypeId,
        depth: usize,
        visited: &mut HashSet<TreeId<Typed>>,
    ) -> Result<bool, PickleError> {
        self.validate_match_flow(
            source_id,
            typed_id,
            method,
            result_type,
            depth,
            visited,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_match_flow(
        &mut self,
        source_id: TreeId<Untyped>,
        typed_id: TreeId<Typed>,
        method: &MethodPicklePlan,
        result_type: TypeId,
        depth: usize,
        visited: &mut HashSet<TreeId<Typed>>,
        returning: bool,
    ) -> Result<bool, PickleError> {
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
        // Check both lengths before allocating private traversal copies.
        if source_match.cases.len() > self.limits.max_items
            || typed_match.cases.len() > self.limits.max_items
        {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::Items,
                limit: self.limits.max_items,
            });
        }
        let source_match = source_match.clone();
        let typed_match = typed_match.clone();
        self.source_typed_binding(source_id, typed_id)?;
        self.paired_source_typed_span(source.position, typed.position, typed_id)?;
        let supported = [
            p.definitions.int,
            p.definitions.boolean,
            p.definitions.unit,
            p.definitions.long,
            p.definitions.float,
            p.definitions.double,
        ]
        .contains(&result_type);
        let bottom = typed.ty == p.definitions.nothing_type;
        let type_matches = (returning && bottom)
            || typed.ty == result_type
            || matches!((self.ty(typed.ty)?, result_type), (Type::Constant(Constant::Long(_)), ty) if ty == p.definitions.long)
            || matches!((self.ty(typed.ty)?, result_type), (Type::Constant(Constant::FloatBits(_)), ty) if ty == p.definitions.float)
            || matches!((self.ty(typed.ty)?, result_type), (Type::Constant(Constant::DoubleBits(_)), ty) if ty == p.definitions.double)
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
        let selector =
            p.typed_ast
                .try_get(typed_match.selector)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: typed_match.selector,
                })?;
        let selector_type = self
            .primitive_flow_type(selector.ty)?
            .filter(|ty| *ty == p.definitions.int || *ty == p.definitions.boolean)
            .ok_or(PickleError::BodyType {
                tree: typed_match.selector,
                ty: selector.ty,
            })?;
        if selector_type == p.definitions.int {
            self.validate_int_expression(
                source_match.selector,
                typed_match.selector,
                method,
                depth + 1,
                visited,
            )?;
        } else {
            self.validate_if_condition(
                source_match.selector,
                typed_match.selector,
                method,
                depth + 1,
                visited,
            )?;
        }
        let mut all_exit = returning;
        let mut semantic_join = None;
        let mut boolean_coverage = 0u8;
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
            if source_case.guard.is_some() != case.guard.is_some() || typed.ty != body.ty {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                );
            }
            let visible = self.visible_locals.clone();
            let (wildcard, coverage) = self.validate_case_pattern(
                source_case.pattern,
                case.pattern,
                selector_type,
                method,
                depth + 2,
                visited,
                false,
            )?;
            let last = ordinal + 1 == source_match.cases.len();
            if case.guard.is_none() {
                boolean_coverage |= coverage;
            }
            let fallback = wildcard && case.guard.is_none();
            let exhaustive_boolean = selector_type == p.definitions.boolean
                && case.guard.is_none()
                && boolean_coverage == 3;
            if (fallback && !last) || (last && !fallback && !exhaustive_boolean) {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                );
            }
            if let (Some(source_guard), Some(guard)) = (source_case.guard, case.guard) {
                self.validate_if_condition(source_guard, guard, method, depth + 2, visited)?;
            }
            if returning {
                if body.ty != p.definitions.nothing_type {
                    let ty = self
                        .primitive_flow_type(body.ty)?
                        .ok_or(PickleError::BodyType {
                            tree: case.body,
                            ty: body.ty,
                        })?;
                    if semantic_join.is_some_and(|prior| prior != ty) {
                        return Err(PickleError::BodyType {
                            tree: typed_id,
                            ty: typed.ty,
                        });
                    }
                    semantic_join = Some(ty);
                }
                all_exit &= self.validate_return_flow(
                    source_case.body,
                    case.body,
                    method,
                    result_type,
                    depth + 2,
                    visited,
                )?;
            } else {
                self.validate_match_body(
                    source_case.body,
                    case.body,
                    method,
                    result_type,
                    depth + 2,
                    visited,
                )?;
            }
            self.visible_locals = visible;
        }
        // A constant Boolean selector and one binder case can retain the
        // unreachable tail's singleton type. Only proven exits may bypass the
        // canonical join check; no normal value path is widened by this rule.
        let boolean_exit_singleton = all_exit
            && result_type == p.definitions.boolean
            && semantic_join == Some(p.definitions.boolean)
            && matches!(self.ty(typed.ty)?, Type::Constant(Constant::Boolean(_)));
        if (bottom && !all_exit)
            || (returning
                && typed.ty != semantic_join.unwrap_or(p.definitions.nothing_type)
                && !boolean_exit_singleton)
        {
            return Err(PickleError::BodyType {
                tree: typed_id,
                ty: typed.ty,
            });
        }
        Ok(all_exit)
    }

    // Return irrefutability and a two-bit Boolean coverage summary obtained
    // during the same authenticated, budgeted traversal (false=1, true=2).
    #[allow(clippy::too_many_arguments)]
    fn validate_case_pattern(
        &mut self,
        source_id: TreeId<Untyped>,
        typed_id: TreeId<Typed>,
        selector_type: TypeId,
        method: &MethodPicklePlan,
        depth: usize,
        visited: &mut HashSet<TreeId<Typed>>,
        literal_only: bool,
    ) -> Result<(bool, u8), PickleError> {
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
        if literal_only
            && !matches!(
                &typed.kind,
                TreeKind::Alternative(_)
                    | TreeKind::Literal(dotty_core::ast::Literal {
                        value: Constant::Int(_) | Constant::Boolean(_)
                    })
            )
        {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        }
        if let TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) = &source.kind {
            self.source_typed_binding(source_id, typed_id)?;
            self.span(source.position)?;
            self.charge(1)?;
            return self.validate_case_pattern(
                parens.inner,
                typed_id,
                selector_type,
                method,
                depth + 1,
                visited,
                literal_only,
            );
        }
        if let TreeKind::Alternative(alternative) = &typed.kind {
            self.charge(1)?;
            self.source_typed_binding(source_id, typed_id)?;
            self.paired_source_typed_span(source.position, typed.position, typed_id)?;
            if !visited.insert(typed_id) {
                return Err(PickleError::RepeatedExpression { tree: typed_id });
            }
            let TreeKind::Alternative(source_alternative) = &source.kind else {
                return Err(PickleError::SourceShape {
                    tree: source_id,
                    expected: SourceRole::Body,
                });
            };
            if typed.ty != selector_type {
                return Err(PickleError::BodyType {
                    tree: typed_id,
                    ty: typed.ty,
                });
            }
            if alternative.alternatives.len() > self.limits.max_items
                || source_alternative.alternatives.len() > self.limits.max_items
            {
                return Err(PickleError::LimitExceeded {
                    kind: LimitKind::Items,
                    limit: self.limits.max_items,
                });
            }
            if alternative.alternatives.is_empty()
                || alternative.alternatives.len() != source_alternative.alternatives.len()
            {
                return Err(PickleError::SourceShape {
                    tree: source_id,
                    expected: SourceRole::Body,
                });
            }
            // Keep each original child edge and its order, including nested
            // alternatives. No declaration identity is reconstructed here.
            let children: Vec<_> = source_alternative
                .alternatives
                .iter()
                .copied()
                .zip(alternative.alternatives.iter().copied())
                .collect();
            let mut coverage = 0;
            for (source, typed) in children {
                let (_, child_coverage) = self.validate_case_pattern(
                    source,
                    typed,
                    selector_type,
                    method,
                    depth + 1,
                    visited,
                    true,
                )?;
                coverage |= child_coverage;
            }
            return Ok((false, coverage));
        }
        if let TreeKind::Bind(bind) = typed.kind {
            self.charge(1)?;
            self.source_typed_binding(source_id, typed_id)?;
            self.paired_source_typed_span(source.position, typed.position, typed_id)?;
            if !visited.insert(typed_id) {
                return Err(PickleError::RepeatedExpression { tree: typed_id });
            }
            let binding = *self.pattern_bindings.get(&source_id).ok_or_else(|| {
                self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
            })?;
            let symbol = self.symbol(binding.symbol)?;
            let name = self.name_text(bind.name.text())?;
            let value_type = self.complete(binding.symbol)?;
            if binding.typed_declaration != typed_id
                || bind.given
                || !bind.name.is_term()
                || name == "_"
                || symbol.name != bind.name
                || symbol.kind != SymbolKind::Local
                || symbol.owner != Some(method.symbol)
                || symbol.origin != SymbolOrigin::Source(p.source.id)
                || symbol.position != source.position
                || !symbol.flags.is_empty()
                || !symbol.annotations.is_empty()
                || symbol.visibility != Visibility::Public
                || !self.match_pattern_type(value_type, selector_type)?
                || !matches!(self.ty(typed.ty)?, Type::TermRef { prefix, target: TermRefTarget::Symbol(target) } if *target == binding.symbol && *prefix == p.definitions.no_prefix)
                || !p.store.scopes.contains(binding.scope)
                || p.store.scopes.get(binding.scope).owner != Some(method.symbol)
                || p.store.scopes.get(binding.scope).lookup_all(&symbol.name) != [binding.symbol]
            {
                return Err(PickleError::Reference {
                    symbol: binding.symbol,
                    reason: crate::ReferenceError::Owner,
                });
            }
            if !self.registered_local_symbols.insert(binding.symbol)
                || !self.seen_local_symbols.insert(binding.symbol)
            {
                return Err(PickleError::DuplicateDeclaration {
                    symbol: binding.symbol,
                });
            }
            let coverage = match &source.kind {
                TreeKind::Ident(ident)
                    if !ident.backquoted
                        && ident.name == bind.name
                        && name
                            .chars()
                            .next()
                            .is_some_and(|c| c == '_' || c.is_lowercase()) =>
                {
                    self.charge(1)?;
                    if !visited.insert(bind.body) {
                        return Err(PickleError::RepeatedExpression { tree: bind.body });
                    }
                    let child = p
                        .typed_ast
                        .try_get(bind.body)
                        .ok_or(PickleError::InvalidTypedTree { tree: bind.body })?;
                    if !matches!(&child.kind, TreeKind::Ident(ident) if ident.name.is_term() && !ident.backquoted && self.name_text(ident.name.text())? == "_")
                        || child.ty != value_type
                    {
                        return Err(PickleError::BodyType {
                            tree: bind.body,
                            ty: child.ty,
                        });
                    }
                    (true, 3)
                }
                TreeKind::Bind(source_bind)
                    if !source_bind.given && source_bind.name == bind.name =>
                {
                    if matches!(
                        p.typed_ast.try_get(bind.body).map(|n| &n.kind),
                        Some(TreeKind::Bind(_) | TreeKind::Alternative(_))
                    ) {
                        return Err(self
                            .unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
                    }
                    self.validate_case_pattern(
                        source_bind.body,
                        bind.body,
                        selector_type,
                        method,
                        depth + 1,
                        visited,
                        false,
                    )?
                }
                _ => {
                    return Err(
                        self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                    );
                }
            };
            self.visible_locals.insert(binding.symbol);
            return Ok(coverage);
        }
        if matches!(&typed.kind, TreeKind::Ident(ident) if ident.name.is_term() && !ident.backquoted && self.name_text(ident.name.text())? == "_")
        {
            self.charge(1)?;
            self.source_typed_binding(source_id, typed_id)?;
            self.paired_source_typed_span(source.position, typed.position, typed_id)?;
            if !matches!((&source.kind, &typed.kind), (TreeKind::Ident(a), TreeKind::Ident(b)) if a.name == b.name && !a.backquoted)
                || !self.match_pattern_type(typed.ty, selector_type)?
            {
                return Err(PickleError::BodyType {
                    tree: typed_id,
                    ty: typed.ty,
                });
            }
            if !visited.insert(typed_id) {
                return Err(PickleError::RepeatedExpression { tree: typed_id });
            }
            return Ok((true, 3));
        }
        if !matches!(&typed.kind, TreeKind::Literal(literal) if matches!(literal.value, Constant::Int(_) | Constant::Boolean(_)))
        {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        }
        if !self.match_pattern_type(typed.ty, selector_type)? {
            return Err(PickleError::BodyType {
                tree: typed_id,
                ty: typed.ty,
            });
        }
        if selector_type == p.definitions.int {
            self.validate_int_expression(source_id, typed_id, method, depth, visited)?;
        } else {
            self.validate_if_condition(source_id, typed_id, method, depth, visited)?;
        }
        let coverage = match &typed.kind {
            TreeKind::Literal(dotty_core::ast::Literal {
                value: Constant::Boolean(value),
            }) => {
                if *value {
                    2
                } else {
                    1
                }
            }
            _ => 0,
        };
        Ok((false, coverage))
    }

    fn match_pattern_type(&self, ty: TypeId, selector_type: TypeId) -> Result<bool, PickleError> {
        Ok(ty == selector_type
            || match self.ty(ty)? {
                Type::Constant(Constant::Int(_)) => selector_type == self.plan.definitions.int,
                Type::Constant(Constant::Boolean(_)) => {
                    selector_type == self.plan.definitions.boolean
                }
                _ => false,
            })
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
