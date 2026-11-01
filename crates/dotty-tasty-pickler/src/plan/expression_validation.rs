use super::*;

mod assignment;
mod block;
mod local;
mod while_loop;

fn validate_cross_class_target_signature(
    signature_type: TypeId,
    signature: &dotty_core::MethodType,
    int_type: TypeId,
) -> Result<(), PickleError> {
    if signature.kind != MethodKind::Plain
        || !signature.params.is_empty()
        || signature.result != int_type
    {
        return Err(PickleError::MalformedType {
            ty: signature_type,
            role: TypeRole::Parameter,
        });
    }
    Ok(())
}

impl Validator<'_, '_> {
    fn validate_int_expression(
        &mut self,
        source_id: TreeId<Untyped>,
        typed_id: TreeId<Typed>,
        method: &MethodPicklePlan,
        depth: usize,
        visited: &mut HashSet<TreeId<Typed>>,
    ) -> Result<(), PickleError> {
        let expression_depth_limit = self.limits.expression_depth_limit();
        if depth > expression_depth_limit {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::ExpressionDepth,
                limit: expression_depth_limit,
            });
        }
        let source = self.source_tree(source_id)?;
        let typed = self
            .plan
            .typed_ast
            .try_get(typed_id)
            .ok_or(PickleError::InvalidTypedTree { tree: typed_id })?;
        match (&source.kind, &typed.kind) {
            (TreeKind::PhaseSpecific(UntypedNode::Parens(parens)), _) => {
                self.source_typed_binding(source_id, typed_id)?;
                self.span(source.position)?;
                self.charge(1)?;
                self.validate_int_expression(parens.inner, typed_id, method, depth + 1, visited)
            }
            (TreeKind::PhaseSpecific(UntypedNode::InfixOp(_)), TreeKind::Apply(_)) => {
                let result = self
                    .validate_primitive_application(source_id, typed_id, method, depth, visited)?;
                if result != self.plan.definitions.int {
                    return Err(PickleError::BodyType {
                        tree: typed_id,
                        ty: self.plan.typed_ast.get(typed_id).ty,
                    });
                }
                Ok(())
            }
            (TreeKind::Apply(source_apply), TreeKind::Apply(typed_apply))
                if source_apply.args.is_empty() && typed_apply.args.is_empty() =>
            {
                self.validate_cross_class_noarg_call(source_id, typed_id, method, depth, visited)
            }
            (TreeKind::Apply(_), TreeKind::Apply(_)) => {
                self.validate_nested_application(source_id, typed_id, method, depth, visited)
            }
            (TreeKind::If(_), TreeKind::If(_)) => {
                self.validate_if_expression(source_id, typed_id, method, depth + 1, visited)?;
                Ok(())
            }
            (TreeKind::Block(_), TreeKind::Block(_)) => self.validate_block_expression(
                source_id,
                typed_id,
                method,
                self.plan.definitions.int,
                depth + 1,
                visited,
            ),
            _ => self.validate_method_expression_inner(
                Some(source_id),
                Some(typed_id),
                method,
                self.plan.definitions.int,
                depth,
                (Some(visited), false),
            ),
        }
    }

    fn validate_primitive_application(
        &mut self,
        source_id: TreeId<Untyped>,
        typed_id: TreeId<Typed>,
        method: &MethodPicklePlan,
        depth: usize,
        visited: &mut HashSet<TreeId<Typed>>,
    ) -> Result<TypeId, PickleError> {
        let expression_depth_limit = self.limits.expression_depth_limit();
        if depth > expression_depth_limit {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::ExpressionDepth,
                limit: expression_depth_limit,
            });
        }
        self.charge(1)?;
        if !visited.insert(typed_id) {
            return Err(PickleError::RepeatedExpression { tree: typed_id });
        }
        let p = self.plan;
        let source_tree = self.source_tree(source_id)?;
        let typed_tree = p
            .typed_ast
            .try_get(typed_id)
            .ok_or(PickleError::InvalidTypedTree { tree: typed_id })?;
        self.source_typed_binding(source_id, typed_id)?;
        self.paired_source_typed_span(source_tree.position, typed_tree.position, typed_id)?;
        let (TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)), TreeKind::Apply(application)) =
            (&source_tree.kind, &typed_tree.kind)
        else {
            return Err(PickleError::SourceShape {
                tree: source_id,
                expected: SourceRole::Body,
            });
        };
        if application.kind != ApplyKind::Regular || application.args.len() != 1 {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        }
        if !visited.insert(application.function) {
            return Err(PickleError::RepeatedExpression {
                tree: application.function,
            });
        }
        self.charge(1)?;
        let typed_function =
            p.typed_ast
                .try_get(application.function)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: application.function,
                })?;
        let TreeKind::Select(selection) = &typed_function.kind else {
            return Err(PickleError::SourceShape {
                tree: source_id,
                expected: SourceRole::Body,
            });
        };
        self.paired_source_typed_span(
            source_tree.position,
            typed_function.position,
            application.function,
        )?;
        let Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(target),
        } = self.ty(typed_function.ty)?
        else {
            return Err(PickleError::BodyType {
                tree: application.function,
                ty: typed_function.ty,
            });
        };
        let int_symbols = p
            .definitions
            .int_primitive_operator_symbols(p.store)
            .ok_or(PickleError::BuiltinIdentity {
                builtin: Builtin::Int,
                ty: p.definitions.int,
            })?;
        let int_index = int_symbols
            .all()
            .iter()
            .position(|&symbol| symbol == *target);
        let boolean_symbols =
            p.definitions
                .boolean_primitive_operator_ids()
                .ok_or(PickleError::BuiltinIdentity {
                    builtin: Builtin::Boolean,
                    ty: p.definitions.boolean,
                })?;
        let (operator_name, operand_type, operator_result) = if let Some(index) = int_index {
            (
                ["+", "-", "*", "/", "%", "<", "<=", ">", ">="][index],
                p.definitions.int,
                if index < 5 {
                    p.definitions.int
                } else {
                    p.definitions.boolean
                },
            )
        } else if *target == boolean_symbols.and_then {
            ("&&", p.definitions.boolean, p.definitions.boolean)
        } else if *target == boolean_symbols.or_else {
            ("||", p.definitions.boolean, p.definitions.boolean)
        } else {
            return Err(PickleError::Reference {
                symbol: *target,
                reason: crate::ReferenceError::NotRegistered,
            });
        };
        let typed_result = self.ty(typed_tree.ty)?;
        let result_matches = typed_tree.ty == operator_result
            || matches!((operator_result, typed_result), (ty, Type::Constant(Constant::Int(_))) if ty == p.definitions.int)
            || matches!((operator_result, typed_result), (ty, Type::Constant(Constant::Boolean(_))) if ty == p.definitions.boolean);
        if !result_matches {
            return Err(PickleError::BodyType {
                tree: typed_id,
                ty: typed_tree.ty,
            });
        }
        let source_operator = self.name_text(infix.op.text())?;
        let selected_name = self.symbol(*target)?.name;
        let selected_operator = self.name_text(selected_name.text())?;
        if source_operator != operator_name
            || selected_operator != operator_name
            || selection.name != selected_name
        {
            return Err(PickleError::SourceShape {
                tree: source_id,
                expected: SourceRole::Body,
            });
        }
        let typed_receiver =
            p.typed_ast
                .try_get(selection.qualifier)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: selection.qualifier,
                })?;
        if *prefix != typed_receiver.ty {
            return Err(PickleError::Reference {
                symbol: *target,
                reason: crate::ReferenceError::Owner,
            });
        }
        if operand_type == p.definitions.int {
            self.validate_int_expression(
                infix.left,
                selection.qualifier,
                method,
                depth + 1,
                visited,
            )?;
            self.validate_int_expression(
                infix.right,
                application.args[0],
                method,
                depth + 1,
                visited,
            )?;
        } else {
            self.validate_if_condition(
                infix.left,
                selection.qualifier,
                method,
                depth + 1,
                visited,
            )?;
            self.validate_if_condition(
                infix.right,
                application.args[0],
                method,
                depth + 1,
                visited,
            )?;
        }
        Ok(operator_result)
    }

    fn validate_boolean_prefix(
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
        let (TreeKind::PhaseSpecific(UntypedNode::PrefixOp(prefix)), TreeKind::Select(selection)) =
            (&source.kind, &typed.kind)
        else {
            return Err(PickleError::SourceShape {
                tree: source_id,
                expected: SourceRole::Body,
            });
        };
        if self.name_text(prefix.op.text())? != "!" {
            return Err(PickleError::SourceShape {
                tree: source_id,
                expected: SourceRole::Body,
            });
        }
        let Type::TermRef {
            prefix: selected_prefix,
            target: TermRefTarget::Symbol(target),
        } = self.ty(typed.ty)?
        else {
            return Err(PickleError::BodyType {
                tree: typed_id,
                ty: typed.ty,
            });
        };
        let symbols = self
            .plan
            .definitions
            .boolean_primitive_operator_ids()
            .ok_or(PickleError::BuiltinIdentity {
                builtin: Builtin::Boolean,
                ty: self.plan.definitions.boolean,
            })?;
        let function_name = self.symbol(*target)?.name;
        if *target != symbols.unary_not
            || self.name_text(function_name.text())? != "unary_!"
            || selection.name != function_name
            || self.complete(*target)? != self.plan.definitions.boolean
        {
            return Err(PickleError::Reference {
                symbol: *target,
                reason: crate::ReferenceError::NotRegistered,
            });
        }
        let receiver = self.plan.typed_ast.try_get(selection.qualifier).ok_or(
            PickleError::InvalidTypedTree {
                tree: selection.qualifier,
            },
        )?;
        if *selected_prefix != receiver.ty {
            return Err(PickleError::Reference {
                symbol: *target,
                reason: crate::ReferenceError::Owner,
            });
        }
        self.validate_if_condition(
            prefix.operand,
            selection.qualifier,
            method,
            depth + 1,
            visited,
        )
    }

    fn same_class_method_target(
        &mut self,
        target: SymbolId,
        source_name: dotty_core::Name,
        prefix: TypeId,
        source_tree: TreeId<Untyped>,
    ) -> Result<(), PickleError> {
        let p = self.plan;
        let target_method = p
            .methods
            .iter()
            .find(|candidate| candidate.symbol == target)
            .ok_or(PickleError::Reference {
                symbol: target,
                reason: crate::ReferenceError::NotRegistered,
            })?;
        self.kind(target, SymbolKind::Method)?;
        self.owner(target, Some(p.class))?;
        let target_tree = self.source_tree(target_method.source_tree)?;
        let TreeKind::DefDef(target_definition) = &target_tree.kind else {
            return Err(PickleError::SourceShape {
                tree: target_method.source_tree,
                expected: SourceRole::Method,
            });
        };
        if target_definition.name.as_name() != &source_name
            || !target_method.parameter_clauses.is_empty()
        {
            return Err(PickleError::NameMismatch {
                symbol: target,
                tree: source_tree,
            });
        }
        self.source_symbol(
            target,
            target_method.source_tree,
            SymbolKind::Method,
            *target_definition.name.as_name(),
        )?;
        self.metadata(&target_definition.metadata, target)?;
        let target_type = self.complete(target)?;
        if target_type != p.definitions.int {
            return Err(PickleError::MalformedType {
                ty: target_type,
                role: TypeRole::MethodResult,
            });
        }
        if !matches!(self.ty(prefix)?, Type::ThisType { class } if *class == p.class) {
            return Err(PickleError::Reference {
                symbol: target,
                reason: crate::ReferenceError::Owner,
            });
        }
        Ok(())
    }

    fn same_class_field_target(
        &mut self,
        target: SymbolId,
        source_name: dotty_core::Name,
        prefix: TypeId,
        typed_tree: TreeId<Typed>,
        result_type: TypeId,
    ) -> Result<(), PickleError> {
        let p = self.plan;
        let field = self
            .class_members
            .and_then(|members| {
                members.iter().find_map(|member| match member {
                    ClassMemberPicklePlan::Field(field) if field.symbol == target => Some(*field),
                    _ => None,
                })
            })
            .ok_or(PickleError::Reference {
                symbol: target,
                reason: crate::ReferenceError::NotRegistered,
            })?;
        let record = self.symbol(target)?;
        if record.kind != SymbolKind::Field
            || record.owner != Some(p.class)
            || record.origin != SymbolOrigin::Source(p.source.id)
            || record.visibility != Visibility::Private
            || !(record.flags.is_empty() || record.flags == SymbolFlags::MUTABLE)
            || !record.annotations.is_empty()
            || record.name != source_name
            || p.source_index.symbol_at(p.source.id, field.source_tree) != Some(target)
            || p.source_index.definition_of(target)
                != Some(SourceDefinition::Canonical {
                    source: p.source.id,
                    tree: field.source_tree,
                })
        {
            return Err(PickleError::Reference {
                symbol: target,
                reason: crate::ReferenceError::Metadata,
            });
        }
        self.span(record.position)?;
        if self.complete(target)? != field.declared_type || field.declared_type != result_type {
            return Err(PickleError::BodyType {
                tree: typed_tree,
                ty: result_type,
            });
        }
        let immutable_planned_reference_field = self.reference_fields.contains(&target)
            && record.flags.is_empty()
            && self.is_planned_sibling_class_type(field.declared_type, None);
        if result_type != p.definitions.int
            && result_type != p.definitions.boolean
            && !immutable_planned_reference_field
        {
            return Err(PickleError::UnsupportedTypeReference { ty: result_type });
        }
        if !matches!(self.ty(prefix)?, Type::ThisType { class } if *class == p.class) {
            return Err(PickleError::Reference {
                symbol: target,
                reason: crate::ReferenceError::Owner,
            });
        }
        Ok(())
    }

    fn unqualified_name_is_shadowed(
        &self,
        source_name: dotty_core::Name,
        method: &MethodPicklePlan,
    ) -> bool {
        method.parameter_clauses.iter().flatten().any(|parameter| {
            self.plan.store.symbols.contains(*parameter)
                && self.plan.store.symbols.get(*parameter).name == source_name
        }) || self.visible_locals.iter().any(|local| {
            self.plan.store.symbols.contains(*local)
                && self.plan.store.symbols.get(*local).name == source_name
                && self.plan.store.symbols.get(*local).owner == Some(method.symbol)
        })
    }

    fn validate_cross_class_noarg_call(
        &mut self,
        source_id: TreeId<Untyped>,
        typed_id: TreeId<Typed>,
        method: &MethodPicklePlan,
        _depth: usize,
        visited: &mut HashSet<TreeId<Typed>>,
    ) -> Result<(), PickleError> {
        let p = self.plan;
        let source = self.source_tree(source_id)?;
        let typed = p
            .typed_ast
            .try_get(typed_id)
            .ok_or(PickleError::InvalidTypedTree { tree: typed_id })?;
        let (TreeKind::Apply(source_apply), TreeKind::Apply(typed_apply)) =
            (&source.kind, &typed.kind)
        else {
            return Err(PickleError::SourceShape {
                tree: source_id,
                expected: SourceRole::Body,
            });
        };
        if source_apply.kind != ApplyKind::Regular
            || typed_apply.kind != ApplyKind::Regular
            || !source_apply.args.is_empty()
            || !typed_apply.args.is_empty()
            || typed.ty != p.definitions.int
        {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        }
        self.charge(1)?;
        if !visited.insert(typed_id) {
            return Err(PickleError::RepeatedExpression { tree: typed_id });
        }
        self.source_typed_binding(source_id, typed_id)?;
        self.root_source_typed_span(source.position, typed.position, typed_id)?;

        let source_function = self.source_tree(source_apply.function)?;
        let typed_function =
            p.typed_ast
                .try_get(typed_apply.function)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: typed_apply.function,
                })?;
        let (TreeKind::Select(source_select), TreeKind::Select(typed_select)) =
            (&source_function.kind, &typed_function.kind)
        else {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        };
        self.charge(1)?;
        if !visited.insert(typed_apply.function) {
            return Err(PickleError::RepeatedExpression {
                tree: typed_apply.function,
            });
        }
        self.source_typed_binding(source_apply.function, typed_apply.function)?;
        self.paired_source_typed_span(
            source_function.position,
            typed_function.position,
            typed_apply.function,
        )?;
        if source_select.name != typed_select.name
            || source_select.backquoted != typed_select.backquoted
        {
            return Err(PickleError::SourceShape {
                tree: source_apply.function,
                expected: SourceRole::Body,
            });
        }

        let source_receiver = self.source_tree(source_select.qualifier)?;
        let typed_receiver =
            p.typed_ast
                .try_get(typed_select.qualifier)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: typed_select.qualifier,
                })?;
        let (TreeKind::Ident(source_ident), TreeKind::Ident(typed_ident)) =
            (&source_receiver.kind, &typed_receiver.kind)
        else {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        };
        self.charge(1)?;
        if !visited.insert(typed_select.qualifier) {
            return Err(PickleError::RepeatedExpression {
                tree: typed_select.qualifier,
            });
        }
        self.source_typed_binding(source_select.qualifier, typed_select.qualifier)?;
        self.paired_source_typed_span(
            source_receiver.position,
            typed_receiver.position,
            typed_select.qualifier,
        )?;
        if source_ident.name != typed_ident.name
            || source_ident.backquoted != typed_ident.backquoted
        {
            return Err(PickleError::SourceShape {
                tree: source_select.qualifier,
                expected: SourceRole::Body,
            });
        }
        let Type::TermRef {
            prefix: receiver_prefix,
            target: TermRefTarget::Symbol(receiver_symbol),
        } = self.ty(typed_receiver.ty)?
        else {
            return Err(PickleError::BodyType {
                tree: typed_select.qualifier,
                ty: typed_receiver.ty,
            });
        };
        let receiver_is_parameter = method
            .parameter_clauses
            .iter()
            .flatten()
            .any(|parameter| parameter == receiver_symbol);
        if receiver_is_parameter {
            if *receiver_prefix != p.definitions.no_prefix
                || !matches!(self.ty(*receiver_prefix)?, Type::NoPrefix)
                || self.symbol(*receiver_symbol)?.owner != Some(method.symbol)
                || self.symbol(*receiver_symbol)?.name != source_ident.name
            {
                return Err(PickleError::Reference {
                    symbol: *receiver_symbol,
                    reason: crate::ReferenceError::Owner,
                });
            }
        } else if self.reference_fields.contains(receiver_symbol) {
            if self.unqualified_name_is_shadowed(source_ident.name, method) {
                return Err(PickleError::Reference {
                    symbol: *receiver_symbol,
                    reason: crate::ReferenceError::NotRegistered,
                });
            }
            let receiver_type = self.complete(*receiver_symbol)?;
            self.same_class_field_target(
                *receiver_symbol,
                source_ident.name,
                *receiver_prefix,
                typed_select.qualifier,
                receiver_type,
            )?;
        } else {
            return Err(PickleError::Reference {
                symbol: *receiver_symbol,
                reason: crate::ReferenceError::Owner,
            });
        }
        let receiver_type = self.complete(*receiver_symbol)?;
        if !self.is_planned_sibling_class_type(receiver_type, None) {
            return Err(PickleError::UnsupportedTypeReference { ty: receiver_type });
        }
        let Type::TypeRef {
            target: TypeRefTarget::Symbol(receiver_class),
            ..
        } = self.ty(receiver_type)?
        else {
            return Err(PickleError::UnsupportedTypeReference { ty: receiver_type });
        };
        let Type::TermRef {
            prefix: selected_prefix,
            target: TermRefTarget::Symbol(target),
        } = self.ty(typed_function.ty)?
        else {
            return Err(PickleError::BodyType {
                tree: typed_apply.function,
                ty: typed_function.ty,
            });
        };
        if *receiver_class == p.class
            || *selected_prefix != typed_receiver.ty
            || !p
                .typed_bindings
                .planned_class_symbols()
                .contains(receiver_class)
            || !p.typed_bindings.planned_method_symbols().contains(target)
        {
            return Err(PickleError::Reference {
                symbol: *target,
                reason: crate::ReferenceError::NotRegistered,
            });
        }
        let Some(SourceDefinition::Canonical {
            source: class_source,
            tree: class_tree,
        }) = p.source_index.definition_of(*receiver_class)
        else {
            return Err(PickleError::SourceDefinition {
                symbol: *receiver_class,
            });
        };
        if class_source != p.source.id
            || p.source_index.symbol_at(class_source, class_tree) != Some(*receiver_class)
        {
            return Err(PickleError::SourceDefinition {
                symbol: *receiver_class,
            });
        }
        self.kind(*target, SymbolKind::Method)?;
        let target_record = self.symbol(*target)?;
        if target_record.owner != Some(*receiver_class)
            || target_record.origin != SymbolOrigin::Source(p.source.id)
            || target_record.visibility != Visibility::Public
            || !target_record.flags.is_empty()
            || !target_record.annotations.is_empty()
        {
            return Err(PickleError::Reference {
                symbol: *target,
                reason: crate::ReferenceError::Metadata,
            });
        }
        let Some(SourceDefinition::Canonical { source, tree }) =
            p.source_index.definition_of(*target)
        else {
            return Err(PickleError::SourceDefinition { symbol: *target });
        };
        if source != p.source.id || p.source_index.symbol_at(source, tree) != Some(*target) {
            return Err(PickleError::SourceDefinition { symbol: *target });
        }
        let target_tree = self.source_tree(tree)?;
        let TreeKind::DefDef(target_definition) = &target_tree.kind else {
            return Err(PickleError::SourceShape {
                tree,
                expected: SourceRole::Method,
            });
        };
        if target_definition.name.as_name() != &source_select.name
            || !target_definition.type_params.is_empty()
            || target_definition.value_param_clauses.len() != 1
            || !target_definition.value_param_clauses[0].is_empty()
            || target_definition.rhs.is_none()
        {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        }
        self.metadata(&target_definition.metadata, *target)?;
        let owner_type = self.complete(*receiver_class)?;
        let Type::ClassInfo(owner_info) = self.ty(owner_type)? else {
            return Err(PickleError::MalformedType {
                ty: owner_type,
                role: TypeRole::ClassInfo,
            });
        };
        if p.source_index.scope_of(*receiver_class) != Some(owner_info.declarations)
            || p.store
                .scopes
                .get(owner_info.declarations)
                .lookup_all(&target_record.name)
                != [*target]
        {
            return Err(PickleError::Reference {
                symbol: *target,
                reason: crate::ReferenceError::Ambiguous,
            });
        }
        let signature_type = self.complete(*target)?;
        let Type::Method(signature) = self.ty(signature_type)? else {
            return Err(PickleError::MalformedType {
                ty: signature_type,
                role: TypeRole::Parameter,
            });
        };
        validate_cross_class_target_signature(signature_type, signature, p.definitions.int)
    }

    fn same_class_method_application_target(
        &mut self,
        target: SymbolId,
        source_name: dotty_core::Name,
        prefix: TypeId,
        source_tree: TreeId<Untyped>,
    ) -> Result<usize, PickleError> {
        let p = self.plan;
        let target_method = p
            .methods
            .iter()
            .find(|candidate| candidate.symbol == target)
            .ok_or(PickleError::Reference {
                symbol: target,
                reason: crate::ReferenceError::NotRegistered,
            })?;
        self.kind(target, SymbolKind::Method)?;
        self.owner(target, Some(p.class))?;
        let target_tree = self.source_tree(target_method.source_tree)?;
        let TreeKind::DefDef(definition) = &target_tree.kind else {
            return Err(PickleError::SourceShape {
                tree: target_method.source_tree,
                expected: SourceRole::Method,
            });
        };
        if definition.name.as_name() != &source_name
            || target_method.parameter_clauses.len() != 1
            || target_method.parameter_clauses[0].is_empty()
            || definition.value_param_clauses.len() != 1
        {
            return Err(PickleError::NameMismatch {
                symbol: target,
                tree: source_tree,
            });
        }
        self.source_symbol(
            target,
            target_method.source_tree,
            SymbolKind::Method,
            *definition.name.as_name(),
        )?;
        self.metadata(&definition.metadata, target)?;
        let ty = self.complete(target)?;
        let Type::Method(signature) = self.ty(ty)? else {
            return Err(PickleError::MalformedType {
                ty,
                role: TypeRole::Parameter,
            });
        };
        if signature.kind != MethodKind::Plain
            || signature.params.len() != target_method.parameter_clauses[0].len()
            || signature.result != p.definitions.int
        {
            return Err(PickleError::MalformedType {
                ty,
                role: TypeRole::Parameter,
            });
        }
        for (formal, &parameter) in signature
            .params
            .iter()
            .zip(&target_method.parameter_clauses[0])
        {
            if formal.name.as_name() != &self.symbol(parameter)?.name
                || formal.ty != p.definitions.int
                || formal.erased
                || formal.varargs
                || self.complete(parameter)? != p.definitions.int
            {
                return Err(PickleError::MalformedType {
                    ty,
                    role: TypeRole::Parameter,
                });
            }
        }
        if !matches!(self.ty(prefix)?, Type::ThisType { class } if *class == p.class) {
            return Err(PickleError::Reference {
                symbol: target,
                reason: crate::ReferenceError::Owner,
            });
        }
        Ok(signature.params.len())
    }

    fn validate_nested_application(
        &mut self,
        source_id: TreeId<Untyped>,
        typed_id: TreeId<Typed>,
        current_method: &MethodPicklePlan,
        depth: usize,
        visited: &mut HashSet<TreeId<Typed>>,
    ) -> Result<(), PickleError> {
        let expression_depth_limit = self.limits.expression_depth_limit();
        if depth > expression_depth_limit {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::ExpressionDepth,
                limit: expression_depth_limit,
            });
        }
        self.charge(1)?;
        if !visited.insert(typed_id) {
            return Err(PickleError::RepeatedExpression { tree: typed_id });
        }
        let p = self.plan;
        let source_tree = self.source_tree(source_id)?;
        let typed_tree = p
            .typed_ast
            .try_get(typed_id)
            .ok_or(PickleError::InvalidTypedTree { tree: typed_id })?;
        self.source_typed_binding(source_id, typed_id)?;
        self.paired_source_typed_span(source_tree.position, typed_tree.position, typed_id)?;
        let (TreeKind::Apply(source_apply), TreeKind::Apply(typed_apply)) =
            (&source_tree.kind, &typed_tree.kind)
        else {
            return Err(PickleError::SourceShape {
                tree: source_id,
                expected: SourceRole::Body,
            });
        };
        if source_apply.kind != ApplyKind::Regular
            || typed_apply.kind != ApplyKind::Regular
            || source_apply.args.is_empty()
            || source_apply.args.len() != typed_apply.args.len()
        {
            return Err(self.unsupported(
                UnsupportedFeature::BodyExpression,
                Some(current_method.symbol),
            ));
        }
        if !visited.insert(typed_apply.function) {
            return Err(PickleError::RepeatedExpression {
                tree: typed_apply.function,
            });
        }
        let source_function = self.source_tree(source_apply.function)?;
        let typed_function =
            p.typed_ast
                .try_get(typed_apply.function)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: typed_apply.function,
                })?;
        self.paired_source_typed_span(
            source_function.position,
            typed_function.position,
            typed_apply.function,
        )?;
        let (name, _explicit_this) = match (&source_function.kind, &typed_function.kind) {
            (TreeKind::Ident(source), TreeKind::Ident(typed))
                if source.name == typed.name && source.backquoted == typed.backquoted =>
            {
                (source.name, false)
            }
            (TreeKind::Select(source), TreeKind::Select(typed))
                if source.name == typed.name && source.backquoted == typed.backquoted =>
            {
                if !visited.insert(typed.qualifier) {
                    return Err(PickleError::RepeatedExpression {
                        tree: typed.qualifier,
                    });
                }
                let source_qualifier = self.source_tree(source.qualifier)?;
                let typed_qualifier =
                    p.typed_ast
                        .try_get(typed.qualifier)
                        .ok_or(PickleError::InvalidTypedTree {
                            tree: typed.qualifier,
                        })?;
                if !matches!(source_qualifier.kind, TreeKind::This(this) if this.qual.is_none())
                    || !matches!(typed_qualifier.kind, TreeKind::This(this) if this.qual.is_none())
                {
                    return Err(self.unsupported(
                        UnsupportedFeature::BodyExpression,
                        Some(current_method.symbol),
                    ));
                }
                self.source_typed_binding(source.qualifier, typed.qualifier)?;
                self.paired_source_typed_span(
                    source_qualifier.position,
                    typed_qualifier.position,
                    typed.qualifier,
                )?;
                if !matches!(self.ty(typed_qualifier.ty)?, Type::ThisType { class } if *class == p.class)
                {
                    return Err(PickleError::Reference {
                        symbol: current_method.symbol,
                        reason: crate::ReferenceError::Owner,
                    });
                }
                (source.name, true)
            }
            _ => {
                return Err(PickleError::SourceShape {
                    tree: source_apply.function,
                    expected: SourceRole::Body,
                });
            }
        };
        self.source_typed_binding(source_apply.function, typed_apply.function)?;
        let Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(target),
        } = self.ty(typed_function.ty)?
        else {
            return Err(PickleError::BodyType {
                tree: typed_apply.function,
                ty: typed_function.ty,
            });
        };
        let count = self.same_class_method_application_target(
            *target,
            name,
            *prefix,
            source_apply.function,
        )?;
        if count != source_apply.args.len() {
            return Err(PickleError::BodyType {
                tree: typed_apply.function,
                ty: typed_function.ty,
            });
        }
        for (&source_arg, &typed_arg) in source_apply.args.iter().zip(&typed_apply.args) {
            let source_argument = self.source_tree(source_arg)?;
            let typed_argument = p
                .typed_ast
                .try_get(typed_arg)
                .ok_or(PickleError::InvalidTypedTree { tree: typed_arg })?;
            if matches!(
                source_argument.kind,
                TreeKind::PhaseSpecific(UntypedNode::Parens(_))
            ) {
                self.validate_int_expression(
                    source_arg,
                    typed_arg,
                    current_method,
                    depth + 1,
                    visited,
                )?;
                continue;
            }
            if matches!(
                source_argument.kind,
                TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
            ) && matches!(typed_argument.kind, TreeKind::Apply(_))
            {
                let result = self.validate_primitive_application(
                    source_arg,
                    typed_arg,
                    current_method,
                    depth + 1,
                    visited,
                )?;
                if result != p.definitions.int {
                    return Err(PickleError::BodyType {
                        tree: typed_arg,
                        ty: typed_argument.ty,
                    });
                }
                continue;
            }
            if matches!(source_argument.kind, TreeKind::Apply(_))
                || matches!(typed_argument.kind, TreeKind::Apply(_))
            {
                self.validate_nested_application(
                    source_arg,
                    typed_arg,
                    current_method,
                    depth + 1,
                    visited,
                )?;
                continue;
            }
            self.charge(1)?;
            if !visited.insert(typed_arg) {
                return Err(PickleError::RepeatedExpression { tree: typed_arg });
            }
            self.source_typed_binding(source_arg, typed_arg)?;
            self.paired_source_typed_span(
                source_argument.position,
                typed_argument.position,
                typed_arg,
            )?;
            match (&source_argument.kind, &typed_argument.kind) {
                (TreeKind::Ident(source), TreeKind::Ident(typed))
                    if source.name == typed.name && source.backquoted == typed.backquoted =>
                {
                    let Type::TermRef {
                        prefix,
                        target: TermRefTarget::Symbol(symbol),
                    } = self.ty(typed_argument.ty)?
                    else {
                        return Err(PickleError::BodyType {
                            tree: typed_arg,
                            ty: typed_argument.ty,
                        });
                    };
                    if !matches!(self.ty(*prefix)?, Type::NoPrefix) {
                        return Err(PickleError::Reference {
                            symbol: *symbol,
                            reason: crate::ReferenceError::Owner,
                        });
                    }
                    self.kind(*symbol, SymbolKind::Parameter)?;
                    self.owner(*symbol, Some(current_method.symbol))?;
                    if !current_method
                        .parameter_clauses
                        .iter()
                        .flatten()
                        .any(|parameter| parameter == symbol)
                        || self.symbol(*symbol)?.name != source.name
                        || self.complete(*symbol)? != p.definitions.int
                    {
                        return Err(PickleError::Reference {
                            symbol: *symbol,
                            reason: crate::ReferenceError::NotRegistered,
                        });
                    }
                }
                (TreeKind::Literal(source), TreeKind::Literal(typed))
                    if source.value == typed.value
                        && matches!((&typed.value, self.ty(typed_argument.ty)?), (Constant::Int(value), Type::Constant(Constant::Int(ty_value))) if value == ty_value) =>
                    {}
                (
                    TreeKind::PhaseSpecific(UntypedNode::Number(number)),
                    TreeKind::Literal(typed),
                ) => {
                    let digits = self.name_text(number.text)?.replace('_', "");
                    let value = match number.kind {
                        NumberKind::Whole(10) => digits.parse::<i32>().ok(),
                        NumberKind::Whole(radix @ (2 | 16)) => {
                            let digits = if radix == 16 {
                                digits
                                    .strip_prefix("0x")
                                    .or_else(|| digits.strip_prefix("0X"))
                            } else {
                                digits
                                    .strip_prefix("0b")
                                    .or_else(|| digits.strip_prefix("0B"))
                            };
                            digits
                                .and_then(|digits| u32::from_str_radix(digits, radix).ok())
                                .map(|bits| bits as i32)
                        }
                        _ => None,
                    };
                    if value
                        != match typed.value {
                            Constant::Int(value) => Some(value),
                            _ => None,
                        }
                        || !matches!((value, self.ty(typed_argument.ty)?), (Some(value), Type::Constant(Constant::Int(type_value))) if value == *type_value)
                    {
                        return Err(PickleError::BodyType {
                            tree: typed_arg,
                            ty: typed_argument.ty,
                        });
                    }
                }
                _ => {
                    return Err(PickleError::SourceShape {
                        tree: source_arg,
                        expected: SourceRole::Body,
                    });
                }
            }
        }
        if typed_tree.ty != p.definitions.int
            && !matches!(self.ty(typed_tree.ty)?, Type::Constant(Constant::Int(_)))
        {
            return Err(PickleError::BodyType {
                tree: typed_id,
                ty: typed_tree.ty,
            });
        }
        Ok(())
    }

    fn validate_if_condition(
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
        let source = self.source_tree(source_id)?;
        let typed = self
            .plan
            .typed_ast
            .try_get(typed_id)
            .ok_or(PickleError::InvalidTypedTree { tree: typed_id })?;
        if let TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) = &source.kind {
            self.source_typed_binding(source_id, typed_id)?;
            self.span(source.position)?;
            self.charge(1)?;
            return self.validate_if_condition(parens.inner, typed_id, method, depth + 1, visited);
        }
        if matches!(
            source.kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
        ) && matches!(typed.kind, TreeKind::Apply(_))
        {
            let result =
                self.validate_primitive_application(source_id, typed_id, method, depth, visited)?;
            if result != self.plan.definitions.boolean {
                return Err(PickleError::BodyType {
                    tree: typed_id,
                    ty: typed.ty,
                });
            }
            return Ok(());
        }
        if matches!(
            source.kind,
            TreeKind::PhaseSpecific(UntypedNode::PrefixOp(_))
        ) && matches!(typed.kind, TreeKind::Select(_))
        {
            return self.validate_boolean_prefix(source_id, typed_id, method, depth, visited);
        }
        self.charge(1)?;
        if !visited.insert(typed_id) {
            return Err(PickleError::RepeatedExpression { tree: typed_id });
        }
        self.source_typed_binding(source_id, typed_id)?;
        self.paired_source_typed_span(source.position, typed.position, typed_id)?;
        match (&source.kind, &typed.kind) {
            (TreeKind::Literal(source), TreeKind::Literal(literal))
                if source.value == literal.value =>
            {
                match (&literal.value, self.ty(typed.ty)?) {
                    (Constant::Boolean(expected), Type::Constant(Constant::Boolean(actual)))
                        if expected == actual =>
                    {
                        Ok(())
                    }
                    _ => Err(PickleError::BodyType {
                        tree: typed_id,
                        ty: typed.ty,
                    }),
                }
            }
            (TreeKind::Ident(source), TreeKind::Ident(ident))
                if source.name == ident.name && source.backquoted == ident.backquoted =>
            {
                let Type::TermRef {
                    prefix,
                    target: TermRefTarget::Symbol(symbol),
                } = self.ty(typed.ty)?
                else {
                    return Err(PickleError::BodyType {
                        tree: typed_id,
                        ty: typed.ty,
                    });
                };
                if self.class_members.is_some()
                    && self.plan.store.symbols.contains(*symbol)
                    && self.plan.store.symbols.get(*symbol).kind == SymbolKind::Field
                {
                    if self.unqualified_name_is_shadowed(source.name, method) {
                        return Err(PickleError::Reference {
                            symbol: *symbol,
                            reason: crate::ReferenceError::NotRegistered,
                        });
                    }
                    self.same_class_field_target(
                        *symbol,
                        source.name,
                        *prefix,
                        typed_id,
                        self.plan.definitions.boolean,
                    )?;
                    return Ok(());
                }
                if !matches!(self.ty(*prefix)?, Type::NoPrefix) {
                    return Err(PickleError::Reference {
                        symbol: *symbol,
                        reason: crate::ReferenceError::Owner,
                    });
                }
                self.kind(*symbol, SymbolKind::Parameter)?;
                self.owner(*symbol, Some(method.symbol))?;
                if !method
                    .parameter_clauses
                    .iter()
                    .flatten()
                    .any(|parameter| parameter == symbol)
                    || self.symbol(*symbol)?.name != source.name
                    || self.complete(*symbol)? != self.plan.definitions.boolean
                {
                    return Err(PickleError::BodyType {
                        tree: typed_id,
                        ty: typed.ty,
                    });
                }
                Ok(())
            }
            (TreeKind::Select(source), TreeKind::Select(selection))
                if source.name == selection.name && source.backquoted == selection.backquoted =>
            {
                let source_qualifier = self.source_tree(source.qualifier)?;
                let typed_qualifier = self.plan.typed_ast.try_get(selection.qualifier).ok_or(
                    PickleError::InvalidTypedTree {
                        tree: selection.qualifier,
                    },
                )?;
                if !matches!(source_qualifier.kind, TreeKind::This(this) if this.qual.is_none())
                    || !matches!(typed_qualifier.kind, TreeKind::This(this) if this.qual.is_none())
                {
                    return Err(
                        self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                    );
                }
                self.source_typed_binding(source.qualifier, selection.qualifier)?;
                self.paired_source_typed_span(
                    source_qualifier.position,
                    typed_qualifier.position,
                    selection.qualifier,
                )?;
                let Type::TermRef {
                    prefix,
                    target: TermRefTarget::Symbol(symbol),
                } = self.ty(typed.ty)?
                else {
                    return Err(PickleError::BodyType {
                        tree: typed_id,
                        ty: typed.ty,
                    });
                };
                self.same_class_field_target(
                    *symbol,
                    source.name,
                    *prefix,
                    typed_id,
                    self.plan.definitions.boolean,
                )
            }
            _ => Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))),
        }
    }

    fn validate_if_branch(
        &mut self,
        source_id: TreeId<Untyped>,
        typed_id: TreeId<Typed>,
        method: &MethodPicklePlan,
        depth: usize,
        visited: &mut HashSet<TreeId<Typed>>,
    ) -> Result<TypeId, PickleError> {
        let depth_limit = self.limits.expression_depth_limit();
        if depth > depth_limit {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::ExpressionDepth,
                limit: depth_limit,
            });
        }
        let source = self.source_tree(source_id)?;
        let typed = self
            .plan
            .typed_ast
            .try_get(typed_id)
            .ok_or(PickleError::InvalidTypedTree { tree: typed_id })?;
        if let TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) = &source.kind {
            self.source_typed_binding(source_id, typed_id)?;
            self.span(source.position)?;
            self.charge(1)?;
            return self.validate_if_branch(parens.inner, typed_id, method, depth + 1, visited);
        }
        if matches!(source.kind, TreeKind::If(_)) || matches!(typed.kind, TreeKind::If(_)) {
            return self.validate_if_expression(source_id, typed_id, method, depth + 1, visited);
        }
        if matches!(typed.kind, TreeKind::Apply(_))
            && (matches!(source.kind, TreeKind::Apply(_))
                || matches!(
                    source.kind,
                    TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
                ))
        {
            if matches!(
                source.kind,
                TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
            ) {
                self.validate_primitive_application(source_id, typed_id, method, depth, visited)?;
            } else {
                self.validate_nested_application(source_id, typed_id, method, depth, visited)?;
            }
            return Ok(typed.ty);
        }
        if matches!(source.kind, TreeKind::Block(_)) && matches!(typed.kind, TreeKind::Block(_)) {
            self.validate_method_expression_inner(
                Some(source_id),
                Some(typed_id),
                method,
                self.plan.definitions.int,
                depth + 1,
                (Some(visited), false),
            )?;
            return Ok(typed.ty);
        }
        self.validate_method_expression_inner(
            Some(source_id),
            Some(typed_id),
            method,
            self.plan.definitions.int,
            depth,
            (Some(visited), false),
        )?;
        Ok(typed.ty)
    }

    fn validate_if_expression(
        &mut self,
        source_id: TreeId<Untyped>,
        typed_id: TreeId<Typed>,
        method: &MethodPicklePlan,
        depth: usize,
        visited: &mut HashSet<TreeId<Typed>>,
    ) -> Result<TypeId, PickleError> {
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
        let (TreeKind::If(source_if), TreeKind::If(typed_if)) = (&source.kind, &typed.kind) else {
            return Err(PickleError::SourceShape {
                tree: source_id,
                expected: SourceRole::Body,
            });
        };
        let source_if = *source_if;
        let typed_if = *typed_if;
        self.validate_if_condition(source_if.cond, typed_if.cond, method, depth, visited)?;
        let then_type = self.validate_if_branch(
            source_if.then_branch,
            typed_if.then_branch,
            method,
            depth,
            visited,
        )?;
        let else_type = self.validate_if_branch(
            source_if.else_branch,
            typed_if.else_branch,
            method,
            depth,
            visited,
        )?;

        let branch_join = match self.ty(typed.ty)? {
            Type::Constant(Constant::Int(value)) => {
                let then_join =
                    self.int_branch_constant(typed_if.then_branch, then_type, method)?;
                let else_join =
                    self.int_branch_constant(typed_if.else_branch, else_type, method)?;
                if then_join != Some(*value) || else_join != Some(*value) {
                    return Err(PickleError::BodyType {
                        tree: typed_id,
                        ty: typed.ty,
                    });
                }
                typed.ty
            }
            _ if typed.ty == self.plan.definitions.int => typed.ty,
            _ => {
                return Err(PickleError::BodyType {
                    tree: typed_id,
                    ty: typed.ty,
                });
            }
        };
        if branch_join == self.plan.definitions.int {
            self.int_branch_constant(typed_if.then_branch, then_type, method)?;
            self.int_branch_constant(typed_if.else_branch, else_type, method)?;
        }
        Ok(branch_join)
    }

    /// Returns `Some(value)` for an Int singleton and `None` for a widened Int
    /// (including a previously validated Int formal or parameterless method reference).
    fn int_branch_constant(
        &mut self,
        tree: TreeId<Typed>,
        ty: TypeId,
        method: &MethodPicklePlan,
    ) -> Result<Option<i32>, PickleError> {
        if ty == self.plan.definitions.int {
            return Ok(None);
        }
        match self.ty(ty)? {
            Type::Constant(Constant::Int(value)) => Ok(Some(*value)),
            Type::TermRef {
                prefix,
                target: TermRefTarget::Symbol(symbol),
            } if self.plan.store.symbols.contains(*symbol)
                && self.plan.store.symbols.get(*symbol).kind == SymbolKind::Field =>
            {
                // The branch has already been recursively checked. Its source
                // may be a block or a parenthesized expression, so use the
                // validated field target rather than reinterpreting the outer
                // source shape during the Int join.
                let source_name = self.symbol(*symbol)?.name;
                self.same_class_field_target(
                    *symbol,
                    source_name,
                    *prefix,
                    tree,
                    self.plan.definitions.int,
                )?;
                Ok(None)
            }
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } if self.plan.store.symbols.contains(*symbol)
                && (matches!(
                    self.plan.store.symbols.get(*symbol).kind,
                    SymbolKind::Parameter | SymbolKind::Method
                ) || (self.plan.store.symbols.get(*symbol).kind == SymbolKind::Local
                    && self.registered_local_symbols.contains(symbol)
                    && self.visible_locals.contains(symbol)
                    && self.plan.store.symbols.get(*symbol).owner == Some(method.symbol)))
                && self.complete(*symbol)? == self.plan.definitions.int =>
            {
                Ok(None)
            }
            _ => Err(PickleError::BodyType { tree, ty }),
        }
    }

    pub(super) fn validate_method_expression(
        &mut self,
        rhs: Option<TreeId<Untyped>>,
        body: Option<TreeId<Typed>>,
        method: &MethodPicklePlan,
        result_type: TypeId,
    ) -> Result<(), PickleError> {
        self.validate_method_expression_inner(rhs, body, method, result_type, 0, (None, false))
    }

    fn validate_method_expression_inner(
        &mut self,
        rhs: Option<TreeId<Untyped>>,
        body: Option<TreeId<Typed>>,
        method: &MethodPicklePlan,
        result_type: TypeId,
        depth: usize,
        traversal: (Option<&mut HashSet<TreeId<Typed>>>, bool),
    ) -> Result<(), PickleError> {
        // The identity requirement is enabled only for a reference-valued block tail.
        let (mut visited, require_constructor_result_identity) = traversal;
        let depth_limit = self.limits.expression_depth_limit();
        if depth > depth_limit {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::ExpressionDepth,
                limit: depth_limit,
            });
        }
        let Some(rhs) = rhs else {
            return Err(self.unsupported(UnsupportedFeature::AbstractMethod, Some(method.symbol)));
        };
        let p = self.plan;
        let source_rhs = self.source_tree(rhs)?;
        let body = body.ok_or(PickleError::MissingBody {
            method: method.symbol,
        })?;
        let node = p
            .typed_ast
            .try_get(body)
            .ok_or(PickleError::InvalidTypedTree { tree: body })?;
        if self.is_constructor_application_shape(rhs, body)? {
            let mut local_visited = HashSet::new();
            let seen = visited.as_deref_mut().unwrap_or(&mut local_visited);
            return self.validate_cross_class_constructor_application(
                rhs,
                body,
                method,
                result_type,
                depth,
                (seen, require_constructor_result_identity),
            );
        }
        if matches!(
            source_rhs.kind,
            TreeKind::PhaseSpecific(UntypedNode::Parens(_))
        ) {
            let mut local_visited = HashSet::new();
            let seen = visited.as_deref_mut().unwrap_or(&mut local_visited);
            return if result_type == p.definitions.int {
                self.validate_int_expression(rhs, body, method, depth, seen)
            } else if result_type == p.definitions.boolean {
                self.validate_if_condition(rhs, body, method, depth, seen)
            } else if result_type == p.definitions.unit {
                self.validate_unit_expression(rhs, body, method, depth, seen)
            } else {
                Err(PickleError::BodyType {
                    tree: body,
                    ty: node.ty,
                })
            };
        }
        if matches!(
            source_rhs.kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
        ) && matches!(node.kind, TreeKind::Apply(_))
        {
            let mut local_visited = HashSet::new();
            let seen = visited.as_deref_mut().unwrap_or(&mut local_visited);
            let operator_result =
                self.validate_primitive_application(rhs, body, method, depth, seen)?;
            if result_type != operator_result {
                return Err(PickleError::BodyType {
                    tree: body,
                    ty: node.ty,
                });
            }
            return Ok(());
        }
        if result_type == p.definitions.int
            && matches!(&source_rhs.kind, TreeKind::Apply(application) if application.args.is_empty())
            && matches!(&node.kind, TreeKind::Apply(application) if application.args.is_empty())
        {
            let mut local_visited = HashSet::new();
            let seen = visited.as_deref_mut().unwrap_or(&mut local_visited);
            return self.validate_cross_class_noarg_call(rhs, body, method, depth, seen);
        }
        if result_type != p.definitions.int
            && self.is_planned_sibling_class_type(result_type, None)
            && matches!(source_rhs.kind, TreeKind::Block(_))
            && matches!(node.kind, TreeKind::Block(_))
        {
            let mut local_visited = HashSet::new();
            let seen = visited.as_deref_mut().unwrap_or(&mut local_visited);
            return self.validate_block_expression(rhs, body, method, result_type, depth + 1, seen);
        }
        if result_type == p.definitions.boolean
            && matches!(
                source_rhs.kind,
                TreeKind::PhaseSpecific(UntypedNode::PrefixOp(_))
            )
            && matches!(node.kind, TreeKind::Select(_))
        {
            let mut local_visited = HashSet::new();
            let seen = visited.as_deref_mut().unwrap_or(&mut local_visited);
            return self.validate_boolean_prefix(rhs, body, method, depth, seen);
        }
        if result_type == p.definitions.unit
            && matches!(source_rhs.kind, TreeKind::Assign(_))
            && matches!(node.kind, TreeKind::Assign(_))
        {
            let mut local_visited = HashSet::new();
            let seen = visited.as_deref_mut().unwrap_or(&mut local_visited);
            return self.validate_unit_expression(rhs, body, method, depth, seen);
        }
        if result_type == p.definitions.unit
            && matches!(source_rhs.kind, TreeKind::Block(_))
            && matches!(node.kind, TreeKind::Block(_))
        {
            let mut local_visited = HashSet::new();
            let seen = visited.as_deref_mut().unwrap_or(&mut local_visited);
            return self.validate_unit_expression(rhs, body, method, depth, seen);
        }
        self.source_typed_binding(rhs, body)?;
        self.root_source_typed_span(source_rhs.position, node.position, body)?;
        let body_type = self.ty(node.ty)?;
        let expected = match (&source_rhs.kind, &node.kind) {
            (TreeKind::Ident(source_ident), TreeKind::Ident(typed_ident)) => {
                if let Some(seen) = visited.as_deref_mut() {
                    self.charge(1)?;
                    if !seen.insert(body) {
                        return Err(PickleError::RepeatedExpression { tree: body });
                    }
                }
                if source_ident.name != typed_ident.name
                    || source_ident.backquoted != typed_ident.backquoted
                {
                    return Err(PickleError::SourceShape {
                        tree: rhs,
                        expected: SourceRole::Body,
                    });
                }
                let Type::TermRef {
                    prefix,
                    target: TermRefTarget::Symbol(target),
                } = body_type
                else {
                    return Err(PickleError::BodyType {
                        tree: body,
                        ty: node.ty,
                    });
                };
                if p.methods
                    .iter()
                    .any(|candidate| candidate.symbol == *target)
                {
                    if result_type != p.definitions.int {
                        return Err(PickleError::BodyType {
                            tree: body,
                            ty: node.ty,
                        });
                    }
                    self.same_class_method_target(*target, source_ident.name, *prefix, rhs)?;
                    None
                } else if self.class_members.is_some()
                    && p.store.symbols.contains(*target)
                    && p.store.symbols.get(*target).kind == SymbolKind::Field
                {
                    if self.unqualified_name_is_shadowed(source_ident.name, method) {
                        return Err(PickleError::Reference {
                            symbol: *target,
                            reason: crate::ReferenceError::NotRegistered,
                        });
                    }
                    self.same_class_field_target(
                        *target,
                        source_ident.name,
                        *prefix,
                        body,
                        result_type,
                    )?;
                    None
                } else if self.registered_local_symbols.contains(target) {
                    self.kind(*target, SymbolKind::Local)?;
                    self.owner(*target, Some(method.symbol))?;
                    let local_type = self.complete(*target)?;
                    let type_matches = (result_type == p.definitions.int
                        && local_type == p.definitions.int)
                        || self.same_planned_sibling_class_type(local_type, result_type);
                    if !type_matches
                        || self.symbol(*target)?.name != source_ident.name
                        || !self.visible_locals.contains(target)
                    {
                        return Err(PickleError::Reference {
                            symbol: *target,
                            reason: crate::ReferenceError::NotRegistered,
                        });
                    }
                    if *prefix != p.definitions.no_prefix
                        || !matches!(self.ty(*prefix)?, Type::NoPrefix)
                    {
                        return Err(PickleError::Reference {
                            symbol: *target,
                            reason: crate::ReferenceError::Owner,
                        });
                    }
                    None
                } else {
                    let parameter = target;
                    self.kind(*parameter, SymbolKind::Parameter)?;
                    self.owner(*parameter, Some(method.symbol))?;
                    if !method
                        .parameter_clauses
                        .iter()
                        .flatten()
                        .any(|candidate| candidate == parameter)
                    {
                        return Err(PickleError::Reference {
                            symbol: *parameter,
                            reason: crate::ReferenceError::NotRegistered,
                        });
                    }
                    if self.symbol(*parameter)?.name != source_ident.name {
                        return Err(PickleError::NameMismatch {
                            symbol: *parameter,
                            tree: rhs,
                        });
                    }
                    if *prefix != p.definitions.no_prefix
                        || !matches!(self.ty(*prefix)?, Type::NoPrefix)
                    {
                        return Err(PickleError::Reference {
                            symbol: *parameter,
                            reason: crate::ReferenceError::Owner,
                        });
                    }
                    let parameter_type = self.complete(*parameter)?;
                    if parameter_type != result_type
                        && !self.same_planned_sibling_class_type(parameter_type, result_type)
                    {
                        return Err(PickleError::UnsupportedTypeReference { ty: node.ty });
                    }
                    None
                }
            }
            (TreeKind::Select(source_select), TreeKind::Select(typed_select)) => {
                if let Some(seen) = visited.as_deref_mut() {
                    self.charge(2)?;
                    if !seen.insert(body) {
                        return Err(PickleError::RepeatedExpression { tree: body });
                    }
                    if !seen.insert(typed_select.qualifier) {
                        return Err(PickleError::RepeatedExpression {
                            tree: typed_select.qualifier,
                        });
                    }
                }
                if source_select.name != typed_select.name
                    || source_select.backquoted != typed_select.backquoted
                {
                    return Err(PickleError::SourceShape {
                        tree: rhs,
                        expected: SourceRole::Body,
                    });
                }
                let source_qualifier = self.source_tree(source_select.qualifier)?;
                let typed_qualifier = p.typed_ast.try_get(typed_select.qualifier).ok_or(
                    PickleError::InvalidTypedTree {
                        tree: typed_select.qualifier,
                    },
                )?;
                if !matches!(source_qualifier.kind, TreeKind::This(this) if this.qual.is_none())
                    || !matches!(typed_qualifier.kind, TreeKind::This(this) if this.qual.is_none())
                {
                    return Err(PickleError::Unsupported {
                        feature: UnsupportedFeature::BodyExpression,
                        symbol: Some(method.symbol),
                    });
                }
                self.source_typed_binding(source_select.qualifier, typed_select.qualifier)?;
                self.paired_source_typed_span(
                    source_qualifier.position,
                    typed_qualifier.position,
                    typed_select.qualifier,
                )?;
                let Type::TermRef {
                    prefix,
                    target: TermRefTarget::Symbol(target),
                } = body_type
                else {
                    return Err(PickleError::BodyType {
                        tree: body,
                        ty: node.ty,
                    });
                };
                if !matches!(self.ty(typed_qualifier.ty)?, Type::ThisType { class } if *class == p.class)
                {
                    return Err(PickleError::Reference {
                        symbol: *target,
                        reason: crate::ReferenceError::Owner,
                    });
                }
                if p.methods
                    .iter()
                    .any(|candidate| candidate.symbol == *target)
                {
                    if result_type != p.definitions.int {
                        return Err(PickleError::BodyType {
                            tree: body,
                            ty: node.ty,
                        });
                    }
                    self.same_class_method_target(*target, source_select.name, *prefix, rhs)?;
                } else if self.class_members.is_some()
                    && p.store.symbols.contains(*target)
                    && p.store.symbols.get(*target).kind == SymbolKind::Field
                {
                    self.same_class_field_target(
                        *target,
                        source_select.name,
                        *prefix,
                        body,
                        result_type,
                    )?;
                } else {
                    return Err(PickleError::Reference {
                        symbol: *target,
                        reason: crate::ReferenceError::NotRegistered,
                    });
                }
                None
            }
            (TreeKind::Literal(source_literal), TreeKind::Literal(literal)) => {
                if let Some(seen) = visited.as_deref_mut() {
                    self.charge(1)?;
                    if !seen.insert(body) {
                        return Err(PickleError::RepeatedExpression { tree: body });
                    }
                }
                let literal_type_matches = match (&literal.value, body_type) {
                    (Constant::Int(value), Type::Constant(Constant::Int(typed))) => {
                        value == typed && result_type == p.definitions.int
                    }
                    (Constant::Boolean(value), Type::Constant(Constant::Boolean(typed))) => {
                        value == typed && result_type == p.definitions.boolean
                    }
                    (Constant::Unit, Type::Constant(Constant::Unit)) => {
                        result_type == p.definitions.unit
                    }
                    _ => false,
                };
                if !literal_type_matches {
                    return Err(PickleError::BodyType {
                        tree: body,
                        ty: node.ty,
                    });
                }
                if source_literal.value != literal.value {
                    return Err(match (&source_literal.value, &literal.value) {
                        (Constant::Int(expected), Constant::Int(actual)) => {
                            PickleError::BodySourceValue {
                                source_tree: rhs,
                                typed_tree: body,
                                expected: *expected,
                                actual: *actual,
                            }
                        }
                        _ => PickleError::BodyType {
                            tree: body,
                            ty: node.ty,
                        },
                    });
                }
                match source_literal.value {
                    Constant::Int(value) => Some(value),
                    _ => None,
                }
            }
            (TreeKind::If(_), TreeKind::If(_)) => {
                if result_type != p.definitions.int {
                    return Err(PickleError::BodyType {
                        tree: body,
                        ty: node.ty,
                    });
                }
                let mut local_visited = HashSet::new();
                let seen = visited.as_deref_mut().unwrap_or(&mut local_visited);
                self.validate_if_expression(rhs, body, method, depth, seen)?;
                None
            }
            (TreeKind::Block(_), TreeKind::Block(_)) => {
                if result_type != p.definitions.int {
                    return Err(PickleError::BodyType {
                        tree: body,
                        ty: node.ty,
                    });
                }
                let mut local_visited = HashSet::new();
                let seen = visited.as_deref_mut().unwrap_or(&mut local_visited);
                self.validate_block_expression(rhs, body, method, result_type, depth, seen)?;
                None
            }
            (TreeKind::Apply(source_apply), TreeKind::Apply(typed_apply)) => {
                if result_type != p.definitions.int {
                    return Err(PickleError::BodyType {
                        tree: body,
                        ty: node.ty,
                    });
                }
                self.charge(1)?;
                let mut local_visited = HashSet::new();
                let visited = visited.take().unwrap_or(&mut local_visited);
                if !visited.insert(body) {
                    return Err(PickleError::RepeatedExpression { tree: body });
                }
                if source_apply.kind != ApplyKind::Regular
                    || typed_apply.kind != ApplyKind::Regular
                    || source_apply.args.len() != typed_apply.args.len()
                {
                    return Err(
                        self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                    );
                }
                let source_function = self.source_tree(source_apply.function)?;
                if !visited.insert(typed_apply.function) {
                    return Err(PickleError::RepeatedExpression {
                        tree: typed_apply.function,
                    });
                }
                let typed_function = p.typed_ast.try_get(typed_apply.function).ok_or(
                    PickleError::InvalidTypedTree {
                        tree: typed_apply.function,
                    },
                )?;
                let (name, explicit_this) = match (&source_function.kind, &typed_function.kind) {
                    (TreeKind::Ident(source), TreeKind::Ident(typed))
                        if source.name == typed.name =>
                    {
                        (source.name, false)
                    }
                    (TreeKind::Select(source), TreeKind::Select(typed))
                        if source.name == typed.name =>
                    {
                        let qualifier = self.source_tree(source.qualifier)?;
                        if !visited.insert(typed.qualifier) {
                            return Err(PickleError::RepeatedExpression {
                                tree: typed.qualifier,
                            });
                        }
                        let typed_qualifier = p.typed_ast.try_get(typed.qualifier).ok_or(
                            PickleError::InvalidTypedTree {
                                tree: typed.qualifier,
                            },
                        )?;
                        if !matches!(qualifier.kind, TreeKind::This(this) if this.qual.is_none())
                            || !matches!(typed_qualifier.kind, TreeKind::This(this) if this.qual.is_none())
                        {
                            return Err(self.unsupported(
                                UnsupportedFeature::BodyExpression,
                                Some(method.symbol),
                            ));
                        }
                        self.source_typed_binding(source.qualifier, typed.qualifier)?;
                        self.paired_source_typed_span(
                            qualifier.position,
                            typed_qualifier.position,
                            typed.qualifier,
                        )?;
                        if !matches!(self.ty(typed_qualifier.ty)?, Type::ThisType { class } if *class == p.class)
                        {
                            return Err(PickleError::Reference {
                                symbol: method.symbol,
                                reason: crate::ReferenceError::Owner,
                            });
                        }
                        (source.name, true)
                    }
                    _ => {
                        return Err(PickleError::SourceShape {
                            tree: source_apply.function,
                            expected: SourceRole::Body,
                        });
                    }
                };
                self.source_typed_binding(source_apply.function, typed_apply.function)?;
                let Type::TermRef {
                    prefix,
                    target: TermRefTarget::Symbol(target),
                } = self.ty(typed_function.ty)?
                else {
                    return Err(PickleError::BodyType {
                        tree: typed_apply.function,
                        ty: typed_function.ty,
                    });
                };
                if !p
                    .methods
                    .iter()
                    .any(|candidate| candidate.symbol == *target)
                {
                    return Err(PickleError::Reference {
                        symbol: *target,
                        reason: crate::ReferenceError::NotRegistered,
                    });
                }
                let parameter_count = self.same_class_method_application_target(
                    *target,
                    name,
                    *prefix,
                    source_apply.function,
                )?;
                if source_apply.args.len() != parameter_count {
                    return Err(PickleError::BodyType {
                        tree: typed_apply.function,
                        ty: typed_function.ty,
                    });
                }
                if explicit_this
                    && !matches!(self.ty(typed_function.ty)?, Type::TermRef { prefix, .. } if matches!(self.ty(*prefix)?, Type::ThisType { class } if *class == p.class))
                {
                    return Err(PickleError::Reference {
                        symbol: *target,
                        reason: crate::ReferenceError::Owner,
                    });
                }
                for (&source_arg, &typed_arg) in source_apply.args.iter().zip(&typed_apply.args) {
                    let source_argument = self.source_tree(source_arg)?;
                    let typed_argument = p
                        .typed_ast
                        .try_get(typed_arg)
                        .ok_or(PickleError::InvalidTypedTree { tree: typed_arg })?;
                    if matches!(
                        source_argument.kind,
                        TreeKind::PhaseSpecific(UntypedNode::Parens(_))
                    ) {
                        self.validate_int_expression(
                            source_arg,
                            typed_arg,
                            method,
                            depth + 1,
                            visited,
                        )?;
                        continue;
                    }
                    if matches!(typed_argument.kind, TreeKind::Apply(_))
                        && matches!(
                            source_argument.kind,
                            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
                        )
                    {
                        let result = self.validate_primitive_application(
                            source_arg,
                            typed_arg,
                            method,
                            depth + 1,
                            visited,
                        )?;
                        if result != p.definitions.int {
                            return Err(PickleError::BodyType {
                                tree: typed_arg,
                                ty: typed_argument.ty,
                            });
                        }
                        continue;
                    }
                    if matches!(source_argument.kind, TreeKind::Apply(_))
                        && matches!(typed_argument.kind, TreeKind::Apply(_))
                    {
                        self.validate_nested_application(
                            source_arg,
                            typed_arg,
                            method,
                            depth + 1,
                            visited,
                        )?;
                        continue;
                    }
                    self.charge(1)?;
                    if !visited.insert(typed_arg) {
                        return Err(PickleError::RepeatedExpression { tree: typed_arg });
                    }
                    self.source_typed_binding(source_arg, typed_arg)?;
                    self.paired_source_typed_span(
                        source_argument.position,
                        typed_argument.position,
                        typed_arg,
                    )?;
                    match (&source_argument.kind, &typed_argument.kind) {
                        (TreeKind::Ident(source), TreeKind::Ident(typed))
                            if source.name == typed.name
                                && source.backquoted == typed.backquoted =>
                        {
                            let Type::TermRef {
                                target: TermRefTarget::Symbol(symbol),
                                ..
                            } = self.ty(typed_argument.ty)?
                            else {
                                return Err(PickleError::BodyType {
                                    tree: typed_arg,
                                    ty: typed_argument.ty,
                                });
                            };
                            if !method
                                .parameter_clauses
                                .iter()
                                .flatten()
                                .any(|parameter| parameter == symbol)
                            {
                                return Err(PickleError::Reference {
                                    symbol: *symbol,
                                    reason: crate::ReferenceError::NotRegistered,
                                });
                            }
                            if self.symbol(*symbol)?.name != source.name {
                                return Err(PickleError::NameMismatch {
                                    symbol: *symbol,
                                    tree: source_arg,
                                });
                            }
                        }
                        (TreeKind::Literal(source), TreeKind::Literal(typed))
                            if source.value == typed.value
                                && matches!(typed.value, Constant::Int(_)) => {}
                        (
                            TreeKind::PhaseSpecific(UntypedNode::Number(number)),
                            TreeKind::Literal(typed),
                        ) => {
                            let digits = self.name_text(number.text)?.replace('_', "");
                            let value = match number.kind {
                                NumberKind::Whole(10) => digits.parse::<i32>().ok(),
                                NumberKind::Whole(radix @ (2 | 16)) => {
                                    let digits = if radix == 16 {
                                        digits
                                            .strip_prefix("0x")
                                            .or_else(|| digits.strip_prefix("0X"))
                                    } else {
                                        digits
                                            .strip_prefix("0b")
                                            .or_else(|| digits.strip_prefix("0B"))
                                    };
                                    digits
                                        .and_then(|digits| u32::from_str_radix(digits, radix).ok())
                                        .map(|bits| bits as i32)
                                }
                                _ => None,
                            };
                            if value
                                != match typed.value {
                                    Constant::Int(value) => Some(value),
                                    _ => None,
                                }
                            {
                                return Err(PickleError::BodyType {
                                    tree: typed_arg,
                                    ty: typed_argument.ty,
                                });
                            }
                        }
                        _ => {
                            return Err(PickleError::SourceShape {
                                tree: source_arg,
                                expected: SourceRole::Body,
                            });
                        }
                    }
                    let argument_type = self.ty(typed_argument.ty)?;
                    let valid_argument_type = match argument_type {
                        Type::Constant(Constant::Int(_)) => true,
                        Type::TermRef {
                            prefix,
                            target: TermRefTarget::Symbol(symbol),
                        } => {
                            if !matches!(self.ty(*prefix)?, Type::NoPrefix) {
                                false
                            } else {
                                self.kind(*symbol, SymbolKind::Parameter)?;
                                self.owner(*symbol, Some(method.symbol))?;
                                self.complete(*symbol)? == p.definitions.int
                            }
                        }
                        _ => typed_argument.ty == p.definitions.int,
                    };
                    if !valid_argument_type {
                        return Err(PickleError::BodyType {
                            tree: typed_arg,
                            ty: typed_argument.ty,
                        });
                    }
                }
                if result_type != p.definitions.int
                    || (node.ty != p.definitions.int
                        && !matches!(self.ty(node.ty)?, Type::Constant(Constant::Int(_))))
                {
                    return Err(PickleError::BodyType {
                        tree: body,
                        ty: node.ty,
                    });
                }
                None
            }
            (TreeKind::PhaseSpecific(UntypedNode::Number(number)), TreeKind::Literal(literal)) => {
                if let Some(seen) = visited {
                    self.charge(1)?;
                    if !seen.insert(body) {
                        return Err(PickleError::RepeatedExpression { tree: body });
                    }
                }
                if result_type != p.definitions.int
                    || !matches!((&literal.value, body_type), (Constant::Int(value), Type::Constant(Constant::Int(ty_value))) if value == ty_value)
                {
                    return Err(PickleError::BodyType {
                        tree: body,
                        ty: node.ty,
                    });
                }
                // Expression parsing retains unsuffixed integers as raw numbers.
                // Bound the interned spelling before normalizing or decoding it.
                let digits = self.name_text(number.text)?.replace('_', "");
                let value = match number.kind {
                    NumberKind::Whole(10) => digits.parse::<i32>().ok(),
                    NumberKind::Whole(radix @ (2 | 16)) => {
                        let digits = if radix == 16 {
                            digits
                                .strip_prefix("0x")
                                .or_else(|| digits.strip_prefix("0X"))
                        } else {
                            digits
                                .strip_prefix("0b")
                                .or_else(|| digits.strip_prefix("0B"))
                        };
                        // Nondecimal Int literals retain their signed 32-bit pattern,
                        // matching the source typer (e.g. 0xffffffff is -1).
                        digits
                            .and_then(|digits| u32::from_str_radix(digits, radix).ok())
                            .map(|bits| bits as i32)
                    }
                    NumberKind::Whole(_) => None,
                    NumberKind::Decimal | NumberKind::Floating => {
                        return Err(self
                            .unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
                    }
                };
                Some(value.ok_or(PickleError::SourceShape {
                    tree: rhs,
                    expected: SourceRole::Body,
                })?)
            }
            (TreeKind::Ident(_), _) => {
                return Err(PickleError::SourceShape {
                    tree: rhs,
                    expected: SourceRole::Body,
                });
            }
            (_, _) => {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                );
            }
        };
        if let Some(expected) = expected {
            let TreeKind::Literal(literal) = &node.kind else {
                return Err(PickleError::SourceShape {
                    tree: rhs,
                    expected: SourceRole::Body,
                });
            };
            if let Constant::Int(actual) = &literal.value
                && expected != *actual
            {
                return Err(PickleError::BodySourceValue {
                    source_tree: rhs,
                    typed_tree: body,
                    expected,
                    actual: *actual,
                });
            }
        }
        Ok(())
    }
    fn is_constructor_application_shape(
        &self,
        source: TreeId<Untyped>,
        typed: TreeId<Typed>,
    ) -> Result<bool, PickleError> {
        let source_root = self.source_tree(source)?;
        let Some(typed_root) = self.plan.typed_ast.try_get(typed) else {
            return Err(PickleError::InvalidTypedTree { tree: typed });
        };
        let (TreeKind::Apply(source_apply), TreeKind::Apply(typed_apply)) =
            (&source_root.kind, &typed_root.kind)
        else {
            return Ok(false);
        };
        let source_function = self.source_tree(source_apply.function)?;
        let typed_function = self.plan.typed_ast.try_get(typed_apply.function).ok_or(
            PickleError::InvalidTypedTree {
                tree: typed_apply.function,
            },
        )?;
        let source_new = matches!(&source_function.kind, TreeKind::Select(select)
            if matches!(self.plan.source_ast.try_get(select.qualifier).map(|tree| &tree.kind), Some(TreeKind::New(_))));
        let typed_new = matches!(&typed_function.kind, TreeKind::Select(select)
            if matches!(self.plan.typed_ast.try_get(select.qualifier).map(|tree| &tree.kind), Some(TreeKind::New(_))));
        Ok(source_new || typed_new)
    }

    fn validate_cross_class_constructor_application(
        &mut self,
        source_id: TreeId<Untyped>,
        typed_id: TreeId<Typed>,
        method: &MethodPicklePlan,
        result_type: TypeId,
        depth: usize,
        traversal: (&mut HashSet<TreeId<Typed>>, bool),
    ) -> Result<(), PickleError> {
        let (visited, require_constructor_result_identity) = traversal;
        let depth_limit = self.limits.expression_depth_limit();
        if depth > depth_limit {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::ExpressionDepth,
                limit: depth_limit,
            });
        }
        self.charge(4)?;
        if !visited.insert(typed_id) {
            return Err(PickleError::RepeatedExpression { tree: typed_id });
        }
        let p = self.plan;
        let source_root = self.source_tree(source_id)?;
        let typed_root = p
            .typed_ast
            .try_get(typed_id)
            .ok_or(PickleError::InvalidTypedTree { tree: typed_id })?;
        self.source_typed_binding(source_id, typed_id)?;
        self.root_source_typed_span(source_root.position, typed_root.position, typed_id)?;
        let (TreeKind::Apply(source_apply), TreeKind::Apply(typed_apply)) =
            (&source_root.kind, &typed_root.kind)
        else {
            return Err(PickleError::SourceShape {
                tree: source_id,
                expected: SourceRole::Body,
            });
        };
        if source_apply.kind != ApplyKind::Regular
            || typed_apply.kind != ApplyKind::Regular
            || source_apply.args.len() != typed_apply.args.len()
            || !matches!(source_apply.args.len(), 1 | 2)
        {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        }
        let source_function = self.source_tree(source_apply.function)?;
        let typed_function =
            p.typed_ast
                .try_get(typed_apply.function)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: typed_apply.function,
                })?;
        self.charge(2)?;
        if !visited.insert(typed_apply.function) {
            return Err(PickleError::RepeatedExpression {
                tree: typed_apply.function,
            });
        }
        self.source_typed_binding(source_apply.function, typed_apply.function)?;
        self.paired_source_typed_span(
            source_function.position,
            typed_function.position,
            typed_apply.function,
        )?;
        let (TreeKind::Select(source_select), TreeKind::Select(typed_select)) =
            (&source_function.kind, &typed_function.kind)
        else {
            return Err(PickleError::SourceShape {
                tree: source_apply.function,
                expected: SourceRole::Body,
            });
        };
        let source_new = self.source_tree(source_select.qualifier)?;
        let typed_new =
            p.typed_ast
                .try_get(typed_select.qualifier)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: typed_select.qualifier,
                })?;
        self.charge(1)?;
        if !visited.insert(typed_select.qualifier) {
            return Err(PickleError::RepeatedExpression {
                tree: typed_select.qualifier,
            });
        }
        self.source_typed_binding(source_select.qualifier, typed_select.qualifier)?;
        self.paired_source_typed_span(
            source_new.position,
            typed_new.position,
            typed_select.qualifier,
        )?;
        let typed_new_result_type = typed_new.ty;
        let (TreeKind::New(source_new), TreeKind::New(typed_new)) =
            (&source_new.kind, &typed_new.kind)
        else {
            return Err(PickleError::SourceShape {
                tree: source_select.qualifier,
                expected: SourceRole::Body,
            });
        };
        let source_type = self.source_tree(source_new.tpt)?;
        let typed_type =
            p.typed_ast
                .try_get(typed_new.tpt)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: typed_new.tpt,
                })?;
        self.source_typed_binding(source_new.tpt, typed_new.tpt)?;
        self.paired_source_typed_span(source_type.position, typed_type.position, typed_new.tpt)?;
        let TreeKind::Ident(source_class_name) = &source_type.kind else {
            return Err(PickleError::SourceShape {
                tree: source_new.tpt,
                expected: SourceRole::TypeSyntax,
            });
        };
        if !matches!(typed_type.kind, TreeKind::TypeTree(_)) {
            return Err(PickleError::SourceShape {
                tree: source_new.tpt,
                expected: SourceRole::TypeSyntax,
            });
        }
        let class_type = typed_type.ty;
        let Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(class),
            ..
        }) = p.store.types.try_get(class_type)
        else {
            return Err(PickleError::BodyType {
                tree: typed_new.tpt,
                ty: class_type,
            });
        };
        let class = *class;
        if !self.is_planned_sibling_class_type(result_type, Some(source_class_name.name))
            || !self.same_planned_sibling_class_type(class_type, result_type)
            || !self.same_planned_sibling_class_type(typed_new_result_type, class_type)
            || p.store.symbols.get(class).name != source_class_name.name
            || !matches!(p.store.types.try_get(typed_root.ty), Some(Type::TypeRef {
                target: TypeRefTarget::Symbol(result_class), ..
            }) if *result_class == class)
        {
            return Err(PickleError::BodyType {
                tree: typed_id,
                ty: typed_root.ty,
            });
        }

        let Type::TermRef {
            prefix: selected_prefix,
            target: TermRefTarget::Symbol(constructor),
        } = self.ty(typed_function.ty)?
        else {
            return Err(PickleError::BodyType {
                tree: typed_apply.function,
                ty: typed_function.ty,
            });
        };
        let constructor = *constructor;
        let argument_count = source_apply.args.len();
        self.validate_planned_primary_constructor(class, constructor, argument_count)?;
        let constructor_signature_type = self.complete(constructor)?;
        let Type::Method(constructor_signature) = self.ty(constructor_signature_type)? else {
            return Err(PickleError::MalformedType {
                ty: constructor_signature_type,
                role: TypeRole::ConstructorSignature,
            });
        };
        let constructor_result_type = constructor_signature.result;
        if require_constructor_result_identity
            && (typed_root.ty != constructor_result_type
                || !matches!(self.ty(constructor_result_type)?, Type::TypeRef {
                    prefix,
                    target: TypeRefTarget::Symbol(result_class),
                } if *prefix == p.definitions.no_prefix && *result_class == class))
        {
            return Err(PickleError::BodyType {
                tree: typed_id,
                ty: typed_root.ty,
            });
        }
        if constructor_signature.kind != MethodKind::Plain
            || constructor_signature.params.len() != argument_count
            || constructor_signature
                .params
                .iter()
                .any(|parameter| parameter.erased || parameter.varargs)
        {
            return Err(PickleError::MalformedType {
                ty: constructor_signature_type,
                role: TypeRole::ConstructorSignature,
            });
        }
        if source_select.name.text() != self.symbol(constructor)?.name.text()
            || p.store.names.resolve(source_select.name.text()) != "<init>"
            || typed_select.name != source_select.name
            || !matches!(self.ty(*selected_prefix)?, Type::TypeRef {
                target: TypeRefTarget::Symbol(selected_class), ..
            } if *selected_class == class)
            || !self.same_planned_sibling_class_type(*selected_prefix, class_type)
        {
            return Err(PickleError::Reference {
                symbol: constructor,
                reason: crate::ReferenceError::Owner,
            });
        }

        if method.parameter_clauses.len() != 1
            || method.parameter_clauses[0].len() != argument_count
        {
            return Err(self.unsupported(UnsupportedFeature::ParameterClauses, Some(method.symbol)));
        }
        if argument_count == 1
            && p.typed_bindings
                .nested_constructor_call_pair(p.source.id, method.symbol)
                .is_some()
        {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        }
        if argument_count == 2 {
            let Some(pair_binding) = p
                .typed_bindings
                .nested_constructor_call_pair(p.source.id, method.symbol)
            else {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                );
            };
            if p.typed_bindings
                .nested_constructor_call(p.source.id, method.symbol)
                .is_some()
            {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                );
            }
            for (index, ((binding, argument_formal), (source_argument_id, typed_argument_id))) in
                pair_binding
                    .calls
                    .into_iter()
                    .zip(pair_binding.argument_formals)
                    .zip(
                        source_apply
                            .args
                            .iter()
                            .copied()
                            .zip(typed_apply.args.iter().copied()),
                    )
                    .enumerate()
            {
                let formal = method.parameter_clauses[0][index];
                let formal_record = self.symbol(formal)?;
                let formal_type = match formal_record.info {
                    SymbolInfo::Complete(ty) => ty,
                    _ => {
                        return Err(PickleError::MalformedType {
                            ty: self.complete(method.symbol)?,
                            role: TypeRole::Parameter,
                        });
                    }
                };
                if formal_record.kind != SymbolKind::Parameter
                    || formal_record.owner != Some(method.symbol)
                    || formal_type != p.definitions.int
                {
                    return Err(PickleError::Reference {
                        symbol: formal,
                        reason: crate::ReferenceError::Owner,
                    });
                }
                if binding.source_apply != source_argument_id
                    || binding.typed_apply != typed_argument_id
                {
                    return Err(
                        self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                    );
                }
                let source_argument = self.source_tree(source_argument_id)?;
                let typed_argument = p.typed_ast.try_get(typed_argument_id).ok_or(
                    PickleError::InvalidTypedTree {
                        tree: typed_argument_id,
                    },
                )?;
                self.charge(1)?;
                if !visited.insert(typed_argument_id) {
                    return Err(PickleError::RepeatedExpression {
                        tree: typed_argument_id,
                    });
                }
                self.source_typed_binding(source_argument_id, typed_argument_id)?;
                self.paired_source_typed_span(
                    source_argument.position,
                    typed_argument.position,
                    typed_argument_id,
                )?;
                self.validate_nested_constructor_argument(
                    binding,
                    argument_formal,
                    constructor_signature.params[index].ty,
                    method,
                    depth + 1,
                    visited,
                )?;
            }
            return Ok(());
        }
        let constructor_argument_type = constructor_signature.params[0].ty;
        let formal = method.parameter_clauses[0][0];
        let source_argument_id = source_apply.args[0];
        let typed_argument_id = typed_apply.args[0];
        let source_argument = self.source_tree(source_argument_id)?;
        let typed_argument =
            p.typed_ast
                .try_get(typed_argument_id)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: typed_argument_id,
                })?;
        self.charge(1)?;
        if !visited.insert(typed_argument_id) {
            return Err(PickleError::RepeatedExpression {
                tree: typed_argument_id,
            });
        }
        self.source_typed_binding(source_argument_id, typed_argument_id)?;
        self.paired_source_typed_span(
            source_argument.position,
            typed_argument.position,
            typed_argument_id,
        )?;
        let typed_argument_type = typed_argument.ty;
        if let Some(binding) = p
            .typed_bindings
            .nested_constructor_call(p.source.id, method.symbol)
        {
            if binding.source_apply != source_argument_id
                || binding.typed_apply != typed_argument_id
                || !matches!(self.ty(constructor_argument_type)?, Type::TypeRef { target: TypeRefTarget::Symbol(class), .. } if *class == binding.class)
                || !matches!(self.ty(binding.result_type)?, Type::TypeRef { target: TypeRefTarget::Symbol(class), .. } if *class == binding.class)
                || !matches!(self.ty(typed_argument_type)?, Type::TypeRef { target: TypeRefTarget::Symbol(class), .. } if *class == binding.class)
                || !self.is_planned_sibling_class_type(constructor_argument_type, None)
            {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                );
            }
            return self.validate_nested_constructor_argument(
                binding,
                formal,
                constructor_argument_type,
                method,
                depth + 1,
                visited,
            );
        }
        if let Some(binding) = p
            .typed_bindings
            .planned_call(p.source.id, method.symbol)
            .filter(|binding| binding.source_apply == source_argument_id)
        {
            if binding.typed_apply != typed_argument_id || binding.caller_formal != formal {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                );
            }
            return self.validate_planned_call_argument(
                binding,
                constructor_argument_type,
                method,
                depth + 1,
                visited,
            );
        }
        let (TreeKind::Ident(source_argument), TreeKind::Ident(typed_argument)) =
            (&source_argument.kind, &typed_argument.kind)
        else {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        };
        let formal_record = self.symbol(formal)?;
        let formal_type_matches = matches!(formal_record.info,
            SymbolInfo::Complete(argument_type)
                if (constructor_argument_type == p.definitions.int
                    && argument_type == p.definitions.int)
                    || self.same_planned_sibling_class_type(argument_type, constructor_argument_type));
        let typed_target = match self.ty(typed_argument_type)? {
            Type::TermRef {
                prefix,
                target: TermRefTarget::Symbol(target),
            } if *prefix == p.definitions.no_prefix => Some(*target),
            _ => None,
        };
        let argument_identity_matches = match typed_target {
            Some(target) if target == formal => {
                formal_record.kind == SymbolKind::Parameter
                    && formal_record.owner == Some(method.symbol)
                    && formal_record.name.text() == source_argument.name.text()
            }
            Some(target) if self.registered_local_symbols.contains(&target) => {
                let local = self.symbol(target)?;
                local.kind == SymbolKind::Local
                    && local.owner == Some(method.symbol)
                    && local.name == source_argument.name
                    && self.visible_locals.contains(&target)
                    && self.same_planned_sibling_class_type(
                        self.complete(target)?,
                        constructor_argument_type,
                    )
            }
            _ => false,
        };
        let argument_type_matches = match typed_target {
            Some(target) if self.registered_local_symbols.contains(&target) => self
                .same_planned_sibling_class_type(self.complete(target)?, constructor_argument_type),
            _ => formal_type_matches,
        };
        if formal_record.kind != SymbolKind::Parameter
            || formal_record.owner != Some(method.symbol)
            || typed_argument.name != source_argument.name
            || !argument_type_matches
            || !argument_identity_matches
            || !matches!(self.ty(typed_root.ty)?, Type::TypeRef {
                target: TypeRefTarget::Symbol(target), ..
            } if *target == class)
        {
            return Err(PickleError::Reference {
                symbol: formal,
                reason: crate::ReferenceError::Owner,
            });
        }
        Ok(())
    }

    fn validate_planned_call_argument(
        &mut self,
        binding: crate::PlannedCallBinding,
        expected_type: TypeId,
        caller: &MethodPicklePlan,
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
        let source_apply = self.source_tree(binding.source_apply)?;
        let typed_apply =
            p.typed_ast
                .try_get(binding.typed_apply)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: binding.typed_apply,
                })?;
        let (TreeKind::Apply(source_call), TreeKind::Apply(typed_call)) =
            (&source_apply.kind, &typed_apply.kind)
        else {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(caller.symbol)));
        };
        if source_call.kind != ApplyKind::Regular
            || typed_call.kind != ApplyKind::Regular
            || source_call.function != binding.source_function
            || typed_call.function != binding.typed_function
            || source_call.args.as_slice() != [binding.source_argument]
            || typed_call.args.as_slice() != [binding.typed_argument]
            || typed_apply.ty != binding.result_type
        {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(caller.symbol)));
        }
        self.source_typed_binding(binding.source_apply, binding.typed_apply)?;
        self.paired_source_typed_span(
            source_apply.position,
            typed_apply.position,
            binding.typed_apply,
        )?;
        let source_function = self.source_tree(binding.source_function)?;
        let typed_function =
            p.typed_ast
                .try_get(binding.typed_function)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: binding.typed_function,
                })?;
        self.source_typed_binding(binding.source_function, binding.typed_function)?;
        self.paired_source_typed_span(
            source_function.position,
            typed_function.position,
            binding.typed_function,
        )?;
        self.charge(1)?;
        if !visited.insert(binding.typed_function) {
            return Err(PickleError::RepeatedExpression {
                tree: binding.typed_function,
            });
        }
        let (source_name, source_backquoted) = match &source_function.kind {
            TreeKind::Ident(identifier) if binding.source_receiver.is_none() => {
                if self.unqualified_name_is_shadowed(identifier.name, caller) {
                    return Err(
                        self.unsupported(UnsupportedFeature::BodyExpression, Some(caller.symbol))
                    );
                }
                (identifier.name, identifier.backquoted)
            }
            TreeKind::Select(selection) if binding.source_receiver == Some(selection.qualifier) => {
                let receiver = self.source_tree(selection.qualifier)?;
                let typed_selection = match &typed_function.kind {
                    TreeKind::Select(selection)
                        if binding.typed_receiver == Some(selection.qualifier) =>
                    {
                        selection
                    }
                    _ => {
                        return Err(self
                            .unsupported(UnsupportedFeature::BodyExpression, Some(caller.symbol)));
                    }
                };
                let typed_receiver = p.typed_ast.try_get(typed_selection.qualifier).ok_or(
                    PickleError::InvalidTypedTree {
                        tree: typed_selection.qualifier,
                    },
                )?;
                if !matches!(receiver.kind, TreeKind::This(_))
                    || !matches!(typed_receiver.kind, TreeKind::This(_))
                    || !matches!(self.ty(typed_receiver.ty)?, Type::ThisType { class } if *class == binding.owner)
                {
                    return Err(
                        self.unsupported(UnsupportedFeature::BodyExpression, Some(caller.symbol))
                    );
                }
                self.source_typed_binding(selection.qualifier, typed_selection.qualifier)?;
                self.charge(1)?;
                if !visited.insert(typed_selection.qualifier) {
                    return Err(PickleError::RepeatedExpression {
                        tree: typed_selection.qualifier,
                    });
                }
                self.paired_source_typed_span(
                    receiver.position,
                    typed_receiver.position,
                    typed_selection.qualifier,
                )?;
                (selection.name, selection.backquoted)
            }
            _ => {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(caller.symbol))
                );
            }
        };
        let (typed_name, typed_backquoted) = match &typed_function.kind {
            TreeKind::Ident(identifier) if binding.typed_receiver.is_none() => {
                (identifier.name, identifier.backquoted)
            }
            TreeKind::Select(selection) if binding.typed_receiver == Some(selection.qualifier) => {
                (selection.name, selection.backquoted)
            }
            _ => {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(caller.symbol))
                );
            }
        };
        if source_name != typed_name || source_backquoted != typed_backquoted {
            return Err(PickleError::SourceShape {
                tree: binding.source_function,
                expected: SourceRole::Body,
            });
        }
        let Type::TermRef {
            prefix: selected_prefix,
            target: TermRefTarget::Symbol(selected),
        } = self.ty(typed_function.ty)?
        else {
            return Err(PickleError::BodyType {
                tree: binding.typed_function,
                ty: typed_function.ty,
            });
        };
        let caller_owner = self
            .symbol(caller.symbol)?
            .owner
            .ok_or(PickleError::InvalidSymbol {
                symbol: caller.symbol,
            })?;
        let helper = self.symbol(binding.method)?;
        let definition = self.source_tree(binding.helper_source_tree)?;
        let TreeKind::DefDef(helper_definition) = &definition.kind else {
            return Err(PickleError::SourceShape {
                tree: binding.helper_source_tree,
                expected: SourceRole::Method,
            });
        };
        let source = p.source.id;
        let helper_plan = p.methods.iter().find(|plan| plan.symbol == binding.method);
        let Some(helper_plan) = helper_plan else {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(caller.symbol)));
        };
        if binding.owner != caller_owner
            || helper.owner != Some(binding.owner)
            || helper.kind != SymbolKind::Method
            || helper.origin != SymbolOrigin::Source(source)
            || helper.visibility != Visibility::Public
            || !helper.flags.is_empty()
            || !helper.annotations.is_empty()
            || helper.name != *helper_definition.name.as_name()
            || helper.name != source_name
            || p.source_index.definition_of(binding.method)
                != Some(SourceDefinition::Canonical {
                    source,
                    tree: binding.helper_source_tree,
                })
            || p.source_index.symbol_at(source, binding.helper_source_tree) != Some(binding.method)
            || !helper_definition.type_params.is_empty()
            || helper_definition.rhs != Some(binding.helper_source_rhs)
            || helper_plan.source_tree != binding.helper_source_tree
            || helper_plan.body != Some(binding.helper_typed_rhs)
            || helper_plan.parameter_clauses.as_slice() != [vec![binding.helper_formal]]
            || !p
                .typed_bindings
                .planned_method_symbols()
                .contains(&binding.method)
            || *selected != binding.method
            || !matches!(self.ty(*selected_prefix)?, Type::ThisType { class } if *class == binding.owner)
        {
            return Err(PickleError::Reference {
                symbol: binding.method,
                reason: crate::ReferenceError::Owner,
            });
        }
        self.source_typed_binding(binding.helper_source_rhs, binding.helper_typed_rhs)?;
        let helper_rhs = self.source_tree(binding.helper_source_rhs)?;
        let typed_helper_rhs =
            p.typed_ast
                .try_get(binding.helper_typed_rhs)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: binding.helper_typed_rhs,
                })?;
        self.paired_source_typed_span(
            helper_rhs.position,
            typed_helper_rhs.position,
            binding.helper_typed_rhs,
        )?;

        let caller_parameter = self.symbol(binding.caller_formal)?;
        let caller_argument_source = self.source_tree(binding.source_argument)?;
        let caller_argument_typed =
            p.typed_ast
                .try_get(binding.typed_argument)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: binding.typed_argument,
                })?;
        let (TreeKind::Ident(source_argument), TreeKind::Ident(typed_argument)) =
            (&caller_argument_source.kind, &caller_argument_typed.kind)
        else {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(caller.symbol)));
        };
        self.source_typed_binding(binding.source_argument, binding.typed_argument)?;
        self.paired_source_typed_span(
            caller_argument_source.position,
            caller_argument_typed.position,
            binding.typed_argument,
        )?;
        if !visited.insert(binding.typed_argument) {
            return Err(PickleError::RepeatedExpression {
                tree: binding.typed_argument,
            });
        }
        self.charge(1)?;
        let caller_parameters = caller.parameter_clauses.iter().flatten().copied();
        let helper_parameters = helper_plan.parameter_clauses.iter().flatten().copied();
        if caller_parameter.kind != SymbolKind::Parameter
            || caller_parameter.owner != Some(caller.symbol)
            || caller_parameter.name.text() != source_argument.name.text()
            || typed_argument.name != source_argument.name
            || caller_parameter.info != SymbolInfo::Complete(binding.formal_type)
            || !caller_parameters
                .into_iter()
                .any(|symbol| symbol == binding.caller_formal)
            || !helper_parameters
                .into_iter()
                .any(|symbol| symbol == binding.helper_formal)
            || !matches!(self.ty(caller_argument_typed.ty)?, Type::TermRef {
                prefix,
                target: TermRefTarget::Symbol(target),
            } if *prefix == p.definitions.no_prefix && *target == binding.caller_formal)
        {
            return Err(PickleError::Reference {
                symbol: binding.caller_formal,
                reason: crate::ReferenceError::Owner,
            });
        }
        let Type::TypeRef {
            target: TypeRefTarget::Symbol(result_class),
            ..
        } = self.ty(binding.result_type)?
        else {
            return Err(PickleError::BodyType {
                tree: binding.typed_apply,
                ty: binding.result_type,
            });
        };
        let Type::TypeRef {
            target: TypeRefTarget::Symbol(expected_class),
            ..
        } = self.ty(expected_type)?
        else {
            return Err(PickleError::BodyType {
                tree: binding.typed_apply,
                ty: expected_type,
            });
        };
        if result_class != expected_class
            || !self.is_planned_sibling_class_type(binding.result_type, None)
            || !self.is_planned_sibling_class_type(expected_type, None)
        {
            return Err(PickleError::BodyType {
                tree: binding.typed_apply,
                ty: binding.result_type,
            });
        }
        let helper_signature_type = self.complete(binding.method)?;
        let Type::Method(helper_signature) = self.ty(helper_signature_type)? else {
            return Err(PickleError::MalformedType {
                ty: helper_signature_type,
                role: TypeRole::Parameter,
            });
        };
        if helper_signature.kind != MethodKind::Plain
            || helper_signature.params.len() != 1
            || helper_signature.params[0].erased
            || helper_signature.params[0].varargs
            || binding.formal_type != p.definitions.int
            || helper_signature.params[0].ty != binding.formal_type
            || helper_signature.result != binding.result_type
        {
            return Err(PickleError::MalformedType {
                ty: helper_signature_type,
                role: TypeRole::Parameter,
            });
        }
        let helper_formal = self.symbol(binding.helper_formal)?;
        if helper_formal.kind != SymbolKind::Parameter
            || helper_formal.owner != Some(binding.method)
            || helper_formal.info != SymbolInfo::Complete(binding.formal_type)
        {
            return Err(PickleError::Reference {
                symbol: binding.helper_formal,
                reason: crate::ReferenceError::Owner,
            });
        }
        Ok(())
    }

    fn validate_nested_constructor_argument(
        &mut self,
        binding: crate::NestedConstructorCallBinding,
        caller_formal: SymbolId,
        expected_type: TypeId,
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
        let source_apply = self.source_tree(binding.source_apply)?;
        let typed_apply =
            p.typed_ast
                .try_get(binding.typed_apply)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: binding.typed_apply,
                })?;
        let (TreeKind::Apply(source_apply_node), TreeKind::Apply(typed_apply_node)) =
            (&source_apply.kind, &typed_apply.kind)
        else {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        };
        if source_apply_node.kind != ApplyKind::Regular
            || typed_apply_node.kind != ApplyKind::Regular
            || source_apply_node.function != binding.source_select
            || typed_apply_node.function != binding.typed_select
            || source_apply_node.args.as_slice() != [binding.source_argument]
            || typed_apply_node.args.as_slice() != [binding.typed_argument]
            || typed_apply.ty != binding.result_type
            || !matches!(self.ty(typed_apply.ty)?, Type::TypeRef { target: TypeRefTarget::Symbol(class), .. } if *class == binding.class)
            || !matches!(self.ty(expected_type)?, Type::TypeRef { target: TypeRefTarget::Symbol(class), .. } if *class == binding.class)
        {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        }
        let source_select = self.source_tree(binding.source_select)?;
        let typed_select =
            p.typed_ast
                .try_get(binding.typed_select)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: binding.typed_select,
                })?;
        let (TreeKind::Select(source_select_node), TreeKind::Select(typed_select_node)) =
            (&source_select.kind, &typed_select.kind)
        else {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        };
        if source_select_node.qualifier != binding.source_new
            || typed_select_node.qualifier != binding.typed_new
            || p.store.names.resolve(source_select_node.name.text()) != "<init>"
            || typed_select_node.name != source_select_node.name
        {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        }
        let source_new = self.source_tree(binding.source_new)?;
        let typed_new =
            p.typed_ast
                .try_get(binding.typed_new)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: binding.typed_new,
                })?;
        let (TreeKind::New(source_new_node), TreeKind::New(typed_new_node)) =
            (&source_new.kind, &typed_new.kind)
        else {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        };
        let source_type = self.source_tree(source_new_node.tpt)?;
        let typed_type =
            p.typed_ast
                .try_get(typed_new_node.tpt)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: typed_new_node.tpt,
                })?;
        let TreeKind::Ident(source_class_name) = &source_type.kind else {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        };
        let class_type = typed_type.ty;
        if !matches!(typed_type.kind, TreeKind::TypeTree(_))
            || !self.is_planned_sibling_class_type(class_type, Some(source_class_name.name))
            || !matches!(self.ty(class_type)?, Type::TypeRef { target: TypeRefTarget::Symbol(class), .. } if *class == binding.class)
            || self.symbol(binding.class)?.name != source_class_name.name
            || !matches!(self.ty(typed_new.ty)?, Type::TypeRef { target: TypeRefTarget::Symbol(class), .. } if *class == binding.class)
            || !matches!(self.ty(binding.result_type)?, Type::TypeRef { target: TypeRefTarget::Symbol(class), .. } if *class == binding.class)
        {
            return Err(PickleError::BodyType {
                tree: binding.typed_new,
                ty: typed_new.ty,
            });
        }
        let Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(constructor),
        } = self.ty(typed_select.ty)?
        else {
            return Err(PickleError::BodyType {
                tree: binding.typed_select,
                ty: typed_select.ty,
            });
        };
        if *constructor != binding.constructor
            || !matches!(self.ty(*prefix)?, Type::TypeRef { target: TypeRefTarget::Symbol(class), .. } if *class == binding.class)
        {
            return Err(PickleError::Reference {
                symbol: binding.constructor,
                reason: crate::ReferenceError::Owner,
            });
        }
        self.validate_planned_primary_constructor(binding.class, binding.constructor, 1)?;
        let constructor_type = self.complete(binding.constructor)?;
        let Type::Method(signature) = self.ty(constructor_type)? else {
            return Err(PickleError::MalformedType {
                ty: constructor_type,
                role: TypeRole::ConstructorSignature,
            });
        };
        let constructor_source = self
            .plan
            .source_index
            .definition_of(binding.constructor)
            .and_then(|definition| match definition {
                SourceDefinition::Canonical { source, tree } if source == p.source.id => Some(tree),
                _ => None,
            })
            .ok_or(PickleError::SourceDefinition {
                symbol: binding.constructor,
            })?;
        let TreeKind::DefDef(constructor_definition) = &self.source_tree(constructor_source)?.kind
        else {
            return Err(PickleError::SourceShape {
                tree: constructor_source,
                expected: SourceRole::Constructor,
            });
        };
        let Some(&constructor_formal_tree) = constructor_definition
            .value_param_clauses
            .first()
            .and_then(|clause| clause.first())
        else {
            return Err(self.unsupported(
                UnsupportedFeature::PrimaryConstructorInput,
                Some(binding.constructor),
            ));
        };
        let canonical_formal = p
            .source_index
            .derived_symbol_at(binding.constructor, p.source.id, constructor_formal_tree)
            .ok_or(PickleError::SourceDefinition {
                symbol: binding.constructor,
            })?;
        if canonical_formal != binding.formal
            || signature.kind != MethodKind::Plain
            || signature.params.len() != 1
            || signature.params[0].ty != binding.formal_type
            || binding.formal_type != p.definitions.int
            || !matches!(self.ty(signature.result)?, Type::TypeRef { target: TypeRefTarget::Symbol(class), .. } if *class == binding.class)
            || self.complete(caller_formal)? != binding.formal_type
        {
            return Err(PickleError::MalformedType {
                ty: constructor_type,
                role: TypeRole::ConstructorSignature,
            });
        }
        let caller_formal_record = self.symbol(caller_formal)?;
        if caller_formal_record.kind != SymbolKind::Parameter
            || caller_formal_record.owner != Some(method.symbol)
            || !method
                .parameter_clauses
                .iter()
                .any(|clause| clause.contains(&caller_formal))
        {
            return Err(PickleError::Reference {
                symbol: caller_formal,
                reason: crate::ReferenceError::Owner,
            });
        }
        let source_argument = self.source_tree(binding.source_argument)?;
        let typed_argument =
            p.typed_ast
                .try_get(binding.typed_argument)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: binding.typed_argument,
                })?;
        let (TreeKind::Ident(source_ident), TreeKind::Ident(typed_ident)) =
            (&source_argument.kind, &typed_argument.kind)
        else {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        };
        if !matches!(self.ty(typed_argument.ty)?, Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) } if *prefix == p.definitions.no_prefix && *symbol == caller_formal)
            || typed_ident.name != source_ident.name
            || self.symbol(caller_formal)?.name != source_ident.name
        {
            return Err(PickleError::Reference {
                symbol: caller_formal,
                reason: crate::ReferenceError::Owner,
            });
        }
        for (source, typed) in [
            (binding.source_select, binding.typed_select),
            (binding.source_new, binding.typed_new),
            (source_new_node.tpt, typed_new_node.tpt),
            (binding.source_argument, binding.typed_argument),
        ] {
            self.source_typed_binding(source, typed)?;
            let source_node = self.source_tree(source)?;
            let typed_node = p
                .typed_ast
                .try_get(typed)
                .ok_or(PickleError::InvalidTypedTree { tree: typed })?;
            self.paired_source_typed_span(source_node.position, typed_node.position, typed)?;
            if !visited.insert(typed) {
                return Err(PickleError::RepeatedExpression { tree: typed });
            }
            self.charge(1)?;
        }
        Ok(())
    }

    fn validate_planned_primary_constructor(
        &self,
        class: SymbolId,
        constructor: SymbolId,
        parameter_count: usize,
    ) -> Result<(), PickleError> {
        let p = self.plan;
        let class_definition = p
            .source_index
            .definition_of(class)
            .ok_or(PickleError::SourceDefinition { symbol: class })?;
        let SourceDefinition::Canonical {
            source,
            tree: class_tree,
        } = class_definition
        else {
            return Err(PickleError::SourceDefinition { symbol: class });
        };
        if source != p.source.id || !p.typed_bindings.planned_class_symbols().contains(&class) {
            return Err(PickleError::SourceDefinition { symbol: class });
        }
        let TreeKind::TypeDef(class_definition) = &self.source_tree(class_tree)?.kind else {
            return Err(PickleError::SourceShape {
                tree: class_tree,
                expected: SourceRole::Class,
            });
        };
        let TreeKind::Template(template) = &self.source_tree(class_definition.rhs)?.kind else {
            return Err(PickleError::SourceShape {
                tree: class_definition.rhs,
                expected: SourceRole::Class,
            });
        };
        let constructor_tree = template.constructor;
        if p.source_index.symbol_at(p.source.id, constructor_tree) != Some(constructor)
            || p.source_index.definition_of(constructor)
                != Some(SourceDefinition::Canonical {
                    source: p.source.id,
                    tree: constructor_tree,
                })
            || template.body.iter().any(|tree| {
                p.source_ast.try_get(*tree).is_some_and(|node| {
                    matches!(
                        &node.kind,
                        TreeKind::DefDef(definition)
                            if p.store.names.resolve(definition.name.as_name().text()) == "<init>"
                    )
                })
            })
        {
            return Err(PickleError::SourceDefinition {
                symbol: constructor,
            });
        }
        let TreeKind::DefDef(definition) = &self.source_tree(constructor_tree)?.kind else {
            return Err(PickleError::SourceShape {
                tree: constructor_tree,
                expected: SourceRole::Constructor,
            });
        };
        if !definition.type_params.is_empty()
            || definition.value_param_clauses.len() != 1
            || definition.value_param_clauses[0].len() != parameter_count
            || definition
                .source_param_clause_order
                .as_ref()
                .is_some_and(|order| {
                    order.as_slice() != [dotty_core::ast::DefParamClauseOrder::ValueParams(0)]
                })
        {
            return Err(self.unsupported(
                UnsupportedFeature::PrimaryConstructorInput,
                Some(constructor),
            ));
        }
        let constructor_record = self.symbol(constructor)?;
        let class_record = self.symbol(class)?;
        if class_record.kind != SymbolKind::Class
            || class_record.owner != self.symbol(p.class)?.owner
            || class_record.origin != SymbolOrigin::Source(p.source.id)
            || class_record.visibility != Visibility::Public
            || !class_record.flags.is_empty()
            || !class_record.annotations.is_empty()
            || constructor_record.kind != SymbolKind::Constructor
            || constructor_record.owner != Some(class)
            || constructor_record.origin != SymbolOrigin::Source(p.source.id)
            || constructor_record.visibility != Visibility::Public
            || !constructor_record.flags.is_empty()
            || !constructor_record.annotations.is_empty()
        {
            return Err(PickleError::Reference {
                symbol: constructor,
                reason: crate::ReferenceError::Metadata,
            });
        }
        let class_type = self.complete(class)?;
        let Type::ClassInfo(info) = self.ty(class_type)? else {
            return Err(PickleError::MalformedType {
                ty: class_type,
                role: TypeRole::ClassInfo,
            });
        };
        if info.class != class
            || info.prefix != p.definitions.no_prefix
            || info.parents.as_slice() != [p.definitions.object_type]
            || info.self_type.is_some()
            || p.store
                .scopes
                .get(info.declarations)
                .entered_symbols()
                .any(|symbol| {
                    p.store.symbols.contains(symbol)
                        && p.store.symbols.get(symbol).kind == SymbolKind::TypeParameter
                })
        {
            return Err(PickleError::UnsupportedTypeReference { ty: class_type });
        }
        let constructor_formal_trees = &definition.value_param_clauses[0];
        self.validate_constructor_signature_and_formals(
            class,
            constructor,
            constructor_formal_trees,
        )
    }

    fn validate_constructor_signature_and_formals(
        &self,
        class: SymbolId,
        constructor: SymbolId,
        formal_trees: &[TreeId<Untyped>],
    ) -> Result<(), PickleError> {
        let p = self.plan;
        let mut formal_data = Vec::with_capacity(formal_trees.len());
        for &formal_tree in formal_trees {
            let TreeKind::ValDef(formal) = &self.source_tree(formal_tree)?.kind else {
                return Err(PickleError::SourceShape {
                    tree: formal_tree,
                    expected: SourceRole::Parameter,
                });
            };
            let TreeKind::Ident(source_type) = &self.source_tree(formal.tpt)?.kind else {
                return Err(PickleError::SourceShape {
                    tree: formal.tpt,
                    expected: SourceRole::TypeSyntax,
                });
            };
            let formal_symbol = p
                .source_index
                .derived_symbol_at(constructor, p.source.id, formal_tree)
                .ok_or(PickleError::SourceDefinition {
                    symbol: constructor,
                })?;
            let formal_record = self.symbol(formal_symbol)?;
            let SymbolInfo::Complete(formal_type) = formal_record.info else {
                return Err(PickleError::IncompleteSymbol {
                    symbol: formal_symbol,
                    info: formal_record.info,
                });
            };
            let is_int_formal = formal_type == p.definitions.int
                && p.store.names.resolve(source_type.name.text()) == "Int";
            let reference_formal = matches!(
                self.ty(formal_type)?,
                Type::TypeRef {
                    prefix,
                    target: TypeRefTarget::Symbol(formal_class),
                } if *formal_class != class
                    && self.is_planned_sibling_class_type(formal_type, Some(source_type.name))
                    && matches!(self.ty(*prefix)?, Type::TypeRef {
                        prefix,
                        target: TypeRefTarget::Symbol(package),
                    } if *prefix == p.definitions.no_prefix
                        && Some(*package) == self.symbol(class)?.owner)
            );
            if (!is_int_formal && !reference_formal)
                || formal.rhs.is_some()
                || formal_record.kind != SymbolKind::Parameter
                || formal_record.owner != Some(constructor)
                || formal_record.origin != SymbolOrigin::Source(p.source.id)
                || formal_record.visibility != Visibility::Public
                || !formal_record.flags.is_empty()
                || !formal_record.annotations.is_empty()
                || formal_record.name.text() != formal.name.as_name().text()
                || formal_record.info != SymbolInfo::Complete(formal_type)
            {
                return Err(PickleError::UnsupportedTypeReference { ty: formal_type });
            }
            formal_data.push((
                formal.name.as_name().text().to_owned(),
                formal_type,
                reference_formal,
            ));
        }
        if formal_data.len() == 2
            && (!formal_data.iter().all(|(_, _, is_reference)| *is_reference)
                || !self.same_planned_sibling_class_type(formal_data[0].1, formal_data[1].1))
        {
            return Err(self.unsupported(
                UnsupportedFeature::PrimaryConstructorInput,
                Some(constructor),
            ));
        }
        let signature_type = self.complete(constructor)?;
        let Type::Method(signature) = self.ty(signature_type)? else {
            return Err(PickleError::MalformedType {
                ty: signature_type,
                role: TypeRole::ConstructorSignature,
            });
        };
        if signature.kind != MethodKind::Plain
            || signature.params.len() != formal_data.len()
            || signature
                .params
                .iter()
                .zip(&formal_data)
                .any(|(parameter, (name, ty, _))| {
                    parameter.name.as_name().text() != *name
                        || parameter.ty != *ty
                        || parameter.erased
                        || parameter.varargs
                })
            || !matches!(self.ty(signature.result)?, Type::TypeRef {
                prefix,
                target: TypeRefTarget::Symbol(target),
            } if *prefix == p.definitions.no_prefix && *target == class)
        {
            return Err(PickleError::MalformedType {
                ty: signature_type,
                role: TypeRole::ConstructorSignature,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod cross_class_target_signature_tests {
    use super::*;
    use dotty_core::MethodType;

    #[test]
    fn rejects_a_selected_method_with_a_nonempty_parameter_list() {
        let mut store = dotty_core::SemanticStore::new();
        let int_type = store.types.alloc(Type::NoType);
        let signature_type = store.types.alloc(Type::NoType);
        let parameter_name = dotty_core::TermName::new(store.names.intern("value"));
        let signature = MethodType {
            params: vec![dotty_core::MethodParam {
                name: parameter_name,
                ty: int_type,
                erased: false,
                varargs: false,
            }],
            result: int_type,
            kind: MethodKind::Plain,
        };
        assert_eq!(
            validate_cross_class_target_signature(signature_type, &signature, int_type),
            Err(PickleError::MalformedType {
                ty: signature_type,
                role: TypeRole::Parameter,
            })
        );
    }

    #[test]
    fn rejects_a_selected_method_with_a_non_int_result() {
        let mut store = dotty_core::SemanticStore::new();
        let int_type = store.types.alloc(Type::NoType);
        let wrong_result = store.types.alloc(Type::NoType);
        let signature_type = store.types.alloc(Type::NoType);
        let signature = MethodType {
            params: vec![],
            result: wrong_result,
            kind: MethodKind::Plain,
        };
        assert_eq!(
            validate_cross_class_target_signature(signature_type, &signature, int_type),
            Err(PickleError::MalformedType {
                ty: signature_type,
                role: TypeRole::Parameter,
            })
        );
    }
}
