use super::*;

impl Validator<'_, '_> {
    pub(super) fn validate_local_value_statement(
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
        if local_type != self.plan.definitions.int || typed.ty != local_type {
            return Err(PickleError::BodyType {
                tree: typed_id,
                ty: typed.ty,
            });
        }
        let int_symbol = self
            .ty(self.plan.definitions.int)?
            .reference_symbol()
            .ok_or(PickleError::BuiltinIdentity {
                builtin: Builtin::Int,
                ty: self.plan.definitions.int,
            })?;
        let int_name = self.symbol(int_symbol)?.name;
        let source_tpt = self.source_tree(source_val.tpt)?;
        let typed_tpt =
            self.plan
                .typed_ast
                .try_get(typed_val.tpt)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: typed_val.tpt,
                })?;
        if !matches!(&source_tpt.kind, TreeKind::Ident(ident) if ident.name == int_name)
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
        self.validate_method_expression_inner(
            Some(source_rhs),
            Some(typed_rhs),
            method,
            self.plan.definitions.int,
            rhs_depth,
            Some(visited),
        )?;

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
}
