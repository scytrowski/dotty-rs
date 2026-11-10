use super::*;

impl Validator<'_, '_> {
    pub(super) fn validate_local_value_statement(
        &mut self,
        source_id: TreeId<Untyped>,
        typed_id: TreeId<Typed>,
        method: &MethodPicklePlan,
        reference_local_type: Option<TypeId>,
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
        let (TreeKind::ValDef(source_val), TreeKind::ValDef(typed_val)) =
            (&source.kind, &typed.kind)
        else {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        };

        let binding = *self.local_bindings.get(&source_id).ok_or_else(|| {
            self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
        })?;
        if binding.typed_declaration != typed_id {
            return Err(PickleError::BodyBinding {
                source: self.plan.source.id,
                tree: source_id,
                expected: binding.typed_declaration,
                actual: Some(typed_id),
            });
        }
        if !self.seen_local_bindings.insert(source_id)
            || !self.seen_local_symbols.insert(binding.symbol)
        {
            return Err(PickleError::DuplicateDeclaration {
                symbol: binding.symbol,
            });
        }

        let symbol = self.symbol(binding.symbol)?;
        self.kind(binding.symbol, SymbolKind::Local)?;
        self.owner(binding.symbol, Some(method.symbol))?;
        if symbol.origin != SymbolOrigin::Source(self.plan.source.id) {
            return Err(PickleError::SourceOrigin {
                symbol: binding.symbol,
                expected: self.plan.source.id,
                actual: symbol.origin,
            });
        }
        self.span(symbol.position)?;
        self.charge(source_val.metadata.modifiers.len())?;
        let mutable = match source_val.metadata.modifiers.as_slice() {
            [] => false,
            [dotty_core::ast::Modifier::Var] => true,
            _ => {
                return Err(
                    self.unsupported(UnsupportedFeature::SourceMetadata, Some(binding.symbol))
                );
            }
        };
        if reference_local_type.is_some() && mutable {
            return Err(self.unsupported(UnsupportedFeature::SourceMetadata, Some(binding.symbol)));
        }
        if source_val.metadata.visibility.is_some() || !source_val.metadata.annotations.is_empty() {
            return Err(self.unsupported(UnsupportedFeature::SourceMetadata, Some(binding.symbol)));
        }
        let expected_flags = if mutable {
            SymbolFlags::MUTABLE
        } else {
            SymbolFlags::EMPTY
        };
        if symbol.visibility != Visibility::Public
            || symbol.flags != expected_flags
            || !symbol.annotations.is_empty()
        {
            return Err(self.unsupported(UnsupportedFeature::SymbolMetadata, Some(binding.symbol)));
        }
        if !source_val.name.as_name().is_term()
            || source_val.name != typed_val.name
            || *source_val.name.as_name() != symbol.name
        {
            return Err(PickleError::NameMismatch {
                symbol: binding.symbol,
                tree: source_id,
            });
        }

        if !self.plan.store.scopes.contains(binding.scope) {
            return Err(PickleError::InvalidScope {
                scope: binding.scope,
            });
        }
        let scope = self.plan.store.scopes.get(binding.scope);
        if scope.owner != Some(method.symbol) || scope.lookup_all(&symbol.name) != [binding.symbol]
        {
            return Err(PickleError::ScopeMembers {
                owner: method.symbol,
            });
        }

        let local_type = self.complete(binding.symbol)?;
        if (local_type != self.plan.definitions.int
            && !reference_local_type.is_some_and(|expected| {
                expected == local_type && self.is_planned_sibling_class_type(local_type, None)
            }))
            || typed.ty != local_type
        {
            return Err(PickleError::BodyType {
                tree: typed_id,
                ty: typed.ty,
            });
        }
        let source_tpt = self.source_tree(source_val.tpt)?;
        let typed_tpt =
            self.plan
                .typed_ast
                .try_get(typed_val.tpt)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: typed_val.tpt,
                })?;
        if reference_local_type.is_some()
            && (source.position.is_none()
                || typed.position.is_none()
                || source_tpt.position.is_none()
                || typed_tpt.position.is_none())
        {
            return Err(PickleError::BodySpan { tree: typed_id });
        }
        let declared_name = match self.ty(local_type)? {
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } => self.symbol(*symbol)?.name,
            _ => return Err(PickleError::UnsupportedTypeReference { ty: local_type }),
        };
        if reference_local_type.is_some()
            && !self.is_planned_sibling_class_type(local_type, Some(declared_name))
        {
            return Err(PickleError::UnsupportedTypeReference { ty: local_type });
        }
        if !matches!(&source_tpt.kind, TreeKind::Ident(ident) if ident.name == declared_name)
            || !matches!(typed_tpt.kind, TreeKind::TypeTree(_))
            || typed_tpt.ty != local_type
        {
            return Err(PickleError::UnsupportedTypeReference { ty: typed_tpt.ty });
        }
        self.source_typed_binding(source_val.tpt, typed_val.tpt)?;
        self.paired_source_typed_span(source_tpt.position, typed_tpt.position, typed_val.tpt)?;

        let (Some(source_rhs), Some(typed_rhs)) = (source_val.rhs, typed_val.rhs) else {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        };
        if reference_local_type.is_some()
            && (self.source_tree(source_rhs)?.position.is_none()
                || self
                    .plan
                    .typed_ast
                    .try_get(typed_rhs)
                    .ok_or(PickleError::InvalidTypedTree { tree: typed_rhs })?
                    .position
                    .is_none())
        {
            return Err(PickleError::BodySpan { tree: typed_rhs });
        }
        if self
            .plan
            .typed_bindings
            .typed_at(self.plan.source.id, source_rhs)
            != Some(typed_rhs)
        {
            return Err(PickleError::BodyBinding {
                source: self.plan.source.id,
                tree: source_rhs,
                expected: typed_rhs,
                actual: self
                    .plan
                    .typed_bindings
                    .typed_at(self.plan.source.id, source_rhs),
            });
        }
        let rhs_depth = if matches!(
            self.plan.typed_ast.try_get(typed_rhs).map(|rhs| &rhs.kind),
            Some(TreeKind::Apply(_) | TreeKind::Block(_) | TreeKind::If(_))
        ) {
            depth + 1
        } else {
            depth
        };
        if let Some(reference_type) = reference_local_type {
            if reference_type != local_type {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                );
            }
            let initializers = self
                .plan
                .typed_bindings
                .local_counter_initializers(self.plan.source.id, method.symbol);
            let alias = self
                .plan
                .typed_bindings
                .local_reference_alias(self.plan.source.id, method.symbol);
            if let Some(alias) = alias.filter(|alias| alias.source_declaration == source_id) {
                self.validate_local_reference_alias_initializer(
                    alias, binding, local_type, method, visited,
                )?;
            } else if let Some(initializer) = initializers
                .iter()
                .find(|initializer| initializer.source_declaration == source_id)
                .copied()
            {
                let pair = self
                    .plan
                    .typed_bindings
                    .formal_local_constructor_call_pair(self.plan.source.id, method.symbol)
                    .ok_or_else(|| {
                        self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                    })?;
                let paired_local = initializers.len() == 2
                    || pair.arguments.iter().any(|argument| {
                        matches!(
                            argument.origin,
                            crate::FormalLocalConstructorCallArgumentOrigin::LocalValue {
                                symbol,
                                ..
                            } if symbol == initializer.local
                        )
                    });
                if initializer.typed_declaration != typed_id
                    || initializer.source_initializer != source_rhs
                    || initializer.typed_initializer != typed_rhs
                    || initializer.call.source_apply != source_rhs
                    || initializer.call.typed_apply != typed_rhs
                    || initializer.source_type_tree != source_val.tpt
                    || initializer.typed_type_tree != typed_val.tpt
                    || initializer.local != binding.symbol
                    || initializer.scope != binding.scope
                    || initializer.local_type != local_type
                    || !paired_local
                {
                    return Err(
                        self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                    );
                }
                let source_initializer = self.source_tree(source_rhs)?;
                let typed_initializer = self
                    .plan
                    .typed_ast
                    .try_get(typed_rhs)
                    .ok_or(PickleError::InvalidTypedTree { tree: typed_rhs })?;
                self.source_typed_binding(source_rhs, typed_rhs)?;
                self.paired_source_typed_span(
                    source_initializer.position,
                    typed_initializer.position,
                    typed_rhs,
                )?;
                if !visited.insert(typed_rhs) {
                    return Err(PickleError::RepeatedExpression { tree: typed_rhs });
                }
                self.validate_nested_constructor_argument(
                    initializer.call,
                    initializer.argument_formal,
                    local_type,
                    method,
                    rhs_depth,
                    visited,
                )?;
            } else {
                let binding = self
                    .plan
                    .typed_bindings
                    .planned_call(self.plan.source.id, method.symbol)
                    .ok_or_else(|| {
                        self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                    })?;
                if binding.source_apply != source_rhs || binding.typed_apply != typed_rhs {
                    return Err(
                        self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                    );
                }
                if !visited.insert(typed_rhs) {
                    return Err(PickleError::RepeatedExpression { tree: typed_rhs });
                }
                self.validate_planned_call_argument(
                    binding, local_type, method, rhs_depth, visited,
                )?;
            }
        } else {
            self.validate_method_expression_inner(
                Some(source_rhs),
                Some(typed_rhs),
                method,
                self.plan.definitions.int,
                rhs_depth,
                (Some(visited), false),
            )?;
        }

        if self
            .visible_locals
            .iter()
            .any(|&visible| self.plan.store.symbols.get(visible).name == symbol.name)
        {
            return Err(PickleError::DuplicateDeclaration {
                symbol: binding.symbol,
            });
        }
        self.visible_locals.insert(binding.symbol);
        Ok(())
    }

    fn validate_local_reference_alias_initializer(
        &mut self,
        alias: crate::LocalReferenceAliasBinding,
        binding: LocalValueBinding,
        local_type: TypeId,
        method: &MethodPicklePlan,
        visited: &mut HashSet<TreeId<Typed>>,
    ) -> Result<(), PickleError> {
        let p = self.plan;
        let source_declaration = binding.source_tree;
        let typed_declaration = binding.typed_declaration;
        let source_initializer = alias.source_initializer;
        let typed_initializer = alias.typed_initializer;
        let source_block = self.source_tree(alias.source_block)?;
        let typed_block =
            p.typed_ast
                .try_get(alias.typed_block)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: alias.typed_block,
                })?;
        let source_alias = self.source_tree(source_declaration)?;
        let typed_alias =
            p.typed_ast
                .try_get(typed_declaration)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: typed_declaration,
                })?;
        let source_target = self.source_tree(alias.source_target)?;
        let typed_target =
            p.typed_ast
                .try_get(alias.typed_target)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: alias.typed_target,
                })?;
        let (TreeKind::Block(source_block), TreeKind::Block(typed_block)) =
            (&source_block.kind, &typed_block.kind)
        else {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        };
        let (TreeKind::ValDef(source_alias), TreeKind::ValDef(typed_alias)) =
            (&source_alias.kind, &typed_alias.kind)
        else {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        };
        let (TreeKind::Ident(source_target_ident), TreeKind::Ident(typed_target_ident)) =
            (&source_target.kind, &typed_target.kind)
        else {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        };
        let target_initializer = p
            .typed_bindings
            .local_counter_initializers(p.source.id, method.symbol)
            .into_iter()
            .find(|initializer| initializer.local == alias.target);
        let target_type = self.complete(alias.target)?;
        let target_has_same_reference_type = matches!(
            (self.ty(target_type)?, self.ty(local_type)?),
            (
                Type::TypeRef { target: target_class, .. },
                Type::TypeRef { target: alias_class, .. }
            ) if target_class == alias_class
        );
        if alias.source_declaration != source_declaration
            || alias.typed_declaration != typed_declaration
            || alias.source_initializer != source_initializer
            || alias.typed_initializer != typed_initializer
            || alias.source_target != source_initializer
            || alias.typed_target != typed_initializer
            || alias.local != binding.symbol
            || alias.scope != binding.scope
            || source_alias.rhs != Some(source_initializer)
            || typed_alias.rhs != Some(typed_initializer)
            || alias.source_type_tree != source_alias.tpt
            || alias.typed_type_tree != typed_alias.tpt
            || source_block.stats.get(2) != Some(&source_declaration)
            || typed_block.stats.get(2) != Some(&typed_declaration)
            || alias.local == alias.target
            || alias.local_type != local_type
            || p.store.symbols.get(alias.target).kind != SymbolKind::Local
            || p.store.symbols.get(alias.target).owner != Some(method.symbol)
            || p.store.symbols.get(alias.target).flags != SymbolFlags::EMPTY
            || !self.visible_locals.contains(&alias.target)
            || source_target_ident.name != self.symbol(alias.target)?.name
            || typed_target_ident.name != source_target_ident.name
            || !target_has_same_reference_type
            || !matches!(
                self.ty(typed_target.ty)?,
                Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) }
                    if *prefix == p.definitions.no_prefix && *symbol == alias.target
            )
            || target_initializer.is_none_or(|initializer| initializer.local_type != target_type)
        {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        }
        self.source_typed_binding(alias.source_target, alias.typed_target)?;
        self.paired_source_typed_span(
            source_target.position,
            typed_target.position,
            alias.typed_target,
        )?;
        if !visited.insert(alias.typed_target) {
            return Err(PickleError::RepeatedExpression {
                tree: alias.typed_target,
            });
        }
        Ok(())
    }
}
