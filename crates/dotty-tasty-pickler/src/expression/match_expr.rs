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
        let pattern = ast
            .try_get(case.pattern)
            .ok_or(PickleError::InvalidTypedTree { tree: case.pattern })?;
        let pattern = if let TreeKind::Ident(ident) = &pattern.kind {
            budget.visit(case.pattern, depth + 2)?;
            if ident.backquoted || store.names.try_resolve(ident.name.text()) != Some("_") {
                return Err(PickleError::BodyType {
                    tree: case.pattern,
                    ty: pattern.ty,
                });
            }
            let name = context.intern_wire_text("_")?;
            let ty = context.lower_type(pattern.ty)?;
            context.wire_node(AstNode::NatAst {
                tag: dotty_tasty::tasty::IDENT_TAG,
                value: dotty_tasty::tasty::AstOperand::Nat(name),
                child: ty,
            })?
        } else {
            lower_expression_inner(
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
            )?
        };
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
        if case.guard.is_some() {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::BodyExpression,
                symbol: Some(method),
            });
        }
        let case = context.wire_node(AstNode::Length {
            tag: dotty_tasty::tasty::CASEDEF_TAG,
            fields: vec![AstField::Tree(pattern), AstField::Tree(body)],
        })?;
        fields.push(AstField::Tree(case));
    }
    context.wire_node(AstNode::Length {
        tag: dotty_tasty::tasty::MATCH_TAG,
        fields,
    })
}
