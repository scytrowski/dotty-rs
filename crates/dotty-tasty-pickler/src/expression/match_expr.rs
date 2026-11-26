use super::*;

pub(super) fn lower_match(
    store: &SemanticStore,
    ast: &TypedAst,
    matched: &dotty_core::ast::Match<Typed>,
    method: SymbolId,
    parameters: &[SymbolId],
    state: ExpressionState<'_>,
    context: &mut EncodingContext<'_>,
) -> Result<AstLabel, PickleError> {
    let ExpressionState { depth, budget } = state;
    let selector = lower_expression_inner(
        store,
        ast,
        matched.selector,
        method,
        parameters,
        ExpressionState {
            depth: depth + 1,
            budget,
        },
        context,
    )?;
    let mut fields = vec![AstField::Tree(selector)];
    for &case_id in &matched.cases {
        budget.visit(case_id, depth + 1)?;
        let node = ast
            .try_get(case_id)
            .ok_or(PickleError::InvalidTypedTree { tree: case_id })?;
        let TreeKind::CaseDef(case) = node.kind else {
            return Err(PickleError::BodyType {
                tree: case_id,
                ty: node.ty,
            });
        };
        let pattern = lower_pattern(
            store,
            ast,
            case.pattern,
            method,
            parameters,
            ExpressionState {
                depth: depth + 2,
                budget,
            },
            context,
        )?;
        let body = lower_expression_inner(
            store,
            ast,
            case.body,
            method,
            parameters,
            ExpressionState {
                depth: depth + 2,
                budget,
            },
            context,
        )?;
        let mut case_fields = vec![AstField::Tree(pattern), AstField::Tree(body)];
        if let Some(guard) = case.guard {
            let guard = lower_expression_inner(
                store,
                ast,
                guard,
                method,
                parameters,
                ExpressionState {
                    depth: depth + 2,
                    budget,
                },
                context,
            )?;
            case_fields.push(AstField::Tree(guard));
        }
        let case = context.wire_node(AstNode::Length {
            tag: dotty_tasty::tasty::CASEDEF_TAG,
            fields: case_fields,
        })?;
        fields.push(AstField::Tree(case));
    }
    context.wire_node(AstNode::Length {
        tag: dotty_tasty::tasty::MATCH_TAG,
        fields,
    })
}

fn lower_pattern(
    store: &SemanticStore,
    ast: &TypedAst,
    root: TreeId<Typed>,
    method: SymbolId,
    parameters: &[SymbolId],
    state: ExpressionState<'_>,
    context: &mut EncodingContext<'_>,
) -> Result<AstLabel, PickleError> {
    let node = ast
        .try_get(root)
        .ok_or(PickleError::InvalidTypedTree { tree: root })?;
    if let TreeKind::Bind(bind) = node.kind {
        state.budget.visit(root, state.depth)?;
        let symbol = context
            .local_value_symbol(root)
            .ok_or(PickleError::BodyType {
                tree: root,
                ty: node.ty,
            })?;
        let SymbolInfo::Complete(ty) = store.symbols.info(symbol) else {
            return Err(PickleError::BodyType {
                tree: root,
                ty: node.ty,
            });
        };
        let name = context.intern_wire_text(store.names.resolve(bind.name.text()))?;
        let ty = lower_pattern_type(store, *ty, context)?;
        let body = lower_pattern(
            store,
            ast,
            bind.body,
            method,
            parameters,
            ExpressionState {
                depth: state.depth + 1,
                budget: state.budget,
            },
            context,
        )?;
        return context.bind_pattern_declaration(
            symbol,
            AstNode::Length {
                tag: dotty_tasty::tasty::BIND_TAG,
                fields: vec![
                    AstField::Nat(name),
                    AstField::Tree(ty),
                    AstField::Tree(body),
                ],
            },
        );
    }
    if let TreeKind::Ident(ident) = &node.kind {
        state.budget.visit(root, state.depth)?;
        if ident.backquoted || store.names.try_resolve(ident.name.text()) != Some("_") {
            return Err(PickleError::BodyType {
                tree: root,
                ty: node.ty,
            });
        }
        let name = context.intern_wire_text("_")?;
        let ty = lower_pattern_type(store, node.ty, context)?;
        return context.wire_node(AstNode::NatAst {
            tag: dotty_tasty::tasty::IDENT_TAG,
            value: dotty_tasty::tasty::AstOperand::Nat(name),
            child: ty,
        });
    }
    lower_expression_inner(store, ast, root, method, parameters, state, context)
}

fn lower_pattern_type(
    store: &SemanticStore,
    ty: dotty_core::TypeId,
    context: &mut EncodingContext<'_>,
) -> Result<AstLabel, PickleError> {
    if let Some(Type::Constant(Constant::Int(value))) = store.types.try_get(ty) {
        let term = SimpleTerm::new(INTCONST_TAG, TermValue::Int(*value)).map_err(|cause| {
            PickleError::Encoding {
                cause: Box::new(cause.into()),
            }
        })?;
        return context.wire_node(AstNode::Leaf(term));
    }
    context.lower_type(ty)
}
