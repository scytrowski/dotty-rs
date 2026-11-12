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
            if !self.same_reference_type(reference_type, local_type)? {
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
            let selection = self
                .plan
                .typed_bindings
                .conditional_local_reference_selection(self.plan.source.id, method.symbol);
            if let Some(selection) =
                selection.filter(|selection| selection.source_declaration == source_id)
            {
                self.validate_conditional_local_reference_initializer(
                    selection, binding, local_type, method, depth, visited,
                )?;
            } else if let Some(alias) = alias.filter(|alias| alias.source_declaration == source_id)
            {
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
                    || !self.same_reference_type(initializer.local_type, local_type)?
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

    fn validate_conditional_local_reference_initializer(
        &mut self,
        binding: crate::ConditionalLocalReferenceSelectionBinding,
        local: LocalValueBinding,
        local_type: TypeId,
        method: &MethodPicklePlan,
        depth: usize,
        visited: &mut HashSet<TreeId<Typed>>,
    ) -> Result<(), PickleError> {
        let p = self.plan;
        if depth > self.limits.expression_depth_limit() {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::ExpressionDepth,
                limit: self.limits.expression_depth_limit(),
            });
        }
        let source_block = self.source_tree(binding.source_block)?;
        let typed_block =
            p.typed_ast
                .try_get(binding.typed_block)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: binding.typed_block,
                })?;
        let source_declaration = self.source_tree(binding.source_declaration)?;
        let typed_declaration = p.typed_ast.try_get(binding.typed_declaration).ok_or(
            PickleError::InvalidTypedTree {
                tree: binding.typed_declaration,
            },
        )?;
        let source_if = self.source_tree(binding.source_if)?;
        let typed_if =
            p.typed_ast
                .try_get(binding.typed_if)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: binding.typed_if,
                })?;
        let typed_if_type = typed_if.ty;
        let (TreeKind::Block(source_block), TreeKind::Block(typed_block)) =
            (&source_block.kind, &typed_block.kind)
        else {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        };
        let (TreeKind::ValDef(source_declaration), TreeKind::ValDef(typed_declaration)) =
            (&source_declaration.kind, &typed_declaration.kind)
        else {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        };
        let (TreeKind::If(source_if), TreeKind::If(typed_if)) = (&source_if.kind, &typed_if.kind)
        else {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        };
        let actual_type = self.complete(binding.selected)?;
        let boolean_formal = self.symbol(binding.condition_formal)?;
        let initializer_pair = p
            .typed_bindings
            .local_counter_initializers(p.source.id, method.symbol);
        if binding.source_id != p.source.id
            || binding.method != method.symbol
            || binding.owner != p.class
            || binding.scope != local.scope
            || binding.source_declaration != local.source_tree
            || binding.typed_declaration != local.typed_declaration
            || binding.selected != local.symbol
            || binding.local_type != local_type
            || actual_type != local_type
            || source_block.stats.get(2) != Some(&binding.source_declaration)
            || typed_block.stats.get(2) != Some(&binding.typed_declaration)
            || source_declaration.rhs != Some(binding.source_if)
            || typed_declaration.rhs != Some(binding.typed_if)
            || binding.source_if != source_declaration.rhs.unwrap_or(binding.source_if)
            || binding.typed_if != typed_declaration.rhs.unwrap_or(binding.typed_if)
            || source_if.cond != binding.source_condition
            || typed_if.cond != binding.typed_condition
            || [source_if.then_branch, source_if.else_branch] != binding.source_arms
            || [typed_if.then_branch, typed_if.else_branch] != binding.typed_arms
            || boolean_formal.kind != SymbolKind::Parameter
            || boolean_formal.owner != Some(method.symbol)
            || self.complete(binding.condition_formal)? != p.definitions.boolean
            || !method
                .parameter_clauses
                .iter()
                .flatten()
                .any(|formal| *formal == binding.condition_formal)
            || initializer_pair.len() != 2
            || initializer_pair
                .iter()
                .any(|initializer| initializer.scope != binding.scope)
            || binding.targets[0] == binding.targets[1]
            || binding.targets.contains(&binding.selected)
            || !self.visible_locals.contains(&binding.targets[0])
            || !self.visible_locals.contains(&binding.targets[1])
        {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        }
        let Type::TypeRef {
            target: TypeRefTarget::Symbol(class),
            ..
        } = self.ty(local_type)?
        else {
            return Err(PickleError::UnsupportedTypeReference { ty: local_type });
        };
        if !self.same_reference_type(typed_if_type, local_type)? {
            return Err(PickleError::BodyType {
                tree: binding.typed_if,
                ty: typed_if_type,
            });
        }
        for (source_arm, typed_arm, target) in [
            (
                binding.source_arms[0],
                binding.typed_arms[0],
                binding.targets[0],
            ),
            (
                binding.source_arms[1],
                binding.typed_arms[1],
                binding.targets[1],
            ),
        ] {
            let source = self.source_tree(source_arm)?;
            let typed = p
                .typed_ast
                .try_get(typed_arm)
                .ok_or(PickleError::InvalidTypedTree { tree: typed_arm })?;
            let (TreeKind::Ident(source_ident), TreeKind::Ident(typed_ident)) =
                (&source.kind, &typed.kind)
            else {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                );
            };
            if source_ident.name != self.symbol(target)?.name
                || typed_ident.name != source_ident.name
                || self.symbol(target)?.kind != SymbolKind::Local
                || self.symbol(target)?.owner != Some(method.symbol)
                || !self.same_reference_type(self.complete(target)?, local_type)?
                || !initializer_pair.iter().any(|initializer| {
                    initializer.local == target
                        && self
                            .same_reference_type(initializer.local_type, local_type)
                            .unwrap_or(false)
                        && initializer.call.class == *class
                })
                || !matches!(self.ty(typed.ty)?, Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) } if *prefix == p.definitions.no_prefix && *symbol == target)
            {
                return Err(PickleError::BodyType {
                    tree: typed_arm,
                    ty: typed.ty,
                });
            }
            self.source_typed_binding(source_arm, typed_arm)?;
            self.paired_source_typed_span(source.position, typed.position, typed_arm)?;
        }
        let condition_source = self.source_tree(binding.source_condition)?;
        let condition_typed_id = p
            .typed_bindings
            .typed_at(p.source.id, binding.source_condition)
            .ok_or(PickleError::BodyBinding {
                source: p.source.id,
                tree: binding.source_condition,
                expected: binding.typed_condition,
                actual: None,
            })?;
        if condition_typed_id != binding.typed_condition {
            return Err(PickleError::BodyBinding {
                source: p.source.id,
                tree: binding.source_condition,
                expected: binding.typed_condition,
                actual: Some(condition_typed_id),
            });
        }
        let condition_typed =
            p.typed_ast
                .try_get(binding.typed_condition)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: binding.typed_condition,
                })?;
        if !matches!(&condition_source.kind, TreeKind::Ident(ident) if ident.name == boolean_formal.name)
            || !matches!(&condition_typed.kind, TreeKind::Ident(ident) if ident.name == boolean_formal.name)
            || !matches!(self.ty(condition_typed.ty)?, Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) } if *prefix == p.definitions.no_prefix && *symbol == binding.condition_formal)
        {
            return Err(PickleError::BodyType {
                tree: binding.typed_condition,
                ty: condition_typed.ty,
            });
        }
        for (source_id, typed_id) in [
            (binding.source_if, binding.typed_if),
            (binding.source_condition, binding.typed_condition),
        ] {
            self.source_typed_binding(source_id, typed_id)?;
            let source = self.source_tree(source_id)?;
            let typed = p
                .typed_ast
                .try_get(typed_id)
                .ok_or(PickleError::InvalidTypedTree { tree: typed_id })?;
            self.paired_source_typed_span(source.position, typed.position, typed_id)?;
        }
        if !visited.insert(binding.typed_if)
            || !visited.insert(binding.typed_condition)
            || binding
                .typed_arms
                .into_iter()
                .any(|arm| !visited.insert(arm))
        {
            return Err(PickleError::RepeatedExpression {
                tree: binding.typed_if,
            });
        }
        Ok(())
    }

    fn same_reference_type(&self, left: TypeId, right: TypeId) -> Result<bool, PickleError> {
        let (
            Type::TypeRef {
                prefix: left_prefix,
                target: TypeRefTarget::Symbol(left_class),
            },
            Type::TypeRef {
                prefix: right_prefix,
                target: TypeRefTarget::Symbol(right_class),
            },
        ) = (self.ty(left)?, self.ty(right)?)
        else {
            return Ok(false);
        };
        Ok(left_class == right_class && self.same_reference_prefix(*left_prefix, *right_prefix)?)
    }

    fn same_reference_prefix(&self, left: TypeId, right: TypeId) -> Result<bool, PickleError> {
        match (self.ty(left)?, self.ty(right)?) {
            (Type::NoPrefix, Type::NoPrefix) => Ok(true),
            (
                Type::TypeRef {
                    prefix: left_prefix,
                    target: TypeRefTarget::Symbol(left_owner),
                },
                Type::TypeRef {
                    prefix: right_prefix,
                    target: TypeRefTarget::Symbol(right_owner),
                },
            ) => Ok(left_owner == right_owner
                && matches!(self.ty(*left_prefix)?, Type::NoPrefix)
                && matches!(self.ty(*right_prefix)?, Type::NoPrefix)),
            _ => Ok(false),
        }
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
