use super::*;

mod block;
mod local;

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
                Some(visited),
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
        if source_operator != operator_name || selected_operator != operator_name {
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
                Some(visited),
            )?;
            return Ok(typed.ty);
        }
        self.validate_method_expression_inner(
            Some(source_id),
            Some(typed_id),
            method,
            self.plan.definitions.int,
            depth,
            Some(visited),
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
                    self.int_branch_constant(typed_if.then_branch, then_type, method.symbol)?;
                let else_join =
                    self.int_branch_constant(typed_if.else_branch, else_type, method.symbol)?;
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
            self.int_branch_constant(typed_if.then_branch, then_type, method.symbol)?;
            self.int_branch_constant(typed_if.else_branch, else_type, method.symbol)?;
        }
        Ok(branch_join)
    }

    /// Returns `Some(value)` for an Int singleton and `None` for a widened Int
    /// (including a previously validated Int formal or parameterless method reference).
    fn int_branch_constant(
        &self,
        tree: TreeId<Typed>,
        ty: TypeId,
        method: SymbolId,
    ) -> Result<Option<i32>, PickleError> {
        if ty == self.plan.definitions.int {
            return Ok(None);
        }
        match self.ty(ty)? {
            Type::Constant(Constant::Int(value)) => Ok(Some(*value)),
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
                    && self.plan.store.symbols.get(*symbol).owner == Some(method)))
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
        self.validate_method_expression_inner(rhs, body, method, result_type, 0, None)
    }

    fn validate_method_expression_inner(
        &mut self,
        rhs: Option<TreeId<Untyped>>,
        body: Option<TreeId<Typed>>,
        method: &MethodPicklePlan,
        result_type: TypeId,
        depth: usize,
        mut visited: Option<&mut HashSet<TreeId<Typed>>>,
    ) -> Result<(), PickleError> {
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
                } else if self.registered_local_symbols.contains(target) {
                    self.kind(*target, SymbolKind::Local)?;
                    self.owner(*target, Some(method.symbol))?;
                    if result_type != p.definitions.int
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
                        || self.complete(*target)? != p.definitions.int
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
                    if self.complete(*parameter)? != result_type {
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
                if result_type != p.definitions.int {
                    return Err(PickleError::BodyType {
                        tree: body,
                        ty: node.ty,
                    });
                }
                self.same_class_method_target(*target, source_select.name, *prefix, rhs)?;
                if !matches!(self.ty(typed_qualifier.ty)?, Type::ThisType { class } if *class == p.class)
                {
                    return Err(PickleError::Reference {
                        symbol: *target,
                        reason: crate::ReferenceError::Owner,
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
}
