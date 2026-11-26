use super::*;

pub(super) fn emit_match(
    context: &mut JvmLoweringContext<'_>,
    tree: TreeId<Typed>,
    code: &mut CodeBuilder,
    depth: usize,
) -> Result<Flow, JvmCodegenError> {
    let node = context.tree(tree)?;
    let TreeKind::Match(matched) = &node.kind else {
        unreachable!("match shape was checked")
    };
    let matched = matched.clone();
    let expected = if is_unit_type(context.store, node.ty, context.definitions) {
        Flow::Unit
    } else {
        match primitive_type(context.store, node.ty, context.definitions) {
            Some(value @ (JvmType::Int | JvmType::Boolean)) => Flow::Value(value),
            _ => return Err(unsupported(tree, "unsupported match result type")),
        }
    };
    if matched.cases.is_empty() {
        return Err(unsupported(tree, "match requires a final wildcard"));
    }
    let mut cases = Vec::with_capacity(matched.cases.len());
    for (index, &case_id) in matched.cases.iter().enumerate() {
        context.visit(case_id, depth + 1)?;
        let case_node = context.tree(case_id)?;
        let TreeKind::CaseDef(case) = case_node.kind else {
            return Err(unsupported(case_id, "expected match case"));
        };
        if case.guard.is_some() {
            return Err(unsupported(case_id, "guarded match case"));
        }
        if case_node.ty != context.tree(case.body)?.ty {
            return Err(unsupported(case_id, "inconsistent match case type"));
        }
        context.visit(case.pattern, depth + 2)?;
        let pattern = context.tree(case.pattern)?;
        let literal = match &pattern.kind {
            TreeKind::Literal(literal) if matches!(literal.value, Constant::Int(_)) => {
                let Constant::Int(value) = literal.value else {
                    unreachable!()
                };
                if !matches!(context.store.types.try_get(pattern.ty), Some(Type::Constant(Constant::Int(typed))) if *typed == value)
                {
                    return Err(unsupported(case.pattern, "inconsistent Int match literal"));
                }
                Some(value)
            }
            TreeKind::Ident(ident)
                if !ident.backquoted
                    && ident.name.is_term()
                    && context.store.names.try_resolve(ident.name.text()) == Some("_") =>
            {
                None
            }
            _ => return Err(unsupported(case.pattern, "unsupported match pattern")),
        };
        if literal.is_none() != (index + 1 == matched.cases.len()) {
            return Err(unsupported(
                case.pattern,
                "match requires exactly one final wildcard",
            ));
        }
        if primitive_type(context.store, pattern.ty, context.definitions) != Some(JvmType::Int) {
            return Err(unsupported(case.pattern, "non-Int match pattern type"));
        }
        cases.push((case.body, literal));
    }
    if emit_expression(context, matched.selector, code, depth + 1)? != Flow::Value(JvmType::Int) {
        return Err(unsupported(matched.selector, "non-Int match selector"));
    }
    let slot = context.reserve_temporary_local_slots(tree, 1)?;
    let result = emit_cases(context, tree, &cases, expected, slot, code, depth);
    context.release_temporary_local_slots(1);
    result
}

fn emit_cases(
    context: &mut JvmLoweringContext<'_>,
    tree: TreeId<Typed>,
    cases: &[(TreeId<Typed>, Option<i32>)],
    expected: Flow,
    slot: usize,
    code: &mut CodeBuilder,
    depth: usize,
) -> Result<Flow, JvmCodegenError> {
    let build_error = |error| JvmCodegenError::CodeBuilder { tree, error };
    code.istore(slot).map_err(build_error)?;
    let join = code.new_label().map_err(build_error)?;
    let before = context.initialized_locals.clone();
    let mut joined: Option<HashSet<SymbolId>> = None;
    for &(body, literal) in cases {
        context.initialized_locals = before.clone();
        let next = if let Some(value) = literal {
            let next = code.new_label().map_err(build_error)?;
            code.iload(slot)
                .and_then(|c| c.iconst(value))
                .and_then(|c| c.if_icmpne(next.clone()))
                .map_err(build_error)?;
            Some(next)
        } else {
            None
        };
        let flow = emit_expression(context, body, code, depth + 2)?;
        validate_branch(context, body, flow)?;
        if flow != Flow::Diverges {
            if flow != expected {
                return Err(unsupported(body, "match case result mismatch"));
            }
            joined = Some(match joined {
                None => context.initialized_locals.clone(),
                Some(previous) => previous
                    .intersection(&context.initialized_locals)
                    .copied()
                    .collect(),
            });
            if next.is_some() {
                code.goto(join.clone()).map_err(build_error)?;
            }
        }
        if let Some(next) = next {
            code.mark(next).map_err(build_error)?;
        }
    }
    code.mark(join).map_err(build_error)?;
    let flow = if let Some(initialized) = joined {
        context.initialized_locals = initialized;
        expected
    } else {
        context.initialized_locals = before;
        Flow::Diverges
    };
    Ok(flow)
}

fn unsupported(tree: TreeId<Typed>, node: &'static str) -> JvmCodegenError {
    JvmCodegenError::UnsupportedTypedExpression { tree, node }
}
