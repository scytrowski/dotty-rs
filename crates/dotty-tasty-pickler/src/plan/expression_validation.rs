Warning: truncated output (original token count: 38995)
Total output lines: 3713

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
     …26995 tokens truncated… Err(PickleError::BodyType {
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
