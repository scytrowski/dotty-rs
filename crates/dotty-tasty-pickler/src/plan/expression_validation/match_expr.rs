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
            if source_case.guard.is_some() != case.guard.is_some() || typed.ty != body.ty {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                );
            }
            let visible = self.visible_locals.clone();
            let wildcard = self.validate_int_case_pattern(
                source_case.pattern,
                case.pattern,
                method,
                depth + 2,
                visited,
            )?;
            let last = ordinal + 1 == source_match.cases.len();
            if (wildcard && case.guard.is_none()) != last {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                );
            }
            if let (Some(source_guard), Some(guard)) = (source_case.guard, case.guard) {
                self.validate_if_condition(source_guard, guard, method, depth + 2, visited)?;
            }
            self.validate_match_body(
                source_case.body,
                case.body,
                method,
                result_type,
                depth + 2,
                visited,
            )?;
            self.visible_locals = visible;
        }
        Ok(())
    }

    fn validate_int_case_pattern(
        &mut self,
        source_id: TreeId<Untyped>,
        typed_id: TreeId<Typed>,
        method: &MethodPicklePlan,
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
                || (value_type != p.definitions.int
                    && !matches!(self.ty(value_type)?, Type::Constant(Constant::Int(_))))
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
            let wildcard = match &source.kind {
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
                    true
                }
                TreeKind::Bind(source_bind)
                    if !source_bind.given && source_bind.name == bind.name =>
                {
                    if matches!(
                        p.typed_ast.try_get(bind.body).map(|n| &n.kind),
                        Some(TreeKind::Bind(_))
                    ) {
                        return Err(self
                            .unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
                    }
                    self.validate_int_case_pattern(
                        source_bind.body,
                        bind.body,
                        method,
                        depth + 1,
                        visited,
                    )?
                }
                _ => {
                    return Err(
                        self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                    );
                }
            };
            self.visible_locals.insert(binding.symbol);
            return Ok(wildcard);
        }
        if matches!(&typed.kind, TreeKind::Ident(ident) if ident.name.is_term() && !ident.backquoted && self.name_text(ident.name.text())? == "_")
        {
            self.charge(1)?;
            self.source_typed_binding(source_id, typed_id)?;
            self.paired_source_typed_span(source.position, typed.position, typed_id)?;
            if !matches!((&source.kind, &typed.kind), (TreeKind::Ident(a), TreeKind::Ident(b)) if a.name == b.name && !a.backquoted)
                || (typed.ty != p.definitions.int
                    && !matches!(self.ty(typed.ty)?, Type::Constant(Constant::Int(_))))
            {
                return Err(PickleError::BodyType {
                    tree: typed_id,
                    ty: typed.ty,
                });
            }
            if !visited.insert(typed_id) {
                return Err(PickleError::RepeatedExpression { tree: typed_id });
            }
            return Ok(true);
        }
        if !matches!(&typed.kind, TreeKind::Literal(literal) if matches!(literal.value, Constant::Int(_)))
        {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        }
        self.validate_int_expression(source_id, typed_id, method, depth, visited)?;
        Ok(false)
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
