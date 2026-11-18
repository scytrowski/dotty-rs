//! Source and typed identity checks for a unary returned contextual closure.
use super::*;
use dotty_core::ast::Modifier;

impl Validator<'_, '_> {
    pub(super) fn context_result_closure(
        &mut self,
        definition: &DefDef<Untyped>,
        method: &MethodPicklePlan,
        declared: TypeId,
    ) -> Result<(), PickleError> {
        let p = self.plan;
        let invalid = || PickleError::Unsupported {
            feature: UnsupportedFeature::BodyExpression,
            symbol: Some(method.symbol),
        };
        self.charge(8)?;
        let source = definition.rhs.ok_or_else(invalid)?;
        let body = method.body.ok_or_else(invalid)?;
        self.source_typed_binding(source, body)?;
        let checked = dotty_core::checked_unary_context_closure(
            p.store,
            p.definitions,
            p.typed_ast,
            method.symbol,
            body,
            declared,
        )
        .ok_or_else(invalid)?;
        let typed = |id| {
            p.typed_ast
                .try_get(id)
                .ok_or(PickleError::InvalidTypedTree { tree: id })
        };
        let int = p.definitions.int;
        if checked.input != int
            || checked.result != int
            || p.typed_bindings
                .synthetic_definition_symbol(checked.definition)
                != Some(checked.method)
            || p.typed_bindings
                .synthetic_definition_symbol(checked.parameter_tree)
                != Some(checked.parameter)
        {
            return Err(invalid());
        }
        let TreeKind::Closure(closure) = &typed(body)?.kind else {
            return Err(invalid());
        };
        self.root_source_typed_span(
            self.source_tree(source)?.position,
            typed(body)?.position,
            body,
        )?;
        let TreeKind::PhaseSpecific(UntypedNode::Function(lambda)) =
            &self.source_tree(source)?.kind
        else {
            return Err(invalid());
        };
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(result)) =
            &self.source_tree(definition.tpt)?.kind
        else {
            return Err(invalid());
        };
        let [input] = result.params.as_slice() else {
            return Err(invalid());
        };
        let [parameter_tree] = lambda.params.as_slice() else {
            return Err(invalid());
        };
        let TreeKind::ValDef(parameter) = &self.source_tree(*parameter_tree)?.kind else {
            return Err(invalid());
        };
        let given = |mods: &Modifiers| {
            mods.modifiers == [Modifier::Given]
                && mods.annotations.is_empty()
                && mods.visibility.is_none()
        };
        if result.erased_params != [false]
            || !given(&result.modifiers)
            || !given(&parameter.metadata)
            || parameter.rhs.is_some()
            || self.symbol(checked.parameter)?.name != *parameter.name.as_name()
        {
            return Err(invalid());
        }
        let int_name = self
            .symbol(self.ty(int)?.reference_symbol().ok_or_else(invalid)?)?
            .name;
        for tree in [*input, result.result, parameter.tpt] {
            if !matches!(&self.source_tree(tree)?.kind,TreeKind::Ident(ident) if ident.name == int_name)
            {
                return Err(invalid());
            }
        }
        self.source_typed_binding(*parameter_tree, checked.parameter_tree)?;
        let TreeKind::ValDef(typed_parameter) = &typed(checked.parameter_tree)?.kind else {
            return Err(invalid());
        };
        self.source_typed_binding(parameter.tpt, typed_parameter.tpt)?;
        self.source_typed_binding(lambda.body, checked.body)?;
        // Account for each captured input by the exact reference in the body. A single
        // primitive application, a formal reference and an integer literal are bounded.
        let leaves = match &typed(checked.body)?.kind {
            TreeKind::Apply(app) => {
                if !matches!(
                    self.source_tree(lambda.body)?.kind,
                    TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
                ) {
                    return Err(invalid());
                }
                let TreeKind::Select(operator) = &typed(app.function)?.kind else {
                    return Err(invalid());
                };
                let [rhs] = app.args.as_slice() else {
                    return Err(invalid());
                };
                vec![operator.qualifier, *rhs]
            }
            _ => vec![checked.body],
        };
        let mut captures = Vec::new();
        let mut parameters = vec![checked.parameter];
        for leaf in leaves {
            let node = typed(leaf)?;
            match &node.kind {
                TreeKind::Literal(_) => {}
                TreeKind::Ident(_) => {
                    let target = self.ty(node.ty)?.reference_symbol().ok_or_else(invalid)?;
                    if target != checked.parameter && !parameters.contains(&target) {
                        if !method
                            .parameter_clauses
                            .iter()
                            .flatten()
                            .any(|&p| p == target)
                        {
                            return Err(invalid());
                        }
                        self.owner(target, Some(method.symbol))?;
                        if self.symbol(target)?.name == *parameter.name.as_name()
                            || self.complete(target)? != int
                        {
                            return Err(invalid());
                        }
                        captures.push(leaf);
                        parameters.push(target);
                    }
                }
                _ => return Err(invalid()),
            }
        }
        if closure.env != captures {
            return Err(invalid());
        }
        self.validate_method_expression(
            Some(lambda.body),
            Some(checked.body),
            &MethodPicklePlan {
                symbol: checked.method,
                source_tree: source,
                parameter_clauses: vec![parameters],
                body: Some(checked.body),
            },
            int,
        )?;
        self.context_closures.insert(method.symbol, checked);
        Ok(())
    }
}
